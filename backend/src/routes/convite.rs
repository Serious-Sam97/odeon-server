//! R26 — o convite.
//!
//! Como alguém de fora ganha conta no servidor, e por que não é "crie uma conta
//! e me mande o usuário".
//!
//! ## O convite é do servidor, e não tem nada a ver com amizade (R28)
//!
//! Ele apontava pra um **círculo**, porque era o único jeito de dizer *onde*
//! alguém entrava. Sem grupo, a resposta é a única que sempre fez sentido: o
//! servidor.
//!
//! E ele não cria vínculo social nenhum. **Amizade é entre duas contas que já
//! existem** (`routes/amigos.rs`) — resgatar um convite te dá uma conta, não um
//! amigo, e quem entrou pede amizade a quem quiser como qualquer um. Amarrar as
//! duas coisas faria "aceitei seu convite" significar "aceitei te mostrar o que
//! estou assistindo", que são consentimentos diferentes.
//!
//! ## O convite é um segredo curto, e some
//!
//! O código não é guardado — guardamos o SHA-256, exatamente como
//! `auth_session` faz desde o §9b. Vazar o banco não dá convite a ninguém.
//!
//! E ele **vence em sete dias**, o mesmo prazo da fita (§35). A coincidência é
//! de propósito: as duas coisas são empréstimos de acesso, e um convite eterno é
//! uma senha permanente esquecida num aplicativo de mensagem.
//!
//! ## Quem entra é `guest`, e isso não é hierarquia
//!
//! Um convidado não é um morador com menos botões: é alguém cujo disco aquele
//! **não é**. A diferença tem uma consequência só, e ela é a R26 inteira: ele
//! assiste o que pegou emprestado, e mais nada (`auth/acesso.rs`).

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth;
use crate::error::{AppError, AppResult};
use crate::AppState;

/// Quanto dura um convite não usado. Mesmo prazo da fita (§35).
const DIAS: i64 = 7;

/// Tamanho do código, em bytes de entropia.
///
/// 16 bytes = 128 bits, que é o mesmo teto prático do token de sessão do §9b:
/// não é adivinhável por força bruta, e cabe numa mensagem sem parecer um
/// despejo de banco.
const BYTES: usize = 16;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ConviteNaLista {
    pub para: Option<String>,
    pub criado_em: chrono::DateTime<chrono::Utc>,
    pub expira_em: chrono::DateTime<chrono::Utc>,
    pub usado_em: Option<chrono::DateTime<chrono::Utc>>,
    pub usado_por_nome: Option<String>,
    /// Vencido e não usado. A lista mostra, em vez de sumir com ele: quem
    /// convidou precisa saber que o convite morreu sem ser usado.
    pub vencido: bool,
}

#[derive(Debug, Deserialize)]
pub struct NovoConvite {
    pub para: Option<String>,
}

/// Cria um convite. Só quem administra — convidar alguém pro seu servidor é
/// decisão de dono, não de morador.
pub async fn criar(
    State(state): State<AppState>,
    auth::AdminUser(user): auth::AdminUser,
    Json(corpo): Json<NovoConvite>,
) -> AppResult<Json<Value>> {
    // O código só existe nesta resposta. Se quem convidou perder, emite outro —
    // e é por isso que ele não pode ser reexibido depois.
    let codigo = codigo_novo();

    sqlx::query(
        "INSERT INTO convite (codigo_hash, criado_por, para, expira_em)
         VALUES ($1, $2, $3, now() + ($4 || ' days')::interval)",
    )
    .bind(auth::hash_token(&codigo))
    .bind(user.id)
    .bind(corpo.para.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(DIAS.to_string())
    .execute(&state.pool)
    .await?;

    Ok(Json(json!({
        "codigo": codigo,
        "expira_em_dias": DIAS,
        // Dito na resposta porque a tela precisa avisar **antes** de quem
        // convidou fechar a janela.
        "aviso": "este código aparece uma vez só",
    })))
}

/// Os convites do servidor. Sem os códigos — eles não existem mais.
///
/// Não há filtro por quem emitiu: a rota já é de administrador, e um servidor de
/// uma casa em que um dono não vê o convite que o outro mandou é um servidor que
/// esconde de si mesmo quem tem a chave.
pub async fn listar(
    State(state): State<AppState>,
    auth::AdminUser(_): auth::AdminUser,
) -> AppResult<Json<Vec<ConviteNaLista>>> {
    let lista = sqlx::query_as::<_, ConviteNaLista>(
        "SELECT c.para, c.criado_em, c.expira_em, c.usado_em,
                u.display_name AS usado_por_nome,
                (c.usado_em IS NULL AND c.expira_em <= now()) AS vencido
         FROM convite c
         LEFT JOIN app_user u ON u.id = c.usado_por
         ORDER BY c.criado_em DESC
         LIMIT 50",
    )
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(lista))
}

