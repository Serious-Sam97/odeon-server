//! Navegador de pastas.
//!
//! Existe pra a pessoa escolher onde ficam os filmes sem editar `.env` e
//! reiniciar container. Mas ele expõe o filesystem do servidor, então duas
//! regras não são negociáveis:
//!
//! 1. **Só admin.** Listar diretórios é reconhecimento de terreno.
//! 2. **Só dentro das raízes montadas.** Todo caminho pedido é canonicalizado
//!    e conferido contra a lista — sem isso, `../../..` leria o container
//!    inteiro, e um symlink apontando pra fora faria o mesmo em silêncio.

use std::path::{Path, PathBuf};

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::auth::AdminUser;
use crate::error::{AppError, AppResult};
use crate::AppState;

/// Extensões que contam como mídia — só pra dizer "esta pasta tem N vídeos",
/// que é o sinal que a pessoa procura ao escolher onde apontar a biblioteca.
const VIDEO_EXTS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "m4v", "webm", "ts", "m2ts", "mpg", "mpeg", "wmv", "flv", "ogv",
];

/// O que uma pasta **parece** ser, lido dos nomes dos arquivos dentro dela.
///
/// É um palpite, e o nome diz isso de propósito (§18): quem decide o tipo da
/// biblioteca continua sendo a pessoa. Ele só existe pra ela não escolher no
/// escuro.
#[derive(Debug, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Palpite {
    Filme,
    Serie,
    Mistura,
}

/// A pasta já está numa biblioteca — e o servidor **recusa** as duas direções.
///
/// `create_library` devolve 400 tanto pra pasta dentro de uma biblioteca quanto
/// pra pasta que contém uma: *"um arquivo pertence a UMA biblioteca"*. Oferecer
/// o botão nesses casos é o §53 — o produto oferecendo o que ele sabe que vai
/// negar —, então a tela precisa saber disso **antes** do clique.
#[derive(Debug, Serialize, Clone)]
pub struct Cobertura {
    pub biblioteca: String,
    /// `true`: esta pasta está DENTRO da biblioteca.
    /// `false`: esta pasta CONTÉM a biblioteca.
    pub dentro: bool,
}

#[derive(Debug, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    /// Quantos vídeos direto nesta pasta (não conta subpastas).
    pub video_count: usize,
    /// E quantos nas subpastas dela — **o número que faltava**.
    ///
    /// Sem ele, 30 das 40 pastas de `/media/TV Show` diziam "0 vídeos ·
    /// subpastas", porque episódio mora em pasta de temporada. A tela dizia
    /// nada justamente onde a pessoa mais precisa de alguma coisa.
    pub videos_abaixo: usize,
    pub has_subdirs: bool,
    pub coberta_por: Option<Cobertura>,
    pub palpite: Option<Palpite>,
    /// A contagem bateu no teto e parou. A tela mostra "+" em vez de um número
    /// que seria mentira.
    pub truncado: bool,
}

#[derive(Debug, Serialize)]
pub struct Listing {
    pub path: String,
    /// `None` quando já se está numa raiz — a UI esconde o "subir".
    pub parent: Option<String>,
    pub roots: Vec<String>,
    pub entries: Vec<Entry>,
    pub video_count: usize,
    /// O que há abaixo desta pasta, somando as subpastas listadas. Sai da
    /// mesma varredura das entradas — nenhuma leitura a mais.
    pub videos_abaixo: usize,
    pub coberta_por: Option<Cobertura>,
    pub palpite: Option<Palpite>,
    pub truncado: bool,
}

#[derive(Debug, Deserialize)]
pub struct BrowseQuery {
    #[serde(default)]
    pub path: Option<String>,
}

/// Canonicaliza e confere contra as raízes.
///
/// `canonicalize` resolve symlink e `..` — é o que impede uma pasta "atalho"
/// dentro de `/media` de virar porta pro resto do sistema de arquivos.
fn resolve(requested: &Path, roots: &[PathBuf]) -> Result<PathBuf, AppError> {
    let canonical = requested
        .canonicalize()
        .map_err(|_| AppError::BadRequest(format!("caminho não existe: {}", requested.display())))?;

    let inside = roots.iter().any(|root| {
        root.canonicalize()
            .map(|r| canonical.starts_with(&r))
            .unwrap_or(false)
    });

    if !inside {
        return Err(AppError::Forbidden(
            "caminho fora das pastas montadas no servidor".into(),
        ));
    }
    Ok(canonical)
}

fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Teto de vídeos contados por pasta.
///
/// **Não é uma paginação, é um freio contra árvore patológica.** A maior pasta
/// deste acervo é `A Grande Família`, com 949 arquivos — o teto está uma ordem
/// de grandeza acima, e nunca dispara aqui. Se disparar, a tela diz "5000+" em
/// vez de um número redondo que seria mentira (§18).
const TETO: usize = 5_000;

/// Quantos nomes bastam pra dizer o que uma pasta é.
///
/// Vinte e quatro, e não todos: o palpite é sobre o **formato** dos nomes, e o
/// vigésimo quinto arquivo de uma série não acrescenta nada ao que os vinte e
/// quatro primeiros já disseram. Rodar o parser em 949 nomes pra chegar na
/// mesma resposta é trabalho jogado fora.
const AMOSTRA: usize = 24;

/// O que se descobre abrindo uma pasta e a primeira camada de subpastas.
#[derive(Default)]
struct Olhada {
    diretos: usize,
    abaixo: usize,
    subpastas: bool,
    amostra: Vec<PathBuf>,
    truncado: bool,
    /// Quantos vídeos há em cada subpasta que tem algum. **É a forma da
    /// pasta**, e ela separa dois casos que o nome do arquivo sozinho não
    /// separa — ver `palpitar` e `tipica`.
    por_subpasta: Vec<usize>,
}

/// A subpasta típica, e **não a mais cheia**.
///
/// O máximo mente com um único caso fora da curva, e o acervo tem um: as 143
/// pastas de `/media2/Movies` têm 1 vídeo cada, menos `007 Coleção`, que tem
/// 24. Pelo máximo, a segunda maior biblioteca de filmes da casa ficava sem
/// palpite por causa de um box set. Pela mediana, ela volta a ser o que é.
///
/// E o caso que a regra existe pra pegar não escapa: as pastas de temporada do
/// `Bob Esponja` têm ~39 vídeos **cada uma**, então a mediana também é ~39.
fn tipica(mut por_subpasta: Vec<usize>) -> usize {
    if por_subpasta.is_empty() {
        return 0;
    }
    por_subpasta.sort_unstable();
    por_subpasta[por_subpasta.len() / 2]
}

