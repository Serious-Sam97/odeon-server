pub mod acesso;
pub mod middleware;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::AppState;

/// 90 dias. É um servidor de casa: obrigar login semanal na TV seria hostil, e
/// a revogação por sessão já cobre o caso "perdi o aparelho".
const SESSION_DAYS: i64 = 90;

/// Validade do token de mídia, em horas — **o número da R27** (§43).
///
/// Ele tem que sobreviver a assistir a coisa mais longa do acervo sem
/// interrupção. Medido: o arquivo mais longo tem **4,9 h**, o filme mais longo
/// 4,04 h, 15 arquivos passam de 3 h e **nenhum passa de 5 h**.
///
/// Oito horas cobrem o maior arquivo com três de pausa por cima, e são
/// **1/270 da validade da sessão**. Menos que isso quebraria a reprodução no
/// meio; mais desfaria a razão de a fase existir.
const MEDIA_TOKEN_HOURS: i64 = 8;

/// Um ano — o prazo do token de **arte** (R45).
///
/// Ele não mede reprodução, mede esquecimento: o caso que a fileira da home da
/// Google TV precisa atender é o de quem passou um mês sem abrir o app e olha a
/// primeira tela da TV. As oito horas do token de mídia existem porque aquele
/// entrega o filme; este entrega pôster baixado do TMDB, e o que ele arrisca ao
/// vazar é revelar que esta casa tem a capa de tal filme.
///
/// Finito de propósito: a linha vence e some sozinha, e é isso que separa um
/// token longo de simplesmente deixar `/artwork` aberto.
const ARTE_TOKEN_DIAS: i64 = 365;

/// Quantos tokens de arte por pessoa sobrevivem à poda.
///
/// Não pode ser 1. Emitir um novo **não** pode derrubar o anterior: se
/// derrubasse, a segunda TV da casa apagaria a fileira da primeira, e um
/// aparelho republicando as URLs invalidaria as que ele mesmo acabou de
/// publicar. Cinco cobre "algumas telas na casa" e ainda é um teto.
const ARTE_TOKENS_POR_PESSOA: i64 = 5;

/// Abaixo disto nem adianta ter Argon2.
const MIN_PASSWORD_LEN: usize = 8;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    pub role: String,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SessionRow {
    pub id: Uuid,
    pub device_label: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub device_label: Option<String>,
}

// ------------------------------------------------------------------ senhas

pub fn hash_password(password: &str) -> Result<String, AppError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(AppError::BadRequest(format!(
            "a senha precisa de pelo menos {MIN_PASSWORD_LEN} caracteres"
        )));
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| AppError::Other(anyhow::anyhow!("falha ao cifrar a senha: {e}")))
}

