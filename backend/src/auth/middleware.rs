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
            // R46: idem pra TV trocar o código do celular por sessão. Aqui o
            // código tem 40 bits, e o que compensa são os cinco minutos, o uso
            // único e o código único por pessoa — ver `0040_pareamento.sql`.
            | "/api/pareamento/resgatar"
    )
}

/// Rotas buscadas diretamente por elementos HTML, que não mandam header.
///
/// ## R44 — o `EventSource` estava fora desta lista, e o barramento inteiro
/// estava morto
///
/// `EventSource` **não manda header** — é a mesma limitação de `<img>` e
/// `<video>`, e é justamente o que esta função existe pra resolver. Sem
/// `/api/events` aqui, toda conexão do navegador ao barramento tomava **401**,
/// e a API do `EventSource` reage a isso reconectando pra sempre, em silêncio:
/// nenhum erro na tela, nenhum log no cliente, nada.
///
/// **Medido**: `GET /api/events?token=<mídia>` devolvia 401 enquanto o mesmo
/// pedido com header de sessão devolvia 200; no navegador, `readyState = 2`
/// (fechado) a cada tentativa.
///
/// O que estava morto por causa disso, tudo do M3 em diante: o aviso de
/// programa agendado, as atualizações ao vivo do mural, o pedido de fita de
/// volta na locadora — que o §49 chamou de *"o que separa uma rede social de um
/// relatório"* — e a sincronia do player entre aparelhos.
///
/// **O que isto alarga, dito com todas as letras**: um token de mídia vazado
/// passa a poder LER o barramento por 8 horas, além de baixar bytes. O
/// barramento não carrega credencial nem conteúdo — carrega "o que está
/// acontecendo na casa agora", que é o que o §49 já decidiu ser comum entre
/// quem mora aqui. A alternativa era um terceiro tipo de token só pro
/// barramento, e três escopos pra isso é mais máquina do que o risco pede.
fn accepts_query_token(path: &str) -> bool {
    // R80: `/api/stream/{id}/baixar` cai aqui pelo mesmo prefixo — é bytes de
    // mídia buscados por um download, que também não manda cabeçalho.
    path.starts_with("/api/stream/")
        || path.starts_with("/api/hls/")
        || path.starts_with("/artwork/")
        || path.starts_with("/scrub/")
        || path == "/api/events"
        || (path.starts_with("/api/media/") && path.contains("/subtitles"))
}

