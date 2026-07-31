//! Nome de arquivo (e de pasta) → estrutura.
//!
//! O diferencial nº1 sobre o Jellyfin. Três coisas que ele faz mal e aqui são
//! tratadas de propósito:
//!
//! 1. **Contexto de diretório.** `Severance/Season 2/S02E07.mkv` não tem título
//!    nenhum no arquivo — o título está duas pastas acima.
//! 2. **Anime.** `[SubsPlease] Frieren - 12 [1080p][A1B2].mkv` não tem temporada,
//!    tem episódio absoluto, e o `[SubsPlease]` não é parte do nome.
//! 3. **Nunca inventar.** Se não dá pra saber, o campo fica `None` e a obra cai
//!    na fila de revisão. Chutar em silêncio é o pecado do Jellyfin.

use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;

/// Ano plausível pra uma obra audiovisual. Serve pra não confundir o "2049" de
/// "Blade Runner 2049" com o ano de lançamento.
const MIN_YEAR: i32 = 1880;
const MAX_YEAR: i32 = 2035;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Guess {
    pub title: String,
    pub year: Option<i32>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    /// Anime numera direto de 1 a N, ignorando temporada.
    pub absolute_episode: Option<i32>,
    pub release_group: Option<String>,
    pub looks_like_anime: bool,
}

impl Guess {
    pub fn kind(&self, library_default: &str) -> String {
        if self.episode.is_some() || self.absolute_episode.is_some() {
            "episode".to_string()
        } else {
            library_default.to_string()
        }
    }

    /// Qualquer numeração de episódio, pro matcher não precisar saber qual é.
    pub fn any_episode(&self) -> Option<i32> {
        self.episode.or(self.absolute_episode)
    }
}

static LEADING_GROUP: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\[([^\]]{1,40})\]\s*").unwrap());
static SEASON_EP: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bs(\d{1,2})[\s._-]*e(\d{1,3})\b").unwrap());
static SEASON_EP_X: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(\d{1,2})x(\d{2,3})\b").unwrap());
/// "Título - 12" / "Título - 12v2" — numeração absoluta de anime.
static ABSOLUTE_EP: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\s-\s*(\d{1,4})(?:v\d+)?\s*(?:\[|\(|$)").unwrap());
static YEAR: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:\b|\()(\d{4})(?:\b|\))").unwrap());
static BRACKETED: Lazy<Regex> = Lazy::new(|| Regex::new(r"[\[\(\{][^\]\)\}]*[\]\)\}]").unwrap());
/// "Season 2", "Temporada 2", "S02" — pasta que não carrega o nome da obra.
static SEASON_DIR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(season|temporada|saison|staffel|s)\s*[._-]?\s*\d{1,2}$").unwrap()
});

/// Tokens que sinalizam "daqui pra frente é metadata de release, não título".
static JUNK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        \b(
            2160p|1080p|720p|480p|uhd|hdr10\+?|hdr|
            x264|x265|h\.?264|h\.?265|hevc|avc|xvid|divx|10bit|8bit|
            bluray|blu-ray|bdrip|brrip|webrip|web-?dl|hdtv|dvdrip|remux|
            proper|repack|extended|unrated|imax|
            aac|ac3|eac3|truehd|atmos|dts(-hd)?|ddp?5[\s._-]?1|
            dual|dublado|legendado|nacional|multi|subbed
        )\b",
    )
    .unwrap()
});

static SPACES: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s{2,}").unwrap());

pub fn guess_from_filename(filename: &str) -> Guess {
    guess_with_hint(filename, false)
}

/// Versão consciente do caminho: usa as pastas acima quando o arquivo sozinho
/// não diz nada, e detecta anime pela árvore de diretórios.
pub fn guess_from_path(path: &Path, root: &Path) -> Guess {
    let filename = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();

    let relative = path.strip_prefix(root).unwrap_or(path);
    let anime_hint = relative.components().any(|component| {
        let name = component.as_os_str().to_string_lossy().to_lowercase();
        name.contains("anime") || name.contains("animes")
    });

    let mut guess = guess_with_hint(&filename, anime_hint);

    if !is_informative(&guess.title) {
        // Sobe a árvore pulando pastas de temporada, até achar um nome que
        // signifique alguma coisa. Dois níveis cobrem Série/Temporada/arquivo.
        let mut current = path.parent();
        for _ in 0..2 {
            let Some(directory) = current else { break };
            if directory == root {
                break;
            }
            let name = directory
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();

            if !SEASON_DIR.is_match(name.trim()) {
                let from_directory = guess_with_hint(&name, anime_hint);
                if is_informative(&from_directory.title) {
                    guess.title = from_directory.title;
                    guess.year = guess.year.or(from_directory.year);
                    break;
                }
            }
            current = directory.parent();
        }
    }

    guess
}

