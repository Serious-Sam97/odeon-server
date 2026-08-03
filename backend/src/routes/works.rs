use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser};
use crate::error::{AppError, AppResult};
use crate::models::{
    MediaFileSummary, ProgressReport, Work, WorkDetail, WorkListItem,
};
use crate::routes::graph;
use crate::AppState;

/// A obra é considerada assistida a partir daqui. 92% deixa os créditos de fora.
const FINISHED_RATIO: f64 = 0.92;

/// Projeção compartilhada por biblioteca, "continuar assistindo" e coleção.
/// Fica num const só pra as três não divergirem.
const WORK_COLUMNS: &str = r#"
    w.id, w.kind, w.title, w.year, w.season_number, w.episode_number,
    w.match_state, w.match_confidence, w.dominant_color,
    w.artwork->>'poster' AS poster,
    w.artwork->>'backdrop' AS backdrop,
    w.artwork->>'still' AS still,
    f.id AS media_file_id, f.duration_seconds, f.width, f.height,
    f.video_codec, f.audio_codec, f.container, f.size_bytes,
    ps.position_seconds, ps.finished,
    tg.tags
"#;

/// Melhor arquivo da obra, nome da série (subindo episódio→temporada→série) e
/// as tags achatadas em `namespace:valor`.
const WORK_JOINS: &str = r#"
LEFT JOIN LATERAL (
    SELECT m.* FROM media_file m
    WHERE m.work_id = w.id AND m.status = 'probed'
    ORDER BY m.size_bytes DESC LIMIT 1
) f ON true
-- Só temporada/série contam como "obra-mãe". Sem este filtro, uma playlist ou
-- uma ordem de exibição apareceria no card como se fosse o nome da série.
LEFT JOIN LATERAL (
    SELECT COALESCE(series.title, season.title) AS series_title
    FROM collection_item ci
    JOIN collection season ON season.id = ci.collection_id
    LEFT JOIN collection series ON series.id = season.parent_id
    WHERE ci.work_id = w.id
      AND season.kind IN ('season', 'series')
    LIMIT 1
) s ON true
LEFT JOIN LATERAL (
    SELECT array_agg(t.namespace || ':' || t.value ORDER BY t.namespace, t.value) AS tags
    FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
    WHERE wt.work_id = w.id
) tg ON true
LEFT JOIN playback_state ps ON ps.work_id = w.id AND ps.user_id = $1
"#;

#[derive(Debug, Deserialize)]
pub struct ListParams {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    /// Lista separada por vírgula em `namespace:valor` — `genre:Ação,format:anime`
    #[serde(default)]
    pub tags: Option<String>,
    /// `all` exige todas as tags; `any` aceita qualquer uma.
    #[serde(default = "default_tag_mode")]
    pub tag_mode: String,
    #[serde(default)]
    pub year_from: Option<i32>,
    #[serde(default)]
    pub year_to: Option<i32>,
    #[serde(default)]
    pub min_minutes: Option<i32>,
    #[serde(default)]
    pub max_minutes: Option<i32>,
    /// Inclui a subárvore inteira da coleção, não só os filhos diretos.
    #[serde(default)]
    pub collection: Option<Uuid>,
    #[serde(default)]
    pub state: Option<String>,
    /// "tudo do Villeneuve" — a consulta que justifica a tabela `credit`.
    #[serde(default)]
    pub person: Option<Uuid>,
    #[serde(default)]
    pub person_role: Option<String>,
    #[serde(default = "default_sort")]
    pub sort: String,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    120
}
/// `featured` e não `title`: ver o comentário em `library_order_by`. Para
/// `/api/works` isto não muda nada — `order_by` não conhece "featured" e cai no
/// braço padrão, que já era a ordenação por título.
fn default_sort() -> String {
    "featured".to_string()
}
fn default_tag_mode() -> String {
    "all".to_string()
}

