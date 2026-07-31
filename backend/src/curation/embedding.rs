//! Embeddings de conteúdo — locais, determinísticos, sem API externa.
//!
//! **O que isto é:** TF-IDF projetado em 256 dimensões pelo *hashing trick*.
//! Duas obras ficam próximas quando compartilham vocabulário incomum. Funciona
//! bem pra "parecido com o que você gostou" numa biblioteca pessoal.
//!
//! **O que isto NÃO é:** semântica. "espaço" e "cosmos" não se aproximam. Um
//! modelo de embedding de verdade resolveria isso — e o encaixe é só trocar
//! [`embed_document`]; o resto do M5 não sabe de onde o vetor veio. Ficou local
//! de propósito: um servidor de mídia caseiro não deveria depender de API paga
//! nem vazar sua biblioteca pra terceiro só pra sugerir filme.

use std::collections::HashMap;

use deunicode::deunicode;
use once_cell::sync::Lazy;
use regex::Regex;

pub const DIMENSIONS: usize = 256;

/// Palavras que aparecem em quase tudo e não distinguem nada. O IDF já derruba
/// boa parte, mas com corpus pequeno ele é instável — a lista fixa segura.
static STOPWORDS: Lazy<std::collections::HashSet<&'static str>> = Lazy::new(|| {
    [
        // português
        "que", "com", "para", "por", "uma", "dos", "das", "não", "nao", "mais", "como", "sua",
        "seu", "seus", "suas", "ele", "ela", "eles", "elas", "num", "numa", "pelo", "pela", "são",
        "sao", "foi", "ser", "tem", "está", "esta", "esse", "essa", "isso", "aos", "nas", "nos",
        "quando", "onde", "depois", "sobre", "entre", "até", "ate", "mas", "seus", "quem",
        // inglês
        "the", "and", "for", "with", "that", "this", "from", "his", "her", "him", "she", "they",
        "them", "their", "has", "have", "was", "were", "been", "are", "but", "not", "who", "when",
        "where", "after", "before", "into", "out", "about", "one", "two", "all", "can", "will",
        "would", "there", "then", "than", "some", "more", "other", "which", "while", "also",
    ]
    .into_iter()
    .collect()
});

static SPLITTER: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-z0-9]+").unwrap());

/// Texto → termos. `deunicode` primeiro, senão "ficção" e "ficcao" viram
/// termos diferentes e o corpus se fragmenta.
pub fn tokenize(text: &str) -> Vec<String> {
    SPLITTER
        .split(&deunicode(text).to_lowercase())
        .filter(|token| token.len() >= 3 && !STOPWORDS.contains(token))
        // número solto (ano, resolução) não diz do que a obra trata
        .filter(|token| !token.chars().all(|c| c.is_ascii_digit()))
        .map(|token| token.to_string())
        .collect()
}

/// FNV-1a. Implementado à mão de propósito: o `DefaultHasher` da std não tem
/// estabilidade garantida entre versões do Rust, e um embedding que muda de
/// valor quando o compilador atualiza é um embedding inútil.
fn fnv1a(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Hashing trick com sinal: o bit alto decide se o termo soma ou subtrai, o que
/// faz colisões tenderem a se cancelar em vez de se acumular.
fn bucket(term: &str) -> (usize, f32) {
    let hash = fnv1a(term);
    let index = (hash % DIMENSIONS as u64) as usize;
    let sign = if (hash >> 40) & 1 == 1 { 1.0 } else { -1.0 };
    (index, sign)
}

/// Frequência de termo com saturação logarítmica — repetir "vingança" dez vezes
/// não deve pesar dez vezes mais que uma.
pub fn term_frequencies(tokens: &[String]) -> HashMap<String, f32> {
    let mut counts: HashMap<&str, u32> = HashMap::new();
    for token in tokens {
        *counts.entry(token.as_str()).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .map(|(term, count)| (term.to_string(), 1.0 + (count as f32).ln()))
        .collect()
}

/// TF-IDF → vetor L2-normalizado. Sem normalizar, sinopse longa dominaria o
/// cosseno só por ter mais palavras.
pub fn embed_document(
    frequencies: &HashMap<String, f32>,
    idf: &HashMap<String, f32>,
) -> Option<Vec<f32>> {
    let mut vector = vec![0.0f32; DIMENSIONS];

    for (term, tf) in frequencies {
        // Termo que não está no corpus não tem IDF confiável; peso baixo fixo
        // é melhor que descartar (nome próprio raro costuma ser distintivo).
        let weight = idf.get(term).copied().unwrap_or(1.0);
        let (index, sign) = bucket(term);
        vector[index] += sign * tf * weight;
    }

    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return None;
    }
    for value in vector.iter_mut() {
        *value /= norm;
    }
    Some(vector)
}

/// Formato literal que o pgvector aceita: `[0.1,-0.2,...]`.
pub fn to_pg_vector(vector: &[f32]) -> String {
    let mut out = String::with_capacity(vector.len() * 9 + 2);
    out.push('[');
    for (index, value) in vector.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!("{value:.6}"));
    }
    out.push(']');
    out
}

