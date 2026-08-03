use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{self, middleware::COOKIE_NAME, AdminUser, AuthUser, Credentials, SessionRow, User};
use crate::error::{AppError, AppResult};
use crate::AppState;

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub user: User,
}

/// Sessão longa: é servidor de casa, e a revogação por aparelho já cobre perda.
const COOKIE_MAX_AGE: i64 = 90 * 24 * 3600;

/// O cookie serve pro caso web-e-API-na-mesma-origem. `Lax` porque `None`
/// exigiria `Secure`, e não há HTTPS numa tailnet HTTP. Cross-origin usa Bearer.
fn session_cookie(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={COOKIE_MAX_AGE}"
    ))
    .expect("token é hexadecimal")
}

fn clear_cookie() -> HeaderValue {
    HeaderValue::from_static("odeon_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)?
        .to_str()
        .ok()
        .map(|value| value.chars().take(200).collect())
}

/// Pública. Diz se é primeira execução — a UI decide entre login e setup.
pub async fn status(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "needs_setup": auth::needs_setup(&state.pool).await }))
}

/// Cria (ou reivindica) o primeiro administrador.
///
/// Só responde enquanto NINGUÉM tem senha. Depois disso vira 403 — senão
/// qualquer um criaria admin a qualquer momento.
pub async fn setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Credentials>,
) -> AppResult<Response> {
    if !auth::needs_setup(&state.pool).await {
        return Err(AppError::Forbidden(
            "já existe administrador — use o login".into(),
        ));
    }

    let username = body.username.trim().to_lowercase();
    if username.is_empty() {
        return Err(AppError::BadRequest("usuário é obrigatório".into()));
    }
    let hash = auth::hash_password(&body.password)?;

    // Se o usuário semeado no M0 ainda está sem senha, ele é REIVINDICADO em
    // vez de duplicado — assim todo o histórico de reprodução continua sendo
    // desta pessoa, e não fica órfão numa conta fantasma.
    let existing: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM app_user WHERE password_hash IS NULL ORDER BY created_at LIMIT 1")
            .fetch_optional(&state.pool)
            .await?;

    let user_id = match existing {
        Some(id) => {
            sqlx::query(
                "UPDATE app_user SET username = $2, display_name = $3, password_hash = $4,
                                     role = 'admin', is_active = true
                 WHERE id = $1",
            )
            .bind(id)
            .bind(&username)
            .bind(&body.username)
            .bind(&hash)
            .execute(&state.pool)
            .await?;
            id
        }
        None => sqlx::query_scalar(
            "INSERT INTO app_user (username, display_name, password_hash, role)
             VALUES ($1, $2, $3, 'admin') RETURNING id",
        )
        .bind(&username)
        .bind(&body.username)
        .bind(&hash)
        .fetch_one(&state.pool)
        .await?,
    };

    issue(&state, user_id, body.device_label.as_deref(), &headers).await
}

pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Credentials>,
) -> AppResult<Response> {
    let username = body.username.trim().to_lowercase();

    let row: Option<(Uuid, Option<String>, bool)> = sqlx::query_as(
        "SELECT id, password_hash, is_active FROM app_user WHERE lower(username) = $1",
    )
    .bind(&username)
    .fetch_optional(&state.pool)
    .await?;

    // Mensagem idêntica pros três casos (não existe / sem senha / senha errada):
    // distinguir entregaria de graça a lista de usuários válidos.
    let invalid = || AppError::Unauthorized;

    let (user_id, stored, active) = row.ok_or_else(invalid)?;
    let stored = stored.ok_or_else(invalid)?;
    if !active || !auth::verify_password(&body.password, &stored) {
        return Err(invalid());
    }

    sqlx::query("UPDATE app_user SET last_login_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(&state.pool)
        .await?;

    issue(&state, user_id, body.device_label.as_deref(), &headers).await
}

async fn issue(
    state: &AppState,
    user_id: Uuid,
    device_label: Option<&str>,
    headers: &HeaderMap,
) -> AppResult<Response> {
    let token = auth::create_session(
        &state.pool,
        user_id,
        device_label,
        user_agent(headers).as_deref(),
    )
    .await?;

    let user = auth::load_user(state, user_id)
        .await
        .ok_or(AppError::NotFound)?;

    let mut response = Json(LoginResponse {
        token: token.clone(),
        user,
    })
    .into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, session_cookie(&token));
    Ok(response)
}

pub async fn me(AuthUser(user): AuthUser) -> Json<User> {
    Json(user)
}

/// Emite um token de mídia pro aparelho que está pedindo.
///
/// Autenticada por header ou cookie, como qualquer rota de API — o token de
/// mídia é o que ela **devolve**, nunca o que a abre.
pub async fn media_token(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Value>> {
    let token = auth::emitir_token_de_midia(&state.pool, user.id).await?;
    Ok(Json(json!({
        "token": token,
        "horas": auth::horas_do_token_de_midia(),
    })))
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    // Revoga o token exato que veio, não todos os do usuário: deslogar o
    // notebook não pode derrubar a TV.
    if let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        auth::revoke(&state.pool, token).await;
    }

    let mut response = Json(json!({ "ok": true })).into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, clear_cookie());
    Ok(response)
}

