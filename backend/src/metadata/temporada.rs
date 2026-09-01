//! R63 — a temporada como coisa, e não como número.
//!
//! ## O que faltava
//!
//! A ficha de série desenha uma fileira de temporadas. Medido em 18/08/2026:
//! **473 temporadas no acervo, zero com pôster e zero com sinopse.** O que
//! existia era `title = "Temporada N"`, gerado pelo próprio scanner, e o
//! `position`. O cliente contornava com o `still` do primeiro episódio — a
//! cena de um episódio fazendo as vezes de capa da temporada.
//!
//! O dado existe no TMDB e o cano inteiro já estava construído: a série tem
//! `external_ids->>'tmdb'`, a `collection` tem `artwork` e `overview` desde o
//! M1, e o `artwork::fetch` baixa, guarda e extrai a cor dominante. Faltava
//! preencher — como no §32 das sagas, é dívida de dados e não de schema.
//!
//! ## Uma chamada por série, não por temporada
//!
//! O `/tv/{id}` do TMDB devolve `seasons[]` com `name`, `overview`,
//! `poster_path` e `air_date` de **todas** as temporadas de uma vez. São 120
//! chamadas contra as 473 que o `/tv/{id}/season/{n}` custaria — e as duas
//! devolvem o mesmo `poster_path`. É o mesmo argumento que o `season()` do
//! `tmdb.rs` já usa pra identificar episódio em lote.
//!
//! ## A retomada sai de graça
//!
//! O alvo é "temporada de série identificada que ainda não tem pôster". Uma
//! segunda rodada continua de onde parou sem coluna de controle — o mesmo
//! truque do `repair-series` (§21), da trivia, da produção (§38) e das sagas
//! (§32).
//!
//! **O falso negativo é assumido**, e aqui ele tem nome: a temporada 0
//! (especiais) costuma vir sem pôster no TMDB, e vai ser perguntada em toda
//! rodada. Marcá-la exigiria uma tabela pra guardar ausência — schema pra
//! registrar que algo não existe, que é o que o §38 recusou.
//!
//! ## O que este job NÃO faz
//!
//! **Não renomeia temporada que alguém nomeou.** O `title` só é trocado quando
//! ele é exatamente o `"Temporada {n}"` que o scanner gerou; qualquer outro
//! nome é decisão de gente e fica. Sem essa guarda, uma rodada do job
//! desfaria toda correção manual do acervo — e a correção manual é o único
//! dado aqui que ninguém consegue reproduzir.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::jobs::Job;
use crate::metadata::Providers;

/// Quantas séries por bloco antes de respirar. Igual ao das sagas: o TMDB
/// tolera bem mais, e a barra de progresso quer andar.
const LOTE: usize = 10;
const INTERVALO: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Debug, Default, Clone, Serialize)]
pub struct TemporadaStatus {
    pub running: bool,
    pub total: usize,
    pub feitos: usize,
    /// Temporadas que ganharam pôster local.
    pub capas: usize,
    /// Temporadas que ganharam sinopse.
    pub sinopses: usize,
    /// Temporadas que ganharam nome próprio (`"Livro Um: Água"`).
    pub nomes: usize,
    /// Temporadas que o TMDB não descreve — a 0 de especiais é a campeã.
    pub sem_ficha: usize,
    pub falhas: usize,
    pub atual: Option<String>,
}

/// Uma série a visitar, com as temporadas dela que ainda não têm capa.
struct Alvo {
    tmdb: String,
    titulo: String,
    temporadas: Vec<(Uuid, i32, String)>,
}

