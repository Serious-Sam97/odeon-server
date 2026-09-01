//! Legendas: listar, extrair pra WebVTT, ou queimar.
//!
//! **Duas origens.** Embutidas no container, e em arquivo ao lado (`.srt`,
//! `.ass`). O Odeon só lia as embutidas, e isso deixava a maioria dos filmes
//! sem legenda nenhuma: medido neste acervo, **348 de 400 filmes sem faixa
//! embutida têm arquivo de legenda na pasta** — 4.136 arquivos no total. Era a
//! diferença entre "o Jellyfin mostra e o Odeon não".
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
    /// `embutida` ou `arquivo`. A interface mostra a diferença porque ela
    /// importa: faixa em arquivo não pode ser queimada pelo caminho de hoje.
    pub origem: &'static str,
}

/// Extensões que valem como legenda em arquivo.
const EXTENSOES: &[&str] = &["srt", "ass", "ssa", "vtt", "sub"];

/// Índice das externas: **negativo**, começando em -1.
///
/// Espaço separado de propósito. O índice positivo é `0:s:N`, o que o ffmpeg
/// espera pra faixa embutida; misturar os dois faria uma externa de índice 2
/// virar a terceira faixa embutida na hora de extrair. Negativo não colide com
/// nada que o ffmpeg produza.
pub fn indice_externo(ordem: usize) -> i32 {
    -(ordem as i32 + 1)
}

pub fn e_externo(index: i32) -> bool {
    index < 0
}

/// Adivinha idioma e "forçada" pelo nome do arquivo.
///
/// Os dois padrões que aparecem no acervo real:
///
///   `Filme.2019.1080p...pt-BR.srt`          → sufixo depois do nome do vídeo
///   `Subs/Brazilian (Forced).por.srt`       → nome próprio dentro de `Subs/`
///
/// Devolve `(idioma, forcada, rotulo)`. Idioma fica `None` quando o nome não
/// diz — inventar "inglês" porque a maioria é inglês seria mentir com cara de
/// metadado.
pub fn descreve_arquivo(nome_arquivo: &str, stem_do_video: &str) -> (Option<String>, bool, String) {
    let sem_ext = nome_arquivo.rsplit_once('.').map(|(a, _)| a).unwrap_or(nome_arquivo);

    // Tira o nome do vídeo da frente, quando o arquivo é irmão dele.
    let resto = sem_ext
        .strip_prefix(stem_do_video)
        .map(|r| r.trim_start_matches(['.', '-', '_', ' ']))
        .unwrap_or(sem_ext);

    let minusculo = resto.to_lowercase();
    let forcada = minusculo.contains("forced") || minusculo.contains("forçad");

    // O idioma é o último pedaço separado por ponto, quando parece código.
    let idioma = resto
        .rsplit('.')
        .next()
        .map(str::trim)
        .filter(|p| {
            let n = p.chars().count();
            (2..=5).contains(&n) && p.chars().all(|c| c.is_ascii_alphabetic() || c == '-')
        })
        .map(|p| p.to_string());

    // O rótulo é o que sobra sem o código de idioma — e quando não sobra nada,
    // o próprio idioma serve.
    let sem_idioma = match &idioma {
        Some(i) => resto.trim_end_matches(i).trim_end_matches('.').trim().to_string(),
        None => resto.trim().to_string(),
    };
    // Quando não sobra nada, o próprio idioma serve — mas dito, e não em
    // código: `Filme.pt-BR.srt` vira "Português", não "pt-BR".
    let base = if sem_idioma.is_empty() {
        idioma
            .as_deref()
            .map(|i| crate::metadata::regiao::idioma_capitalizado(i).unwrap_or_else(|| i.to_string()))
            .unwrap_or_else(|| "Legenda".into())
    } else {
        sem_idioma
    };

    let rotulo = if forcada && !base.to_lowercase().contains("forc") {
        format!("{base} (forçada)")
    } else {
        base
    };

    (idioma, forcada, rotulo)
}

