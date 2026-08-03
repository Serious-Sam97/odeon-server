//! R23 — a nota e a resenha.
//!
//! A fase inteira foi decidida por uma frase do `IDEIAS.md` §4.6: **sinal fraco
//! não manda no forte.** O M5 nasceu de *"nada é declarado"* e *"terminar >
//! assistir"*, porque nota é enviesada — as pessoas dão 5 estrelas pro que
//! acham que **deveriam** gostar. Deixar a nota mandar mais que o comportamento
//! desfaria o M5 inteiro.
//!
//! Então a nota entra na curadoria com peso limitado, e o limite é numérico:
//! `PESO_DA_NOTA` em `curation/taste.rs`, escolhido como **o maior valor que
//! não inverte nada**. Cinco estrelas no que você abandonou aos oito minutos
//! continua contando como rejeição. Há teste.
//!
//! ## A nota de quem você conhece
//!
//! O que este módulo faz além do óbvio: a ficha mostra **as notas dos seus
//! amigos**, não uma média global. Um número de gente que você conhece diz algo;
//! a média de estranhos é o IMDb com passos extras, e disso o mundo já tem.
//!
//! Era "do seu círculo" até a R28, e a troca muda o resultado de verdade: no
//! círculo você via a nota de quem entrou no mesmo grupo que você, quisesse ou
//! não. Agora você vê a de quem aceitou ser seu amigo — e como não há chave de
//! privacidade (`IDEIAS.md` §2.2), esse aceite é o consentimento inteiro.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::AppState;

/// Quantos caracteres uma resenha comporta.
///
/// Não é limite de banco (a coluna é `text`), é limite de tela: acima disso a
/// ficha vira um blog. Duas mil dão uns quatro parágrafos, que é mais do que
/// qualquer um escreve sobre um filme que acabou de ver.
const MAX_TEXTO: usize = 2000;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AvaliacaoDeAlguem {
    pub user_id: Uuid,
    pub quem: String,
    pub nota: i32,
    pub texto: Option<String>,
    pub atualizado_em: chrono::DateTime<chrono::Utc>,
    /// Se sou eu. A tela usa pra saber qual linha é editável sem comparar ids.
    pub meu: bool,
}

#[derive(Debug, Serialize)]
pub struct AvaliacoesDaObra {
    pub minha: Option<AvaliacaoDeAlguem>,
    /// As dos seus amigos.
    pub de_amigos: Vec<AvaliacaoDeAlguem>,
    /// Média sua e dos seus amigos. `None` quando ninguém avaliou —
    /// e aí a seção some, em vez de mostrar um traço (§24).
    pub media: Option<f32>,
    pub quantas: usize,
}

#[derive(Debug, Deserialize)]
pub struct NovaAvaliacao {
    pub nota: i32,
    pub texto: Option<String>,
}

/// As avaliações de uma obra: a sua e a dos seus amigos.
pub async fn listar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(work_id): Path<Uuid>,
) -> AppResult<Json<AvaliacoesDaObra>> {
    // A sua **e** a de quem é seu amigo. Sem o `a.user_id = $1` do meio, a sua
    // própria nota sumiria da sua ficha no dia em que você não tivesse amigo
    // nenhum — que é o estado de qualquer conta recém-criada.
    //
    // O escopo é o fragmento de `amigos.rs`, e não uma cópia dele: se a ficha e
    // o mural discordassem sobre quem é seu amigo, um deles mostraria alguém
    // que o outro esconde.
    let sql = format!(
        "SELECT a.user_id, u.display_name AS quem, a.nota, a.texto, a.atualizado_em,
                a.user_id = $1 AS meu
         FROM avaliacao a
         JOIN app_user u ON u.id = a.user_id
         WHERE a.work_id = $2
           AND (a.user_id = $1 OR a.user_id IN ({amigos}))
         ORDER BY a.atualizado_em DESC",
        amigos = crate::routes::amigos::IDS_DOS_MEUS_AMIGOS,
    );

    let linhas: Vec<AvaliacaoDeAlguem> = sqlx::query_as(&sql)
        .bind(user.id)
        .bind(work_id)
        .fetch_all(&state.pool)
        .await?;

    let quantas = linhas.len();
    let media = (quantas > 0)
        .then(|| linhas.iter().map(|a| a.nota as f32).sum::<f32>() / quantas as f32);

    let (minhas, outras): (Vec<_>, Vec<_>) = linhas.into_iter().partition(|a| a.meu);

    Ok(Json(AvaliacoesDaObra {
        minha: minhas.into_iter().next(),
        de_amigos: outras,
        media,
        quantas,
    }))
}

