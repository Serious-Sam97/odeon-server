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

/// Abaixo daqui a pessoa não está retomando: ela **recomeçou**.
///
/// 5% é o mesmo corte que o mural já usa pra separar "largou" de "abriu e
/// fechou" (`feed.rs`), e a mesma ideia do menu, onde posição zero "não é
/// continuar de onde parou — é o começo" (`menu.rs`). Reaproveitar o número
/// evita que três telas discordem sobre o que é estar no início.
///
/// **Por que tão baixo, e não qualquer coisa abaixo dos 92%:** o `finished`
/// grudento foi uma decisão tomada de propósito (o comentário no `progress`
/// conta a história), e a objeção registrada era nominal — "quem reabrisse no
/// minuto 30 um filme já visto voltava a constar como não visto". Minuto 30 de
/// um filme de duas horas é 0,25, bem acima daqui: retomar no meio continua
/// sem apagar nada. O que passa a apagar é só o gesto inequívoco de pôr do
/// começo.
const RESTART_RATIO: f64 = 0.05;

/// Onde a posição caiu, em fração da duração. `None` quando a duração é
/// desconhecida — sem ela não há fração, e chutar seria decidir no escuro.
fn razao(position_seconds: f64, duration_seconds: Option<f64>) -> Option<f64> {
    match duration_seconds {
        Some(d) if d > 0.0 => Some(position_seconds / d),
        _ => None,
    }
}

/// Chegou ao fim?
fn terminou(position_seconds: f64, duration_seconds: Option<f64>) -> bool {
    razao(position_seconds, duration_seconds).is_some_and(|r| r >= FINISHED_RATIO)
}

/// Voltou pro começo?
///
/// Sem duração devolve `false`, e isso é deliberado: o `finished` só liga com
/// duração conhecida, então desligá-lo sem ela seria apagar por um critério que
/// nunca serviu pra ligar.
fn recomecou(position_seconds: f64, duration_seconds: Option<f64>) -> bool {
    razao(position_seconds, duration_seconds).is_some_and(|r| r < RESTART_RATIO)
}

/// Projeção compartilhada por biblioteca, "continuar assistindo" e coleção.
/// Fica num const só pra as três não divergirem.
const WORK_COLUMNS: &str = r#"
    w.id, w.kind, w.title, w.year, w.season_number, w.episode_number,
    w.match_state, w.match_confidence, w.dominant_color,
    -- R63: a sinopse do episódio na **lista**, e não só na ficha.
    --
    -- É a linha que responde "qual era esse mesmo?" sem abrir. Sem ela o
    -- cliente teria de pedir 9 fichas pra desenhar 9 linhas, que não é uma
    -- tela. Já existe em 7.628 dos 14.844 episódios; quem não tem manda nulo e
    -- a linha não é desenhada.
    w.overview,
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
      AND season.kind IN ('season', 'series', 'channel')
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
    /// **A negação** (R65). Mesma forma do `tags`, sentido oposto: a obra sai
    /// se tiver **qualquer uma** delas.
    ///
    /// ## Por que ela precisou existir
    ///
    /// O celular separou filmes e séries em abas, e a API não sabia dizer
    /// "tudo menos série". As duas saídas que existiam eram ruins e o cliente
    /// mediu as duas:
    ///
    /// | tentativa | o que acontece |
    /// |---|---|
    /// | fixar `tags=format:filme` | some com as ~2.182 entradas sem formato (§10) |
    /// | cortar na tela | o `total` do cabeçalho continua sendo o do acervo inteiro, e a grade mostra menos do que o número diz |
    ///
    /// A segunda é a que estava no ar, e o defeito dela é o da §14 de novo: um
    /// contador que fala de um conjunto e uma grade que desenha outro. Só quem
    /// filtra pode contar — e quem filtra é esta consulta.
    ///
    /// **`any` e não `all`, sem opção de trocar.** "Tudo menos série e menos
    /// anime" é a leitura útil; a outra ("só sai quem for série *e* anime") não
    /// tem caso de uso e teria de ser explicada em toda tela.
    #[serde(default)]
    pub tags_not: Option<String>,
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
    /// `flat` desliga o agrupamento de versões da R47 e devolve um cartão por
    /// rip, como antes dela.
    ///
    /// **A saída existe porque a revisão do acervo precisa dela.** Quem confere
    /// o que foi baixado enxerga arquivo por arquivo; um agrupamento sem escape
    /// esconderia exatamente de quem precisa ver. Qualquer outro valor (inclusive
    /// a ausência) agrupa.
    #[serde(default)]
    pub versions: Option<String>,
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

