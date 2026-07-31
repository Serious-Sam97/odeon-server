//! Navegador de pastas.
//!
//! Existe pra a pessoa escolher onde ficam os filmes sem editar `.env` e
//! reiniciar container. Mas ele expõe o filesystem do servidor, então duas
//! regras não são negociáveis:
//!
//! 1. **Só admin.** Listar diretórios é reconhecimento de terreno.
//! 2. **Só dentro das raízes montadas.** Todo caminho pedido é canonicalizado
//!    e conferido contra a lista — sem isso, `../../..` leria o container
//!    inteiro, e um symlink apontando pra fora faria o mesmo em silêncio.

use std::path::{Path, PathBuf};

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::auth::AdminUser;
use crate::error::{AppError, AppResult};
use crate::AppState;

/// Extensões que contam como mídia — só pra dizer "esta pasta tem N vídeos",
/// que é o sinal que a pessoa procura ao escolher onde apontar a biblioteca.
const VIDEO_EXTS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "m4v", "webm", "ts", "m2ts", "mpg", "mpeg", "wmv", "flv", "ogv",
];

#[derive(Debug, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    /// Quantos vídeos direto nesta pasta (não conta subpastas).
    pub video_count: usize,
    pub has_subdirs: bool,
}

#[derive(Debug, Serialize)]
pub struct Listing {
    pub path: String,
    /// `None` quando já se está numa raiz — a UI esconde o "subir".
    pub parent: Option<String>,
    pub roots: Vec<String>,
    pub entries: Vec<Entry>,
    pub video_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct BrowseQuery {
    #[serde(default)]
    pub path: Option<String>,
}

/// Canonicaliza e confere contra as raízes.
///
/// `canonicalize` resolve symlink e `..` — é o que impede uma pasta "atalho"
/// dentro de `/media` de virar porta pro resto do sistema de arquivos.
fn resolve(requested: &Path, roots: &[PathBuf]) -> Result<PathBuf, AppError> {
    let canonical = requested
        .canonicalize()
        .map_err(|_| AppError::BadRequest(format!("caminho não existe: {}", requested.display())))?;

    let inside = roots.iter().any(|root| {
        root.canonicalize()
            .map(|r| canonical.starts_with(&r))
            .unwrap_or(false)
    });

    if !inside {
        return Err(AppError::Forbidden(
            "caminho fora das pastas montadas no servidor".into(),
        ));
    }
    Ok(canonical)
}

fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub async fn browse(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<BrowseQuery>,
) -> AppResult<Json<Listing>> {
    let roots = &state.config.media_roots;

    // Sem `path`, mostra a primeira raiz. Com uma raiz só (o caso comum), isso
    // já é a pasta certa.
    let requested = match &params.path {
        Some(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => roots
            .first()
            .cloned()
            .ok_or_else(|| AppError::BadRequest("nenhuma pasta montada".into()))?,
    };

    let current = resolve(&requested, roots)?;

    let mut entries: Vec<Entry> = Vec::new();
    let mut video_count = 0usize;

    let mut dir = tokio::fs::read_dir(&current)
        .await
        .map_err(|e| AppError::BadRequest(format!("não consegui ler a pasta: {e}")))?;

    while let Ok(Some(item)) = dir.next_entry().await {
        let path = item.path();
        let name = item.file_name().to_string_lossy().to_string();

        // Ocultos e as pastas de metadata de NAS só poluem a escolha.
        if name.starts_with('.') || name == "@eaDir" || name == "lost+found" {
            continue;
        }

        let Ok(kind) = item.file_type().await else {
            continue;
        };

        if kind.is_file() {
            if is_video(&path) {
                video_count += 1;
            }
            continue;
        }

        if !kind.is_dir() {
            continue;
        }

        // Espia dentro pra mostrar quantos vídeos há — sem descer recursivo,
        // que numa biblioteca grande travaria a listagem.
        let (mut inner_videos, mut has_subdirs) = (0usize, false);
        if let Ok(mut inner) = tokio::fs::read_dir(&path).await {
            while let Ok(Some(child)) = inner.next_entry().await {
                match child.file_type().await {
                    Ok(t) if t.is_dir() => has_subdirs = true,
                    Ok(t) if t.is_file() && is_video(&child.path()) => inner_videos += 1,
                    _ => {}
                }
            }
        }

        entries.push(Entry {
            name,
            path: path.to_string_lossy().to_string(),
            video_count: inner_videos,
            has_subdirs,
        });
    }

    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    // Só oferece "subir" enquanto não se está numa raiz.
    let at_root = roots
        .iter()
        .any(|r| r.canonicalize().map(|r| r == current).unwrap_or(false));
    let parent = if at_root {
        None
    } else {
        current
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .filter(|_| true)
    };

    Ok(Json(Listing {
        path: current.to_string_lossy().to_string(),
        parent,
        roots: roots.iter().map(|r| r.to_string_lossy().to_string()).collect(),
        entries,
        video_count,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> Vec<PathBuf> {
        vec![PathBuf::from("/tmp")]
    }

    #[test]
    fn caminho_inexistente_e_recusado() {
        let err = resolve(Path::new("/tmp/nao-existe-mesmo-12345"), &roots());
        assert!(matches!(err, Err(AppError::BadRequest(_))));
    }

    #[test]
    fn caminho_fora_das_raizes_e_proibido() {
        // /etc existe, mas não está na lista
        let err = resolve(Path::new("/etc"), &roots());
        assert!(matches!(err, Err(AppError::Forbidden(_))));
    }

    #[test]
    fn travessia_com_pontos_nao_escapa() {
        // canonicalize resolve o `..` antes da conferência
        let err = resolve(Path::new("/tmp/../etc"), &roots());
        assert!(matches!(err, Err(AppError::Forbidden(_))));
    }

    #[test]
    fn a_propria_raiz_e_permitida() {
        assert!(resolve(Path::new("/tmp"), &roots()).is_ok());
    }

    #[test]
    fn reconhece_extensao_de_video() {
        assert!(is_video(Path::new("/x/filme.MKV")));
        assert!(is_video(Path::new("/x/a.mp4")));
        assert!(!is_video(Path::new("/x/legenda.srt")));
        assert!(!is_video(Path::new("/x/sem-extensao")));
    }
}