/// Revoga um convite que ainda não foi usado.
///
/// Pelo `para`, e não por um id: o código não está guardado e o rótulo é a
/// única coisa que quem convidou reconhece. Ambíguo por construção — se houver
/// dois "pro Rudney", some o mais antigo, que é o que alguém quis dizer com
/// "cancela aquele convite".
pub async fn revogar(
    State(state): State<AppState>,
    auth::AdminUser(_): auth::AdminUser,
    Path(para): Path<String>,
) -> AppResult<Json<Value>> {
    let n = sqlx::query(
        "DELETE FROM convite c
         WHERE c.usado_em IS NULL
           AND c.codigo_hash = (
               SELECT c2.codigo_hash FROM convite c2
               WHERE c2.usado_em IS NULL AND COALESCE(c2.para, '') = $1
               ORDER BY c2.criado_em LIMIT 1
           )",
    )
    .bind(&para)
    .execute(&state.pool)
    .await?
    .rows_affected();

    Ok(Json(json!({ "revogado": n > 0 })))
}

#[derive(Debug, Deserialize)]
pub struct Resgate {
    pub codigo: String,
    pub username: String,
    pub display_name: Option<String>,
    pub password: String,
}

/// Trocar um código por uma conta de convidado.
///
/// **Rota pública**, como o login — quem resgata ainda não tem sessão. É o
/// código que autentica, e é por isso que ele tem 128 bits e vence.
pub async fn resgatar(
    State(state): State<AppState>,
    Json(r): Json<Resgate>,
) -> AppResult<Json<Value>> {
    let username = r.username.trim().to_lowercase();
    if username.len() < 2 {
        return Err(AppError::BadRequest("nome de usuário curto demais".into()));
    }

    // Uma transação: entre validar o convite e criar a conta não pode caber um
    // segundo resgate do mesmo código.
    let mut tx = state.pool.begin().await?;

    // `FOR UPDATE` trava a linha do convite; dois resgates simultâneos do mesmo
    // código serializam, e o segundo encontra `usado_em` preenchido.
    let convite: Option<(String,)> = sqlx::query_as(
        "SELECT codigo_hash FROM convite
         WHERE codigo_hash = $1 AND usado_em IS NULL AND expira_em > now()
         FOR UPDATE",
    )
    .bind(auth::hash_token(&r.codigo))
    .fetch_optional(&mut *tx)
    .await?;

    // A mesma frase pra código errado, vencido e já usado. Distinguir diria a
    // quem tenta se o código existe — que é a diferença entre um "não" e um
    // oráculo.
    convite.ok_or_else(|| AppError::Forbidden("convite inválido, vencido ou já usado".into()))?;

    let hash = auth::hash_password(&r.password)?;

    let novo: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO app_user (username, display_name, password_hash, role)
         VALUES ($1, $2, $3, 'guest')
         ON CONFLICT (username) DO NOTHING
         RETURNING id",
    )
    .bind(&username)
    .bind(r.display_name.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&username))
    .bind(&hash)
    .fetch_optional(&mut *tx)
    .await?;

    let (user_id,) = novo.ok_or_else(|| AppError::BadRequest("este nome já existe".into()))?;

    // Nenhum vínculo é criado aqui. Até a R28 esta linha inseria o convidado num
    // círculo — hoje entrar no servidor **é** entrar na locadora, e amizade é
    // pedida e aceita depois, por quem quiser.
    sqlx::query("UPDATE convite SET usado_em = now(), usado_por = $2 WHERE codigo_hash = $1")
        .bind(auth::hash_token(&r.codigo))
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    tracing::info!(%username, "convidado entrou por convite");
    Ok(Json(json!({ "ok": true, "username": username })))
}

/// Um código legível o suficiente pra ser ditado, e aleatório o suficiente pra
/// não ser adivinhado.
///
/// Hex e não base64: base64 tem `+`, `/` e maiúsculas misturadas, e um código
/// que alguém vai copiar de uma mensagem não deve depender de acertar
/// maiúscula. 32 caracteres hex são os mesmos 128 bits.
fn codigo_novo() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 128 bits, o mesmo teto do token de sessão do §9b. Um código curto seria
    /// adivinhável, e este código **é** a autenticação de quem resgata.
    #[test]
    fn o_codigo_tem_entropia_de_sessao() {
        assert_eq!(BYTES * 8, 128);
        let c = codigo_novo();
        assert_eq!(c.len(), BYTES * 2);
        assert!(c.chars().all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()));
    }

    /// Dois convites nunca são iguais. Um gerador que repetisse transformaria
    /// um convite usado em chave de outro.
    #[test]
    fn dois_codigos_nao_colidem() {
        let a = codigo_novo();
        let b = codigo_novo();
        assert_ne!(a, b);
    }

    /// O prazo do convite é o da fita (§35), e a coincidência é intencional:
    /// as duas coisas são empréstimos de acesso.
    #[test]
    fn o_convite_vence_no_prazo_da_fita() {
        assert_eq!(DIAS, 7);
    }
}
