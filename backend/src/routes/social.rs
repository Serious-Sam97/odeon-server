//! R33 — a rede social: posts, comentários, presença e conversa.
//!
//! ## O que esta fase desfaz
//!
//! Duas podas por privacidade, e as duas contra a visão de quem decide.
//!
//! O §41 escreveu que o mural conta *"o que terminou, não o que abriu"* e
//! chamou isso de decisão de privacidade — *"anunciar cada coisa que se provou e
//! abandonou é vigilância com cara de recurso social"*. O §42 fechou a rota que
//! diz quem está assistindo o quê, tratando-a como vazamento.
//!
//! A decisão 2.2 do `IDEIAS.md` diz o contrário, e é explícita: **amigo vê o que
//! você está assistindo agora, o que largou no meio, o que terminou, suas
//! notas. Sem chave de privacidade.** O que as duas seções chamaram de vazamento
//! é a feature.
//!
//! Isso não torna as duas seções burras — elas foram escritas quando o escopo
//! era um "círculo" que podia ter um convidado dentro, e ali a preocupação fazia
//! sentido. Com **amizade que se aceita** (R28, §44), o aceite *é* o
//! consentimento: você só aparece pra quem você deixou entrar.
//!
//! ## A presença não vem do transcode
//!
//! O §42 fechou `/api/transcode/sessions`, e essa rota continua fechada — não
//! por privacidade, mas porque ela é o **pior** dos dois sinais disponíveis: ela
//! só enxerga quem está transcodificando, e o §3 decidiu que aqui o caso comum é
//! **Direct Play**. Uma lista de presença construída sobre ela diria que ninguém
//! está assistindo nada na maior parte do tempo.
//!
//! Os dois sinais honestos já estavam no banco:
//!
//! | pergunta | fonte |
//! |---|---|
//! | está online? | `auth_session.last_seen_at`, tocado a cada requisição |
//! | está assistindo? | `playback_state.updated_at`, tocado a cada heartbeat |
//!
//! O corte de "assistindo agora" é o mesmo 90s da locadora (§35) — nove batidas
//! perdidas do player. Repetir o número aqui faria duas telas discordarem sobre
//! quem está no meio de um filme.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::AppState;

/// Quanto tempo sem requisição até alguém deixar de estar online.
///
/// Cinco minutos, e não noventa segundos: `last_seen_at` é tocado por
/// requisição, e quem está lendo a ficha de um filme pode passar dois minutos
/// sem pedir nada. O corte curto faria a lista piscar.
const ONLINE: &str = "5 minutes";

/// E o de "assistindo agora" — o mesmo da locadora (§35).
const ASSISTINDO: &str = "90 seconds";

// ------------------------------------------------------------------- presença

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Presente {
    pub id: Uuid,
    pub display_name: String,
    /// O que está assistindo **agora**, quando está. `None` é "está online e
    /// não está vendo nada" — e a tela não inventa uma frase pra isso.
    pub assistindo: Option<String>,
    pub work_id: Option<Uuid>,
    pub poster: Option<String>,
    /// Se é amigo seu. É o que separa as duas listas sem pedir duas consultas.
    pub amigo: bool,
    pub eu: bool,
    /// R43 — o rosto escolhido, já como caminho de arte. `None` é quem não
    /// escolheu, e a tela cai na marca derivada do nome (R42).
    ///
    /// Sai do banco como **chave** e é trocado pela arte antes de responder: o
    /// catálogo é código (`enfeites.rs`), então SQL nenhum sabe traduzi-lo.
    pub rosto: Option<String>,
}