/// `genre:Ação, format:anime` → `["genre:Ação", "format:anime"]`.
///
/// `None` quando não sobra nada — e é `None`, e não lista vazia, de propósito:
/// as duas consultas leem o parâmetro como `IS NULL OR …`, e um array vazio
/// faria `?tags=` (ou `?tags_not=,`) filtrar tudo pra fora em vez de não
/// filtrar nada.
fn lista_de_tags(raw: Option<&String>) -> Option<Vec<String>> {
    let parsed: Vec<String> = raw?
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    (!parsed.is_empty()).then_some(parsed)
}

/// O recorte da negação, compartilhado pelas duas listagens (R65).
///
/// Escrito uma vez porque `/api/works` e `/api/library` **têm** de concordar:
/// a grade sai de uma e o cabeçalho da aba conta pela outra.
fn sem_tags(marcador: &str) -> String {
    format!(
        "AND ({marcador}::text[] IS NULL OR NOT EXISTS (
                SELECT 1 FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
                 WHERE wt.work_id = w.id
                   AND (t.namespace || ':' || t.value) = ANY({marcador})))"
    )
}

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<ListParams>,
) -> AppResult<Json<Vec<WorkListItem>>> {
    let tags = lista_de_tags(params.tags.as_ref());
    let tags_not = lista_de_tags(params.tags_not.as_ref());
    let negacao = sem_tags("$16");

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
          {negacao}
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
        .bind(tags_not)
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
        collections: graph::collections_of(&state, id, user.id()).await?,
        relations: graph::relations_of(&state, id).await?,
    }))
}

