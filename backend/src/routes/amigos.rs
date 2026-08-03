//! R28 — amigos.
//!
//! ## O que isto substitui
//!
//! O **círculo** da R19: um grupo fechado, com dono, do qual se era membro. Ele
//! escopava empréstimo, rotação, nota, feed, convite e acesso — seis coisas
//! penduradas num conceito que ninguém tinha pedido. A palavra das anotações
//! originais é *"uma mini rede social somente com amigos"*, e amizade não é
//! grupo: é uma relação entre **duas** pessoas.
//!
//! A diferença que mais importa não é de vocabulário. Um grupo tem lista de
//! membros comum — se você entra, vê todo mundo e todo mundo te vê. Amizade é
//! por par: a sua lista e a minha são diferentes, e é isso que faz o feed ser
//! *seu* feed.
//!
//! ## Uma linha por amizade, e o banco garante
//!
//! O par é **ordenado pelo uuid** (`a < b`, imposto por CHECK), então
//! `(sam, rudney)` e `(rudney, sam)` são a mesma chave primária. Duas linhas
//! dizendo a mesma coisa, e podendo discordar, são inrepresentáveis.
//!
//! **Quem ordena é o Postgres**, com `least()`/`greatest()`, e não o Rust. Não
//! por preguiça: se o Rust ordenasse, a ordenação que produz a linha e a que o
//! CHECK confere seriam duas implementações da mesma comparação, e o dia em que
//! divergissem o INSERT falharia com erro de constraint sem ninguém entender por
//! quê. Uma comparação, um lugar.
//!
//! ## Pedido e aceite, e o que acontece quando os dois pedem juntos
//!
//! Amizade tem aceite porque a decisão 2.2 do `IDEIAS.md` é forte: amigo vê o
//! que você está assistindo **agora**, o que largou no meio, o que terminou e
//! suas notas. Não há chave de privacidade — então o aceite **é** o
//! consentimento, e ele é o único lugar onde ele acontece.
//!
//! E se os dois se pedirem ao mesmo tempo, viram amigos: o segundo INSERT cai no
//! `ON CONFLICT`, vê que quem pediu foi o outro, e aceita. Não é um caso de
//! borda tratado — é a mesma regra lida de trás pra frente.

use axum::extract::{Path, State};
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::AppState;

/// **Os ids de quem é meu amigo**, aceito, com `$1` sendo eu.
///
/// Mora aqui e é usado pelo feed (`routes/feed.rs`) e pela ficha do filme
/// (`routes/avaliacao.rs`). Uma definição, um lugar — é a mesma razão pela qual
/// a razão de "terminada" (§8f) não é reescrita em cada tela: duas cópias desta
/// consulta divergindo fariam o mural mostrar alguém cuja nota a ficha esconde.
///
/// O `CASE` existe porque a linha é simétrica e não sabe qual dos dois é você.
pub const IDS_DOS_MEUS_AMIGOS: &str = r#"
    SELECT CASE WHEN am.a = $1 THEN am.b ELSE am.a END AS user_id
    FROM amizade am
    WHERE am.aceito_em IS NOT NULL AND (am.a = $1 OR am.b = $1)
"#;

/// Alguém do servidor, visto de onde você está.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Alguem {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    /// R43 — o rosto escolhido, já como arte. Ver `social::Presente`.
    pub rosto: Option<String>,
    /// Desde quando somos amigos, ou desde quando o pedido está parado.
    pub desde: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct MinhasAmizades {
    pub amigos: Vec<Alguem>,
    /// Pedidos que chegaram pra mim, esperando resposta.
    pub recebidos: Vec<Alguem>,
    /// Pedidos que eu mandei e ninguém respondeu ainda.
    pub enviados: Vec<Alguem>,
    /// Quem mais está no servidor e não tem relação nenhuma comigo.
    ///
    /// Vem junto porque hoje o servidor tem **2 contas**: uma tela de busca pra
    /// escolher entre uma pessoa seria cerimônia. Quando houver gente demais pra
    /// caber numa lista, isto vira `?q=` — e é a única coisa desta rota que a
    /// fase 6 (`IDEIAS.md` §3.8) precisa trocar.
    pub no_servidor: Vec<Alguem>,
}

