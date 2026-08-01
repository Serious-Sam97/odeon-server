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

/// Evidência que NÃO sai do nome do arquivo.
///
/// Existe porque o teto de um episódio sem ano é exatamente 0.820
/// (0.65 título + 0.05 "sem ano" + 0.08 formato + 0.04 popularidade), e o
/// limiar automático é 0.85. Medido no acervo real: **22.219 de 33.114
/// candidatos (67%)** eram estruturalmente incapazes de entrar sozinhos, e 653
/// deles tinham título IDÊNTICO — casamento perfeito que o score não conseguia
/// expressar.
///
/// A saída óbvia seria baixar o limiar. Seria a errada: o número deixaria de
/// significar "tenho certeza" e passaria a auto-aplicar todo candidato de 0.78,
/// inclusive os que estão errados. Aqui a escolha é a inversa — **adicionar a
/// evidência que falta**, em vez de compensar a ausência dela.
#[derive(Debug, Clone, Default)]
pub struct Evidence {
    /// Quantos OUTROS arquivos da mesma pasta apontam pra este mesmo
    /// `(provider, provider_id)`.
    pub siblings: usize,
}

pub fn score_candidate(guess: &Guess, candidate: &Candidate) -> Score {
    score_with_evidence(guess, candidate, &Evidence::default())
}

pub fn score_with_evidence(
    guess: &Guess,
    candidate: &Candidate,
    evidence: &Evidence,
) -> Score {
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
    //
    // `any_episode()` e não `episode`: o episódio pode vir da numeração ABSOLUTA
    // (`[SubsPlease] Frieren - 12`), que é como fansub numera. A busca em
    // `metadata::search` já usa `any_episode()` e procura em séries; usar
    // `episode` aqui contradizia aquilo — o arquivo era buscado como série e
    // depois penalizado em 0.20 por "parecer filme" contra a série certa.
    let looks_serial = guess.any_episode().is_some();
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

    // 5. Corroboração dos vizinhos — a evidência que o nome do arquivo não tem.
    //
    // Uma pasta de série tem uma série só: medido, 474 de 487 diretórios (97,3%)
    // com episódios já casados apontam para exatamente uma obra. Se cinco
    // arquivos da mesma pasta escolheram a mesma série, o sexto escolhendo ela
    // não é coincidência.
    //
    // TRÊS FREIOS, e eles são o ponto:
    //
    //   - `similarity >= 0.90`: a corroboração CONFIRMA um título que já estava
    //     bom, nunca resgata um ruim. Sem isso, uma pasta inteira errada se
    //     auto-referendaria com alta confiança;
    //   - teto de 0.10: sozinha ela nunca leva nada de 0.55 a 0.85;
    //   - mínimo de 3 vizinhos: dois arquivos concordando é acaso comum.
    //
    // O risco que sobra, e que fica registrado: a evidência é CORRELACIONADA.
    // Se a pasta inteira estiver errada, todos erram juntos e com convicção.
    // Por isso o motivo entra na lista — quem auditar vê de onde veio a certeza.
    if evidence.siblings >= 3 && similarity >= 0.90 {
        let bonus = (evidence.siblings as f32 / 100.0).min(0.10);
        value += bonus;
        reasons.push(format!(
            "outros {} arquivos desta pasta apontam pra esta mesma obra",
            evidence.siblings
        ));
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
    fn episodio_absoluto_tambem_conta_como_seriado() {
        // Fansub numera em episódio ABSOLUTO, sem temporada — é o formato de
        // `[SubsPlease] Frieren - 12`. `metadata::search` já busca isso em
        // séries; se o score olhasse só `guess.episode`, o mesmo arquivo levaria
        // −0.20 por "parecer filme" contra a série certa que ele mesmo achou.
        let absoluto = Guess {
            title: "Frieren".into(),
            absolute_episode: Some(12),
            looks_like_anime: true,
            ..Default::default()
        };

        let serie = score_candidate(&absoluto, &candidate("Frieren", None, "tv"));
        let filme = score_candidate(&absoluto, &candidate("Frieren", None, "movie"));

        assert!(
            serie.value > filme.value,
            "série {} deveria ganhar de filme {}",
            serie.value,
            filme.value
        );
        assert!(serie
            .reasons
            .iter()
            .any(|r| r.contains("parece episódio e o resultado é seriado")));
    }

    #[test]
    fn vizinhos_confirmam_titulo_bom_e_destravam_o_sem_ano() {
        // O caso que motiva o componente: episódio sem ano, título perfeito.
        // Teto sem evidência = 0.820, abaixo do limiar de 0.85.
        let g = guess("Chaves", None, Some(39));
        let c = candidate("Chaves", None, "tv");

        let sozinho = score_candidate(&g, &c);
        assert!(sozinho.value < AUTO_THRESHOLD, "sozinho deu {}", sozinho.value);

        let com_vizinhos = score_with_evidence(&g, &c, &Evidence { siblings: 8 });
        assert!(
            com_vizinhos.value >= AUTO_THRESHOLD,
            "com vizinhos deu {}",
            com_vizinhos.value
        );
        assert!(com_vizinhos
            .reasons
            .iter()
            .any(|r| r.contains("outros 8 arquivos")));
    }

    #[test]
    fn vizinhos_nao_resgatam_titulo_ruim() {
        // O freio que importa: a corroboração CONFIRMA, não resgata. Sem isso
        // uma pasta inteira errada se auto-referendaria com convicção.
        let g = guess("Alguma Coisa", None, Some(1));
        let c = candidate("Outra Coisa Bem Diferente", None, "tv");

        let sozinho = score_candidate(&g, &c);
        let com_vizinhos = score_with_evidence(&g, &c, &Evidence { siblings: 40 });
        assert_eq!(sozinho.value, com_vizinhos.value);
    }

    #[test]
    fn dois_vizinhos_nao_bastam() {
        // Dois arquivos concordando é acaso comum; três já é padrão.
        let g = guess("Chaves", None, Some(39));
        let c = candidate("Chaves", None, "tv");
        assert_eq!(
            score_candidate(&g, &c).value,
            score_with_evidence(&g, &c, &Evidence { siblings: 2 }).value
        );
        assert!(
            score_with_evidence(&g, &c, &Evidence { siblings: 3 }).value
                > score_candidate(&g, &c).value
        );
    }

    #[test]
    fn a_corroboracao_tem_teto() {
        // Uma pasta com 500 arquivos não vale mais que uma com 10.
        let g = guess("Chaves", None, Some(39));
        let c = candidate("Chaves", None, "tv");
        let dez = score_with_evidence(&g, &c, &Evidence { siblings: 10 });
        let quinhentos = score_with_evidence(&g, &c, &Evidence { siblings: 500 });
        assert!((quinhentos.value - dez.value) <= 0.1001);
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