/// Whitelist. O `sort` vem da query string e é concatenado no SQL — só pode
/// virar SQL o que estiver aqui.
fn order_by(sort: &str) -> &'static str {
    match sort {
        "year" => "w.year DESC NULLS LAST, w.title",
        "added" => "w.created_at DESC",
        "recent" => "w.updated_at DESC",
        "duration" => "f.duration_seconds DESC NULLS LAST",
        "random" => "random()",
        // "title" e qualquer coisa não reconhecida
        _ => "COALESCE(s.series_title, w.title), w.season_number NULLS FIRST, \
              w.episode_number NULLS FIRST",
    }
}

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<ListParams>,
) -> AppResult<Json<Vec<WorkListItem>>> {
    let tags: Option<Vec<String>> = params.tags.as_ref().and_then(|raw| {
        let parsed: Vec<String> = raw
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        (!parsed.is_empty()).then_some(parsed)
    });

    let sql = format!(
        r#"
        SELECT {WORK_COLUMNS}, s.series_title
        FROM work w
        {WORK_JOINS}
        WHERE ($2::text IS NULL
               OR w.title ILIKE '%' || $2 || '%'
               OR s.series_title ILIKE '%' || $2 || '%'
               OR w.search_vector @@ websearch_to_tsquery('simple', $2))
          AND ($3::text IS NULL OR w.kind = $3)
          AND ($6::text[] IS NULL OR (
                SELECT count(DISTINCT t.namespace || ':' || t.value)
                FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
                WHERE wt.work_id = w.id
                  AND (t.namespace || ':' || t.value) = ANY($6)
              ) >= CASE WHEN $7 THEN cardinality($6) ELSE 1 END)
          AND ($8::int  IS NULL OR w.year >= $8)
          AND ($9::int  IS NULL OR w.year <= $9)
          AND ($10::int IS NULL OR f.duration_seconds >= $10 * 60)
          AND ($11::int IS NULL OR f.duration_seconds <= $11 * 60)
          AND ($12::uuid IS NULL OR EXISTS (
                SELECT 1 FROM collection_item ci
                WHERE ci.work_id = w.id
                  AND ci.collection_id IN (
                    WITH RECURSIVE subtree AS (
                        SELECT id FROM collection WHERE id = $12
                        UNION ALL
                        SELECT c.id FROM collection c JOIN subtree ON c.parent_id = subtree.id
                    ) SELECT id FROM subtree
                  )
              ))
          AND ($13::text IS NULL OR w.match_state = $13)
          AND ($14::uuid IS NULL OR EXISTS (
                SELECT 1 FROM credit c
                WHERE c.work_id = w.id AND c.person_id = $14
                  AND ($15::text IS NULL OR c.role = $15)
              ))
        ORDER BY {}
        LIMIT $4 OFFSET $5
        "#,
        order_by(&params.sort)
    );

    let items = sqlx::query_as::<_, WorkListItem>(&sql)
        .bind(user.id())
        .bind(params.q.filter(|s| !s.trim().is_empty()))
        .bind(params.kind)
        .bind(params.limit.clamp(1, 500))
        .bind(params.offset.max(0))
        .bind(tags)
        .bind(params.tag_mode != "any")
        .bind(params.year_from)
        .bind(params.year_to)
        .bind(params.min_minutes)
        .bind(params.max_minutes)
        .bind(params.collection)
        .bind(params.state)
        .bind(params.person)
        .bind(params.person_role)
        .fetch_all(&state.pool)
        .await?;

    Ok(Json(items))
}

pub async fn continue_watching(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Vec<WorkListItem>>> {
    let sql = format!(
        r#"
        SELECT {WORK_COLUMNS}, s.series_title
        FROM work w
        {WORK_JOINS}
        WHERE ps.user_id = $1 AND ps.position_seconds > 30 AND NOT ps.finished
        ORDER BY ps.updated_at DESC
        LIMIT 30
        "#
    );

    let items = sqlx::query_as::<_, WorkListItem>(&sql)
        .bind(user.id())
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(items))
}

