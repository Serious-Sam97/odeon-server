//! Negociação de codec — e o **porquê** dela.
//!
//! "Por que isso está transcodificando?" é a pergunta que todo mundo faz pro
//! Jellyfin e nunca recebe resposta direita. Aqui o plano de reprodução carrega
//! a lista de motivos, do mesmo jeito que o match do M1 e a recomendação do M5.
//!
//! A escada é sempre a mesma, do mais barato pro mais caro:
//!
//!   Direct Play    → o arquivo vai como está. Custo zero.
//!   Direct Stream  → remuxa o container, copia os streams. Custo quase zero.
//!   Transcode      → recodifica vídeo e/ou áudio. Custo alto.
//!
//! Via Tailscale nos aparelhos do próprio dono, Direct Play cobre quase tudo —
//! foi essa aposta que permitiu adiar este milestone até o fim.

use serde::{Deserialize, Serialize};

/// Acima disto o navegador engasga mesmo suportando o codec, e o ganho visual
/// numa TV doméstica é marginal. Serve de teto quando o cliente não declara um.
const DEFAULT_MAX_HEIGHT: i32 = 2160;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    DirectPlay,
    DirectStream,
    Transcode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamAction {
    Copy,
    Encode,
}

/// O que o cliente diz saber tocar. Tudo opcional: cliente que não declara nada
/// recebe o padrão de navegador moderno.
#[derive(Debug, Clone, Deserialize)]
pub struct ClientCapabilities {
    #[serde(default = "default_containers")]
    pub containers: Vec<String>,
    #[serde(default = "default_video_codecs")]
    pub video_codecs: Vec<String>,
    #[serde(default = "default_audio_codecs")]
    pub audio_codecs: Vec<String>,
    #[serde(default)]
    pub max_height: Option<i32>,
    #[serde(default)]
    pub max_bitrate: Option<i64>,
    #[serde(default = "yes")]
    pub supports_hls: bool,
    /// Índice da faixa de legenda a queimar na imagem. Força transcode.
    #[serde(default)]
    pub burn_subtitle: Option<i32>,
}

fn yes() -> bool {
    true
}

fn default_containers() -> Vec<String> {
    vec!["mp4".into(), "mov".into(), "webm".into()]
}

fn default_video_codecs() -> Vec<String> {
    vec!["h264".into(), "vp8".into(), "vp9".into()]
}

fn default_audio_codecs() -> Vec<String> {
    vec!["aac".into(), "mp3".into(), "opus".into(), "vorbis".into()]
}

impl Default for ClientCapabilities {
    fn default() -> Self {
        Self {
            containers: default_containers(),
            video_codecs: default_video_codecs(),
            audio_codecs: default_audio_codecs(),
            max_height: None,
            max_bitrate: None,
            supports_hls: true,
            burn_subtitle: None,
        }
    }
}

/// O que o arquivo é, na prática. Vem direto do `media_file` preenchido no M0.
#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub height: Option<i32>,
    pub bitrate: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlaybackPlan {
    pub mode: PlaybackMode,
    pub video: StreamAction,
    pub audio: StreamAction,
    /// Altura de saída quando há downscale. `None` = mantém a original.
    pub target_height: Option<i32>,
    pub burn_subtitle: Option<i32>,
    /// Por que este plano, em português. É a razão de este módulo existir.
    pub reasons: Vec<String>,
}

impl PlaybackPlan {
    pub fn needs_session(&self) -> bool {
        self.mode != PlaybackMode::DirectPlay
    }
}

/// ffprobe e navegador nem sempre chamam o codec pelo mesmo nome; isto
/// devolve o nome canônico dos dois lados.
fn codec_static(codec: &str) -> &'static str {
    match codec.to_ascii_lowercase().as_str() {
        "h265" | "hevc" | "h.265" => "hevc",
        "h.264" | "avc" | "avc1" | "h264" => "h264",
        "mp4a" | "aac" => "aac",
        "eac3" | "e-ac-3" => "eac3",
        "ac3" => "ac3",
        "mp3" => "mp3",
        "opus" => "opus",
        "vorbis" => "vorbis",
        "vp8" => "vp8",
        "vp9" => "vp9",
        "av1" => "av1",
        "flac" => "flac",
        "dts" => "dts",
        "truehd" => "truehd",
        "matroska" => "matroska",
        "mp4" => "mp4",
        "mov" => "mov",
        "webm" => "webm",
        _ => "desconhecido",
    }
}

