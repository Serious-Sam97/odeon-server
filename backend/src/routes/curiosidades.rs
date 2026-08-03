//! Curiosidades sobre uma obra — e a regra que decide quais existem.
//!
//! O pedido foi "curiosidades pra entreter". A primeira pergunta é de onde elas
//! vêm, e a resposta descartou três fontes antes de sobrar uma:
//!
//! * **O TMDB não tem trivia.** Nem o AniList. Conferido: o que eles devolvem é
//!   ficha — título, sinopse, ano, gênero, elenco.
//! * **A Wikipédia tem**, mas é texto solto, com licença e uma dependência de
//!   rede nova pra cada obra.
//! * **Um LLM gerando** é a tentação óbvia e a pior de todas: ele inventa com
//!   confiança, e o §18 já fixou a regra da casa — sem sufixo reconhecível o
//!   idioma fica `None` em vez de chutar "inglês". Curiosidade inventada sobre
//!   um filme que a pessoa ama é pior que curiosidade nenhuma.
//!
//! Sobrou a melhor: **o próprio grafo**. Curiosidade sobre o filme qualquer site
//! tem; curiosidade sobre *o seu acervo* e *o seu histórico com aquele filme* só
//! este servidor pode dar. É o mesmo raciocínio da R18, uma camada abaixo.
//!
//! Tudo aqui é derivado e conferível. Nada é escrito que o banco não sustente.
//!
//! ### A regra que mais corta código: só nasce se for notável
//!
//! Uma curiosidade que vale pra toda obra não é curiosidade. "Este filme tem
//! elenco" é verdade e é lixo. Então cada consulta carrega um limiar, e quando
//! ele não é atingido a curiosidade **não existe** — não vira "nenhuma
//! informação disponível". É a regra do §24 (linha limpa some) aplicada a
//! entretenimento: um painel que sempre diz alguma coisa ensina a não ser lido.
//!
//! Medido em *007: Cassino Royale*: gênero não dispara (Ação tem 200 filmes no
//! acervo, não é raridade nenhuma) e duração não dispara (145 min, com 56
//! filmes mais longos). Disparam trilha, diretor e reencontro de elenco. É o
//! comportamento desejado.

use axum::extract::{Path, State};
use axum::Json;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::AppResult;
use crate::AppState;

pub use crate::trivia::Curiosidade;

/// Obras identificadas contam; o resto do acervo não.
///
/// Mesma regra que a R15 (§26) aplicou à curadoria e a R18 aos eixos: uma
/// curiosidade que compara contra 4.410 arquivos sem identificação compara
/// contra ruído.
const IDENTIFICADA: &str = "w2.match_state IN ('auto', 'confirmed')";

/// `w2` é **outra obra de verdade** — nem a mesma linha, nem uma segunda cópia
/// do mesmo filme.
///
/// A segunda metade parece paranoia e não é. A primeira versão desta rota
/// dizia, na ficha de *1408*: *"De Mikael Håfström você também tem 1408"* — e
/// estava tecnicamente certa, porque são **duas linhas em `work`** com o mesmo
/// `{"tmdb": "3021"}`, uma `auto` e outra `confirmed`. O acervo tem 6 grupos
/// assim: o mesmo filme, dois arquivos.
///
/// `media_file.path` é UNIQUE, mas nada impede duas obras apontando pro mesmo
/// título — e o §8b até favorece isso, já que o matcher nunca sobrescreve o que
/// um humano confirmou. Então a comparação certa é pelo id do provider, que é
/// o que responde "é o mesmo filme?".
const OUTRA_OBRA: &str = "w2.id <> w.id
      AND NOT (w.external_ids <> '{}'::jsonb AND w2.external_ids = w.external_ids)";

