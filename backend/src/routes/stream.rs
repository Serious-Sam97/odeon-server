//! Direct Play.
//!
//! O M0 não transcodifica nada: serve o arquivo original com suporte a HTTP
//! Range (seek). Como o acesso é via Tailscale nos seus próprios aparelhos, a
//! banda existe e os codecs são conhecidos — Direct Play cobre a esmagadora
//! maioria dos casos. Transcode é o M6, e é de propósito: é o maior sumidouro
//! de complexidade do projeto inteiro.

use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use crate::auth::{acesso, AuthUser};
use crate::error::{AppError, AppResult};
use crate::AppState;

/// **O arquivo inteiro, remuxado, num download só** — R80.
///
/// ## O pedido, e por que ele é pequeno
///
/// Os baixados do cliente Mac tocam sem servidor, e cobrem 65% do acervo:
/// medido sobre 60 obras, `direct_play` 39 e `direct_stream` 21. O terço que
/// falta é matroska — tocá-lo exige remux, e sem rede não há quem remuxe.
///
/// O custo foi medido do lado do cliente e é o que torna isto barato: dos 30
/// matroska olhados, **os 30 saem com `video=copy` e `audio=copy`**. Nenhum
/// recodifica; só troca a embalagem. É I/O, não CPU.
///
/// ⚠️ **Isto não é o segmentador VOD que ficou parado.** Aquele precisa saber o
/// tamanho de cada segmento antes de produzi-lo — 1m33s por arquivo só pra
/// levantar keyframes. Aqui não há playlist nem segmento: é um `-f mp4` no
/// lugar do `-f hls`, com o mesmo `-c copy`.
///
/// ## O que ele recusa, e por quê
///
/// **Só remux.** Se o plano disser que algum fluxo precisa ser recodificado, a
/// rota devolve 400 com a frase — baixar não pode virar um transcode de duas
/// horas que a pessoa não pediu e não vê. Quem quiser assistir mesmo assim tem
/// o HLS, que existe exatamente pra isso.
///
/// ## O formato
///
/// MP4 **fragmentado** (`frag_keyframe+empty_moov`). Um MP4 comum precisa
/// escrever o índice no começo do arquivo, o que exige saída procurável — e a
/// saída aqui é um cano. O fragmentado dispensa isso, e o `AVPlayer` o abre
/// como qualquer outro.
///
/// ⚠️ **Sem `Content-Length`, e por isso sem barra de progresso.** O tamanho
/// final só é conhecido quando o ffmpeg termina. Mentir um número aqui seria
/// pior que não ter: o cliente que confiasse nele cortaria o arquivo.
pub async fn baixar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(media_file_id): Path<Uuid>,
) -> AppResult<Response> {
    if !acesso::pode_assistir(&state.pool, &user, media_file_id).await {
        return Err(acesso::negado());
    }

    let row: Option<(String, String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT path, status, filename, video_codec, audio_codec
           FROM media_file WHERE id = $1",
    )
    .bind(media_file_id)
    .fetch_optional(&state.pool)
    .await?;

    let (path, status, filename, video, audio) = row.ok_or(AppError::NotFound)?;
    if status == "missing" {
        return Err(AppError::BadRequest(
            "arquivo marcado como sumido — rode um scan".into(),
        ));
    }

    // O contêiner de saída é mp4, e ele não carrega tudo. Recusar aqui, com o
    // nome do codec, é melhor que entregar um arquivo que não abre.
    for (rotulo, codec) in [("vídeo", video.as_deref()), ("áudio", audio.as_deref())] {
        if let Some(c) = codec {
            if !cabe_em_mp4(c) {
                return Err(AppError::BadRequest(format!(
                    "o {rotulo} deste arquivo é {c}, que não cabe num mp4 sem recodificar —                      use o HLS pra assistir"
                )));
            }
        }
    }

    let saida = format!(
        "{}.mp4",
        std::path::Path::new(&filename)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "odeon".into())
    );

    let mut comando = tokio::process::Command::new("ffmpeg");
    comando.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &path,
            "-map",
            "0:v:0?",
            "-map",
            "0:a?",
            "-c",
            "copy",
            // Sem estes dois o mp4 recusa legenda de imagem — e deixa passar
            // uma faixa `bin_data` que veio do mkv — e aborta o arquivo inteiro
            // por causa de algo que ninguém pediu.
            "-sn",
            "-dn",
            "-movflags",
            // ⚠️ `delay_moov` não é enfeite: **sem ele o ffmpeg recusa AC3**,
            // e AC3 é 23 dos 30 matroska que o cliente mediu. A mensagem é
            // exata — *"Cannot write moov atom before AC3 packets"* — e o
            // arquivo sai com 1.023 bytes, só o cabeçalho.
            "frag_keyframe+empty_moov+default_base_moof+delay_moov",
            "-f",
            "mp4",
        ]);

    // ⚠️ `hvc1`, e não o `hev1` que o ffmpeg escolhe sozinho — a mesma lição da
    // R66. São os dois nomes da mesma amostra de HEVC no MP4, e o `AVPlayer`
    // recusa o segundo. Baixar um arquivo que não abre seria o pior dos dois
    // mundos: o download termina e o filme não toca.
    if video.as_deref().map(crate::transcode::decide::codec_static) == Some("hevc") {
        comando.args(["-tag:v", "hvc1"]);
    }

    let mut filho = comando
        .arg("pipe:1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| AppError::Other(anyhow::anyhow!("ffmpeg não subiu: {e}")))?;

    let stdout = filho
        .stdout
        .take()
        .ok_or_else(|| AppError::Other(anyhow::anyhow!("ffmpeg sem saída")))?;

    // Sem `tokio-util` no projeto: o cano vira `Stream` por um canal, e o
    // `spawn` que o alimenta morre junto quando o cliente desiste — o receptor
    // fecha, o `send` falha e o laço sai.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(8);
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut stdout = stdout;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(Ok(axum::body::Bytes::copy_from_slice(&buf[..n]))).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    break;
                }
            }
        }
        // Sem isto o ffmpeg vira zumbi quando o download é abandonado.
        let _ = filho.kill().await;
    });

    let corpo = axum::body::Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx));

    let mut resposta = Response::new(corpo);
    let cab = resposta.headers_mut();
    cab.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{saida}\"")) {
        cab.insert(header::CONTENT_DISPOSITION, v);
    }
    // Um cano não aceita `Range`, e prometer que aceita faria o cliente pedir
    // pedaço e receber o arquivo inteiro do começo.
    cab.insert(header::ACCEPT_RANGES, HeaderValue::from_static("none"));
    Ok(resposta)
}