pub fn verify_password(password: &str, stored: &str) -> bool {
    match PasswordHash::new(stored) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

// ------------------------------------------------------------------ tokens

/// 256 bits de `OsRng`. Entropia suficiente pra dispensar hash lento no
/// armazenamento — força bruta contra isso não é uma ameaça real.
fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

// ----------------------------------------------------------------- sessões

pub async fn create_session(
    pool: &PgPool,
    user_id: Uuid,
    device_label: Option<&str>,
    user_agent: Option<&str>,
) -> Result<String, AppError> {
    let token = generate_token();
    sqlx::query(
        "INSERT INTO auth_session (token_hash, user_id, device_label, user_agent, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(hash_token(&token))
    .bind(user_id)
    .bind(device_label)
    .bind(user_agent)
    .bind(Utc::now() + Duration::days(SESSION_DAYS))
    .execute(pool)
    .await?;
    Ok(token)
}

/// O aparelho que abriu a requisição, pra quem precisa dele no handler.
///
/// Envelope de um `Uuid` porque vai nas `extensions` da requisição, e lá o tipo
/// é a chave: um `Uuid` cru colidiria com qualquer outro que alguém puser.
/// Ausente quando a requisição chegou por token de mídia ou de arte, que não
/// pertencem a uma sessão.
#[derive(Debug, Clone, Copy)]
pub struct Sessao(pub Uuid);

/// Quem pediu, e de qual aparelho.
///
/// O `sessao_id` entrou na R61: sem ele o servidor sabe *de quem* é a
/// requisição e não *de onde*, e foi por isso que o token de mídia só sabia
/// podar por conta — derrubando a TV quando o celular renovava. Vem junto na
/// mesma consulta porque ela já lê a linha da sessão; perguntar de novo seria
/// uma ida ao banco por requisição.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Autenticado {
    #[sqlx(flatten)]
    pub user: User,
    pub sessao_id: Uuid,
}

/// Resolve o token e renova o `last_seen_at`. Sessão expirada é apagada na hora
/// em vez de esperar um job — a consulta já está aqui.
pub async fn user_for_token(pool: &PgPool, token: &str) -> Option<Autenticado> {
    let token_hash = hash_token(token);

    let user: Option<Autenticado> = sqlx::query_as(
        "SELECT u.id, u.username, u.display_name, u.role, u.is_active,
                u.created_at, u.last_login_at, s.id AS sessao_id
         FROM auth_session s JOIN app_user u ON u.id = s.user_id
         WHERE s.token_hash = $1 AND s.expires_at > now() AND u.is_active",
    )
    .bind(&token_hash)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    if user.is_some() {
        let _ = sqlx::query("UPDATE auth_session SET last_seen_at = now() WHERE token_hash = $1")
            .bind(&token_hash)
            .execute(pool)
            .await;
    } else {
        let _ = sqlx::query("DELETE FROM auth_session WHERE token_hash = $1 OR expires_at <= now()")
            .bind(&token_hash)
            .execute(pool)
            .await;
    }

    user
}

// ------------------------------------------------------- token de mídia
//
// O compromisso que o §9b declarou no M6 e que o §6.5 listou como dívida:
// `<video src>`, `<img src>` e `<track>` não mandam header, então o token vai
// na query — e query vaza pra log de acesso e histórico de navegador.
//
// Até a R27 o que ia ali era o **token de sessão**: 90 dias, acesso total à
// API. Agora vai um token que só abre mídia e vence em horas.

/// Emite um token de mídia pra **este aparelho** (R61).
///
/// **Aposenta o anterior do mesmo aparelho, e só dele.** Um aparelho que pede
/// um token novo é um aparelho que perdeu o antigo de vista; deixar os velhos
/// vivos só aumentaria a janela de um vazamento sem servir a ninguém. Mas isso
/// vale pra ele, não pra casa: a poda era por conta, e com celular e TV
/// abertos ao mesmo tempo um derrubava o outro no meio do filme.
///
/// `sessao` é `None` quando não deu pra saber qual aparelho pediu. Aí a poda
/// alcança só os outros órfãos da mesma conta — nunca um token que tem dono.
/// Sem sessão não há como distinguir dois aparelhos, e apagar o do vizinho por
/// não saber de quem é seria o defeito de volta.
pub async fn emitir_token_de_midia(
    pool: &PgPool,
    user_id: Uuid,
    sessao: Option<Uuid>,
) -> Result<String, AppError> {
    let token = generate_token();
    let mut tx = pool.begin().await?;

    // **`escopo = 'midia'` na primeira metade, e ele é o ponto (R45).** Antes
    // desta linha o `DELETE` levava tudo do usuário, e era isso que apagava a
    // fileira da home da TV toda vez que alguém abria o app no celular: um
    // aparelho pedindo bytes aposentava a credencial que outro tinha publicado
    // no `TvProvider` e não tem como renovar.
    //
    // **`sessao_id IS NOT DISTINCT FROM $2` é a R61.** `IS NOT DISTINCT FROM`
    // e não `=` porque os dois lados podem ser nulos e `NULL = NULL` não casa
    // — órfão só aposenta órfão.
    //
    // A segunda metade continua sem escopo de propósito: varrer o que já venceu
    // é limpeza, e não tem por que poupar arte vencida.
    sqlx::query(
        "DELETE FROM media_token
         WHERE (user_id = $1 AND escopo = 'midia' AND sessao_id IS NOT DISTINCT FROM $2)
            OR expira_em <= now()",
    )
    .bind(user_id)
    .bind(sessao)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO media_token (token_hash, user_id, expira_em, escopo, sessao_id)
         VALUES ($1, $2, $3, 'midia', $4)",
    )
    .bind(hash_token(&token))
    .bind(user_id)
    .bind(Utc::now() + Duration::hours(MEDIA_TOKEN_HOURS))
    .bind(sessao)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(token)
}