pub async fn detail(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<WorkDetail>> {
    let work = sqlx::query_as::<_, Work>("SELECT * FROM work WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let files = sqlx::query_as::<_, MediaFileSummary>(
        "SELECT id, path, filename, size_bytes, container, duration_seconds, bitrate,
                video_codec, width, height, frame_rate, audio_codec, audio_channels,
                subtitle_langs, status
         FROM media_file WHERE work_id = $1 ORDER BY size_bytes DESC",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    let state_row: Option<(f64, bool)> = sqlx::query_as(
        "SELECT position_seconds, finished FROM playback_state WHERE user_id = $1 AND work_id = $2",
    )
    .bind(user.id())
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;

    let (position_seconds, finished) = state_row.unwrap_or((0.0, false));

    Ok(Json(WorkDetail {
        work,
        files,
        position_seconds,
        finished,
        tags: graph::tags_of(&state, id).await?,
        credits: crate::routes::people::credits_of(&state, id).await?,
        collections: graph::collections_of(&state, id).await?,
        relations: graph::relations_of(&state, id).await?,
    }))
}

pub async fn progress(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<ProgressReport>,
) -> AppResult<Json<Value>> {
    if body.position_seconds < 0.0 || !body.position_seconds.is_finite() {
        return Err(AppError::BadRequest("position_seconds inválido".into()));
    }

    let mut tx = state.pool.begin().await?;

    // 1. O log cru. Fonte da verdade, nunca sobrescrito.
    sqlx::query(
        "INSERT INTO play_event
            (user_id, work_id, media_file_id, event_type, position_seconds, duration_seconds, client)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(user.id())
    .bind(id)
    .bind(body.media_file_id)
    .bind(&body.event_type)
    .bind(body.position_seconds)
    .bind(body.duration_seconds)
    .bind(body.client.as_deref().unwrap_or("unknown"))
    .execute(&mut *tx)
    .await?;

    // 2. O cache derivado, pro "continuar assistindo" ser um SELECT barato.
    let finished = match body.duration_seconds {
        Some(d) if d > 0.0 => body.position_seconds / d >= FINISHED_RATIO,
        _ => false,
    };

    // `finished` é ACUMULATIVO, não o estado do instante.
    //
    // Ele era `finished = EXCLUDED.finished`, e isso apagava o terminado: quem
    // reabrisse no minuto 30 um filme já visto voltava a constar como não
    // visto. Medido neste acervo — 16 linhas, ZERO com `finished`, e ainda
    // assim *Cassino Royale* com `play_count = 1`, que só sobe na transição
    // falso→verdadeiro. O contador era o fóssil de um `finished` que existiu.
    //
    // "Terminado" responde *"eu já terminei isto alguma vez?"*, e a resposta
    // não deixa de ser sim porque a pessoa começou de novo. Reassistir tem
    // coluna própria desde o M0, que é o `play_count` logo abaixo.
    //
    // O contador precisou mudar junto: ele dependia de `NOT
    // playback_state.finished`, então com o `finished` grudando ele congelaria
    // em 1 para sempre — e levaria junto o bônus de reassistir do M5 (§8f),
    // que é o sinal positivo mais forte que existe. Agora quem decide é a
    // POSIÇÃO guardada: só conta como uma exibição nova quem chega ao fim
    // vindo de um ponto que ainda não estava no fim. Isso também evita somar
    // um a cada heartbeat depois dos 92%.
    sqlx::query(
        "INSERT INTO playback_state
            (user_id, work_id, position_seconds, duration_seconds, finished, play_count, updated_at)
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $5 THEN 1 ELSE 0 END, now())
         ON CONFLICT (user_id, work_id) DO UPDATE SET
            position_seconds = EXCLUDED.position_seconds,
            duration_seconds = COALESCE(EXCLUDED.duration_seconds, playback_state.duration_seconds),
            finished         = playback_state.finished OR EXCLUDED.finished,
            play_count       = playback_state.play_count
                               + CASE WHEN EXCLUDED.finished
                                       AND COALESCE(
                                             playback_state.position_seconds
                                             / NULLIF(playback_state.duration_seconds, 0), 0
                                           ) < $6
                                      THEN 1 ELSE 0 END,
            updated_at       = now()",
    )
    .bind(user.id())
    .bind(id)
    .bind(body.position_seconds)
    .bind(body.duration_seconds)
    .bind(finished)
    .bind(FINISHED_RATIO)
    .execute(&mut *tx)
    .await?;

    // 3. **E a fita anda** (R30).
    //
    // Uma fita é um objeto, e um objeto fica onde a última pessoa o deixou —
    // tenha ela alugado ou não, devolvido ou não. Levantar no meio e sair já
    // deixa a fita zoada pro próximo, que é o que a anotação original pediu:
    // *"saber que estado deixou a fita para o próximo uso"*.
    //
    // Note que isto **não** substitui o `playback_state` acima, e a distinção é
    // a fase inteira: aquele é a sua memória, este é o objeto. Rebobinar mexe
    // num e não no outro, e é por isso que rebobinar deixou de ser destrutivo.
    //
    // O `WHERE` do ano faz a coisa toda virar no-op para DVD, sem um `if` aqui:
    // disco não rebobina, ele lembra onde parou (§35). O número é servido pela
    // locadora, e é o mesmo que a tela usa pra desenhar a lombada — se os dois
    // divergissem, uma caixa desenhada como VHS não teria fita.
    sqlx::query(
        "INSERT INTO fita (work_id, posicao_segundos, duracao_segundos, deixada_por, deixada_em)
         SELECT w.id, $2, $3, $4, now()
         FROM work w
         WHERE w.id = $1 AND w.year IS NOT NULL AND w.year <= $5
         ON CONFLICT (work_id) DO UPDATE SET
            posicao_segundos = EXCLUDED.posicao_segundos,
            -- A duração só cresce pra um valor conhecido: um heartbeat sem ela
            -- não deve apagar o comprimento da fita.
            duracao_segundos = COALESCE(EXCLUDED.duracao_segundos, fita.duracao_segundos),
            deixada_por      = EXCLUDED.deixada_por,
            deixada_em       = now()",
    )
    .bind(id)
    .bind(body.position_seconds.max(0.0))
    .bind(body.duration_seconds.filter(|d| *d > 0.0))
    .bind(user.id())
    .bind(crate::routes::locadora::ULTIMO_ANO_VHS)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    // 4. **As conquistas.** Depois do commit, e de propósito: elas leem os
    //    fatos que a transação acabou de gravar, e ler de dentro dela veria o
    //    estado pela metade. Falhar aqui não desfaz o progresso — conquista não
    //    é caminho crítico de reprodução (`conquistas::avaliar` engole o erro).
    // 4. **O evento da semana vem ANTES das conquistas** (R34), e a ordem é
    //    um defeito que a verificação encontrou: avaliando primeiro, a
    //    conquista "Esteve lá" só abriria na ação seguinte — a pessoa termina o
    //    filme do evento e a medalha aparece amanhã, quando ela clicar em outra
    //    coisa. Registrar a participação primeiro é o que faz a recompensa
    //    chegar no mesmo gesto que a mereceu.
    if finished {
        crate::routes::revista::talvez_participou(&state, user.id(), id).await;
    }

    // 5. Os desafios da janela, conferidos antes das conquistas pela mesma
    //    razão que o evento: a conquista "Topou" tem que chegar no mesmo gesto
    //    que fechou o desafio, não na ação seguinte.
    crate::desafios::conferir(&state.pool, user.id()).await;

    // 6. E aí as conquistas, que já enxergam o desafio e a participação de agora.
    let novas = crate::conquistas::avaliar(&state.pool, user.id()).await;

    // 7. Avisa os outros aparelhos. Quem emitiu ignora o próprio eco pelo
    //    device_id — sem isso o player brigaria com a própria atualização.
    crate::events::publish(
        &state.events,
        crate::events::AppEvent::Progress {
            work_id: id,
            position_seconds: body.position_seconds,
            duration_seconds: body.duration_seconds,
            finished,
            device_id: body.device_id.unwrap_or_else(|| "desconhecido".to_string()),
        },
    );

    // As conquistas novas voltam na resposta do heartbeat, e não por evento:
    // quem terminou o filme é quem tem que ver a medalha, e ele já está
    // esperando esta resposta. Um evento no barramento avisaria os outros
    // aparelhos da pessoa — e ninguém quer um pop-up de conquista no celular
    // enquanto assiste na sala.
    Ok(Json(json!({
        "ok": true,
        "finished": finished,
        "conquistas": novas.iter().map(|q| json!({
            "chave": q.chave, "nome": q.nome, "camada": q.camada, "pontos": q.pontos,
        })).collect::<Vec<_>>(),
    })))
}

// ------------------------------------------------------- biblioteca agrupada

/// Uma entrada da biblioteca: ou uma **série inteira**, ou uma obra avulsa.
///
/// A tela mostrava 14.657 episódios como cartões iguais — isso é listagem de
/// arquivo, não biblioteca. As séries já existiam no grafo desde o M1
/// (`collection(series)` → `collection(season)`); faltava a tela usá-las.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct LibraryEntry {
    /// Id da série (quando `is_series`) ou da própria obra.
    pub id: Uuid,
    pub is_series: bool,
    pub title: String,
    pub year: Option<i32>,
    pub poster: Option<String>,
    pub dominant_color: Option<String>,
    /// Quantas obras a entrada reúne. 1 para avulsa.
    pub work_count: i64,
    /// Temporadas distintas. 0 para avulsa.
    pub season_count: i64,
    /// Quantas o usuário terminou — é o que alimenta a barra do cartão.
    pub finished_count: i64,
    /// Só faz sentido para avulsa: é o que permite tocar direto do cartão.
    pub media_file_id: Option<Uuid>,
    pub duration_seconds: Option<f64>,
    /// Só para avulsa: a série não tem um arquivo só.
    pub height: Option<i32>,
    pub size_bytes: Option<i64>,
    pub kind: Option<String>,
    pub match_state: Option<String>,
    pub position_seconds: Option<f64>,
    /// Repetido em toda linha pelo `count(*) OVER ()` — é o preço de saber o
    /// total sem uma segunda ida ao banco.
    pub total: i64,
}