/// Codecs que o contêiner mp4 carrega sem recodificar.
///
/// A lista é curta de propósito: `mp4` aceita bem menos que `matroska`, e o
/// caso deste acervo é estreito — medido pelo cliente, os 30 matroska olhados
/// são `h264+ac3` (23), `hevc+aac` (4), `h264+aac` (2) e `h264+mp3` (1).
fn cabe_em_mp4(codec: &str) -> bool {
    matches!(
        crate::transcode::decide::codec_static(codec),
        "h264" | "hevc" | "av1" | "aac" | "ac3" | "eac3" | "mp3" | "alac"
    )
}

pub async fn stream(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(media_file_id): Path<Uuid>,
    request: Request,
) -> AppResult<Response> {
    // R26: quem não é morador só recebe bytes do que pegou emprestado.
    //
    // A checagem vem ANTES de tocar no banco por causa do caminho: um 403 que
    // acontece depois de a rota já ter lido o `path` funciona igual, mas passa
    // a mensagem errada pra quem lê o código — a autorização é a primeira
    // pergunta, não um detalhe do fim.
    if !acesso::pode_assistir(&state.pool, &user, media_file_id).await {
        return Err(acesso::negado());
    }

    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT path, status FROM media_file WHERE id = $1",
    )
    .bind(media_file_id)
    .fetch_optional(&state.pool)
    .await?;

    let (path, status) = row.ok_or(AppError::NotFound)?;

    if status == "missing" {
        return Err(AppError::BadRequest(
            "arquivo marcado como sumido — rode um scan".into(),
        ));
    }

    // ServeFile já implementa Range, If-Range, HEAD e Content-Type.
    let response = ServeFile::new(&path)
        .oneshot(request)
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("falha ao servir arquivo: {e}")))?;

    Ok(response.map(axum::body::Body::new).into_response())
}
