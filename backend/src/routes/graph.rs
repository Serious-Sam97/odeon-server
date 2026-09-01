//! O grafo: tags, coleções recursivas e relações obra↔obra.
//!
//! É aqui que o modelo do 0001 finalmente se paga. Nada nestas rotas precisou
//! de tabela nova — franquia, playlist, ordem de exibição alternativa e "corte
//! do diretor de" saem todas das mesmas três estruturas.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser};
use crate::error::{AppError, AppResult};
use crate::models::{
    AddItem, AttachTag, CollectionNode, CollectionRow, NewCollection, NewRelation, RelationRow,
    ReorderItems, TagNamespace, TagRow, UpdateCollection, WorkTag,
};
use crate::AppState;

/// R63 — a arte e o progresso da coleção, que a ficha de série precisa.
///
/// O cliente montava a fileira de temporadas de `/api/works?collection=…`,
/// agrupando por `season_number`, porque daqui não saía arte nenhuma: os
/// campos existiam na tabela (`artwork`, `dominant_color`) e não na resposta.
/// `finished_count` entra pelo mesmo motivo do cartão da biblioteca — é o que
/// desenha a barra de "quanto desta temporada eu já vi".
const COLLECTION_COLUMNS: &str = r#"
    c.id, c.kind, c.parent_id, c.title, c.year, c.overview, c.description,
    c.position, c.origin, c.provider_key,
    c.artwork->>'poster'   AS poster,
    c.artwork->>'backdrop' AS backdrop,
    c.dominant_color,
    COALESCE(agg.item_count, 0) AS item_count,
    COALESCE(agg.finished_count, 0) AS finished_count,
    agg.posters
"#;

/// Contagem e capas da **subárvore**, não dos filhos diretos.
///
/// Antes era `count(*) WHERE collection_id = c.id`, o que dá zero para toda
/// série: episódio pertence à temporada, não à série. A tela mostrava
/// "SÉRIE · 0" em cima de 70 episódios.
///
/// `DISTINCT` porque nada impede a mesma obra de estar numa temporada e numa
/// ordem de exibição dentro da mesma franquia — contá-la duas vezes seria
/// inventar acervo.
///
/// **Um rollup para todas as coleções, não uma CTE por linha.**
///
/// A primeira versão era um `LATERAL` com `WITH RECURSIVE` correlacionado em
/// `c.id` — correto, e 4,9 s na árvore, porque roda 576 vezes. Aqui a recursão
/// acontece uma vez só: `descendencia` casa cada coleção com toda a sua
/// subárvore (raiz inclusa), e o `GROUP BY` agrega em cima disso.
/// `$USUARIO` é trocado pelo marcador do id de quem pergunta — ver
/// `collection_with`. Ele existe só pelo `finished_count`: "quantas eu já vi"
/// é uma pergunta que não tem resposta sem saber quem é "eu".
const COLLECTION_WITH: &str = r#"
WITH RECURSIVE descendencia AS (
    SELECT id AS raiz, id AS no FROM collection
    UNION ALL
    SELECT d.raiz, filho.id
    FROM descendencia d JOIN collection filho ON filho.parent_id = d.no
),
agg AS (
    SELECT d.raiz,
           count(DISTINCT ci.work_id) AS item_count,
           count(DISTINCT ci.work_id) FILTER (WHERE ps.finished) AS finished_count,
           (array_agg(w.artwork->>'poster')
               FILTER (WHERE w.artwork ? 'poster'))[1:4] AS posters
    FROM descendencia d
    JOIN collection_item ci ON ci.collection_id = d.no
    JOIN work w ON w.id = ci.work_id
    LEFT JOIN playback_state ps ON ps.work_id = w.id AND ps.user_id = $USUARIO
    GROUP BY d.raiz
)
"#;