/// Ordenação da biblioteca. Mesma regra do `order_by`: whitelist, porque isto
/// é concatenado no SQL.
///
/// O padrão é `featured` e não `title` por um motivo medido: ordenar por título
/// põe `001 - Draw My Life As a Gamer` e companhia na frente, porque número vem
/// antes de letra — e as 8.616 obras identificadas ficam enterradas depois de
/// quatro mil arquivos sem match. "Tem arte primeiro" resolve sem esconder
/// nada: o que não foi identificado continua ali, no fim.
fn library_order_by(sort: &str) -> &'static str {
    match sort {
        "title" => "title",
        "year" => "year DESC NULLS LAST, title",
        "added" => "created_at DESC",
        "recent" => "updated_at DESC",
        "duration" => "duration_seconds DESC NULLS LAST, title",
        "random" => "random()",
        _ => "(poster IS NULL), title",
    }
}

pub async fn library(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<ListParams>,
) -> AppResult<Json<Vec<LibraryEntry>>> {
    let tags: Option<Vec<String>> = params.tags.as_ref().and_then(|raw| {
        let parsed: Vec<String> = raw
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        (!parsed.is_empty()).then_some(parsed)
    });

    // O `grupo` sobe episódio → temporada → série. Quando a temporada não tem
    // série mãe, ela mesma é o grupo — é o mesmo COALESCE que o `series_title`
    // já fazia, só que agora carregando o id junto.
    let sql = format!(
        r#"
        WITH filtrado AS (
            SELECT
                w.id, w.kind, w.title, w.year, w.match_state, w.dominant_color,
                w.created_at, w.updated_at,
                w.artwork->>'poster' AS poster,
                f.id AS media_file_id, f.duration_seconds, f.height, f.size_bytes,
                ps.position_seconds, COALESCE(ps.finished, false) AS finished,
                g.grupo_id, g.grupo_title, g.grupo_year, g.grupo_poster, g.season_id
            FROM work w
            {WORK_JOINS}
            LEFT JOIN LATERAL (
                SELECT COALESCE(series.id, season.id)       AS grupo_id,
                       COALESCE(series.title, season.title) AS grupo_title,
                       COALESCE(series.year, season.year)   AS grupo_year,
                       COALESCE(series.artwork->>'poster',
                                season.artwork->>'poster')  AS grupo_poster,
                       season.id                            AS season_id
                FROM collection_item ci
                JOIN collection season ON season.id = ci.collection_id
                LEFT JOIN collection series ON series.id = season.parent_id
                WHERE ci.work_id = w.id AND season.kind IN ('season', 'series')
                LIMIT 1
            ) g ON true
            WHERE ($2::text IS NULL
                   OR w.title ILIKE '%' || $2 || '%'
                   OR s.series_title ILIKE '%' || $2 || '%'
                   OR w.search_vector @@ websearch_to_tsquery('simple', $2))
              AND ($3::text IS NULL OR w.kind = $3)
              -- Ignorada é obra que alguém DESCARTOU de propósito. Ela só
              -- reaparece quando o filtro pede por ela explicitamente.
              AND (w.match_state <> 'ignored' OR $13 = 'ignored')
              AND ($6::text[] IS NULL OR (
                    SELECT count(DISTINCT t.namespace || ':' || t.value)
                    FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
                    WHERE wt.work_id = w.id
                      AND (t.namespace || ':' || t.value) = ANY($6)
                  ) >= CASE WHEN $7 THEN cardinality($6) ELSE 1 END)
              AND ($8::int  IS NULL OR w.year >= $8)
              AND ($9::int  IS NULL OR w.year <= $9)
              AND ($10::int IS NULL OR f.duration_seconds >= $10 * 60)
              AND ($11::int IS NULL OR f.duration_seconds <= $11 * 60)
              AND ($12::uuid IS NULL OR EXISTS (
                    SELECT 1 FROM collection_item ci
                    WHERE ci.work_id = w.id
                      AND ci.collection_id IN (
                        WITH RECURSIVE subtree AS (
                            SELECT id FROM collection WHERE id = $12
                            UNION ALL
                            SELECT c.id FROM collection c JOIN subtree ON c.parent_id = subtree.id
                        ) SELECT id FROM subtree
                      )
                  ))
              AND ($13::text IS NULL OR w.match_state = $13)
              AND ($14::uuid IS NULL OR EXISTS (
                    SELECT 1 FROM credit c
                    WHERE c.work_id = w.id AND c.person_id = $14
                      AND ($15::text IS NULL OR c.role = $15)
                  ))
        ),
        series AS (
            SELECT
                grupo_id AS id,
                true     AS is_series,
                grupo_title AS title,
                -- O ano da série; sem ele, o do episódio mais antigo.
                COALESCE(grupo_year, min(year))       AS year,
                -- O pôster da série; sem ele, o de qualquer episódio que tenha.
                COALESCE(grupo_poster, min(poster))   AS poster,
                min(dominant_color)                   AS dominant_color,
                count(*)                              AS work_count,
                count(DISTINCT season_id)             AS season_count,
                count(*) FILTER (WHERE finished)      AS finished_count,
                NULL::uuid    AS media_file_id,
                sum(duration_seconds)                 AS duration_seconds,
                NULL::int     AS height,
                NULL::bigint  AS size_bytes,
                NULL::text    AS kind,
                NULL::text    AS match_state,
                NULL::float8  AS position_seconds,
                min(created_at) AS created_at,
                max(updated_at) AS updated_at
            FROM filtrado
            WHERE grupo_id IS NOT NULL
            GROUP BY grupo_id, grupo_title, grupo_year, grupo_poster
        ),
        avulsas AS (
            SELECT
                id, false AS is_series, title, year, poster, dominant_color,
                1::bigint AS work_count,
                0::bigint AS season_count,
                (CASE WHEN finished THEN 1 ELSE 0 END)::bigint AS finished_count,
                media_file_id, duration_seconds, height, size_bytes,
                kind, match_state, position_seconds,
                created_at, updated_at
            FROM filtrado
            WHERE grupo_id IS NULL
        ),
        tudo AS (
            SELECT * FROM series
            UNION ALL
            SELECT * FROM avulsas
        )
        SELECT id, is_series, title, year, poster, dominant_color,
               work_count, season_count, finished_count,
               media_file_id, duration_seconds, height, size_bytes,
               kind, match_state, position_seconds,
               count(*) OVER () AS total
        FROM tudo
        ORDER BY {}
        LIMIT $4 OFFSET $5
        "#,
        library_order_by(&params.sort)
    );

    let items = sqlx::query_as::<_, LibraryEntry>(&sql)
        .bind(user.id())
        .bind(params.q.filter(|s| !s.trim().is_empty()))
        .bind(params.kind)
        .bind(params.limit.clamp(1, 500))
        .bind(params.offset.max(0))
        .bind(tags)
        .bind(params.tag_mode != "any")
        .bind(params.year_from)
        .bind(params.year_to)
        .bind(params.min_minutes)
        .bind(params.max_minutes)
        .bind(params.collection)
        .bind(params.state)
        .bind(params.person)
        .bind(params.person_role)
        .fetch_all(&state.pool)
        .await?;

    Ok(Json(items))
}

