//! R46 — o pareamento: entrar na TV pelo celular.
//!
//! O celular já logado pede um código curto; a TV o troca por sessão. O
//! raciocínio inteiro — por que o código pode ser curto aqui e não podia no
//! convite — está na migração `0040_pareamento.sql`, e não se repete aqui.
//!
//! O que este módulo guarda de comentário é o que só se vê no código: o
//! alfabeto, a normalização, e o fato de o resgate ser uma consulta só.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::AppState;

/// Cinco minutos. O código nasce pra atravessar a sala, não pra ser guardado.
const MINUTOS: i64 = 5;

/// Oito caracteres de cinco bits — 40 bits.
///
/// O tamanho é uma conta de D-pad, e não de criptografia: oito apertos de
/// direcional contra os oitenta de uma senha de doze caracteres com maiúscula e
/// símbolo. O que compra a segurança são os cinco minutos, o uso único e o
/// código único por pessoa (ver a migração).
const TAMANHO: usize = 8;

/// Crockford base32: 32 símbolos, sem `I`, `L`, `O` e `U`.
///
/// Os três primeiros somem porque um deles vira o outro na tela de uma TV a
/// três metros — `I` e `1`, `O` e `0`. O `U` sai pelo motivo de sempre no
/// Crockford: sem ele, nenhuma palavra ofensiva se forma por acidente.
///
/// **32 divide 256**, e é isso que torna `byte % 32` uma escolha uniforme. Um
/// alfabeto de 31 ou 33 símbolos enviesaria os primeiros — de pouco, mas de
/// graça, e sem que nenhum teste notasse.
const ALFABETO: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Debug, Deserialize)]
pub struct Resgate {
    pub codigo: String,
    /// Como esta TV vai aparecer na lista de sessões. Mesmo campo do login.
    #[serde(default)]
    pub device_label: Option<String>,
}

/// O celular pede. Autenticada como qualquer rota de API: é a sessão do celular
/// que está sendo emprestada pra TV, então ela tem que existir.
pub async fn criar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Value>> {
    let codigo = codigo_novo();

    let mut tx = state.pool.begin().await?;

    // Um código vivo por pessoa. Pedir outro é ter perdido o primeiro de vista
    // — e deixar os dois valendo dobraria o alvo sem servir a ninguém.
    sqlx::query("DELETE FROM pareamento WHERE user_id = $1 AND usado_em IS NULL")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "INSERT INTO pareamento (codigo_hash, user_id, expira_em)
         VALUES ($1, $2, now() + ($3 || ' minutes')::interval)",
    )
    .bind(hash(&codigo))
    .bind(user.id)
    .bind(MINUTOS.to_string())
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(json!({
        "codigo": codigo,
        "minutos": MINUTOS,
    })))
}

/// A TV troca. **Pública** — é o mesmo motivo de `/api/convites/resgatar`: a
/// troca acontece antes de haver sessão deste lado, e o código é a credencial.
pub async fn resgatar(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Resgate>,
) -> AppResult<Response> {
    // **Queimar e resolver na mesma consulta.** Ler primeiro e marcar depois
    // deixaria a janela em que duas TVs entram com o mesmo código; o `UPDATE …
    // WHERE usado_em IS NULL … RETURNING` fecha isso no banco, que é onde a
    // locadora também deixou a corrida (ver o índice único da 0029).
    let dono: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE pareamento
            SET usado_em = now()
          WHERE codigo_hash = $1 AND usado_em IS NULL AND expira_em > now()
      RETURNING user_id",
    )
    .bind(hash(&body.codigo))
    .fetch_optional(&state.pool)
    .await?;

    // Mensagem única pros três casos (não existe / já usado / vencido), como no
    // login: distinguir contaria a quem chuta se o chute passou perto.
    let (user_id,) = dono.ok_or(AppError::Unauthorized)?;

    super::auth::issue(&state, user_id, body.device_label.as_deref(), &headers).await
}

/// Maiúsculas e sem separador — a TV pode ter recebido `abcd-efgh` de um
/// teclado que insiste em minúsculas, e o hífen é enfeite de leitura.
fn normalizar(codigo: &str) -> String {
    codigo
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn hash(codigo: &str) -> String {
    let digest = Sha256::digest(normalizar(codigo).as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn codigo_novo() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; TAMANHO];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| ALFABETO[(*b as usize) % ALFABETO.len()] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 40 bits. Menos que os 128 do convite, e a migração diz por quê — mas o
    /// número não pode escorregar sem que alguém veja: com 5 minutos e uso
    /// único, é ele que segura a conta de força bruta.
    #[test]
    fn o_codigo_tem_quarenta_bits() {
        assert_eq!(ALFABETO.len(), 32);
        assert_eq!(TAMANHO * 5, 40);
        let c = codigo_novo();
        assert_eq!(c.len(), TAMANHO);
    }

    /// O alfabeto tem que dividir 256, senão `% 32` favorece os primeiros
    /// símbolos e a entropia real fica abaixo dos 40 bits declarados.
    #[test]
    fn o_alfabeto_divide_o_byte() {
        assert_eq!(256 % ALFABETO.len(), 0);
    }

    /// Os pares que a TV confunde a três metros não podem estar os dois na
    /// mesa. Um `I` lido como `1` é um código digitado errado com o D-pad
    /// inteiro pago.
    #[test]
    fn nao_ha_simbolo_ambiguo() {
        for ambiguo in [b'I', b'L', b'O', b'U'] {
            assert!(
                !ALFABETO.contains(&ambiguo),
                "{} voltou pro alfabeto",
                ambiguo as char
            );
        }
    }

    #[test]
    fn o_codigo_so_usa_o_alfabeto() {
        for _ in 0..64 {
            assert!(codigo_novo().bytes().all(|b| ALFABETO.contains(&b)));
        }
    }

    /// Dois códigos nunca são iguais — um gerador que repetisse transformaria
    /// um pareamento queimado em chave de outra pessoa.
    #[test]
    fn dois_codigos_nao_colidem() {
        let a = codigo_novo();
        let b = codigo_novo();
        assert_ne!(a, b);
    }

    /// O que a TV digita e o que o celular mostrou têm que bater mesmo quando o
    /// teclado da TV resolve mandar minúsculas ou o usuário digita o hífen que
    /// viu na tela.
    #[test]
    fn a_normalizacao_aceita_o_que_a_tv_manda() {
        let canonico = hash("ABCD2345");
        assert_eq!(hash("abcd2345"), canonico);
        assert_eq!(hash("ABCD-2345"), canonico);
        assert_eq!(hash(" abcd 2345 "), canonico);
        // e não junta códigos diferentes
        assert_ne!(hash("ABCD2346"), canonico);
    }

    /// O código não vai pro disco — só o SHA-256 dele, como `auth_session`,
    /// `convite` e `media_token`.
    #[test]
    fn o_disco_guarda_so_o_hash() {
        let c = codigo_novo();
        let h = hash(&c);
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert!(!h.contains(&c));
    }
}