fn guess_with_hint(filename: &str, anime_hint: bool) -> Guess {
    let stem = filename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(filename)
        .to_string();

    let mut guess = Guess {
        looks_like_anime: anime_hint,
        ..Default::default()
    };

    // [SubsPlease] à frente é grupo de fansub, não título.
    let without_group = if let Some(captures) = LEADING_GROUP.captures(&stem) {
        guess.release_group = Some(captures[1].to_string());
        guess.looks_like_anime = true;
        stem[captures.get(0).unwrap().end()..].to_string()
    } else {
        stem.clone()
    };

    // Heurística de scene release: se não há espaço nenhum, os pontos/underscores
    // SÃO os espaços. Se já há espaços, mexer nos pontos quebraria "Mr. Robot".
    let normalized = if without_group.contains(' ') {
        without_group.replace('_', " ")
    } else {
        without_group.replace('.', " ").replace('_', " ")
    };
    let normalized = SPACES.replace_all(&normalized, " ").trim().to_string();

    let mut cut = normalized.len();

    if let Some(c) = SEASON_EP.captures(&normalized) {
        guess.season = c.get(1).and_then(|m| m.as_str().parse().ok());
        guess.episode = c.get(2).and_then(|m| m.as_str().parse().ok());
        cut = cut.min(c.get(0).unwrap().start());
    } else if let Some(c) = SEASON_EP_X.captures(&normalized) {
        guess.season = c.get(1).and_then(|m| m.as_str().parse().ok());
        guess.episode = c.get(2).and_then(|m| m.as_str().parse().ok());
        cut = cut.min(c.get(0).unwrap().start());
    } else if guess.looks_like_anime {
        // Só procura episódio absoluto quando há sinal de anime — senão
        // "Ocean's - 11" e afins viram episódio por engano.
        if let Some(c) = ABSOLUTE_EP.captures(&normalized) {
            guess.absolute_episode = c.get(1).and_then(|m| m.as_str().parse().ok());
            cut = cut.min(c.get(0).unwrap().start());
        }
    }

    // Primeiro número de 4 dígitos que (a) não abre o nome — senão "2001 A Space
    // Odyssey" perde o título — e (b) é um ano plausível — senão "Blade Runner
    // 2049" vira "Blade Runner".
    for c in YEAR.captures_iter(&normalized) {
        let m = c.get(1).unwrap();
        let Ok(value) = m.as_str().parse::<i32>() else {
            continue;
        };
        if m.start() == 0 || !(MIN_YEAR..=MAX_YEAR).contains(&value) {
            continue;
        }
        guess.year = Some(value);
        cut = cut.min(c.get(0).unwrap().start());
        break;
    }

    if let Some(m) = JUNK.find(&normalized) {
        cut = cut.min(m.start());
    }

    let cut_title = &normalized[..cut];
    // Sobra de [1080p] / (hash) que não caiu em nenhuma regra acima.
    let cleaned = BRACKETED.replace_all(cut_title, " ");
    let cleaned = SPACES.replace_all(&cleaned, " ");
    let title = cleaned
        .trim()
        .trim_end_matches(|c: char| "-_.([ ".contains(c))
        .trim()
        .to_string();

    guess.title = if title.is_empty() { normalized } else { title };
    guess
}

