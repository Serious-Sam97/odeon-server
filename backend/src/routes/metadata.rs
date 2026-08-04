//! Identificação e fila de revisão.
//!
//! A diferença de filosofia em relação ao Jellyfin vive aqui: quando o matcher
//! não tem certeza, ele **não escreve nada** na obra. Ele grava os candidatos
//! com o score e os motivos, marca `needs_review`, e espera um humano.

use axum::extract::{Path as AxumPath, Query, State};
use axum::Json;
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::{AppError, AppResult};
use crate::metadata::{self, score, Candidate};
use crate::models::{
    BulkMatch, BulkState, ConfirmMatch, GuessView, ManualSearch, MatchCandidateRow, MatchRequest,
    ReparseParams, ReviewItem, ReviewQuery, ReviewWork,
};
use crate::scanner::guess::{guess_from_path, Guess};
use crate::AppState;

const CANDIDATE_COLUMNS: &str = "id, work_id, provider, provider_id, provider_kind, title,
     original_title, year, overview, poster_url, backdrop_url, score, reasons";

pub async fn start(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<MatchRequest>,
) -> AppResult<Json<Value>> {
    if state.matching.lock().await.running {
        return Ok(Json(
            json!({ "started": false, "reason": "identificação já em andamento" }),
        ));
    }

    let force = params.force;
    let pool = state.pool.clone();
    let providers = state.providers.clone();
    let artwork_dir = state.config.artwork_dir.clone();
    let status = state.matching.clone();

    // O job é aberto ANTES do spawn: se o índice único recusar, a resposta
    // diz que já há uma identificação rodando em vez de abrir uma segunda.
    let job = crate::jobs::Job::start(
        &state.pool,
        "match",
        json!({ "force": force }),
        None,
    )
    .await;
    let job_id = job.as_ref().map(|j| j.id);

    let bus = state.events.clone();
    tokio::spawn(async move {
        metadata::run_matching(pool, providers, artwork_dir, status.clone(), force, job).await;
        let finished = status.lock().await.clone();
        crate::events::publish(
            &bus,
            crate::events::AppEvent::MatchFinished {
                auto: finished.matched_auto,
                needs_review: finished.needs_review,
            },
        );
    });

    Ok(Json(json!({ "started": true, "force": force, "job_id": job_id })))
}

/// Formato inalterado de propósito — quatro alvos de cliente leem isto.
///
/// A mudança é de onde vem: quando o processo reiniciou e não há execução nesta
/// instância, o status é reconstruído do último job no banco. Antes, um restart
/// fazia a interface dizer "nunca rodou" sobre uma varredura de 17 mil arquivos
/// que tinha sido interrompida minutos antes.
pub async fn status(State(state): State<AppState>) -> Json<metadata::MatchStatus> {
    let mut current = state.matching.lock().await.clone();

    if current.started_at.is_none() {
        if let Some(job) = crate::jobs::latest(&state.pool, "match").await {
            if let Ok(anterior) = serde_json::from_value::<metadata::MatchStatus>(job.progress) {
                current = anterior;
                // O que ficou pendurado NÃO é apresentado como em andamento:
                // o processo que o executava não existe mais.
                current.running = false;
            }
        }
    }

    current.tmdb_enabled = state.providers.tmdb.is_some();

    // O que falta pra biblioteca contar tudo. Uma consulta, no mesmo endereço
    // que a tela já pergunta.
    current.nao_identificadas = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*)
        FROM work w
        WHERE NOT EXISTS (
                SELECT 1 FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
                WHERE wt.work_id = w.id AND t.namespace = 'format'
              )
          AND w.match_state <> 'ignored'
          AND EXISTS (
                SELECT 1 FROM media_file mf JOIN library l ON l.id = mf.library_id
                WHERE mf.work_id = w.id AND l.provider_hint <> 'none'
              )
        "#,
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);

    Json(current)
}

