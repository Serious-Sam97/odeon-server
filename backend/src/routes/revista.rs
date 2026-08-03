//! R34 — o guia dinâmico: a revista da semana.
//!
//! ## O que o §30 entregou, e por que não era isto
//!
//! A R18 entregou um **índice**: cartões por diretor, elenco, compositor,
//! gênero, década e país, com uma ficha por pessoa cruzando com o seu histórico.
//! É simples e é útil — e é uma **enciclopédia**, não uma revista.
//!
//! O pedido é outro: *"um guia dinâmico, que muda de temática por dia ou semana,
//! faz eventos de um filme ou saga específica para incentivar as pessoas a
//! assistir, e usa o acervo para ensinar história do cinema. Útil, não
//! decorativo. Igual para todo mundo, para haver assunto em comum."*
//!
//! O índice **não morre**: ele vira a parte de consulta, atrás da revista.
//!
//! ## Igual pra todo mundo, e por isso sem tabela
//!
//! O tema e o evento são `md5(semana || eixo)` sobre o acervo, com a **mesma
//! semente semanal da locadora** (§36) — e portanto viram na mesma
//! segunda-feira. Duas visitas na mesma semana veem o mesmo tema; duas pessoas
//! veem o mesmo tema; e não há nada pra sincronizar nem pra expirar.
//!
//! Isso é o §2.4 do `IDEIAS.md` virando código: **o guia é coletivo de
//! propósito**, porque é o que dá assunto em comum. Os desafios (fase 8) são o
//! oposto — sorteados por pessoa.
//!
//! É a terceira vez que este truque paga: emissora (§25), vitrine (§36), guia.
//!
//! ## O evento
//!
//! Um filme ou uma saga em cartaz na semana. Participar é **terminar durante a
//! janela** — o mesmo sinal do §8f que a curadoria, a locadora, o mural e as
//! conquistas já usam. Uma sexta definição de "participou" seria uma sexta
//! chance de discordarem.

use std::collections::HashMap;

use axum::extract::State;
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::AppResult;
use crate::AppState;

/// Quantos filmes a capa mostra.
///
/// Oito é o que cabe numa fileira sem rolagem e o que alguém consegue considerar
/// de uma vez. Um tema com quarenta filmes não é um tema, é um filtro.
const NA_CAPA: i64 = 8;

/// Quantas obras um eixo precisa ter pra virar tema.
///
/// Abaixo disso a capa fica com três caixas e o ensaio não tem o que costurar —
/// e o §24 já dizia que meia tela é pior que nenhuma.
const MINIMO: i64 = 5;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FilmeDaCapa {
    pub id: Uuid,
    pub titulo: String,
    pub ano: Option<i32>,
    pub poster: Option<String>,
    pub diretor: Option<String>,
    /// Se **você** já terminou. A capa é igual pra todo mundo; esta coluna é a
    /// única coisa dela que é sua, e ela existe pra o evento saber onde você
    /// está sem uma segunda requisição.
    pub visto: bool,
}