/// Dar ou trocar a nota.
///
/// `PUT` e não `POST`: avaliar duas vezes é **trocar de ideia**, não criar uma
/// segunda avaliação. A chave primária `(user_id, work_id)` já dizia isso; o
/// verbo passa a dizer também.
pub async fn salvar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(work_id): Path<Uuid>,
    Json(nova): Json<NovaAvaliacao>,
) -> AppResult<Json<Value>> {
    if !(1..=5).contains(&nova.nota) {
        return Err(AppError::BadRequest("a nota vai de 1 a 5".into()));
    }

    // Texto em branco é ausência de texto, não texto vazio. Sem isto a ficha
    // renderizaria um parágrafo de nada abaixo das estrelas.
    let texto = nova
        .texto
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().take(MAX_TEXTO).collect::<String>());

    // `atualizado_em` é reescrito e `criado_em` não: trocar a nota não apaga
    // quando você viu o filme pela primeira vez, e a R24 vai querer os dois.
    let feito = sqlx::query(
        "INSERT INTO avaliacao (user_id, work_id, nota, texto)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (user_id, work_id) DO UPDATE
           SET nota = EXCLUDED.nota,
               texto = EXCLUDED.texto,
               atualizado_em = now()",
    )
    .bind(user.id)
    .bind(work_id)
    .bind(nova.nota)
    .bind(texto.as_deref())
    .execute(&state.pool)
    .await;

    if let Err(e) = feito {
        // A obra pode ter sumido entre abrir a ficha e avaliar. Uma violação de
        // chave estrangeira aqui é 404, não 500 — o §8b chama errar em silêncio
        // de defeito, e errar com o código errado é a versão barulhenta disso.
        if matches!(&e, sqlx::Error::Database(db) if db.code().as_deref() == Some("23503")) {
            return Err(AppError::NotFound);
        }
        return Err(e.into());
    }

    // R35: avaliar e resenhar são desafios possíveis. Conferir aqui é o que
    // faz o desafio fechar no gesto que o cumpriu, em vez de na próxima vez que
    // a pessoa der play em alguma coisa.
    crate::desafios::conferir(&state.pool, user.id).await;
    crate::conquistas::avaliar(&state.pool, user.id).await;

    Ok(Json(json!({ "ok": true })))
}

/// Tirar a nota.
///
/// Existe porque "não sei mais o que achei" é um estado legítimo, e porque a
/// alternativa — deixar a pessoa presa numa nota antiga — faria a nota deixar
/// de ser dada.
pub async fn apagar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(work_id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let n = sqlx::query("DELETE FROM avaliacao WHERE user_id = $1 AND work_id = $2")
        .bind(user.id)
        .bind(work_id)
        .execute(&state.pool)
        .await?
        .rows_affected();

    Ok(Json(json!({ "apagada": n > 0 })))
}

#[cfg(test)]
mod tests {
    /// O limite do texto é de tela, não de banco — e existe pra ficha não
    /// virar blog. Se alguém mexer nele, que seja de propósito.
    #[test]
    fn o_limite_do_texto_cabe_numa_ficha() {
        assert_eq!(super::MAX_TEXTO, 2000);
        // Quatro parágrafos generosos, e não um capítulo.
        assert!(super::MAX_TEXTO < 10_000);
    }
}
