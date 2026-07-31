//! Confiança de casamento — e o **porquê** dela.
//!
//! O ponto do M1 não é acertar sempre (impossível). É nunca errar em silêncio.
//! Cada componente do score registra um motivo legível, que fica gravado em
//! `match_candidate.reasons` e aparece na fila de revisão.

use deunicode::deunicode;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;

use crate::scanner::guess::Guess;

use super::Candidate;

/// Acima disso o matcher aceita sozinho.
pub const AUTO_THRESHOLD: f32 = 0.85;
/// Abaixo disso nem vale perguntar — fica `unmatched`, mas os candidatos ficam
/// salvos pra busca manual.
pub const REVIEW_THRESHOLD: f32 = 0.55;

#[derive(Debug, Clone, Serialize)]
pub struct Score {
    pub value: f32,
    pub reasons: Vec<String>,
}

static NON_ALNUM: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-z0-9]+").unwrap());

/// "Cidade de Deus" e "cidade-de-deus" e "CIDADE DE DEUS" viram a mesma coisa.
/// `deunicode` resolve acento, que é o que quebra busca em português e japonês
/// romanizado.
fn normalize(text: &str) -> String {
    let folded = deunicode(text).to_lowercase();
    NON_ALNUM.replace_all(&folded, " ").trim().to_string()
}

fn title_similarity(guess_title: &str, candidate: &Candidate) -> (f32, String) {
    let needle = normalize(guess_title);

    let mut best = 0.0f32;
    let mut best_against = String::new();

    for option in [Some(&candidate.title), candidate.original_title.as_ref()]
        .into_iter()
        .flatten()
    {
        let hay = normalize(option);
        if hay.is_empty() {
            continue;
        }
        let sim = strsim::jaro_winkler(&needle, &hay) as f32;
        if sim > best {
            best = sim;
            best_against = option.clone();
        }
    }

    let reason = if best >= 0.97 {
        format!("título idêntico a \"{best_against}\"")
    } else if best >= 0.85 {
        format!("título parecido com \"{best_against}\" ({best:.2})")
    } else {
        format!("título diferente de \"{best_against}\" ({best:.2})")
    };

    (best, reason)
}

pub fn score_candidate(guess: &Guess, candidate: &Candidate) -> Score {
    let mut reasons = Vec::new();

    // 1. Título — o sinal dominante (peso 0.65).
    let (similarity, title_reason) = title_similarity(&guess.title, candidate);
    reasons.push(title_reason);
    let mut value = similarity * 0.65;

    // 2. Ano — o desempate mais forte que existe. Um ano batendo transforma um
    //    "talvez" em "é esse"; um ano errado por mais de um mata o candidato.
    match (guess.year, candidate.year) {
        (Some(g), Some(c)) if g == c => {
            value += 0.25;
            reasons.push(format!("ano confere: {c}"));
        }
        (Some(g), Some(c)) if (g - c).abs() == 1 => {
            value += 0.15;
            reasons.push(format!("ano quase confere: {g} vs {c} (lançamento x estreia)"));
        }
        (Some(g), Some(c)) => {
            value -= 0.30;
            reasons.push(format!("ano NÃO confere: {g} vs {c}"));
        }
        (None, _) | (_, None) => {
            value += 0.05;
            reasons.push("sem ano pra comparar".to_string());
        }
    }

    // 3. Formato — se o arquivo parece episódio, um filme não serve.
    let looks_serial = guess.episode.is_some();
    let candidate_serial = candidate.provider_kind != "movie";
    if looks_serial == candidate_serial {
        value += 0.08;
        reasons.push(if looks_serial {
            "arquivo parece episódio e o resultado é seriado".to_string()
        } else {
            "arquivo parece filme e o resultado é filme".to_string()
        });
    } else {
        value -= 0.20;
        reasons.push(if looks_serial {
            "arquivo parece episódio, mas o resultado é filme".to_string()
        } else {
            "arquivo parece filme, mas o resultado é seriado".to_string()
        });
    }

    // 4. Popularidade — só desempate. Nunca decide sozinha, senão todo arquivo
    //    obscuro vira blockbuster de nome parecido.
    let popularity_bonus = (candidate.popularity.min(100.0) / 100.0) * 0.04;
    if popularity_bonus > 0.01 {
        value += popularity_bonus;
        reasons.push(format!("título popular no provider ({:.0})", candidate.popularity));
    }

    Score {
        value: value.clamp(0.0, 1.0),
        reasons,
    }
}

/// Score → estado. `auto` entra sozinho, `needs_review` chama humano,
/// `unmatched` nem pergunta (mas guarda os candidatos).
pub fn state_for(score: f32) -> &'static str {
    if score >= AUTO_THRESHOLD {
        "auto"
    } else if score >= REVIEW_THRESHOLD {
        "needs_review"
    } else {
        "unmatched"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(title: &str, year: Option<i32>, kind: &str) -> Candidate {
        Candidate {
            provider: "tmdb".into(),
            provider_id: "1".into(),
            provider_kind: kind.into(),
            title: title.into(),
            original_title: None,
            year,
            overview: None,
            poster_url: None,
            backdrop_url: None,
            genres: Vec::new(),
            accent_color: None,
            popularity: 0.0,
            raw: serde_json::Value::Null,
        }
    }

    fn guess(title: &str, year: Option<i32>, episode: Option<i32>) -> Guess {
        Guess {
            title: title.into(),
            year,
            season: episode.map(|_| 1),
            episode,
            ..Default::default()
        }
    }

    #[test]
    fn casamento_perfeito_entra_sozinho() {
        let s = score_candidate(
            &guess("Blade Runner 2049", Some(2017), None),
            &candidate("Blade Runner 2049", Some(2017), "movie"),
        );
        assert!(s.value >= AUTO_THRESHOLD, "score foi {}", s.value);
    }

    #[test]
    fn acento_nao_atrapalha() {
        let s = score_candidate(
            &guess("Cidade de Deus", Some(2002), None),
            &candidate("Cidade de Deus", Some(2002), "movie"),
        );
        assert!(s.value >= AUTO_THRESHOLD);
    }

    #[test]
    fn ano_errado_derruba_pra_revisao() {
        let s = score_candidate(
            &guess("Dune", Some(2021), None),
            &candidate("Dune", Some(1984), "movie"),
        );
        assert!(s.value < AUTO_THRESHOLD, "score foi {}", s.value);
    }

    #[test]
    fn episodio_nao_casa_com_filme() {
        let serie = score_candidate(
            &guess("Severance", None, Some(7)),
            &candidate("Severance", None, "tv"),
        );
        let filme = score_candidate(
            &guess("Severance", None, Some(7)),
            &candidate("Severance", None, "movie"),
        );
        assert!(serie.value > filme.value);
    }

    #[test]
    fn motivos_sempre_existem() {
        let s = score_candidate(
            &guess("Alguma Coisa", None, None),
            &candidate("Outra Coisa", None, "movie"),
        );
        assert!(!s.reasons.is_empty());
    }
}
