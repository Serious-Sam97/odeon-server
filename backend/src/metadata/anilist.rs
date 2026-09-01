//! Cliente AniList — GraphQL, sem chave de API.
//!
//! É este provider que conserta anime. O TMDB trata anime como série comum:
//! numeração de temporada não bate com o que os fansubs usam, e os títulos
//! romanizados batem mal. O AniList indexa romaji, inglês e nativo ao mesmo
//! tempo — e ainda devolve uma cor de destaque da capa de brinde.

use anyhow::Context;
use serde::Deserialize;

use super::Candidate;

const ENDPOINT: &str = "https://graphql.anilist.co";

const SEARCH_QUERY: &str = r#"
query ($search: String) {
  Page(perPage: 8) {
    media(search: $search, type: ANIME, sort: SEARCH_MATCH) {
      id
      title { romaji english native }
      startDate { year }
      description(asHtml: false)
      coverImage { extraLarge color }
      bannerImage
      format
      genres
      popularity
      episodes
    }
  }
}
"#;

const CREDITS_QUERY: &str = r#"
query ($id: Int) {
  Media(id: $id, type: ANIME) {
    staff(perPage: 12, sort: RELEVANCE) {
      edges { role node { id name { full } image { large } } }
    }
    characters(perPage: 10, sort: [ROLE, RELEVANCE]) {
      edges {
        node { name { full } }
        voiceActors(language: JAPANESE, sort: RELEVANCE) {
          id name { full } image { large }
        }
      }
    }
  }
}
"#;

/// Cargos do AniList → vocabulário do Odeon. O AniList é bem mais verboso que
/// o TMDB ("Key Animation", "2nd Key Animation", "In-Between Animation"), então
/// aqui a filtragem é por prefixo em vez de igualdade.
fn role_for_staff(role: &str) -> Option<&'static str> {
    let lower = role.to_ascii_lowercase();
    if lower.contains("original creator") || lower.contains("original story") {
        Some("creator")
    } else if lower.starts_with("director") || lower == "chief director" {
        Some("director")
    } else if lower.contains("series composition") || lower.contains("script") {
        Some("writer")
    } else if lower.contains("music") {
        Some("composer")
    } else if lower.contains("character design") || lower.contains("animation director") {
        Some("animation")
    } else {
        None
    }
}

#[derive(Clone)]
pub struct AniList {
    http: reqwest::Client,
}

impl AniList {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    pub async fn search(&self, title: &str) -> anyhow::Result<Vec<Candidate>> {
        let body = serde_json::json!({
            "query": SEARCH_QUERY,
            "variables": { "search": title },
        });

        let response = self
            .http
            .post(ENDPOINT)
            .json(&body)
            .send()
            .await
            .context("AniList não respondeu")?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("AniList devolveu {status}: {}", text.trim());
        }

        let parsed: GraphQlResponse = response.json().await.context("AniList: JSON inesperado")?;

        if let Some(errors) = parsed.errors {
            if !errors.is_empty() {
                anyhow::bail!(
                    "AniList: {}",
                    errors
                        .iter()
                        .map(|e| e.message.as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                );
            }
        }

        Ok(parsed
            .data
            .map(|d| d.page.media)
            .unwrap_or_default()
            .into_iter()
            .map(Candidate::from)
            .collect())
    }

    /// Staff e dubladores. Em anime, "quem dubla" é informação de primeira
    /// classe — muita gente escolhe o que assistir por isso.
    pub async fn credits(&self, id: &str) -> anyhow::Result<Vec<super::tmdb::CreditPerson>> {
        let numeric: i64 = id.parse().context("id do AniList não é numérico")?;
        let body = serde_json::json!({
            "query": CREDITS_QUERY,
            "variables": { "id": numeric },
        });

        let response = self.http.post(ENDPOINT).json(&body).send().await?;
        if !response.status().is_success() {
            anyhow::bail!("AniList credits devolveu {}", response.status());
        }

        let parsed: CreditsEnvelope = response.json().await?;
        let Some(media) = parsed.data.and_then(|d| d.media) else {
            return Ok(Vec::new());
        };

        let mut out = Vec::new();

        for edge in media.staff.map(|s| s.edges).unwrap_or_default() {
            let Some(role) = role_for_staff(edge.role.as_deref().unwrap_or("")) else {
                continue;
            };
            let Some(node) = edge.node else { continue };
            out.push(super::tmdb::CreditPerson {
                provider_key: format!("anilist:staff:{}", node.id),
                name: node.name.and_then(|n| n.full).unwrap_or_default(),
                role: role.into(),
                character_name: None,
                position: 0,
                image_url: node.image.and_then(|i| i.large),
                known_for: edge.role,
            });
        }

        for (index, edge) in media
            .characters
            .map(|c| c.edges)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            let character = edge.node.and_then(|n| n.name).and_then(|n| n.full);
            // Só o dublador principal; a lista completa por idioma explodiria.
            let Some(actor) = edge.voice_actors.into_iter().next() else {
                continue;
            };
            out.push(super::tmdb::CreditPerson {
                provider_key: format!("anilist:staff:{}", actor.id),
                name: actor.name.and_then(|n| n.full).unwrap_or_default(),
                role: "voice".into(),
                character_name: character,
                position: index as i32,
                image_url: actor.image.and_then(|i| i.large),
                known_for: Some("Dublagem".to_string()),
            });
        }

