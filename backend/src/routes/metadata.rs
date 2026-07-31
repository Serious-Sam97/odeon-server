//! Identificação e fila de revisão.
//!
//! A diferença de filosofia em relação ao Jellyfin vive aqui: quando o matcher
//! não tem certeza, ele **não escreve nada** na obra. Ele grava os candidatos
//! com o score e os motivos, marca `needs_review`, e espera um humano.

use axum::extract::{Path as AxumPath, Query, State};
use axum::Json;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::{AppError, AppResult};
use crate::metadata::{self, score, Candidate};
use crate::models::{
    ConfirmMatch, GuessView, ManualSearch, MatchCandidateRow, MatchRequest, ReviewItem, ReviewWork,
};
use crate::scanner::guess::Guess;
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

    let bus = state.events.clone();
    tokio::spawn(async move {
        metadata::run_matching(pool, providers, artwork_dir, status.clone(), force).await;
        let finished = status.lock().await.clone();
        crate::events::publish(
            &bus,
            crate::events::AppEvent::MatchFinished {
                auto: finished.matched_auto,
                needs_review: finished.needs_review,
            },
        );
    });

    Ok(Json(json!({ "started": true, "force": force })))
}

pub async fn status(State(state): State<AppState>) -> Json<metadata::MatchStatus> {
    let mut current = state.matching.lock().await.clone();
    current.tmdb_enabled = state.providers.tmdb.is_some();
    Json(current)
}

/// A fila. Obras em que o matcher ficou em dúvida, com o que ele entendeu do
/// nome do arquivo ao lado dos candidatos — pra decisão levar dois segundos.
pub async fn review(State(state): State<AppState>) -> AppResult<Json<Vec<ReviewItem>>> {
    let works = sqlx::query_as::<_, ReviewWork>(
        r#"
        SELECT DISTINCT ON (w.id)
            w.id, w.title, w.year, w.kind, w.season_number, w.episode_number,
            w.match_state, w.match_confidence, m.filename
        FROM work w
        JOIN media_file m ON m.work_id = w.id
        WHERE w.match_state = 'needs_review'
        ORDER BY w.id, m.size_bytes DESC
        LIMIT 50
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    let mut items = Vec::with_capacity(works.len());
    for work in works {
        let candidates = candidates_for(&state, work.id).await?;
        let guess = metadata::load_work_context(&state.pool, work.id)
            .await
            .map(|(_, g)| view_of(&g))
            .unwrap_or_else(|_| view_of(&Guess::default()));

        items.push(ReviewItem {
            work,
            guess,
            candidates,
        });
    }

    Ok(Json(items))
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

    Ok(Json(json!({ "ok": true, "title": row.title })))
}

/// Desfaz a identificação e devolve a obra pra fila.
pub async fn reset(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    AxumPath(work_id): AxumPath<Uuid>,
) -> AppResult<Json<Value>> {
    sqlx::query(
        "UPDATE work SET match_state = 'unmatched', match_confidence = NULL,
                         matched_candidate_id = NULL, matched_at = NULL,
                         artwork = '{}'::jsonb, dominant_color = NULL, updated_at = now()
         WHERE id = $1",
    )
    .bind(work_id)
    .execute(&state.pool)
    .await?;
    Ok(Json(json!({ "ok": true })))
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