#[derive(Debug, Serialize)]
pub struct Evento {
    /// `obra` | `saga`.
    pub tipo: &'static str,
    pub id: Uuid,
    pub titulo: String,
    pub poster: Option<String>,
    /// Quantas obras ele tem (1 num filme, N numa saga).
    pub obras: i64,
    /// Quantas **você** já terminou.
    pub suas: i64,
    /// Se você já fechou a participação desta semana.
    pub participou: bool,
    /// Quem já participou, pra ser assunto em comum.
    pub participantes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Revista {
    /// A segunda-feira desta edição, e quando ela vira.
    pub semana_de: chrono::NaiveDate,
    pub vira_em: chrono::DateTime<chrono::Utc>,
    /// `genero` | `decada` | `pais` | `diretor` | `saga`.
    pub eixo: &'static str,
    /// O tema em si — *"Terror"*, *"Anos 80"*, *"Coleção 007"*.
    pub tema: String,
    pub filmes: Vec<FilmeDaCapa>,
    /// O ensaio, quando existe.
    ///
    /// `None` quando não há chave do LLM **ou** quando o texto ainda não foi
    /// gerado. A tela omite a seção — não mostra "carregando" nem inventa
    /// prosa. É o §18 e o §24 na mesma decisão.
    pub ensaio: Option<String>,
    /// Qual modelo escreveu. **É o selo**, e ele aparece na tela do mesmo jeito
    /// que a curiosidade da Wikipédia leva o crédito (§32).
    pub ensaio_por: Option<String>,
    pub evento: Option<Evento>,
}

/// Os eixos que podem virar tema, na ordem em que o sorteio os considera.
///
/// Cinco, e todos saem de dado que já existe: gênero e década do M2, país do
/// §38, diretor do M1, saga da R32. **Nenhum eixo novo foi inventado pra esta
/// fase** — o guia usa o acervo, que é o pedido.
const EIXOS: &[&str] = &["genero", "decada", "pais", "diretor", "saga"];

/// A revista da semana.
pub async fn revista(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Revista>> {
    let (semana, vira_em) = crate::routes::locadora::semana_e_virada(chrono::Utc::now());
    let semente = semana.to_string();

    // O eixo, sorteado pela semana. Determinístico: todo mundo vê o mesmo.
    let eixo = EIXOS[(hash_da_semana(&semente) as usize) % EIXOS.len()];

    let (tema, filmes) = escolher(&state, &semente, eixo, user.id).await?;

    let (ensaio, ensaio_por) = ensaio_guardado(&state, semana, &tema).await;
    // Se não há ensaio e há redator, gera **em segundo plano**: a capa não
    // espera por uma chamada a um modelo. Na visita seguinte ele está lá.
    if ensaio.is_none() {
        if let Some(redator) = state.llm.clone() {
            let pool = state.pool.clone();
            let tema2 = tema.clone();
            let paises = paises_dos(&state, &filmes).await;
            let no_acervo = quantos_no_tema(&state, eixo, &tema).await;
            let fatos = fatos_pro_ensaio(eixo, &tema, &filmes, &paises, no_acervo);
            tokio::spawn(async move {
                gerar(pool, redator, semana, tema2, fatos).await;
            });
        }
    }

    let evento = evento_da_semana(&state, &semente, &filmes, user.id, semana).await?;

    Ok(Json(Revista {
        semana_de: semana,
        vira_em,
        eixo,
        tema,
        filmes,
        ensaio,
        ensaio_por,
        evento,
    }))
}

/// Um número estável a partir da semana. Não precisa ser criptográfico — precisa
/// ser o mesmo em toda máquina que rodar este código na mesma semana.
fn hash_da_semana(semente: &str) -> u32 {
    semente.bytes().fold(2166136261u32, |h, b| {
        (h ^ b as u32).wrapping_mul(16777619)
    })
}

/// O tema do eixo, e os filmes dele.
///
/// **Uma consulta por eixo**, e não uma genérica: os cinco eixos moram em
/// lugares diferentes do grafo (tag, coluna, crédito, coleção), e uma consulta
/// que servisse os cinco seria um `CASE` de quarenta linhas que ninguém lê.
async fn escolher(
    state: &AppState,
    semente: &str,
    eixo: &str,
    quem: Uuid,
) -> AppResult<(String, Vec<FilmeDaCapa>)> {
    // O `terminadas` é o §8f, a mesma definição de sempre.
    const VISTO: &str = r#"
        LEFT JOIN LATERAL (
            SELECT 1 AS ok FROM play_event pe
            WHERE pe.user_id = $1 AND pe.work_id = w.id
            GROUP BY pe.work_id
            HAVING count(*) FILTER (WHERE pe.event_type = 'finish') > 0
                OR max(pe.position_seconds / NULLIF(pe.duration_seconds, 0)) >= 0.92
        ) v ON true
    "#;
    const DIRETOR: &str = r#"
        LEFT JOIN LATERAL (
            SELECT p.name FROM credit c JOIN person p ON p.id = c.person_id
            WHERE c.work_id = w.id AND c.role = 'director'
            ORDER BY p.name LIMIT 1
        ) d ON true
    "#;

    // Cada ramo devolve (tema, sql dos filmes). O `$2` é sempre a semente e o
    // `$3` o tema escolhido.
    let (tema, sql) = match eixo {
        "genero" => {
            let tema: Option<(String,)> = sqlx::query_as(
                "SELECT t.value FROM tag t JOIN work_tag wt ON wt.tag_id = t.id
                 JOIN work w ON w.id = wt.work_id AND w.kind = 'movie' AND w.artwork ? 'poster'
                 WHERE t.namespace = 'genre'
                 GROUP BY t.value HAVING count(*) >= $1
                 ORDER BY md5($2 || t.value) LIMIT 1",
            )
            .bind(MINIMO)
            .bind(semente)
            .fetch_optional(&state.pool)
            .await?;
            (
                tema.map(|t| t.0),
                format!(
                    "SELECT w.id, w.title AS titulo, w.year AS ano,
                            w.artwork->>'poster' AS poster, d.name AS diretor,
                            v.ok IS NOT NULL AS visto
                     FROM work w
                     JOIN work_tag wt ON wt.work_id = w.id
                     JOIN tag t ON t.id = wt.tag_id AND t.namespace = 'genre' AND t.value = $3
                     {VISTO} {DIRETOR}
                     WHERE w.kind = 'movie' AND w.artwork ? 'poster'
                     ORDER BY md5($2 || w.id::text) LIMIT {NA_CAPA}"
                ),
            )
        }
        "decada" => {
            let tema: Option<(i32,)> = sqlx::query_as(
                "SELECT (w.year / 10) * 10 AS d FROM work w
                 WHERE w.kind = 'movie' AND w.year IS NOT NULL AND w.artwork ? 'poster'
                 GROUP BY d HAVING count(*) >= $1
                 ORDER BY md5($2 || d::text) LIMIT 1",
            )
            .bind(MINIMO)
            .bind(semente)
            .fetch_optional(&state.pool)
            .await?;
            (
                tema.map(|t| format!("Anos {}", t.0)),
                format!(
                    "SELECT w.id, w.title AS titulo, w.year AS ano,
                            w.artwork->>'poster' AS poster, d.name AS diretor,
                            v.ok IS NOT NULL AS visto
                     FROM work w {VISTO} {DIRETOR}
                     WHERE w.kind = 'movie' AND w.artwork ? 'poster'
                       AND (w.year / 10) * 10 = replace($3, 'Anos ', '')::int
                     ORDER BY md5($2 || w.id::text) LIMIT {NA_CAPA}"
                ),
            )
        }
        "pais" => {
            let tema: Option<(String,)> = sqlx::query_as(
                "SELECT t.value FROM tag t JOIN work_tag wt ON wt.tag_id = t.id
                 JOIN work w ON w.id = wt.work_id AND w.kind = 'movie' AND w.artwork ? 'poster'
                 WHERE t.namespace = 'country'
                 GROUP BY t.value HAVING count(*) >= $1
                 ORDER BY md5($2 || t.value) LIMIT 1",
            )
            .bind(MINIMO)
            .bind(semente)
            .fetch_optional(&state.pool)
            .await?;
            (
                tema.map(|t| t.0),
                format!(
                    "SELECT w.id, w.title AS titulo, w.year AS ano,
                            w.artwork->>'poster' AS poster, d.name AS diretor,
                            v.ok IS NOT NULL AS visto
                     FROM work w
                     JOIN work_tag wt ON wt.work_id = w.id
                     JOIN tag t ON t.id = wt.tag_id AND t.namespace = 'country' AND t.value = $3
                     {VISTO} {DIRETOR}
                     WHERE w.kind = 'movie' AND w.artwork ? 'poster'
                     ORDER BY md5($2 || w.id::text) LIMIT {NA_CAPA}"
                ),
            )
        }
        "diretor" => {
            let tema: Option<(String,)> = sqlx::query_as(
                "SELECT p.name FROM person p
                 JOIN credit c ON c.person_id = p.id AND c.role = 'director'
                 JOIN work w ON w.id = c.work_id AND w.kind = 'movie' AND w.artwork ? 'poster'
                 GROUP BY p.name HAVING count(DISTINCT w.id) >= $1
                 ORDER BY md5($2 || p.name) LIMIT 1",
            )
            .bind(MINIMO.min(3))
            .bind(semente)
            .fetch_optional(&state.pool)
            .await?;
            (
                tema.map(|t| t.0),
                format!(
                    "SELECT DISTINCT ON (w.id) w.id, w.title AS titulo, w.year AS ano,
                            w.artwork->>'poster' AS poster, $3::text AS diretor,
                            v.ok IS NOT NULL AS visto
                     FROM work w
                     JOIN credit c ON c.work_id = w.id AND c.role = 'director'
                     JOIN person p ON p.id = c.person_id AND p.name = $3
                     {VISTO}
                     WHERE w.kind = 'movie' AND w.artwork ? 'poster'
                     ORDER BY w.id, md5($2 || w.id::text) LIMIT {NA_CAPA}"
                ),
            )
        }
        _ => {
            let tema: Option<(String,)> = sqlx::query_as(
                "SELECT c.title FROM collection c
                 JOIN collection_item ci ON ci.collection_id = c.id
                 JOIN work w ON w.id = ci.work_id AND w.artwork ? 'poster'
                 WHERE c.kind = 'franchise'
                 GROUP BY c.title HAVING count(*) >= $1
                 ORDER BY md5($2 || c.title) LIMIT 1",
            )
            .bind(MINIMO.min(3))
            .bind(semente)
            .fetch_optional(&state.pool)
            .await?;
            (
                tema.map(|t| t.0),
                format!(
                    "SELECT w.id, w.title AS titulo, w.year AS ano,
                            w.artwork->>'poster' AS poster, d.name AS diretor,
                            v.ok IS NOT NULL AS visto
                     FROM work w
                     JOIN collection_item ci ON ci.work_id = w.id
                     JOIN collection col ON col.id = ci.collection_id
                        AND col.kind = 'franchise' AND col.title = $3
                     {VISTO} {DIRETOR}
                     WHERE w.artwork ? 'poster'
                     ORDER BY w.year NULLS LAST LIMIT {NA_CAPA}"
                ),
            )
        }
    };

    // Eixo sem nenhum candidato: capa vazia, e a tela some com ela (§24). Não
    // acontece neste acervo, e acontecer não é motivo pra 500.
    let Some(tema) = tema else {
        return Ok((String::new(), Vec::new()));
    };

    let filmes = sqlx::query_as::<_, FilmeDaCapa>(&sql)
        .bind(quem)
        .bind(semente)
        .bind(&tema)
        .fetch_all(&state.pool)
        .await?;

    Ok((tema, filmes))
}

/// O evento da semana: uma saga, quando o tema tem uma; senão um filme da capa.
///
/// **Saga primeiro** porque foi o pedido — *"eventos de um filme ou saga
/// específica"* — e porque uma saga dá o que fazer a semana inteira, enquanto um
/// filme dá duas horas.
async fn evento_da_semana(
    state: &AppState,
    semente: &str,
    filmes: &[FilmeDaCapa],
    quem: Uuid,
    semana: chrono::NaiveDate,
) -> AppResult<Option<Evento>> {
    if filmes.is_empty() {
        return Ok(None);
    }

    // Uma saga que contenha algum dos filmes da capa.
    let saga: Option<(Uuid, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT c.id, c.title, c.artwork->>'poster', count(*) AS obras
         FROM collection c
         JOIN collection_item ci ON ci.collection_id = c.id
         WHERE c.kind = 'franchise'
           AND EXISTS (SELECT 1 FROM collection_item x
                        WHERE x.collection_id = c.id AND x.work_id = ANY($1))
         GROUP BY c.id, c.title, c.artwork
         HAVING count(*) >= 2
         ORDER BY md5($2 || c.id::text) LIMIT 1",
    )
    .bind(filmes.iter().map(|f| f.id).collect::<Vec<_>>())
    .bind(semente)
    .fetch_optional(&state.pool)
    .await?;

    let (tipo, id, titulo, poster, obras) = match saga {
        Some((id, titulo, poster, obras)) => ("saga", id, titulo, poster, obras),
        None => {
            // O primeiro da capa: a ordem já é o sorteio da semana.
            let f = &filmes[0];
            ("obra", f.id, f.titulo.clone(), f.poster.clone(), 1)
        }
    };

    // Quantas obras do evento você já terminou, e quem já participou.
    let (suas, participou, participantes): (i64, bool, Vec<String>) = sqlx::query_as(
        r#"
        WITH obras AS (
            SELECT CASE WHEN $2 = 'saga'
                        THEN ci.work_id ELSE $3::uuid END AS work_id
            FROM (SELECT 1) _
            LEFT JOIN collection_item ci
              ON $2 = 'saga' AND ci.collection_id = $3
        ),
        minhas AS (
            SELECT count(DISTINCT pe.work_id) AS n
            FROM play_event pe JOIN obras o ON o.work_id = pe.work_id
            WHERE pe.user_id = $1
            GROUP BY pe.work_id
            HAVING count(*) FILTER (WHERE pe.event_type = 'finish') > 0
                OR max(pe.position_seconds / NULLIF(pe.duration_seconds, 0)) >= 0.92
        )
        SELECT COALESCE((SELECT count(*) FROM minhas), 0),
               EXISTS (SELECT 1 FROM evento_participacao
                        WHERE user_id = $1 AND semana = $4),
               COALESCE(ARRAY(
                   SELECT u.display_name FROM evento_participacao ep
                   JOIN app_user u ON u.id = ep.user_id
                   WHERE ep.semana = $4 ORDER BY ep.em
               ), '{}')
        "#,
    )
    .bind(quem)
    .bind(tipo)
    .bind(id)
    .bind(semana)
    .fetch_one(&state.pool)
    .await?;

    Ok(Some(Evento {
        tipo,
        id,
        titulo,
        poster,
        obras,
        suas,
        participou,
        participantes,
    }))
}

// ------------------------------------------------------------------ o ensaio

async fn ensaio_guardado(
    state: &AppState,
    semana: chrono::NaiveDate,
    tema: &str,
) -> (Option<String>, Option<String>) {
    let r: Option<(String, String)> =
        sqlx::query_as("SELECT texto, modelo FROM ensaio WHERE semana = $1 AND tema = $2")
            .bind(semana)
            .bind(tema)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);
    match r {
        Some((t, m)) => (Some(t), Some(m)),
        None => (None, None),
    }
}

/// **Os fatos, prontos.** É esta string que vai pro modelo — e é ela que
/// garante a ressalva da decisão 2.3: a lista de filmes, anos, diretores e
/// países sai do banco, e o modelo só redige em volta.
///
/// Ele nunca é perguntado *"quais filmes de terror existem?"*, porque a resposta
/// a essa pergunta é exatamente o que ele inventaria com confiança.
///
/// ## Por que os fatos cresceram
///
/// A primeira versão mandava só título, ano e diretor — e o texto que voltou era
/// honesto e **inútil**: *"Romance é o tema da semana. Temos alguns filmes que
/// se encaixam nesse gênero (…) Esses filmes estão disponíveis em nossa locadora
/// para alugar."* Nenhum fato inventado, e nada aprendido.
///
/// O `IDEIAS.md` §3.1 pede o contrário: *"usa o acervo para ensinar história do
/// cinema. Útil, não decorativo."* Um modelo só escreve sobre o que recebe, e
/// com três colunas não havia o que dizer além de listar.
///
/// Então entram o **país**, o **intervalo de anos**, os **agrupamentos** (mesmo
/// diretor, mesma década, mesmo país) e o **tamanho do tema no acervo**. Nada
/// disso é opinião: são todos `SELECT`s. O que o modelo faz continua sendo só a
/// costura.
fn fatos_pro_ensaio(
    eixo: &str,
    tema: &str,
    filmes: &[FilmeDaCapa],
    paises: &HashMap<Uuid, String>,
    no_acervo: i64,
) -> String {
    let lista = filmes
        .iter()
        .map(|f| {
            let ano = f.ano.map(|a| a.to_string()).unwrap_or_else(|| "s/ano".into());
            let pais = paises.get(&f.id).map(|p| format!(", {p}")).unwrap_or_default();
            match &f.diretor {
                Some(d) => format!("- {} ({}), dirigido por {}{}", f.titulo, ano, d, pais),
                None => format!("- {} ({}){}", f.titulo, ano, pais),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let qual = match eixo {
        "genero" => "gênero",
        "decada" => "década",
        "pais" => "país",
        "diretor" => "diretor",
        _ => "saga",
    };

    // As ligações entre os filmes. **São o material do ensaio**: sem elas o
    // modelo só tem uma lista, e uma lista só rende uma lista.
    let mut ligacoes: Vec<String> = Vec::new();

    let anos: Vec<i32> = filmes.iter().filter_map(|f| f.ano).collect();
    if let (Some(min), Some(max)) = (anos.iter().min(), anos.iter().max()) {
        if max > min {
            ligacoes.push(format!("os oito vão de {min} a {max}"));
        }
    }

    let mut por_decada: HashMap<i32, usize> = HashMap::new();
    for a in &anos {
        *por_decada.entry((a / 10) * 10).or_insert(0) += 1;
    }
    let mut decadas: Vec<(i32, usize)> = por_decada.into_iter().filter(|(_, n)| *n > 1).collect();
    decadas.sort();
    for (d, n) in decadas {
        ligacoes.push(format!("{n} são dos anos {d}"));
    }

    let mut por_pais: HashMap<&str, usize> = HashMap::new();
    for p in filmes.iter().filter_map(|f| paises.get(&f.id)) {
        *por_pais.entry(p.as_str()).or_insert(0) += 1;
    }
    let mut lugares: Vec<(&str, usize)> = por_pais.into_iter().collect();
    lugares.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    if let Some((p, n)) = lugares.first() {
        if lugares.len() > 1 {
            ligacoes.push(format!("{n} vêm de: {p}; os outros vêm de {} lugares diferentes", lugares.len() - 1));
        }
    }

    let vistos = filmes.iter().filter(|f| f.visto).count();
    if vistos > 0 {
        ligacoes.push(format!("quem lê já viu {vistos} deles"));
    }
    if no_acervo > filmes.len() as i64 {
        ligacoes.push(format!(
            "a locadora tem {no_acervo} filmes deste tema ao todo; estes oito são os da vitrine desta semana"
        ));
    }

    format!(
        "Tema da semana ({qual}): {tema}\n\n         Os filmes desta locadora que entram no tema:\n{lista}\n\n         O que estes filmes têm em comum, medido no acervo:\n- {}",
        ligacoes.join("\n- ")
    )
}

/// O país de cada filme da capa, numa consulta.
///
/// Fora das cinco consultas de eixo de propósito: acrescentar mais um `LATERAL`
/// nas cinco seria repetir a mesma junção cinco vezes pra um dado que só o
/// ensaio usa.
async fn paises_dos(state: &AppState, filmes: &[FilmeDaCapa]) -> HashMap<Uuid, String> {
    if filmes.is_empty() {
        return HashMap::new();
    }
    sqlx::query_as::<_, (Uuid, String)>(
        "SELECT DISTINCT ON (wt.work_id) wt.work_id, t.value
         FROM work_tag wt JOIN tag t ON t.id = wt.tag_id AND t.namespace = 'country'
         WHERE wt.work_id = ANY($1)
         ORDER BY wt.work_id, t.value",
    )
    .bind(filmes.iter().map(|f| f.id).collect::<Vec<_>>())
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .collect()
}

/// Quantos filmes do tema o acervo tem ao todo.
///
/// É o denominador — o mesmo cuidado da placa da estante que diz "3 de 113"
/// (§14). Sem ele, o ensaio deixaria a pessoa achar que o tema tem oito filmes.
async fn quantos_no_tema(state: &AppState, eixo: &str, tema: &str) -> i64 {
    let sql = match eixo {
        "genero" | "pais" => {
            let ns = if eixo == "genero" { "genre" } else { "country" };
            format!(
                "SELECT count(DISTINCT w.id) FROM work w
                 JOIN work_tag wt ON wt.work_id = w.id
                 JOIN tag t ON t.id = wt.tag_id AND t.namespace = '{ns}' AND t.value = $1
                 WHERE w.kind = 'movie'"
            )
        }
        "decada" => "SELECT count(*) FROM work w WHERE w.kind = 'movie'
                     AND (w.year / 10) * 10 = replace($1, 'Anos ', '')::int"
            .into(),
        "diretor" => "SELECT count(DISTINCT c.work_id) FROM credit c
                      JOIN person p ON p.id = c.person_id AND p.name = $1
                      WHERE c.role = 'director'"
            .into(),
        _ => "SELECT count(*) FROM collection_item ci
              JOIN collection c ON c.id = ci.collection_id AND c.title = $1"
            .into(),
    };
    sqlx::query_scalar::<_, i64>(&sql)
        .bind(tema)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0)
}

/// Gera e guarda. Roda fora da requisição.
async fn gerar(
    pool: sqlx::PgPool,
    redator: std::sync::Arc<crate::llm::Llm>,
    semana: chrono::NaiveDate,
    tema: String,
    fatos: String,
) {
    match redator.costurar(crate::llm::SISTEMA, &fatos).await {
        Ok(texto) => {
            let _ = sqlx::query(
                "INSERT INTO ensaio (semana, tema, texto, modelo) VALUES ($1, $2, $3, $4)
                 ON CONFLICT DO NOTHING",
            )
            .bind(semana)
            .bind(&tema)
            .bind(&texto)
            .bind(&redator.modelo)
            .execute(&pool)
            .await;
            tracing::info!(%tema, "ensaio da semana escrito");
        }
        // Falhar não deixa marca: a capa continua sem ensaio e a próxima visita
        // tenta de novo. Um ensaio pela metade seria pior que nenhum.
        Err(e) => tracing::warn!(erro = %e, %tema, "não deu pra escrever o ensaio"),
    }
}

// ------------------------------------------------------- a participação

/// Registra a participação de quem acabou de terminar uma obra, **se** ela for a
/// do evento desta semana.
///
/// Chamada do mesmo lugar que grava o progresso, depois do commit. Barata: uma
/// consulta que quase sempre não encontra nada.
pub async fn talvez_participou(state: &AppState, quem: Uuid, work_id: Uuid) {
    let (semana, _) = crate::routes::locadora::semana_e_virada(chrono::Utc::now());
    let semente = semana.to_string();

    // Reconstrói o evento da semana — é determinístico, então não precisa estar
    // guardado em lugar nenhum. Se a obra terminada não for a do evento (nem
    // parte da saga dele), o `INSERT` simplesmente não acontece.
    let Ok((_, filmes)) = escolher(
        state,
        &semente,
        EIXOS[(hash_da_semana(&semente) as usize) % EIXOS.len()],
        quem,
    )
    .await
    else {
        return;
    };

    let Ok(Some(evento)) = evento_da_semana(state, &semente, &filmes, quem, semana).await else {
        return;
    };

    let no_evento: bool = match evento.tipo {
        "saga" => sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM collection_item WHERE collection_id = $1 AND work_id = $2)",
        )
        .bind(evento.id)
        .bind(work_id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(false),
        _ => evento.id == work_id,
    };

    if !no_evento {
        return;
    }

    let feito = sqlx::query(
        "INSERT INTO evento_participacao (user_id, semana, work_id)
         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(quem)
    .bind(semana)
    .bind(work_id)
    .execute(&state.pool)
    .await;

    if matches!(feito, Ok(r) if r.rows_affected() > 0) {
        tracing::info!(%quem, %evento.titulo, "participou do evento da semana");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **O tema é estável na semana e muda na seguinte.** Se ele deixar de ser
    /// estável, duas visitas no mesmo dia mostram capas diferentes; se deixar de
    /// mudar, a revista para de ser semanal — e as duas quebram em silêncio.
    #[test]
    fn o_eixo_e_estavel_na_semana_e_vira_na_seguinte() {
        let a = hash_da_semana("2026-08-03");
        assert_eq!(a, hash_da_semana("2026-08-03"), "o mesmo dia deu dois hashes");

        // Ao longo de um ano, os cinco eixos têm que aparecer — um sorteio que
        // caísse sempre no mesmo faria a revista ter um tema só.
        let mut vistos = std::collections::HashSet::new();
        for semana in 0..52 {
            let d = chrono::NaiveDate::from_ymd_opt(2026, 1, 5).unwrap()
                + chrono::Duration::weeks(semana);
            vistos.insert(EIXOS[(hash_da_semana(&d.to_string()) as usize) % EIXOS.len()]);
        }
        assert_eq!(vistos.len(), EIXOS.len(), "eixos que nunca saem: {vistos:?}");
    }

    /// O mínimo de obras existe pra a capa não abrir com três caixas. Se ele
    /// cair pra 1, qualquer etiqueta com um filme vira "tema da semana".
    #[test]
    fn o_tema_precisa_de_acervo() {
        assert!(MINIMO >= 5);
        assert!(NA_CAPA >= MINIMO);
    }

    /// Os fatos que vão pro modelo **são a lista do banco**, e nada além. É a
    /// ressalva da decisão 2.3, e é o que impede o ensaio de citar filme que
    /// esta locadora não tem.
    #[test]
    fn os_fatos_do_ensaio_sao_do_banco() {
        let filmes = vec![FilmeDaCapa {
            id: Uuid::nil(),
            titulo: "Suspiria".into(),
            ano: Some(1977),
            poster: None,
            diretor: Some("Dario Argento".into()),
            visto: false,
        }];
        let mut paises = HashMap::new();
        paises.insert(Uuid::nil(), "Itália".to_string());
        let fatos = fatos_pro_ensaio("genero", "Terror", &filmes, &paises, 113);
        assert!(fatos.contains("Suspiria (1977), dirigido por Dario Argento, Itália"));
        assert!(fatos.contains("Tema da semana (gênero): Terror"));
        // **O denominador vai junto** — é o mesmo cuidado da placa que diz
        // "3 de 113" (§14): sem ele o ensaio deixaria a pessoa achar que o tema
        // tem oito filmes.
        assert!(fatos.contains("113 filmes deste tema"));
        // E o sistema proíbe acrescentar o que não está na lista.
        assert!(crate::llm::SISTEMA.contains("NÃO acrescente nenhum filme"));
        // E proíbe o enchimento que a primeira versão produziu.
        assert!(crate::llm::SISTEMA.contains("disponíveis para alugar"));
        assert!(crate::llm::SISTEMA.contains("é o tema da semana"));
    }
}