/// **Desfaz** o que a obra registrou pra você — R69.
///
/// ## Por que existe
///
/// O mural diz "fulano terminou X", e essa frase nasce do `play_event`, que
/// nunca era apagado. Zerar o progresso não a derruba: medido pelo cliente em
/// 19/08/2026, uma obra ficou em `position_seconds = 0`, `finished = false`, e
/// o mural seguiu anunciando que ela tinha sido terminada. **Estado e histórico
/// são coisas diferentes**, e só o estado tinha escrita de volta.
///
/// O caso que encontrou não foi o pior: um ensaio de cliente tocou um episódio
/// até o fim. O pior é o autoplay que atravessou a temporada de madrugada, ou
/// o play sem querer — cada um vira uma frase que as outras pessoas da casa
/// leem como fato, e que alimenta a curadoria, os desafios e o "continuar".
///
/// ## O que ele apaga, e por que os dois
///
/// O log cru (`play_event`) **e** o cache derivado (`playback_state`), na mesma
/// transação. Apagar só o primeiro deixaria a obra no "continuar assistindo";
/// apagar só o segundo é exatamente o que já dava pra fazer e não resolvia
/// nada. Desfazer é um gesto só.
///
/// ## O que ele **não** apaga
///
/// **Nada de ninguém além de você.** O `user_id` sai da sessão e não do corpo,
/// então não há como escrever a rota de um jeito que alcance o histórico do
/// outro. E não apaga sua avaliação nem seus posts: aquilo você digitou, e o
/// §41 já separou o que o produto deduziu do que a pessoa disse.
///
/// ⚠️ **Conquista já ganha fica.** `conquista_do_usuario` guarda a data em que
/// foi conquistada, e desfazer uma reprodução não desconquista o que já foi
/// anunciado à casa — retirar um troféu do passado seria reescrever o mural em
/// vez de corrigi-lo. O que muda é o futuro: a próxima conferência lê o
/// histórico sem esta obra.
pub async fn apagar_progresso(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;

    let eventos = sqlx::query("DELETE FROM play_event WHERE user_id = $1 AND work_id = $2")
        .bind(user.id())
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    let estado = sqlx::query("DELETE FROM playback_state WHERE user_id = $1 AND work_id = $2")
        .bind(user.id())
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    tx.commit().await?;

    // Sem evento e sem estado é 404: dizer "ok" a quem tentou desfazer o que
    // não existe esconde um id errado atrás de um sucesso.
    if eventos == 0 && estado == 0 {
        return Err(AppError::NotFound);
    }

    tracing::info!(quem = %user.0.username, %id, eventos, estado, "progresso desfeito");
    Ok(Json(json!({ "eventos": eventos, "estado": estado })))
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
    let finished = terminou(body.position_seconds, body.duration_seconds);
    let recomecou = recomecou(body.position_seconds, body.duration_seconds);

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
    // **Grudento, com uma saída: o recomeço.** Do jeito puramente acumulativo
    // não existia retomar um rewatch. Terminar um filme congelava tudo: a ficha
    // dizia "assistir" pra sempre, e a obra não voltava pro `/api/continue`,
    // que exige `NOT finished`. A posição *andava* — o `position_seconds =
    // EXCLUDED.position_seconds` aqui embaixo nunca deixou de andar, e dá pra
    // ver isso no acervo, em linhas com `finished` e posição no minuto 1 — mas
    // andava invisível, porque quem lê a fileira olha o `finished` antes.
    //
    // Então ele volta a desligar num caso só: quando a posição cai abaixo do
    // `RESTART_RATIO`, que é pôr do começo. A objeção original continua de pé,
    // porque ela era sobre o minuto 30 (0,25) e isso segue sem apagar nada; o
    // que mudou é que "comecei de novo" virou um gesto que o servidor sabe ler.
    //
    // O histórico não depende disto pra sobreviver: `play_event` é o log cru e
    // nunca é sobrescrito, as conquistas contam por ele e por `play_count > 1`,
    // e o gosto (`curation/taste.rs`) deriva de `event_type = 'finish'`. Nada
    // que já foi ganho se perde quando a marca desliga.
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
            finished         = CASE
                                 WHEN EXCLUDED.finished THEN true
                                 WHEN $7                THEN false
                                 ELSE playback_state.finished
                               END,
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
    .bind(recomecou)
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
            user_id: user.id(),
            quem_nome: user.0.display_name.clone(),
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
    /// As outras cópias do mesmo filme (R47), quando há mais de uma.
    ///
    /// **Ausente quando o filme tem uma versão só**, que é o caso de quase todo
    /// o acervo: 712 dos 754 filmes agrupáveis. Mandar um array de um item em
    /// 17.930 linhas seria peso por nada, e deixa a regra do cliente trivial —
    /// **há `versions`, há modal**.
    ///
    /// Cada item traz `id` (o work id, e é por ele que a ficha abre),
    /// `media_file_id`, `height`, `size_bytes`, `duration_seconds`,
    /// `audio_langs`, `position_seconds` e `finished`. Nada é fundido: as duas
    /// obras continuam existindo, com os progressos delas separados. O que muda
    /// é quantas vezes o filme ocupa a grade.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub versions: Option<serde_json::Value>,
    /// Repetido em toda linha pelo `count(*) OVER ()` — é o preço de saber o
    /// total sem uma segunda ida ao banco.
    ///
    /// **Conta entradas agrupadas** desde a R47: ele alimenta o `carregadas /
    /// total` do cabeçalho e o gatilho de paginação, e contar rips enquanto a
    /// grade desenha grupos faria os dois números falarem de coisas diferentes.
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
    let tags = lista_de_tags(params.tags.as_ref());
    let tags_not = lista_de_tags(params.tags_not.as_ref());
    // R65: a negação entra **dentro do `filtrado`**, e não depois do
    // agrupamento. A diferença aparece numa série: excluir `format:anime` tem
    // de tirar os episódios de anime da conta da série, e não a série inteira
    // do resultado por causa de um episódio.
    let negacao = sem_tags("$17");

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
                g.grupo_id, g.grupo_title, g.grupo_year, g.grupo_poster, g.season_id,
                -- R47: a chave que junta dois rips do mesmo filme. É a
                -- **identificação**, e nunca título+ano: dois rips com títulos
                -- ligeiramente diferentes, ou um deles `unmatched` cujo "título"
                -- é o nome do arquivo, juntariam errado.
                w.external_ids->>'tmdb' AS tmdb,
                -- Os idiomas do áudio, **como o arquivo os declara** e nada
                -- além. `und` sai: ele quer dizer "não declarado", e escrevê-lo
                -- como idioma faria a modal oferecer "und" como escolha. Array
                -- vazio é a resposta honesta pra "o arquivo não diz" — e ela é
                -- comum: no acervo, 3.842 streams de áudio não têm tag nenhuma
                -- e 2.236 dizem `und`, incluindo o do 007 em inglês que motivou
                -- este pedido.
                -- `ARRAY[]::text[]` e não a chave literal: esta consulta passa
                -- por um `format!`, e lá a chave é marcador de argumento.
                (SELECT COALESCE(array_agg(DISTINCT s->'tags'->>'language'), ARRAY[]::text[])
                   FROM jsonb_array_elements(
                            CASE WHEN jsonb_typeof(f.probe->'streams') = 'array'
                                 THEN f.probe->'streams' ELSE '[]'::jsonb END) s
                  WHERE s->>'codec_type' = 'audio'
                    AND s->'tags'->>'language' IS NOT NULL
                    AND s->'tags'->>'language' <> 'und') AS audio_langs
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
                WHERE ci.work_id = w.id AND season.kind IN ('season', 'series', 'channel')
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
              {negacao}
        ),
        -- **R62 — os totais da série não dependem do filtro.**
        --
        -- Medido em 18/08/2026, na TCL: `GET /api/library` devolvia *Arcane*
        -- com `work_count = 84`… não, com 18; e `GET /api/library?q=dr`
        -- devolvia a **mesma série** com `work_count = 1` — junto de 18
        -- episódios. A tela escrevia `18 de 1`.
        --
        -- A causa é que o `series` agregava sobre `filtrado`, e `filtrado` já
        -- passou pela busca: `count(*)` contava *os episódios que casaram*, não
        -- os que existem. O mesmo valia pro `season_count` e pro
        -- `finished_count` — e este último é o que desenha a barra do cartão,
        -- então a barra ficava cheia sempre que o filtro deixava passar só o
        -- que já foi visto.
        --
        -- A regra que sai daqui: **o filtro decide se a série aparece; ele não
        -- decide o que o cartão diz sobre ela.** Um cartão de série descreve a
        -- série, e a série é a mesma nas duas rotas.
        --
        -- ⚠️ Duas coisas foram medidas aqui, e as duas contrariam o palpite —
        -- `GET /api/library?limit=60`, 18/08/2026, base **2,8 s**:
        --
        -- | variante | tempo |
        -- |---|---|
        -- | como está | **2,8 s** |
        -- | com `SELECT DISTINCT` em `serie_inteira` | 3,1 s |
        -- | limitando com `IN (SELECT grupo_id FROM filtrado)` | 3,2 s |
        --
        -- O `IN` parecia a economia óbvia — contar só as séries que saem na
        -- resposta — e é o contrário: ele obriga o `filtrado` inteiro, 17.930
        -- linhas com a subconsulta de idiomas dentro, a ser varrido mais uma
        -- vez. Varrer as 120 séries do acervo sem filtro nenhum sai de graça.
        --
        -- E o `DISTINCT` na CTE era redundante com os `count(DISTINCT …)` de
        -- baixo; tirá-lo devolveu os 0,3 s que a R62 tinha custado. Se alguma
        -- das duas ideias reaparecer, ela já foi medida.
        serie_inteira AS (
            SELECT COALESCE(pai.id, season.id) AS grupo_id,
                   ci.work_id,
                   season.id                   AS season_id
            FROM collection_item ci
            JOIN collection season ON season.id = ci.collection_id
                                  AND season.kind IN ('season', 'series', 'channel')
            LEFT JOIN collection pai ON pai.id = season.parent_id
            -- Ignorada é obra que alguém descartou: ela não conta no total
            -- pelo mesmo motivo que não aparece na grade.
            JOIN work w2 ON w2.id = ci.work_id AND w2.match_state <> 'ignored'
        ),
        totais_da_serie AS (
            SELECT si.grupo_id,
                   count(DISTINCT si.work_id)   AS work_count,
                   count(DISTINCT si.season_id) AS season_count,
                   count(DISTINCT si.work_id) FILTER (WHERE ps2.finished)
                                                AS finished_count
            FROM serie_inteira si
            LEFT JOIN playback_state ps2
                   ON ps2.work_id = si.work_id AND ps2.user_id = $1
            GROUP BY si.grupo_id
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
                -- R62: da série inteira, e não do que o filtro deixou passar.
                -- `COALESCE` porque uma série cujo grupo não está em
                -- `serie_inteira` (coleção que não é temporada/série) cai fora
                -- da conta — e zero seria pior que o que o filtro viu.
                COALESCE((SELECT t.work_count FROM totais_da_serie t
                           WHERE t.grupo_id = filtrado.grupo_id),
                         count(*))                    AS work_count,
                COALESCE((SELECT t.season_count FROM totais_da_serie t
                           WHERE t.grupo_id = filtrado.grupo_id),
                         count(DISTINCT season_id))   AS season_count,
                COALESCE((SELECT t.finished_count FROM totais_da_serie t
                           WHERE t.grupo_id = filtrado.grupo_id),
                         count(*) FILTER (WHERE finished)) AS finished_count,
                NULL::uuid    AS media_file_id,
                sum(duration_seconds)                 AS duration_seconds,
                NULL::int     AS height,
                NULL::bigint  AS size_bytes,
                NULL::text    AS kind,
                NULL::text    AS match_state,
                NULL::float8  AS position_seconds,
                min(created_at) AS created_at,
                max(updated_at) AS updated_at,
                -- Série não agrupa por versão (R47). Agrupar episódio pela
                -- identificação juntaria a temporada inteira num cartão só: os
                -- episódios de uma série compartilham o `tmdb` dela.
                NULL::jsonb   AS versions
            FROM filtrado
            WHERE grupo_id IS NOT NULL
            GROUP BY grupo_id, grupo_title, grupo_year, grupo_poster
        ),
        -- R47 — as três etapas do agrupamento de versões.
        --
        -- 1. `avulsas_cruas` continua sendo uma linha por rip, só que carregando
        --    a chave. `chave_versao` é NULL fora de `kind = 'movie'` e fora do
        --    que já foi identificado: dois `unmatched` não têm chave, e chutar
        --    neles seria decidir por semelhança o que aqui é uma chave.
        avulsas_cruas AS (
            SELECT
                id, title, year, poster, dominant_color,
                finished, media_file_id, duration_seconds, height, size_bytes,
                kind, match_state, position_seconds, created_at, updated_at,
                audio_langs,
                CASE WHEN kind = 'movie' AND tmdb IS NOT NULL THEN tmdb END AS chave_versao
            FROM filtrado
            WHERE grupo_id IS NULL
        ),
        -- 2. As janelas, numa passada só sobre todas as avulsas.
        --
        --    **`COALESCE(chave_versao, id::text)` é o que faz isto ser barato.**
        --    Sem ele, as milhares de linhas sem chave cairiam numa partição só e
        --    o `jsonb_agg` montaria um array gigante pra jogar fora em seguida.
        --    Com ele, cada linha sem chave é uma partição de um item.
        --
        --    ⚠️ **Já foi tentado rodar as janelas só sobre as ~800 candidatas**,
        --    separando o resto num `UNION ALL`. Fica **mais lento**, não mais
        --    rápido: `avulsas_cruas` passa a ser lida duas vezes e o `filtrado`
        --    inteiro é varrido de novo. Medido nesta base — 683 ms sem
        --    agrupamento nenhum, **916 ms** com esta passada única, 1.451 ms com
        --    a separação (e `MATERIALIZED` não salva). Se a ideia reaparecer,
        --    ela já foi medida.
        --
        --    A representante é a primeira da ordenação: quem tem pôster, depois
        --    a de maior altura, e o id pra desempatar — determinístico, senão o
        --    cartão trocaria de capa entre duas paginações.
        avulsas_marcadas AS (
            SELECT *,
                count(*) OVER (PARTITION BY COALESCE(chave_versao, id::text))
                    AS versoes_no_grupo,
                row_number() OVER (
                    PARTITION BY COALESCE(chave_versao, id::text)
                    ORDER BY (poster IS NULL), height DESC NULLS LAST, id
                ) AS posicao,
                jsonb_agg(jsonb_build_object(
                    'id',               id,
                    'media_file_id',    media_file_id,
                    'height',           height,
                    'size_bytes',       size_bytes,
                    'duration_seconds', duration_seconds,
                    'audio_langs',      to_jsonb(audio_langs),
                    'position_seconds', position_seconds,
                    'finished',         finished
                )) OVER (
                    PARTITION BY COALESCE(chave_versao, id::text)
                    ORDER BY (poster IS NULL), height DESC NULLS LAST, id
                    ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
                ) AS versoes_do_grupo
            FROM avulsas_cruas
        ),
        -- 3. Só a representante sobrevive — a menos que `?versions=flat` peça
        --    os rips separados, e aí nada colapsa e `versions` fica nulo.
        avulsas AS (
            SELECT
                id, false AS is_series, title, year, poster, dominant_color,
                1::bigint AS work_count,
                0::bigint AS season_count,
                (CASE WHEN finished THEN 1 ELSE 0 END)::bigint AS finished_count,
                media_file_id, duration_seconds, height, size_bytes,
                kind, match_state, position_seconds,
                created_at, updated_at,
                CASE WHEN $16 <> 'flat' AND versoes_no_grupo > 1
                     THEN versoes_do_grupo END AS versions
            FROM avulsas_marcadas
            WHERE $16 = 'flat' OR posicao = 1
        ),
        tudo AS (
            SELECT * FROM series
            UNION ALL
            SELECT * FROM avulsas
        )
        SELECT id, is_series, title, year, poster, dominant_color,
               work_count, season_count, finished_count,
               media_file_id, duration_seconds, height, size_bytes,
               kind, match_state, position_seconds, versions,
               -- Conta o que a grade desenha, e não os rips: a colapsagem já
               -- aconteceu em `avulsas`, então esta janela vê grupos.
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
        // $16 — o escape da R47. Ausente vira string vazia, que não é `flat`,
        // e o padrão do servidor é agrupar.
        .bind(params.versions.unwrap_or_default())
        .bind(tags_not)
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
/// Um arquivo que o scanner não achou mais no disco — **R70**.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct ArquivoSumido {
    pub id: Uuid,
    /// O caminho inteiro, e é o campo que faz esta rota valer a pena: é por ele
    /// que se descobre que sumiu **um disco**, e não onze arquivos.
    pub path: String,
    pub filename: String,
    pub size_bytes: i64,
    pub library_id: Uuid,
    pub library_name: Option<String>,
    /// A obra a que ele pertencia, quando pertencia a alguma.
    pub work_id: Option<Uuid>,
    pub work_title: Option<String>,
    /// **Visto pela última vez**, e não "sumiu em". A diferença é honesta e
    /// importa: o scanner marca `missing` comparando `scanned_at` com o início
    /// da varredura, e não escreve data nova ao marcar. Então o que existe é o
    /// último instante em que o arquivo **estava lá** — o sumiço aconteceu em
    /// algum momento entre essa data e a varredura que o notou.
    pub visto_pela_ultima_vez: chrono::DateTime<chrono::Utc>,
}