/// A fila. Obras em que o matcher ficou em dúvida, com o que ele entendeu do
/// nome do arquivo ao lado dos candidatos — pra decisão levar dois segundos.
pub async fn review(
    State(state): State<AppState>,
    // Operação de servidor, como as irmãs que mutam (confirm/reset/search).
    // Ver a lista do README: varrer, identificar e bibliotecas são de admin.
    AdminUser(_): AdminUser,
    Query(params): Query<ReviewQuery>,
) -> AppResult<Json<Value>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        title: String,
        year: Option<i32>,
        kind: String,
        season_number: Option<i32>,
        episode_number: Option<i32>,
        match_state: String,
        match_confidence: Option<f32>,
        match_reasons: Value,
        // `updated_at` é selecionada pra servir ao `sort=recent`, mas não é
        // lida em Rust — o sqlx ignora coluna que a struct não declara.
        filename: String,
        path: String,
        root_path: String,
        default_kind: String,
    }

    let estados: Vec<String> = params
        .state
        .as_deref()
        .unwrap_or("needs_review")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Ordenação por whitelist — mesmo padrão do `works::order_by`. Os nomes
    // aqui são os da SAÍDA da subconsulta, sem prefixo de tabela.
    let order = match params.sort.as_deref() {
        Some("confidence_asc") => "match_confidence ASC NULLS LAST",
        Some("path") => "path ASC",
        Some("recent") => "updated_at DESC",
        // O padrão põe primeiro quem está mais perto do limiar: é o que se
        // resolve com menos esforço, e faz a fila encolher mais rápido.
        _ => "match_confidence DESC NULLS LAST",
    };

    let limit = params.limit.clamp(1, 200);

    // Um FROM só, reusado pela página e pela contagem.
    let filtro = r#"
        FROM work w
        JOIN media_file m ON m.work_id = w.id AND m.status = 'probed'
        JOIN library l    ON l.id = m.library_id
        WHERE w.match_state = ANY($1)
          AND ($2::uuid IS NULL OR m.library_id = $2)
          AND ($3::text IS NULL OR m.dir_path LIKE $3 || '%')
          AND ($4::text IS NULL OR w.kind = $4)
          AND ($5::text IS NULL OR m.filename ILIKE '%' || $5 || '%'
                                OR w.title    ILIKE '%' || $5 || '%')
          AND ($6::bool IS NULL OR
               ($6 = true  AND EXISTS (SELECT 1 FROM match_candidate c WHERE c.work_id = w.id)) OR
               ($6 = false AND NOT EXISTS (SELECT 1 FROM match_candidate c WHERE c.work_id = w.id)))
    "#;

    let sql = format!(
        r#"
        SELECT DISTINCT ON (w.id)
            w.id, w.title, w.year, w.kind, w.season_number, w.episode_number,
            w.match_state, w.match_confidence, w.match_reasons, w.updated_at,
            m.filename, m.path, l.root_path, l.default_kind
        {filtro}
        ORDER BY w.id, m.size_bytes DESC
        "#
    );
    // O DISTINCT ON obriga a ordenar por `w.id` primeiro; a ordenação pedida
    // vai por FORA, sobre o conjunto já reduzido a uma linha por obra.
    let sql = format!("SELECT * FROM ({sql}) t ORDER BY {order} LIMIT $7 OFFSET $8");

    let rows: Vec<Row> = sqlx::query_as(&sql)
        .bind(&estados)
        .bind(params.library)
        .bind(params.dir.as_deref())
        .bind(params.kind.as_deref())
        .bind(params.q.as_deref())
        .bind(params.has_candidates)
        .bind(limit)
        .bind(params.offset.max(0))
        .fetch_all(&state.pool)
        .await?;

    let total: i64 = sqlx::query_scalar(&format!(
        "SELECT count(DISTINCT w.id) {filtro}"
    ))
    .bind(&estados)
    .bind(params.library)
    .bind(params.dir.as_deref())
    .bind(params.kind.as_deref())
    .bind(params.q.as_deref())
    .bind(params.has_candidates)
    .fetch_one(&state.pool)
    .await?;

    // Uma query pra todos os candidatos da página, em vez de uma por obra.
    // Antes eram 101 idas ao banco pra 50 itens (1 + 50 candidatos + 50
    // contextos); agora são 3.
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let mut por_obra: HashMap<Uuid, Vec<MatchCandidateRow>> = HashMap::new();
    if !ids.is_empty() {
        let candidatos: Vec<MatchCandidateRow> = sqlx::query_as(&format!(
            "SELECT {CANDIDATE_COLUMNS} FROM (
                 SELECT *, row_number() OVER (PARTITION BY work_id ORDER BY score DESC) AS n
                 FROM match_candidate WHERE work_id = ANY($1)
             ) c WHERE n <= 8"
        ))
        .bind(&ids)
        .fetch_all(&state.pool)
        .await?;
        for c in candidatos {
            por_obra.entry(c.work_id).or_default().push(c);
        }
    }

    // Contagem por estado, do BANCO.
    //
    // O badge da interface lia um `Arc<Mutex<MatchStatus>>` em memória, que
    // zerava a cada restart do processo: 2.388 obras na fila e o contador
    // dizendo 0. Aqui o número é derivado, não lembrado.
    let counts: Vec<(String, i64)> =
        sqlx::query_as("SELECT match_state, count(*) FROM work GROUP BY match_state")
            .fetch_all(&state.pool)
            .await?;

    let items: Vec<ReviewItem> = rows
        .into_iter()
        .map(|r| {
            // O guess é computado em Rust, do caminho que a query já trouxe —
            // sem a ida ao banco por obra que o `load_work_context` fazia.
            let guess = guess_from_path(
                std::path::Path::new(&r.path),
                std::path::Path::new(&r.root_path),
                r.default_kind == "episode",
            );
            ReviewItem {
                candidates: por_obra.remove(&r.id).unwrap_or_default(),
                guess: view_of(&guess),
                work: ReviewWork {
                    id: r.id,
                    title: r.title,
                    year: r.year,
                    kind: r.kind,
                    season_number: r.season_number,
                    episode_number: r.episode_number,
                    match_state: r.match_state,
                    match_confidence: r.match_confidence,
                    match_reasons: r.match_reasons,
                    filename: r.filename,
                },
            }
        })
        .collect();

    Ok(Json(json!({
        "total": total,
        "limit": limit,
        "offset": params.offset.max(0),
        "counts": counts
            .into_iter()
            .map(|(estado, n)| (estado, Value::from(n)))
            .collect::<serde_json::Map<String, Value>>(),
        "items": items,
    })))
}

pub async fn candidates(
    State(state): State<AppState>,
    AxumPath(work_id): AxumPath<Uuid>,
) -> AppResult<Json<Vec<MatchCandidateRow>>> {
    Ok(Json(candidates_for(&state, work_id).await?))
}

/// Busca manual: o humano digita o nome certo e o matcher tenta de novo.
/// É a válvula de escape pra quando o nome do arquivo é irrecuperável.
pub async fn manual_search(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    AxumPath(work_id): AxumPath<Uuid>,
    Json(body): Json<ManualSearch>,
) -> AppResult<Json<Vec<MatchCandidateRow>>> {
    if body.query.trim().is_empty() {
        return Err(AppError::BadRequest("busca vazia".into()));
    }

    let (work, mut guess) = metadata::load_work_context(&state.pool, work_id).await?;
    guess.title = body.query.trim().to_string();
    if body.year.is_some() {
        guess.year = body.year;
    }

    let hint = if body.provider == "auto" {
        work.provider_hint.clone()
    } else {
        body.provider.clone()
    };

    let found = metadata::search(&state.providers, &guess, &hint).await;
    for candidate in &found {
        let scored = score::score_candidate(&guess, candidate);
        metadata::persist_candidate(&state.pool, work_id, candidate, &scored).await?;
    }

    Ok(Json(candidates_for(&state, work_id).await?))
}

