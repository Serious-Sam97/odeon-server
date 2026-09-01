//! Cliente TMDB — filmes e séries.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use serde::Deserialize;
use tokio::sync::RwLock;

use super::Candidate;

const API: &str = "https://api.themoviedb.org/3";
const IMG: &str = "https://image.tmdb.org/t/p";

/// Metadata em pt-BR quando o TMDB tiver; o `original_title` continua no idioma
/// original, então o matcher pode comparar contra os dois.
const LANGUAGE: &str = "pt-BR";

#[derive(Clone)]
pub struct Tmdb {
    http: reqwest::Client,
    credential: Credential,
    /// O TMDB devolve gênero como id numérico. O dicionário id→nome é fixo e
    /// pequeno: busca uma vez e guarda pelo resto do processo.
    genres: Arc<RwLock<HashMap<i64, String>>>,
    /// Quantas temporadas tem cada série, por id (R72). Mesmo espírito do
    /// `genres`: o número não muda durante uma varredura, e sem cache um
    /// desempate custaria uma chamada por arquivo — 485 no caso que o pediu.
    temporadas: Arc<RwLock<HashMap<String, Option<i32>>>>,
}

#[derive(Clone)]
enum Credential {
    /// Chave v3 — vai na query string.
    ApiKey(String),
    /// Token v4 — vai no header Authorization.
    Bearer(String),
}

