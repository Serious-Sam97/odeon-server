//! R76 — identificar a **pasta**, e depois colocar os episódios.
//!
//! ## O degrau que isto existe pra vencer
//!
//! Medido em 20/08/2026, no acervo inteiro:
//!
//! ```text
//! episódio COM ano   3.895 auto + 4.253 confirmado     456 em revisão
//! episódio SEM ano       0 auto                      2.820 em revisão
//! ```
//!
//! **Zero.** E não é azar, é aritmética: um episódio sem ano, com título
//! idêntico e formato batendo, chega a `0,65 + 0,05 + 0,08 = 0,78`, mais no
//! máximo 0,04 de popularidade e 0,10 de corroboração — e a corroboração só
//! conta irmãos **já casados**, que numa pasta virgem são zero. O limiar é
//! 0,85. O ovo nunca choca.
//!
//! ## O que muda de lugar
//!
//! A identidade de um episódio não é um fato do arquivo — é um fato da
//! **pasta**. Pedir a mesma resposta 485 vezes não é só caro; é pedi-la 485
//! vezes sem a evidência que existiria se fosse pedida uma vez:
//!
//! | a pasta sabe | o arquivo não sabe |
//! |---|---|
//! | são 215 arquivos | um arquivo |
//! | cobrem as temporadas 1 a 14 | esta é a 6 |
//! | a temporada 3 tem 37 episódios | este é o 37 |
//!
//! O sinal do meio já entrou como desempate na R72 e resolveu 345 dos 485
//! arquivos de *A Grande Família*. Como sinal de pasta ele decide os 485 — e o
//! de baixo, que é o mais forte de todos, não tinha como existir antes.
//!
//! ## Por que o limiar daqui é mais alto
//!
//! Errar um arquivo custa um arquivo; errar uma pasta custa 215. O
//! [`LIMIAR`] é 0,90 contra os 0,85 do arquivo, e não é conservadorismo
//! decorativo: é o mesmo raciocínio que fez a corroboração de vizinhos ter
//! teto — quanto mais a decisão alcança, mais evidência ela tem de exigir.

use serde::Serialize;

use crate::metadata::tmdb::SeasonSummary;
use crate::metadata::Candidate;
use crate::scanner::guess::{guess_from_filename, Guess};

/// Acima disto a pasta pode ser aplicada sozinha.
///
/// Mais alto que o `AUTO_THRESHOLD` do arquivo (0,85) porque o estrago é
/// proporcional ao alcance — ver o cabeçalho do módulo.
pub const LIMIAR: f32 = 0.90;

/// ⚠️ **A calibragem foi refeita depois de reproduzir o próprio degrau que
/// este módulo existe pra vencer.**
///
/// A primeira versão dava 0,55 ao título, 0,10 às temporadas e 0,20 ao
/// encaixe. Teto sem ano: **0,85** — exatamente abaixo do limiar de 0,90, e
/// *A Grande Família* com título idêntico, 14 temporadas conferindo e 485
/// arquivos encaixando **não passava**. Era o erro do escorregador de arquivo
/// cometido um nível acima.
///
/// O conserto não foi baixar o limiar: foi reconhecer que o encaixe de
/// episódios é o sinal mais forte que existe aqui e pagar por ele. Teto sem
/// ano hoje: `0,55 + 0,12 + 0,25 = 0,92`.
const PESO_TITULO: f32 = 0.55;
const PESO_TEMPORADAS: f32 = 0.12;
const PESO_ENCAIXE: f32 = 0.25;

/// Quantos arquivos a pasta precisa ter pra a evidência estrutural valer
/// inteira.
///
/// **Sem isto, três arquivos valeriam o mesmo que 485.** Uma pasta com 3
/// episódios "cabe" em quase qualquer série do mundo — o encaixe só é
/// evidência quando há massa pra encaixar. Abaixo de 20, o bônus estrutural é
/// proporcional; uma pasta de 3 fica em 15% dele e vai pra revisão, que é o
/// lugar certo.
const MASSA_CHEIA: f32 = 20.0;

/// O que a pasta sabe sobre si mesma, sem perguntar nada a ninguém.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EvidenciaDaPasta {
    pub arquivos: i64,
    /// A biblioteca a que a pasta pertence — o `identify` precisa dela.
    #[serde(skip)]
    pub library_id: Option<uuid::Uuid>,
    /// `(temporada, quantos episódios desta temporada estão na pasta)`,
    /// ordenado. Vazio quando nenhum arquivo declara temporada.
    pub temporadas: Vec<(i32, i64)>,
    /// O ano que o nome da pasta declara, quando declara.
    pub ano: Option<i32>,
}