/// Confirmação humana. `confirmed` é o único estado que o matcher automático
/// nunca sobrescreve, nem com `force`.
pub async fn confirm(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    AxumPath(work_id): AxumPath<Uuid>,
    Json(body): Json<ConfirmMatch>,
) -> AppResult<Json<Value>> {
    let row = sqlx::query_as::<_, MatchCandidateRow>(&format!(
        "SELECT {CANDIDATE_COLUMNS} FROM match_candidate WHERE id = $1 AND work_id = $2"
    ))
    .bind(body.candidate_id)
    .bind(work_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let accent: Option<String> =
        sqlx::query_scalar("SELECT accent_color FROM match_candidate WHERE id = $1")
            .bind(body.candidate_id)
            .fetch_one(&state.pool)
            .await?;

    let candidate = Candidate {
        provider: row.provider.clone(),
        provider_id: row.provider_id.clone(),
        provider_kind: row.provider_kind.clone(),
        title: row.title.clone(),
        original_title: row.original_title.clone(),
        year: row.year,
        overview: row.overview.clone(),
        poster_url: row.poster_url.clone(),
        backdrop_url: row.backdrop_url.clone(),
        genres: Vec::new(),
        accent_color: accent,
        popularity: 0.0,
        raw: Value::Null,
    };

    // --- quem mais recebe esta decisão ------------------------------------
    //
    // Os irmãos são achados pelo DIRETÓRIO, nunca pelo título adivinhado.
    // Medido: 474 de 487 diretórios (97,3%) com episódios já casados apontam
    // para exatamente uma série. Título adivinhado colapsaria "Naruto" e
    // "Naruto Shippuden", que são pastas vizinhas.
    let irmaos: Vec<Uuid> = match body.apply_to.as_str() {
        "work" => Vec::new(),
        escopo @ ("directory" | "series") => {
            // `series` sobe um nível quando a pasta atual é de temporada, pra
            // alcançar as outras temporadas da mesma série.
            let recursivo = escopo == "series";
            sqlx::query_scalar(
                r#"
                WITH alvo AS (
                    SELECT DISTINCT ON (m.work_id) m.dir_path
                    FROM media_file m WHERE m.work_id = $1
                    ORDER BY m.work_id, m.size_bytes DESC
                ),
                raiz AS (
                    SELECT CASE
                        WHEN $2 AND dir_path ~ '/([Ss]eason|[Tt]emporada|[Ss]pecials?|[Ee]speciais?)[^/]*$'
                            THEN regexp_replace(dir_path, '/[^/]+$', '')
                        ELSE dir_path
                    END AS base FROM alvo
                )
                -- CROSS JOIN explícito e não vírgula: misturar a lista do FROM
                -- com JOIN faz o `JOIN work` se ligar a `raiz` em vez de a `m`.
                SELECT DISTINCT m.work_id
                FROM media_file m
                JOIN work w ON w.id = m.work_id
                CROSS JOIN raiz
                WHERE m.status = 'probed'
                  AND (m.dir_path = raiz.base OR ($2 AND m.dir_path LIKE raiz.base || '/%'))
                  AND m.work_id <> $1
                  -- `confirmed` e `auto` nunca são sobrescritos por propagação.
                  -- Quem decidiu foi um humano ou o matcher acima do limiar;
                  -- um vizinho não desfaz isso (§8b).
                  AND w.match_state IN ('unmatched', 'needs_review')
                "#,
            )
            .bind(work_id)
            .bind(recursivo)
            .fetch_all(&state.pool)
            .await?
        }
        outro => {
            return Err(AppError::BadRequest(format!(
                "apply_to `{outro}` não existe — use work, directory ou series"
            )))
        }
    };

    if body.dry_run {
        return Ok(Json(json!({
            "dry_run": true,
            "title": row.title,
            "esta_obra": 1,
            "irmaos": irmaos.len(),
        })));
    }

    let (work, guess) = metadata::load_work_context(&state.pool, work_id).await?;

    // R54 — O MESMO GUARDA QUE OS IRMÃOS SEMPRE TIVERAM.
    //
    // A regra do §M1 — *"nunca inventar; se não dá pra saber, o campo fica None
    // e a obra cai na fila"* — estava escrita três vezes e aplicada em três dos
    // quatro caminhos que confirmam alguma coisa:
    //
    // | caminho | guardava? |
    // |---|---|
    // | `scopes::identify` (a pasta inteira) | sim |
    // | a regra de escopo, no `metadata/mod.rs` | sim |
    // | `propagar` (os irmãos deste clique) | sim, com comentário explicando |
    // | **este aqui — a obra em que se clicou** | **não** |
    //
    // Faltava justamente onde a decisão humana entra. E o sintoma foi silencioso
    // do pior jeito: 22 arquivos de `Arrested Development` viraram 22 obras
    // `confirmed` **sem número de episódio** — e sem episódio o scanner não cria
    // `collection(série)`, então a biblioteca mostrou 22 cartões idênticos em vez
    // de uma série. Nada errou em voz alta.
    //
    // Um arquivo cujo nome não diz o episódio recebe a **identidade da série** e
    // fica na fila com o motivo. É mais do que ele tinha antes: agora se sabe
    // QUAL é a série, e a revisão vira uma pergunta só ("qual episódio?") em vez
    // de duas.
    if guess.any_episode().is_none() && candidate.provider_kind != "movie" {
        sqlx::query(
            "UPDATE work SET match_state = 'needs_review', match_reasons = $2, updated_at = now()
             WHERE id = $1",
        )
        .bind(work_id)
        .bind(json!([
            format!("a série é {} (confirmada por você)", candidate.title),
            "mas o número do episódio não está neste arquivo".to_string(),
        ]))
        .execute(&state.pool)
        .await?;
    } else {
        metadata::apply_candidate(
            &state.pool,
            &state.providers,
            &state.config.artwork_dir,
            &work,
            &guess,
            &candidate,
            row.id,
            1.0,
            "confirmed",
        )
        .await?;
    }

    // Os irmãos recebem a MESMA série, mas cada um resolve o próprio episódio:
    // é o número dele que diz qual episódio é, não o do vizinho.
    let mut propagados = 0usize;
    let mut falhas = Vec::new();
    for irmao in &irmaos {
        match propagar(&state, *irmao, &candidate, &pontuacao_de(&row)).await {
            Ok(()) => propagados += 1,
            Err(e) => {
                if falhas.len() < 20 {
                    falhas.push(json!({ "work_id": irmao, "erro": e.to_string() }));
                }
            }
        }
    }

    Ok(Json(json!({
        "ok": true,
        "title": row.title,
        "propagados": propagados,
        "falhas": falhas,
    })))
}

/// A razão gravada quando a decisão veio de um vizinho, não de um score.
fn pontuacao_de(row: &MatchCandidateRow) -> score::Score {
    score::Score {
        value: 1.0,
        reasons: vec![format!(
            "confirmado por você em outro arquivo da mesma pasta ({})",
            row.title
        )],
    }
}

/// Aplica a série a um irmão, resolvendo o episódio DELE.
async fn propagar(
    state: &AppState,
    work_id: Uuid,
    candidate: &Candidate,
    pontuacao: &score::Score,
) -> anyhow::Result<()> {
    let (work, guess) = metadata::load_work_context(&state.pool, work_id).await?;

    // Sem número de episódio próprio, o irmão não vira `confirmed`: receberia a
    // série certa e um episódio inventado. Fica na fila com o motivo — que é
    // mais útil do que estava antes, porque agora se sabe QUAL é a série.
    if guess.any_episode().is_none() && candidate.provider_kind != "movie" {
        sqlx::query(
            "UPDATE work SET match_state = 'needs_review', match_reasons = $2, updated_at = now()
             WHERE id = $1 AND match_state IN ('unmatched','needs_review')",
        )
        .bind(work_id)
        .bind(json!([
            format!("a série é {} (confirmada por você numa vizinha)", candidate.title),
            "mas o número do episódio não está neste arquivo".to_string(),
        ]))
        .execute(&state.pool)
        .await?;
        return Ok(());
    }

    let candidate_id = metadata::persist_candidate(&state.pool, work_id, candidate, pontuacao).await?;
    metadata::apply_candidate(
        &state.pool,
        &state.providers,
        &state.config.artwork_dir,
        &work,
        &guess,
        candidate,
        candidate_id,
        1.0,
        "confirmed",
    )
    .await?;

    sqlx::query("UPDATE work SET match_reasons = $2 WHERE id = $1")
        .bind(work_id)
        .bind(serde_json::to_value(&pontuacao.reasons).unwrap_or_else(|_| json!([])))
        .execute(&state.pool)
        .await?;
    Ok(())
}

/// Confirma várias obras de uma vez, cada uma com o SEU candidato.
///
/// Não existe "aplicar o mesmo candidato a todas": candidato é por obra, e o
/// que a interface seleciona é um conjunto de decisões, não uma só repetida.
/// Para "esta série vale pra pasta inteira" existe o escopo (`/api/scopes`).
pub async fn bulk_match(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(body): Json<BulkMatch>,
) -> AppResult<Json<Value>> {
    if body.items.len() > 500 {
        return Err(AppError::BadRequest(
            "no máximo 500 por chamada — acima disso use o escopo por pasta".into(),
        ));
    }

    if body.dry_run {
        return Ok(Json(json!({ "dry_run": true, "seriam_aplicados": body.items.len() })));
    }

    let mut aplicados = 0usize;
    let mut falhas = Vec::new();

    for item in &body.items {
        let resultado = confirmar_um(&state, item.work_id, item.candidate_id).await;
        match resultado {
            Ok(()) => aplicados += 1,
            Err(e) => {
                if falhas.len() < 50 {
                    falhas.push(json!({ "work_id": item.work_id, "erro": e.to_string() }));
                }
            }
        }
    }

    Ok(Json(json!({ "aplicados": aplicados, "falhas": falhas })))
}

/// Marca um conjunto de obras com um estado terminal.
///
/// O caso que motivou: 1.234 arquivos em `Featurettes/` e `Extras/` que nunca
/// vão casar com o TMDB e ficavam na fila fingindo ser trabalho pendente.
pub async fn bulk_state(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(body): Json<BulkState>,
) -> AppResult<Json<Value>> {
    // `confirmed` e `auto` não entram: mudar estado em lote é operação grossa,
    // e desfazer identificação boa por engano é caro. Para isso existe o reset,
    // que é por obra e explícito.
    if !matches!(body.state.as_str(), "ignored" | "unmatched") {
        return Err(AppError::BadRequest(
            "estado em lote só aceita `ignored` ou `unmatched`".into(),
        ));
    }

    let motivos = match &body.reason {
        Some(r) if !r.trim().is_empty() => json!([r.trim()]),
        _ => json!([]),
    };

    let afetadas = sqlx::query(
        "UPDATE work SET match_state = $2, match_reasons = $3,
                         match_confidence = NULL, updated_at = now()
         WHERE id = ANY($1) AND match_state IN ('unmatched','needs_review','ignored')",
    )
    .bind(&body.work_ids)
    .bind(&body.state)
    .bind(&motivos)
    .execute(&state.pool)
    .await?
    .rows_affected();

    Ok(Json(json!({
        "afetadas": afetadas,
        "pedidas": body.work_ids.len(),
        "estado": body.state,
    })))
}

/// Confirma uma obra — o caminho comum, sem propagação.
async fn confirmar_um(state: &AppState, work_id: Uuid, candidate_id: Uuid) -> anyhow::Result<()> {
    let row = sqlx::query_as::<_, MatchCandidateRow>(&format!(
        "SELECT {CANDIDATE_COLUMNS} FROM match_candidate WHERE id = $1 AND work_id = $2"
    ))
    .bind(candidate_id)
    .bind(work_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| anyhow::anyhow!("candidato não pertence a esta obra"))?;

    let accent: Option<String> =
        sqlx::query_scalar("SELECT accent_color FROM match_candidate WHERE id = $1")
            .bind(candidate_id)
            .fetch_one(&state.pool)
            .await?;

    let candidate = Candidate {
        provider: row.provider.clone(),
        provider_id: row.provider_id.clone(),
        provider_kind: row.provider_kind.clone(),
        title: row.title.clone(),
        original_title: row.original_title.clone(),
        year: row.year,
        overview: row.overview.clone(),
        poster_url: row.poster_url.clone(),
        backdrop_url: row.backdrop_url.clone(),
        genres: Vec::new(),
        accent_color: accent,
        popularity: 0.0,
        raw: Value::Null,
    };

    let (work, guess) = metadata::load_work_context(&state.pool, work_id).await?;
    metadata::apply_candidate(
        &state.pool,
        &state.providers,
        &state.config.artwork_dir,
        &work,
        &guess,
        &candidate,
        row.id,
        1.0,
        "confirmed",
    )
    .await?;
    Ok(())
}

/// Corrige o que o parser entendeu, e GUARDA a correção.
///
/// Sem isto, a correção manual morria: o `manual_search` mutava um `Guess`
/// local só pra montar a consulta e o descartava, e o `confirm` re-derivava
/// tudo do caminho. Escolher a série certa não bastava — a busca do episódio
/// continuava usando a numeração errada do nome do arquivo.
///
/// Só os campos enviados mudam; o resto continua vindo do caminho.
pub async fn set_parse(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    AxumPath(work_id): AxumPath<Uuid>,
    Json(body): Json<Value>,
) -> AppResult<Json<Value>> {
    if !body.is_object() {
        return Err(AppError::BadRequest("esperava um objeto JSON".into()));
    }

    // Whitelist: campo desconhecido é erro de quem chama, não coisa a guardar.
    const CAMPOS: &[&str] = &["title", "year", "season", "episode", "absolute_episode"];
    let mut limpo = serde_json::Map::new();
    for (chave, valor) in body.as_object().unwrap() {
        if !CAMPOS.contains(&chave.as_str()) {
            return Err(AppError::BadRequest(format!(
                "campo `{chave}` não faz parte do parse — aceito: {}",
                CAMPOS.join(", ")
            )));
        }
        limpo.insert(chave.clone(), valor.clone());
    }

    sqlx::query("UPDATE work SET parse_override = $2, updated_at = now() WHERE id = $1")
        .bind(work_id)
        .bind(Value::Object(limpo))
        .execute(&state.pool)
        .await?;

    let (_, guess) = metadata::load_work_context(&state.pool, work_id).await?;
    Ok(Json(json!({ "ok": true, "guess": view_of(&guess) })))
}

/// Volta a confiar só no caminho.
pub async fn clear_parse(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    AxumPath(work_id): AxumPath<Uuid>,
) -> AppResult<Json<Value>> {
    sqlx::query("UPDATE work SET parse_override = NULL, updated_at = now() WHERE id = $1")
        .bind(work_id)
        .execute(&state.pool)
        .await?;
    let (_, guess) = metadata::load_work_context(&state.pool, work_id).await?;
    Ok(Json(json!({ "ok": true, "guess": view_of(&guess) })))
}

/// Desfaz a identificação — TUDO que veio do provider, e só isso.
///
/// A versão anterior limpava estado, confiança e artwork, mas deixava para trás
/// título, sinopse, `external_ids`, `kind`, coleções, créditos e tags do match
/// desfeito. O resultado era uma obra "não identificada" ainda exibindo o nome
/// da série errada — o pior dos dois mundos.
///
/// O que SOBREVIVE, de propósito:
///   - `parse_override`: é o que a pessoa ensinou sobre o arquivo, não um
///     resultado de match;
///   - coleções com `origin = 'manual'`: playlist e ordem de exibição são
///     trabalho humano (§8c), e o provider não as criou;
///   - tags com `source = 'manual'`: idem.
pub async fn reset(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    AxumPath(work_id): AxumPath<Uuid>,
) -> AppResult<Json<Value>> {
    // Transação: uma obra meio-desfeita é pior que uma obra mal identificada.
    let mut tx = state.pool.begin().await?;

    sqlx::query(
        "DELETE FROM collection_item ci
         USING collection c
         WHERE ci.collection_id = c.id AND ci.work_id = $1 AND c.origin = 'provider'",
    )
    .bind(work_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM credit WHERE work_id = $1")
        .bind(work_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM work_tag WHERE work_id = $1 AND source <> 'manual'")
        .bind(work_id)
        .execute(&mut *tx)
        .await?;

    // O título volta pro parse efetivo (override por cima do caminho), que é o
    // melhor palpite disponível sem provider — e é o que a fila vai mostrar.
    let (work, guess) = metadata::load_work_context(&state.pool, work_id).await?;
    let titulo = if guess.title.trim().is_empty() {
        None
    } else {
        Some(guess.title.clone())
    };

    sqlx::query(
        "UPDATE work SET
             match_state = 'unmatched', match_confidence = NULL,
             matched_candidate_id = NULL, matched_at = NULL,
             match_reasons = '[]'::jsonb,
             title = COALESCE($2, title),
             original_title = NULL, overview = NULL, year = $3,
             external_ids = '{}'::jsonb,
             kind = $4,
             season_number = $5, episode_number = $6,
             artwork = '{}'::jsonb, dominant_color = NULL,
             updated_at = now()
         WHERE id = $1",
    )
    .bind(work_id)
    .bind(titulo)
    .bind(guess.year)
    .bind(guess.kind(&work.default_kind))
    .bind(guess.season)
    .bind(guess.any_episode())
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Json(json!({ "ok": true, "guess": view_of(&guess) })))
}

/// Re-deriva o parse das obras ainda não identificadas.
///
/// Existe porque as correções do parser só afetam arquivos NOVOS: `title`,
/// `season_number` e `episode_number` são gravados uma vez, na criação da obra
/// (`scanner::process_file`). Um acervo já varrido carrega o parse antigo pra
/// sempre, e re-varrer não ajuda — o scanner pula arquivo intacto pelo
/// `(size, mtime)`, que é justamente o que o torna rápido.
///
/// **Nunca toca `auto` nem `confirmed`.** Ali houve decisão — do matcher acima
/// do limiar ou de um humano — e reescrever o título por baixo dela é
/// exatamente o que o DESIGN §8b proíbe. Só mexe em `unmatched`/`needs_review`,
/// onde não há nada a preservar.
///
/// `dry_run` é o padrão: uma operação que reescreve milhares de linhas mostra o
/// que vai fazer antes de fazer.
pub async fn reparse(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<ReparseParams>,
) -> AppResult<Json<Value>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        path: String,
        root_path: String,
        default_kind: String,
        title: String,
        season_number: Option<i32>,
        episode_number: Option<i32>,
    }

    let rows: Vec<Row> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (w.id)
            w.id, m.path, l.root_path, l.default_kind,
            w.title, w.season_number, w.episode_number
        FROM work w
        JOIN media_file m ON m.work_id = w.id AND m.status = 'probed'
        JOIN library l ON l.id = m.library_id
        WHERE w.match_state IN ('unmatched', 'needs_review')
        ORDER BY w.id, m.size_bytes DESC
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    let scanned = rows.len();
    let mut changed = 0usize;
    let mut sample = Vec::new();

    for row in rows {
        let guess = guess_from_path(
            std::path::Path::new(&row.path),
            std::path::Path::new(&row.root_path),
            row.default_kind == "episode",
        );
        let episode = guess.any_episode();

        // Título vazio não é melhoria — deixa como está.
        let title = if guess.title.trim().is_empty() {
            row.title.clone()
        } else {
            guess.title.clone()
        };

        if title == row.title
            && guess.season == row.season_number
            && episode == row.episode_number
        {
            continue;
        }
        changed += 1;

        if sample.len() < 40 {
            sample.push(json!({
                "work_id": row.id,
                "path": row.path,
                "de": { "titulo": row.title, "temporada": row.season_number,
                        "episodio": row.episode_number },
                "para": { "titulo": title, "temporada": guess.season, "episodio": episode },
            }));
        }

        if !params.dry_run {
            sqlx::query(
                "UPDATE work SET title = $2, season_number = $3, episode_number = $4,
                                 updated_at = now()
                 WHERE id = $1 AND match_state IN ('unmatched', 'needs_review')",
            )
            .bind(row.id)
            .bind(&title)
            .bind(guess.season)
            .bind(episode)
            .execute(&state.pool)
            .await?;
        }
    }

    Ok(Json(json!({
        "dry_run": params.dry_run,
        "analisadas": scanned,
        "mudariam": changed,
        "inalteradas": scanned - changed,
        "amostra": sample,
    })))
}

