mod artwork;
mod enfeites;
mod auth;
mod config;
mod conquistas;
mod cors;
mod curation;
mod desafios;
mod error;
mod events;
mod jobs;
mod llm;
mod live;
mod metadata;
mod models;
mod routes;
mod scanner;
mod scrub;
mod tls;
mod transcode;
mod trivia;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::Mutex;
use axum::http::{header, HeaderValue};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub config: Arc<config::Config>,
    pub scan: scanner::SharedStatus,
    pub matching: metadata::SharedMatchStatus,
    pub scrub: scrub::SharedScrubStatus,
    pub embedding: curation::SharedEmbedStatus,
    pub transcode: Arc<transcode::SessionManager>,
    pub hwaccel: Arc<transcode::Capabilities>,
    pub events: events::Bus,
    pub providers: metadata::Providers,
    /// O redator do guia (R34). `None` quando não há chave — e aí a capa mostra
    /// o tema e os filmes, e omite o ensaio, em vez de inventar prosa.
    pub llm: Option<Arc<llm::Llm>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("odeon=debug,info")),
        )
        .init();

    let config = config::Config::from_env()?;

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&config.database_url)
        .await
        .context("não consegui conectar no Postgres")?;

    // 0012 adiciona o índice de collection_item(work_id) — ver a migração.
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("falha ao rodar as migrations")?;
    tracing::info!("migrations em dia");

    // Fecha o que ficou pendurado de uma execução anterior.
    //
    // Antes de servir, e não depois: o índice único de job ativo bloquearia
    // qualquer operação nova enquanto o job morto continuasse "rodando" — o
    // servidor recusaria varrer alegando varredura em andamento, para sempre.
    let interrompidos = jobs::recover(&pool).await;
    if interrompidos > 0 {
        tracing::warn!(
            interrompidos,
            "operações que estavam em andamento quando o processo foi encerrado"
        );
    }

    seed_default_library(&pool, &config).await?;

    for dir in [&config.artwork_dir, &config.scrub_dir] {
        tokio::fs::create_dir_all(dir)
            .await
            .with_context(|| format!("não consegui criar {}", dir.display()))?;
    }

    let http = reqwest::Client::builder()
        .user_agent(concat!("Odeon/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .context("não consegui montar o cliente HTTP")?;

    let providers = metadata::Providers::new(http, config.tmdb_api_key.clone());

    // Encode de teste real em cada encoder — ver transcode/hwaccel.rs pra por quê.
    let hwaccel = transcode::hwaccel::detect().await;
    let transcode_dir = config.cache_dir.join("transcode");
    // Sessões de execuções anteriores não sobrevivem a restart: o ffmpeg morreu
    // junto e os segmentos ficaram órfãos.
    let _ = tokio::fs::remove_dir_all(&transcode_dir).await;
    tokio::fs::create_dir_all(&transcode_dir).await?;
    let sessions = transcode::SessionManager::new(transcode_dir, hwaccel.chosen.clone());

    let tls_cert = config.tls_cert.clone();
    let tls_key = config.tls_key.clone();
    let https_bind = config.https_bind.clone();
    let https_only = config.https_only;
    let cors_origins = config.allowed_origins.clone();
    let cors_any = config.allow_any_origin;
    let pool_for_check = pool.clone();
    let bind = config.bind.clone();
    let artwork_dir = config.artwork_dir.clone();
    let scrub_dir = config.scrub_dir.clone();
    let state = AppState {
        pool,
        // O redator nasce antes do `Arc`, porque ele lê a chave — e o `config`
        // é movido pra dentro do estado logo abaixo.
        llm: llm::Llm::novo(&config).map(Arc::new),
        config: Arc::new(config),
        scan: Arc::new(Mutex::new(scanner::ScanStatus::default())),
        matching: Arc::new(Mutex::new(metadata::MatchStatus::default())),
        scrub: Arc::new(Mutex::new(scrub::ScrubStatus::default())),
        embedding: Arc::new(Mutex::new(curation::EmbedStatus::default())),
        transcode: sessions,
        hwaccel: Arc::new(hwaccel),
        events: events::bus(),
        providers,
    };

    if auth::needs_setup(&pool_for_check).await {
        tracing::warn!(
            "primeira execução: nenhuma senha definida. \
             Crie o administrador em POST /api/auth/setup — até lá tudo responde 401."
        );
    }

    // O vigia dos lembretes de programa ao vivo.
    live::vigiar_lembretes(state.pool.clone(), state.events.clone());
    // E o da grade, que reimporta antes de a programação acabar.
    live::vigiar_grade(
        state.pool.clone(),
        state.providers.http.clone(),
        state.config.artwork_dir.clone(),
    );

    let app = routes::router(state.clone())
        // pôsteres e backdrops baixados dos providers
        .nest_service("/artwork", ServeDir::new(artwork_dir))
        // folhas de sprite pro preview de seek
        .nest_service("/scrub", ServeDir::new(scrub_dir))
        // O portão fica DEPOIS dos serviços estáticos na ordem de declaração,
        // mas as camadas do axum aplicam de fora pra dentro — então isto cobre
        // /artwork e /scrub também, que é justamente o que se quer.
        .layer(axum::middleware::from_fn_with_state(
            state,
            auth::middleware::require_auth,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(cors::layer(cors_origins, cors_any));

    // --- TLS -------------------------------------------------------------
    match (tls_cert, tls_key) {
        (Some(cert), Some(key)) => {
            // HSTS só quando há TLS de verdade. Mandar o cabeçalho sem
            // certificado prenderia o navegador num HTTPS que não existe.
            let secure_app = app.clone().layer(SetResponseHeaderLayer::if_not_present(
                header::STRICT_TRANSPORT_SECURITY,
                HeaderValue::from_static(tls::HSTS_VALUE),
            ));

            let https_port: u16 = https_bind
                .rsplit(':')
                .next()
                .and_then(|p| p.parse().ok())
                .unwrap_or(8443);

            // A porta HTTP continua de pé: ou servindo (migração), ou só
            // redirecionando. Fechá-la de vez deixaria quem digitou http://
            // sem resposta nenhuma.
            let plain = if https_only {
                tracing::info!("HTTP em {bind} apenas redireciona pra HTTPS");
                tls::redirect_router(https_port)
            } else {
                tracing::warn!(
                    "HTTP em {bind} continua servindo conteúdo. \
                     Ligue ODEON_HTTPS_ONLY=true pra forçar o redirecionamento."
                );
                app
            };

            let plain_listener = tokio::net::TcpListener::bind(&bind)
                .await
                .with_context(|| format!("não consegui abrir {bind}"))?;
            tokio::spawn(async move {
                if let Err(e) = axum::serve(plain_listener, plain).await {
                    tracing::error!(error = %e, "listener HTTP caiu");
                }
            });

            tls::serve(&https_bind, &cert, &key, secure_app).await?;
        }
        (None, None) => {
            let listener = tokio::net::TcpListener::bind(&bind)
                .await
                .with_context(|| format!("não consegui abrir {bind}"))?;
            tracing::info!("Odeon ouvindo em http://{bind} (sem TLS)");
            axum::serve(listener, app).await?;
        }
        // Um dos dois sozinho é quase certamente erro de configuração.
        _ => anyhow::bail!(
            "ODEON_TLS_CERT e ODEON_TLS_KEY precisam ser definidos juntos"
        ),
    }

    Ok(())
}

/// Primeira subida: cria uma biblioteca só quando isso claramente ajuda.
///
/// **Só semeia se houver vídeo solto na raiz.** Se a raiz tem apenas
/// subpastas (`Filmes/`, `Séries/`, `Anime/`), semear uma biblioteca em cima
/// dela reivindicaria tudo como um tipo só — e depois seria preciso apagá-la,
/// perdendo o scan, pra criar as separadas. Nesse caso é melhor não fazer nada
/// e deixar a pessoa escolher na interface.
async fn seed_default_library(pool: &sqlx::PgPool, config: &config::Config) -> anyhow::Result<()> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM library")
        .fetch_one(pool)
        .await?;
    if count > 0 {
        return Ok(());
    }

    let Some(root) = config.media_roots.first() else {
        tracing::warn!("nenhuma pasta de mídia montada — confira MEDIA_PATH no .env");
        return Ok(());
    };

    let mut has_loose_video = false;
    if let Ok(mut dir) = tokio::fs::read_dir(root).await {
        while let Ok(Some(entry)) = dir.next_entry().await {
            let path = entry.path();
            let is_video = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    matches!(
                        e.to_ascii_lowercase().as_str(),
                        "mkv" | "mp4" | "avi" | "mov" | "m4v" | "webm" | "ts" | "m2ts"
                    )
                })
                .unwrap_or(false);
            if is_video && entry.metadata().await.map(|m| m.is_file()).unwrap_or(false) {
                has_loose_video = true;
                break;
            }
        }
    }

    if !has_loose_video {
        tracing::info!(
            root = %root.display(),
            "raiz só tem subpastas — crie as bibliotecas pela aba Pastas, uma por tipo"
        );
        return Ok(());
    }

    let root = root.to_string_lossy().to_string();
    sqlx::query("INSERT INTO library (name, root_path, default_kind) VALUES ($1, $2, 'movie')")
        .bind("Mídias")
        .bind(&root)
        .execute(pool)
        .await?;
    tracing::info!(root = %root, "biblioteca padrão criada");
    Ok(())
}