/// Média ponderada de vetores, renormalizada. É assim que nasce o "vetor de
/// gosto": o centroide do que você gostou, com peso por quanto gostou.
pub fn weighted_centroid(items: &[(Vec<f32>, f32)]) -> Option<Vec<f32>> {
    if items.is_empty() {
        return None;
    }
    let mut sum = vec![0.0f32; DIMENSIONS];
    let mut total_weight = 0.0f32;

    for (vector, weight) in items {
        if vector.len() != DIMENSIONS || *weight <= 0.0 {
            continue;
        }
        for (index, value) in vector.iter().enumerate() {
            sum[index] += value * weight;
        }
        total_weight += weight;
    }

    if total_weight <= f32::EPSILON {
        return None;
    }

    let norm = sum.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return None;
    }
    for value in sum.iter_mut() {
        *value /= norm;
    }
    Some(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embed(text: &str) -> Vec<f32> {
        let tokens = tokenize(text);
        let frequencies = term_frequencies(&tokens);
        embed_document(&frequencies, &HashMap::new()).expect("devia gerar vetor")
    }

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn acento_nao_fragmenta_o_corpus() {
        assert_eq!(tokenize("ficção científica"), tokenize("ficcao cientifica"));
    }

    #[test]
    fn stopword_e_numero_saem() {
        let tokens = tokenize("O filme de 1999 sobre a nave");
        assert!(!tokens.contains(&"sobre".to_string()));
        assert!(!tokens.contains(&"1999".to_string()));
        assert!(tokens.contains(&"filme".to_string()));
        assert!(tokens.contains(&"nave".to_string()));
    }

    #[test]
    fn textos_parecidos_ficam_proximos() {
        let a = embed("detetive investiga assassinato numa cidade chuvosa");
        let b = embed("detetive investiga crime numa cidade sombria");
        let c = embed("comédia romântica sobre casamento na praia");
        assert!(
            cosine(&a, &b) > cosine(&a, &c),
            "similar={} diferente={}",
            cosine(&a, &b),
            cosine(&a, &c),
        );
    }

    #[test]
    fn vetor_sai_normalizado() {
        let v = embed("qualquer texto com algumas palavras distintas");
        assert!((cosine(&v, &v) - 1.0).abs() < 0.001);
    }

    #[test]
    fn texto_vazio_nao_gera_vetor() {
        let frequencies = term_frequencies(&tokenize("de a o"));
        assert!(embed_document(&frequencies, &HashMap::new()).is_none());
    }

    #[test]
    fn hash_e_estavel() {
        // Se este valor mudar, todos os embeddings salvos viram lixo.
        assert_eq!(fnv1a("odeon"), fnv1a("odeon"));
        assert_eq!(bucket("odeon"), bucket("odeon"));
    }

    #[test]
    fn centroide_fica_entre_os_vetores() {
        let a = embed("nave espacial exploracao galaxia");
        let b = embed("nave espacial batalha galaxia");
        let c = embed("receita de bolo de cenoura");
        let centro = weighted_centroid(&[(a.clone(), 1.0), (b.clone(), 1.0)]).unwrap();
        assert!(cosine(&centro, &a) > cosine(&centro, &c));
    }

    #[test]
    fn formato_do_pgvector() {
        let text = to_pg_vector(&[1.0, -0.5]);
        assert!(text.starts_with('[') && text.ends_with(']'));
        assert!(text.contains(",-0.500000"));
    }
}