/// O idioma entra pelo nome, pelo mesmo motivo do `label_for` do áudio: o
/// `language` do container é ISO 639-2 e ia cru pra tela (`eng`). Código fora
/// da tabela cai nele mesmo.
fn label_for(title: &Option<String>, language: &Option<String>, index: i32, forced: bool) -> String {
    let base = title
        .clone()
        .or_else(|| {
            language.as_deref().map(|iso| {
                crate::metadata::regiao::idioma_capitalizado(iso).unwrap_or_else(|| iso.to_string())
            })
        })
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
                origem: "embutida",
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
/// Procura legendas em arquivo ao lado do vídeo.
///
/// Dois lugares, que é onde elas realmente estão: **na mesma pasta**, com nome
/// começando pelo do vídeo (padrão YTS), e numa subpasta **`Subs/`**, onde o
/// nome é do idioma e não do filme.
///
/// Feito na hora do pedido e não no scan de propósito: jogar um `.srt` na pasta
/// passa a valer na hora, sem revarrer 17 mil arquivos.
pub async fn externas(video: &Path) -> Vec<(std::path::PathBuf, SubtitleTrack)> {
    let Some(dir) = video.parent() else { return Vec::new() };
    let stem = video.file_stem().and_then(|s| s.to_str()).unwrap_or("");

    let mut achadas: Vec<(std::path::PathBuf, String)> = Vec::new();

    // 1) irmãs, com o nome do vídeo na frente
    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let caminho = e.path();
            let Some(nome) = caminho.file_name().and_then(|n| n.to_str()) else { continue };
            let ext = caminho
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.to_lowercase())
                .unwrap_or_default();
            if EXTENSOES.contains(&ext.as_str()) && nome.starts_with(stem) {
                achadas.push((caminho.clone(), nome.to_string()));
            }
        }
    }

    // 2) a subpasta Subs/
    for pasta in ["Subs", "subs", "Subtitles", "legendas"] {
        let sub = dir.join(pasta);
        if let Ok(mut rd) = tokio::fs::read_dir(&sub).await {
            while let Ok(Some(e)) = rd.next_entry().await {
                let caminho = e.path();
                let Some(nome) = caminho.file_name().and_then(|n| n.to_str()) else { continue };
                let ext = caminho
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.to_lowercase())
                    .unwrap_or_default();
                if EXTENSOES.contains(&ext.as_str()) {
                    achadas.push((caminho.clone(), nome.to_string()));
                }
            }
        }
    }

    // Ordem estável: `read_dir` não garante nenhuma, e índice que muda entre
    // dois pedidos faria o player pedir a faixa errada.
    achadas.sort_by(|a, b| a.1.cmp(&b.1));

    achadas
        .into_iter()
        .enumerate()
        .map(|(i, (caminho, nome))| {
            let codec = caminho
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or("srt")
                .to_lowercase();
            let (idioma, forcada, rotulo) = descreve_arquivo(&nome, stem);
            let track = SubtitleTrack {
                origem: "arquivo",
                index: indice_externo(i),
                text_based: is_text_based(&codec),
                styled: is_styled(&codec),
                label: rotulo,
                language: idioma,
                title: None,
                forced: forcada,
                default: false,
                codec,
            };
            (caminho, track)
        })
        .collect()
}

/// Converte uma legenda em arquivo pra WebVTT.
pub async fn arquivo_para_webvtt(caminho: &Path) -> anyhow::Result<String> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(caminho)
        .args(["-f", "webvtt", "-"])
        .output()
        .await
        .context("falha ao executar ffmpeg")?;

    if !output.status.success() {
        anyhow::bail!(
            "conversão de {} falhou: {}",
            caminho.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

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

    #[test]
    fn le_idioma_do_sufixo_yts() {
        let (idioma, forcada, rotulo) = descreve_arquivo(
            "1917.2019.1080p.BluRay.x264.AAC5.1-[YTS.MX].pt-BR.srt",
            "1917.2019.1080p.BluRay.x264.AAC5.1-[YTS.MX]",
        );
        // O campo guarda o código do arquivo; o rótulo é o que a tela lê.
        assert_eq!(idioma.as_deref(), Some("pt-BR"));
        assert!(!forcada);
        assert_eq!(rotulo, "Português");
    }

    #[test]
    fn le_nome_dentro_de_subs() {
        let (idioma, forcada, rotulo) =
            descreve_arquivo("Brazilian (Forced).por.srt", "28.Years.Later.2026.1080p");
        assert_eq!(idioma.as_deref(), Some("por"));
        assert!(forcada);
        assert_eq!(rotulo, "Brazilian (Forced)");
    }

    #[test]
    fn sem_sufixo_de_idioma_nao_inventa() {
        // Inventar "eng" porque a maioria é inglês seria mentir com cara de
        // metadado.
        let (idioma, _, rotulo) = descreve_arquivo("Filme.2019.srt", "Filme.2019");
        assert_eq!(idioma, None);
        assert_eq!(rotulo, "Legenda");
    }

    #[test]
    fn indices_externos_nao_colidem_com_embutidos() {
        assert_eq!(indice_externo(0), -1);
        assert_eq!(indice_externo(2), -3);
        assert!(e_externo(-1));
        assert!(!e_externo(0));
    }

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
        assert_eq!(tracks[1].label, "Japonês (forçada)");
    }

    #[test]
    fn arquivo_sem_legenda_devolve_lista_vazia() {
        assert!(from_probe(&json!({ "streams": [] })).is_empty());
        assert!(from_probe(&json!({})).is_empty());
    }
}