        Ok(out.into_iter().filter(|p| !p.name.is_empty()).collect())
    }
}

#[derive(Debug, Deserialize)]
struct CreditsEnvelope {
    data: Option<CreditsData>,
}

#[derive(Debug, Deserialize)]
struct CreditsData {
    #[serde(rename = "Media")]
    media: Option<CreditsMedia>,
}

#[derive(Debug, Deserialize)]
struct CreditsMedia {
    staff: Option<StaffConnection>,
    characters: Option<CharacterConnection>,
}

#[derive(Debug, Deserialize)]
struct StaffConnection {
    #[serde(default = "Vec::new")]
    edges: Vec<StaffEdge>,
}

#[derive(Debug, Deserialize)]
struct StaffEdge {
    role: Option<String>,
    node: Option<PersonNode>,
}

#[derive(Debug, Deserialize)]
struct CharacterConnection {
    #[serde(default = "Vec::new")]
    edges: Vec<CharacterEdge>,
}

#[derive(Debug, Deserialize)]
struct CharacterEdge {
    node: Option<CharacterNode>,
    #[serde(rename = "voiceActors", default = "Vec::new")]
    voice_actors: Vec<PersonNode>,
}

#[derive(Debug, Deserialize)]
struct CharacterNode {
    name: Option<PersonName>,
}

#[derive(Debug, Deserialize)]
struct PersonNode {
    id: i64,
    name: Option<PersonName>,
    image: Option<PersonImage>,
}

#[derive(Debug, Deserialize)]
struct PersonName {
    full: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PersonImage {
    large: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphQlResponse {
    data: Option<Data>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct Data {
    #[serde(rename = "Page")]
    page: Page,
}

#[derive(Debug, Deserialize)]
struct Page {
    #[serde(default = "Vec::new")]
    media: Vec<Media>,
}

#[derive(Debug, Deserialize)]
struct Media {
    id: i64,
    title: Title,
    #[serde(rename = "startDate")]
    start_date: Option<FuzzyDate>,
    description: Option<String>,
    #[serde(rename = "coverImage")]
    cover_image: Option<CoverImage>,
    #[serde(rename = "bannerImage")]
    banner_image: Option<String>,
    format: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    popularity: f32,
}

#[derive(Debug, Deserialize)]
struct Title {
    romaji: Option<String>,
    english: Option<String>,
    native: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FuzzyDate {
    year: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct CoverImage {
    #[serde(rename = "extraLarge")]
    extra_large: Option<String>,
    color: Option<String>,
}

impl From<Media> for Candidate {
    fn from(m: Media) -> Self {
        // Preferência de exibição: inglês → romaji → nativo. O romaji vira
        // `original_title` pra o scorer ter as duas grafias pra comparar.
        let display = m
            .title
            .english
            .clone()
            .or_else(|| m.title.romaji.clone())
            .or_else(|| m.title.native.clone())
            .unwrap_or_else(|| format!("AniList #{}", m.id));

        let original = m.title.romaji.clone().or_else(|| m.title.native.clone());

        // MOVIE no AniList é filme; TV/OVA/ONA/SPECIAL são seriados.
        let kind = if m.format.as_deref() == Some("MOVIE") {
            "movie"
        } else {
            "anime"
        };

        Candidate {
            provider: "anilist".into(),
            provider_id: m.id.to_string(),
            provider_kind: kind.into(),
            title: display,
            original_title: original,
            year: m.start_date.and_then(|d| d.year),
            // A sinopse do AniList vem com <br> e <i> mesmo pedindo asHtml:false.
            overview: m.description.map(|d| strip_html(&d)).filter(|d| !d.is_empty()),
            poster_url: m.cover_image.as_ref().and_then(|c| c.extra_large.clone()),
            backdrop_url: m.banner_image,
            // O AniList não tem catálogo traduzido: `genres` vem em inglês e
            // criava um segundo vocabulário dentro do namespace `genre` —
            // `Comedy` (43) ao lado de `Comédia` (3.228) no painel de filtros.
            genres: m.genres.iter().map(|g| super::genero::em_portugues(g)).collect(),
            accent_color: m.cover_image.and_then(|c| c.color),
            // A escala de popularidade do AniList é bem maior que a do TMDB;
            // normaliza pra o bônus de desempate não dominar o score.
            popularity: (m.popularity / 1000.0).min(100.0),
            raw: serde_json::Value::Null,
        }
    }
}

fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut inside_tag = false;
    for c in input.chars() {
        match c {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&#039;", "'")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::strip_html;

    #[test]
    fn tira_tags_e_entidades() {
        let raw = "Um <i>anime</i> sobre<br>algo &amp; mais.";
        assert_eq!(strip_html(raw), "Um anime sobrealgo & mais.");
    }
}