/// O `COLLECTION_WITH` com o marcador do usuário no lugar certo.
///
/// Cada rota numera seus binds de um jeito — a lista já usa `$1` e `$2` pro
/// filtro —, então o marcador não pode estar fixo no texto.
fn collection_with(usuario: &str) -> String {
    COLLECTION_WITH.replace("$USUARIO", usuario)
}

/// `COALESCE` porque coleção vazia não aparece no `agg` — e `NULL` viraria erro
/// de decode no `i64` do `item_count`.
const COLLECTION_JOIN: &str = "LEFT JOIN agg ON agg.raiz = c.id";

// ------------------------------------------------------------------- tags

pub async fn list_tags(State(state): State<AppState>) -> AppResult<Json<Vec<TagRow>>> {
    let tags = sqlx::query_as::<_, TagRow>(
        "SELECT t.id, t.namespace, t.value, t.color,
                (SELECT count(*) FROM work_tag wt WHERE wt.tag_id = t.id) AS work_count
         FROM tag t
         ORDER BY t.namespace, t.value",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(tags))
}

/// Namespaces com rótulo e cor. Não é uma lista de permitidos — qualquer
/// namespace novo funciona; estes só ganham tratamento visual fixo.
///
/// **Todo namespace em uso sai daqui, tenha linha em `tag_namespace` ou não.**
///
/// Antes era um `SELECT` seco na tabela, e a tabela só conhece o que alguém
/// semeou: o painel de filtros do celular mostrava `FORMATO · GÊNERO ·
/// COUNTRY`, porque `country` nasceu no M4 e ninguém voltou no 0003. O
/// `FULL OUTER JOIN` inverte a garantia — a lista passa a ser *o que o acervo
/// tem*, e a tabela vira só a decoração de quem ela conhece.
///
/// O rótulo de queda é `initcap` do próprio namespace, e ele é feio de
/// propósito: "Origin" na tela é um pedido de linha em `tag_namespace`, e é
/// melhor que uma tradução chutada — o servidor não tem como saber que
/// `country` se diz "País" (§18). Feio, porém, é diferente de gritado: a tela
/// põe em caixa alta o que recebe, e `COUNTRY` não se lia como rótulo nenhum.
///
/// Posição 900 joga o não-nomeado pro fim, depois de todo mundo que tem lugar
/// marcado — um namespace sem rótulo também não tem opinião sobre ordem.
pub async fn list_namespaces(State(state): State<AppState>) -> AppResult<Json<Vec<TagNamespace>>> {
    let rows = sqlx::query_as::<_, TagNamespace>(
        "SELECT COALESCE(tn.namespace, uso.namespace)      AS namespace,
                COALESCE(tn.label, initcap(uso.namespace)) AS label,
                tn.color,
                COALESCE(tn.position, 900)                 AS position
         FROM tag_namespace tn
         FULL OUTER JOIN (SELECT DISTINCT namespace FROM tag) uso
                      ON uso.namespace = tn.namespace
         ORDER BY position, label",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

pub async fn work_tags(
    State(state): State<AppState>,
    Path(work_id): Path<Uuid>,
) -> AppResult<Json<Vec<WorkTag>>> {
    Ok(Json(tags_of(&state, work_id).await?))
}

/// Tag e relação são **metadado do acervo**, e por isso são de administrador.
///
/// A diferença com a coleção é de dono: uma ordem de exibição `manual` é sua —
/// a ordem Machete é uma opinião, e opinião é de quem tem. Uma tag na obra e um
/// "corte do diretor de" mudam o que **todo mundo** vê, inclusive a curadoria
/// (§8f) e o guia, que leem `work_tag` como verdade sobre o acervo.
///
/// Estavam sem guarda nenhuma: qualquer conta autenticada — inclusive um
/// `guest` — podia etiquetar e desetiquetar qualquer obra.
pub async fn attach_tag(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(work_id): Path<Uuid>,
    Json(body): Json<AttachTag>,
) -> AppResult<Json<Vec<WorkTag>>> {
    let namespace = body.namespace.trim().to_lowercase();
    let value = body.value.trim().to_string();
    if namespace.is_empty() || value.is_empty() {
        return Err(AppError::BadRequest("namespace e valor são obrigatórios".into()));
    }

    let tag_id: Uuid = sqlx::query_scalar(
        "INSERT INTO tag (namespace, value, color) VALUES ($1, $2, $3)
         ON CONFLICT (namespace, value) DO UPDATE SET color = COALESCE(EXCLUDED.color, tag.color)
         RETURNING id",
    )
    .bind(&namespace)
    .bind(&value)
    .bind(body.color)
    .fetch_one(&state.pool)
    .await?;

    sqlx::query(
        "INSERT INTO work_tag (work_id, tag_id, source) VALUES ($1, $2, 'manual')
         ON CONFLICT (work_id, tag_id) DO UPDATE SET source = 'manual'",
    )
    .bind(work_id)
    .bind(tag_id)
    .execute(&state.pool)
    .await?;

    Ok(Json(tags_of(&state, work_id).await?))
}

pub async fn detach_tag(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path((work_id, tag_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<Vec<WorkTag>>> {
    sqlx::query("DELETE FROM work_tag WHERE work_id = $1 AND tag_id = $2")
        .bind(work_id)
        .bind(tag_id)
        .execute(&state.pool)
        .await?;
    Ok(Json(tags_of(&state, work_id).await?))
}

pub async fn tags_of(state: &AppState, work_id: Uuid) -> AppResult<Vec<WorkTag>> {
    let rows = sqlx::query_as::<_, WorkTag>(
        "SELECT t.id, t.namespace, t.value, t.color, wt.source
         FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
         WHERE wt.work_id = $1
         ORDER BY t.namespace, t.value",
    )
    .bind(work_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows)
}

// ------------------------------------------------------------- coleções

#[derive(Debug, Deserialize)]
pub struct CollectionFilter {
    pub kind: Option<String>,
    /// Só as raízes (sem pai) — útil pra listar franquias e playlists.
    #[serde(default)]
    pub roots_only: bool,
}

pub async fn list_collections(
    State(state): State<AppState>,
    user: AuthUser,
    Query(filter): Query<CollectionFilter>,
) -> AppResult<Json<Vec<CollectionRow>>> {
    let com = collection_with("$3");
    let rows = sqlx::query_as::<_, CollectionRow>(&format!(
        "{com}
         SELECT {COLLECTION_COLUMNS}
         FROM collection c
         {COLLECTION_JOIN}
         WHERE ($1::text IS NULL OR c.kind = $1)
           AND (NOT $2 OR c.parent_id IS NULL)
         ORDER BY c.kind, c.position NULLS LAST, c.title"
    ))
    .bind(filter.kind)
    .bind(filter.roots_only)
    .bind(user.id())
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

/// A árvore inteira. Franquia → série → temporada, quantos níveis houver.
pub async fn collection_tree(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Vec<CollectionNode>>> {
    let com = collection_with("$1");
    let all = sqlx::query_as::<_, CollectionRow>(&format!(
        "{com} SELECT {COLLECTION_COLUMNS} FROM collection c {COLLECTION_JOIN}
         ORDER BY c.position NULLS LAST, c.title"
    ))
    .bind(user.id())
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(build_tree(all)))
}

/// Montagem em memória: uma CTE recursiva devolveria linhas planas que teríamos
/// que remontar aqui de qualquer jeito.
///
/// Agrupa por pai uma vez e vai consumindo o mapa — além de ser O(n), o
/// `remove` garante que um ciclo acidental de `parent_id` não vire recursão
/// infinita.
fn build_tree(all: Vec<CollectionRow>) -> Vec<CollectionNode> {
    let mut by_parent: std::collections::HashMap<Option<Uuid>, Vec<CollectionRow>> =
        std::collections::HashMap::new();
    for collection in all {
        by_parent.entry(collection.parent_id).or_default().push(collection);
    }
    take_children(&mut by_parent, None)
}

fn take_children(
    by_parent: &mut std::collections::HashMap<Option<Uuid>, Vec<CollectionRow>>,
    parent: Option<Uuid>,
) -> Vec<CollectionNode> {
    by_parent
        .remove(&parent)
        .unwrap_or_default()
        .into_iter()
        .map(|collection| {
            let id = collection.id;
            CollectionNode {
                collection,
                children: take_children(by_parent, Some(id)),
            }
        })
        .collect()
}

pub async fn collection_detail(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let com = collection_with("$2");
    let collection = sqlx::query_as::<_, CollectionRow>(&format!(
        "{com} SELECT {COLLECTION_COLUMNS} FROM collection c {COLLECTION_JOIN} WHERE c.id = $1"
    ))
    .bind(id)
    .bind(user.id())
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    // Os filhos são as **temporadas** quando isto é uma série, e é deles que
    // sai a fileira de temporadas da ficha (R63): cada um traz `title`,
    // `overview`, `poster`, `position` (o número da temporada), `item_count`
    // (quantos episódios) e `finished_count` (quantos você viu).
    let children = sqlx::query_as::<_, CollectionRow>(&format!(
        "{com} SELECT {COLLECTION_COLUMNS} FROM collection c {COLLECTION_JOIN}
         WHERE c.parent_id = $1 ORDER BY c.position NULLS LAST, c.title"
    ))
    .bind(id)
    .bind(user.id())
    .fetch_all(&state.pool)
    .await?;

    // Ordem explícita da coleção — é isto que faz "ordem Machete" funcionar.
    //
    // R39: **e o ano decide quando ela não existe.** As sagas do TMDB chegam sem
    // `position` — medido: 315 itens de `franchise`, zero com posição, todos com
    // ano —, então a lista caía no alfabético e *Câmara Secreta* vinha antes de
    // *Pedra Filosofal*. As 8.410 linhas de temporada têm posição e não sentem
    // esta mudança.
    //
    // A `position` continua mandando onde existe porque é ela que carrega a
    // ordem Machete e as ordens manuais — que são **opinião**, e opinião tem
    // precedência sobre cronologia.
    let items = sqlx::query_as::<_, crate::models::WorkListItem>(
        r#"
        SELECT
            w.id, w.kind, w.title, w.year, w.season_number, w.episode_number,
            w.match_state, w.match_confidence, w.dominant_color,
            w.overview,
            w.artwork->>'poster' AS poster,
            w.artwork->>'backdrop' AS backdrop,
            w.artwork->>'still' AS still,
            NULL::text AS series_title,
            f.id AS media_file_id, f.duration_seconds, f.width, f.height,
            f.video_codec, f.audio_codec, f.container, f.size_bytes,
            ps.position_seconds, ps.finished,
            tg.tags
        FROM collection_item ci
        JOIN work w ON w.id = ci.work_id
        LEFT JOIN LATERAL (
            SELECT m.* FROM media_file m
            WHERE m.work_id = w.id AND m.status = 'probed'
            ORDER BY m.size_bytes DESC LIMIT 1
        ) f ON true
        LEFT JOIN LATERAL (
            SELECT array_agg(t.namespace || ':' || t.value ORDER BY t.namespace, t.value) AS tags
            FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
            WHERE wt.work_id = w.id
        ) tg ON true
        LEFT JOIN playback_state ps ON ps.work_id = w.id AND ps.user_id = $2
        WHERE ci.collection_id = $1
        ORDER BY ci.position NULLS LAST, w.year NULLS LAST, w.title
        "#,
    )
    .bind(id)
    .bind(user.id())
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(json!({
        "collection": collection,
        "children": children,
        "items": items,
    })))
}

/// Uma coleção que **não veio do provider**, ou o erro certo.
///
/// ## O furo que isto fecha
///
/// `delete_collection` já conferia a origem desde o §17 — e conferia bem. As
/// outras quatro rotas que mexem numa coleção, não: renomear, acrescentar obra,
/// tirar obra e reordenar aceitavam qualquer coleção de qualquer conta
/// autenticada.
///
/// Medido antes de consertar: as **709 coleções deste servidor são `provider`**
/// — série, temporada e as 133 sagas da R32. Ou seja, todas elas. Um morador
/// comum podia renomear "Harry Potter: Coleção" ou tirar um filme de dentro
/// dela, e o único motivo de nunca ter acontecido é ninguém ter tentado.
///
/// ## Por que origem, e não papel
///
/// Porque coleção é **as duas coisas**: as do provider são acervo, e as
/// `manual` são a feature de "suas ordens" (§17) — a ordem Machete é do
/// usuário, e exigir administrador pra criar uma mataria a feature.
///
/// `create_collection` já grava `'manual'` fixo, então ninguém cria uma
/// `provider` por aqui. A origem é a linha divisória certa.
async fn so_manual(state: &AppState, id: Uuid) -> AppResult<()> {
    let origin: Option<String> = sqlx::query_scalar("SELECT origin FROM collection WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    match origin.as_deref() {
        None => Err(AppError::NotFound),
        Some("provider") => Err(AppError::Forbidden(
            "esta coleção veio do provider — refaça a identificação em vez de editar".into(),
        )),
        _ => Ok(()),
    }
}

pub async fn create_collection(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<NewCollection>,
) -> AppResult<Json<CollectionRow>> {
    if body.title.trim().is_empty() {
        return Err(AppError::BadRequest("título é obrigatório".into()));
    }

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO collection (kind, title, parent_id, description, year, origin)
         VALUES ($1, $2, $3, $4, $5, 'manual') RETURNING id",
    )
    .bind(&body.kind)
    .bind(body.title.trim())
    .bind(body.parent_id)
    .bind(body.description)
    .bind(body.year)
    .fetch_one(&state.pool)
    .await?;

    fetch_collection(&state, id, user.id()).await.map(Json)
}

pub async fn update_collection(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateCollection>,
) -> AppResult<Json<CollectionRow>> {
    so_manual(&state, id).await?;

    sqlx::query(
        "UPDATE collection SET
            title       = COALESCE($2, title),
            description = COALESCE($3, description),
            year        = COALESCE($4, year)
         WHERE id = $1",
    )
    .bind(id)
    .bind(body.title)
    .bind(body.description)
    .bind(body.year)
    .execute(&state.pool)
    .await?;

    fetch_collection(&state, id, user.id()).await.map(Json)
}

pub async fn delete_collection(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    // Série e temporada vieram do provider; apagar na mão só desincroniza. A
    // regra é a mesma das outras quatro rotas desde a R37, e mora num lugar só.
    so_manual(&state, id).await?;

    sqlx::query("DELETE FROM collection WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn add_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<AddItem>,
) -> AppResult<Json<Value>> {
    so_manual(&state, id).await?;

    // Sem posição explícita, entra no fim da fila.
    let position = match body.position {
        Some(p) => p,
        None => sqlx::query_scalar::<_, Option<i32>>(
            "SELECT max(position) FROM collection_item WHERE collection_id = $1",
        )
        .bind(id)
        .fetch_one(&state.pool)
        .await?
        .unwrap_or(0)
            + 1,
    };

    sqlx::query(
        "INSERT INTO collection_item (collection_id, work_id, position) VALUES ($1, $2, $3)
         ON CONFLICT (collection_id, work_id) DO UPDATE SET position = EXCLUDED.position",
    )
    .bind(id)
    .bind(body.work_id)
    .bind(position)
    .execute(&state.pool)
    .await?;

    Ok(Json(json!({ "ok": true, "position": position })))
}

pub async fn remove_item(
    State(state): State<AppState>,
    Path((id, work_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<Value>> {
    so_manual(&state, id).await?;

    sqlx::query("DELETE FROM collection_item WHERE collection_id = $1 AND work_id = $2")
        .bind(id)
        .bind(work_id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

/// Reordena a coleção inteira numa transação. É o que torna "ordem Machete"
/// editável de verdade em vez de um enfeite.
pub async fn reorder(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ReorderItems>,
) -> AppResult<Json<Value>> {
    so_manual(&state, id).await?;

    let mut tx = state.pool.begin().await?;
    for entry in &body.items {
        sqlx::query(
            "UPDATE collection_item SET position = $3
             WHERE collection_id = $1 AND work_id = $2",
        )
        .bind(id)
        .bind(entry.work_id)
        .bind(entry.position)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({ "ok": true, "reordered": body.items.len() })))
}

async fn fetch_collection(state: &AppState, id: Uuid, usuario: Uuid) -> AppResult<CollectionRow> {
    let com = collection_with("$2");
    sqlx::query_as::<_, CollectionRow>(&format!(
        "{com} SELECT {COLLECTION_COLUMNS} FROM collection c {COLLECTION_JOIN} WHERE c.id = $1"
    ))
    .bind(id)
    .bind(usuario)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)
}

/// As coleções a que uma obra pertence — é o que a ficha desenha embaixo.
///
/// ⚠️ **Recebe o usuário porque o `COLLECTION_WITH` conta `finished_count`**
/// (R63). Não é opcional: sem o id, o `$USUARIO` do texto chega cru ao
/// Postgres e a ficha inteira sai em 500. Foi o que aconteceu — ver o teste
/// `nenhuma_consulta_de_colecao_esquece_o_usuario`.
pub async fn collections_of(
    state: &AppState,
    work_id: Uuid,
    usuario: Uuid,
) -> AppResult<Vec<CollectionRow>> {
    let com = collection_with("$2");
    let rows = sqlx::query_as::<_, CollectionRow>(&format!(
        "{com}
         SELECT {COLLECTION_COLUMNS}
         FROM collection_item ci
         JOIN collection c ON c.id = ci.collection_id
         {COLLECTION_JOIN}
         WHERE ci.work_id = $1
         ORDER BY c.kind, c.title"
    ))
    .bind(work_id)
    .bind(usuario)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows)
}

// ------------------------------------------------------------- relações

pub async fn relations(
    State(state): State<AppState>,
    Path(work_id): Path<Uuid>,
) -> AppResult<Json<Vec<RelationRow>>> {
    Ok(Json(relations_of(&state, work_id).await?))
}

/// Uma aresta é lida dos dois lados: quem aponta ("out") e quem é apontado
/// ("in"). "É corte alternativo de" e "tem corte alternativo" são a mesma linha.
pub async fn relations_of(state: &AppState, work_id: Uuid) -> AppResult<Vec<RelationRow>> {
    let rows = sqlx::query_as::<_, RelationRow>(
        r#"
        SELECT e.kind, e.label, e.position, 'out' AS direction,
               w.id AS other_id, w.title AS other_title, w.year AS other_year,
               w.artwork->>'poster' AS other_poster
        FROM work_edge e JOIN work w ON w.id = e.to_work
        WHERE e.from_work = $1
        UNION ALL
        SELECT e.kind, e.label, e.position, 'in' AS direction,
               w.id AS other_id, w.title AS other_title, w.year AS other_year,
               w.artwork->>'poster' AS other_poster
        FROM work_edge e JOIN work w ON w.id = e.from_work
        WHERE e.to_work = $1
        ORDER BY kind, position NULLS LAST, other_title
        "#,
    )
    .bind(work_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows)
}

pub async fn create_relation(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(work_id): Path<Uuid>,
    Json(body): Json<NewRelation>,
) -> AppResult<Json<Vec<RelationRow>>> {
    if body.to_work == work_id {
        return Err(AppError::BadRequest("uma obra não se relaciona consigo mesma".into()));
    }

    sqlx::query(
        "INSERT INTO work_edge (from_work, to_work, kind, label, position)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (from_work, to_work, kind)
         DO UPDATE SET label = EXCLUDED.label, position = EXCLUDED.position",
    )
    .bind(work_id)
    .bind(body.to_work)
    .bind(&body.kind)
    .bind(body.label)
    .bind(body.position)
    .execute(&state.pool)
    .await
    .map_err(|e| match &e {
        // O CHECK do 0001 limita os tipos de aresta.
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23514") => {
            AppError::BadRequest(format!("tipo de relação inválido: {}", body.kind))
        }
        _ => AppError::Db(e),
    })?;

    Ok(Json(relations_of(&state, work_id).await?))
}

pub async fn delete_relation(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path((work_id, other, kind)): Path<(Uuid, Uuid, String)>,
) -> AppResult<Json<Vec<RelationRow>>> {
    sqlx::query(
        "DELETE FROM work_edge
         WHERE kind = $3 AND ((from_work = $1 AND to_work = $2)
                           OR (from_work = $2 AND to_work = $1))",
    )
    .bind(work_id)
    .bind(other)
    .bind(&kind)
    .execute(&state.pool)
    .await?;

    Ok(Json(relations_of(&state, work_id).await?))
}

#[cfg(test)]
mod tests {
    /// **A opinião tem precedência sobre a cronologia.**
    ///
    /// A ordem de uma coleção é `position` primeiro — é ela que carrega a ordem
    /// Machete e as ordens manuais. O ano só decide onde `position` não existe,
    /// que é o caso das 133 sagas do TMDB; o título só decide o empate de ano.
    ///
    /// Inverter isto quebraria a ordem Machete em silêncio, que é o tipo de
    /// regressão que nenhuma tela denuncia.
    #[test]
    fn a_ordem_da_colecao_e_posicao_ano_titulo() {
        let fonte = include_str!("graph.rs");
        assert!(
            fonte.contains("ORDER BY ci.position NULLS LAST, w.year NULLS LAST, w.title"),
            "a ordem dos itens da coleção mudou de forma"
        );
    }

    /// **A regressão que este teste existe pra não deixar acontecer de novo.**
    ///
    /// A R63 pôs `finished_count` no `COLLECTION_WITH`, e com ele um
    /// `$USUARIO` que **cada rota numera do seu jeito** — a lista já usa `$1` e
    /// `$2` no filtro. Quem esqueceu de passar pelo `collection_with` mandou o
    /// marcador cru pro Postgres, e o que chegou de volta foi
    /// `syntax error at or near "$"` — em `/api/works/{id}`, ou seja, a ficha
    /// inteira em 500 ao clicar num filme. Duas funções auxiliares ficaram pra
    /// trás porque não são handlers e não apareceram na busca por rota.
    ///
    /// O erro é invisível na compilação: é texto virando SQL. Então a guarda é
    /// ler o próprio arquivo — nenhuma consulta interpola a `const` direto.
    #[test]
    fn nenhuma_consulta_de_colecao_esquece_o_usuario() {
        let fonte = include_str!("graph.rs");
        // A `const` só pode ser citada onde ela é definida e onde o
        // `collection_with` a consome — nunca dentro de um `format!`.
        //
        // A agulha é montada em pedaços de propósito: escrita inteira, ela
        // apareceria neste próprio arquivo e o teste falharia sozinho.
        let interpolada = format!("{}{}{}", "{", "COLLECTION_WITH", "}");
        assert!(
            !fonte.contains(&interpolada),
            "alguma consulta interpola COLLECTION_WITH direto e vai mandar $USUARIO cru"
        );
        // E o marcador tem de continuar existindo: se alguém o renomear sem
        // renomear no `replace`, o teste acima passaria e a query voltaria a
        // quebrar.
        assert!(fonte.contains(r#"ps.user_id = $USUARIO"#));
        assert!(fonte.contains(r#"COLLECTION_WITH.replace("$USUARIO", usuario)"#));
    }
}
