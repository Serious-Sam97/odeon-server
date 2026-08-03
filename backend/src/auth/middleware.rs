//! O portão.
//!
//! **O problema que domina o desenho:** `<video src>`, `<img src>` e `<track>`
//! não mandam header `Authorization`. E cookie cross-origin exige
//! `SameSite=None; Secure`, ou seja, HTTPS — que não existe num servidor HTTP
//! na tailnet.
//!
//! A saída é aceitar o token por três caminhos, com escopos diferentes:
//!
//! 1. `Authorization: Bearer` — API, e é o que os clientes Kotlin usam.
//! 2. cookie `odeon_session` — funciona quando web e API são a mesma origem.
//! 3. `?token=` na query — **só nas rotas de mídia**, exatamente porque são as
//!    que um elemento HTML busca sozinho.
//!
//! O nº 3 é um compromisso consciente: token em query string vaza pra log de
//! acesso e histórico do navegador. Restringi-lo às rotas de mídia limita o
//! estrago, e num servidor de uma pessoa só na tailnet o risco é aceitável. Se
//! um dia isso for exposto de verdade, o certo é emitir um token de mídia
//! curto e separado do token de sessão.

use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::Response;

use crate::auth;
use crate::error::AppError;
use crate::AppState;

pub const COOKIE_NAME: &str = "odeon_session";

/// Rotas que precisam responder sem sessão — senão não há como criar a primeira.
fn is_public(path: &str) -> bool {
    matches!(
        path,
        "/api/health"
            | "/api/auth/status"
            | "/api/auth/login"
            | "/api/auth/setup"
            // R26: trocar um convite por conta acontece antes de haver sessão.
            // O código é a credencial, e ele tem 128 bits e vence em 7 dias.
            | "/api/convites/resgatar"
    )
}

/// Rotas buscadas diretamente por elementos HTML, que não mandam header.
fn accepts_query_token(path: &str) -> bool {
    path.starts_with("/api/stream/")
        || path.starts_with("/api/hls/")
        || path.starts_with("/artwork/")
        || path.starts_with("/scrub/")
        || (path.starts_with("/api/media/") && path.contains("/subtitles"))
}

fn bearer(request: &Request) -> Option<String> {
    request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_string)
}

fn cookie(request: &Request) -> Option<String> {
    request
        .headers()
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, value)| value.to_string())
}

fn query_token(request: &Request) -> Option<String> {
    let query = request.uri().query()?;
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == "token")
        .map(|(_, value)| urldecode(value))
}

fn urldecode(value: &str) -> String {
    // Token é hexadecimal, então na prática não há o que decodificar — mas
    // aceitar `%` mal formado sem quebrar evita 500 por URL torta.
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            match u8::from_str_radix(&hex, 16) {
                Ok(byte) => out.push(byte as char),
                Err(_) => out.push('%'),
            }
        } else if c == '+' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

