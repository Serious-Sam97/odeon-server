pub mod embedding;
pub mod taste;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::models::WorkListItem;
use taste::TasteProfile;

/// Quantos candidatos o banco entrega pra pontuação fina em Rust. Ordenados por
/// similaridade quando há vetor de gosto, então o corte não é arbitrário.
const CANDIDATE_POOL: i64 = 400;

/// "Tenho 40 minutos" não pode devolver um filme de 3h. 1.5x dá folga pros
/// créditos e pra quem arredondou o tempo disponível.
const TIME_BUDGET_SLACK: f64 = 1.5;

#[derive(Debug, Clone, Default, Serialize)]
pub struct EmbedStatus {
    pub running: bool,
    pub total: u64,
    pub done: u64,
    pub skipped: u64,
    pub corpus_terms: u64,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub errors: Vec<String>,
}

pub type SharedEmbedStatus = Arc<Mutex<EmbedStatus>>;

#[derive(Debug, Clone, Serialize)]
pub struct Recommendation {
    #[serde(flatten)]
    pub work: WorkListItem,
    pub score: f32,
    /// Mesma filosofia do M1: a máquina nunca decide em silêncio.
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Context {
    /// Tempo disponível, em minutos.
    pub minutes: Option<i32>,
    /// Valor da tag `mood:` — "melancólico", "leve"…
    pub mood: Option<String>,
    pub include_finished: bool,
    pub limit: i64,
}

// ---------------------------------------------------------------- corpus

#[derive(Debug, sqlx::FromRow)]
struct Document {
    id: Uuid,
    text: String,
}

/// Texto de cada obra: título, título original, sinopse, tags e nome da série.
const DOCUMENTS_SQL: &str = r#"
SELECT w.id,
       concat_ws(' ',
           w.title,
           w.original_title,
           w.overview,
           (SELECT string_agg(t.value, ' ')
              FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
             WHERE wt.work_id = w.id),
           (SELECT max(c.title)
              FROM collection_item ci JOIN collection c ON c.id = ci.collection_id
             WHERE ci.work_id = w.id AND c.kind IN ('series', 'season'))
       ) AS text
FROM work w
"#;

/// Reconstrói o IDF do corpus e reembute todas as obras.
///
/// As duas coisas juntas de propósito: mudar o IDF sem reembutir deixaria
/// vetores antigos e novos em escalas diferentes, e o cosseno entre eles
/// passaria a mentir.
pub async fn rebuild(pool: PgPool, status: SharedEmbedStatus) -> bool {
    {
        let mut s = status.lock().await;
        if s.running {
            return false;
        }
        *s = EmbedStatus {
            running: true,
            started_at: Some(Utc::now()),
            ..Default::default()
        };
    }

    let outcome = rebuild_inner(&pool, &status).await;

    let mut s = status.lock().await;
    if let Err(e) = outcome {
        s.errors.push(e.to_string());
    }
    s.running = false;
    s.finished_at = Some(Utc::now());
    tracing::info!(done = s.done, termos = s.corpus_terms, "embeddings prontos");
    true
}

async fn rebuild_inner(pool: &PgPool, status: &SharedEmbedStatus) -> anyhow::Result<()> {
    let documents: Vec<Document> = sqlx::query_as(DOCUMENTS_SQL).fetch_all(pool).await?;
    status.lock().await.total = documents.len() as u64;

    // 1ª passada: quantos documentos contêm cada termo.
    let mut document_frequency: HashMap<String, u32> = HashMap::new();
    let mut tokenized: Vec<(Uuid, HashMap<String, f32>)> = Vec::with_capacity(documents.len());

    for document in &documents {
        let tokens = embedding::tokenize(&document.text);
        let frequencies = embedding::term_frequencies(&tokens);
        for term in frequencies.keys() {
            *document_frequency.entry(term.clone()).or_insert(0) += 1;
        }
        tokenized.push((document.id, frequencies));
    }

    // IDF suavizado: `ln(N / (1 + df)) + 1` nunca fica negativo nem explode
    // quando um termo aparece em todos os documentos.
    let total_documents = documents.len().max(1) as f32;
    let idf: HashMap<String, f32> = document_frequency
        .iter()
        .map(|(term, count)| {
            let value = (total_documents / (1.0 + *count as f32)).ln() + 1.0;
            (term.clone(), value.max(0.0))
        })
        .collect();

    // Persiste o IDF pra embutir uma obra nova depois sem reprocessar tudo.
    let mut tx = pool.begin().await?;
    sqlx::query("TRUNCATE corpus_term").execute(&mut *tx).await?;
    for (term, count) in &document_frequency {
        sqlx::query(
            "INSERT INTO corpus_term (term, document_count, idf) VALUES ($1, $2, $3)
             ON CONFLICT (term) DO UPDATE SET document_count = EXCLUDED.document_count,
                                             idf = EXCLUDED.idf",
        )
        .bind(term)
        .bind(*count as i32)
        .bind(idf.get(term).copied().unwrap_or(1.0))
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE corpus_stats SET document_count = $1, built_at = now() WHERE id = 1")
        .bind(documents.len() as i32)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    status.lock().await.corpus_terms = document_frequency.len() as u64;

    // 2ª passada: o vetor de cada obra.
    for (work_id, frequencies) in tokenized {
        match embedding::embed_document(&frequencies, &idf) {
            Some(vector) => {
                sqlx::query(
                    "UPDATE work SET embedding = $2::vector, embedded_at = now() WHERE id = $1",
                )
                .bind(work_id)
                .bind(embedding::to_pg_vector(&vector))
                .execute(pool)
                .await?;
                status.lock().await.done += 1;
            }
            None => {
                // Obra sem texto útil (só nome de arquivo cru, por exemplo).
                status.lock().await.skipped += 1;
            }
        }
    }

    Ok(())
}

// -------------------------------------------------------- recomendação

#[derive(Debug, sqlx::FromRow)]
struct CandidateRow {
    #[sqlx(flatten)]
    work: WorkListItem,
    similarity: Option<f64>,
    feedback: Option<String>,
    people: Option<Vec<String>>,
}

/// O miolo, compartilhado pelos dois caminhos de recomendação.
///
/// `{origem}`, `{similaridade}` e `{ordem}` são os únicos pontos que variam —
/// e são preenchidos por constantes do próprio código, nunca por entrada do
/// usuário.
const CANDIDATES_BODY: &str = r#"
    w.id, w.kind, w.title, w.year, w.season_number, w.episode_number,
    w.match_state, w.match_confidence, w.dominant_color,
    w.artwork->>'poster' AS poster,
    w.artwork->>'backdrop' AS backdrop,
    w.artwork->>'still' AS still,
    s.series_title,
    f.id AS media_file_id, f.duration_seconds, f.width, f.height,
    f.video_codec, f.audio_codec, f.container, f.size_bytes,
    ps.position_seconds, ps.finished,
    tg.tags,
    pc.people,
    {similaridade} AS similarity,
    fb.verdict AS feedback
FROM {origem}
JOIN LATERAL (
    SELECT m.* FROM media_file m
    WHERE m.work_id = w.id AND m.status = 'probed'
    ORDER BY m.size_bytes DESC LIMIT 1
) f ON true
LEFT JOIN LATERAL (
    SELECT COALESCE(series.title, season.title) AS series_title
    FROM collection_item ci
    JOIN collection season ON season.id = ci.collection_id
    LEFT JOIN collection series ON series.id = season.parent_id
    WHERE ci.work_id = w.id AND season.kind IN ('season', 'series')
    LIMIT 1
) s ON true
LEFT JOIN LATERAL (
    SELECT array_agg(t.namespace || ':' || t.value ORDER BY t.namespace, t.value) AS tags
    FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
    WHERE wt.work_id = w.id
) tg ON true
LEFT JOIN LATERAL (
    SELECT array_agg(c.person_id::text) AS people
    FROM credit c JOIN credit_role r ON r.role = c.role AND r.featured
    WHERE c.work_id = w.id
) pc ON true
LEFT JOIN playback_state ps ON ps.work_id = w.id AND ps.user_id = $1
LEFT JOIN work_feedback fb ON fb.work_id = w.id AND fb.user_id = $1
WHERE COALESCE(fb.verdict, '') <> 'block'
  AND ($3 OR COALESCE(ps.finished, false) = false)
  -- Recomendar exige saber O QUE se está recomendando.
  --
  -- Sem estas duas linhas, 12 dos 24 primeiros itens eram nome de arquivo com
  -- um score do lado: "310 Fly", "055 - Rambo - The Video Game : A Primeira
  -- FAIL Hora". Pior, 1.234 obras `ignored` — que a biblioteca esconde desde a
  -- R3 — apareciam aqui como sugestão.
  --
  -- O corte deixa 8.596 obras, o que é acervo de sobra. E não é escolha
  -- editorial: material sem identificação se resolve na fila de revisão, que é
  -- onde ele mora.
  AND w.match_state IN ('auto', 'confirmed')
  AND w.artwork ? 'poster'
ORDER BY {ordem}
"#;

/// Com vetor de gosto: a busca por vizinhança vem ANTES de tudo, e sozinha.
///
/// A forma anterior era `ORDER BY 1 - (embedding <=> $2) DESC, updated_at DESC`
/// com um `CASE` em volta. Três coisas ali impedem o pgvector de usar o índice
/// HNSW ao mesmo tempo: o `CASE`, a inversão `1 - (...)` e a segunda chave de
/// ordenação. Confirmado com EXPLAIN nesta máquina:
///
///     antes:  Seq Scan on work (17.498 linhas) + Sort
///     agora:  Index Scan using work_embedding_idx
///
/// O ganho é duplo: o índice entra, E os quatro `JOIN LATERAL` (melhor arquivo,
/// título da série, tags, pessoas) deixam de rodar para 17 mil linhas antes de
/// ordenar — passam a rodar só para as 400 que sobrevivem.
fn candidates_sql() -> String {
    format!(
        "WITH near AS (
            SELECT id, embedding <=> $2::vector AS distance
            FROM work
            WHERE embedding IS NOT NULL
            ORDER BY embedding <=> $2::vector
            LIMIT $4
        )
        SELECT {}",
        CANDIDATES_BODY
            .replace("{origem}", "near JOIN work w ON w.id = near.id")
            .replace("{similaridade}", "1 - near.distance")
            .replace("{ordem}", "near.distance ASC")
    )
}

/// Sem vetor de gosto — usuário novo, ou biblioteca sem embeddings.
///
/// Caminho separado de propósito. A versão anterior resolvia os dois casos com
/// `NULLIF($2,'')::vector`, e era esse truque que forçava o `CASE` no `ORDER BY`
/// e matava o índice para TODO mundo. Um caminho não deve pagar pelo outro.
fn candidates_cold_sql() -> String {
    format!(
        "SELECT {} LIMIT $4",
        CANDIDATES_BODY
            .replace("{origem}", "work w")
            .replace("{similaridade}", "NULL::float8")
            .replace("{ordem}", "w.updated_at DESC")
    )
}

pub async fn recommend(
    pool: &PgPool,
    user_id: Uuid,
    context: &Context,
) -> anyhow::Result<(TasteProfile, Vec<Recommendation>)> {
    let profile = taste::build(pool, user_id).await?;

    let taste_literal = profile
        .taste_vector
        .as_ref()
        .map(|v| embedding::to_pg_vector(v))
        .unwrap_or_default();

    // Sem vetor não há vizinhança a consultar: a query fria nem toca no
    // operador de distância.
    let sql = if taste_literal.is_empty() {
        candidates_cold_sql()
    } else {
        candidates_sql()
    };

    let candidates: Vec<CandidateRow> = sqlx::query_as(&sql)
        .bind(user_id)
        .bind(&taste_literal)
        .bind(context.include_finished)
        .bind(CANDIDATE_POOL)
        .fetch_all(pool)
        .await?;

    let mut scored: Vec<Recommendation> = candidates
        .into_iter()
        .filter_map(|candidate| score(candidate, &profile, context))
        .collect();

    scored.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut scored = diversify(scored);
    scored.truncate(context.limit.clamp(1, 100) as usize);

    Ok((profile, scored))
}

/// No máximo duas obras da mesma série na frente da fila.
const MAX_POR_SERIE: usize = 2;

/// Espalha as séries pelo topo da lista.
///
/// **Isto não é correção de pontuação — o score está certo.** Um perfil
/// concentrado (alguém que terminou 200 episódios da mesma série) faz o vizinho
/// mais próximo do vetor de gosto ser sempre aquela série, e as cinco primeiras
/// posições viram vitrine de um produto só. Repetição é problema de
/// APRESENTAÇÃO, e é por isso que a correção mora aqui e não no `score()`:
/// mexer no score pra resolver layout estragaria o "por quê" de cada item, que
/// é o que a tela promete.
///
/// O excedente é **empurrado pro fim, nunca descartado**. Numa biblioteca pouco
/// variada pode não existir o que colocar no lugar, e devolver meia tela vazia
/// seria pior do que repetir. Obra sem série — filme — nunca agrupa.
fn diversify(scored: Vec<Recommendation>) -> Vec<Recommendation> {
    let mut por_serie: HashMap<String, usize> = HashMap::new();
    let mut frente: Vec<Recommendation> = Vec::with_capacity(scored.len());
    let mut excedente: Vec<Recommendation> = Vec::new();

    for rec in scored {
        match rec.work.series_title.as_deref() {
            Some(serie) => {
                let vistos = por_serie.entry(serie.to_owned()).or_insert(0);
                if *vistos < MAX_POR_SERIE {
                    *vistos += 1;
                    frente.push(rec);
                } else {
                    excedente.push(rec);
                }
            }
            // Filme, stand-up, documentário solto: não há do que se repetir.
            None => frente.push(rec),
        }
    }

    // Os dois lados já vêm ordenados por score, então concatenar preserva a
    // ordem dentro de cada grupo.
    frente.extend(excedente);
    frente
}

/// A pontuação. Devolve `None` quando a obra deve sumir da lista (não cabe no
/// tempo disponível) em vez de aparecer no fim — oferecer o impossível é ruído.
fn score(
    candidate: CandidateRow,
    profile: &TasteProfile,
    context: &Context,
) -> Option<Recommendation> {
    let work = candidate.work;
    let tags = work.tags.clone().unwrap_or_default();
    let minutes = work.duration_seconds.map(|d| (d / 60.0).round() as i32);

    let mut reasons: Vec<String> = Vec::new();
    let mut total = 0.0f32;

    // --- 1. tempo disponível: filtro duro antes de qualquer gosto ----------
    if let (Some(available), Some(duration)) = (context.minutes, minutes) {
        if (duration as f64) > available as f64 * TIME_BUDGET_SLACK {
            return None;
        }
        if duration <= available {
            total += 0.20;
            reasons.push(format!("cabe nos seus {available} min ({duration} min)"));
        } else {
            total += 0.08;
            reasons.push(format!("passa um pouco: {duration} min"));
        }
    }

    // --- 2. humor pedido --------------------------------------------------
    if let Some(mood) = &context.mood {
        let wanted = format!("mood:{mood}");
        if tags.iter().any(|t| t.eq_ignore_ascii_case(&wanted)) {
            total += 0.18;
            reasons.push(format!("marcada como {mood}"));
        }
    }

    // Sem histórico não há o que curar; a lista vira catálogo e a UI diz isso.
    if profile.is_cold_start() {
        total += 0.05;
        reasons.push("ainda sem histórico pra personalizar".to_string());
        return Some(Recommendation {
            work,
            score: total,
            reasons,
        });
    }

    // --- 3. afinidade por tag --------------------------------------------
    if !tags.is_empty() {
        let affinities: Vec<(String, f32)> = tags
            .iter()
            .map(|tag| (tag.clone(), profile.affinity_of(tag)))
            .filter(|(_, value)| value.abs() > 0.05)
            .collect();

        if !affinities.is_empty() {
            let mean =
                affinities.iter().map(|(_, v)| *v).sum::<f32>() / affinities.len() as f32;
            total += mean * 0.35;

            if let Some((tag, value)) = affinities
                .iter()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|(_, v)| *v > 0.2)
            {
                let label = tag.split_once(':').map(|(_, v)| v).unwrap_or(tag);
                let percent = value * 100.0;
                reasons.push(format!("você costuma terminar {label} ({percent:.0}%)"));
            }
            if let Some((tag, _)) = affinities
                .iter()
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|(_, v)| *v < -0.3)
            {
                let label = tag.split_once(':').map(|(_, v)| v).unwrap_or(tag);
                reasons.push(format!("mas você larga {label} com frequência"));
            }
        }
    }

    // --- 3b. quem trabalhou nisso ----------------------------------------
    // Nome forte é um sinal mais nítido que gênero: "tudo do Villeneuve" é uma
    // preferência de verdade, "drama" quase não é.
    if let Some(people) = &candidate.people {
        let mut best: Option<&crate::curation::taste::PersonAffinity> = None;
        for id in people {
            let Ok(uuid) = id.parse::<Uuid>() else { continue };
            if let Some(affinity) = profile.person_score(uuid) {
                if best.is_none_or(|current| affinity.score > current.score) {
                    best = Some(affinity);
                }
            }
        }
        if let Some(affinity) = best.filter(|a| a.score.abs() > 0.2) {
            total += affinity.score * 0.22;
            if affinity.score > 0.0 {
                reasons.push(format!(
                    "você terminou {} obras com {}",
                    affinity.works, affinity.name
                ));
            } else {
                reasons.push(format!("você costuma largar coisas com {}", affinity.name));
            }
        }
    }

    // --- 4. parecido com o que você gostou --------------------------------
    if let Some(similarity) = candidate.similarity {
        let value = similarity.clamp(0.0, 1.0) as f32;
        total += value * 0.30;
        if value > 0.35 {
            reasons.push(format!("parece com o que você gosta ({:.0}%)", value * 100.0));
        }
    }

    // --- 5. duração que você de fato termina ------------------------------
    if let (Some((low, high)), Some(duration)) = (profile.preferred_minutes, minutes) {
        if duration >= low && duration <= high {
            total += 0.08;
            reasons.push(format!("na duração que você termina ({low}–{high} min)"));
        }
    }

    // --- 6. largado no meio ------------------------------------------------
    if let (Some(position), Some(duration)) = (work.position_seconds, work.duration_seconds) {
        let ratio = if duration > 0.0 { position / duration } else { 0.0 };
        if (0.05..0.9).contains(&ratio) {
            total += 0.15;
            let left = ((duration - position) / 60.0).round() as i32;
            reasons.push(format!("você parou faltando {left} min"));
        }
    }

    // --- 7. feedback explícito vence heurística ---------------------------
    if candidate.feedback.as_deref() == Some("love") {
        total += 0.25;
        reasons.push("você marcou como favorita".to_string());
    }

    if reasons.is_empty() {
        reasons.push("da sua biblioteca, ainda não assistida".to_string());
    }

    Some(Recommendation {
        work,
        score: total,
        reasons,
    })
}

