use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{acesso, AdminUser, AuthUser};
use crate::error::{AppError, AppResult};
use crate::transcode::{decide, session::SessionInfo, subtitles, MediaInfo, PlaybackPlan};
use crate::AppState;

#[derive(Debug, sqlx::FromRow)]
struct FileRow {
    path: String,
    status: String,
    container: Option<String>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    height: Option<i32>,
    bitrate: Option<i64>,
    probe: Option<serde_json::Value>,
}

async fn load_file(state: &AppState, media_file_id: Uuid) -> AppResult<FileRow> {
    sqlx::query_as::<_, FileRow>(
        "SELECT path, status, container, video_codec, audio_codec, height, bitrate, probe
         FROM media_file WHERE id = $1",
    )
    .bind(media_file_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)
}

/// Capacidades do cliente pela query string. Listas separadas por vírgula.
#[derive(Debug, Deserialize, Default)]
pub struct CapsQuery {
    pub containers: Option<String>,
    pub video_codecs: Option<String>,
    pub audio_codecs: Option<String>,
    pub max_height: Option<i32>,
    pub max_bitrate: Option<i64>,
    pub supports_hls: Option<bool>,
    pub burn_subtitle: Option<i32>,
    /// Só pro início da sessão.
    pub start: Option<f64>,
}

fn split(value: &Option<String>) -> Option<Vec<String>> {
    value.as_ref().map(|raw| {
        raw.split(',')
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect()
    })
}

impl CapsQuery {
    fn to_capabilities(&self) -> decide::ClientCapabilities {
        let defaults = decide::ClientCapabilities::default();
        decide::ClientCapabilities {
            containers: split(&self.containers).unwrap_or(defaults.containers),
            video_codecs: split(&self.video_codecs).unwrap_or(defaults.video_codecs),
            audio_codecs: split(&self.audio_codecs).unwrap_or(defaults.audio_codecs),
            max_height: self.max_height,
            max_bitrate: self.max_bitrate,
            supports_hls: self.supports_hls.unwrap_or(true),
            burn_subtitle: self.burn_subtitle,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PlanResponse {
    #[serde(flatten)]
    pub plan: PlaybackPlan,
    /// URL a usar quando o plano é Direct Play. `None` nos demais.
    pub direct_url: Option<String>,
    pub subtitles: Vec<subtitles::SubtitleTrack>,
}

/// Só decide — não gasta CPU nem cria sessão. O cliente consulta antes de tocar.
pub async fn plan(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(media_file_id): Path<Uuid>,
    Query(caps): Query<CapsQuery>,
) -> AppResult<Json<PlanResponse>> {
    // R26: bytes de mídia passam pelo `acesso`. Ver `auth/acesso.rs`.
    if !acesso::pode_assistir(&state.pool, &user, media_file_id).await {
        return Err(acesso::negado());
    }

    let file = load_file(&state, media_file_id).await?;

    let media = MediaInfo {
        container: file.container.clone(),
        video_codec: file.video_codec.clone(),
        audio_codec: file.audio_codec.clone(),
        height: file.height,
        bitrate: file.bitrate,
    };

    let plan = decide::plan(&media, &caps.to_capabilities());
    let tracks = todas_as_legendas(&file).await;

    Ok(Json(PlanResponse {
        direct_url: (!plan.needs_session()).then(|| format!("/api/stream/{media_file_id}")),
        plan,
        subtitles: tracks,
    }))
}

#[derive(Debug, Serialize)]
pub struct SessionResponse {
    #[serde(flatten)]
    pub session: SessionInfo,
    pub playlist_url: String,
}

/// Inicia (ou reinicia, com outro offset) uma sessão de transcode.
pub async fn start_session(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(media_file_id): Path<Uuid>,
    Query(caps): Query<CapsQuery>,
) -> AppResult<Json<SessionResponse>> {
    // R26: bytes de mídia passam pelo `acesso`. Ver `auth/acesso.rs`.
    if !acesso::pode_assistir(&state.pool, &user, media_file_id).await {
        return Err(acesso::negado());
    }

    let file = load_file(&state, media_file_id).await?;
    if file.status == "missing" {
        return Err(AppError::BadRequest(
            "arquivo marcado como sumido — rode um scan".into(),
        ));
    }

    let media = MediaInfo {
        container: file.container.clone(),
        video_codec: file.video_codec.clone(),
        audio_codec: file.audio_codec.clone(),
        height: file.height,
        bitrate: file.bitrate,
    };
    let plan = decide::plan(&media, &caps.to_capabilities());

    let session = state
        .transcode
        .start(
            media_file_id,
            std::path::Path::new(&file.path),
            plan,
            caps.start.unwrap_or(0.0).max(0.0),
            user.id,
        )
        .await?;

    Ok(Json(SessionResponse {
        playlist_url: format!("/api/hls/{}/{}", session.id, crate::transcode::PLAYLIST_NAME),
        session,
    }))
}

/// Playlist e segmentos. Cada pedido renova o relógio de ociosidade da sessão.
/// Playlist e segmentos.
///
/// **A sessão pertence a quem a abriu** (R26, §42). Antes, o `session_id` era a
/// autorização inteira: quem o tivesse recebia os bytes. Um UUID não é
/// adivinhável, mas id impalpável é *capacidade*, não permissão — a mesma
/// ressalva que o §9b já fazia sobre o `?token=`, e que deixa de ser acadêmica
/// no dia em que há um convidado no círculo.
///
/// Morador continua alcançando as sessões da casa (`Uuid::nil()`), que são as
/// dos canais ao vivo (§25) e não pertencem a ninguém em particular.
pub async fn hls_file(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((session_id, filename)): Path<(Uuid, String)>,
) -> AppResult<Response> {
    match state.transcode.dono(session_id).await {
        Some(dono) if dono == user.id => {}
        Some(dono) if dono.is_nil() && acesso::e_morador(&user) => {}
        // Sessão de outra pessoa é 404 e não 403: quem pede não deveria saber
        // que ela existe.
        Some(_) => return Err(AppError::NotFound),
        None => return Err(AppError::NotFound),
    }

    let is_playlist = filename.ends_with(".m3u8");

    let path = if is_playlist {
        // O ffmpeg só escreve o .m3u8 depois de fechar o primeiro segmento.
        state
            .transcode
            .wait_for_playlist(session_id)
            .await
            .ok_or_else(|| {
                AppError::BadRequest("a sessão não produziu playlist a tempo".into())
            })?
    } else {
        state
            .transcode
            .resolve(session_id, &filename)
            .await
            .ok_or(AppError::NotFound)?
    };

    let bytes = tokio::fs::read(&path).await.map_err(|_| AppError::NotFound)?;

    let content_type = if is_playlist {
        "application/vnd.apple.mpegurl"
    } else {
        "video/mp2t"
    };

    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type),
    );
    // Segmento de transcode é efêmero: cachear é pedir pra tocar lixo depois
    // que a sessão morreu.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

/// Encerra a sessão. Só o dono — senão qualquer conta derruba a reprodução
/// de qualquer outra.
pub async fn stop_session(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(session_id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    // Encerrar a sessão de outra pessoa derrubaria a reprodução dela. Silêncio
    // e `ok: true` seria pior que 404 — quem chama precisa saber que não fez o
    // que pediu.
    match state.transcode.dono(session_id).await {
        Some(dono) if dono == user.id => {}
        Some(dono) if dono.is_nil() && acesso::e_morador(&user) => {}
        _ => return Err(AppError::NotFound),
    }
    state.transcode.stop(session_id).await;
    Ok(Json(json!({ "ok": true })))
}

/// Quem está transcodificando o quê, agora.
///
/// **Virou rota de administrador na R26.** A auditoria da §42 mostrou que ela
/// respondia 200 pra qualquer conta — e uma lista de sessões ativas é uma lista
/// de quem está assistindo o quê neste instante. Entre moradores isso é
/// convivência; com um convidado no círculo é vigilância.
pub async fn sessions(State(state): State<AppState>, AdminUser(_): AdminUser) -> Json<Vec<SessionInfo>> {
    Json(state.transcode.list().await)
}

/// O que este servidor consegue fazer — com a lista de tudo que foi testado e
/// por que cada coisa foi recusada.
pub async fn capabilities(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "encoder": state.hwaccel.chosen,
        "has_hardware": state.hwaccel.has_hardware,
        "probed": state.hwaccel.probed,
        "active_sessions": state.transcode.list().await.len(),
    }))
}