impl Tmdb {
    pub fn new(http: reqwest::Client, key: String) -> Self {
        // Token v4 é um JWT; chave v3 é um hex de 32 caracteres.
        let credential = if key.starts_with("eyJ") {
            Credential::Bearer(key)
        } else {
            Credential::ApiKey(key)
        };
        Self {
            http,
            credential,
            genres: Arc::new(RwLock::new(HashMap::new())),
            temporadas: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn get<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> anyhow::Result<T> {
        let mut request = self.http.get(format!("{API}{path}"));
        let mut query = query.to_vec();
        query.push(("language", LANGUAGE.to_string()));

        match &self.credential {
            Credential::ApiKey(k) => query.push(("api_key", k.clone())),
            Credential::Bearer(t) => request = request.bearer_auth(t),
        }

        let response = request
            .query(&query)
            .send()
            .await
            .with_context(|| format!("TMDB {path} não respondeu"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("TMDB {path} devolveu {status}: {}", body.trim());
        }

        response
            .json()
            .await
            .with_context(|| format!("TMDB {path} devolveu JSON inesperado"))
    }

    /// Carrega o dicionário de gêneros na primeira vez que for preciso.
    async fn genre_map(&self) -> HashMap<i64, String> {
        if let Some(cached) = {
            let guard = self.genres.read().await;
            (!guard.is_empty()).then(|| guard.clone())
        } {
            return cached;
        }

        let mut map = HashMap::new();
        for path in ["/genre/movie/list", "/genre/tv/list"] {
            match self.get::<GenreList>(path, &[]).await {
                Ok(list) => {
                    for genre in list.genres {
                        map.insert(genre.id, genre.name);
                    }
                }
                Err(e) => tracing::warn!(error = %e, path, "lista de gêneros falhou"),
            }
        }

        *self.genres.write().await = map.clone();
        map
    }

    pub async fn search_movie(
        &self,
        title: &str,
        year: Option<i32>,
    ) -> anyhow::Result<Vec<Candidate>> {
        let mut query = vec![("query", title.to_string())];
        if let Some(y) = year {
            query.push(("year", y.to_string()));
        }
        let page: SearchPage<MovieHit> = self.get("/search/movie", &query).await?;
        let genres = self.genre_map().await;
        Ok(page
            .results
            .into_iter()
            .take(8)
            .map(|hit| hit.into_candidate(&genres))
            .collect())
    }

    pub async fn search_tv(&self, title: &str, year: Option<i32>) -> anyhow::Result<Vec<Candidate>> {
        let mut query = vec![("query", title.to_string())];
        if let Some(y) = year {
            query.push(("first_air_date_year", y.to_string()));
        }
        let page: SearchPage<TvHit> = self.get("/search/tv", &query).await?;
        let genres = self.genre_map().await;
        Ok(page
            .results
            .into_iter()
            .take(8)
            .map(|hit| hit.into_candidate(&genres))
            .collect())
    }

    /// Elenco e equipe. Chamada extra, feita só quando o match é aceito —
    /// não vale gastar requisição em candidato que vai ser descartado.
    pub async fn credits(&self, kind: &str, id: &str) -> anyhow::Result<Vec<CreditPerson>> {
        // `tv` e `movie` têm o mesmo formato de resposta.
        let path = if kind == "movie" {
            format!("/movie/{id}/credits")
        } else {
            format!("/tv/{id}/credits")
        };

        let response: CreditsResponse = self.get(&path, &[]).await?;
        let mut out = Vec::new();

        // Elenco: ordenado pelo próprio TMDB por relevância; cortar no topo é
        // o que separa "elenco" de "lista telefônica".
        for (index, member) in response.cast.into_iter().take(MAX_CAST).enumerate() {
            out.push(CreditPerson {
                provider_key: format!("tmdb:person:{}", member.id),
                name: member.name,
                role: "actor".into(),
                character_name: member.character.filter(|c| !c.trim().is_empty()),
                position: index as i32,
                image_url: image(&member.profile_path, "w185"),
                known_for: member.known_for_department,
            });
        }

        // Equipe: só os cargos que alguém procura. Importar os 200 nomes de um
        // filme grande enterraria o diretor no meio da equipe de efeitos.
        for member in response.crew {
            let Some(role) = role_for_job(member.job.as_deref().unwrap_or("")) else {
                continue;
            };
            out.push(CreditPerson {
                provider_key: format!("tmdb:person:{}", member.id),
                name: member.name,
                role: role.into(),
                character_name: None,
                position: 0,
                image_url: image(&member.profile_path, "w185"),
                known_for: member.known_for_department,
            });
        }

        Ok(out)
    }

    /// Detalhe do episódio. Só faz sentido depois que a SÉRIE já foi casada —
    /// é por isso que o matcher casa a série e só então desce pro episódio.
    pub async fn episode(
        &self,
        series_id: &str,
        season: i32,
        episode: i32,
    ) -> anyhow::Result<EpisodeDetail> {
        self.get(
            &format!("/tv/{series_id}/season/{season}/episode/{episode}"),
            &[],
        )
        .await
    }

    /// A temporada INTEIRA numa chamada.
    ///
    /// Identificar uma temporada de 24 episódios um a um custa 24 requisições
    /// que devolvem, somadas, exatamente o que esta devolve sozinha. Numa pasta
    /// como Naruto Shippuden (499 arquivos) a diferença é 21 chamadas contra
    /// 499 — deixa de ser aceitável esperar.
    pub async fn season(&self, series_id: &str, season: i32) -> anyhow::Result<SeasonDetail> {
        self.get(&format!("/tv/{series_id}/season/{season}"), &[])
            .await
    }

    /// A ficha de produção de um filme.
    pub async fn producao_do_filme(&self, id: &str) -> anyhow::Result<FichaDeProducao> {
        self.get(&format!("/movie/{id}"), &[]).await
    }

    /// A saga a que um filme pertence, se pertencer a alguma (R32).
    ///
    /// **Mesmo endpoint da ficha de produção** — `belongs_to_collection` já vem
    /// na mesma resposta que `production_countries`. São dois módulos porque são
    /// dois jobs com retomadas diferentes; é uma chamada porque o TMDB manda
    /// tudo junto e pedir duas vezes seria pagar dobrado pela mesma linha.
    pub async fn saga_do_filme(&self, id: &str) -> anyhow::Result<Option<SagaDoProvider>> {
        let ficha: FichaDeSaga = self.get(&format!("/movie/{id}"), &[]).await?;
        Ok(ficha.belongs_to_collection)
    }

    /// A série pelo ID, já como `Candidate`.
    ///
    /// A busca por texto devolve `Candidate`; o escopo por pasta já SABE o id
    /// (veio da decisão humana ou dos irmãos já casados) e precisa da mesma
    /// estrutura pra reusar o `apply_candidate` — sem duplicar a lógica de
    /// coleção, artwork e tags.
    pub async fn tv_by_id(&self, id: &str) -> anyhow::Result<Candidate> {
        let hit: TvHit = self.get(&format!("/tv/{id}"), &[]).await?;
        let genres = self.genre_map().await;
        Ok(hit.into_candidate(&genres))
    }

    /// **Quantas temporadas a série tem** — o desempate da R72.
    ///
    /// Ignora a temporada 0: no TMDB ela é a dos especiais, e "tem especiais"
    /// não diz nada sobre até onde a série foi.
    ///
    /// `None` quando a chamada falha ou a série não lista temporada nenhuma —
    /// e `None` **não desempata**, pelo mesmo motivo de sempre: não saber não
    /// é evidência.
    pub async fn maior_temporada(&self, series_id: &str) -> Option<i32> {
        if let Some(guardado) = self.temporadas.read().await.get(series_id) {
            return *guardado;
        }
        let maior = self
            .series_seasons(series_id)
            .await
            .ok()
            .and_then(|t| t.iter().map(|s| s.season_number).filter(|n| *n > 0).max());
        self.temporadas
            .write()
            .await
            .insert(series_id.to_string(), maior);
        maior
    }

    /// As temporadas da série, com quantos episódios cada uma tem.
    ///
    /// É o índice que traduz numeração ABSOLUTA (a que fansub usa) para o par
    /// temporada/episódio que o provider entende: somando `episode_count` até
    /// passar do número absoluto, chega-se na temporada.
    pub async fn series_seasons(&self, series_id: &str) -> anyhow::Result<Vec<SeasonSummary>> {
        let detail: SeriesDetail = self.get(&format!("/tv/{series_id}"), &[]).await?;
        Ok(detail.seasons)
    }
}

/// Quantos nomes de elenco guardar. Além disso vira ruído: ninguém procura o
/// 40º figurante, e cada nome é um retrato baixado.
const MAX_CAST: usize = 15;

/// Papéis do TMDB → o vocabulário do Odeon. `None` descarta o cargo.
fn role_for_job(job: &str) -> Option<&'static str> {
    match job {
        "Director" => Some("director"),
        "Writer" | "Screenplay" | "Story" | "Author" => Some("writer"),
        "Creator" | "Series Creator" => Some("creator"),
        "Original Music Composer" | "Music" | "Composer" => Some("composer"),
        "Producer" | "Executive Producer" => Some("producer"),
        _ => None,
    }
}

/// Uma pessoa num trabalho, já no vocabulário do Odeon.
#[derive(Debug, Clone)]
pub struct CreditPerson {
    /// Chave estável do provider — é o que impede duplicar a mesma pessoa.
    pub provider_key: String,
    pub name: String,
    pub role: String,
    pub character_name: Option<String>,
    pub position: i32,
    pub image_url: Option<String>,
    pub known_for: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreditsResponse {
    #[serde(default = "Vec::new")]
    cast: Vec<CastMember>,
    #[serde(default = "Vec::new")]
    crew: Vec<CrewMember>,
}

#[derive(Debug, Deserialize)]
struct CastMember {
    id: i64,
    name: String,
    character: Option<String>,
    profile_path: Option<String>,
    known_for_department: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CrewMember {
    id: i64,
    name: String,
    job: Option<String>,
    profile_path: Option<String>,
    known_for_department: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GenreList {
    #[serde(default = "Vec::new")]
    genres: Vec<Genre>,
}

#[derive(Debug, Deserialize)]
struct Genre {
    id: i64,
    name: String,
}

/// A ficha de produção — país e idioma original.
///
/// Vem de `/movie/{id}`, e **não** da busca: `production_countries` não existe
/// no resultado de `/search/movie`. Por isso ela é buscada na hora de APLICAR o
/// candidato, uma vez por filme, e não uma vez por candidato avaliado — a mesma
/// forma do `ensure_serie` do §21, que faz um `GET /tv/{id}` por série em vez de
/// um por episódio.
/// Medido em 40 filmes sorteados do acervo, antes de decidir o que entra:
///
/// | | |
/// |---|---|
/// | país de produção | **100%** |
/// | idioma original | **100%** |
/// | empresa produtora | 100% |
/// | orçamento e bilheteria | 92% |
///
/// **Nem tudo que veio entra**, e as duas exclusões têm motivo medido:
///
/// * **Empresa produtora fica de fora.** 40 filmes trouxeram **34 empresas
///   distintas** — quase uma por filme. Um eixo em que cada item tem uma obra
///   não é eixo, é lista; é a mesma reprovação que o corte de "2+ obras" do
///   §8h aplica às pessoas, e a mesma razão pela qual a R18 (§30) recusou o
///   eixo de produção.
/// * **Orçamento e bilheteria ficam de fora**, apesar dos 92%. O §33 já os
///   traz do Wikidata **com a moeda**, e lá isso foi decisão explícita: valor
///   em moeda que não sabemos nomear não vira curiosidade. Os campos `budget`
///   e `revenue` do TMDB são número puro, **sem moeda nenhuma** — escrever
///   "US$" sobre um orçamento em euros é exatamente a mentira com cara de
///   metadado que o §18 proíbe. Duas fontes para o mesmo fato, e uma delas
///   pior, é pior que uma fonte só.
#[derive(Debug, Default, Deserialize)]
pub struct FichaDeProducao {
    #[serde(default)]
    pub production_countries: Vec<PaisDoProvider>,
    /// ISO 639-1. O TMDB usa `xx` para "sem diálogo".
    pub original_language: Option<String>,
}

/// O envelope de `/movie/{id}` visto pelo lado da saga.
#[derive(Debug, Default, Deserialize)]
pub struct FichaDeSaga {
    /// `null` na maioria dos filmes — a maioria não pertence a saga nenhuma, e
    /// isso é fato, não falha.
    pub belongs_to_collection: Option<SagaDoProvider>,
}

/// Uma saga, como o TMDB a declara.
#[derive(Debug, Clone, Deserialize)]
pub struct SagaDoProvider {
    pub id: i64,
    /// Em pt-BR quando o TMDB tem — *"Coleção 007"*, *"Coleção De Volta para o
    /// Futuro"*.
    pub name: String,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PaisDoProvider {
    pub iso_3166_1: String,
    /// Em INGLÊS, mesmo pedindo `pt-BR` — ver `metadata/regiao.rs`.
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct SearchPage<T> {
    #[serde(default = "Vec::new")]
    results: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct MovieHit {
    id: i64,
    title: String,
    original_title: Option<String>,
    release_date: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    #[serde(default)]
    genre_ids: Vec<i64>,
    #[serde(default)]
    popularity: f32,
}

#[derive(Debug, Deserialize)]
struct TvHit {
    id: i64,
    name: String,
    original_name: Option<String>,
    first_air_date: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    /// A BUSCA devolve os gêneros como ids…
    #[serde(default)]
    genre_ids: Vec<i64>,
    /// …e `/tv/{id}` devolve como objetos. Aceitar as duas formas é o que
    /// permite reusar esta struct pro `tv_by_id` sem perder as tags de gênero,
    /// que alimentam o filtro do M2 e a curadoria do M5.
    #[serde(default)]
    genres: Vec<Genre>,
    #[serde(default)]
    popularity: f32,
}

#[derive(Debug, Deserialize)]
pub struct EpisodeDetail {
    pub name: Option<String>,
    pub overview: Option<String>,
    pub still_path: Option<String>,
    pub air_date: Option<String>,
}

impl EpisodeDetail {
    pub fn still_url(&self) -> Option<String> {
        self.still_path.as_ref().map(|p| format!("{IMG}/w780{p}"))
    }
}

#[derive(Debug, Deserialize)]
pub struct SeasonDetail {
    #[serde(default)]
    pub episodes: Vec<SeasonEpisode>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeasonEpisode {
    pub episode_number: i32,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub still_path: Option<String>,
}

impl SeasonEpisode {
    pub fn still_url(&self) -> Option<String> {
        self.still_path.as_ref().map(|p| format!("{IMG}/w780{p}"))
    }
}

#[derive(Debug, Deserialize)]
struct SeriesDetail {
    #[serde(default)]
    seasons: Vec<SeasonSummary>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeasonSummary {
    pub season_number: i32,
    #[serde(default)]
    pub episode_count: i32,
    /// R63 — a ficha da temporada, que vem **na mesma resposta** de `/tv/{id}`.
    ///
    /// É por isso que o job de temporadas custa uma chamada por *série* e não
    /// por temporada: 120 contra 473 neste acervo. O `/tv/{id}/season/{n}`
    /// existe e traz o mesmo `poster_path` — pedir lá seria pagar quatro vezes
    /// pelo que já está aqui.
    pub name: Option<String>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub air_date: Option<String>,
}

/// "2017-10-04" → 2017
fn year_of(date: &Option<String>) -> Option<i32> {
    date.as_ref()
        .filter(|d| d.len() >= 4)
        .and_then(|d| d[..4].parse().ok())
}

fn image(path: &Option<String>, size: &str) -> Option<String> {
    path.as_ref().map(|p| url_de_imagem(p, size))
}

/// A URL de uma imagem do TMDB a partir do caminho que ele devolve
/// (`/mv0MySTq….jpg`).
///
/// Público porque o conserto das capas de saga (R38) remonta a URL a partir do
/// caminho que a R32 deixou guardado no banco — o insumo do reparo já está lá,
/// e nenhuma pergunta precisa ser refeita à API.
pub fn url_de_imagem(caminho: &str, tamanho: &str) -> String {
    format!("{IMG}/{tamanho}{caminho}")
}

/// Os tamanhos que o resto do código pede. São os mesmos do pipeline de série
/// desde o M1: um pôster serve a moldura, um backdrop serve a tela inteira.
pub const POSTER: &str = "w500";
pub const BACKDROP: &str = "w1280";

/// TMDB devolve string vazia em vez de null quando não tem sinopse.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

fn names_of(ids: &[i64], genres: &HashMap<i64, String>) -> Vec<String> {
    ids.iter().filter_map(|id| genres.get(id).cloned()).collect()
}

impl MovieHit {
    fn into_candidate(self, genres: &HashMap<i64, String>) -> Candidate {
        Candidate {
            provider: "tmdb".into(),
            provider_id: self.id.to_string(),
            provider_kind: "movie".into(),
            year: year_of(&self.release_date),
            poster_url: image(&self.poster_path, POSTER),
            backdrop_url: image(&self.backdrop_path, BACKDROP),
            genres: names_of(&self.genre_ids, genres),
            title: self.title,
            original_title: self.original_title,
            overview: non_empty(self.overview),
            accent_color: None,
            popularity: self.popularity,
            raw: serde_json::Value::Null,
        }
    }
}

impl TvHit {
    fn into_candidate(self, genres: &HashMap<i64, String>) -> Candidate {
        Candidate {
            provider: "tmdb".into(),
            provider_id: self.id.to_string(),
            provider_kind: "tv".into(),
            year: year_of(&self.first_air_date),
            poster_url: image(&self.poster_path, POSTER),
            backdrop_url: image(&self.backdrop_path, BACKDROP),
            genres: if self.genres.is_empty() {
                names_of(&self.genre_ids, genres)
            } else {
                self.genres.iter().map(|g| g.name.clone()).collect()
            },
            title: self.name,
            original_title: self.original_name,
            overview: non_empty(self.overview),
            accent_color: None,
            popularity: self.popularity,
            raw: serde_json::Value::Null,
        }
    }
}