/// Conserta obras cujo título ficou "Episódio N".
///
/// Esse título é o que o `apply_candidate` grava quando NÃO consegue o detalhe
/// do episódio no provider. Enquanto a busca era `/season/{s}/episode/{e}`
/// direto, ela falhava em toda série cujo TMDB numera de forma absoluta dentro
/// da temporada — e a obra era gravada como `auto`/`confirmed` afirmando uma
/// certeza que não tinha.
///
/// A obra já sabe a série (`external_ids`) e o par temporada/episódio, então o
/// reparo não precisa buscar nada de novo: pega a temporada e resolve pelo
/// mesmo `find_episode` que o resto do código usa agora.
///
/// Só título e sinopse. O still do episódio fica de fora de propósito — seria
/// um download por obra, e isso é trabalho para uma reidentificação completa,
/// não para um reparo.
pub async fn repair_episode_titles(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<crate::models::RepairParams>,
) -> AppResult<Json<Value>> {
    let tmdb = state
        .providers
        .tmdb
        .clone()
        .ok_or_else(|| AppError::BadRequest("TMDB_API_KEY não configurada".into()))?;

    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        serie: String,
        season_number: i32,
        episode_number: i32,
        title: String,
    }

    let rows: Vec<Row> = sqlx::query_as(
        r#"
        SELECT w.id,
               w.external_ids ->> 'tmdb' AS serie,
               w.season_number, w.episode_number, w.title
        FROM work w
        WHERE w.title ~ '^Episódio [0-9]+$'
          AND w.external_ids ? 'tmdb'
          AND w.season_number IS NOT NULL
          AND w.episode_number IS NOT NULL
        ORDER BY w.external_ids ->> 'tmdb', w.season_number, w.episode_number
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    // Uma chamada por (série, temporada), não por obra. A ordenação acima já
    // agrupa, mas o cache torna a garantia explícita.
    let mut cache: HashMap<(String, i32), Vec<crate::metadata::tmdb::SeasonEpisode>> =
        HashMap::new();

    let mut corrigidas = 0usize;
    let mut sem_solucao = 0usize;
    let mut episodio_inexistente = 0usize;
    let mut requeued = 0usize;
    let mut amostra = Vec::new();
    let mut suspeitas = Vec::new();

    for row in &rows {
        let chave = (row.serie.clone(), row.season_number);
        if !cache.contains_key(&chave) {
            let eps = tmdb
                .season(&row.serie, row.season_number)
                .await
                .map(|s| s.episodes)
                .unwrap_or_default();
            cache.insert(chave.clone(), eps);
        }

        // Dois desfechos muito diferentes, que não podem virar um número só:
        //
        //  - o episódio NÃO EXISTE na série → a obra está marcada como
        //    identificada com uma numeração que o provider desmente. É estado
        //    errado no banco, não limite do provider;
        //  - o episódio existe mas não tem nome → é o provider que não sabe, e
        //    "Episódio N" é a resposta honesta.
        let Some(detalhe) = cache
            .get(&chave)
            .and_then(|eps| metadata::find_episode(eps, row.episode_number))
        else {
            episodio_inexistente += 1;
            if suspeitas.len() < 30 {
                suspeitas.push(json!({
                    "work_id": row.id,
                    "serie_tmdb": row.serie,
                    "temporada": row.season_number,
                    "episodio": row.episode_number,
                }));
            }

            // Devolve pra fila: a obra está marcada como identificada afirmando
            // um episódio que o provider desmente. Manter esse estado é o que o
            // §8b proíbe — melhor perguntar do que sustentar o erro.
            //
            // O título e a numeração ficam como estão: desfazer direito é o
            // `reset` simétrico, e destruir dado aqui atrapalharia quem for
            // revisar. O motivo gravado diz exatamente o que houve.
            if params.requeue && !params.dry_run {
                requeued += 1;
                sqlx::query(
                    "UPDATE work SET match_state = 'needs_review', match_confidence = NULL,
                                     match_reasons = $2, updated_at = now()
                     WHERE id = $1",
                )
                .bind(row.id)
                .bind(json!([format!(
                    "o provider diz que S{:02}E{:02} não existe nesta série — \
                     a identificação anterior afirmava um episódio inexistente",
                    row.season_number, row.episode_number
                )]))
                .execute(&state.pool)
                .await?;
            }
            continue;
        };

        let Some(novo) = detalhe.name.clone().filter(|n| !n.trim().is_empty()) else {
            sem_solucao += 1;
            continue;
        };

        // O TMDB devolve literalmente "Episódio N" quando a série não tem nome
        // de episódio catalogado. Aí não há nada a consertar — e contar isso
        // como correção inflaria o número sem melhorar nada.
        if novo == row.title {
            sem_solucao += 1;
            continue;
        }

        corrigidas += 1;
        if amostra.len() < 30 {
            amostra.push(json!({
                "work_id": row.id,
                "de": row.title,
                "para": novo,
                "temporada": row.season_number,
                "episodio": row.episode_number,
            }));
        }

        if !params.dry_run {
            sqlx::query(
                "UPDATE work SET title = $2,
                                 overview = COALESCE($3, overview),
                                 updated_at = now()
                 WHERE id = $1",
            )
            .bind(row.id)
            .bind(&novo)
            .bind(detalhe.overview.clone().filter(|o| !o.trim().is_empty()))
            .execute(&state.pool)
            .await?;
        }
    }

    Ok(Json(json!({
        "dry_run": params.dry_run,
        "analisadas": rows.len(),
        "corrigidas": corrigidas,
        "sem_titulo_no_provider": sem_solucao,
        // Estas merecem voltar pra revisão: o provider diz que o episódio não
        // existe, então a identificação afirma algo que ele desmente.
        "episodio_nao_existe_na_serie": episodio_inexistente,
        "devolvidas_pra_revisao": requeued,
        "chamadas_de_temporada": cache.len(),
        "amostra": amostra,
        "suspeitas": suspeitas,
    })))
}