/// Quem está aqui.
///
/// **Duas listas foi o pedido** — *"quem está online no servidor e quem está
/// online entre os seus amigos"* —, e elas são a mesma consulta com uma coluna a
/// mais. Separar em duas rotas devolveria a mesma pessoa duas vezes e daria à
/// tela a chance de discordar de si mesma sobre quem é amigo.
pub async fn presenca(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Vec<Presente>>> {
    let sql = format!(
        r#"
        SELECT u.id, u.display_name,
               pf.avatar                    AS rosto,
               w.title                     AS assistindo,
               w.id                         AS work_id,
               w.artwork->>'poster'         AS poster,
               (u.id IN ({amigos}))         AS amigo,
               (u.id = $1)                  AS eu
        FROM app_user u
        LEFT JOIN perfil pf ON pf.user_id = u.id
        JOIN LATERAL (
            SELECT max(s.last_seen_at) AS visto
            FROM auth_session s WHERE s.user_id = u.id
        ) sess ON true
        -- O que está sendo assistido agora: a linha de progresso mais recente
        -- dentro da janela do heartbeat. `LEFT JOIN` porque estar online e não
        -- estar vendo nada é o caso comum.
        LEFT JOIN LATERAL (
            SELECT ps.work_id
            FROM playback_state ps
            WHERE ps.user_id = u.id
              AND ps.updated_at > now() - interval '{ASSISTINDO}'
            ORDER BY ps.updated_at DESC LIMIT 1
        ) agora ON true
        LEFT JOIN work w ON w.id = agora.work_id
        WHERE u.is_active
          AND (sess.visto > now() - interval '{ONLINE}' OR agora.work_id IS NOT NULL)
        ORDER BY (agora.work_id IS NOT NULL) DESC, u.display_name
        "#,
        amigos = crate::routes::amigos::IDS_DOS_MEUS_AMIGOS,
    );

    let mut lista = sqlx::query_as::<_, Presente>(&sql)
        .bind(user.id)
        .fetch_all(&state.pool)
        .await?;

    // A chave vira arte aqui, uma consulta pra lista inteira.
    let arte = crate::enfeites::arte_por_chave(&state.pool).await;
    for p in &mut lista {
        p.rosto = p.rosto.as_deref().and_then(|c| arte.get(c).cloned());
    }

    Ok(Json(lista))
}

// ---------------------------------------------------------------------- busca

#[derive(Debug, Deserialize)]
pub struct Busca {
    pub q: Option<String>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Achado {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    /// `amigo` | `enviado` | `recebido` | `nenhuma` — o mesmo vocabulário de
    /// `amigos.rs`, pra a tela não ter duas gramáticas pro mesmo estado.
    pub relacao: String,
}

/// Procurar gente.
///
/// Foi pedido junto com "adicionar pessoas" (§3.8). Hoje o servidor tem duas
/// contas e a busca é cerimônia — mas é ela que a fase 6 precisa ter pra a lista
/// de `amigos.rs` deixar de mandar o servidor inteiro quando ele crescer.
///
/// **Sem `q`, devolve todo mundo.** Com dois usuários isso é a resposta certa;
/// o `LIMIT` é o que impede a mesma rota de virar um despejo quando não for.
pub async fn pessoas(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(b): Query<Busca>,
) -> AppResult<Json<Vec<Achado>>> {
    let termo = b.q.as_deref().map(str::trim).filter(|s| !s.is_empty());

    Ok(Json(
        sqlx::query_as::<_, Achado>(
            r#"
            SELECT u.id, u.username, u.display_name,
                   CASE
                       WHEN am.aceito_em IS NOT NULL  THEN 'amigo'
                       WHEN am.pedido_por = $1        THEN 'enviado'
                       WHEN am.pedido_por IS NOT NULL THEN 'recebido'
                       ELSE 'nenhuma'
                   END AS relacao
            FROM app_user u
            LEFT JOIN amizade am
              ON am.a = least(u.id, $1) AND am.b = greatest(u.id, $1)
            WHERE u.id <> $1 AND u.is_active
              AND ($2::text IS NULL
                   OR u.display_name ILIKE '%' || $2 || '%'
                   OR u.username     ILIKE '%' || $2 || '%')
            ORDER BY u.display_name
            LIMIT 50
            "#,
        )
        .bind(user.id)
        .bind(termo)
        .fetch_all(&state.pool)
        .await?,
    ))
}

// ----------------------------------------------------------------------- post

#[derive(Debug, Deserialize)]
pub struct NovoPost {
    pub texto: String,
    /// A obra citada, quando há. É o que faz o post ser sobre alguma coisa.
    pub work_id: Option<Uuid>,
}

/// Postar.
pub async fn postar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(novo): Json<NovoPost>,
) -> AppResult<Json<Value>> {
    let texto = novo.texto.trim();
    if texto.is_empty() {
        return Err(AppError::BadRequest("um post vazio não é um post".into()));
    }

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO post (user_id, texto, work_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(user.id)
    .bind(texto.chars().take(500).collect::<String>())
    .bind(novo.work_id)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(json!({ "id": id })))
}

/// Apagar o próprio post. O `WHERE user_id` é a autorização inteira — apagar o
/// post de outra pessoa não é um 403, é um 404: você não tem esse post.
pub async fn apagar_post(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let n = sqlx::query("DELETE FROM post WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "apagado": true })))
}

// ----------------------------------------------------------------- comentário

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Comentario {
    pub id: Uuid,
    pub quem: String,
    pub quem_id: Uuid,
    pub meu: bool,
    pub texto: String,
    pub criado_em: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
pub struct NovoComentario {
    pub texto: String,
    /// Um alvo, e só um — a mesma regra do CHECK da 0032, lida do outro lado.
    pub post_id: Option<Uuid>,
    pub review_user: Option<Uuid>,
    pub review_work: Option<Uuid>,
}

/// Comentar num post ou numa review.
///
/// **Uma rota pros dois**, espelhando a tabela. Duas rotas quase idênticas
/// seriam duas telas e duas chances de divergirem sobre o que é um comentário —
/// e a tela é literalmente a mesma nos dois lugares.
pub async fn comentar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(novo): Json<NovoComentario>,
) -> AppResult<Json<Value>> {
    let texto = novo.texto.trim();
    if texto.is_empty() {
        return Err(AppError::BadRequest("um comentário vazio não é um comentário".into()));
    }

    let tem_post = novo.post_id.is_some();
    let tem_review = novo.review_user.is_some() && novo.review_work.is_some();
    if tem_post == tem_review {
        return Err(AppError::BadRequest(
            "comente num post ou numa review, exatamente um".into(),
        ));
    }

    let feito = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO comentario (user_id, texto, post_id, review_user, review_work)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(user.id)
    .bind(texto.chars().take(500).collect::<String>())
    .bind(novo.post_id)
    .bind(novo.review_user)
    .bind(novo.review_work)
    .fetch_one(&state.pool)
    .await;

    match feito {
        Ok(id) => Ok(Json(json!({ "id": id }))),
        // 23503 = violação de chave estrangeira: o post ou a review sumiu entre
        // abrir a tela e comentar. É 404, não 500 (§8b).
        Err(e) => {
            if matches!(&e, sqlx::Error::Database(db) if db.code().as_deref() == Some("23503")) {
                return Err(AppError::NotFound);
            }
            Err(e.into())
        }
    }
}

pub async fn apagar_comentario(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let n = sqlx::query("DELETE FROM comentario WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "apagado": true })))
}

/// Os comentários de uma review, pra ficha do filme.
pub async fn comentarios_da_review(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((quem, obra)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<Vec<Comentario>>> {
    Ok(Json(
        sqlx::query_as::<_, Comentario>(
            "SELECT c.id, u.display_name AS quem, u.id AS quem_id, u.id = $1 AS meu,
                    c.texto, c.criado_em
             FROM comentario c JOIN app_user u ON u.id = c.user_id
             WHERE c.review_user = $2 AND c.review_work = $3
             ORDER BY c.criado_em",
        )
        .bind(user.id)
        .bind(quem)
        .bind(obra)
        .fetch_all(&state.pool)
        .await?,
    ))
}

// ------------------------------------------------------------------ mensagem

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Conversa {
    pub com: Uuid,
    pub display_name: String,
    pub ultima: Option<String>,
    pub quando: Option<chrono::DateTime<chrono::Utc>>,
    pub nao_lidas: i64,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Mensagem {
    pub id: i64,
    pub minha: bool,
    pub texto: String,
    pub criado_em: chrono::DateTime<chrono::Utc>,
}

/// As conversas: uma linha por amigo, com a última mensagem e o que falta ler.
///
/// **Uma por amigo, e não uma por conversa existente.** Um amigo com quem você
/// nunca falou aparece com a linha vazia — é assim que se começa a falar. Uma
/// lista que só mostra conversas já iniciadas exige uma segunda tela pra
/// escolher com quem falar.
pub async fn conversas(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Vec<Conversa>>> {
    let sql = format!(
        r#"
        SELECT u.id AS com, u.display_name,
               ult.texto      AS ultima,
               ult.criado_em  AS quando,
               COALESCE(nl.n, 0) AS nao_lidas
        FROM app_user u
        LEFT JOIN LATERAL (
            SELECT m.texto, m.criado_em FROM mensagem m
            WHERE (m.de = $1 AND m.para = u.id) OR (m.de = u.id AND m.para = $1)
            ORDER BY m.criado_em DESC LIMIT 1
        ) ult ON true
        LEFT JOIN LATERAL (
            SELECT count(*) AS n FROM mensagem m
            WHERE m.de = u.id AND m.para = $1 AND m.lido_em IS NULL
        ) nl ON true
        WHERE u.is_active AND u.id IN ({amigos})
        ORDER BY ult.criado_em DESC NULLS LAST, u.display_name
        "#,
        amigos = crate::routes::amigos::IDS_DOS_MEUS_AMIGOS,
    );

    Ok(Json(
        sqlx::query_as::<_, Conversa>(&sql)
            .bind(user.id)
            .fetch_all(&state.pool)
            .await?,
    ))
}

/// Uma conversa, e ela é marcada como lida ao ser aberta.
pub async fn conversa(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(com): Path<Uuid>,
) -> AppResult<Json<Vec<Mensagem>>> {
    let msgs = sqlx::query_as::<_, Mensagem>(
        "SELECT m.id, m.de = $1 AS minha, m.texto, m.criado_em
         FROM mensagem m
         WHERE (m.de = $1 AND m.para = $2) OR (m.de = $2 AND m.para = $1)
         ORDER BY m.criado_em
         LIMIT 200",
    )
    .bind(user.id)
    .bind(com)
    .fetch_all(&state.pool)
    .await?;

    // Abrir a conversa é ler. Um botão "marcar como lida" seria trabalho pra
    // alguém confirmar o que os olhos já fizeram.
    sqlx::query("UPDATE mensagem SET lido_em = now() WHERE para = $1 AND de = $2 AND lido_em IS NULL")
        .bind(user.id)
        .bind(com)
        .execute(&state.pool)
        .await?;

    Ok(Json(msgs))
}

#[derive(Debug, Deserialize)]
pub struct NovaMensagem {
    pub texto: String,
}

/// Mandar. **Só pra amigo** — e esta é a única restrição de toda a fase.
///
/// Não é privacidade sobre o que você assiste (a decisão 2.2 abriu isso): é que
/// mensagem direta de estranho é o mecanismo pelo qual toda rede social vira
/// desagradável, e a amizade aqui já tem aceite. Quem quer falar com você pede
/// amizade primeiro, que é uma tela e um clique.
pub async fn mandar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(para): Path<Uuid>,
    Json(nova): Json<NovaMensagem>,
) -> AppResult<Json<Value>> {
    let texto = nova.texto.trim();
    if texto.is_empty() {
        return Err(AppError::BadRequest("mensagem vazia".into()));
    }

    let amigos: bool = crate::routes::amigos::sao_amigos(&state.pool, user.id, para).await;
    if !amigos {
        return Err(AppError::Forbidden(
            "vocês precisam ser amigos pra conversar".into(),
        ));
    }

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO mensagem (de, para, texto) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(user.id)
    .bind(para)
    .bind(texto.chars().take(2000).collect::<String>())
    .fetch_one(&state.pool)
    .await?;

    // Chega na hora, pelo barramento que o M3 já mantém aberto. É a mesma
    // escolha do "pedir de volta" (§35): uma conversa em que a resposta chega
    // amanhã não é uma conversa.
    crate::events::publish(
        &state.events,
        crate::events::AppEvent::Mensagem {
            de: user.id,
            de_nome: user.display_name.clone(),
            para,
        },
    );

    Ok(Json(json!({ "id": id })))
}

#[cfg(test)]
mod tests {
    /// **O corte de "assistindo agora" é o da locadora.** Duas telas com números
    /// diferentes discordariam sobre quem está no meio de um filme — e a
    /// presença diria "não está vendo nada" de alguém que a locadora se recusa
    /// a interromper.
    #[test]
    fn assistindo_agora_usa_o_corte_da_locadora() {
        assert_eq!(super::ASSISTINDO, "90 seconds");
    }

    /// E o de "online" é mais longo de propósito: `last_seen_at` é tocado por
    /// requisição, e quem está lendo uma ficha passa minutos sem pedir nada. Um
    /// corte curto faria a lista piscar.
    #[test]
    fn online_e_mais_folgado_que_assistindo() {
        assert_eq!(super::ONLINE, "5 minutes");
    }
}