/// Um título só serve se sobrar letra. "S02E07", "12", "video" não servem.
fn is_informative(title: &str) -> bool {
    let trimmed = title.trim();
    if trimmed.len() < 3 {
        return false;
    }
    if !trimmed.chars().any(|c| c.is_alphabetic()) {
        return false;
    }
    if SEASON_EP.is_match(trimmed) || SEASON_EP_X.is_match(trimmed) {
        return false;
    }
    let lower = trimmed.to_lowercase();
    !matches!(
        lower.as_str(),
        "video" | "movie" | "film" | "filme" | "index" | "main" | "playback" | "episode"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn g(s: &str) -> Guess {
        guess_from_filename(s)
    }

    #[test]
    fn scene_release_de_filme() {
        let r = g("Blade.Runner.2049.2017.1080p.BluRay.x265-RARBG.mkv");
        assert_eq!(r.title, "Blade Runner 2049");
        assert_eq!(r.year, Some(2017));
        assert_eq!(r.episode, None);
    }

    #[test]
    fn episodio_sxxexx() {
        let r = g("Severance.S02E07.1080p.WEB-DL.mkv");
        assert_eq!(r.title, "Severance");
        assert_eq!(r.season, Some(2));
        assert_eq!(r.episode, Some(7));
    }

    #[test]
    fn episodio_formato_x() {
        let r = g("Arcane 1x03 - The Base Violence Necessary for Change.mkv");
        assert_eq!(r.title, "Arcane");
        assert_eq!(r.season, Some(1));
        assert_eq!(r.episode, Some(3));
    }

    #[test]
    fn nome_ja_limpo_com_espacos() {
        let r = g("Cidade de Deus (2002).mp4");
        assert_eq!(r.title, "Cidade de Deus");
        assert_eq!(r.year, Some(2002));
    }

    #[test]
    fn ano_no_inicio_faz_parte_do_titulo() {
        let r = g("2001 A Space Odyssey 1968 1080p.mkv");
        assert_eq!(r.title, "2001 A Space Odyssey");
        assert_eq!(r.year, Some(1968));
    }

    #[test]
    fn sem_metadata_nenhuma() {
        let r = g("gravacao aleatoria.mov");
        assert_eq!(r.title, "gravacao aleatoria");
        assert_eq!(r.year, None);
    }

    // --- anime ---

    #[test]
    fn anime_com_grupo_e_episodio_absoluto() {
        let r = g("[SubsPlease] Sousou no Frieren - 12 [1080p][A1B2C3D4].mkv");
        assert_eq!(r.title, "Sousou no Frieren");
        assert_eq!(r.absolute_episode, Some(12));
        assert_eq!(r.season, None);
        assert_eq!(r.release_group.as_deref(), Some("SubsPlease"));
        assert!(r.looks_like_anime);
    }

    #[test]
    fn anime_com_versao_de_release() {
        let r = g("[Erai-raws] Bocchi the Rock! - 08v2 [1080p].mkv");
        assert_eq!(r.absolute_episode, Some(8));
        assert!(r.title.starts_with("Bocchi the Rock"));
    }

    #[test]
    fn traco_com_numero_sem_sinal_de_anime_nao_vira_episodio() {
        // sem [grupo] e sem pasta "anime", isso é só um título
        let r = g("Ocean's - 11.mkv");
        assert_eq!(r.absolute_episode, None);
    }

    // --- contexto de diretório ---

    #[test]
    fn titulo_vem_da_pasta_pulando_a_temporada() {
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Severance/Season 2/S02E07.mkv");
        let r = guess_from_path(&path, &root);
        assert_eq!(r.title, "Severance");
        assert_eq!(r.season, Some(2));
        assert_eq!(r.episode, Some(7));
    }

    #[test]
    fn pasta_com_ano_completa_o_arquivo() {
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Cidade de Deus (2002)/video.mkv");
        let r = guess_from_path(&path, &root);
        assert_eq!(r.title, "Cidade de Deus");
        assert_eq!(r.year, Some(2002));
    }

    #[test]
    fn nome_de_arquivo_bom_ignora_a_pasta() {
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Baixados/Blade.Runner.2049.2017.1080p.mkv");
        let r = guess_from_path(&path, &root);
        assert_eq!(r.title, "Blade Runner 2049");
    }

    #[test]
    fn pasta_anime_liga_a_heuristica() {
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Anime/Frieren/Frieren - 04.mkv");
        let r = guess_from_path(&path, &root);
        assert!(r.looks_like_anime);
        assert_eq!(r.absolute_episode, Some(4));
    }
}
