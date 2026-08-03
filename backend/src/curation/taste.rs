//! Perfil de gosto derivado do `play_event`.
//!
//! Nada aqui é declarado pelo usuário. Tudo sai do log cru que o M0 começou a
//! guardar — e é por isso que aquele `play_event` "exagerado" existia.
//!
//! O sinal mais honesto não é "assistiu", é **terminou**. Largar aos 8 minutos
//! diz muito mais que dar play.

use std::collections::HashMap;

use chrono::{DateTime, Timelike, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::embedding;

/// Meia-vida do interesse, em dias. O que você amava há seis meses conta menos
/// que o que você terminou semana passada.
const RECENCY_HALF_LIFE_DAYS: f64 = 60.0;

/// Abaixo disto, começar e parar é rejeição — não interrupção.
const ABANDON_RATIO: f64 = 0.15;
/// Acima disto conta como "gostou", mesmo sem o evento `finish`.
const LIKED_RATIO: f64 = 0.60;
const FINISHED_RATIO: f64 = 0.92;

#[derive(Debug, Clone, Serialize)]
pub struct TasteProfile {
    pub works_touched: usize,
    pub finished: usize,
    pub abandoned: usize,
    /// `namespace:valor` → afinidade em -1..1
    pub tag_affinity: Vec<(String, f32)>,
    /// Pessoas cujo trabalho você termina. Exige mais de uma obra: com uma só,
    /// o elenco inteiro de um filme que você gostou viraria "gosto favorito".
    pub person_affinity: Vec<PersonAffinity>,
    /// Faixa de duração que você de fato termina, em minutos.
    pub preferred_minutes: Option<(i32, i32)>,
    /// 24 posições, normalizadas — a que horas você assiste.
    pub hour_histogram: Vec<f32>,
    /// Centroide do que você gostou. `None` até haver histórico suficiente.
    #[serde(skip)]
    pub taste_vector: Option<Vec<f32>>,
    pub has_taste_vector: bool,
    /// Quantas obras você **avaliou**, e a média — separado de tudo acima.
    ///
    /// O §4.6 exigiu que o perfil inspecionável mostrasse comportamento e
    /// declaração **em campos distintos**. Misturados, não daria pra responder
    /// "o Odeon está me recomendando por causa do que eu vi ou do que eu disse
    /// que gostei?" — e essa pergunta é a razão de o perfil ser inspecionável.
    pub avaliadas: usize,
    pub nota_media: Option<f32>,
}

impl TasteProfile {
    /// Afinidade com uma pessoa, independente do papel — o maior valor manda.
    pub fn person_score(&self, person_id: Uuid) -> Option<&PersonAffinity> {
        self.person_affinity.iter().find(|p| p.id == person_id)
    }

    pub fn affinity_of(&self, tag: &str) -> f32 {
        self.tag_affinity
            .iter()
            .find(|(name, _)| name == tag)
            .map(|(_, value)| *value)
            .unwrap_or(0.0)
    }

    /// Sem histórico não dá pra curar — a UI mostra o catálogo e explica.
    pub fn is_cold_start(&self) -> bool {
        self.works_touched < 3
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PersonAffinity {
    pub id: Uuid,
    pub name: String,
    pub role: String,
    pub score: f32,
    pub works: i64,
}

/// Uma obra só não é evidência. Duas já dizem alguma coisa.
const MIN_WORKS_FOR_PERSON: i64 = 2;

/// Quanto uma nota declarada pode mexer na afinidade — **o número que define a
/// R23** (§39).
///
/// O §4.6 do `IDEIAS.md` deixou a regra e ela é dura: *sinal fraco não manda no
/// forte*. Este módulo inteiro nasceu de "nada é declarado" e "terminar >
/// assistir", porque nota é enviesada — as pessoas dão 5 estrelas pro que acham
/// que **deveriam** gostar, e 1 estrela por raiva do final.
///
/// 0,3 não é um número escolhido pelo gosto: ele é **o maior valor que não
/// inverte nada** na escala que já existia aqui.
///
/// ```text
/// terminou  1.0  +  nota 1 (−0,3)  =  0,7   → continua positivo
/// largou   −0.8  +  nota 5 (+0,3)  = −0,5   → continua negativo
/// ```
///
/// Ou seja: a nota move a obra dentro da faixa que o comportamento já
/// determinou, e não atravessa o zero. Dar cinco estrelas pro que você
/// abandonou aos oito minutos não convence o Odeon de que você gostou — e é
/// exatamente isso que a regra existe pra garantir. Um teste trava isto.
const PESO_DA_NOTA: f64 = 0.3;

/// A nota, de 1 a 5, virada em ajuste de −PESO a +PESO. Três é o meio e vale
/// zero: "achei ok" não é informação a favor nem contra.
fn ajuste_da_nota(nota: i32) -> f64 {
    (nota.clamp(1, 5) - 3) as f64 * (PESO_DA_NOTA / 2.0)
}

#[derive(Debug, sqlx::FromRow)]
struct WorkSignal {
    work_id: Uuid,
    max_ratio: Option<f64>,
    finishes: i64,
    starts: i64,
    last_seen: DateTime<Utc>,
    play_count: Option<i32>,
    duration_seconds: Option<f64>,
    embedding: Option<String>,
    /// A nota declarada, quando existe (R23).
    ///
    /// Ela só chega aqui **junto de um sinal de comportamento**, porque a
    /// consulta parte do `play_event`. Avaliar um filme que você nunca abriu
    /// não cria gosto nenhum — o que é a mesma tese do módulo, aplicada ao
    /// sinal novo: a nota qualifica o que você assistiu, ela não substitui o
    /// assistir.
    nota: Option<i32>,
}

/// Quanto esta obra conta como "gostei", de -1.1 a +1.7.
fn affinity_of(signal: &WorkSignal) -> f32 {
    let ratio = signal.max_ratio.unwrap_or(0.0);

    let base = if signal.finishes > 0 || ratio >= FINISHED_RATIO {
        1.0
    } else if ratio >= LIKED_RATIO {
        0.6
    } else if signal.starts > 0 && ratio <= ABANDON_RATIO {
        // deu play e desistiu cedo
        -0.8
    } else {
        0.1
    };

    // Reassistir é o sinal positivo mais forte que existe.
    let rewatch_bonus = match signal.play_count.unwrap_or(0) {
        0 | 1 => 0.0,
        2 => 0.2,
        _ => 0.4,
    };

    // A nota entra por último e limitada — ver `PESO_DA_NOTA`.
    let nota = signal.nota.map(ajuste_da_nota).unwrap_or(0.0);

    (base + rewatch_bonus + nota) as f32
}

/// Decaimento exponencial por recência.
fn recency_weight(last_seen: DateTime<Utc>, now: DateTime<Utc>) -> f32 {
    let days = (now - last_seen).num_seconds() as f64 / 86_400.0;
    (0.5f64).powf(days.max(0.0) / RECENCY_HALF_LIFE_DAYS) as f32
}

pub async fn build(pool: &PgPool, user_id: Uuid) -> anyhow::Result<TasteProfile> {
    let signals: Vec<WorkSignal> = sqlx::query_as(
        r#"
        SELECT
            pe.work_id,
            max(pe.position_seconds / NULLIF(pe.duration_seconds, 0)) AS max_ratio,
            count(*) FILTER (WHERE pe.event_type = 'finish') AS finishes,
            count(*) FILTER (WHERE pe.event_type = 'start')  AS starts,
            max(pe.created_at) AS last_seen,
            max(ps.play_count) AS play_count,
            max(pe.duration_seconds) AS duration_seconds,
            max(w.embedding::text) AS embedding,
            max(av.nota) AS nota
        FROM play_event pe
        JOIN work w ON w.id = pe.work_id
        LEFT JOIN playback_state ps
               ON ps.work_id = pe.work_id AND ps.user_id = pe.user_id
        -- R23: a nota, quando existe. `LEFT JOIN` porque avaliar é opcional e
        -- sempre será — a curadoria não pode depender de ninguém declarar nada.
        LEFT JOIN avaliacao av
               ON av.work_id = pe.work_id AND av.user_id = pe.user_id
        WHERE pe.user_id = $1
        GROUP BY pe.work_id
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let now = Utc::now();

    let mut finished = 0usize;
    let mut abandoned = 0usize;
    let mut avaliadas = 0usize;
    let mut soma_notas = 0i32;
    let mut liked_vectors: Vec<(Vec<f32>, f32)> = Vec::new();
    let mut work_weights: HashMap<Uuid, f32> = HashMap::new();
    let mut finished_durations: Vec<i32> = Vec::new();

    for signal in &signals {
        if let Some(n) = signal.nota {
            avaliadas += 1;
            soma_notas += n;
        }
        let affinity = affinity_of(signal);
        let weight = affinity * recency_weight(signal.last_seen, now);
        work_weights.insert(signal.work_id, weight);

        if affinity >= 1.0 {
            finished += 1;
            if let Some(duration) = signal.duration_seconds {
                finished_durations.push((duration / 60.0).round() as i32);
            }
        } else if affinity < 0.0 {
            abandoned += 1;
        }

        if weight > 0.0 {
            if let Some(parsed) = signal.embedding.as_deref().and_then(parse_pg_vector) {
                liked_vectors.push((parsed, weight));
            }
        }
    }

    // --- afinidade por tag ------------------------------------------------
    // Cada obra empresta seu peso pras tags que carrega; a normalização por
    // frequência evita que "genre:drama" (que está em tudo) vença sempre.
    // Só as obras que ESTE usuário tocou.
    //
    // A consulta trazia `work_tag ⋈ tag` inteiro — 13.728 linhas neste acervo —
    // e o filtro acontecia em Rust, contra um conjunto que pode ter 3 entradas.
    // O custo escalava com o tamanho da BIBLIOTECA em vez de com o histórico da
    // pessoa, e isso a cada `/for-you` e cada `/taste`.
    let tocadas: Vec<Uuid> = work_weights.keys().copied().collect();

    let tag_rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT wt.work_id, t.namespace || ':' || t.value
         FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
         WHERE wt.work_id = ANY($1)",
    )
    .bind(&tocadas)
    .fetch_all(pool)
    .await?;

    let mut tag_totals: HashMap<String, (f32, f32)> = HashMap::new();
    for (work_id, tag) in tag_rows {
        if let Some(weight) = work_weights.get(&work_id) {
            let entry = tag_totals.entry(tag).or_insert((0.0, 0.0));
            entry.0 += *weight;
            entry.1 += 1.0;
        }
    }

    let mut tag_affinity: Vec<(String, f32)> = tag_totals
        .into_iter()
        .filter(|(_, (_, count))| *count >= 1.0)
        .map(|(tag, (sum, count))| (tag, (sum / count).clamp(-1.0, 1.0)))
        .collect();
    tag_affinity.sort_by(|a, b| b.1.total_cmp(&a.1));
    tag_affinity.truncate(40);

    // --- afinidade por pessoa ---------------------------------------------
    // Mesma lógica das tags: cada obra empresta seu peso a quem trabalhou nela.
    // Só papéis de destaque — o compositor de um filme que você largou não diz
    // nada sobre você.
    // Mesmo caso: eram 61.919 linhas de crédito trazidas pra casar com o
    // punhado de obras que a pessoa assistiu.
    let credit_rows: Vec<(Uuid, Uuid, String, String)> = sqlx::query_as(
        "SELECT c.work_id, p.id, p.name, c.role
         FROM credit c JOIN person p ON p.id = c.person_id
         JOIN credit_role r ON r.role = c.role AND r.featured
         WHERE c.work_id = ANY($1)",
    )
    .bind(&tocadas)
    .fetch_all(pool)
    .await?;

    let mut person_totals: HashMap<(Uuid, String), (f32, i64, String)> = HashMap::new();
    for (work_id, person_id, name, role) in credit_rows {
        if let Some(weight) = work_weights.get(&work_id) {
            let entry = person_totals
                .entry((person_id, role))
                .or_insert((0.0, 0, name));
            entry.0 += *weight;
            entry.1 += 1;
        }
    }

    let mut person_affinity: Vec<PersonAffinity> = person_totals
        .into_iter()
        .filter(|(_, (_, works, _))| *works >= MIN_WORKS_FOR_PERSON)
        .map(|((id, role), (sum, works, name))| PersonAffinity {
            id,
            name,
            role,
            score: (sum / works as f32).clamp(-1.0, 1.0),
            works,
        })
        .collect();
    person_affinity.sort_by(|a, b| b.score.total_cmp(&a.score));
    person_affinity.truncate(20);

    // --- faixa de duração que você termina --------------------------------
    let preferred_minutes = if finished_durations.len() >= 3 {
        finished_durations.sort_unstable();
        let low = finished_durations[finished_durations.len() / 4];
        let high = finished_durations[finished_durations.len() * 3 / 4];
        Some((low, high.max(low + 5)))
    } else {
        None
    };

    // --- a que horas você assiste -----------------------------------------
    let hours: Vec<(DateTime<Utc>,)> = sqlx::query_as(
        "SELECT created_at FROM play_event WHERE user_id = $1 AND event_type = 'start'",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let mut hour_histogram = vec![0.0f32; 24];
    for (timestamp,) in &hours {
        hour_histogram[timestamp.hour() as usize] += 1.0;
    }
    let peak = hour_histogram.iter().cloned().fold(0.0f32, f32::max);
    if peak > 0.0 {
        for value in hour_histogram.iter_mut() {
            *value /= peak;
        }
    }

    let taste_vector = embedding::weighted_centroid(&liked_vectors);

    Ok(TasteProfile {
        works_touched: signals.len(),
        finished,
        abandoned,
        tag_affinity,
        person_affinity,
        preferred_minutes,
        hour_histogram,
        has_taste_vector: taste_vector.is_some(),
        taste_vector,
        avaliadas,
        nota_media: (avaliadas > 0).then(|| soma_notas as f32 / avaliadas as f32),
    })
}

/// pgvector devolve `[0.1,-0.2,...]` quando lido como texto.
pub fn parse_pg_vector(text: &str) -> Option<Vec<f32>> {
    let inner = text.trim().strip_prefix('[')?.strip_suffix(']')?;
    let values: Vec<f32> = inner
        .split(',')
        .filter_map(|part| part.trim().parse::<f32>().ok())
        .collect();
    (values.len() == embedding::DIMENSIONS).then_some(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(ratio: f64, finishes: i64, starts: i64, plays: i32) -> WorkSignal {
        WorkSignal {
            work_id: Uuid::nil(),
            max_ratio: Some(ratio),
            finishes,
            starts,
            last_seen: Utc::now(),
            play_count: Some(plays),
            duration_seconds: Some(3600.0),
            embedding: None,
            // Sem nota: os testes de comportamento medem comportamento, e o
            // sinal da R23 tem os seus, logo abaixo.
            nota: None,
        }
    }

    #[test]
    fn terminar_e_o_sinal_positivo() {
        assert!(affinity_of(&signal(0.95, 1, 1, 1)) >= 1.0);
    }

    #[test]
    fn largar_cedo_e_sinal_negativo() {
        assert!(affinity_of(&signal(0.08, 0, 1, 1)) < 0.0);
    }

    #[test]
    fn parar_no_meio_e_quase_neutro() {
        let value = affinity_of(&signal(0.35, 0, 1, 1));
        assert!(value > 0.0 && value < 0.3, "veio {value}");
    }

    #[test]
    fn reassistir_pesa_mais_que_assistir_uma_vez() {
        assert!(affinity_of(&signal(0.95, 1, 1, 3)) > affinity_of(&signal(0.95, 1, 1, 1)));
    }

    #[test]
    fn recencia_decai_pela_metade_na_meia_vida() {
        let now = Utc::now();
        let old = now - chrono::Duration::days(RECENCY_HALF_LIFE_DAYS as i64);
        let weight = recency_weight(old, now);
        assert!((weight - 0.5).abs() < 0.01, "veio {weight}");
    }

    #[test]
    fn vetor_do_pg_com_dimensao_errada_e_rejeitado() {
        assert!(parse_pg_vector("[1,2,3]").is_none());
        let full = format!("[{}]", vec!["0.1"; embedding::DIMENSIONS].join(","));
        assert!(parse_pg_vector(&full).is_some());
    }
}

#[cfg(test)]
mod tests_da_nota {
    use super::*;

    fn sinal(ratio: f64, finishes: i64, starts: i64, nota: Option<i32>) -> WorkSignal {
        WorkSignal {
            work_id: Uuid::nil(),
            max_ratio: Some(ratio),
            finishes,
            starts,
            last_seen: Utc::now(),
            play_count: Some(1),
            duration_seconds: Some(6000.0),
            embedding: None,
            nota,
        }
    }

    /// **A invariante da R23** (§39), e a razão de `PESO_DA_NOTA` valer 0,3.
    ///
    /// O §4.6 do `IDEIAS.md` deixou a regra: sinal fraco não manda no forte. Um
    /// peso maior que 0,3 quebraria isto em silêncio — a curadoria continuaria
    /// respondendo, só que errado, e ninguém notaria até o `/for-you` começar a
    /// recomendar o que a pessoa abandonou.
    #[test]
    fn a_nota_ajusta_mas_nunca_inverte() {
        // Cinco estrelas no que foi abandonado aos 8 minutos continua rejeição.
        let odiado_mas_nota_5 = affinity_of(&sinal(0.05, 0, 1, Some(5)));
        assert!(
            odiado_mas_nota_5 < 0.0,
            "cinco estrelas converteram um abandono em gosto: {odiado_mas_nota_5}"
        );

        // Uma estrela no que foi terminado continua contando como visto até o
        // fim — porque terminar é comportamento, e comportamento manda.
        let terminado_mas_nota_1 = affinity_of(&sinal(1.0, 1, 1, Some(1)));
        assert!(
            terminado_mas_nota_1 > 0.0,
            "uma estrela apagou um filme terminado: {terminado_mas_nota_1}"
        );
    }

    /// A nota move alguma coisa — senão ela seria enfeite, e a fase não teria
    /// acontecido. O que ela move é o **lugar dentro da faixa**, não o sinal.
    #[test]
    fn a_nota_move_dentro_da_faixa() {
        let sem = affinity_of(&sinal(1.0, 1, 1, None));
        let cinco = affinity_of(&sinal(1.0, 1, 1, Some(5)));
        let um = affinity_of(&sinal(1.0, 1, 1, Some(1)));
        assert!(cinco > sem, "cinco estrelas não somaram nada");
        assert!(um < sem, "uma estrela não subtraiu nada");
        assert!((cinco - um - 2.0 * PESO_DA_NOTA as f32).abs() < 1e-5);
    }

    /// Três é o meio e vale zero. "Achei ok" não é informação a favor nem
    /// contra, e tratá-lo como qualquer uma das duas seria inventar opinião.
    #[test]
    fn tres_estrelas_nao_dizem_nada() {
        assert_eq!(ajuste_da_nota(3), 0.0);
        assert_eq!(
            affinity_of(&sinal(1.0, 1, 1, Some(3))),
            affinity_of(&sinal(1.0, 1, 1, None))
        );
    }

    /// O ajuste é simétrico e limitado ao peso — se um dia alguém mexer na
    /// escala pra 1..10 sem mexer no divisor, este teste cai.
    #[test]
    fn o_ajuste_respeita_o_peso() {
        assert_eq!(ajuste_da_nota(1), -PESO_DA_NOTA);
        assert_eq!(ajuste_da_nota(5), PESO_DA_NOTA);
        // Nota fora da faixa é grampeada, não amplificada.
        assert_eq!(ajuste_da_nota(99), PESO_DA_NOTA);
        assert_eq!(ajuste_da_nota(-4), -PESO_DA_NOTA);
    }
}
