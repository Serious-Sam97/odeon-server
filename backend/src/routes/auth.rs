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

/// As duas formas de o token de sessão chegar. São as mesmas duas que o
/// `require_auth` aceita — e é por isso que o `logout` tem de conhecer as duas:
/// revogar só a que veio no header deixava a sessão da web viva.
fn bearer_do_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_string)
}

fn token_do_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|parte| parte.trim().split_once('='))
        .find(|(nome, _)| *nome == COOKIE_NAME)
        .map(|(_, valor)| valor.to_string())
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

/// Cria a sessão e monta a resposta — token no corpo, cookie no cabeçalho.
///
/// `pub(crate)` por causa da R46: o resgate de pareamento entra por outro
/// módulo e tem que sair **exatamente** com esta resposta. Um segundo lugar
/// montando `LoginResponse` na mão é um segundo lugar pra esquecer o cookie.
pub(crate) async fn issue(
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
    // R61: qual aparelho pediu. Sem isto a poda era por conta, e o celular
    // derrubava a TV no meio do filme.
    sessao: Option<axum::Extension<auth::Sessao>>,
) -> AppResult<Json<Value>> {
    let sessao = sessao.map(|axum::Extension(auth::Sessao(id))| id);
    let token = auth::emitir_token_de_midia(&state.pool, user.id, sessao).await?;
    Ok(Json(json!({
        "token": token,
        "horas": auth::horas_do_token_de_midia(),
    })))
}

/// Emite um token de **arte** (R45), pra URL que vai ser guardada fora do app.
///
/// O caso é a fileira da home da Google TV: o que se entrega ao `TvProvider` é
/// uma `Uri`, e quem a baixa é o launcher, dias depois, com o Odeon fechado.
/// Um token de mídia não serve ali — ele vence em oito horas e é aposentado
/// quando qualquer outro aparelho pede o dele.
pub async fn artwork_token(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Value>> {
    let token = auth::emitir_token_de_arte(&state.pool, user.id).await?;
    Ok(Json(json!({
        "token": token,
        "dias": auth::dias_do_token_de_arte(),
    })))
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    // Revoga o token exato que veio, não todos os do usuário: deslogar o
    // notebook não pode derrubar a TV.
    //
    // **Header OU cookie** (R61). Só o header era metade do trabalho: o `login`
    // devolve as duas coisas e a web usa o cookie, então "sair" ali limpava o
    // cookie do navegador e **deixava a sessão viva no servidor** — 90 dias de
    // credencial que a pessoa acredita ter encerrado. Medido nesta casa: dois
    // `POST /api/auth/logout` responderam 200 e as duas linhas continuaram em
    // `auth_session`.
    //
    // A ordem é a mesma do `require_auth`: header primeiro, cookie depois.
    let token = bearer_do_header(&headers).or_else(|| token_do_cookie(&headers));
    if let Some(token) = token {
        auth::revoke(&state.pool, &token).await;
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
    // E os tokens, os dois escopos (R45). O de arte dura um ano e sobrevive de
    // propósito ao logout — este é o único gesto que o derruba, e tem que ser,
    // senão "achei que alguém entrou" deixaria uma credencial de um ano viva.
    sqlx::query("DELETE FROM media_token WHERE user_id = $1")
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
    /// Renomear outra pessoa. Existe pra conta que nasceu com nome errado — o
    /// admin digita o nome no `create_user` e no convite, então o erro é dele e
    /// era ele quem não tinha como consertar.
    ///
    /// **Só o `display_name`.** O `username` é a identidade de entrada e o
    /// endereço `/p/<nome>` que o próprio perfil distribui; trocá-lo por fora
    /// quebraria os links de alguém sem essa pessoa saber.
    #[serde(default)]
    pub display_name: Option<String>,
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

    // Vazio depois do `trim` é recusado, e não tratado como "não mexe": o
    // `COALESCE` abaixo já cobre quem não mandou o campo, então um campo mandado
    // em branco é um pedido — e o pedido é impossível.
    let nome = match body.display_name.as_deref().map(str::trim) {
        Some("") => return Err(AppError::BadRequest("o nome não pode ficar vazio".into())),
        outro => outro,
    };

    let r = sqlx::query(
        "UPDATE app_user SET role = COALESCE($2, role), is_active = COALESCE($3, is_active),
                             display_name = COALESCE($4, display_name)
         WHERE id = $1",
    )
    .bind(user_id)
    .bind(&body.role)
    .bind(body.is_active)
    .bind(nome)
    .execute(&state.pool)
    .await
    .map_err(|e| match &e {
        // O limite é o `CHECK` da 0038. Aqui ele só vira 400 em vez de 500.
        sqlx::Error::Database(db)
            if db.constraint() == Some("app_user_display_name_check") =>
        {
            AppError::BadRequest("o nome precisa ter de 1 a 40 caracteres".into())
        }
        _ => AppError::Db(e),
    })?;
    if r.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cabecalhos(pares: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (nome, valor) in pares {
            h.insert(
                header::HeaderName::from_bytes(nome.as_bytes()).unwrap(),
                HeaderValue::from_str(valor).unwrap(),
            );
        }
        h
    }

    /// **R61.** O `logout` só olhava o header, e a web autentica por cookie:
    /// sair limpava o cookie do navegador e deixava a sessão viva no servidor
    /// por 90 dias. Medido — dois `POST /api/auth/logout` responderam 200 e as
    /// duas linhas continuaram em `auth_session`.
    #[test]
    fn o_logout_acha_o_token_no_cookie_tambem() {
        let so_cookie = cabecalhos(&[("cookie", "outro=1; odeon_session=abc123; mais=2")]);
        assert_eq!(
            bearer_do_header(&so_cookie).or_else(|| token_do_cookie(&so_cookie)),
            Some("abc123".to_string())
        );
    }

    /// O header ganha do cookie, na mesma ordem do `require_auth`. Divergir
    /// faria o `logout` revogar uma sessão diferente da que abriu a chamada.
    #[test]
    fn o_header_ganha_do_cookie() {
        let ambos = cabecalhos(&[
            ("authorization", "Bearer doheader"),
            ("cookie", "odeon_session=docookie"),
        ]);
        assert_eq!(
            bearer_do_header(&ambos).or_else(|| token_do_cookie(&ambos)),
            Some("doheader".to_string())
        );
    }

    /// Sem nenhum dos dois não há o que revogar — e não pode explodir: o
    /// `logout` é público, então ele recebe requisição sem credencial nenhuma.
    #[test]
    fn logout_sem_credencial_nao_explode() {
        let vazio = cabecalhos(&[]);
        assert_eq!(bearer_do_header(&vazio).or_else(|| token_do_cookie(&vazio)), None);
        let cookie_sem_o_nosso = cabecalhos(&[("cookie", "tema=escuro")]);
        assert_eq!(token_do_cookie(&cookie_sem_o_nosso), None);
    }
}