// ------------------------------------------------------- gerenciar a obra
//
// O que a gaveta de gerenciamento precisa e o resto da API não oferecia:
// saber se dá pra apagar, apagar, e sumir com uma obra sem apagar nada.

/// Escrita testada por escrita de verdade — arquivo criado e apagado.
///
/// Não dá pra deduzir isso da configuração: `:ro` no compose, permissão do
/// usuário do container e filesystem de rede montado somente-leitura são três
/// motivos diferentes para o mesmo "não dá", e só a tentativa distingue. É a
/// mesma política do `hwaccel`, que testa o encoder em vez de acreditar no
/// `ffmpeg -encoders`.
async fn gravavel(dir: &std::path::Path) -> bool {
    let alvo = dir.join(".odeon-teste-escrita");
    match tokio::fs::write(&alvo, b"").await {
        Ok(()) => {
            let _ = tokio::fs::remove_file(&alvo).await;
            true
        }
        Err(_) => false,
    }
}

/// O que dá pra fazer com os arquivos deste servidor.
/// O layout das montagens.
///
/// **Virou rota de administrador na R26** (§42). Ela respondia 200 pra conta
/// comum com `/media`, `/media2`, `gravavel: true` e `pode_apagar: true` — ou
/// seja, o mapa do disco e a confirmação de que ele é gravável. É exatamente o
/// terceiro dos três compromissos que o §6.5 listou (a montagem gravável da
/// R10), vazando por uma rota em vez de por uma montagem.
pub async fn storage(
    State(state): State<AppState>,
    crate::auth::AdminUser(_): crate::auth::AdminUser,
) -> Json<Value> {
    let mut raizes = Vec::new();
    for raiz in &state.config.media_roots {
        raizes.push(json!({
            "path": raiz.to_string_lossy(),
            "existe": tokio::fs::metadata(raiz).await.is_ok(),
            "gravavel": gravavel(raiz).await,
        }));
    }
    let alguma = raizes.iter().any(|r| r["gravavel"] == json!(true));
    Json(json!({
        "pode_apagar": alguma,
        "motivo": if alguma { Value::Null } else {
            json!("as pastas de mídia estão montadas somente-leitura — \
                   no docker-compose.yml o volume termina em `:ro`")
        },
        "raizes": raizes,
    }))
}

