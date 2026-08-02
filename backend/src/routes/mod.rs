pub mod auth;
pub mod browse;
pub mod curation;
pub mod graph;
pub mod live;
pub mod metadata;
pub mod people;
pub mod playback;
pub mod scopes;
pub mod scrub;
pub mod stream;
pub mod works;

use axum::extract::{Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::auth::AdminUser;
use crate::error::AppResult;
use crate::models::{Library, NewLibrary};
use crate::scanner;
use crate::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        // --- autenticação ---
        .route("/api/auth/status", get(auth::status))
        .route("/api/auth/setup", post(auth::setup))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/password", post(auth::change_password))
        .route(
            "/api/auth/sessions",
            get(auth::sessions).delete(auth::revoke_all),
        )
        .route(
            "/api/auth/users",
            get(auth::list_users).post(auth::create_user),
        )
        .route(
            "/api/auth/users/{id}",
            delete(auth::delete_user).patch(auth::update_user),
        )
        .route("/api/auth/sessions/{id}", delete(auth::revoke_one))
        .route("/api/libraries", get(list_libraries).post(create_library))
        .route("/api/libraries/{id}", delete(delete_library).patch(update_library))
        .route("/api/browse", get(browse::browse))
        .route("/api/scan", post(start_scan))
        .route("/api/scan/status", get(scan_status))
        // Histórico e controle das operações longas — ver o módulo `jobs`.
        .route("/api/jobs", get(list_jobs))
        .route("/api/jobs/{id}/cancel", post(cancel_job))
        .route("/api/works", get(works::list))
        // A biblioteca agrupada: série vira UMA entrada. Rota separada de
        // `/api/works` de propósito — dentro de uma coleção o que se quer é a
        // lista plana de episódios, e é `/api/works` que responde isso.
        .route("/api/library", get(works::library))
        .route("/api/works/{id}", get(works::detail).delete(works::delete_work))
        .route("/api/works/{id}/progress", post(works::progress))
        .route("/api/continue", get(works::continue_watching))
        .route("/api/stream/{media_file_id}", get(stream::stream))
        // --- M1: identidade ---
        .route("/api/match", post(metadata::start))
        .route("/api/match/status", get(metadata::status))
        .route("/api/review", get(metadata::review))
        // Re-deriva o parse do que ainda não foi identificado. `dry_run` por
        // padrão — ver o comentário no handler.
        .route("/api/maintenance/reparse", post(metadata::reparse))
        .route(
            "/api/maintenance/repair-episode-titles",
            post(metadata::repair_episode_titles),
        )
        .route(
            "/api/maintenance/repair-series",
            post(metadata::repair_series),
        )
        .route(
            "/api/maintenance/artwork-orfao",
            post(metadata::limpar_artwork_orfao),
        )
        // A pasta como unidade de decisão — ver o módulo `scopes`.
        .route("/api/review/scopes", get(scopes::list))
        .route("/api/scopes/search", post(scopes::search))
        .route("/api/scopes/identify", post(scopes::identify))
        .route("/api/works/{id}/candidates", get(metadata::candidates))
        .route("/api/works/{id}/search", post(metadata::manual_search))
        .route("/api/works/{id}/match", post(metadata::confirm))
        // Lote. Ver os handlers: cada obra leva o SEU candidato, e mudança
        // de estado em massa só aceita os estados que não desfazem match bom.
        .route("/api/works/bulk/match", post(metadata::bulk_match))
        .route("/api/works/bulk/state", post(metadata::bulk_state))
        .route("/api/works/{id}/reset", post(metadata::reset))
        .route("/api/works/{id}/ignore", post(works::ignore_work))
        .route("/api/storage", get(works::storage))
        .route("/api/diagnostico", get(works::diagnostico))
        // Correção humana do parse, persistida — ver o handler.
        .route(
            "/api/works/{id}/parse",
            post(metadata::set_parse).delete(metadata::clear_parse),
        )
        // --- M2: o grafo ---
        .route("/api/people", get(people::list))
        .route("/api/people/{id}", get(people::detail))
        .route("/api/works/{id}/credits", get(people::work_credits))
        .route("/api/tags", get(graph::list_tags))
        .route("/api/tag-namespaces", get(graph::list_namespaces))
        .route(
            "/api/works/{id}/tags",
            get(graph::work_tags).post(graph::attach_tag),
        )
        .route("/api/works/{id}/tags/{tag_id}", delete(graph::detach_tag))
        .route(
            "/api/collections",
            get(graph::list_collections).post(graph::create_collection),
        )
        .route("/api/collections/tree", get(graph::collection_tree))
        .route(
            "/api/collections/{id}",
            get(graph::collection_detail)
                .patch(graph::update_collection)
                .delete(graph::delete_collection),
        )
        .route("/api/collections/{id}/items", post(graph::add_item))
        .route(
            "/api/collections/{id}/items/{work_id}",
            delete(graph::remove_item),
        )
        .route("/api/collections/{id}/order", put(graph::reorder))
        .route(
            "/api/works/{id}/relations",
            get(graph::relations).post(graph::create_relation),
        )
        .route(
            "/api/works/{id}/relations/{other}/{kind}",
            delete(graph::delete_relation),
        )
        // --- M3: a alma ---
        .route("/api/scrub", post(scrub::start))
        .route("/api/scrub/status", get(scrub::status))
        .route("/api/media/{media_file_id}/scrub", get(scrub::info))
        .route("/api/events", get(crate::events::stream))
        // --- M5: curadoria ---
        .route("/api/curation/for-you", get(curation::for_you))
        .route("/api/curation/taste", get(curation::taste))
        .route("/api/curation/calibrar", get(curation::calibrar))
        .route("/api/curation/rebuild", post(curation::rebuild))
        .route("/api/curation/rebuild/status", get(curation::rebuild_status))
        .route("/api/works/{id}/similar", get(curation::similar))
        .route(
            "/api/works/{id}/feedback",
            post(curation::feedback).delete(curation::clear_feedback),
        )
        // --- M6: playback pesado ---
        .route("/api/playback/{media_file_id}/plan", get(playback::plan))
        .route("/api/playback/{media_file_id}/session", post(playback::start_session))
        .route("/api/hls/{session_id}/{filename}", get(playback::hls_file))
        .route("/api/hls/{session_id}", delete(playback::stop_session))
        .route("/api/transcode/capabilities", get(playback::capabilities))
        // --- R6: canais ao vivo ---
        .route("/api/live/channels", get(live::channels))
        .route("/api/live/guide", get(live::guide))
        .route("/api/live/odeon", get(live::odeon_guide))
        .route("/api/live/sources", get(live::sources).post(live::create_source))
        .route("/api/live/sources/{id}", delete(live::delete_source))
        .route("/api/live/import", post(live::import))
        .route("/api/live/{id}/watch", post(live::watch))
        .route("/api/live/reminders", get(live::reminders))
        .route(
            "/api/live/reminders/{programme_id}",
            post(live::create_reminder).delete(live::delete_reminder),
        )
        .route("/api/transcode/sessions", get(playback::sessions))
        .route("/api/media/{media_file_id}/subtitles", get(playback::list_subtitles))
        .route(
            "/api/media/{media_file_id}/subtitles/{index}",
            get(playback::subtitle_vtt),
        )
        .with_state(state)
}