/// Uma linha por conta do servidor, com o estado da relação junto.
#[derive(Debug, sqlx::FromRow)]
struct Linha {
    id: Uuid,
    username: String,
    display_name: String,
    rosto: Option<String>,
    /// `amigo` | `enviado` | `recebido` | `nenhuma`.
    relacao: String,
    desde: chrono::DateTime<chrono::Utc>,
}

/// Quem existe, e o que cada um é seu.
///
/// **Uma consulta pras quatro listas**, e não quatro rotas: as quatro são a
/// mesma pergunta ("quem são as outras pessoas daqui?") com a resposta separada
/// por estado, e quatro idas ao banco poderiam voltar de estados diferentes se
/// alguém aceitasse um pedido no meio.
pub async fn listar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<MinhasAmizades>> {
    let linhas: Vec<Linha> = sqlx::query_as(
        r#"
        SELECT u.id, u.username, u.display_name, pf.avatar AS rosto,
               CASE
                   WHEN am.aceito_em IS NOT NULL      THEN 'amigo'
                   WHEN am.pedido_por = $1            THEN 'enviado'
                   WHEN am.pedido_por IS NOT NULL     THEN 'recebido'
                   ELSE 'nenhuma'
               END                                            AS relacao,
               COALESCE(am.aceito_em, am.pedido_em, u.created_at) AS desde
        FROM app_user u
        LEFT JOIN perfil pf ON pf.user_id = u.id
        LEFT JOIN amizade am
          ON am.a = least(u.id, $1) AND am.b = greatest(u.id, $1)
        -- Você não aparece na sua própria lista, e conta desativada não aparece
        -- na de ninguém: um pedido pra quem não entra mais nunca é respondido.
        WHERE u.id <> $1 AND u.is_active
        ORDER BY u.display_name
        "#,
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;

    let mut r = MinhasAmizades {
        amigos: Vec::new(),
        recebidos: Vec::new(),
        enviados: Vec::new(),
        no_servidor: Vec::new(),
    };

    // A chave do rosto vira arte aqui: o catálogo é código, e SQL nenhum sabe
    // traduzi-lo (R43).
    let arte = crate::enfeites::arte_por_chave(&state.pool).await;

    for l in linhas {
        let quem = Alguem {
            id: l.id,
            username: l.username,
            display_name: l.display_name,
            rosto: l.rosto.as_deref().and_then(|c| arte.get(c).cloned()),
            desde: l.desde,
        };
        match l.relacao.as_str() {
            "amigo" => r.amigos.push(quem),
            "recebido" => r.recebidos.push(quem),
            "enviado" => r.enviados.push(quem),
            _ => r.no_servidor.push(quem),
        }
    }

    Ok(Json(r))
}

/// Pedir amizade — ou aceitar a que te pediram.
///
/// **Um verbo pros dois**, e isso não é economia de rota: do ponto de vista de
/// quem clica, "quero ser seu amigo" é o mesmo gesto nas duas pontas. Quem sabe
/// se é pedido ou aceite é a linha que já está lá, e ela sabe melhor que o
/// cliente — que poderia mandar "aceitar" um pedido cancelado meio segundo
/// antes.
pub async fn pedir(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(outro): Path<Uuid>,
) -> AppResult<Json<Value>> {
    if outro == user.id {
        return Err(AppError::BadRequest("você já se tem".into()));
    }

    // A conta precisa existir e estar ativa. Sem isto, o 404 viria como erro de
    // chave estrangeira — 500 na cara de quem clicou num nome que acabou de
    // sumir da lista.
    let alvo: Option<(String,)> =
        sqlx::query_as("SELECT display_name FROM app_user WHERE id = $1 AND is_active")
            .bind(outro)
            .fetch_optional(&state.pool)
            .await?;
    let Some((nome,)) = alvo else {
        return Err(AppError::NotFound);
    };

    // O `WHERE` do `DO UPDATE` é a regra inteira: só aceita pedido **pendente**
    // e **do outro**. Reenviar o seu próprio não mexe em nada, e um pedido já
    // aceito não é reaceito — o que manteria `aceito_em` andando pra frente e
    // apagaria desde quando vocês são amigos.
    let feito: Option<(bool,)> = sqlx::query_as(
        r#"
        INSERT INTO amizade (a, b, pedido_por)
        VALUES (least($1, $2), greatest($1, $2), $1)
        ON CONFLICT (a, b) DO UPDATE
            SET aceito_em = now()
            WHERE amizade.aceito_em IS NULL AND amizade.pedido_por <> $1
        RETURNING aceito_em IS NOT NULL
        "#,
    )
    .bind(user.id)
    .bind(outro)
    .fetch_optional(&state.pool)
    .await?;

    // Nenhuma linha significa que o `WHERE` recusou, e há dois motivos. Dizer
    // qual é o que separa um "não" de um oráculo mudo — aqui não há segredo a
    // proteger, e a tela precisa da frase certa.
    let Some((amigos,)) = feito else {
        let ja: Option<(bool,)> = sqlx::query_as(
            "SELECT aceito_em IS NOT NULL FROM amizade
             WHERE a = least($1, $2) AND b = greatest($1, $2)",
        )
        .bind(user.id)
        .bind(outro)
        .fetch_optional(&state.pool)
        .await?;

        return Ok(Json(match ja {
            Some((true,)) => json!({ "estado": "amigo", "recado": "vocês já são amigos" }),
            _ => json!({ "estado": "enviado", "recado": format!("{nome} ainda não respondeu") }),
        }));
    };

    tracing::info!(de = %user.username, para = %nome, aceite = amigos, "amizade");

    Ok(Json(if amigos {
        json!({ "estado": "amigo", "recado": format!("agora você e {nome} são amigos") })
    } else {
        json!({ "estado": "enviado", "recado": format!("pedido enviado pra {nome}") })
    }))
}

/// Desfazer: recusar um pedido, cancelar o seu, ou deixar de ser amigo.
///
/// **As três apagam a mesma linha**, e por isso são uma rota só. Separá-las
/// exigiria que o cliente soubesse em qual dos três estados a relação está — e
/// ele só sabe o que leu na última vez que carregou a tela.
///
/// **Recusar não deixa marca.** Não há estado "recusada": guardar a recusa
/// serviria pra impedir um segundo pedido, o que num servidor de duas pessoas
/// resolve um problema que não existe e cria um pior — quem pediu ficaria vendo
/// "pendente" pra sempre sem saber que já levou não.
pub async fn desfazer(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(outro): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let apagada: Option<(bool,)> = sqlx::query_as(
        "DELETE FROM amizade
         WHERE a = least($1, $2) AND b = greatest($1, $2)
         RETURNING aceito_em IS NOT NULL",
    )
    .bind(user.id)
    .bind(outro)
    .fetch_optional(&state.pool)
    .await?;

    let Some((era_amizade,)) = apagada else {
        return Err(AppError::NotFound);
    };

    Ok(Json(json!({
        "desfeita": true,
        "era_amizade": era_amizade,
    })))
}

/// Somos amigos?
///
/// Existe pra quem precisa da resposta e não da lista. Hoje ninguém precisa — o
/// feed e a ficha filtram com `IDS_DOS_MEUS_AMIGOS` dentro da própria consulta,
/// que é uma ida ao banco em vez de duas. Fica porque a fase 6 (mensagem direta,
/// presença) pergunta exatamente isto, e porque escrever a comparação de par
/// canônico uma segunda vez à mão é como o defeito nasce.
#[allow(dead_code)]
pub async fn sao_amigos(pool: &sqlx::PgPool, um: Uuid, outro: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
             SELECT 1 FROM amizade
             WHERE a = least($1, $2) AND b = greatest($1, $2)
               AND aceito_em IS NOT NULL
         )",
    )
    .bind(um)
    .bind(outro)
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O fragmento tem que olhar os **dois lados** da linha. Ele é simétrico, e
    /// uma versão que só checasse `a = $1` daria a metade dos amigos a cada
    /// pessoa — e a metade errada, decidida por qual uuid saiu menor.
    #[test]
    fn o_fragmento_olha_os_dois_lados() {
        assert!(IDS_DOS_MEUS_AMIGOS.contains("am.a = $1"));
        assert!(IDS_DOS_MEUS_AMIGOS.contains("am.b = $1"));
        assert!(IDS_DOS_MEUS_AMIGOS.contains("CASE WHEN am.a = $1 THEN am.b ELSE am.a END"));
    }

    /// **Pedido pendente não é amizade.** Sem este filtro, pedir passaria a
    /// dar acesso ao que só o aceite deveria dar — e como não há chave de
    /// privacidade (`IDEIAS.md` §2.2), o aceite é o consentimento inteiro.
    #[test]
    fn pendente_nao_conta_como_amigo() {
        assert!(IDS_DOS_MEUS_AMIGOS.contains("am.aceito_em IS NOT NULL"));
    }
}