/// Emite um token de **arte** pra este aparelho (R45).
///
/// Ao contrário do de mídia, **não aposenta os anteriores** — ver
/// `ARTE_TOKENS_POR_PESSOA`. O que ele faz é podar: passados os cinco mais
/// novos, os velhos saem. Sem a poda, um app que republica a fileira a cada
/// abertura acumularia uma linha por abertura até o vencimento.
pub async fn emitir_token_de_arte(pool: &PgPool, user_id: Uuid) -> Result<String, AppError> {
    let token = generate_token();
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO media_token (token_hash, user_id, expira_em, escopo)
         VALUES ($1, $2, $3, 'arte')",
    )
    .bind(hash_token(&token))
    .bind(user_id)
    .bind(Utc::now() + Duration::days(ARTE_TOKEN_DIAS))
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "DELETE FROM media_token
         WHERE escopo = 'arte'
           AND token_hash IN (
               SELECT token_hash FROM media_token
               WHERE user_id = $1 AND escopo = 'arte'
               ORDER BY criado_em DESC
               OFFSET $2
           )",
    )
    .bind(user_id)
    .bind(ARTE_TOKENS_POR_PESSOA)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(token)
}

/// Resolve um token **de mídia**. Nunca um token de sessão.
///
/// A separação é a fase inteira: um token de sessão na query deixa de
/// funcionar, e é isso que faz o vazamento em log parar de valer uma conta.
pub async fn usuario_por_token_de_midia(pool: &PgPool, token: &str) -> Option<User> {
    // `escopo = 'midia'` explícito: um token de arte dura um ano, e sem esta
    // linha ele abriria `/api/stream/` — que é o filme inteiro (R45).
    sqlx::query_as(
        "SELECT u.id, u.username, u.display_name, u.role, u.is_active,
                u.created_at, u.last_login_at
         FROM media_token m JOIN app_user u ON u.id = m.user_id
         WHERE m.token_hash = $1 AND m.escopo = 'midia'
           AND m.expira_em > now() AND u.is_active",
    )
    .bind(hash_token(token))
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// Resolve um token **de arte** (R45). Só o middleware chama, e só em
/// `/artwork/` — ver `aceita_token_de_arte`.
pub async fn usuario_por_token_de_arte(pool: &PgPool, token: &str) -> Option<User> {
    sqlx::query_as(
        "SELECT u.id, u.username, u.display_name, u.role, u.is_active,
                u.created_at, u.last_login_at
         FROM media_token m JOIN app_user u ON u.id = m.user_id
         WHERE m.token_hash = $1 AND m.escopo = 'arte'
           AND m.expira_em > now() AND u.is_active",
    )
    .bind(hash_token(token))
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// Quanto tempo o token de mídia vale, pro cliente saber quando renovar.
pub fn horas_do_token_de_midia() -> i64 {
    MEDIA_TOKEN_HOURS
}

/// Idem, pro de arte — em dias, que é a unidade em que ele faz sentido.
pub fn dias_do_token_de_arte() -> i64 {
    ARTE_TOKEN_DIAS
}

pub async fn revoke(pool: &PgPool, token: &str) {
    let hash = hash_token(token);

    // Quem é o dono da sessão — precisamos disso ANTES de apagá-la, pra
    // revogar a mídia dele junto. Sair e continuar podendo puxar bytes por oito
    // horas seria um "sair" que não sai.
    let dono: Option<(Uuid,)> =
        sqlx::query_as("SELECT user_id FROM auth_session WHERE token_hash = $1")
            .bind(&hash)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

    let _ = sqlx::query("DELETE FROM auth_session WHERE token_hash = $1")
        .bind(&hash)
        .execute(pool)
        .await;

    if let Some((user_id,)) = dono {
        // **Só a mídia (R45).** "Sair e continuar podendo puxar bytes por oito
        // horas seria um sair que não sai" — e continua sendo, por isso o
        // escopo de mídia morre aqui inteiro.
        //
        // A arte não: deslogar o notebook apagaria a fileira da home da TV, que
        // é exatamente o defeito que a R45 veio consertar, e o mesmo raciocínio
        // do `logout` ("deslogar o notebook não pode derrubar a TV") vale um
        // nível abaixo. Quem quer derrubar a arte tem gesto próprio: trocar a
        // senha, que é o que se faz quando se acha que alguém entrou.
        let _ = sqlx::query("DELETE FROM media_token WHERE user_id = $1 AND escopo = 'midia'")
            .bind(user_id)
            .execute(pool)
            .await;
    }
}

/// Existe alguém com senha definida? Se não, é primeira execução e a rota de
/// setup fica aberta. É o único momento em que se cria admin sem autenticação.
pub async fn needs_setup(pool: &PgPool) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM app_user WHERE password_hash IS NOT NULL")
        .fetch_one(pool)
        .await
        .map(|count| count == 0)
        .unwrap_or(false)
}

// -------------------------------------------------------------- extratores

/// Injeta o usuário autenticado no handler. O middleware já validou e guardou
/// o `User` nas extensões — aqui é só tirar de lá.
#[derive(Debug, Clone)]
pub struct AuthUser(pub User);

impl AuthUser {
    pub fn id(&self) -> Uuid {
        self.0.id
    }
}

impl<S: Send + Sync> FromRequestParts<S> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<User>()
            .cloned()
            .map(AuthUser)
            .ok_or(AppError::Unauthorized)
    }
}