async fn candidates_for(state: &AppState, work_id: Uuid) -> AppResult<Vec<MatchCandidateRow>> {
    let rows = sqlx::query_as::<_, MatchCandidateRow>(&format!(
        "SELECT {CANDIDATE_COLUMNS} FROM match_candidate
         WHERE work_id = $1 ORDER BY score DESC LIMIT 12"
    ))
    .bind(work_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows)
}

fn view_of(guess: &Guess) -> GuessView {
    GuessView {
        title: guess.title.clone(),
        year: guess.year,
        season: guess.season,
        episode: guess.episode,
        absolute_episode: guess.absolute_episode,
        release_group: guess.release_group.clone(),
        looks_like_anime: guess.looks_like_anime,
    }
}

/// Reparo: dá identidade às coleções-série que já existem.
///
/// O identificador só passou a enriquecer a série a partir desta versão. As
/// 115 séries já no banco continuam como o matcher antigo as deixou: título,
/// ano, e `overview` NULL, `external_ids` '{}', `artwork` '{}' em todas.
///
/// São duas correções na mesma passada, porque são a mesma causa:
///
///  1. **Sinopse e ids.** Um `GET /tv/{id}` por série — 114 chamadas, não uma
///     por episódio.
///
///  2. **A arte.** O pôster que o provider devolve para um episódio é o da
///     SÉRIE. Baixado por obra, o acervo guardou 18.004 arquivos para 1.429
///     imagens distintas — 2,19 GB onde cabem 197 MB, uma imagem salva 553
///     vezes. Aqui a série passa a ser dona do arquivo e os episódios apontam
///     pra ele.
///
/// `dry_run` é o padrão, como no reparo de títulos: contar é inofensivo,
/// reescrever 8.471 obras não é.
///
/// O arquivo órfão **não é apagado**: o reparo só relata quantos e quantos
/// bytes ficaram sem dono. Apagar arquivo é decisão de quem administra, e
/// `/api/maintenance/artwork-orfao` faz isso separado.
pub async fn repair_series(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<crate::models::RepairParams>,
) -> AppResult<Json<Value>> {
    let tmdb = state
        .providers
        .tmdb
        .clone()
        .ok_or_else(|| AppError::BadRequest("TMDB_API_KEY não configurada".into()))?;

    #[derive(sqlx::FromRow)]
    struct Serie {
        id: Uuid,
        title: String,
        provider_key: Option<String>,
        overview: Option<String>,
        poster: Option<String>,
    }

    let series: Vec<Serie> = sqlx::query_as(
        "SELECT id, title, provider_key, overview, artwork->>'poster' AS poster
         FROM collection
         WHERE kind = 'series'
           AND (overview IS NULL OR NOT (artwork ? 'poster'))
         ORDER BY title",
    )
    .fetch_all(&state.pool)
    .await?;

    let mut enriquecidas = 0usize;
    let mut sem_provider = 0usize;
    let mut nao_tmdb = 0usize;
    let mut falhou = 0usize;
    let mut amostra = Vec::new();

    for serie in &series {
        let Some(chave) = serie.provider_key.as_deref() else {
            sem_provider += 1;
            continue;
        };
        let Some(("tmdb", id)) = chave.split_once(':').map(|(p, i)| (p, i)) else {
            nao_tmdb += 1;
            continue;
        };

        let candidato = match tmdb.tv_by_id(id).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(serie = %serie.title, error = %e, "tv_by_id falhou");
                falhou += 1;
                continue;
            }
        };

        if params.dry_run {
            enriquecidas += 1;
            if amostra.len() < 8 {
                amostra.push(json!({
                    "serie": serie.title,
                    "ganha_sinopse": serie.overview.is_none() && candidato.overview.is_some(),
                    "ganha_arte": serie.poster.is_none() && candidato.poster_url.is_some(),
                }));
            }
            continue;
        }

        // A arte da série, uma vez, com o nome da coleção.
        let mut poster = serie.poster.clone();
        let mut backdrop: Option<String> = None;
        let mut cor = candidato.accent_color.clone();

        if poster.is_none() {
            if let Some(url) = &candidato.poster_url {
                match crate::artwork::fetch(
                    &state.providers.http,
                    &state.config.artwork_dir,
                    serie.id,
                    "poster",
                    url,
                )
                .await
                {
                    Ok(stored) => {
                        poster = Some(stored.path);
                        cor = cor.or(stored.dominant_color);
                    }
                    Err(e) => tracing::warn!(error = %e, "pôster da série não baixou"),
                }
            }
        }
        if let Some(url) = &candidato.backdrop_url {
            if let Ok(stored) = crate::artwork::fetch(
                &state.providers.http,
                &state.config.artwork_dir,
                serie.id,
                "backdrop",
                url,
            )
            .await
            {
                backdrop = Some(stored.path);
            }
        }

        metadata::grava_identidade_da_serie(
            &state.pool,
            serie.id,
            candidato.overview.as_deref(),
            "tmdb",
            id,
            poster.as_deref(),
            backdrop.as_deref(),
            cor.as_deref(),
        )
        .await?;

        enriquecidas += 1;
        if amostra.len() < 8 {
            amostra.push(json!({ "serie": serie.title, "sinopse": candidato.overview.is_some() }));
        }
    }

    // --- os episódios passam a apontar pra arte da série --------------------
    //
    // Só onde a série TEM arte e a obra ainda aponta pro próprio arquivo. O
    // `still` não é tocado: aquele é genuinamente por episódio.
    let repontadas: i64 = if params.dry_run {
        sqlx::query_scalar(
            "SELECT count(*)
             FROM work w
             JOIN collection_item ci ON ci.work_id = w.id
             JOIN collection temp ON temp.id = ci.collection_id AND temp.kind = 'season'
             JOIN collection s ON s.id = temp.parent_id AND s.kind = 'series'
             WHERE s.artwork ? 'poster'
               AND w.artwork->>'poster' IS DISTINCT FROM s.artwork->>'poster'",
        )
        .fetch_one(&state.pool)
        .await?
    } else {
        sqlx::query_scalar(
            "WITH alvo AS (
                 SELECT w.id AS work_id, s.artwork AS arte, s.dominant_color AS cor
                 FROM work w
                 JOIN collection_item ci ON ci.work_id = w.id
                 JOIN collection temp ON temp.id = ci.collection_id AND temp.kind = 'season'
                 JOIN collection s ON s.id = temp.parent_id AND s.kind = 'series'
                 WHERE s.artwork ? 'poster'
                   AND w.artwork->>'poster' IS DISTINCT FROM s.artwork->>'poster'
             ), feito AS (
                 UPDATE work w SET
                     artwork = w.artwork || alvo.arte,
                     dominant_color = COALESCE(alvo.cor, w.dominant_color),
                     updated_at = now()
                 FROM alvo WHERE w.id = alvo.work_id
                 RETURNING 1
             )
             SELECT count(*) FROM feito",
        )
        .fetch_one(&state.pool)
        .await?
    };

    let orfaos = artwork_orfao(&state).await.unwrap_or((0, 0));

    Ok(Json(json!({
        "dry_run": params.dry_run,
        "series_pendentes": series.len(),
        "series_enriquecidas": enriquecidas,
        "sem_provider_key": sem_provider,
        "provider_sem_suporte": nao_tmdb,
        "falharam": falhou,
        "obras_repontadas": repontadas,
        "artwork_orfao": { "arquivos": orfaos.0, "bytes": orfaos.1 },
        "amostra": amostra,
    })))
}