/// O histórico das operações longas. É o que responde "quando foi a última
/// varredura?" e "o que estava rodando quando o servidor caiu?".
async fn list_jobs(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
) -> AppResult<Json<Value>> {
    Ok(Json(json!(crate::jobs::list(&state.pool, 50).await)))
}

/// Pede o cancelamento. Não mata nada: marca, e o worker para no próximo ponto
/// seguro — interromper no meio de uma gravação deixaria estado pela metade.
async fn cancel_job(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> AppResult<Json<Value>> {
    let pedido = crate::jobs::request_cancel(&state.pool, id).await;
    Ok(Json(json!({
        "ok": pedido,
        "detalhe": if pedido {
            "vai parar no próximo item — o corrente termina de gravar"
        } else {
            "esse job não está rodando"
        }
    })))
}

async fn health(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let one: i32 = sqlx::query_scalar("SELECT 1").fetch_one(&state.pool).await?;
    Ok(Json(json!({
        "status": "ok",
        "db": one == 1,
        "version": env!("CARGO_PKG_VERSION"),
    })))
}

async fn list_libraries(State(state): State<AppState>) -> AppResult<Json<Vec<Library>>> {
    let libraries = sqlx::query_as::<_, Library>("SELECT * FROM library ORDER BY created_at")
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(libraries))
}