/// "Por que você assistiu X" — puro pgvector, sem histórico envolvido.
pub async fn similar(
    pool: &PgPool,
    user_id: Uuid,
    work_id: Uuid,
    limit: i64,
) -> anyhow::Result<Vec<Recommendation>> {
    let source: Option<String> =
        sqlx::query_scalar("SELECT embedding::text FROM work WHERE id = $1")
            .bind(work_id)
            .fetch_optional(pool)
            .await?
            .flatten();

    let Some(literal) = source else {
        return Ok(Vec::new());
    };

    let rows: Vec<CandidateRow> = sqlx::query_as(&candidates_sql())
    .bind(user_id)
    .bind(&literal)
    .bind(true)
    .bind(limit + 1)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        // a própria obra é sempre o vizinho mais próximo de si mesma
        .filter(|row| row.work.id != work_id)
        .take(limit as usize)
        .map(|row| {
            let value = row.similarity.unwrap_or(0.0).clamp(0.0, 1.0) as f32;
            Recommendation {
                work: row.work,
                score: value,
                reasons: vec![format!("{:.0}% de vocabulário em comum", value * 100.0)],
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(titulo: &str, serie: Option<&str>, score: f32) -> Recommendation {
        Recommendation {
            work: WorkListItem {
                id: Uuid::nil(),
                kind: "episode".into(),
                title: titulo.into(),
                year: None,
                season_number: None,
                episode_number: None,
                match_state: "auto".into(),
                match_confidence: None,
                dominant_color: None,
                poster: None,
                backdrop: None,
                still: None,
                series_title: serie.map(|s| s.to_string()),
                media_file_id: None,
                duration_seconds: None,
                width: None,
                height: None,
                video_codec: None,
                audio_codec: None,
                container: None,
                size_bytes: None,
                position_seconds: None,
                finished: None,
                tags: None,
            },
            score,
            reasons: vec![],
        }
    }

    fn titulos(v: &[Recommendation]) -> Vec<&str> {
        v.iter().map(|r| r.work.title.as_str()).collect()
    }

    #[test]
    fn no_maximo_duas_da_mesma_serie_na_frente() {
        let entrada = vec![
            rec("raven 1", Some("Raven"), 0.9),
            rec("raven 2", Some("Raven"), 0.8),
            rec("raven 3", Some("Raven"), 0.7),
            rec("simpsons 1", Some("Simpsons"), 0.6),
        ];
        assert_eq!(
            titulos(&diversify(entrada)),
            ["raven 1", "raven 2", "simpsons 1", "raven 3"]
        );
    }

    #[test]
    fn excedente_e_empurrado_e_nunca_descartado() {
        let entrada = vec![
            rec("a", Some("X"), 0.9),
            rec("b", Some("X"), 0.8),
            rec("c", Some("X"), 0.7),
            rec("d", Some("X"), 0.6),
        ];
        // Biblioteca de uma série só: a lista continua inteira, só reordenada
        // — meia tela vazia seria pior que repetir.
        assert_eq!(diversify(entrada).len(), 4);
    }

    #[test]
    fn filme_nao_agrupa() {
        // `series_title` nulo é filme: três seguidos continuam os três na frente.
        let entrada = vec![
            rec("filme 1", None, 0.9),
            rec("filme 2", None, 0.8),
            rec("filme 3", None, 0.7),
        ];
        assert_eq!(titulos(&diversify(entrada)), ["filme 1", "filme 2", "filme 3"]);
    }

    #[test]
    fn ordem_de_score_e_preservada_dentro_de_cada_grupo() {
        let entrada = vec![
            rec("x1", Some("X"), 0.9),
            rec("y1", Some("Y"), 0.85),
            rec("x2", Some("X"), 0.8),
            rec("x3", Some("X"), 0.75),
            rec("x4", Some("X"), 0.70),
        ];
        assert_eq!(titulos(&diversify(entrada)), ["x1", "y1", "x2", "x3", "x4"]);
    }
}
