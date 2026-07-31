//! Política de origem cruzada.
//!
//! O ponto de partida era `allow_origin(Any)`. Isso nunca foi um buraco de CSRF
//! — a autenticação é Bearer, e o próprio CORS proíbe credenciais junto de `*`
//! — mas é frouxo, e impedia o cookie de sessão funcionar de verdade.
//!
//! **A dificuldade:** o servidor é alcançado por nomes que ele não conhece de
//! antemão (`rog`, `odeon.tailnet.ts.net`, um IP). Uma lista fixa de origens
//! quebraria o acesso em silêncio no dia em que o nome mudasse.
//!
//! **A regra:** aceita a origem cujo *host* é o mesmo host pelo qual a
//! requisição chegou, ignorando a porta. Ou seja, o front em `http://rog:5174`
//! falando com a API em `http://rog:8080` é aceito automaticamente, e
//! `http://evil.com` não é — sem configurar nada.
//!
//! Mais: localhost sempre (dev), e o que estiver em `ODEON_ALLOWED_ORIGINS`.

use std::time::Duration;

use axum::http::header::{HeaderValue, HOST};
use axum::http::request::Parts;
use axum::http::{header, Method};
use tower_http::cors::{AllowOrigin, CorsLayer};

/// Uma hora de cache de preflight. Sem isso, cada requisição com header
/// customizado dispara um OPTIONS antes.
const PREFLIGHT_MAX_AGE: Duration = Duration::from_secs(3600);

/// Tira o esquema e a porta: `http://rog:5174` → `rog`.
fn host_of(value: &str) -> Option<&str> {
    let without_scheme = value
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(value);

    let authority = without_scheme.split('/').next()?;

    // IPv6 literal vem entre colchetes (`[::1]:5174`); a porta fica depois.
    let host = match authority.strip_prefix('[') {
        Some(rest) => rest.split(']').next()?,
        None => authority.split(':').next()?,
    };

    (!host.is_empty()).then_some(host)
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0")
}

/// A decisão, isolada da plumbing do tower pra poder ser testada.
pub fn origin_allowed(origin: &str, request_host: Option<&str>, allowlist: &[String]) -> bool {
    let Some(origin_host) = host_of(origin) else {
        return false;
    };

    // 1. Origem declarada explicitamente.
    if allowlist.iter().any(|entry| {
        entry.eq_ignore_ascii_case(origin)
            || host_of(entry).is_some_and(|h| h.eq_ignore_ascii_case(origin_host))
    }) {
        return true;
    }

    // 2. Dev: o front do Vite roda em localhost noutra porta.
    if is_loopback(origin_host) {
        return true;
    }

    // 3. Mesmo host da requisição, porta diferente. É o caso real: web em
    //    :5174 e API em :8080, ambas no mesmo nome da tailnet.
    request_host
        .and_then(host_of)
        .is_some_and(|host| host.eq_ignore_ascii_case(origin_host))
}

pub fn layer(allowlist: Vec<String>, allow_any: bool) -> CorsLayer {
    let origin = if allow_any {
        tracing::warn!(
            "ODEON_ALLOWED_ORIGINS=* — qualquer origem é aceita. Só faça isso \
             se souber exatamente por quê."
        );
        AllowOrigin::any()
    } else {
        AllowOrigin::predicate(move |origin: &HeaderValue, parts: &Parts| {
            let Ok(origin) = origin.to_str() else {
                return false;
            };
            let request_host = parts.headers.get(HOST).and_then(|h| h.to_str().ok());
            let allowed = origin_allowed(origin, request_host, &allowlist);
            if !allowed {
                tracing::debug!(origin, "origem recusada pelo CORS");
            }
            allowed
        })
    };

    let cors = CorsLayer::new()
        .allow_origin(origin)
        // Só os métodos que a API usa de fato.
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        // Antes era `Any`. Estes dois são os únicos que o cliente manda.
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::RANGE])
        // `Content-Range` e `Accept-Ranges` não estão na lista segura do CORS;
        // sem expor, um player que use `fetch` não enxerga o tamanho do vídeo.
        .expose_headers([header::CONTENT_RANGE, header::ACCEPT_RANGES])
        .max_age(PREFLIGHT_MAX_AGE);

    // Credencial só faz sentido com origem específica — o CORS proíbe
    // `Access-Control-Allow-Origin: *` junto de `allow_credentials`, e o
    // browser recusaria a resposta inteira.
    if allow_any {
        cors
    } else {
        cors.allow_credentials(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowlist() -> Vec<String> {
        vec!["https://odeon.exemplo.com".to_string()]
    }

    #[test]
    fn extrai_host_de_varias_formas() {
        assert_eq!(host_of("http://rog:5174"), Some("rog"));
        assert_eq!(host_of("https://odeon.exemplo.com"), Some("odeon.exemplo.com"));
        assert_eq!(host_of("rog:8080"), Some("rog"));
        assert_eq!(host_of("http://[::1]:5174"), Some("::1"));
        assert_eq!(host_of(""), None);
    }

    #[test]
    fn mesma_maquina_em_porta_diferente_e_aceita() {
        // o caso real: web em :5174 e API em :8080, mesmo nome da tailnet
        assert!(origin_allowed("http://rog:5174", Some("rog:8080"), &[]));
        assert!(origin_allowed(
            "http://odeon.tail1234.ts.net:5174",
            Some("odeon.tail1234.ts.net:8080"),
            &[],
        ));
    }

    #[test]
    fn site_de_terceiro_e_recusado() {
        assert!(!origin_allowed("http://evil.com", Some("rog:8080"), &allowlist()));
        assert!(!origin_allowed("https://evil.com", Some("rog:8080"), &[]));
    }

    #[test]
    fn nome_parecido_nao_passa() {
        // "rog.evil.com" não é "rog" — comparação é do host inteiro
        assert!(!origin_allowed("http://rog.evil.com", Some("rog:8080"), &[]));
        assert!(!origin_allowed("http://notrog", Some("rog:8080"), &[]));
    }

    #[test]
    fn localhost_sempre_passa_em_dev() {
        assert!(origin_allowed("http://localhost:5174", Some("localhost:8080"), &[]));
        // e mesmo quando a API é acessada por outro nome
        assert!(origin_allowed("http://127.0.0.1:5174", Some("rog:8080"), &[]));
    }

    #[test]
    fn allowlist_explicita_funciona() {
        assert!(origin_allowed(
            "https://odeon.exemplo.com",
            Some("rog:8080"),
            &allowlist(),
        ));
    }

    #[test]
    fn sem_host_na_requisicao_so_vale_allowlist_e_loopback() {
        assert!(!origin_allowed("http://rog:5174", None, &[]));
        assert!(origin_allowed("http://localhost:5174", None, &[]));
        assert!(origin_allowed("https://odeon.exemplo.com", None, &allowlist()));
    }

    #[test]
    fn origem_lixo_nao_passa() {
        assert!(!origin_allowed("null", Some("rog:8080"), &[]));
        assert!(!origin_allowed("", Some("rog:8080"), &[]));
    }
}