// ------------------------------------------------------------- legendas

/// Todas as faixas: as embutidas no container e as que estão em arquivo ao
/// lado. O player não precisa saber de onde vieram pra escolher uma.
async fn todas_as_legendas(file: &FileRow) -> Vec<subtitles::SubtitleTrack> {
    let mut faixas = file
        .probe
        .as_ref()
        .map(subtitles::from_probe)
        .unwrap_or_default();
    faixas.extend(
        subtitles::externas(std::path::Path::new(&file.path))
            .await
            .into_iter()
            .map(|(_, t)| t),
    );
    faixas
}

pub async fn list_subtitles(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(media_file_id): Path<Uuid>,
) -> AppResult<Json<Vec<subtitles::SubtitleTrack>>> {
    // R26: bytes de mídia passam pelo `acesso`. Ver `auth/acesso.rs`.
    if !acesso::pode_assistir(&state.pool, &user, media_file_id).await {
        return Err(acesso::negado());
    }
    let file = load_file(&state, media_file_id).await?;
    Ok(Json(todas_as_legendas(&file).await))
}

pub async fn subtitle_vtt(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((media_file_id, index)): Path<(Uuid, i32)>,
) -> AppResult<Response> {
    // R26: bytes de mídia passam pelo `acesso`. Ver `auth/acesso.rs`.
    if !acesso::pode_assistir(&state.pool, &user, media_file_id).await {
        return Err(acesso::negado());
    }
    let file = load_file(&state, media_file_id).await?;

    // Faixa em arquivo: acha o caminho e converte direto, sem passar pelo vídeo.
    if subtitles::e_externo(index) {
        let externas = subtitles::externas(std::path::Path::new(&file.path)).await;
        let (caminho, track) = externas
            .into_iter()
            .find(|(_, t)| t.index == index)
            .ok_or(AppError::NotFound)?;

        if !track.text_based {
            return Err(AppError::BadRequest(format!(
                "a legenda {} é imagem — não vira texto sem OCR",
                track.codec
            )));
        }

        let vtt = subtitles::arquivo_para_webvtt(&caminho).await?;
        return Ok((
            [(header::CONTENT_TYPE, "text/vtt; charset=utf-8")],
            vtt,
        )
            .into_response());
    }

    let tracks = file
        .probe
        .as_ref()
        .map(subtitles::from_probe)
        .unwrap_or_default();

    let track = tracks
        .iter()
        .find(|t| t.index == index)
        .ok_or(AppError::NotFound)?;

    if !track.text_based {
        return Err(AppError::BadRequest(format!(
            "a faixa {index} é {} (imagem) — não vira texto sem OCR. \
             Use burn_subtitle={index} pra queimá-la na imagem.",
            track.codec
        )));
    }

    let vtt = subtitles::extract_webvtt(std::path::Path::new(&file.path), index).await?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/vtt; charset=utf-8")],
        vtt,
    )
        .into_response())
}