/// Olha uma pasta **três níveis pra baixo**.
///
/// Eram dois, e dois cobriam `Série/Temporada 1/ep.mkv`. O terceiro entrou com o
/// `/mnt/SAM`, que agrupa por franquia e portanto tem um degrau a mais —
/// `Movies/All Movies/Filme (2024)/arquivo.mkv` e
/// `TV Shows/Série/Temporada/ep.mkv`. Com dois níveis a tela dizia **"0 vídeos"
/// numa pasta com 131**, que é o §18 pelo avesso: omitir o que existe engana
/// tanto quanto inventar o que não existe.
///
/// ## O terceiro nível é incondicional, e a primeira tentativa não era
///
/// A versão anterior só descia quando o segundo nível vinha vazio — a ideia era
/// pagar o custo só onde a resposta seria inútil. **Ela contava 123 de 261.**
///
/// O que ela perdia são as pastas mistas, e o `/mnt/SAM` tem duas grandes:
/// `All Movies` (5 arquivos soltos e 93 subpastas) e `Animations` (5 e 45). Um
/// arquivo solto ali já fazia a condição falhar, e as 93 subpastas sumiam por
/// causa dos 5.
///
/// **E a economia era quase nada.** Medido no pior caso do acervo,
/// `/media2/TV Show` com 15 mil arquivos: 169ms contra 198ms. Trinta
/// milissegundos não compram metade de uma contagem.
///
/// Não há quarto nível. A cada degrau o custo multiplica pelo número de pastas,
/// e quatro degraus é onde ficam os "Extras" e os "Disco 2" — que não são o que
/// alguém procura ao escolher uma pasta.
async fn olhar(dir: &Path) -> Olhada {
    let mut o = Olhada::default();

    let Ok(mut nivel1) = tokio::fs::read_dir(dir).await else {
        return o;
    };

    while let Ok(Some(item)) = nivel1.next_entry().await {
        let path = item.path();
        let nome = item.file_name().to_string_lossy().to_string();
        if oculto(&nome) {
            continue;
        }
        let Ok(tipo) = item.file_type().await else {
            continue;
        };

        if tipo.is_file() {
            if is_video(&path) {
                o.diretos += 1;
                if o.amostra.len() < AMOSTRA {
                    o.amostra.push(path);
                }
            }
            continue;
        }
        if !tipo.is_dir() {
            continue;
        }

        o.subpastas = true;
        let Ok(mut nivel2) = tokio::fs::read_dir(&path).await else {
            continue;
        };
        let mut nesta = 0usize;
        // Guardadas pro terceiro nível, e só usadas se este aqui vier vazio.
        let mut netos: Vec<PathBuf> = Vec::new();
        while let Ok(Some(neto)) = nivel2.next_entry().await {
            let p = neto.path();
            match neto.file_type().await {
                Ok(t) if t.is_dir() => {
                    if !oculto(&neto.file_name().to_string_lossy()) {
                        netos.push(p);
                    }
                    continue;
                }
                Ok(t) if t.is_file() && is_video(&p) => {}
                _ => continue,
            }
            o.abaixo += 1;
            nesta += 1;
            if o.amostra.len() < AMOSTRA {
                o.amostra.push(p);
            }
            if o.diretos + o.abaixo >= TETO {
                o.truncado = true;
                o.por_subpasta.push(nesta);
                return o;
            }
        }

        // O TERCEIRO NÍVEL. `TV Shows/Love Death and Robots/…S03/ep.mkv`: a
        // pasta da série não tem vídeo solto, tem temporadas — sem descer aqui,
        // ela some da tela. E sem condição: ver o cabeçalho.
        for bisneto in &netos {
            let Ok(mut nivel3) = tokio::fs::read_dir(bisneto).await else {
                continue;
            };
            while let Ok(Some(f)) = nivel3.next_entry().await {
                let p = f.path();
                if !matches!(f.file_type().await, Ok(t) if t.is_file()) || !is_video(&p) {
                    continue;
                }
                o.abaixo += 1;
                nesta += 1;
                if o.amostra.len() < AMOSTRA {
                    o.amostra.push(p);
                }
                if o.diretos + o.abaixo >= TETO {
                    o.truncado = true;
                    o.por_subpasta.push(nesta);
                    return o;
                }
            }
        }

        if nesta > 0 {
            o.por_subpasta.push(nesta);
        }
    }
    o
}

/// Ocultos e as pastas de metadata de NAS só poluem a escolha.
fn oculto(nome: &str) -> bool {
    nome.starts_with('.') || nome == "@eaDir" || nome == "lost+found"
}

/// O palpite, e ele é uma função pura de propósito — assim ele é testável sem
/// disco, que é onde as regras de nome merecem ser exercidas.
///
/// **`serial = false` no `guess_from_path`, e isso importa.** O parser tem
/// regras que só valem quando já se sabe que a biblioteca é de série (índice na
/// frente, episódio absoluto sem sinal de anime) — e o comentário do `guess.rs`
/// avisa: *"num acervo de filmes elas causariam estrago"*. Aqui ninguém sabe
/// ainda o que a pasta é: essa é justamente a pergunta. Então vale só o que o
/// nome diz sozinho, sem ajuda.
/// `concentracao`: quantos vídeos cabem na **mesma** pasta ali dentro, no pior
/// caso. É a forma, e ela é o que separa um caso que o nome sozinho não separa —
/// ver `quando_o_nome_nao_numera_a_forma_decide`.
///
/// **Não é o total, e a diferença custou uma medição errada.** `/media/Movies`
/// tem 9 filmes soltos na raiz além das 143 pastas; contando esses 9 como
/// concentração, a maior biblioteca de filmes do acervo ficava sem palpite.
/// Nove filmes soltos são nove filmes — a concentração mora nas subpastas.
fn palpitar(amostra: &[PathBuf], raiz: &Path, concentracao: usize) -> Option<Palpite> {
    if amostra.is_empty() {
        return None;
    }
    let episodios = amostra
        .iter()
        .filter(|p| {
            crate::scanner::guess::guess_from_path(p, raiz, false)
                .any_episode()
                .is_some()
        })
        .count();

    // Quatro em cinco, e não todos: uma série real tem um "extras" ou um
    // "especial de natal" no meio que não numera, e chamar isso de "mistura"
    // apagaria o sinal em quase toda pasta de série que existe.
    if episodios * 5 >= amostra.len() * 4 {
        return Some(Palpite::Serie);
    }
    if episodios * 5 > amostra.len() {
        return Some(Palpite::Mistura);
    }

    // Nenhum nome numera episódio. Isso NÃO quer dizer "filme".
    //
    // Medido neste acervo: `Bob Esponja` tem 116 vídeos chamados
    // `Bob.Esponja.SO1E09.avi` — com a **letra O** no lugar do zero. O parser
    // está certo em não casar; quem está errado é o nome no disco. Chamar essa
    // pasta de "filme" seria o §18 na veia: mentir com cara de metadado.
    //
    // O que separa os dois casos é a forma, e a medição a mostra sem ambiguidade
    // nenhuma: **as 143 pastas de `/media/Movies` têm exatamente 1 vídeo cada**,
    // e a menor pasta de série tem 6. O corte em 3 fica no meio de um vazio, e
    // não em cima do dado.
    //
    // Acima disso a resposta honesta é não ter resposta — a tela omite (§24).
    if concentracao <= FILME_NO_MAXIMO {
        Some(Palpite::Filme)
    } else {
        None
    }
}

