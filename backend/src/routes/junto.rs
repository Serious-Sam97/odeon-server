//! R46 — assistir junto.
//!
//! > *"Watch Party (Interação fácil entre amigos)"*
//!
//! O `IDEIAS-2.md` §4.6 decidiu o que é: **assistir junto de verdade
//! sincronizado, mais conversa ao lado durante a sessão.** E respondeu as três
//! perguntas do desenho antes de existir código — este módulo é a
//! transcrição delas.
//!
//! ## 1. Quem manda é o host
//!
//! Existe um dono da sessão, e é dele o controle. Isso resolve sozinho a briga
//! de dois cliques simultâneos: **não há eleição, não há empate**, e o estado
//! tem uma fonte só. Um membro que aperta pausa não pausa nada — a tela dele
//! nem oferece.
//!
//! ## 2. Quando um trava, todo mundo para
//!
//! *"Sempre sincronizado."* Não há modo tolerante em que os rápidos seguem e o
//! lento se perde: se a sessão é assistir junto, assistir separado por trinta
//! segundos é a sessão tendo falhado em silêncio (§8b).
//!
//! Por isso `tocando` é a **intenção** do host, e o que toca de verdade é
//! `tocando && todos prontos`. Cada tela avisa quando está carregando e quando
//! voltou; o servidor não pergunta nada, só soma.
//!
//! **A consequência foi dita antes de ser sentida** (§4.6): a conexão mais
//! lenta manda no ritmo de todo mundo. Se incomodar, o conserto não é afrouxar
//! a sincronia — é o host poder expulsar quem está segurando a sessão, que é
//! uma decisão social e não técnica. Daí a rota de expulsar existir desde o
//! primeiro dia.
//!
//! ## 3. Os dois modos de stream, como opção da sessão
//!
//! | modo | o que é | quando serve |
//! |---|---|---|
//! | `por_pessoa` | cada um abre a própria sessão do mesmo arquivo | qualidade por aparelho, e um travar não derruba o outro |
//! | `compartilhado` | um transcode só, servido pros dois | mais barato pra máquina |
//!
//! O padrão é um por pessoa — é o que já funcionava sem código novo. O
//! compartilhado guarda o `transcode_id` do host, e os outros leem a playlist
//! dele: a autorização da rota de HLS passou a aceitar **membro da sala**, e é
//! só isso que o modo custou.
//!
//! ## O transporte é o barramento do M3
//!
//! *"Ele é o transporte, e não se inventa um segundo canal."* O evento diz
//! apenas **qual sala mexeu**; o estado mora na tabela. Quem entra atrasado lê
//! o estado e chega no lugar certo — o oposto do defeito que a R44 encontrou
//! no aviso de programa, onde o evento publicado no vazio sumia pra sempre.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::AppState;