async fn create_library(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(body): Json<NewLibrary>,
) -> AppResult<Json<Library>> {
    // Mesma checagem do navegador de pastas: sem isto a API aceitaria `/etc`
    // e o scanner sairia lendo o container inteiro.
    let path = std::path::Path::new(&body.root_path)
        .canonicalize()
        .map_err(|_| crate::error::AppError::BadRequest(
            format!("a pasta {} não existe no servidor", body.root_path)
        ))?;

    let inside = state.config.media_roots.iter().any(|root| {
        root.canonicalize().map(|r| path.starts_with(&r)).unwrap_or(false)
    });
    if !inside {
        return Err(crate::error::AppError::Forbidden(
            "essa pasta não está montada no servidor — veja MEDIA_PATH no .env".into(),
        ));
    }

    // Biblioteca aninhada dentro de outra é um estado que nunca funciona: um
    // arquivo pertence a UMA biblioteca (o `path` é UNIQUE), então a de dentro
    // nasce vazia e a pessoa fica achando que o scan quebrou.
    let existing: Vec<(uuid::Uuid, String, String)> =
        sqlx::query_as("SELECT id, name, root_path FROM library")
            .fetch_all(&state.pool)
            .await?;

    for (_, name, root) in &existing {
        let other = std::path::Path::new(root);
        if path.starts_with(other) {
            return Err(crate::error::AppError::BadRequest(format!(
                "essa pasta já está dentro da biblioteca \"{name}\" ({root}). \
                 Remova aquela primeiro, ou aponte esta pra outro lugar."
            )));
        }
        if other.starts_with(&path) {
            return Err(crate::error::AppError::BadRequest(format!(
                "essa pasta contém a biblioteca \"{name}\" ({root}). \
                 Remova aquela primeiro, ou escolha uma subpasta."
            )));
        }
    }

    let library = sqlx::query_as::<_, Library>(
        "INSERT INTO library (name, root_path, default_kind, provider_hint)
         VALUES ($1, $2, $3, $4) RETURNING *",
    )
    .bind(&body.name)
    .bind(path.to_string_lossy().to_string())
    .bind(&body.default_kind)
    .bind(&body.provider_hint)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            crate::error::AppError::BadRequest("já existe biblioteca nessa pasta".into())
        }
        _ => crate::error::AppError::Db(e),
    })?;
    Ok(Json(library))
}

/// Varre. Com `?then=match`, encadeia a identificação em seguida.
///
/// O encadeamento existe porque a sequência é sempre a mesma e a espera é
/// longa: varrer 17 mil arquivos leva uma hora, identificar leva mais, e sem
/// isto alguém precisa estar presente no meio pra apertar o segundo botão.
///
/// A identificação só começa se a varredura **terminou de verdade**. Depois de
/// um cancelamento ou de uma falha, encadear seria identificar sobre um
/// acervo pela metade.
async fn start_scan(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<ScanRequest>,
) -> AppResult<Json<Value>> {
    if state.scan.lock().await.running {
        return Ok(Json(json!({ "started": false, "reason": "scan já em andamento" })));
    }

    let pool = state.pool.clone();
    let status = state.scan.clone();
    let job = crate::jobs::Job::start(&state.pool, "scan", json!({}), None).await;
    let job_id = job.as_ref().map(|j| j.id);

    let encadear = params.then.as_deref() == Some("match");
    let bus = state.events.clone();
    let depois = state.clone();

    tokio::spawn(async move {
        scanner::scan_all(pool, status.clone(), job).await;
        let finished = status.lock().await.clone();
        crate::events::publish(
            &bus,
            crate::events::AppEvent::ScanFinished {
                added: finished.files_added,
                updated: finished.files_updated,
            },
        );

        if !encadear {
            return;
        }

        // Só encadeia se a varredura chegou ao fim. `finished_at` é carimbado
        // tanto no sucesso quanto no cancelamento, então quem responde por isso
        // é o ESTADO do job, não a existência da data.
        let concluiu = crate::jobs::latest(&depois.pool, "scan")
            .await
            .map(|j| j.state == "succeeded")
            .unwrap_or(false);
        if !concluiu {
            tracing::info!("varredura não concluiu — identificação não encadeada");
            return;
        }

        let job_match =
            crate::jobs::Job::start(&depois.pool, "match", json!({ "chained": true }), None).await;
        tracing::info!("varredura concluída — identificação encadeada");
        crate::metadata::run_matching(
            depois.pool.clone(),
            depois.providers.clone(),
            depois.config.artwork_dir.clone(),
            depois.matching.clone(),
            false,
            job_match,
        )
        .await;
        let m = depois.matching.lock().await.clone();
        crate::events::publish(
            &bus,
            crate::events::AppEvent::MatchFinished {
                auto: m.matched_auto,
                needs_review: m.needs_review,
            },
        );
    });

    Ok(Json(json!({
        "started": true,
        "job_id": job_id,
        "then": if encadear { Some("match") } else { None },
    })))
}