#[derive(Debug, Deserialize)]
pub struct DeleteParams {
    /// Apaga também os arquivos do disco. Sem isto só o catálogo é limpo — e
    /// o catálogo volta na próxima varredura, porque o arquivo continua lá.
    #[serde(default)]
    pub apagar_arquivos: bool,
}

/// Apaga a obra. Com `apagar_arquivos`, apaga os arquivos antes.
///
/// A ordem importa e é deliberada: **disco primeiro, banco depois**. Se um
/// arquivo se recusar a sumir, nada é removido do catálogo e o erro sobe com o
/// caminho — assim o banco nunca afirma que algo foi apagado quando não foi. O
/// contrário deixaria arquivos invisíveis ocupando disco.
///
/// `media_file.work_id` é `ON DELETE SET NULL`, não cascade: apagar só a obra
/// deixaria o arquivo pendurado no banco sem dono. Os dois vão juntos.
pub async fn delete_work(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<Uuid>,
    Query(params): Query<DeleteParams>,
) -> AppResult<Json<Value>> {
    let titulo: String = sqlx::query_scalar("SELECT title FROM work WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let arquivos: Vec<(String, i64)> =
        sqlx::query_as("SELECT path, size_bytes FROM media_file WHERE work_id = $1")
            .bind(id)
            .fetch_all(&state.pool)
            .await?;

    let mut apagados = 0usize;
    let mut bytes = 0i64;

    if params.apagar_arquivos {
        for (caminho, tamanho) in &arquivos {
            match tokio::fs::remove_file(caminho).await {
                Ok(()) => {
                    apagados += 1;
                    bytes += tamanho;
                }
                // Já não existia: o disco concorda com o que queremos.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(AppError::BadRequest(format!(
                        "não apaguei `{caminho}`: {e}. Nada foi removido do catálogo."
                    )));
                }
            }
        }
    }

    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM media_file WHERE work_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM work WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    tracing::info!(obra = %titulo, arquivos = apagados, "obra apagada");

    Ok(Json(json!({
        "ok": true,
        "titulo": titulo,
        "arquivos_apagados": apagados,
        "bytes_liberados": bytes,
        "aviso": (!params.apagar_arquivos && !arquivos.is_empty())
            .then_some("os arquivos continuam no disco — a próxima varredura traz a obra de volta"),
    })))
}