/// Quantos vídeos ainda cabem numa pasta que é "um filme".
///
/// Um, na prática — este acervo tem 143 de 143 assim. Três dá espaço pra um
/// filme com making-of e cena deletada sem abrir a porta pra uma temporada.
const FILME_NO_MAXIMO: usize = 3;

/// Esta pasta já está numa biblioteca — nos dois sentidos, porque o servidor
/// recusa os dois.
fn cobertura(path: &Path, bibliotecas: &[(String, String)]) -> Option<Cobertura> {
    for (nome, raiz) in bibliotecas {
        let outra = Path::new(raiz);
        if path.starts_with(outra) {
            return Some(Cobertura {
                biblioteca: nome.clone(),
                dentro: true,
            });
        }
        if outra.starts_with(path) {
            return Some(Cobertura {
                biblioteca: nome.clone(),
                dentro: false,
            });
        }
    }
    None
}

pub async fn browse(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<BrowseQuery>,
) -> AppResult<Json<Listing>> {
    let roots = &state.config.media_roots;

    // Sem `path`, mostra a primeira raiz. Com uma raiz só (o caso comum), isso
    // já é a pasta certa.
    let requested = match &params.path {
        Some(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => roots
            .first()
            .cloned()
            .ok_or_else(|| AppError::BadRequest("nenhuma pasta montada".into()))?,
    };

    let current = resolve(&requested, roots)?;

    // As bibliotecas que já existem, pra dizer o que já está tomado. Uma
    // consulta pra a listagem inteira.
    let bibliotecas: Vec<(String, String)> =
        sqlx::query_as("SELECT name, root_path FROM library")
            .fetch_all(&state.pool)
            .await?;

    let mut entries: Vec<Entry> = Vec::new();
    let mut video_count = 0usize;
    let mut videos_abaixo = 0usize;
    let mut truncado = false;
    // A forma desta pasta, vista pelas filhas. Numa pasta de filmes a filha
    // típica tem um vídeo; numa de séries, uma temporada inteira.
    let mut por_entrada: Vec<usize> = Vec::new();
    // A amostra da pasta atual sai das amostras das filhas, sem leitura a mais.
    let mut amostra: Vec<PathBuf> = Vec::new();

    let mut dir = tokio::fs::read_dir(&current)
        .await
        .map_err(|e| AppError::BadRequest(format!("não consegui ler a pasta: {e}")))?;

    while let Ok(Some(item)) = dir.next_entry().await {
        let path = item.path();
        let name = item.file_name().to_string_lossy().to_string();

        if oculto(&name) {
            continue;
        }

        let Ok(kind) = item.file_type().await else {
            continue;
        };

        if kind.is_file() {
            if is_video(&path) {
                video_count += 1;
                if amostra.len() < AMOSTRA {
                    amostra.push(path);
                }
            }
            continue;
        }

        if !kind.is_dir() {
            continue;
        }

        // Espia dentro — dois níveis, que é o que faz `Série/Temporada/ep.mkv`
        // deixar de dizer "0 vídeos".
        let o = olhar(&path).await;

        videos_abaixo += o.diretos + o.abaixo;
        truncado |= o.truncado;
        por_entrada.push(o.diretos + o.abaixo);
        for p in o.amostra.iter().take(AMOSTRA.saturating_sub(amostra.len())) {
            amostra.push(p.clone());
        }

        entries.push(Entry {
            name,
            // Com subpasta que tenha vídeo, a forma é a subpasta típica; sem
            // nenhuma, são os arquivos que ela guarda — uma pasta de 23 vídeos
            // sem subpasta é uma temporada, e uma de 1 é um filme.
            palpite: palpitar(
                &o.amostra,
                &path,
                if o.por_subpasta.is_empty() {
                    o.diretos
                } else {
                    tipica(o.por_subpasta.clone())
                },
            ),
            coberta_por: cobertura(&path, &bibliotecas),
            video_count: o.diretos,
            videos_abaixo: o.abaixo,
            has_subdirs: o.subpastas,
            truncado: o.truncado,
            path: path.to_string_lossy().to_string(),
        });
    }

    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    // Só oferece "subir" enquanto não se está numa raiz.
    let at_root = roots
        .iter()
        .any(|r| r.canonicalize().map(|r| r == current).unwrap_or(false));
    let parent = if at_root {
        None
    } else {
        current
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .filter(|_| true)
    };

    Ok(Json(Listing {
        // A mesma conta um nível acima: a forma desta pasta é a entrada típica
        // dela, e só na ausência de entradas são os arquivos soltos.
        palpite: palpitar(
            &amostra,
            &current,
            if por_entrada.iter().all(|n| *n == 0) {
                video_count
            } else {
                tipica(por_entrada.iter().copied().filter(|n| *n > 0).collect())
            },
        ),
        coberta_por: cobertura(&current, &bibliotecas),
        path: current.to_string_lossy().to_string(),
        parent,
        roots: roots.iter().map(|r| r.to_string_lossy().to_string()).collect(),
        entries,
        video_count,
        videos_abaixo,
        truncado,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> Vec<PathBuf> {
        vec![PathBuf::from("/tmp")]
    }

    #[test]
    fn caminho_inexistente_e_recusado() {
        let err = resolve(Path::new("/tmp/nao-existe-mesmo-12345"), &roots());
        assert!(matches!(err, Err(AppError::BadRequest(_))));
    }

    #[test]
    fn caminho_fora_das_raizes_e_proibido() {
        // /etc existe, mas não está na lista
        let err = resolve(Path::new("/etc"), &roots());
        assert!(matches!(err, Err(AppError::Forbidden(_))));
    }

    #[test]
    fn travessia_com_pontos_nao_escapa() {
        // canonicalize resolve o `..` antes da conferência
        let err = resolve(Path::new("/tmp/../etc"), &roots());
        assert!(matches!(err, Err(AppError::Forbidden(_))));
    }

    #[test]
    fn a_propria_raiz_e_permitida() {
        assert!(resolve(Path::new("/tmp"), &roots()).is_ok());
    }

    #[test]
    fn reconhece_extensao_de_video() {
        assert!(is_video(Path::new("/x/filme.MKV")));
        assert!(is_video(Path::new("/x/a.mp4")));
        assert!(!is_video(Path::new("/x/legenda.srt")));
        assert!(!is_video(Path::new("/x/sem-extensao")));
    }

    /// **O caso que o `/mnt/SAM` trouxe, e ele é de disco, não de regra.**
    ///
    /// Aquele disco agrupa por franquia, então a árvore tem um degrau a mais:
    /// `TV Shows/Série/Temporada/ep.mkv`. Com dois níveis a tela dizia
    /// **"0 vídeos" numa pasta com 131** — e "0" numa pasta cheia é o §18 pelo
    /// avesso.
    ///
    /// O teste monta três formas: a rasa (que já contava), a funda (que sumia)
    /// e a **mista** — um arquivo solto ao lado de subpastas cheias. A mista é
    /// o caso que derrubou a primeira versão desta regra, que só descia quando
    /// o segundo nível vinha vazio: um arquivo solto fazia 93 subpastas
    /// sumirem.
    #[tokio::test]
    async fn o_terceiro_nivel_vale_ate_na_pasta_mista() {
        let raiz = std::env::temp_dir().join(format!("odeon-olhar-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&raiz).await;

        // Rasa: Filmes/Filme (2024)/arquivo.mkv — dois níveis bastam.
        let rasa = raiz.join("rasa").join("Filme (2024)");
        tokio::fs::create_dir_all(&rasa).await.unwrap();
        tokio::fs::write(rasa.join("Filme.2024.mkv"), b"").await.unwrap();

        // Funda: Series/Serie/Temporada 3/ep.mkv — três.
        let funda = raiz.join("funda").join("Serie").join("Temporada 3");
        tokio::fs::create_dir_all(&funda).await.unwrap();
        for ep in ["S03E01", "S03E02", "S03E03"] {
            tokio::fs::write(funda.join(format!("Serie.{ep}.mkv")), b"").await.unwrap();
        }

        let r = olhar(&raiz.join("rasa")).await;
        assert_eq!(r.abaixo, 1, "a pasta rasa continua contando como antes");

        let f = olhar(&raiz.join("funda")).await;
        assert_eq!(f.abaixo, 3, "sem o terceiro nível isto seria 0, e a tela mentiria");
        assert_eq!(
            f.por_subpasta,
            vec![3],
            "os três contam pra FORMA da pasta, senão o palpite decide no escuro"
        );

        // MISTA: um solto e duas subpastas com dois cada. Total 5.
        let mista = raiz.join("mista").join("All Movies");
        tokio::fs::create_dir_all(&mista).await.unwrap();
        tokio::fs::write(mista.join("Solto.2020.mkv"), b"").await.unwrap();
        for pasta in ["Filme A (2021)", "Filme B (2022)"] {
            let d = mista.join(pasta);
            tokio::fs::create_dir_all(&d).await.unwrap();
            tokio::fs::write(d.join("a.mkv"), b"").await.unwrap();
            tokio::fs::write(d.join("b.mkv"), b"").await.unwrap();
        }
        let m = olhar(&raiz.join("mista")).await;
        assert_eq!(
            m.abaixo, 5,
            "o arquivo solto não pode fazer as subpastas sumirem — foi assim que \
             a primeira versão contou 123 de 261 no /mnt/SAM"
        );

        let _ = tokio::fs::remove_dir_all(&raiz).await;
    }

    fn amostra(raiz: &str, nomes: &[&str]) -> Vec<PathBuf> {
        nomes.iter().map(|n| PathBuf::from(raiz).join(n)).collect()
    }

    /// Uma pasta de série é reconhecida pelo formato dos nomes, e é isso que a
    /// tela precisa dizer antes de alguém escolher no escuro.
    #[test]
    fn palpite_de_serie() {
        let raiz = "/media/TV Show/Breaking Bad";
        let a = amostra(
            raiz,
            &[
                "Season 1/Breaking Bad S01E01.mkv",
                "Season 1/Breaking Bad S01E02.mkv",
                "Season 2/Breaking Bad S02E01.mkv",
                "Season 2/Breaking Bad S02E02.mkv",
            ],
        );
        assert_eq!(palpitar(&a, Path::new(raiz), 40), Some(Palpite::Serie));
    }

    #[test]
    fn palpite_de_filme() {
        let raiz = "/media/Movies";
        let a = amostra(
            raiz,
            &[
                "1917 (2019)/1917.2019.1080p.mkv",
                "Drive (2011)/Drive.2011.720p.mkv",
                "28 Weeks Later (2007)/28.Weeks.Later.2007.mkv",
                "Blade Runner 2049 (2017)/Blade.Runner.2049.mkv",
            ],
        );
        assert_eq!(palpitar(&a, Path::new(raiz), 1), Some(Palpite::Filme));
    }

    /// **Um "extras" no meio de uma série não muda o que a pasta é.**
    ///
    /// Sem a folga de um em cinco, quase toda pasta de série viraria "mistura"
    /// — e um rótulo que aparece em todo lugar não informa nada.
    #[test]
    fn um_arquivo_fora_do_padrao_nao_derruba_a_serie() {
        let raiz = "/media/TV Show/Show";
        let a = amostra(
            raiz,
            &[
                "Season 1/Show S01E01.mkv",
                "Season 1/Show S01E02.mkv",
                "Season 1/Show S01E03.mkv",
                "Season 1/Show S01E04.mkv",
                "Extras/Making Of.mkv",
            ],
        );
        assert_eq!(palpitar(&a, Path::new(raiz), 24), Some(Palpite::Serie));
    }

    #[test]
    fn metade_e_metade_e_mistura() {
        let raiz = "/media/Bagunca";
        let a = amostra(
            raiz,
            &[
                "Show S01E01.mkv",
                "Show S01E02.mkv",
                "Drive.2011.mkv",
                "1917.2019.mkv",
            ],
        );
        assert_eq!(palpitar(&a, Path::new(raiz), 4), Some(Palpite::Mistura));
    }

    /// §18: sem arquivo nenhum, a tela **omite** em vez de chutar.
    #[test]
    fn pasta_sem_video_nao_tem_palpite() {
        assert_eq!(palpitar(&[], Path::new("/media/Vazia"), 0), None);
    }

    /// **O caso que veio do acervo, e o que ele ensinou.**
    ///
    /// `Bob Esponja` tem 116 vídeos chamados `Bob.Esponja.SO1E09.avi` — com a
    /// letra **O** no lugar do zero. Nenhum casa como episódio, e o parser está
    /// certo: quem está errado é o nome no disco.
    ///
    /// Antes desta regra a pasta era rotulada **"filme"**, com toda a
    /// confiança, e isso é o §18 na veia. Os mesmos nomes numa pasta de UM
    /// vídeo continuam sendo filme; numa pasta com uma temporada inteira
    /// dentro, a resposta honesta é não ter resposta.
    #[test]
    fn quando_o_nome_nao_numera_a_forma_decide() {
        let raiz = "/media/TV Show/Bob Esponja";
        let a = amostra(
            raiz,
            &[
                "Temporada 1/Bob.Esponja.SO1E09.avi",
                "Temporada 1/Bob.Esponja.SO1E35.avi",
                "Temporada 1/Bob.Esponja.SO1E21.avi",
                "Temporada 1/Bob.Esponja.SO1E20.avi",
            ],
        );
        let p = Path::new(raiz);
        assert_eq!(palpitar(&a, p, 39), None, "39 numa pasta só não é um filme");
        assert_eq!(
            palpitar(&a, p, 1),
            Some(Palpite::Filme),
            "os mesmos nomes, um vídeo só, continuam sendo filme"
        );
    }

    /// O corte fica no vazio entre os dois casos medidos: 143 de 143 pastas de
    /// filme deste acervo têm exatamente 1 vídeo, e a menor pasta de série tem
    /// 6. Se alguém encostar o corte no dado, este teste cai.
    #[test]
    fn o_corte_de_filme_fica_no_vazio_entre_os_dois_casos() {
        assert!(FILME_NO_MAXIMO >= 1, "toda pasta de filme tem 1 vídeo");
        assert!(FILME_NO_MAXIMO < 6, "a menor pasta de série tem 6");
    }

    fn bibliotecas() -> Vec<(String, String)> {
        vec![
            ("Filmes (DAS0)".into(), "/media/Movies".into()),
            ("Séries (DAS0)".into(), "/media/TV Show".into()),
        ]
    }

    /// As duas direções, porque `create_library` recusa as duas — e a tela
    /// precisa saber antes do clique (§53).
    #[test]
    fn cobertura_pega_a_pasta_dentro_da_biblioteca() {
        let c = cobertura(Path::new("/media/Movies/Drive (2011)"), &bibliotecas()).unwrap();
        assert_eq!(c.biblioteca, "Filmes (DAS0)");
        assert!(c.dentro);
    }

    #[test]
    fn cobertura_pega_a_pasta_que_contem_a_biblioteca() {
        let c = cobertura(Path::new("/media"), &bibliotecas()).unwrap();
        assert!(!c.dentro, "/media contém as bibliotecas, não está dentro");
    }

    #[test]
    fn pasta_livre_nao_tem_cobertura() {
        assert!(cobertura(Path::new("/media2/Music"), &bibliotecas()).is_none());
    }
}