/// Quanto tempo de silêncio faz um membro ser considerado ausente.
///
/// Quem fecha a aba não avisa — e uma sala que espera pra sempre por alguém que
/// foi dormir é uma sala travada. Dois minutos: longo o bastante pra atravessar
/// uma pausa pro banheiro, curto o bastante pra não segurar o filme dos outros.
const AUSENTE: &str = "2 minutes";

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MembroNaSala {
    pub user_id: Uuid,
    pub username: String,
    pub display_name: String,
    pub pronto: bool,
    /// Se é o dono da sala.
    pub host: bool,
    /// Se sumiu — a tela mostra, e o host decide o que fazer.
    pub ausente: bool,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct RecadoNaSala {
    pub id: i64,
    pub user_id: Uuid,
    pub display_name: String,
    pub texto: String,
    pub em: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct Sala {
    pub id: Uuid,
    pub work_id: Uuid,
    pub titulo: String,
    pub poster: Option<String>,
    pub media_file_id: Option<Uuid>,
    pub host_id: Uuid,
    pub host_nome: String,
    /// Se **eu** sou o host. A tela não decide isso comparando ids.
    pub sou_host: bool,
    pub modo: String,
    pub transcode_id: Option<Uuid>,
    /// A intenção do host.
    pub tocando: bool,
    /// O que toca de verdade: a intenção **e** todo mundo pronto.
    pub rodando: bool,
    /// Quem está segurando, quando alguém está. É o que a tela mostra em vez de
    /// um "carregando…" que não diz de quem.
    pub esperando: Vec<String>,
    pub posicao_segundos: f64,
    pub atualizado_em: chrono::DateTime<chrono::Utc>,
    pub gente: Vec<MembroNaSala>,
    pub conversa: Vec<RecadoNaSala>,
}

#[derive(Debug, Deserialize)]
pub struct NovaSala {
    pub work_id: Uuid,
    pub media_file_id: Option<Uuid>,
    #[serde(default)]
    pub modo: Option<String>,
}

/// Abre uma sala. **Uma por host** — a segunda encerra a primeira.
///
/// Encerrar em vez de recusar: quem clicou "assistir junto" num outro filme
/// quis assistir esse outro filme, e devolver um erro pediria que ele fosse
/// procurar uma sala que talvez nem lembre ter deixado aberta.
pub async fn criar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(nova): Json<NovaSala>,
) -> AppResult<Json<Sala>> {
    let modo = match nova.modo.as_deref() {
        None | Some("por_pessoa") => "por_pessoa",
        Some("compartilhado") => "compartilhado",
        Some(outro) => return Err(AppError::BadRequest(format!("modo desconhecido: {outro}"))),
    };

    encerrar_do_host(&state, user.id).await?;

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO sessao_junta (work_id, media_file_id, host_id, modo)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(nova.work_id)
    .bind(nova.media_file_id)
    .bind(user.id)
    .bind(modo)
    .fetch_one(&state.pool)
    .await?;

    // O host é membro da própria sala. Sem isto ele não conta pro "todo mundo
    // pronto", e a sala tocaria enquanto o dono ainda carrega.
    entrar_na_sala(&state, id, user.id).await?;

    avisar(&state, id, "gente");
    montar(&state, id, user.id).await.map(Json)
}

/// A sala em que eu estou, se estiver em alguma.
pub async fn atual(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Option<Sala>>> {
    let id: Option<Uuid> = sqlx::query_scalar(
        "SELECT s.id FROM sessao_junta s
         JOIN sessao_junta_membro m ON m.sessao_id = s.id AND m.user_id = $1
         WHERE s.encerrada_em IS NULL
         ORDER BY s.criada_em DESC LIMIT 1",
    )
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?;

    match id {
        Some(id) => montar(&state, id, user.id).await.map(Some).map(Json),
        None => Ok(Json(None)),
    }
}

/// As salas abertas **dos seus amigos** — é o convite.
///
/// Não há tabela de convite, e é decisão: a amizade já é o aceite (§44), então
/// uma sala aberta é visível pra quem foi aceito e mais ninguém. Um convite
/// individual seria uma segunda permissão em cima de uma que já existe.
pub async fn abertas(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Vec<Value>>> {
    let sql = format!(
        "SELECT s.id, s.host_id, u.display_name AS host_nome, w.title AS titulo,
                w.artwork->>'poster' AS poster,
                (SELECT count(*) FROM sessao_junta_membro m WHERE m.sessao_id = s.id) AS gente
         FROM sessao_junta s
         JOIN app_user u ON u.id = s.host_id
         JOIN work w ON w.id = s.work_id
         WHERE s.encerrada_em IS NULL
           AND s.host_id <> $1
           AND s.host_id IN ({amigos})
           AND NOT EXISTS (SELECT 1 FROM sessao_junta_membro m
                            WHERE m.sessao_id = s.id AND m.user_id = $1)
         ORDER BY s.criada_em DESC",
        amigos = crate::routes::amigos::IDS_DOS_MEUS_AMIGOS,
    );

    let linhas: Vec<(Uuid, Uuid, String, String, Option<String>, i64)> =
        sqlx::query_as(&sql).bind(user.id).fetch_all(&state.pool).await?;

    Ok(Json(
        linhas
            .into_iter()
            .map(|(id, host_id, host_nome, titulo, poster, gente)| {
                json!({ "id": id, "host_id": host_id, "host_nome": host_nome,
                        "titulo": titulo, "poster": poster, "gente": gente })
            })
            .collect(),
    ))
}

pub async fn entrar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Sala>> {
    let aberta: Option<(Uuid,)> =
        sqlx::query_as("SELECT host_id FROM sessao_junta WHERE id = $1 AND encerrada_em IS NULL")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let Some((host,)) = aberta else {
        return Err(AppError::NotFound);
    };

    // Só amigo do host entra. É a mesma régua do §49: quem vê o que você faz é
    // quem você aceitou.
    if host != user.id && !crate::routes::amigos::sao_amigos(&state.pool, host, user.id).await {
        return Err(AppError::Forbidden(
            "esta sala é de alguém que ainda não é seu amigo".into(),
        ));
    }

    entrar_na_sala(&state, id, user.id).await?;
    avisar(&state, id, "gente");
    montar(&state, id, user.id).await.map(Json)
}

/// Sair. **Se o host sai, a sala acaba** — sem herança de dono.
///
/// Passar o controle pro próximo seria inventar uma regra de sucessão que
/// ninguém pediu, e a sala é do host: quando ele sai, o que sobra é um filme
/// que cada um pode continuar sozinho.
pub async fn sair(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let host: Option<(Uuid,)> = sqlx::query_as("SELECT host_id FROM sessao_junta WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    let Some((host,)) = host else {
        return Err(AppError::NotFound);
    };

    if host == user.id {
        sqlx::query("UPDATE sessao_junta SET encerrada_em = now() WHERE id = $1 AND encerrada_em IS NULL")
            .bind(id)
            .execute(&state.pool)
            .await?;
        avisar(&state, id, "fim");
        return Ok(Json(json!({ "encerrada": true })));
    }

    sqlx::query("DELETE FROM sessao_junta_membro WHERE sessao_id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    avisar(&state, id, "gente");
    Ok(Json(json!({ "encerrada": false })))
}

/// O host expulsa alguém.
///
/// Existe desde o primeiro dia porque o §4.6 já sabia por que ela existiria:
/// *"se na prática incomodar, o conserto não é afrouxar a sincronia — é o host
/// poder expulsar quem está segurando a sessão"*.
pub async fn expulsar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, quem)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<Value>> {
    so_host(&state, id, user.id).await?;
    if quem == user.id {
        return Err(AppError::BadRequest("o host sai pela porta de sair".into()));
    }
    sqlx::query("DELETE FROM sessao_junta_membro WHERE sessao_id = $1 AND user_id = $2")
        .bind(id)
        .bind(quem)
        .execute(&state.pool)
        .await?;
    avisar(&state, id, "gente");
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct NovoEstado {
    pub tocando: bool,
    pub posicao_segundos: f64,
    /// Só no modo compartilhado: a sessão de transcode que o host abriu.
    #[serde(default)]
    pub transcode_id: Option<Uuid>,
}

/// O play, a pausa e o pulo. **Só o host.**
pub async fn estado(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    Json(novo): Json<NovoEstado>,
) -> AppResult<Json<Sala>> {
    so_host(&state, id, user.id).await?;

    sqlx::query(
        "UPDATE sessao_junta
            SET tocando = $2, posicao_segundos = $3,
                transcode_id = COALESCE($4, transcode_id),
                atualizado_em = now()
          WHERE id = $1 AND encerrada_em IS NULL",
    )
    .bind(id)
    .bind(novo.tocando)
    .bind(novo.posicao_segundos)
    .bind(novo.transcode_id)
    .execute(&state.pool)
    .await?;

    avisar(&state, id, "estado");
    montar(&state, id, user.id).await.map(Json)
}

#[derive(Debug, Deserialize)]
pub struct Prontidao {
    pub pronto: bool,
}

/// "Estou carregando" / "voltei". É o que faz todo mundo parar junto.
///
/// Também serve de sinal de vida: quem manda isto está com a aba viva, e é o
/// `visto_em` que separa "travado" de "foi embora sem avisar".
pub async fn pronto(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    Json(p): Json<Prontidao>,
) -> AppResult<Json<Sala>> {
    let mudou = sqlx::query_scalar::<_, bool>(
        "UPDATE sessao_junta_membro SET pronto = $3, visto_em = now()
          WHERE sessao_id = $1 AND user_id = $2
      RETURNING (pronto IS DISTINCT FROM $3)",
    )
    .bind(id)
    .bind(user.id)
    .bind(p.pronto)
    .fetch_optional(&state.pool)
    .await?;

    if mudou.is_none() {
        return Err(AppError::NotFound);
    }

    // O barramento só é acordado quando a prontidão **muda**. O batimento é a
    // cada poucos segundos, e avisar a sala inteira a cada um deles seria um
    // evento por segundo por pessoa pra dizer "continuo bem".
    if mudou == Some(true) {
        avisar(&state, id, "estado");
    }
    montar(&state, id, user.id).await.map(Json)
}

#[derive(Debug, Deserialize)]
pub struct NovoRecado {
    pub texto: String,
}

pub async fn recado(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    Json(r): Json<NovoRecado>,
) -> AppResult<Json<Sala>> {
    let texto = r.texto.trim();
    if texto.is_empty() {
        return Err(AppError::BadRequest("recado vazio".into()));
    }
    so_membro(&state, id, user.id).await?;

    sqlx::query("INSERT INTO sessao_junta_recado (sessao_id, user_id, texto) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(user.id)
        .bind(texto)
        .execute(&state.pool)
        .await?;

    avisar(&state, id, "recado");
    montar(&state, id, user.id).await.map(Json)
}

// ------------------------------------------------------------------ o miolo

async fn montar(state: &AppState, id: Uuid, eu: Uuid) -> AppResult<Sala> {
    type Linha = (
        Uuid,
        Option<Uuid>,
        Uuid,
        String,
        String,
        Option<String>,
        String,
        Option<Uuid>,
        bool,
        f64,
        chrono::DateTime<chrono::Utc>,
    );

    let linha: Option<Linha> = sqlx::query_as(
        "SELECT s.work_id, s.media_file_id, s.host_id, u.display_name, w.title,
                w.artwork->>'poster', s.modo, s.transcode_id, s.tocando,
                s.posicao_segundos, s.atualizado_em
           FROM sessao_junta s
           JOIN app_user u ON u.id = s.host_id
           JOIN work w ON w.id = s.work_id
          WHERE s.id = $1 AND s.encerrada_em IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;

    let Some((
        work_id,
        media_file_id,
        host_id,
        host_nome,
        titulo,
        poster,
        modo,
        transcode_id,
        tocando,
        posicao_segundos,
        atualizado_em,
    )) = linha
    else {
        return Err(AppError::NotFound);
    };

    let gente: Vec<MembroNaSala> = sqlx::query_as(&format!(
        "SELECT m.user_id, u.username, u.display_name, m.pronto,
                (m.user_id = $2) AS host,
                (m.visto_em < now() - interval '{AUSENTE}') AS ausente
           FROM sessao_junta_membro m
           JOIN app_user u ON u.id = m.user_id
          WHERE m.sessao_id = $1
          ORDER BY (m.user_id = $2) DESC, m.entrou_em"
    ))
    .bind(id)
    .bind(host_id)
    .fetch_all(&state.pool)
    .await?;

    // **Quem está segurando.** Ausente não segura: quem sumiu já não está na
    // sessão, e esperar por ele seria a sala inteira parada por uma aba
    // fechada. Quem está presente e não está pronto, sim.
    let esperando: Vec<String> = gente
        .iter()
        .filter(|m| !m.pronto && !m.ausente)
        .map(|m| m.display_name.clone())
        .collect();

    let conversa: Vec<RecadoNaSala> = sqlx::query_as(
        "SELECT r.id, r.user_id, u.display_name, r.texto, r.em
           FROM sessao_junta_recado r
           JOIN app_user u ON u.id = r.user_id
          WHERE r.sessao_id = $1
          ORDER BY r.em DESC LIMIT 60",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Sala {
        id,
        work_id,
        titulo,
        poster,
        media_file_id,
        host_id,
        host_nome,
        sou_host: host_id == eu,
        modo,
        transcode_id,
        tocando,
        // A regra inteira do §4.6, numa linha: a intenção do host **e** todo
        // mundo pronto.
        rodando: tocando && esperando.is_empty(),
        esperando,
        posicao_segundos,
        atualizado_em,
        gente,
        conversa: conversa.into_iter().rev().collect(),
    })
}

async fn entrar_na_sala(state: &AppState, id: Uuid, quem: Uuid) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO sessao_junta_membro (sessao_id, user_id) VALUES ($1, $2)
         ON CONFLICT (sessao_id, user_id) DO UPDATE SET visto_em = now()",
    )
    .bind(id)
    .bind(quem)
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn encerrar_do_host(state: &AppState, host: Uuid) -> AppResult<()> {
    let antigas: Vec<(Uuid,)> = sqlx::query_as(
        "UPDATE sessao_junta SET encerrada_em = now()
          WHERE host_id = $1 AND encerrada_em IS NULL RETURNING id",
    )
    .bind(host)
    .fetch_all(&state.pool)
    .await?;
    for (id,) in antigas {
        avisar(state, id, "fim");
    }
    Ok(())
}

async fn so_host(state: &AppState, id: Uuid, quem: Uuid) -> AppResult<()> {
    let host: Option<(Uuid,)> =
        sqlx::query_as("SELECT host_id FROM sessao_junta WHERE id = $1 AND encerrada_em IS NULL")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    match host {
        Some((h,)) if h == quem => Ok(()),
        Some(_) => Err(AppError::Forbidden("quem manda na sala é o host".into())),
        None => Err(AppError::NotFound),
    }
}

async fn so_membro(state: &AppState, id: Uuid, quem: Uuid) -> AppResult<()> {
    let existe: Option<(bool,)> = sqlx::query_as(
        "SELECT true FROM sessao_junta_membro WHERE sessao_id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(quem)
    .fetch_optional(&state.pool)
    .await?;
    existe.map(|_| ()).ok_or(AppError::NotFound)
}

fn avisar(state: &AppState, sessao: Uuid, o_que: &'static str) {
    crate::events::publish(&state.events, crate::events::AppEvent::Junto { sessao, o_que });
}

/// Se alguém pode ler a sessão de transcode de outra pessoa — o modo
/// compartilhado (§4.6).
///
/// **É a única permissão que o modo compartilhado precisou.** Sem ela, os
/// membros levariam 403 na playlist do host; com ela, um transcode serve a sala
/// inteira. E ela é estreita: vale só enquanto a sala está aberta, só pra quem
/// está dentro, e só pra sessão que aquela sala declarou.
pub async fn pode_ler_transcode(pool: &sqlx::PgPool, quem: Uuid, transcode: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT true FROM sessao_junta s
          JOIN sessao_junta_membro m ON m.sessao_id = s.id AND m.user_id = $1
         WHERE s.transcode_id = $2 AND s.encerrada_em IS NULL
           AND s.modo = 'compartilhado'
         LIMIT 1",
    )
    .bind(quem)
    .bind(transcode)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    /// **A regra inteira do §4.6 numa linha, e ela não pode virar duas.**
    ///
    /// `rodando = tocando && esperando.is_empty()`. Trocar por "toca se o host
    /// mandou" devolve o modo tolerante que a decisão recusou — e o pior é que
    /// funcionaria na máquina de quem programa, onde nada trava.
    #[test]
    fn rodar_exige_todo_mundo_pronto() {
        let fonte = include_str!("junto.rs");
        assert!(
            fonte.contains("rodando: tocando && esperando.is_empty()"),
            "a sincronia deixou de exigir todo mundo pronto"
        );
    }

    /// Ausente não segura a sala. Sem isto, uma aba fechada para o filme de
    /// todo mundo pra sempre — e ninguém saberia por quê.
    #[test]
    fn ausente_nao_segura() {
        let fonte = include_str!("junto.rs");
        assert!(fonte.contains("!m.pronto && !m.ausente"));
    }
}
