use axum::body::Body;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{acesso, AdminUser, AuthUser};
use crate::error::{AppError, AppResult};
use crate::transcode::{audio, decide, session::SessionInfo, subtitles, MediaInfo, PlaybackPlan};
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
    /// Não entra em nenhuma decisão de plano — sai direto pra resposta (R57),
    /// porque a playlist de HLS não consegue dizer quanto o filme dura.
    duration_seconds: Option<f64>,
}

async fn load_file(state: &AppState, media_file_id: Uuid) -> AppResult<FileRow> {
    sqlx::query_as::<_, FileRow>(
        "SELECT path, status, container, video_codec, audio_codec, height, bitrate, probe,
                duration_seconds
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
    /// Índice da faixa de áudio (`0:a:N`). Ausente = a primeira, que é o que
    /// sempre tocou.
    pub audio_track: Option<i32>,
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
    /// `audio_track` entra já resolvido contra o arquivo (ver `resolver_audio`),
    /// e não cru da query: o `decide` precisa do índice que vai tocar de fato.
    fn to_capabilities(&self, audio_track: Option<i32>) -> decide::ClientCapabilities {
        let defaults = decide::ClientCapabilities::default();
        decide::ClientCapabilities {
            containers: split(&self.containers).unwrap_or(defaults.containers),
            video_codecs: split(&self.video_codecs).unwrap_or(defaults.video_codecs),
            audio_codecs: split(&self.audio_codecs).unwrap_or(defaults.audio_codecs),
            max_height: self.max_height,
            max_bitrate: self.max_bitrate,
            supports_hls: self.supports_hls.unwrap_or(true),
            burn_subtitle: self.burn_subtitle,
            audio_track,
        }
    }
}

/// As faixas do arquivo e o `MediaInfo` que descreve **a que vai tocar**.
///
/// Os dois andam juntos porque separá-los é justamente o defeito: decidir pelo
/// codec de uma faixa e mapear outra faz o selo mentir. `/plan` e `/session`
/// chamam isto pelo mesmo motivo — o plano que o selo mostra tem que ser o
/// plano que a sessão executa.
fn resolver_audio(
    file: &FileRow,
    pedido: Option<i32>,
) -> AppResult<(Vec<audio::AudioTrack>, MediaInfo, Option<i32>)> {
    let faixas = file.probe.as_ref().map(audio::from_probe).unwrap_or_default();
    // Copiados pra soltar o empréstimo — as faixas seguem pra resposta.
    let escolhida = audio::escolher(&faixas, pedido)
        .map_err(AppError::BadRequest)?
        .map(|f| (f.index, f.codec.clone()));

    // Sem faixa escolhida, o codec do banco — que é o da primeira faixa
    // (`probe.rs`) e é o que sempre valeu. Cobre os dois casos em que a lista
    // sai vazia: arquivo realmente mudo (a coluna é nula também, e `None` é o
    // que o `decide` já trata como "não é motivo de transcode") e arquivo cuja
    // `probe` não foi guardada, onde cair pra `None` transformaria um
    // transcode legítimo em direct play que não toca.
    let audio_codec = match &escolhida {
        Some((_, codec)) => Some(codec.clone()),
        None => file.audio_codec.clone(),
    };

    let media = MediaInfo {
        container: file.container.clone(),
        video_codec: file.video_codec.clone(),
        audio_codec,
        height: file.height,
        bitrate: file.bitrate,
        // R67 — sai do `pix_fmt` da probe que já está guardada; nenhuma leitura
        // de disco a mais.
        video_bit_depth: file.probe.as_ref().and_then(|p| profundidade_do_video(p)),
    };

    Ok((faixas, media, escolhida.map(|(index, _)| index)))
}

/// Bits por amostra do vídeo, lidos do `pix_fmt` da `probe` (R67).
///
/// O ffprobe não devolve um campo "profundidade": devolve o formato de pixel,
/// e a profundidade está no nome — `yuv420p` é 8, `yuv420p10le` é 10,
/// `yuv420p12le` é 12. É feio e é a fonte que existe.
///
/// `None` quando não há vídeo ou o formato não diz. Não saber **não** vira
/// motivo pra recodificar: o `profundidade_ok` só barra o que ele tem certeza
/// de que passa de 8 bits.
fn profundidade_do_video(probe: &serde_json::Value) -> Option<u8> {
    let pix_fmt = probe
        .get("streams")?
        .as_array()?
        .iter()
        .find(|s| s.get("codec_type").and_then(|t| t.as_str()) == Some("video"))?
        .get("pix_fmt")?
        .as_str()?;

    Some(profundidade_do_pix_fmt(pix_fmt))
}

/// `yuv420p10le` → 10. `yuv420p` → 8.
///
/// A profundidade é o número que vem **logo depois do `p`** da subamostragem;
/// sem número, são 8 bits. Procurar o número solto na string não serve, e o
/// caso que prova isso é `yuv410p` — um formato de 8 bits que contém "10".
fn profundidade_do_pix_fmt(pix_fmt: &str) -> u8 {
    let bytes = pix_fmt.as_bytes();
    for (i, c) in bytes.iter().enumerate() {
        if *c != b'p' {
            continue;
        }
        let digitos: String = pix_fmt[i + 1..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(bits) = digitos.parse::<u8>() {
            if (8..=16).contains(&bits) {
                return bits;
            }
        }
    }
    8
}

#[derive(Debug, Serialize)]
pub struct PlanResponse {
    #[serde(flatten)]
    pub plan: PlaybackPlan,
    /// URL a usar quando o plano é Direct Play. `None` nos demais.
    pub direct_url: Option<String>,
    pub subtitles: Vec<subtitles::SubtitleTrack>,
    /// Todas as faixas do arquivo — inclusive as que este plano não vai tocar.
    ///
    /// É daqui que o botão de áudio sai. Em transcode o player só enxerga a
    /// faixa que entrou na playlist, então perguntar a ele quantas existem
    /// sempre responde "uma"; a lista tem que vir de quem leu o arquivo.
    pub audio_tracks: Vec<audio::AudioTrack>,
    /// Duração do arquivo inteiro, em segundos.
    ///
    /// **Está aqui porque a playlist de HLS não consegue dizer isso** (R57). Em
    /// sessão de transcode o `.m3u8` é `EVENT` e só lista o que o ffmpeg já
    /// produziu: medido em 17/08/2026, 60 segundos depois de abrir *Django
    /// Livre* (2h45) a playlist declarava 1.128 s — a barra mostrava 18 min de
    /// filme e o "faltam" mentia até o transcode alcançar.
    ///
    /// O servidor **não tem como** declarar a duração na própria playlist: o
    /// tamanho de cada segmento é decidido pelos keyframes da fonte quando o
    /// vídeo é copiado, e levantá-los custa 1m33s por arquivo (medido, sem
    /// decodificar, no mesmo filme) — mais que o tempo todo de espera da
    /// playlist. Então vem por fora, e quem desenha a barra usa este número em
    /// vez do que o player deduziu do `.m3u8`.
    pub duration_seconds: Option<f64>,
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

    let (audio_tracks, media, faixa) = resolver_audio(&file, caps.audio_track)?;

    let plan = decide::plan(&media, &caps.to_capabilities(faixa));
    let tracks = todas_as_legendas(&file).await;

    Ok(Json(PlanResponse {
        direct_url: (!plan.needs_session()).then(|| format!("/api/stream/{media_file_id}")),
        plan,
        subtitles: tracks,
        audio_tracks,
        duration_seconds: file.duration_seconds,
    }))
}

#[derive(Debug, Serialize)]
pub struct SessionResponse {
    #[serde(flatten)]
    pub session: SessionInfo,
    pub playlist_url: String,
    /// Duração do arquivo inteiro — a mesma do `PlanResponse`, e pelo mesmo
    /// motivo (R57). Repetida aqui porque quem abre sessão direto, sem passar
    /// pelo `/plan`, é justamente quem mais precisa dela: a playlist que ele
    /// vai receber declara só o que já foi transcodificado.
    pub duration_seconds: Option<f64>,
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

    let (_, media, faixa) = resolver_audio(&file, caps.audio_track)?;
    let plan = decide::plan(&media, &caps.to_capabilities(faixa));

    let session = state
        .transcode
        .start(
            media_file_id,
            std::path::Path::new(&file.path),
            plan,
            caps.start.unwrap_or(0.0).max(0.0),
            user.id,
            // R50: o codec da fonte escolhe o filtro que reinsere SPS/PPS em
            // cada segmento. Aplicar o filtro errado impede o ffmpeg de subir.
            media.video_codec.as_deref(),
        )
        .await?;

    Ok(Json(SessionResponse {
        playlist_url: format!("/api/hls/{}/{}", session.id, crate::transcode::PLAYLIST_NAME),
        session,
        duration_seconds: file.duration_seconds,
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
    RawQuery(query): RawQuery,
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

    // R48 — a playlist sai com o token nos segmentos, quando o pedido veio por
    // token.
    //
    // O ffmpeg escreve os segmentos como nomes crus (`seg00001.ts`), e um nome
    // cru resolve contra o **caminho** da playlist: a query fica pra trás. Quem
    // manda `Authorization` não se importa — o cabeçalho vai em todo pedido — e
    // é por isso que Android e web nunca esbarraram nisto.
    //
    // O `AVPlayer` do iOS esbarra: `AVFoundation` não deixa injetar cabeçalho
    // de forma suportada, então a única credencial que ele tem é a que está na
    // URL, e ela morre na primeira linha da playlist. Sem isto, os 53% do
    // acervo que chegam ao iOS por `direct_stream` respondem 401 segmento a
    // segmento.
    //
    // **Só reescreve quando o token veio na query.** Cliente de cabeçalho ou de
    // cookie recebe a playlist intacta, byte a byte como antes — o que faz esta
    // mudança não ter lado de fora.
    //
    // E não é credencial nova: é o mesmo token de mídia que já estava na URL da
    // playlist, com o mesmo escopo e o mesmo vencimento. O que muda é ele
    // aparecer também nas linhas de dentro.
    let bytes = match (is_playlist, query.as_deref().and_then(token_da_query)) {
        (true, Some(token)) => reescrever_playlist(&bytes, &token).into_bytes(),
        _ => bytes,
    };

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

/// O `token=` de uma query crua, sem montar mapa nem alocar o resto (R48).
///
/// Mesma leitura que `auth::middleware::query_token` faz do lado do portão — e
/// tem que ser a mesma, senão a playlist sairia carimbada com um token que o
/// middleware não usou pra deixar o pedido entrar.
fn token_da_query(query: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|par| par.split_once('='))
        .find(|(chave, _)| *chave == "token")
        .map(|(_, valor)| valor.to_string())
}

/// Carimba o token em cada URI de segmento da playlist (R48).
///
/// O que **não** é URI e passa intacto: linha vazia e linha de diretiva
/// (`#EXTM3U`, `#EXTINF`, `#EXT-X-ENDLIST`…). O que sobra num `-f hls` de
/// segmento mpegts é o nome do arquivo, e é só nele que se mexe.
///
/// ⚠️ Se um dia a sessão passar a gerar **fMP4** (`-hls_segment_type fmp4`) ou
/// segmento cifrado, aparecem `#EXT-X-MAP:URI="init.mp4"` e
/// `#EXT-X-KEY:...URI="..."` — duas diretivas que **carregam URI dentro** e que
/// esta função deixaria passar sem token. O teste
/// `diretiva_com_uri_e_um_caso_conhecido` guarda essa fronteira.
/// `#EXT-X-MAP:URI="init.mp4"` → `…URI="init.mp4?token=…"`.
///
/// `None` quando a linha não é um `EXT-X-MAP` — e aí quem chama segue com a
/// regra normal. Reescrever por dentro do atributo, e não a linha inteira,
/// preserva o que mais vier junto (`BYTERANGE`, por exemplo).
fn mapa_com_token(corpo: &str, token: &str) -> Option<String> {
    let resto = corpo.strip_prefix("#EXT-X-MAP:")?;
    let inicio = resto.find("URI=\"")? + 5;
    let fim = inicio + resto[inicio..].find('"')?;
    let uri = &resto[inicio..fim];
    let separador = if uri.contains('?') { '&' } else { '?' };
    Some(format!(
        "#EXT-X-MAP:{}{uri}{separador}token={token}{}",
        &resto[..inicio],
        &resto[fim..]
    ))
}

fn reescrever_playlist(bytes: &[u8], token: &str) -> String {
    let texto = String::from_utf8_lossy(bytes);
    let mut saida = String::with_capacity(texto.len() + 16);

    for linha in texto.split_inclusive('\n') {
        let corpo = linha.trim_end_matches(['\r', '\n']);
        let fim = &linha[corpo.len()..];

        // R66 — o `#EXT-X-MAP` é a exceção da regra "linha com `#` é
        // diretiva e passa batido": ele **é** uma URL, só que dentro de um
        // atributo. Sem o token ali, o `AVPlayer` — que não manda cabeçalho —
        // baixa os segmentos e não consegue o `init.mp4`, que é onde moram os
        // parâmetros do codec. Ou seja: 401 no único arquivo sem o qual nada
        // decodifica.
        if let Some(reescrita) = mapa_com_token(corpo, token) {
            saida.push_str(&reescrita);
            saida.push_str(fim);
            continue;
        }

        if corpo.is_empty() || corpo.starts_with('#') {
            saida.push_str(linha);
            continue;
        }

        // O ffmpeg escreve nome cru, mas juntar `?` num nome que já tenha query
        // produziria uma URL quebrada — e sai mais barato conferir que supor.
        let separador = if corpo.contains('?') { '&' } else { '?' };
        saida.push_str(corpo);
        saida.push(separador);
        saida.push_str("token=");
        saida.push_str(token);
        saida.push_str(fim);
    }

    saida
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Uma playlist como o `-f hls` desta casa escreve: diretivas, e nomes de
    /// segmento crus.
    fn playlist() -> &'static str {
        "#EXTM3U\n\
         #EXT-X-VERSION:3\n\
         #EXT-X-TARGETDURATION:4\n\
         #EXT-X-MEDIA-SEQUENCE:0\n\
         #EXTINF:4.000000,\n\
         seg00000.ts\n\
         #EXTINF:4.000000,\n\
         seg00001.ts\n\
         #EXT-X-ENDLIST\n"
    }

    /// **A R48 inteira**: o segmento sai com o token, a diretiva sai intacta.
    #[test]
    fn o_token_entra_so_nas_linhas_de_segmento() {
        let saida = reescrever_playlist(playlist().as_bytes(), "deadbeef");

        assert!(saida.contains("seg00000.ts?token=deadbeef"));
        assert!(saida.contains("seg00001.ts?token=deadbeef"));
        // as diretivas não são tocadas
        assert!(saida.contains("#EXTINF:4.000000,\n"));
        assert!(saida.contains("#EXT-X-ENDLIST\n"));
        assert!(!saida.contains("#EXTM3U?token"));
        // e nenhuma linha ganhou token duas vezes
        assert_eq!(saida.matches("token=").count(), 2);
    }

    /// O `AVPlayer` conta os segmentos; uma linha a mais ou a menos quebra a
    /// reprodução tanto quanto um 401.
    #[test]
    fn a_playlist_mantem_a_forma() {
        let entrada = playlist();
        let saida = reescrever_playlist(entrada.as_bytes(), "abc");
        assert_eq!(entrada.lines().count(), saida.lines().count());
        assert!(saida.ends_with('\n'), "a última quebra sumiu");
        assert!(saida.starts_with("#EXTM3U\n"));
    }

    /// Playlist com CRLF não pode virar `seg.ts?token=abc\r` — o `\r` entraria
    /// no valor do token e o portão recusaria o segmento.
    #[test]
    fn crlf_sobrevive_com_o_token_no_lugar_certo() {
        let saida = reescrever_playlist(b"#EXTM3U\r\nseg0.ts\r\n", "abc");
        assert!(saida.contains("seg0.ts?token=abc\r\n"), "veio {saida:?}");
    }

    /// Nome que já tenha query ganha `&`, e não um segundo `?`.
    #[test]
    fn segmento_com_query_usa_e_comercial() {
        let saida = reescrever_playlist(b"seg0.ts?x=1\n", "abc");
        assert!(saida.contains("seg0.ts?x=1&token=abc"));
    }

    /// A leitura do token tem que ser a mesma do portão: o middleware é quem
    /// deixou o pedido entrar, e carimbar outro valor na playlist mandaria o
    /// player buscar segmento com credencial que não foi validada.
    #[test]
    fn o_token_sai_da_query_como_no_portao() {
        assert_eq!(token_da_query("token=abc").as_deref(), Some("abc"));
        assert_eq!(
            token_da_query("start=10&token=abc&outro=1").as_deref(),
            Some("abc")
        );
        assert!(token_da_query("start=10").is_none());
        assert!(token_da_query("").is_none());
    }

    /// **A fronteira conhecida da R48.** Hoje a sessão gera mpegts, e a única
    /// URI da playlist é a linha de segmento. Se um dia ela gerar fMP4 ou
    /// segmento cifrado, `#EXT-X-MAP` e `#EXT-X-KEY` trazem URI **dentro da
    /// diretiva** — e elas passariam sem token, com o iOS tomando 401 no
    /// `init.mp4` em vez de nos segmentos.
    ///
    /// O teste não conserta: ele registra que a função sabe o que não cobre, e
    /// falha no dia em que alguém mudar o tipo de segmento sem ler isto.
    #[test]
    fn diretiva_com_uri_e_um_caso_conhecido() {
        let saida = reescrever_playlist(b"#EXT-X-MAP:URI=\"init.mp4\"\nseg0.ts\n", "abc");
        assert!(saida.contains("seg0.ts?token=abc"));
        // **O dia chegou.** Este teste nasceu na R48 dizendo "se isto mudar, o
        // gerador passou a emitir fMP4 e a R48 precisa cobrir a diretiva". A
        // R66 passou a emitir fMP4 em HEVC, e a diretiva agora é coberta.
        assert!(
            saida.contains("#EXT-X-MAP:URI=\"init.mp4?token=abc\"\n"),
            "o init.mp4 saiu sem token e nada vai decodificar: {saida}"
        );
    }

    /// O token entra **dentro** do atributo, e o que vier junto continua lá.
    #[test]
    fn o_mapa_preserva_os_outros_atributos() {
        let linha = "#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"1024@0\"";
        let saida = mapa_com_token(linha, "abc").expect("era um EXT-X-MAP");
        assert_eq!(
            saida,
            "#EXT-X-MAP:URI=\"init.mp4?token=abc\",BYTERANGE=\"1024@0\""
        );
    }

    /// Só o `EXT-X-MAP` é exceção; as outras diretivas seguem intocadas — uma
    /// `#EXTINF` com `?` no meio não pode virar URL.
    #[test]
    fn as_outras_diretivas_nao_sao_tocadas() {
        assert_eq!(mapa_com_token("#EXTINF:4.000000,", "abc"), None);
        assert_eq!(mapa_com_token("#EXT-X-ENDLIST", "abc"), None);
        assert_eq!(mapa_com_token("seg0.ts", "abc"), None);
    }
    use serde_json::json;

    fn arquivo(probe: Option<serde_json::Value>, audio_codec: Option<&str>) -> FileRow {
        FileRow {
            path: "/media/f.mkv".into(),
            status: "probed".into(),
            container: Some("matroska".into()),
            video_codec: Some("h264".into()),
            audio_codec: audio_codec.map(str::to_string),
            height: Some(1080),
            bitrate: Some(5_000_000),
            probe,
            duration_seconds: Some(9924.0),
        }
    }

    fn dual() -> serde_json::Value {
        json!({"streams": [
            {"codec_type": "video", "codec_name": "h264"},
            {"codec_type": "audio", "codec_name": "ac3", "channels": 6, "tags": {"language": "por"}},
            {"codec_type": "audio", "codec_name": "aac", "channels": 2, "tags": {"language": "eng"}}
        ]})
    }

    #[test]
    fn sem_pedido_o_plano_decide_pela_primeira_faixa() {
        let (faixas, media, escolhida) = resolver_audio(&arquivo(Some(dual()), Some("ac3")), None).unwrap();
        assert_eq!(faixas.len(), 2);
        assert_eq!(escolhida, Some(0));
        assert_eq!(media.audio_codec.as_deref(), Some("ac3"));
    }

    #[test]
    fn pedir_a_segunda_faixa_troca_o_codec_que_o_plano_le() {
        let (_, media, escolhida) = resolver_audio(&arquivo(Some(dual()), Some("ac3")), Some(1)).unwrap();
        assert_eq!(escolhida, Some(1));
        // é isto que impede o selo de mentir: o veredito sai do aac, não do ac3
        assert_eq!(media.audio_codec.as_deref(), Some("aac"));
    }

    #[test]
    fn faixa_inexistente_e_400_e_nao_sessao_muda() {
        let erro = resolver_audio(&arquivo(Some(dual()), Some("ac3")), Some(7)).unwrap_err();
        assert!(matches!(erro, AppError::BadRequest(_)));
    }

    /// Sem `probe` guardada não há lista de faixas — e cair pra `None` faria um
    /// transcode legítimo virar direct play que não toca.
    #[test]
    fn arquivo_sem_probe_mantem_o_codec_do_banco() {
        let (faixas, media, escolhida) = resolver_audio(&arquivo(None, Some("truehd")), None).unwrap();
        assert!(faixas.is_empty());
        assert_eq!(escolhida, None);
        assert_eq!(media.audio_codec.as_deref(), Some("truehd"));
    }

    #[test]
    fn arquivo_mudo_continua_sem_audio() {
        let probe = json!({"streams": [{"codec_type": "video", "codec_name": "h264"}]});
        let (_, media, escolhida) = resolver_audio(&arquivo(Some(probe), None), None).unwrap();
        assert_eq!(escolhida, None);
        assert_eq!(media.audio_codec, None);
    }

    /// **R67.** A profundidade sai do nome do formato de pixel, e procurar o
    /// número solto na string não serve: `yuv410p` é de 8 bits e contém "10".
    #[test]
    fn a_profundidade_sai_do_p_da_subamostragem() {
        assert_eq!(profundidade_do_pix_fmt("yuv420p"), 8);
        assert_eq!(profundidade_do_pix_fmt("yuv420p10le"), 10);
        assert_eq!(profundidade_do_pix_fmt("yuv422p12le"), 12);
        assert_eq!(profundidade_do_pix_fmt("yuvj420p"), 8);
        // o caso que derruba a busca ingênua por "10"
        assert_eq!(profundidade_do_pix_fmt("yuv410p"), 8);
        // formato sem `p` nenhum cai no padrão
        assert_eq!(profundidade_do_pix_fmt("rgb24"), 8);
    }

    /// Só o fluxo de **vídeo** conta: um `pix_fmt` de capa embutida não pode
    /// decidir o plano.
    #[test]
    fn a_profundidade_vem_do_fluxo_de_video() {
        let probe = json!({"streams": [
            {"codec_type": "audio", "codec_name": "aac"},
            {"codec_type": "video", "codec_name": "hevc", "pix_fmt": "yuv420p10le"}
        ]});
        assert_eq!(profundidade_do_video(&probe), Some(10));
    }

    /// Sem `probe`, ou sem `pix_fmt`, a resposta é `None` — e não saber não
    /// recodifica nada.
    #[test]
    fn sem_pix_fmt_a_profundidade_nao_barra() {
        let probe = json!({"streams": [{"codec_type": "video", "codec_name": "hevc"}]});
        assert_eq!(profundidade_do_video(&probe), None);
    }

    /// O contrato que os quatro clientes leem. O `flatten` do plano e um
    /// `rename` errado quebram isto sem quebrar compilação nenhuma.
    #[test]
    fn o_json_do_plano_tem_as_chaves_que_o_cliente_espera() {
        let (audio_tracks, media, faixa) =
            resolver_audio(&arquivo(Some(dual()), Some("ac3")), Some(1)).unwrap();
        let caps = CapsQuery::default().to_capabilities(faixa);
        let plan = decide::plan(&media, &caps);

        let v = serde_json::to_value(PlanResponse {
            direct_url: None,
            plan,
            subtitles: vec![],
            audio_tracks,
            duration_seconds: Some(9924.0),
        })
        .unwrap();

        assert_eq!(v["audio_track"], json!(1));
        assert_eq!(v["audio_tracks"][0]["index"], json!(0));
        assert_eq!(v["audio_tracks"][0]["codec"], json!("ac3"));
        assert_eq!(v["audio_tracks"][0]["label"], json!("Português (5.1)"));
        assert_eq!(v["audio_tracks"][1]["index"], json!(1));
        assert_eq!(v["audio_tracks"][1]["label"], json!("Inglês (estéreo)"));
        // R57: a barra do player precisa da duração do filme inteiro, e a
        // playlist de HLS em transcode só sabe dizer o que já foi produzido.
        assert_eq!(v["duration_seconds"], json!(9924.0));
        // o que já existia continua no mesmo lugar
        assert!(v.get("mode").is_some());
        assert!(v.get("subtitles").is_some());
    }
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
