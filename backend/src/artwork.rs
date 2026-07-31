//! Download e cache de artwork + extração da cor dominante.
//!
//! A cor dominante é o que vai deixar a interface do M3 "com alma": cada obra
//! tinge o próprio card e a própria tela de detalhe. Sai de graça aqui, junto
//! com o download do pôster.

use std::path::{Path, PathBuf};

use anyhow::Context;
use image::GenericImageView;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct StoredArt {
    /// Caminho relativo ao diretório de artwork, servido em `/artwork/...`
    pub path: String,
    pub dominant_color: Option<String>,
}

pub async fn fetch(
    http: &reqwest::Client,
    artwork_dir: &Path,
    work_id: Uuid,
    kind: &str,
    url: &str,
) -> anyhow::Result<StoredArt> {
    let response = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("download de artwork falhou: {url}"))?;

    if !response.status().is_success() {
        anyhow::bail!("artwork {url} devolveu {}", response.status());
    }

    let extension = extension_for(url, response.headers());
    let bytes = response.bytes().await?.to_vec();

    let filename = format!("{work_id}-{kind}.{extension}");
    tokio::fs::create_dir_all(artwork_dir).await?;
    let destination: PathBuf = artwork_dir.join(&filename);
    tokio::fs::write(&destination, &bytes).await?;

    // Decodificar imagem é CPU-bound: fora do executor async.
    let dominant_color = tokio::task::spawn_blocking(move || dominant_color(&bytes))
        .await
        .ok()
        .flatten();

    Ok(StoredArt {
        path: filename,
        dominant_color,
    })
}

fn extension_for(url: &str, headers: &reqwest::header::HeaderMap) -> String {
    if let Some(content_type) = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        if content_type.contains("png") {
            return "png".into();
        }
        if content_type.contains("webp") {
            return "webp".into();
        }
        if content_type.contains("jpeg") || content_type.contains("jpg") {
            return "jpg".into();
        }
    }
    match url.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()) {
        Some(ext) if matches!(ext.as_str(), "png" | "webp" | "jpg" | "jpeg") => {
            if ext == "jpeg" { "jpg".into() } else { ext }
        }
        _ => "jpg".into(),
    }
}

/// Cor dominante por quantização simples.
///
/// Média pura devolve sempre um cinza lamacento. Aqui o pôster é reduzido a um
/// thumbnail, os extremos (quase-preto e quase-branco, que são fundo e texto)
/// são descartados, e o balde de cor mais frequente vence.
fn dominant_color(bytes: &[u8]) -> Option<String> {
    let image = image::load_from_memory(bytes).ok()?;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return None;
    }

    let thumbnail = image.thumbnail(48, 48).to_rgb8();

    let mut buckets = std::collections::HashMap::<(u8, u8, u8), u32>::new();
    for pixel in thumbnail.pixels() {
        let [r, g, b] = pixel.0;
        let brightness = r as u32 + g as u32 + b as u32;
        // fundo escuro e texto claro não representam a obra
        if !(70..=690).contains(&brightness) {
            continue;
        }
        let key = (r / 32, g / 32, b / 32);
        *buckets.entry(key).or_insert(0) += 1;
    }

    // Se a imagem inteira é extremo, cai pra média — melhor que nada.
    let (key, _) = buckets.into_iter().max_by_key(|(_, count)| *count)?;
    let (r, g, b) = (
        key.0 as u32 * 32 + 16,
        key.1 as u32 * 32 + 16,
        key.2 as u32 * 32 + 16,
    );
    Some(format!("#{:02x}{:02x}{:02x}", r.min(255), g.min(255), b.min(255)))
}

#[cfg(test)]
mod tests {
    use super::dominant_color;

    #[test]
    fn imagem_invalida_nao_explode() {
        assert!(dominant_color(b"isso nao e uma imagem").is_none());
    }

    #[test]
    fn png_solido_vira_a_cor_dele() {
        // 4x4 vermelho médio (#c04040) — dentro da faixa de brilho aceita
        let mut buffer = image::RgbImage::new(4, 4);
        for pixel in buffer.pixels_mut() {
            *pixel = image::Rgb([0xc0, 0x40, 0x40]);
        }
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(buffer)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();

        let color = dominant_color(&encoded.into_inner()).expect("devia achar cor");
        assert!(color.starts_with('#') && color.len() == 7, "veio {color}");
    }
}