/// Quais arquivos sumiram do disco — **R70**.
///
/// O `/api/diagnostico` dizia `sumidos: 11` e mais nada. É a única linha da
/// saúde que representa **perda real**: "3.373 esperando revisão" é trabalho
/// pendente, e um arquivo que sumiu já não está lá. Com o número e sem a lista,
/// o dono sabe que perdeu onze coisas e não sabe quais — não restaura do
/// backup, não confere se foi um disco que desmontou, não decide se importa.
/// Um número que só cobra vira ruído.
///
/// ⚠️ **Esta rota não conserta nada, e é de propósito.** Ela não apaga obra
/// órfã e não re-escaneia: o que fazer com a lista é decisão de quem olha, e
/// essa decisão precisava da lista pra existir.
///
/// De administrador, como o `/api/diagnostico` — o caminho no disco é a planta
/// da casa, e ele não é assunto de convidado.
pub async fn sumidos(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<ListParams>,
) -> AppResult<Json<Value>> {
    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM media_file WHERE status = 'missing'")
            .fetch_one(&state.pool)
            .await?;

    // Ordenado pelo caminho: onze arquivos da mesma pasta contam uma história
    // que onze arquivos por data não contam.
    let arquivos = sqlx::query_as::<_, ArquivoSumido>(
        "SELECT mf.id, mf.path, mf.filename, mf.size_bytes,
                mf.library_id, l.name AS library_name,
                mf.work_id, w.title AS work_title,
                mf.scanned_at AS visto_pela_ultima_vez
         FROM media_file mf
         LEFT JOIN library l ON l.id = mf.library_id
         LEFT JOIN work w    ON w.id = mf.work_id
         WHERE mf.status = 'missing'
         ORDER BY mf.path
         LIMIT $1 OFFSET $2",
    )
    .bind(params.limit.clamp(1, 500))
    .bind(params.offset.max(0))
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(json!({ "total": total, "arquivos": arquivos })))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Um filme de duas horas, que é o caso de que a discussão do `finished`
    /// sempre falou.
    const DUAS_HORAS: Option<f64> = Some(7200.0);

    #[test]
    fn o_fim_liga_o_terminado() {
        assert!(terminou(7000.0, DUAS_HORAS)); // 0,97
        assert!(!terminou(6000.0, DUAS_HORAS)); // 0,83
    }

    #[test]
    fn por_do_comeco_e_recomeco() {
        // o teste que o pedido descreve: 45s de um filme longo
        assert!(recomecou(45.0, DUAS_HORAS));
        assert!(recomecou(0.0, DUAS_HORAS));
    }

    /// A objeção que criou o `finished` grudento era nominal — o minuto 30. Ela
    /// continua valendo, e é o que separa retomar de recomeçar.
    #[test]
    fn retomar_no_minuto_30_nao_apaga_o_terminado() {
        assert!(!recomecou(1800.0, DUAS_HORAS)); // 0,25
        assert!(!terminou(1800.0, DUAS_HORAS));
    }

    #[test]
    fn recomeco_e_fim_nunca_valem_ao_mesmo_tempo() {
        for posicao in [0.0, 45.0, 1800.0, 5000.0, 6624.0, 7000.0, 7200.0] {
            assert!(
                !(terminou(posicao, DUAS_HORAS) && recomecou(posicao, DUAS_HORAS)),
                "posição {posicao} caiu nos dois lados"
            );
        }
    }

    /// Sem duração o `finished` nunca liga; desligá-lo por um critério que não
    /// serve pra ligar seria apagar no escuro.
    #[test]
    fn sem_duracao_nada_liga_nem_desliga() {
        for duracao in [None, Some(0.0)] {
            assert!(!terminou(10.0, duracao));
            assert!(!recomecou(10.0, duracao));
        }
    }

    #[test]
    fn o_limiar_do_recomeco_fica_bem_abaixo_do_fim() {
        assert!(RESTART_RATIO < FINISHED_RATIO);
        // e abaixo do minuto 30 de um filme de duas horas, que é a objeção
        assert!(RESTART_RATIO < 0.25);
    }

    /// `?tags=` vazio não pode virar "nenhuma tag serve".
    ///
    /// As duas consultas leem o parâmetro como `IS NULL OR …`; um array vazio
    /// passaria pelo `IS NULL` e filtraria o acervo inteiro pra fora. É a
    /// diferença entre "não pedi filtro" e "pedi um filtro impossível".
    #[test]
    fn lista_vazia_de_tags_e_ausencia_e_nao_filtro_impossivel() {
        assert_eq!(lista_de_tags(None), None);
        assert_eq!(lista_de_tags(Some(&String::new())), None);
        assert_eq!(lista_de_tags(Some(&",  ,".to_string())), None);
    }

    #[test]
    fn as_tags_vem_separadas_por_virgula_e_sem_espaco_sobrando() {
        assert_eq!(
            lista_de_tags(Some(&"genre:Ação, format:anime".to_string())),
            Some(vec!["genre:Ação".to_string(), "format:anime".to_string()])
        );
    }

    /// **R65.** A negação é `NOT EXISTS`, e o `ANY` dentro dela é o que a faz
    /// ser "qualquer uma das que eu listei" — que é a leitura útil de
    /// "tudo menos série e menos anime".
    #[test]
    fn a_negacao_tira_quem_tem_qualquer_uma_das_tags() {
        let sql = sem_tags("$16");
        assert!(sql.contains("NOT EXISTS"), "{sql}");
        assert!(sql.contains("= ANY($16)"), "{sql}");
        // `IS NULL` primeiro: sem o parâmetro, a cláusula não filtra nada.
        assert!(sql.contains("$16::text[] IS NULL OR"), "{sql}");
    }

    /// O marcador é parâmetro da função porque as duas listagens numeram os
    /// binds de formas diferentes — `/api/works` para no $15, a biblioteca vai
    /// até o $16 do `versions`. Fixá-lo no texto ligaria uma das duas ao
    /// parâmetro errado, em silêncio.
    #[test]
    fn o_marcador_da_negacao_nao_e_fixo() {
        assert!(sem_tags("$17").contains("$17"));
        assert!(!sem_tags("$17").contains("$16"));
    }
}