pub async fn aquecer(
    pool: PgPool,
    providers: Providers,
    artwork_dir: PathBuf,
    mut job: Option<Job>,
) {
    let Some(tmdb) = providers.tmdb.clone() else {
        tracing::warn!("sem chave do TMDB — as temporadas não podem ser buscadas");
        if let Some(j) = job.take() {
            j.finish(
                &TemporadaStatus::default(),
                "failed",
                Some("sem chave do TMDB".into()),
            )
            .await;
        }
        return;
    };

    // `position` é o número da temporada; ele é a chave que casa a nossa linha
    // com a do provider. Temporada sem `position` não tem como ser casada, e
    // chutar pela ordem alfabética do título seria adivinhação.
    let linhas: Vec<(String, String, Uuid, i32, String)> = sqlx::query_as(
        "SELECT serie.external_ids->>'tmdb', serie.title,
                temporada.id, temporada.position, temporada.title
         FROM collection temporada
         JOIN collection serie ON serie.id = temporada.parent_id
         WHERE temporada.kind = 'season'
           AND temporada.position IS NOT NULL
           AND NOT (temporada.artwork ? 'poster')
           AND serie.external_ids ? 'tmdb'
         ORDER BY serie.title, temporada.position",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    // Agrupa por série pra pagar uma chamada por série, não por temporada.
    let mut alvos: Vec<Alvo> = Vec::new();
    for (tmdb_id, serie_titulo, id, numero, titulo) in linhas {
        match alvos.last_mut() {
            Some(a) if a.tmdb == tmdb_id => a.temporadas.push((id, numero, titulo)),
            _ => alvos.push(Alvo {
                tmdb: tmdb_id,
                titulo: serie_titulo,
                temporadas: vec![(id, numero, titulo)],
            }),
        }
    }

    let mut s = TemporadaStatus {
        running: true,
        total: alvos.len(),
        ..Default::default()
    };

    for bloco in alvos.chunks(LOTE) {
        // Cancelamento cooperativo no ponto seguro (§12).
        if matches!(&job, Some(j) if j.cancelled().await) {
            s.running = false;
            if let Some(j) = job.take() {
                j.finish(&s, "cancelled", None).await;
            }
            tracing::info!(feitos = s.feitos, "busca de temporadas cancelada");
            return;
        }

        s.atual = bloco.first().map(|a| a.titulo.clone());

        for alvo in bloco {
            let fichas = match tmdb.series_seasons(&alvo.tmdb).await {
                Ok(f) => f,
                Err(e) => {
                    s.falhas += 1;
                    tracing::warn!(error = %e, serie = alvo.titulo, "temporadas não vieram");
                    continue;
                }
            };

            for (id, numero, titulo_atual) in &alvo.temporadas {
                let Some(ficha) = fichas.iter().find(|f| f.season_number == *numero) else {
                    s.sem_ficha += 1;
                    continue;
                };
                let escrito =
                    aplicar(&pool, &providers, &artwork_dir, *id, *numero, titulo_atual, ficha)
                        .await;
                s.capas += escrito.capa as usize;
                s.sinopses += escrito.sinopse as usize;
                s.nomes += escrito.nome as usize;
                if !escrito.algo() {
                    s.sem_ficha += 1;
                }
            }
        }

        s.feitos += bloco.len();
        if let Some(j) = &job {
            j.tick(&s, s.feitos as i64, Some(s.total as i64)).await;
        }
        tokio::time::sleep(INTERVALO).await;
    }

    s.running = false;
    s.atual = None;
    tracing::info!(
        capas = s.capas,
        sinopses = s.sinopses,
        nomes = s.nomes,
        sem_ficha = s.sem_ficha,
        "temporadas aquecidas"
    );
    if let Some(j) = job {
        j.finish(&s, "succeeded", None).await;
    }
}

#[derive(Default)]
struct Escrito {
    capa: bool,
    sinopse: bool,
    nome: bool,
}

impl Escrito {
    fn algo(&self) -> bool {
        self.capa || self.sinopse || self.nome
    }
}

/// O nome que o scanner gera sozinho. Só ele pode ser substituído.
fn nome_automatico(numero: i32) -> String {
    format!("Temporada {numero}")
}

/// **`"Temporada 3"` do TMDB não é nome, é o mesmo número por extenso.**
///
/// A tradução pt-BR do TMDB devolve exatamente isso na maioria das séries, e
/// gravá-lo seria trocar a nossa string pela idêntica dele — trabalho, log e
/// risco por nada. Só nome de verdade (`"Livro Um: Água"`) passa.
fn nome_proprio(ficha_name: Option<&str>, numero: i32) -> Option<String> {
    let nome = ficha_name?.trim();
    if nome.is_empty() || nome == nome_automatico(numero) || nome == format!("Season {numero}") {
        return None;
    }
    Some(nome.to_string())
}

async fn aplicar(
    pool: &PgPool,
    providers: &Providers,
    artwork_dir: &Path,
    id: Uuid,
    numero: i32,
    titulo_atual: &str,
    ficha: &crate::metadata::tmdb::SeasonSummary,
) -> Escrito {
    use crate::metadata::tmdb::{url_de_imagem, POSTER};

    let mut escrito = Escrito::default();
    let mut arte = serde_json::Map::new();
    let mut cor: Option<String> = None;

    if let Some(caminho) = ficha.poster_path.as_deref() {
        let url = url_de_imagem(caminho, POSTER);
        match crate::artwork::fetch(&providers.http, artwork_dir, id, "poster", &url).await {
            Ok(guardada) => {
                arte.insert("poster".into(), guardada.path.into());
                cor = guardada.dominant_color;
                escrito.capa = true;
            }
            Err(e) => {
                // Mesma escolha da R38: guardar o caminho remoto perderia o
                // alvo do job — `NOT (artwork ? 'poster')` deixaria de casar e
                // a temporada nunca mais seria tentada. Aqui a ausência é o
                // que a torna retomável.
                tracing::warn!(error = %e, "pôster da temporada não baixou");
            }
        }
    }

    // O nome só troca quando o que está lá é o gerado pelo scanner.
    let nome = nome_proprio(ficha.name.as_deref(), numero)
        .filter(|_| titulo_atual == nome_automatico(numero));
    escrito.nome = nome.is_some();

    let sinopse = ficha
        .overview
        .as_deref()
        .map(str::trim)
        .filter(|o| !o.is_empty());
    escrito.sinopse = sinopse.is_some();

    if !escrito.algo() {
        return escrito;
    }

    // `||` no `artwork` e `COALESCE` no resto: o provider preenche o que falta
    // e não derruba o que existe — a regra do §21, aplicada de novo.
    let update = sqlx::query(
        "UPDATE collection
            SET artwork        = artwork || $2,
                dominant_color = COALESCE(dominant_color, $3),
                title          = COALESCE($4, title),
                overview       = COALESCE(overview, $5),
                year           = COALESCE(year, $6)
          WHERE id = $1",
    )
    .bind(id)
    .bind(serde_json::Value::Object(arte))
    .bind(cor.as_deref())
    .bind(nome.as_deref())
    .bind(sinopse)
    .bind(ano_de(ficha.air_date.as_deref()))
    .execute(pool)
    .await;

    if let Err(e) = update {
        tracing::warn!(error = %e, "temporada não gravou");
        return Escrito::default();
    }
    escrito
}

/// `"2021-11-06"` → `2021`.
fn ano_de(data: Option<&str>) -> Option<i32> {
    data.filter(|d| d.len() >= 4)?[..4].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O TMDB pt-BR chama a maioria das temporadas de "Temporada N" — que é
    /// exatamente o que o scanner já escreveu. Gravar seria trocar a string
    /// pela idêntica.
    #[test]
    fn temporada_n_nao_e_nome_proprio() {
        assert_eq!(nome_proprio(Some("Temporada 3"), 3), None);
        assert_eq!(nome_proprio(Some("Season 3"), 3), None);
        assert_eq!(nome_proprio(Some("  "), 3), None);
        assert_eq!(nome_proprio(None, 3), None);
    }

    /// Nome de verdade passa — e é o caso que faz o campo valer a pena.
    #[test]
    fn nome_de_verdade_passa() {
        assert_eq!(
            nome_proprio(Some("Livro Um: Água"), 1).as_deref(),
            Some("Livro Um: Água")
        );
        // "Temporada 2" quando a temporada é a 3 é nome, não numeração nossa.
        assert_eq!(nome_proprio(Some("Temporada 2"), 3).as_deref(), Some("Temporada 2"));
    }

    #[test]
    fn o_ano_sai_da_estreia() {
        assert_eq!(ano_de(Some("2021-11-06")), Some(2021));
        assert_eq!(ano_de(Some("")), None);
        assert_eq!(ano_de(None), None);
    }

    /// A guarda que impede o job de desfazer correção manual: o nome só troca
    /// quando o que está no banco é o gerado pelo scanner.
    #[test]
    fn nome_posto_por_gente_nao_e_sobrescrito() {
        let do_provider = nome_proprio(Some("Livro Um: Água"), 1);
        let manual = do_provider.clone().filter(|_| "Arco do Ninja" == nome_automatico(1));
        assert_eq!(manual, None, "um título editado à mão foi trocado");
        let automatico = do_provider.filter(|_| nome_automatico(1) == nome_automatico(1));
        assert!(automatico.is_some());
    }
}
