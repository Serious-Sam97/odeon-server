//! Legendas embutidas: listar, extrair pra WebVTT, ou queimar.
//!
//! Duas famílias, com destinos diferentes:
//!
//! - **Texto** (`subrip`, `ass`, `mov_text`, `webvtt`) → extrai pra WebVTT e o
//!   player mostra como faixa nativa. Barato, selecionável, sem transcode.
//! - **Imagem** (`hdmv_pgs_subtitle`, `dvd_subtitle`, `dvb_subtitle`) → são
//!   bitmaps. Não existe conversão pra texto sem OCR; a única saída é **queimar**
//!   na imagem, o que obriga transcode.
//!
//! ASS merece nota: ele carrega posicionamento, fonte, cor e karaokê. Convertido
//! pra WebVTT, tudo isso se perde e sobra o texto puro. Por isso a resposta diz
//! `styled: true` — pra a interface poder oferecer "queimar" a quem quer o
//! visual original (típico de anime com letreiro traduzido).

use std::path::Path;

use anyhow::Context;
use serde::Serialize;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct SubtitleTrack {
    /// Índice ENTRE AS LEGENDAS (`0:s:N`), que é o que o ffmpeg e o filtro
    /// `subtitles=si=` esperam. Não confundir com o índice absoluto do stream.
    pub index: i32,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub forced: bool,
    pub default: bool,
    /// Dá pra virar WebVTT sem OCR?
    pub text_based: bool,
    /// Tem estilo/posição que o WebVTT não representa (ASS/SSA).
    pub styled: bool,
    /// Rótulo pronto pro seletor. Vem do servidor pra os quatro clientes não
    /// reimplementarem a mesma regra cada um do seu jeito.
    pub label: String,
}

fn label_for(title: &Option<String>, language: &Option<String>, index: i32, forced: bool) -> String {
    let base = title
        .clone()
        .or_else(|| language.clone())
        .unwrap_or_else(|| format!("Faixa {}", index + 1));
    if forced {
        format!("{base} (forçada)")
    } else {
        base
    }
}

fn is_text_based(codec: &str) -> bool {
    matches!(
        codec,
        "subrip" | "srt" | "ass" | "ssa" | "webvtt" | "mov_text" | "text" | "microdvd"
    )
}

fn is_styled(codec: &str) -> bool {
    matches!(codec, "ass" | "ssa")
}

/// Lê as faixas do JSON do ffprobe já guardado no `media_file` desde o M0 —
/// sem tocar no disco de novo.
pub fn from_probe(probe: &serde_json::Value) -> Vec<SubtitleTrack> {
    let Some(streams) = probe.get("streams").and_then(|s| s.as_array()) else {
        return Vec::new();
    };

    streams
        .iter()
        .filter(|stream| stream.get("codec_type").and_then(|t| t.as_str()) == Some("subtitle"))
        .enumerate()
        .map(|(index, stream)| {
            let codec = stream
                .get("codec_name")
                .and_then(|c| c.as_str())
                .unwrap_or("desconhecido")
                .to_string();

            let tags = stream.get("tags");
            let disposition = stream.get("disposition");
            let flag = |name: &str| {
                disposition
                    .and_then(|d| d.get(name))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
                    == 1
            };

            let language = tags
                .and_then(|t| t.get("language"))
                .and_then(|v| v.as_str())
                .filter(|v| *v != "und")
                .map(str::to_string);
            let title = tags
                .and_then(|t| t.get("title"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let forced = flag("forced");

            SubtitleTrack {
                index: index as i32,
                text_based: is_text_based(&codec),
                styled: is_styled(&codec),
                label: label_for(&title, &language, index as i32, forced),
                language,
                title,
                forced,
                default: flag("default"),
                codec,
            }
        })
        .collect()
}

/// Extrai uma faixa de texto pra WebVTT. Rápido: não decodifica vídeo.
pub async fn extract_webvtt(source: &Path, index: i32) -> anyhow::Result<String> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(source)
        .args([
            "-map",
            &format!("0:s:{index}"),
            "-c:s",
            "webvtt",
            "-f",
            "webvtt",
            "-",
        ])
        .output()
        .await
        .context("falha ao executar ffmpeg")?;

    if !output.status.success() {
        anyhow::bail!(
            "extração da legenda {index} falhou: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn probe() -> serde_json::Value {
        json!({
            "streams": [
                { "codec_type": "video", "codec_name": "h264" },
                { "codec_type": "audio", "codec_name": "aac" },
                {
                    "codec_type": "subtitle", "codec_name": "subrip",
                    "tags": { "language": "por", "title": "Português" },
                    "disposition": { "default": 1, "forced": 0 }
                },
                {
                    "codec_type": "subtitle", "codec_name": "ass",
                    "tags": { "language": "jpn" },
                    "disposition": { "default": 0, "forced": 1 }
                },
                {
                    "codec_type": "subtitle", "codec_name": "hdmv_pgs_subtitle",
                    "tags": { "language": "eng" }
                }
            ]
        })
    }

    #[test]
    fn indice_e_relativo_as_legendas_nao_aos_streams() {
        let tracks = from_probe(&probe());
        assert_eq!(tracks.len(), 3);
        // a 1ª legenda é o stream 2, mas pro ffmpeg ela é 0:s:0
        assert_eq!(tracks[0].index, 0);
        assert_eq!(tracks[2].index, 2);
    }

    #[test]
    fn pgs_e_imagem_e_nao_vira_webvtt() {
        let tracks = from_probe(&probe());
        assert!(!tracks[2].text_based, "PGS é bitmap");
        assert!(tracks[0].text_based);
    }

    #[test]
    fn ass_e_texto_mas_com_estilo() {
        let ass = &from_probe(&probe())[1];
        assert!(ass.text_based);
        assert!(ass.styled, "ASS perde estilo ao virar WebVTT");
    }

    #[test]
    fn rotulo_marca_forcada() {
        let tracks = from_probe(&probe());
        assert_eq!(tracks[0].label, "Português");
        assert_eq!(tracks[1].label, "jpn (forçada)");
    }

    #[test]
    fn arquivo_sem_legenda_devolve_lista_vazia() {
        assert!(from_probe(&json!({ "streams": [] })).is_empty());
        assert!(from_probe(&json!({})).is_empty());
    }
}