impl EvidenciaDaPasta {
    pub fn maior_temporada(&self) -> Option<i32> {
        self.temporadas.iter().map(|(n, _)| *n).max()
    }
}

/// O título e o ano que a pasta declara.
///
/// Reusa o parser de nome de arquivo: `Modern Family (2009)` devolve título e
/// ano, e `A Grande Família` devolve só o título. É o mesmo código que lê o
/// nome do arquivo, e tem de ser — se os dois divergirem, a pasta e o arquivo
/// passam a discordar sobre o que está escrito nelas.
pub fn ler_pasta(nome: &str) -> Guess {
    guess_from_filename(nome)
}

#[derive(Debug, Clone, Serialize)]
pub struct Pontuacao {
    pub valor: f32,
    pub motivos: Vec<String>,
}

/// Pontua um candidato **contra a pasta inteira**.
///
/// `temporadas_do_provider` vem de `/tv/{id}` e é o que permite o sinal mais
/// forte: comparar quantos episódios cada temporada tem lá com quantos estão
/// aqui.
pub fn pontuar(
    evidencia: &EvidenciaDaPasta,
    titulo_da_pasta: &str,
    candidato: &Candidate,
    temporadas_do_provider: &[SeasonSummary],
) -> Pontuacao {
    let mut motivos = Vec::new();

    // 1. Título — dominante, mas menos que no arquivo (0,55 contra 0,65),
    //    porque aqui há evidência estrutural pra dividir o peso.
    let (similaridade, motivo) = crate::metadata::score::similaridade_de_titulo(
        titulo_da_pasta,
        candidato,
    );
    motivos.push(motivo);
    let mut valor = similaridade * PESO_TITULO;

    // Quanto a evidência estrutural desta pasta vale — ver `MASSA_CHEIA`.
    let massa = (evidencia.arquivos as f32 / MASSA_CHEIA).min(1.0);

    // 2. Ano da pasta. Quando ela declara, é um sinal tão forte quanto no
    //    arquivo — e ela declara bem mais vezes que o nome do episódio.
    match (evidencia.ano, candidato.year) {
        (Some(g), Some(c)) if g == c => {
            valor += 0.15;
            motivos.push(format!("ano da pasta confere: {c}"));
        }
        (Some(g), Some(c)) if (g - c).abs() == 1 => {
            valor += 0.08;
            motivos.push(format!("ano quase confere: {g} vs {c}"));
        }
        (Some(g), Some(c)) => {
            valor -= 0.30;
            motivos.push(format!("ano NÃO confere: {g} vs {c}"));
        }
        _ => {}
    }

    // 3. A pasta cabe no candidato? Assimétrico, como na R72: caber é fraco,
    //    não caber é impossibilidade.
    let do_provider: Vec<&SeasonSummary> = temporadas_do_provider
        .iter()
        .filter(|s| s.season_number > 0)
        .collect();
    let maior_do_provider = do_provider.iter().map(|s| s.season_number).max();

    match (evidencia.maior_temporada(), maior_do_provider) {
        (Some(aqui), Some(la)) if la >= aqui => {
            valor += PESO_TEMPORADAS * massa;
            motivos.push(format!("tem {la} temporadas, e a pasta vai até a {aqui}"));
        }
        (Some(aqui), Some(la)) => {
            valor -= 0.25;
            motivos.push(format!(
                "o provider diz que esta série tem {la} temporadas, e a pasta vai até a {aqui}"
            ));
        }
        _ => {}
    }

    // 4. **O sinal que só existe olhando a pasta**: quantos episódios cada
    //    temporada tem lá contra quantos estão aqui.
    //
    //    Uma pasta com 80 arquivos na temporada 1 contra uma série cuja
    //    temporada 1 tem 80 episódios é quase prova. A comparação é "cabe",
    //    e não "é igual": acervo incompleto é o normal, e exigir igualdade
    //    puniria justamente quem tem meia temporada.
    if !evidencia.temporadas.is_empty() && !do_provider.is_empty() {
        let mut cabem = 0usize;
        let mut estouram = Vec::new();
        for (numero, aqui) in &evidencia.temporadas {
            match do_provider.iter().find(|s| s.season_number == *numero) {
                Some(s) if (s.episode_count as i64) >= *aqui => cabem += 1,
                Some(s) => estouram.push(format!("T{numero} tem {aqui} aqui e {} lá", s.episode_count)),
                None => estouram.push(format!("T{numero} não existe lá")),
            }
        }
        let fracao = cabem as f32 / evidencia.temporadas.len() as f32;
        valor += PESO_ENCAIXE * fracao * massa;
        if estouram.is_empty() {
            motivos.push(format!(
                "as {} temporadas da pasta cabem nas do provider",
                evidencia.temporadas.len()
            ));
        } else {
            motivos.push(format!("não encaixa: {}", estouram.join(", ")));
        }
    }

    Pontuacao {
        valor: valor.clamp(0.0, 1.0),
        motivos,
    }
}