#[derive(Debug, Deserialize)]
pub struct ChangePassword {
    pub current_password: String,
    pub new_password: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<ChangePassword>,
) -> AppResult<Json<Value>> {
    let stored: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM app_user WHERE id = $1")
            .bind(user.id)
            .fetch_one(&state.pool)
            .await?;

    let stored = stored.ok_or(AppError::Unauthorized)?;
    if !auth::verify_password(&body.current_password, &stored) {
        return Err(AppError::Unauthorized);
    }

    let hash = auth::hash_password(&body.new_password)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE app_user SET password_hash = $2 WHERE id = $1")
        .bind(user.id)
        .bind(&hash)
        .execute(&mut *tx)
        .await?;
    // Trocar senha derruba as outras sessões: é o gesto de quem acha que
    // alguém entrou. Manter as antigas vivas anularia o motivo da troca.
    sqlx::query("DELETE FROM auth_session WHERE user_id = $1")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(
        json!({ "ok": true, "note": "as outras sessões foram encerradas" }),
    ))
}

pub async fn sessions(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Vec<SessionRow>>> {
    // O `id` é o que permite encerrar UM aparelho. Sem ele, a única saída era
    // revogar tudo — inclusive a sessão de quem estava olhando a lista.
    let rows = sqlx::query_as::<_, SessionRow>(
        "SELECT id, device_label, user_agent, created_at, last_seen_at, expires_at
         FROM auth_session WHERE user_id = $1 ORDER BY last_seen_at DESC",
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

pub async fn revoke_all(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Value>> {
    let result = sqlx::query("DELETE FROM auth_session WHERE user_id = $1")
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "revoked": result.rows_affected() })))
}

// ------------------------------------------------------- admin: usuários

pub async fn list_users(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
) -> AppResult<Json<Vec<User>>> {
    let users = sqlx::query_as::<_, User>(
        "SELECT id, username, display_name, role, is_active, created_at, last_login_at
         FROM app_user ORDER BY created_at",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(users))
}

#[derive(Debug, Deserialize)]
pub struct NewUser {
    pub username: String,
    pub display_name: Option<String>,
    pub password: String,
    #[serde(default = "default_role")]
    pub role: String,
}

fn default_role() -> String {
    "user".to_string()
}

pub async fn create_user(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(body): Json<NewUser>,
) -> AppResult<Json<User>> {
    if !matches!(body.role.as_str(), "admin" | "user") {
        return Err(AppError::BadRequest("role deve ser admin ou user".into()));
    }
    let username = body.username.trim().to_lowercase();
    if username.is_empty() {
        return Err(AppError::BadRequest("usuário é obrigatório".into()));
    }

    let hash = auth::hash_password(&body.password)?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO app_user (username, display_name, password_hash, role)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(&username)
    .bind(body.display_name.unwrap_or_else(|| body.username.clone()))
    .bind(&hash)
    .bind(&body.role)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            AppError::BadRequest(format!("o usuário {username} já existe"))
        }
        _ => AppError::Db(e),
    })?;

    auth::load_user(&state, id).await.map(Json).ok_or(AppError::NotFound)
}

pub async fn delete_user(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(user_id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    if user_id == admin.id {
        return Err(AppError::BadRequest(
            "não dá pra apagar a própria conta".into(),
        ));
    }

    // Ficar sem nenhum admin é um estado do qual não se sai sem mexer no banco.
    let admins: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app_user WHERE role = 'admin' AND is_active AND id <> $1",
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await?;
    if admins == 0 {
        return Err(AppError::BadRequest(
            "isso deixaria o servidor sem administrador".into(),
        ));
    }

    sqlx::query("DELETE FROM app_user WHERE id = $1")
        .bind(user_id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

/// Encerra UM aparelho.
///
/// O `revoke_all` já existia, mas ele derruba também quem está olhando a
/// lista — serve para "perdi o notebook", não para arrumar a casa. Aqui a
/// sessão é escolhida a dedo, e só entre as do próprio usuário: o `WHERE`
/// carrega o `user_id`, então nem um admin encerra a sessão de outra pessoa
/// por esta porta.
pub async fn revoke_one(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(session_id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let r = sqlx::query("DELETE FROM auth_session WHERE id = $1 AND user_id = $2")
        .bind(session_id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    if r.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct MudaUsuario {
    pub role: Option<String>,
    pub is_active: Option<bool>,
}

/// Muda papel e estado de um usuário.
///
/// As mesmas duas travas do `delete_user`, pelas mesmas razões: ninguém se
/// rebaixa nem se desativa sozinho (é um beco sem saída), e o servidor não
/// pode ficar sem nenhum administrador ativo — desse estado não se sai sem
/// mexer no banco à mão.
pub async fn update_user(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(user_id): Path<Uuid>,
    Json(body): Json<MudaUsuario>,
) -> AppResult<Json<Value>> {
    if let Some(r) = &body.role {
        if !matches!(r.as_str(), "admin" | "user") {
            return Err(AppError::BadRequest("papel deve ser admin ou user".into()));
        }
    }
    if user_id == admin.id && (body.role.as_deref() == Some("user") || body.is_active == Some(false))
    {
        return Err(AppError::BadRequest(
            "não dá pra tirar o próprio acesso".into(),
        ));
    }

    let sobram: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM app_user WHERE role = 'admin' AND is_active AND id <> $1",
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await?;
    let deixaria_de_ser_admin =
        body.role.as_deref() == Some("user") || body.is_active == Some(false);
    if sobram == 0 && deixaria_de_ser_admin {
        return Err(AppError::BadRequest(
            "isso deixaria o servidor sem administrador".into(),
        ));
    }

    let r = sqlx::query(
        "UPDATE app_user SET role = COALESCE($2, role), is_active = COALESCE($3, is_active)
         WHERE id = $1",
    )
    .bind(user_id)
    .bind(&body.role)
    .bind(body.is_active)
    .execute(&state.pool)
    .await?;
    if r.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}