/// A trivia externa, com cache no banco (0019).
///
/// **Nunca derruba a rota.** Wikidata fora do ar, rede caída, filme sem entrada:
/// tudo devolve lista vazia e as curiosidades do acervo continuam aparecendo. É
/// a mesma postura do §17 com a arte do programa — o que falta some, o que
/// existe fica.
///
/// A validade é longa (30 dias) porque estes fatos praticamente não mudam: um
/// filme não ganha um Oscar novo em 2007. E a linha vazia **é guardada**, senão
/// todo filme sem entrada no Wikidata seria reconsultado a cada abertura da
/// ficha.
async fn trivia_do_filme(state: &AppState, id: Uuid) -> Vec<Curiosidade> {
    const VALIDADE_DIAS: i64 = 30;

    let cache: Option<(serde_json::Value, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as("SELECT itens, buscado_em FROM work_trivia WHERE work_id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten();

    if let Some((itens, quando)) = cache {
        if (chrono::Utc::now() - quando).num_days() < VALIDADE_DIAS {
            return serde_json::from_value(itens).unwrap_or_default();
        }
    }

    // Só filme, e só com id do TMDB: é ele que casa com o Wikidata (P4947).
    // Episódio não tem entrada lá, e buscar por título seria a adivinhação que
    // este projeto recusa desde o §8b.
    let alvo: Option<(String, String)> = sqlx::query_as(
        "SELECT external_ids->>'tmdb', title FROM work
         WHERE id = $1 AND kind = 'movie' AND external_ids ? 'tmdb'",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();

    let Some((tmdb, titulo)) = alvo else {
        return Vec::new();
    };

    let itens = match crate::trivia::buscar(&state.providers.http, &tmdb, &titulo).await {
        Ok(v) => v,
        Err(e) => {
            // Falha de rede NÃO é gravada: gravar o vazio aqui esconderia a
            // trivia do filme por 30 dias por causa de um segundo ruim.
            tracing::debug!(error = %e, tmdb, "trivia externa indisponível");
            return Vec::new();
        }
    };

    let _ = sqlx::query(
        "INSERT INTO work_trivia (work_id, itens, buscado_em)
         VALUES ($1, $2, now())
         ON CONFLICT (work_id) DO UPDATE SET itens = EXCLUDED.itens, buscado_em = now()",
    )
    .bind(id)
    .bind(serde_json::to_value(&itens).unwrap_or_else(|_| serde_json::json!([])))
    .execute(&state.pool)
    .await;

    itens
}

pub async fn curiosidades(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<Curiosidade>>> {
    let pool = &state.pool;
    let mut achadas: Vec<Curiosidade> = Vec::new();

    // --- 0. o filme em si, de fora ---------------------------------------
    //
    // Vem PRIMEIRO porque é sobre a obra, e o resto desta rota é sobre o seu
    // acervo. Quem abre uma ficha quer saber do filme; o que ele tem a ver com
    // a sua estante é a pergunta seguinte, não a primeira.
    achadas.extend(trivia_do_filme(&state, id).await);

    // --- 1. o diretor -----------------------------------------------------
    //
    // Limiar 1: se ele não tem outra obra aqui, dizer "você tem 1 filme deste
    // diretor" é dizer o óbvio de volta.
    let dir: Option<(String, i64, Option<String>)> = sqlx::query_as(&format!(
        "SELECT p.name,
                count(DISTINCT c2.work_id),
                min(w2.title)
         FROM work w
         JOIN credit c ON c.work_id = w.id AND c.role = 'director'
         JOIN person p ON p.id = c.person_id
         JOIN credit c2 ON c2.person_id = p.id AND c2.role = 'director'
         JOIN work w2 ON w2.id = c2.work_id AND {IDENTIFICADA} AND {OUTRA_OBRA}
         WHERE w.id = $1
         GROUP BY p.name
         ORDER BY 2 DESC
         LIMIT 1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;

    if let Some((nome, outras, exemplo)) = dir {
        achadas.push(Curiosidade::nova(
            "direcao",
            match (outras, exemplo) {
                // Com uma só dá pra nomear, e nomear é mais interessante que contar.
                (1, Some(titulo)) => format!("De {nome} você também tem {titulo}."),
                (n, _) => format!("De {nome} você tem outras {n} obras no acervo."),
            },
        ));
    }

    // --- 2. a trilha ------------------------------------------------------
    //
    // Limiar 3: compositor prolífico é o que rende a frase. Com um ou dois,
    // isto vira ficha técnica repetida.
    let trilha: Option<(String, i64)> = sqlx::query_as(&format!(
        "SELECT p.name, count(DISTINCT c2.work_id)
         FROM work w
         JOIN credit c ON c.work_id = w.id AND c.role = 'composer'
         JOIN person p ON p.id = c.person_id
         JOIN credit c2 ON c2.person_id = p.id AND c2.role = 'composer'
         JOIN work w2 ON w2.id = c2.work_id AND {IDENTIFICADA} AND {OUTRA_OBRA}
         WHERE w.id = $1
         GROUP BY p.name
         HAVING count(DISTINCT c2.work_id) >= 3
         ORDER BY 2 DESC
         LIMIT 1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;

    if let Some((nome, outras)) = trilha {
        achadas.push(Curiosidade::nova("trilha", format!("A trilha é de {nome}, que assina outras {outras} obras do seu acervo.")));
    }

    // --- 3. reencontro de elenco -----------------------------------------
    //
    // Dois nomes desta obra que também dividem outra. É a curiosidade que mais
    // parece curiosidade — e ela é impossível sem o grafo de `credit`
    // deduplicado por `provider_key` (§8h): sem aquilo, "Judi Dench" seria uma
    // linha por filme e nunca cruzaria com nada.
    let reencontro: Option<(String, String, String, Option<i32>)> = sqlx::query_as(&format!(
        "SELECT p1.name, p2.name, w2.title, w2.year
         FROM work w
         JOIN credit c1 ON c1.work_id = w.id AND c1.role = 'actor'
         JOIN credit c2 ON c2.work_id = w.id
                       AND c2.person_id > c1.person_id AND c2.role = 'actor'
         JOIN credit d1 ON d1.person_id = c1.person_id AND d1.role = 'actor'
                       AND d1.work_id <> w.id
         JOIN credit d2 ON d2.person_id = c2.person_id AND d2.role = 'actor'
                       AND d2.work_id = d1.work_id
         JOIN work w2 ON w2.id = d1.work_id AND w2.kind = 'movie'
                     AND {IDENTIFICADA} AND {OUTRA_OBRA}
         JOIN person p1 ON p1.id = c1.person_id
         JOIN person p2 ON p2.id = c2.person_id
         WHERE w.id = $1
         ORDER BY c1.position NULLS LAST, c2.position NULLS LAST
         LIMIT 1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;

    if let Some((a, b, titulo, ano)) = reencontro {
        let quando = ano.map(|y| format!(" ({y})")).unwrap_or_default();
        achadas.push(Curiosidade::nova("elenco", format!("{a} e {b} também dividem a tela em {titulo}{quando}.")));
    }

    // --- 4. raridade de gênero -------------------------------------------
    //
    // Limiar 6: "um dos 200 filmes de ação" não é curiosidade, é o catálogo.
    // O interessante é o canto vazio da estante.
    let raro: Option<(String, i64)> = sqlx::query_as(&format!(
        "SELECT t.value, x.filmes
         FROM work_tag wt
         JOIN tag t ON t.id = wt.tag_id AND t.namespace = 'genre'
         JOIN LATERAL (
             SELECT count(*) AS filmes
             FROM work_tag wt2
             JOIN work w2 ON w2.id = wt2.work_id AND w2.kind = 'movie' AND {IDENTIFICADA}
             WHERE wt2.tag_id = t.id
         ) x ON true
         WHERE wt.work_id = $1 AND x.filmes <= 6
         ORDER BY x.filmes
         LIMIT 1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;

    if let Some((genero, filmes)) = raro {
        let g = genero.to_lowercase();
        achadas.push(Curiosidade::nova(
            "raridade",
            if filmes <= 1 {
                format!("É o único filme de {g} do seu acervo.")
            } else {
                format!("Você só tem {filmes} filmes de {g} — este é um deles.")
            },
        ));
    }

    // --- 5. os extremos da duração ---------------------------------------
    //
    // Só o pódio, dos dois lados. O 40º filme mais longo não é notícia.
    let dur: Option<(f64, i64, i64)> = sqlx::query(
        "SELECT f.duration_seconds,
                (SELECT count(*) FROM media_file f2 JOIN work w2 ON w2.id = f2.work_id
                 WHERE w2.kind = 'movie' AND f2.duration_seconds > f.duration_seconds),
                (SELECT count(*) FROM media_file f2 JOIN work w2 ON w2.id = f2.work_id
                 WHERE w2.kind = 'movie' AND f2.duration_seconds < f.duration_seconds
                   AND f2.duration_seconds > 600)
         FROM media_file f
         WHERE f.work_id = $1 AND f.duration_seconds IS NOT NULL
         ORDER BY f.size_bytes DESC
         LIMIT 1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .map(|row| {
        use sqlx::Row;
        (
            row.get::<f64, _>(0),
            row.get::<i64, _>(1),
            row.get::<i64, _>(2),
        )
    });

    if let Some((segundos, mais_longos, mais_curtos)) = dur {
        let minutos = (segundos / 60.0).round() as i64;
        if mais_longos == 0 {
            achadas.push(Curiosidade::nova("duracao", format!("É o filme mais longo do seu acervo: {minutos} minutos.")));
        } else if mais_longos < 3 {
            achadas.push(Curiosidade::nova(
                "duracao",
                format!(
                    "Com {minutos} minutos, é o {}º filme mais longo do seu acervo.",
                    mais_longos + 1
                ),
            ));
        } else if mais_curtos == 0 && segundos > 600.0 {
            achadas.push(Curiosidade::nova("duracao", format!("É o filme mais curto do seu acervo: {minutos} minutos.")));
        }
    }

    // --- 6. o ano ---------------------------------------------------------
    //
    // Duas perguntas na mesma consulta: quantos filmes daquele ano existem
    // aqui, e se algum é mais antigo que este.
    let ano: Option<(i32, i64, i64)> = sqlx::query(
        "SELECT w.year,
                (SELECT count(*) FROM work w2
                 WHERE w2.kind = 'movie' AND w2.year = w.year
                   AND w2.match_state IN ('auto', 'confirmed')),
                (SELECT count(*) FROM work w2
                 WHERE w2.kind = 'movie' AND w2.year < w.year
                   AND w2.match_state IN ('auto', 'confirmed'))
         FROM work w
         WHERE w.id = $1 AND w.year IS NOT NULL AND w.kind = 'movie'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .map(|row| {
        use sqlx::Row;
        (
            row.get::<i32, _>(0),
            row.get::<i64, _>(1),
            row.get::<i64, _>(2),
        )
    });

    if let Some((y, do_ano, mais_antigos)) = ano {
        if mais_antigos == 0 {
            achadas.push(Curiosidade::nova("ano", format!("É o filme mais antigo do seu acervo — {y}.")));
        } else if do_ano == 1 {
            achadas.push(Curiosidade::nova("ano", format!("É o único filme de {y} que você tem.")));
        }
    }

    // --- 7. você ----------------------------------------------------------
    //
    // A única que nenhum site do mundo pode escrever, porque ela não é sobre o
    // filme — é sobre você e ele. Vem por último de propósito: a leitura começa
    // pela obra e termina em quem está lendo.
    let seu: Option<(i32, bool, f64, Option<f64>)> = sqlx::query(
        "SELECT ps.play_count, ps.finished, ps.position_seconds, ps.duration_seconds
         FROM playback_state ps
         WHERE ps.work_id = $1 AND ps.user_id = $2",
    )
    .bind(id)
    .bind(user.id())
    .fetch_optional(pool)
    .await?
    .map(|row| {
        use sqlx::Row;
        (
            row.get::<i32, _>(0),
            row.get::<bool, _>(1),
            row.get::<f64, _>(2),
            row.get::<Option<f64>, _>(3),
        )
    });

    if let Some((vezes, terminou, posicao, duracao)) = seu {
        // Reassistir é o sinal mais forte do M5 (§8f) e merece ser dito.
        if vezes >= 2 {
            achadas.push(Curiosidade::nova("voce", format!("Você já viu este filme inteiro {vezes} vezes.")));
        } else if terminou {
            achadas.push(Curiosidade::nova("voce", "Você já viu este filme inteiro.".to_string()));
        } else if let Some(total) = duracao.filter(|d| *d > 0.0) {
            let faltam = ((total - posicao) / 60.0).round() as i64;
            if posicao > 60.0 && faltam > 0 {
                achadas.push(Curiosidade::nova("voce", format!("Você parou faltando {faltam} minutos.")));
            }
        }
    }

    Ok(Json(achadas))
}