/// Uma proposta: esta pasta é esta série.
#[derive(Debug, Clone, Serialize)]
pub struct Proposta {
    pub colecao: uuid::Uuid,
    pub pasta: String,
    pub titulo_da_pasta: String,
    pub evidencia: EvidenciaDaPasta,
    pub provider: String,
    pub provider_id: String,
    pub candidato: String,
    pub ano: Option<i32>,
    pub score: f32,
    pub aplicaria: bool,
    pub motivos: Vec<String>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct PropostaStatus {
    pub running: bool,
    pub dry_run: bool,
    /// Pastas efetivamente aplicadas, e o que cada uma mexeu.
    pub aplicadas: Vec<serde_json::Value>,
    pub pastas: usize,
    pub consultadas: usize,
    pub com_proposta: usize,
    pub aplicariam: usize,
    /// Pastas cuja busca não devolveu candidato nenhum — nome que o provider
    /// não conhece. Não é erro, é resposta.
    pub sem_candidato: usize,
    pub propostas: Vec<Proposta>,
}

/// Percorre as pastas montadas pela R75 e propõe uma série pra cada uma.
///
/// **Uma busca e uma ficha por pasta**, não por arquivo: nas 485 de *A Grande
/// Família* isso é 2 chamadas contra 485.
pub async fn propor(
    estado: &crate::AppState,
    quem: uuid::Uuid,
    limite: usize,
    dry_run: bool,
) -> PropostaStatus {
    let pool = &estado.pool;
    let providers = &estado.providers;
    let mut s = PropostaStatus {
        running: true,
        dry_run,
        ..Default::default()
    };

    let pastas: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
        "SELECT id, title, provider_key FROM collection
          WHERE kind = 'series' AND origin = 'estrutura'
            AND provider_key LIKE 'estrutura:%'
          ORDER BY (SELECT count(*) FROM collection_item ci
                     WHERE ci.collection_id = collection.id
                        OR ci.collection_id IN (SELECT id FROM collection f
                                                 WHERE f.parent_id = collection.id)) DESC",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    s.pastas = pastas.len();

    for (colecao, titulo, chave) in pastas.into_iter().take(limite) {
        let dir = chave.trim_start_matches("estrutura:").to_string();
        let evidencia = evidencia_de(pool, colecao).await;
        let lido = ler_pasta(&titulo);
        let termo = if lido.title.trim().is_empty() {
            titulo.clone()
        } else {
            lido.title.clone()
        };

        // Guess sintético que representa a PASTA — episódio presente pra a
        // busca ir em séries, igual ao `scopes::search`.
        let busca = Guess {
            title: termo.clone(),
            year: lido.year,
            episode: Some(1),
            ..Default::default()
        };
        let candidatos = crate::metadata::search(providers, &busca, "auto").await;
        s.consultadas += 1;

        let mut evidencia = evidencia;
        evidencia.ano = lido.year;

        let mut melhor: Option<Proposta> = None;
        for candidato in candidatos.iter().filter(|c| c.provider_kind != "movie").take(3) {
            let temporadas = match (&providers.tmdb, candidato.provider.as_str()) {
                (Some(tmdb), "tmdb") => {
                    tmdb.series_seasons(&candidato.provider_id).await.unwrap_or_default()
                }
                _ => Vec::new(),
            };
            let p = pontuar(&evidencia, &termo, candidato, &temporadas);
            let proposta = Proposta {
                colecao,
                pasta: dir.clone(),
                titulo_da_pasta: termo.clone(),
                evidencia: evidencia.clone(),
                provider: candidato.provider.clone(),
                provider_id: candidato.provider_id.clone(),
                candidato: candidato.title.clone(),
                ano: candidato.year,
                score: p.valor,
                aplicaria: p.valor >= LIMIAR,
                motivos: p.motivos,
            };
            if melhor.as_ref().is_none_or(|m| proposta.score > m.score) {
                melhor = Some(proposta);
            }
        }

        match melhor {
            Some(p) => {
                s.com_proposta += 1;
                s.aplicariam += p.aplicaria as usize;

                // **Aplicar é o `scopes::aplicar`, e nada mais.** Uma segunda
                // implementação de "aplicar uma pasta" divergiria da manual —
                // e é a manual que tem a regra do §8b escrita dentro: arquivo
                // cujo episódio não resolve **não** vira `confirmed`.
                if p.aplicaria && !dry_run {
                    if let Some(library_id) = p.evidencia.library_id {
                        let pedido = crate::models::ScopeIdentify {
                            library_id,
                            dir_path: p.pasta.clone(),
                            recursive: true,
                            provider: p.provider.clone(),
                            provider_id: p.provider_id.clone(),
                            provider_kind: "tv".into(),
                            season_number: None,
                            numbering: "seasonal".into(),
                            absolute_offset: 0,
                            note: Some(format!(
                                "identificada pela pasta (R76), score {:.3}",
                                p.score
                            )),
                            dry_run: false,
                            force: false,
                        };
                        // `None` no job: este laço já roda dentro do job do
                        // `identificar-pastas`, e só cabe um ativo por `kind`
                        // (R81). Quem tica o progresso aqui é o job de fora.
                        match crate::routes::scopes::aplicar(estado, quem, pedido, None).await {
                            Ok(resumo) => s.aplicadas.push(resumo),
                            Err(e) => tracing::warn!(
                                erro = %e, pasta = p.pasta, "pasta não aplicou"
                            ),
                        }
                    }
                }

                s.propostas.push(p);
            }
            None => s.sem_candidato += 1,
        }
    }

    s.running = false;
    s
}

/// O que a pasta sabe: quantos arquivos, e quantos por temporada.
async fn evidencia_de(pool: &sqlx::PgPool, colecao: uuid::Uuid) -> EvidenciaDaPasta {
    let library: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT mf.library_id
           FROM collection_item ci
           JOIN media_file mf ON mf.work_id = ci.work_id
          WHERE ci.collection_id = $1
             OR ci.collection_id IN (SELECT id FROM collection WHERE parent_id = $1)
          LIMIT 1",
    )
    .bind(colecao)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let linhas: Vec<(Option<i32>, i64)> = sqlx::query_as(
        "SELECT w.season_number, count(*)
           FROM collection_item ci
           JOIN work w ON w.id = ci.work_id
          WHERE ci.collection_id = $1
             OR ci.collection_id IN (SELECT id FROM collection WHERE parent_id = $1)
          GROUP BY 1",
    )
    .bind(colecao)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let arquivos = linhas.iter().map(|(_, n)| *n).sum();
    let mut temporadas: Vec<(i32, i64)> = linhas
        .into_iter()
        .filter_map(|(t, n)| t.map(|t| (t, n)))
        .collect();
    temporadas.sort_by_key(|(t, _)| *t);