fn supports(list: &[String], codec: &str) -> bool {
    let wanted = codec_static(codec);
    list.iter()
        .any(|entry| codec_static(entry) == wanted || entry.eq_ignore_ascii_case(codec))
}

/// `matroska` no ffprobe é `.mkv` pro resto do mundo, e nenhum navegador toca.
fn container_matches(list: &[String], container: &str) -> bool {
    let lowered = container.to_ascii_lowercase();
    let normalized = match lowered.as_str() {
        "matroska" | "matroska,webm" => "mkv",
        "mov,mp4,m4a,3gp,3g2,mj2" => "mp4",
        "quicktime" => "mov",
        other => other,
    };
    list.iter().any(|entry| {
        let entry = entry.to_ascii_lowercase();
        entry == normalized
            // mkv e webm compartilham container; se o cliente aceita webm e o
            // arquivo é matroska, ainda depende dos codecs — resolvido acima.
            || (normalized == "mkv" && entry == "mkv")
    })
}

pub fn plan(media: &MediaInfo, caps: &ClientCapabilities) -> PlaybackPlan {
    let mut reasons: Vec<String> = Vec::new();

    // --- vídeo -------------------------------------------------------------
    let video_codec = media.video_codec.as_deref().unwrap_or("desconhecido");
    let codec_ok = supports(&caps.video_codecs, video_codec);
    if !codec_ok {
        reasons.push(format!(
            "o cliente não toca vídeo em {}",
            codec_static(video_codec)
        ));
    }

    let max_height = caps.max_height.unwrap_or(DEFAULT_MAX_HEIGHT);
    let too_tall = media.height.map(|h| h > max_height).unwrap_or(false);
    let target_height = if too_tall { Some(max_height) } else { None };
    if too_tall {
        reasons.push(format!(
            "{}p acima do limite de {}p do cliente",
            media.height.unwrap_or(0),
            max_height
        ));
    }

    let too_fat = match (caps.max_bitrate, media.bitrate) {
        (Some(limit), Some(actual)) if actual > limit => {
            reasons.push(format!(
                "{} kbps acima do limite de {} kbps",
                actual / 1000,
                limit / 1000
            ));
            true
        }
        _ => false,
    };

    // Queimar legenda obriga a redesenhar o quadro — não há como copiar vídeo.
    let burn_subtitle = caps.burn_subtitle;
    if burn_subtitle.is_some() {
        reasons.push("legenda pedida queimada na imagem".to_string());
    }

    let video = if codec_ok && !too_tall && !too_fat && burn_subtitle.is_none() {
        StreamAction::Copy
    } else {
        StreamAction::Encode
    };

    // --- áudio -------------------------------------------------------------
    let audio_codec = media.audio_codec.as_deref().unwrap_or("desconhecido");
    let audio_ok = media.audio_codec.is_none() || supports(&caps.audio_codecs, audio_codec);
    if !audio_ok {
        reasons.push(format!(
            "o cliente não toca áudio em {}",
            codec_static(audio_codec)
        ));
    }
    let audio = if audio_ok {
        StreamAction::Copy
    } else {
        StreamAction::Encode
    };

    // --- container ---------------------------------------------------------
    let container = media.container.as_deref().unwrap_or("desconhecido");
    let container_ok = container_matches(&caps.containers, container);

    // --- o veredito --------------------------------------------------------
    let mode = if video == StreamAction::Copy && audio == StreamAction::Copy {
        if container_ok {
            reasons.push("codecs e container batem: vai o arquivo original".to_string());
            PlaybackMode::DirectPlay
        } else {
            reasons.push(format!(
                "codecs batem, mas o container {container} não serve: só remuxa"
            ));
            PlaybackMode::DirectStream
        }
    } else {
        PlaybackMode::Transcode
    };

    // Cliente sem HLS e que precisaria de sessão não tem saída: avisa em vez de
    // servir um arquivo que ele não vai tocar.
    if mode != PlaybackMode::DirectPlay && !caps.supports_hls {
        reasons.push("cliente não suporta HLS — não há caminho viável".to_string());
    }

    PlaybackPlan {
        mode,
        video,
        audio,
        target_height,
        burn_subtitle,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browser() -> ClientCapabilities {
        ClientCapabilities::default()
    }

    fn mp4_h264() -> MediaInfo {
        MediaInfo {
            container: Some("mp4".into()),
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            height: Some(1080),
            bitrate: Some(5_000_000),
        }
    }

    #[test]
    fn caso_comum_e_direct_play() {
        let plan = plan(&mp4_h264(), &browser());
        assert_eq!(plan.mode, PlaybackMode::DirectPlay);
        assert_eq!(plan.video, StreamAction::Copy);
        assert_eq!(plan.audio, StreamAction::Copy);
    }

    #[test]
    fn mkv_com_codecs_bons_so_remuxa() {
        let mut media = mp4_h264();
        media.container = Some("matroska".into());
        let plan = plan(&media, &browser());
        assert_eq!(plan.mode, PlaybackMode::DirectStream);
        // não recodifica nada — é o ponto do Direct Stream
        assert_eq!(plan.video, StreamAction::Copy);
        assert_eq!(plan.audio, StreamAction::Copy);
    }

    #[test]
    fn hevc_no_navegador_vira_transcode() {
        let mut media = mp4_h264();
        media.video_codec = Some("hevc".into());
        let plan = plan(&media, &browser());
        assert_eq!(plan.mode, PlaybackMode::Transcode);
        assert_eq!(plan.video, StreamAction::Encode);
        assert!(plan.reasons.iter().any(|r| r.contains("hevc")));
    }

    #[test]
    fn audio_ruim_nao_recodifica_o_video() {
        let mut media = mp4_h264();
        media.audio_codec = Some("truehd".into());
        let plan = plan(&media, &browser());
        assert_eq!(plan.mode, PlaybackMode::Transcode);
        // o vídeo continua sendo copiado — recodificar os dois é desperdício
        assert_eq!(plan.video, StreamAction::Copy);
        assert_eq!(plan.audio, StreamAction::Encode);
    }

    #[test]
    fn resolucao_acima_do_limite_faz_downscale() {
        let mut media = mp4_h264();
        media.height = Some(2160);
        let caps = ClientCapabilities {
            max_height: Some(1080),
            ..browser()
        };
        let plan = plan(&media, &caps);
        assert_eq!(plan.mode, PlaybackMode::Transcode);
        assert_eq!(plan.target_height, Some(1080));
    }

    #[test]
    fn bitrate_acima_do_limite_transcodifica() {
        let caps = ClientCapabilities {
            max_bitrate: Some(2_000_000),
            ..browser()
        };
        let plan = plan(&mp4_h264(), &caps);
        assert_eq!(plan.mode, PlaybackMode::Transcode);
        assert!(plan.reasons.iter().any(|r| r.contains("kbps")));
    }

    #[test]
    fn queimar_legenda_impede_copia_de_video() {
        let caps = ClientCapabilities {
            burn_subtitle: Some(0),
            ..browser()
        };
        let plan = plan(&mp4_h264(), &caps);
        assert_eq!(plan.mode, PlaybackMode::Transcode);
        assert_eq!(plan.video, StreamAction::Encode);
    }

    #[test]
    fn sempre_ha_um_motivo() {
        for media in [mp4_h264(), MediaInfo::default()] {
            assert!(!plan(&media, &browser()).reasons.is_empty());
        }
    }

    #[test]
    fn arquivo_sem_audio_nao_e_motivo_de_transcode() {
        let mut media = mp4_h264();
        media.audio_codec = None;
        assert_eq!(plan(&media, &browser()).audio, StreamAction::Copy);
    }
}
