//! Wrapper de `ffprobe`.
//!
//! Decisão de arquitetura: FFmpeg entra como SUBPROCESSO, nunca como binding
//! (`ffmpeg-next`/libav via FFI). É o que o Jellyfin faz, é o que sobrevive a
//! upgrade de FFmpeg, e evita meses de dor com unsafe e build de C.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context};
use serde::Deserialize;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct Probed {
    pub container: Option<String>,
    pub duration_seconds: Option<f64>,
    pub bitrate: Option<i64>,
    pub video_codec: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub frame_rate: Option<f64>,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<i32>,
    pub subtitle_langs: Vec<String>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct FfProbeOutput {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<Format>,
}

#[derive(Debug, Deserialize)]
struct Format {
    format_name: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Stream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
    channels: Option<i32>,
    r_frame_rate: Option<String>,
    #[serde(default)]
    tags: HashMap<String, String>,
}

pub async fn probe(path: &Path) -> anyhow::Result<Probed> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .await
        .context("falha ao executar ffprobe")?;

    if !output.status.success() {
        bail!(
            "ffprobe saiu com {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let raw: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("ffprobe devolveu JSON inválido")?;
    let parsed: FfProbeOutput = serde_json::from_value(raw.clone())?;

    let video = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"));
    let audio = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("audio"));

    let subtitle_langs = parsed
        .streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("subtitle"))
        .map(|s| {
            s.tags
                .get("language")
                .cloned()
                .unwrap_or_else(|| "und".to_string())
        })
        .collect();

    let format = parsed.format;

    Ok(Probed {
        // "matroska,webm" → "matroska"
        container: format
            .as_ref()
            .and_then(|f| f.format_name.as_ref())
            .map(|n| n.split(',').next().unwrap_or(n).to_string()),
        duration_seconds: format
            .as_ref()
            .and_then(|f| f.duration.as_ref())
            .and_then(|d| d.parse().ok()),
        bitrate: format
            .as_ref()
            .and_then(|f| f.bit_rate.as_ref())
            .and_then(|b| b.parse().ok()),
        video_codec: video.and_then(|s| s.codec_name.clone()),
        width: video.and_then(|s| s.width),
        height: video.and_then(|s| s.height),
        frame_rate: video
            .and_then(|s| s.r_frame_rate.as_ref())
            .and_then(|r| parse_rational(r)),
        audio_codec: audio.and_then(|s| s.codec_name.clone()),
        audio_channels: audio.and_then(|s| s.channels),
        subtitle_langs,
        raw,
    })
}

/// ffprobe devolve frame rate como fração: "24000/1001" → 23.976
fn parse_rational(value: &str) -> Option<f64> {
    let (num, den) = value.split_once('/')?;
    let num: f64 = num.parse().ok()?;
    let den: f64 = den.parse().ok()?;
    if den == 0.0 {
        return None;
    }
    Some(num / den)
}

#[cfg(test)]
mod tests {
    use super::parse_rational;

    #[test]
    fn frame_rate_ntsc() {
        let fps = parse_rational("24000/1001").unwrap();
        assert!((fps - 23.976).abs() < 0.001);
    }

    #[test]
    fn frame_rate_zero_nao_explode() {
        assert!(parse_rational("0/0").is_none());
    }
}