/// Mesma coisa, mas recusa quem não é admin. Varrer disco, identificar,
/// transcodificar e gerir usuários são operações de dono.
#[derive(Debug, Clone)]
pub struct AdminUser(pub User);

impl<S: Send + Sync> FromRequestParts<S> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let user = parts
            .extensions
            .get::<User>()
            .cloned()
            .ok_or(AppError::Unauthorized)?;
        if !user.is_admin() {
            return Err(AppError::Forbidden(
                "esta operação é do administrador".into(),
            ));
        }
        Ok(AdminUser(user))
    }
}

/// Conveniência pra handlers que só querem o pool e o id.
pub async fn load_user(state: &AppState, id: Uuid) -> Option<User> {
    sqlx::query_as(
        "SELECT id, username, display_name, role, is_active, created_at, last_login_at
         FROM app_user WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn senha_confere_com_o_proprio_hash() {
        let hash = hash_password("uma senha boa").unwrap();
        assert!(verify_password("uma senha boa", &hash));
        assert!(!verify_password("uma senha ruim", &hash));
    }

    #[test]
    fn hash_e_salgado_por_padrao() {
        // Dois hashes da MESMA senha precisam diferir, senão um vazamento
        // revelaria quem usa a mesma senha que quem.
        let a = hash_password("mesma senha").unwrap();
        let b = hash_password("mesma senha").unwrap();
        assert_ne!(a, b);
        assert!(verify_password("mesma senha", &a));
        assert!(verify_password("mesma senha", &b));
    }

    #[test]
    fn usa_argon2id() {
        let hash = hash_password("qualquer senha").unwrap();
        assert!(hash.starts_with("$argon2id$"), "veio {hash}");
    }

    #[test]
    fn senha_curta_e_recusada() {
        assert!(hash_password("curta").is_err());
        assert!(hash_password("12345678").is_ok());
    }

    #[test]
    fn hash_corrompido_nao_autentica_ninguem() {
        assert!(!verify_password("qualquer", "não é um hash"));
        assert!(!verify_password("", ""));
    }

    #[test]
    fn token_e_unico_e_hexadecimal() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 64); // 32 bytes em hex
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_do_token_e_estavel_e_diferente_do_token() {
        let token = generate_token();
        assert_eq!(hash_token(&token), hash_token(&token));
        assert_ne!(hash_token(&token), token);
        assert_eq!(hash_token(&token).len(), 64);
    }
}