/// Todo caminho de imagem que alguma linha do banco ainda aponta.
///
/// Uma consulta só, e usada nos **dois** lados da limpeza — o ensaio e a
/// execução. Ficarem separadas era um convite a divergirem: o ensaio contaria
/// uma coisa e a execução apagaria outra.
///
/// `programme.arte` entrou aqui junto com a arte da grade (§28). Sem esta
/// linha, "limpar artwork órfão" apagaria a foto de todos os programas no ar —
/// nenhum deles está em `work` nem em `collection`.
const ARTWORK_VIVO: &str = "SELECT jsonb_each_text.value FROM work, jsonb_each_text(work.artwork)
     UNION
     SELECT jsonb_each_text.value FROM collection, jsonb_each_text(collection.artwork)
     UNION
     SELECT image_path FROM person WHERE image_path IS NOT NULL
     UNION
     SELECT arte FROM programme WHERE arte IS NOT NULL";

/// Conta o artwork em disco que nenhuma linha do banco referencia mais.
///
/// Depois do reparo, os `{work_id}-poster.jpg` dos episódios ficam sem dono —
/// era a cópia que aquele episódio guardava da arte da série.
async fn artwork_orfao(state: &AppState) -> anyhow::Result<(usize, u64)> {
    let referenciados: Vec<String> = sqlx::query_scalar(ARTWORK_VIVO)
    .fetch_all(&state.pool)
    .await?;

    let vivos: std::collections::HashSet<&str> =
        referenciados.iter().map(String::as_str).collect();

    let mut arquivos = 0usize;
    let mut bytes = 0u64;
    let mut dir = tokio::fs::read_dir(&state.config.artwork_dir).await?;
    while let Some(entrada) = dir.next_entry().await? {
        let nome = entrada.file_name();
        let Some(nome) = nome.to_str() else { continue };
        if vivos.contains(nome) {
            continue;
        }
        if let Ok(meta) = entrada.metadata().await {
            if meta.is_file() {
                arquivos += 1;
                bytes += meta.len();
            }
        }
    }
    Ok((arquivos, bytes))
}