#[derive(serde::Deserialize)]
struct ScanRequest {
    /// `match` encadeia a identificação depois da varredura.
    then: Option<String>,
}

/// Apagar biblioteca leva junto arquivos e obras (cascade no schema). O
/// histórico de reprodução some com as obras — por isso a confirmação fica na
/// interface, e a API é explícita sobre o que removeu.
async fn delete_library(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> AppResult<Json<Value>> {
    let works: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT work_id) FROM media_file WHERE library_id = $1 AND work_id IS NOT NULL",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await?;

    let mut tx = state.pool.begin().await?;

    let result = sqlx::query("DELETE FROM library WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    // Desfaz na mão em vez de contar com o `Drop`. O `Drop` do sqlx também
    // desfaz, mas só quando a transação é destruída, e sem `await` — ele agenda
    // o rollback no pool. Fechar aqui devolve a conexão imediatamente e deixa a
    // intenção escrita: biblioteca inexistente não mexe em nada.
    if result.rows_affected() == 0 {
        tx.rollback().await?;
        return Err(crate::error::AppError::NotFound);
    }

    // O cascade leva os `media_file` junto, mas `work` não tem FK pra library —
    // sem isto as obras ficam órfãs, sem arquivo nenhum, e a biblioteca passa a
    // mostrar cartões que não tocam.
    let orphans = sqlx::query(
        "DELETE FROM work w WHERE NOT EXISTS (SELECT 1 FROM media_file m WHERE m.work_id = w.id)",
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(json!({
        "ok": true,
        "works_removed": orphans.rows_affected().max(works as u64),
    })))
}

#[derive(serde::Deserialize)]
struct UpdateLibrary {
    name: Option<String>,
    default_kind: Option<String>,
    provider_hint: Option<String>,
}

async fn update_library(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    Json(body): Json<UpdateLibrary>,
) -> AppResult<Json<Library>> {
    sqlx::query(
        "UPDATE library SET
            name = COALESCE($2, name),
            default_kind = COALESCE($3, default_kind),
            provider_hint = COALESCE($4, provider_hint)
         WHERE id = $1",
    )
    .bind(id)
    .bind(body.name)
    .bind(body.default_kind)
    .bind(body.provider_hint)
    .execute(&state.pool)
    .await?;

    sqlx::query_as::<_, Library>("SELECT * FROM library WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .map(Json)
        .ok_or(crate::error::AppError::NotFound)
}

/// Mesmo formato de sempre; a diferença é sobreviver ao restart.
///
/// Sem isto, o `systemctl stop docker` que matou uma varredura de 17 mil
/// arquivos deixava a interface dizendo "nunca rodou" — que foi exatamente o
/// que aconteceu na implantação deste servidor.
async fn scan_status(State(state): State<AppState>) -> Json<scanner::ScanStatus> {
    let current = state.scan.lock().await.clone();
    if current.started_at.is_some() {
        return Json(current);
    }

    if let Some(job) = crate::jobs::latest(&state.pool, "scan").await {
        if let Ok(mut anterior) = serde_json::from_value::<scanner::ScanStatus>(job.progress) {
            // O processo que executava aquilo não existe mais.
            anterior.running = false;
            return Json(anterior);
        }
    }
    Json(current)
}