#[derive(Debug, Deserialize)]
pub struct IgnoreBody {
    pub reason: Option<String>,
}

/// Some com a obra sem apagar nada: `match_state = 'ignored'`.
///
/// É a alternativa honesta a "remover do catálogo": o arquivo fica, a obra sai
/// da biblioteca, e a varredura não a traz de volta — porque a linha continua
/// lá, marcada.
///
/// Separado do `bulk_state` de propósito. Aquele recusa obras `confirmed` — em
/// lote, desfazer uma identificação boa por engano é caro. Aqui é uma obra,
/// escolhida a dedo, com o botão embaixo do nome dela.
pub async fn ignore_work(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<IgnoreBody>,
) -> AppResult<Json<Value>> {
    let motivos = match &body.reason {
        Some(r) if !r.trim().is_empty() => json!([r.trim()]),
        _ => json!(["ignorada manualmente"]),
    };

    let feito = sqlx::query(
        "UPDATE work SET match_state = 'ignored', match_reasons = $2, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(&motivos)
    .execute(&state.pool)
    .await?;

    if feito.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}

/// A saúde do servidor num número por linha.
///
/// `/api/diagnostico`, e não `/api/health`: aquele já existe e é o *liveness*
/// — responde sem autenticação, serve pra saber se o processo está de pé. São
/// perguntas diferentes, e o axum não deixa duas rotas dividirem o mesmo
/// método (foi um pânico no boot que avisou).
///
/// Nasceu de uma varredura manual que achou coisas que ninguém tinha como ver
/// pela interface: cinco arquivos que o `ffprobe` recusa (um deles um PDF com
/// extensão `.mp4`) e um guia de TV que ia secar em 38 horas. Nada disso
/// aparecia em lugar nenhum — e o que não aparece, ninguém conserta.
///
/// Só leitura, e barata: são cinco `count` com índice.
pub async fn diagnostico(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
) -> AppResult<Json<Value>> {
    let (com_erro, sumidos, total): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'error'),
                count(*) FILTER (WHERE status = 'missing'),
                count(*)
         FROM media_file",
    )
    .fetch_one(&state.pool)
    .await?;

    let arquivos_ruins: Vec<(String, String)> = sqlx::query_as(
        "SELECT filename, status FROM media_file
         WHERE status IN ('error', 'missing') ORDER BY filename LIMIT 20",
    )
    .fetch_all(&state.pool)
    .await?;

    let (revisar, sem_id, ignoradas): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE match_state = 'needs_review'),
                count(*) FILTER (WHERE match_state = 'unmatched'),
                count(*) FILTER (WHERE match_state = 'ignored')
         FROM work",
    )
    .fetch_one(&state.pool)
    .await?;

    let sprites: i64 = sqlx::query_scalar("SELECT count(*) FROM scrub_sprite")
        .fetch_one(&state.pool)
        .await?;

    // Horas de grade ainda por vir. É o número que o vigia da grade usa pra
    // decidir se reimporta — mostrá-lo aqui é mostrar o vigia trabalhando.
    // `extract(epoch ...)` devolve NUMERIC no Postgres 14+, não FLOAT8 — o
    // cast explícito é o que separa isto de um 500 em runtime.
    let guia: Option<f64> = sqlx::query_scalar(
        "SELECT (greatest(0, extract(epoch FROM (max(starts_at) - now())) / 3600))::float8
         FROM programme",
    )
    .fetch_one(&state.pool)
    .await?;

    let fontes: Vec<(String, Option<String>, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT name, last_error, last_import_at FROM channel_source ORDER BY name")
            .fetch_all(&state.pool)
            .await?;

    Ok(Json(json!({
        "arquivos": {
            "total": total,
            "com_erro": com_erro,
            "sumidos": sumidos,
            "amostra": arquivos_ruins.iter()
                .map(|(f, s)| json!({ "arquivo": f, "estado": s }))
                .collect::<Vec<_>>(),
        },
        "identificacao": {
            "revisar": revisar, "sem_identificacao": sem_id, "ignoradas": ignoradas,
        },
        "sprites": { "prontos": sprites, "de": total },
        "ao_vivo": {
            "horas_de_grade": guia.map(|h| h.round() as i64),
            "fontes": fontes.iter().map(|(n, e, q)| json!({
                "nome": n, "erro": e, "ultimo_import": q,
            })).collect::<Vec<_>>(),
        },
    })))
}