    EvidenciaDaPasta { arquivos, library_id: library, temporadas, ano: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidato(titulo: &str, ano: Option<i32>) -> Candidate {
        Candidate {
            provider: "tmdb".into(),
            provider_id: "1".into(),
            provider_kind: "tv".into(),
            title: titulo.into(),
            original_title: None,
            year: ano,
            overview: None,
            poster_url: None,
            backdrop_url: None,
            genres: vec![],
            accent_color: None,
            popularity: 0.0,
            raw: serde_json::Value::Null,
        }
    }

    fn temporadas(pares: &[(i32, i32)]) -> Vec<SeasonSummary> {
        pares
            .iter()
            .map(|(n, e)| SeasonSummary {
                season_number: *n,
                episode_count: *e,
                name: None,
                overview: None,
                poster_path: None,
                air_date: None,
            })
            .collect()
    }

    /// **O caso que motivou o módulo.** *A Grande Família*: 485 arquivos, 14
    /// temporadas. Como arquivo, o empate era de 0,0122; como pasta, não há
    /// empate.
    #[test]
    fn a_pasta_decide_o_que_o_arquivo_nao_decidia() {
        let ev = EvidenciaDaPasta { library_id: None, arquivos: 485,
            temporadas: (1..=14).map(|n| (n, 35)).collect(),
            ano: None,
        };
        let de_2001 = pontuar(
            &ev,
            "A Grande Família",
            &candidato("A Grande Família", Some(2001)),
            &temporadas(&(1..=14).map(|n| (n, 40)).collect::<Vec<_>>()),
        );
        let de_1972 = pontuar(
            &ev,
            "A Grande Família",
            &candidato("A Grande Família", Some(1972)),
            &temporadas(&[(1, 13), (2, 13), (3, 13), (4, 13)]),
        );
        assert!(
            de_2001.valor - de_1972.valor > 0.30,
            "{} vs {}",
            de_2001.valor,
            de_1972.valor
        );
        assert!(de_2001.valor >= LIMIAR, "a certa não passou: {}", de_2001.valor);
    }

    /// ⚠️ **Cabe, e não é igual.** Acervo incompleto é o normal; exigir
    /// igualdade puniria quem tem meia temporada.
    #[test]
    fn acervo_incompleto_nao_e_penalizado() {
        let ev = EvidenciaDaPasta { library_id: None, arquivos: 3,
            temporadas: vec![(1, 3)],
            ano: None,
        };
        let p = pontuar(&ev, "Severance", &candidato("Severance", None), &temporadas(&[(1, 9)]));
        assert!(p.motivos.iter().any(|m| m.contains("cabem")), "{:?}", p.motivos);
    }

    /// Uma pasta com mais episódios do que a série tem **não é aquela série**.
    #[test]
    fn pasta_que_estoura_a_temporada_e_recusada() {
        let ev = EvidenciaDaPasta { library_id: None, arquivos: 80,
            temporadas: vec![(1, 80)],
            ano: None,
        };
        let p = pontuar(&ev, "Popeye", &candidato("Popeye", None), &temporadas(&[(1, 20)]));
        assert!(p.valor < LIMIAR);
        assert!(p.motivos.iter().any(|m| m.contains("não encaixa")), "{:?}", p.motivos);
    }

    /// ⚠️ **Três arquivos não valem o mesmo que 485.** Uma pasta pequena
    /// "cabe" em quase qualquer série; o encaixe só é evidência com massa.
    #[test]
    fn pasta_pequena_nao_se_auto_confirma() {
        let poucos = EvidenciaDaPasta { library_id: None, arquivos: 3, temporadas: vec![(1, 3)], ano: None };
        let muitos = EvidenciaDaPasta { library_id: None, arquivos: 485, temporadas: vec![(1, 485)], ano: None };
        let provider = temporadas(&[(1, 500)]);
        let p = pontuar(&poucos, "Coisa", &candidato("Coisa", None), &provider);
        let m = pontuar(&muitos, "Coisa", &candidato("Coisa", None), &provider);
        assert!(p.valor < LIMIAR, "pasta de 3 passou sozinha: {}", p.valor);
        assert!(m.valor >= LIMIAR, "pasta de 485 não passou: {}", m.valor);
    }

    /// O ano da pasta vale tanto quanto o do arquivo — e a pasta o declara
    /// muito mais vezes.
    #[test]
    fn o_ano_da_pasta_conta() {
        let ev = EvidenciaDaPasta { library_id: None, arquivos: 249, temporadas: vec![(1, 24)], ano: Some(2009) };
        let certo = pontuar(&ev, "Modern Family", &candidato("Modern Family", Some(2009)), &temporadas(&[(1, 24)]));
        let errado = pontuar(&ev, "Modern Family", &candidato("Modern Family", Some(1998)), &temporadas(&[(1, 24)]));
        assert!(
            certo.valor - errado.valor >= 0.35,
            "{} vs {}",
            certo.valor,
            errado.valor
        );
    }

    /// O nome da pasta é lido pelo mesmo parser do nome de arquivo — senão a
    /// pasta e o arquivo passam a discordar sobre o que está escrito nelas.
    #[test]
    fn a_pasta_e_lida_pelo_mesmo_parser() {
        let g = ler_pasta("Modern Family (2009)");
        assert_eq!(g.title, "Modern Family");
        assert_eq!(g.year, Some(2009));
    }
}