/// Onde o token **de arte** vale — e é uma lista de um item só (R45).
///
/// O escopo `arte` existe porque a fileira da home da Google TV publica uma
/// `Uri` no `TvProvider` e o launcher a busca dias depois, com o app fechado.
/// Pra isso o token dura um ano, e é justamente o ano que obriga esta função a
/// ser estreita: `/api/stream/` entrega o filme, `/scrub/` entrega quadros do
/// filme, o barramento entrega o que acontece na casa. Nada disso é pôster
/// baixado do TMDB, e nada disso aceita o token longo.
///
/// A conferência não é só aqui: `usuario_por_token_de_arte` filtra
/// `escopo = 'arte'` no `SELECT`, e `usuario_por_token_de_midia` filtra
/// `escopo = 'midia'`. Esta função escolhe **se** vale a pena perguntar; o banco
/// responde **o quê**. Uma camada só seria uma camada a menos do que o prazo
/// pede.
fn aceita_token_de_arte(path: &str) -> bool {
    path.starts_with("/artwork/")
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
    //
    // **E desde a R45 a query resolve dois escopos, nesta ordem.** O de mídia
    // primeiro, porque é o que chega em toda reprodução; o de arte só depois, e
    // só se o caminho for `/artwork/`. Um token de arte numa rota de mídia não
    // encontra linha em nenhuma das duas consultas e cai em 401, que é o que se
    // quer de uma credencial de um ano.
    // **`sessao` só existe no ramo de sessão** (R61). Um token de mídia ou de
    // arte na query não diz de qual aparelho ele é — ele *é* a credencial do
    // aparelho, e quem a emitiu já sabia. Deixar `None` aqui é a resposta
    // honesta, e o `emitir_token_de_midia` trata órfão como órfão.
    let (user, sessao) = match bearer(&request).or_else(|| cookie(&request)) {
        Some(token) => match auth::user_for_token(&state.pool, &token).await {
            Some(autenticado) => (Some(autenticado.user), Some(autenticado.sessao_id)),
            None => (None, None),
        },
        None => {
            let user = match accepts_query_token(&path).then(|| query_token(&request)).flatten() {
                Some(token) => match auth::usuario_por_token_de_midia(&state.pool, &token).await {
                    Some(user) => Some(user),
                    None if aceita_token_de_arte(&path) => {
                        auth::usuario_por_token_de_arte(&state.pool, &token).await
                    }
                    None => None,
                },
                None => None,
            };
            (user, None)
        }
    };

    let Some(user) = user else {
        return Err(AppError::Unauthorized);
    };

    // O handler pega daqui pelo extractor AuthUser/AdminUser.
    request.extensions_mut().insert(user);
    if let Some(sessao) = sessao {
        request.extensions_mut().insert(auth::Sessao(sessao));
    }
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **R44 — o barramento aceita token na query, e é o único jeito.**
    ///
    /// `EventSource` não manda header. Tirar `/api/events` desta lista mata o
    /// aviso de programa, o mural ao vivo, o pedido de fita e a sincronia do
    /// player — tudo de uma vez e **sem nenhum erro na tela**, porque a API do
    /// `EventSource` reconecta pra sempre em silêncio. Foi assim que ficou
    /// quebrado sem ninguém notar.
    #[test]
    fn o_barramento_aceita_token_na_query() {
        assert!(accepts_query_token("/api/events"));
        // E continua sendo uma lista curta: nada de aceitar a API inteira.
        assert!(!accepts_query_token("/api/works"));
        assert!(!accepts_query_token("/api/perfil"));
        assert!(!accepts_query_token("/api/auth/users"));
    }

    #[test]
    fn rotas_de_boot_sao_publicas() {
        assert!(is_public("/api/auth/login"));
        assert!(is_public("/api/auth/setup"));
        assert!(is_public("/api/health"));
        // as duas trocas que acontecem antes de haver sessão deste lado
        assert!(is_public("/api/convites/resgatar"));
        assert!(is_public("/api/pareamento/resgatar"));
        // …mas pedir o código exige sessão: quem pede é o celular já logado
        assert!(!is_public("/api/pareamento"));
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

    /// **A invariante da R45**: o token longo abre pôster, e só pôster.
    ///
    /// Ele dura um ano — 1095 vezes o de mídia. Se esta lista crescer pra
    /// `/api/stream/` ou `/scrub/`, o que vaza num `access.log` deixa de ser
    /// "esta casa tem a capa de tal filme" e passa a ser o acervo, por um ano.
    #[test]
    fn o_token_de_arte_so_abre_arte() {
        assert!(aceita_token_de_arte("/artwork/tmdb-668-poster.jpg"));

        // tudo o mais aceita token na query, mas não o de arte
        for path in [
            "/api/stream/abc",
            "/api/hls/abc/index.m3u8",
            "/scrub/x.jpg",
            "/api/events",
            "/api/media/abc/subtitles/0",
        ] {
            assert!(accepts_query_token(path), "{path} saiu da query");
            assert!(!aceita_token_de_arte(path), "{path} aceitou token de arte");
        }
    }

    /// O escopo não pode viver só no `if` do middleware: quem confere de
    /// verdade é o `SELECT`. Se as duas consultas deixarem de filtrar `escopo`,
    /// um token de arte volta a abrir mídia e o teste acima não percebe.
    #[test]
    fn as_duas_consultas_filtram_escopo() {
        let fonte = include_str!("mod.rs");
        assert!(
            fonte.contains("m.escopo = 'midia'"),
            "a consulta de mídia parou de filtrar escopo"
        );
        assert!(
            fonte.contains("m.escopo = 'arte'"),
            "a consulta de arte parou de filtrar escopo"
        );
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