/// Apaga o artwork que ninguém referencia. `dry_run` por padrão.
pub async fn limpar_artwork_orfao(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<crate::models::RepairParams>,
) -> AppResult<Json<Value>> {
    let referenciados: Vec<String> = sqlx::query_scalar(ARTWORK_VIVO)
    .fetch_all(&state.pool)
    .await?;

    let vivos: std::collections::HashSet<&str> =
        referenciados.iter().map(String::as_str).collect();

    let mut apagados = 0usize;
    let mut bytes = 0u64;
    let mut dir = tokio::fs::read_dir(&state.config.artwork_dir).await?;
    while let Some(entrada) = dir.next_entry().await? {
        let nome = entrada.file_name();
        let Some(texto) = nome.to_str() else { continue };
        if vivos.contains(texto) {
            continue;
        }
        let Ok(meta) = entrada.metadata().await else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        apagados += 1;
        bytes += meta.len();
        if !params.dry_run {
            if let Err(e) = tokio::fs::remove_file(entrada.path()).await {
                tracing::warn!(arquivo = %texto, error = %e, "não apagou artwork órfão");
            }
        }
    }

    Ok(Json(json!({
        "dry_run": params.dry_run,
        "arquivos": apagados,
        "bytes": bytes,
        "gb": format!("{:.2}", bytes as f64 / 1e9),
    })))
}