pub async fn require_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let path = request.uri().path().to_string();

    if is_public(&path) {
        return Ok(next.run(request).await);
    }

    // Enquanto não há nenhuma senha definida o servidor está em primeira
    // execução; qualquer rota responderia lixo. Melhor dizer o que fazer.
    if auth::needs_setup(&state.pool).await {
        return Err(AppError::Unauthorized);
    }

    // **Header e cookie resolvem sessão; a query resolve mídia.** (R27, §43)
    //
    // Até aqui os três caminhos caíam no mesmo `user_for_token`, e o que ia na
    // query era o token de sessão — 90 dias, acesso total à API. Um
    // `access.log` de proxy, um histórico de navegador ou um print com a URL
    // do vídeo entregava uma conta inteira.
    //
    // Agora são duas tabelas e dois escopos. Um token de mídia vazado abre
    // mídia, e por oito horas; ele não lista biblioteca, não aluga e não
    // administra nada. E um token de sessão **deixa de funcionar na query** —
    // que é o que faz a separação valer alguma coisa em vez de ser cerimônia.
    let user = match bearer(&request).or_else(|| cookie(&request)) {
        Some(token) => auth::user_for_token(&state.pool, &token).await,
        None => match accepts_query_token(&path).then(|| query_token(&request)).flatten() {
            Some(token) => auth::usuario_por_token_de_midia(&state.pool, &token).await,
            None => None,
        },
    };

    let Some(user) = user else {
        return Err(AppError::Unauthorized);
    };

    // O handler pega daqui pelo extractor AuthUser/AdminUser.
    request.extensions_mut().insert(user);
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotas_de_boot_sao_publicas() {
        assert!(is_public("/api/auth/login"));
        assert!(is_public("/api/auth/setup"));
        assert!(is_public("/api/health"));
        // e nada além disso
        assert!(!is_public("/api/works"));
        assert!(!is_public("/api/auth/users"));
        assert!(!is_public("/api/stream/abc"));
    }

    /// **A invariante da R27** (§43): a query resolve mídia, e só mídia.
    ///
    /// O middleware escolhe a tabela pelo caminho por onde o token chegou —
    /// header/cookie vão pra `auth_session`, query vai pra `media_token`. Se
    /// alguém um dia reunir os três num `or_else` de novo, o token de sessão
    /// volta a valer na URL e a fase inteira desaparece sem nenhum teste
    /// quebrar. Este aqui quebra.
    #[test]
    fn a_query_nao_resolve_sessao() {
        let fonte = include_str!("middleware.rs");
        let corpo = fonte
            .split("pub async fn require_auth")
            .nth(1)
            .expect("require_auth sumiu");

        let i_query = corpo.find("query_token(&request)").expect("query_token sumiu");
        let i_midia = corpo
            .find("usuario_por_token_de_midia")
            .expect("a query deixou de resolver token de mídia");
        let i_sessao = corpo.find("user_for_token").expect("user_for_token sumiu");

        // `user_for_token` (sessão) tem que aparecer ANTES do ramo da query —
        // ou seja, no ramo do header/cookie. Se ele aparecer depois, a query
        // está caindo na sessão de novo.
        assert!(
            i_sessao < i_query,
            "o token de sessão voltou a ser resolvido pelo ramo da query"
        );
        assert!(
            i_midia > i_query,
            "a query deixou de resolver pelo `media_token`"
        );
    }

    #[test]
    fn token_na_query_so_vale_pra_midia() {
        assert!(accepts_query_token("/api/stream/abc"));
        assert!(accepts_query_token("/artwork/x-poster.jpg"));
        assert!(accepts_query_token("/scrub/x.jpg"));
        assert!(accepts_query_token("/api/hls/abc/index.m3u8"));
        assert!(accepts_query_token("/api/media/abc/subtitles/0"));

        // rotas de dados NÃO aceitam — token em query vaza pra log
        assert!(!accepts_query_token("/api/works"));
        assert!(!accepts_query_token("/api/auth/users"));
        assert!(!accepts_query_token("/api/curation/for-you"));
    }

    #[test]
    fn cookie_e_lido_no_meio_de_outros() {
        let request = Request::builder()
            .header(header::COOKIE, "outro=1; odeon_session=abc123; mais=2")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(cookie(&request).as_deref(), Some("abc123"));
    }

    #[test]
    fn cookie_ausente_nao_explode() {
        let request = Request::builder()
            .header(header::COOKIE, "outro=1")
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(cookie(&request).is_none());
    }

    #[test]
    fn bearer_exige_o_prefixo() {
        let good = Request::builder()
            .header(header::AUTHORIZATION, "Bearer abc")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(bearer(&good).as_deref(), Some("abc"));

        let basic = Request::builder()
            .header(header::AUTHORIZATION, "Basic abc")
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(bearer(&basic).is_none());
    }

    #[test]
    fn token_da_query_ignora_outros_parametros() {
        let request = Request::builder()
            .uri("/api/stream/x?start=10&token=deadbeef&other=1")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(query_token(&request).as_deref(), Some("deadbeef"));
    }
}
