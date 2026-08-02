use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::AppState;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PersonRow {
    pub id: Uuid,
    pub name: String,
    pub known_for: Option<String>,
    /// Caminho relativo servido em `/artwork/...`; `None` até baixar.
    pub image_path: Option<String>,
    pub work_count: i64,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CreditRow {
    pub person_id: Uuid,
    pub name: String,
    pub role: String,
    pub role_label: String,
    pub character_name: Option<String>,
    pub position: Option<i32>,
    pub image_path: Option<String>,
    pub featured: bool,
    pub role_position: i32,
}

const CREDITS_SQL: &str = r#"
SELECT c.person_id, p.name, c.role,
       COALESCE(r.label, initcap(c.role)) AS role_label,
       c.character_name, c.position, p.image_path,
       COALESCE(r.featured, false) AS featured,
       COALESCE(r.position, 999) AS role_position
FROM credit c
JOIN person p ON p.id = c.person_id
LEFT JOIN credit_role r ON r.role = c.role
WHERE c.work_id = $1
ORDER BY role_position, c.position NULLS LAST, p.name
"#;

pub async fn credits_of(state: &AppState, work_id: Uuid) -> AppResult<Vec<CreditRow>> {
    Ok(sqlx::query_as::<_, CreditRow>(CREDITS_SQL)
        .bind(work_id)
        .fetch_all(&state.pool)
        .await?)
}

pub async fn work_credits(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(work_id): Path<Uuid>,
) -> AppResult<Json<Vec<CreditRow>>> {
    Ok(Json(credits_of(&state, work_id).await?))
}

#[derive(Debug, Deserialize)]
pub struct PeopleQuery {
    #[serde(default)]
    pub q: Option<String>,
    /// Filtra por papel: só diretores, só elenco…
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    60
}

/// Quem existe na biblioteca, ordenado por quantidade de trabalhos. É a
/// pergunta "de quem eu tenho mais coisa aqui?".
pub async fn list(
    State(state): State<AppState>,
    _user: AuthUser,
    Query(params): Query<PeopleQuery>,
) -> AppResult<Json<Vec<PersonRow>>> {
    let people = sqlx::query_as::<_, PersonRow>(
        r#"
        SELECT p.id, p.name, p.known_for, p.image_path,
               count(DISTINCT c.work_id) AS work_count
        FROM person p
        JOIN credit c ON c.person_id = p.id
        WHERE ($1::text IS NULL OR p.name ILIKE '%' || $1 || '%')
          AND ($2::text IS NULL OR c.role = $2)
        GROUP BY p.id
        ORDER BY work_count DESC, p.name
        LIMIT $3
        "#,
    )
    .bind(params.q.filter(|q| !q.trim().is_empty()))
    .bind(params.role)
    .bind(params.limit.clamp(1, 200))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(people))
}

/// A pessoa e a filmografia dela **dentro da sua biblioteca** — não o catálogo
/// do TMDB. Listar filme que você não tem seria propaganda, não navegação.
pub async fn detail(
    State(state): State<AppState>,
    user: AuthUser,
    Path(person_id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let person = sqlx::query_as::<_, PersonRow>(
        "SELECT p.id, p.name, p.known_for, p.image_path,
                (SELECT count(DISTINCT work_id) FROM credit WHERE person_id = p.id) AS work_count
         FROM person p WHERE p.id = $1",
    )
    .bind(person_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let works = sqlx::query_as::<_, crate::models::WorkListItem>(
        r#"
        SELECT
            w.id, w.kind, w.title, w.year, w.season_number, w.episode_number,
            w.match_state, w.match_confidence, w.dominant_color,
            w.artwork->>'poster' AS poster,
            w.artwork->>'backdrop' AS backdrop,
            w.artwork->>'still' AS still,
            NULL::text AS series_title,
            f.id AS media_file_id, f.duration_seconds, f.width, f.height,
            f.video_codec, f.audio_codec, f.container, f.size_bytes,
            ps.position_seconds, ps.finished,
            tg.tags
        FROM credit c
        JOIN work w ON w.id = c.work_id
        LEFT JOIN LATERAL (
            SELECT m.* FROM media_file m
            WHERE m.work_id = w.id AND m.status = 'probed'
            ORDER BY m.size_bytes DESC LIMIT 1
        ) f ON true
        LEFT JOIN LATERAL (
            SELECT array_agg(t.namespace || ':' || t.value ORDER BY t.namespace, t.value) AS tags
            FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
            WHERE wt.work_id = w.id
        ) tg ON true
        LEFT JOIN playback_state ps ON ps.work_id = w.id AND ps.user_id = $2
        WHERE c.person_id = $1
        GROUP BY w.id, f.id, f.duration_seconds, f.width, f.height, f.video_codec,
                 f.audio_codec, f.container, f.size_bytes,
                 ps.position_seconds, ps.finished, tg.tags
        ORDER BY w.year DESC NULLS LAST, w.title
        "#,
    )
    .bind(person_id)
    .bind(user.id())
    .fetch_all(&state.pool)
    .await?;

    // Em que papéis esta pessoa aparece na sua biblioteca.
    let roles: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT c.role, COALESCE(r.label, initcap(c.role)), count(*)
         FROM credit c LEFT JOIN credit_role r ON r.role = c.role
         WHERE c.person_id = $1
         GROUP BY c.role, r.label, r.position
         ORDER BY COALESCE(r.position, 999)",
    )
    .bind(person_id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(json!({
        "person": person,
        "roles": roles.into_iter().map(|(role, label, count)| json!({
            "role": role, "label": label, "count": count,
        })).collect::<Vec<_>>(),
        "works": works,
    })))
}
