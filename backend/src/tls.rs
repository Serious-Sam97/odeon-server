//! TLS.
//!
//! **De onde vem o certificado.** Auto-assinado é a saída errada aqui: cada
//! aparelho precisaria confiar numa CA nova, e instalar CA na Android TV é um
//! suplício. A Tailscale emite certificado **Let's Encrypt de verdade** pra
//! nomes `*.ts.net`:
//!
//! ```sh
//! tailscale cert odeon.SEU-TAILNET.ts.net
//! ```
//!
//! Sai um par `.crt`/`.key` que todo navegador e todo app já confiam, sem
//! instalar nada em lugar nenhum. É o caminho documentado no README.
//!
//! **Por que TLS aqui e não num proxy na frente.** Um Caddy/nginx resolveria,
//! mas seria mais um container pra um servidor de uma pessoa só. O axum já
//! sabe fazer isso, e a renovação continua sendo `tailscale cert` num cron.

use std::net::SocketAddr;
use std::path::Path;

use axum::response::{IntoResponse, Redirect, Response};
use axum::Router;
use axum_server::tls_openssl::OpenSSLConfig;

/// Um ano. Abaixo disso o HSTS quase não protege; e como o certificado é
/// renovável pela Tailscale, não há risco de ficar preso sem TLS.
pub const HSTS_VALUE: &str = "max-age=31536000";

/// Sobe o listener HTTPS. Devolve erro se o par de arquivos não servir — falhar
/// alto é melhor que subir em HTTP achando que está protegido.
pub async fn serve(
    bind: &str,
    cert: &Path,
    key: &Path,
    app: Router,
) -> anyhow::Result<()> {
    let config = OpenSSLConfig::from_pem_file(cert, key).map_err(|e| {
        anyhow::anyhow!(
            "certificado inválido ({} / {}): {e}",
            cert.display(),
            key.display()
        )
    })?;

    let addr: SocketAddr = bind.parse()?;
    tracing::info!(%addr, "Odeon ouvindo em HTTPS");

    axum_server::bind_openssl(addr, config)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

/// Roteador mínimo que só manda todo mundo pro HTTPS.
///
/// Fica na porta HTTP quando `ODEON_HTTPS_ONLY=true`. Não serve conteúdo: quem
/// chegar em texto puro é redirecionado antes de qualquer coisa sensível
/// trafegar — inclusive antes do `Authorization`.
pub fn redirect_router(https_port: u16) -> Router {
    Router::new().fallback(move |request: axum::extract::Request| async move {
        redirect_to_https(request, https_port)
    })
}

fn redirect_to_https(request: axum::extract::Request, https_port: u16) -> Response {
    let host = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        // Descarta a porta do HTTP: o destino é outra.
        .map(|value| value.split(':').next().unwrap_or(value).to_string());

    let Some(host) = host else {
        // Sem Host não dá pra montar destino. 400 é mais honesto que adivinhar.
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "sem cabeçalho Host — não sei pra onde redirecionar",
        )
            .into_response();
    };

    let path = request
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");

    // 308 e não 302: preserva o método e o corpo. Um POST redirecionado com
    // 302 viraria GET e o login sumiria no caminho.
    Redirect::permanent(&format!("https://{host}:{https_port}{path}")).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};

    fn request(host: Option<&str>, uri: &str) -> axum::extract::Request {
        let mut builder = Request::builder().uri(uri).method("POST");
        if let Some(host) = host {
            builder = builder.header(axum::http::header::HOST, host);
        }
        builder.body(Body::empty()).unwrap()
    }

    fn location(response: &Response) -> Option<&str> {
        response
            .headers()
            .get(axum::http::header::LOCATION)?
            .to_str()
            .ok()
    }

    #[test]
    fn redireciona_preservando_caminho_e_query() {
        let response = redirect_to_https(request(Some("rog:8080"), "/api/works?limit=5"), 8443);
        assert_eq!(
            location(&response),
            Some("https://rog:8443/api/works?limit=5")
        );
    }

    #[test]
    fn usa_308_pra_nao_transformar_post_em_get() {
        let response = redirect_to_https(request(Some("rog:8080"), "/api/auth/login"), 8443);
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    }

    #[test]
    fn troca_a_porta_do_http_pela_do_https() {
        let response = redirect_to_https(request(Some("odeon.ts.net:8080"), "/"), 443);
        assert_eq!(location(&response), Some("https://odeon.ts.net:443/"));
    }

    #[test]
    fn sem_host_recusa_em_vez_de_adivinhar() {
        let response = redirect_to_https(request(None, "/"), 8443);
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
