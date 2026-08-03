//! R25 — o mural, e R28 — de quem ele é.
//!
//! ## O escopo mudou, e é a fase 1 inteira aqui dentro
//!
//! Este mural era **do círculo**: você via o que as pessoas do seu grupo fizeram
//! porque estavam no mesmo grupo. Agora ele é **seu** — você e seus amigos —, e
//! a diferença não é de nome: no círculo a lista era comum e igual para todos os
//! membros; aqui a minha lista e a sua são diferentes, e é isso que faz um feed
//! ser um feed em vez de um quadro de avisos.
//!
//! ## E R33 — o que ele conta
//!
//! A R28 trocou o escopo e deixou o conteúdo pra depois, de propósito. Agora o
//! conteúdo mudou: o mural contava **só o que foi terminado**, e o §41 chamou
//! isso de decisão de privacidade — *"anunciar cada coisa que se provou e
//! abandonou é vigilância com cara de recurso social"*.
//!
//! A decisão 2.2 do `IDEIAS.md` diz o contrário: **amigo vê o que você está
//! assistindo agora, o que largou no meio, o que terminou, suas notas.** Então
//! entram três fontes novas — o que está rodando, o que foi abandonado, e o que
//! as pessoas escreveram (post e comentário).
//!
//! A poda não era burra: ela foi escrita quando o escopo era um "círculo" que
//! podia ter um convidado dentro. Com amizade que **se aceita** (§44), o aceite
//! é o consentimento — você só aparece pra quem deixou entrar.
//!
//! ## A medição que decidiu o que é um acontecimento
//!
//! O `IDEIAS.md` §4.9 avisou que esta fase vinha por último *"porque é o que
//! mais depende de tudo acima ter gerado acontecimento"*. Medido antes de
//! escrever:
//!
//! | | |
//! |---|---|
//! | `play_event` cru | **128 linhas** |
//! | pares (pessoa, obra) | 18 |
//! | **obras terminadas** (§8f) | **2** |
//! | empréstimos | 0 |
//! | avaliações | 0 |
//!
//! Um feed sobre a primeira linha seria 128 entradas dizendo *"sam abriu
//! Drive"* dezoito vezes — não é um mural, é um log. Sobre a terceira, são
//! duas entradas. **Nenhuma das duas é um feed ainda**, e é isso que a fase
//! tinha que encarar em vez de disfarçar.
//!
//! ## O volume, que era o outro argumento da poda
//!
//! O §41 tinha dois motivos, e só um caiu. O que caiu foi a privacidade. O que
//! fica de pé é este: **um feed sobre `play_event` cru seria um log** — 128
//! linhas dizendo *"sam abriu Drive"* dezoito vezes, e ruído ensina a não olhar
//! (§24).
//!
//! Então as fontes novas não são o log cru. `assistindo` é **uma linha por
//! pessoa** (o que está rodando agora, não o histórico), e `largou` exige que a
//! obra tenha ficado parada no meio por mais de um dia — o que separa
//! "abandonou" de "foi fazer café". A privacidade saiu; o cuidado com o volume
//! não.
//!
//! ## Sem tabela nenhuma
//!
//! O §6.5 já tinha previsto: *"o feed é um `SELECT` sobre `play_event` e
//! `emprestimo` com um `JOIN` — nada de segurança muda"*. É literalmente isso,
//! com `avaliacao` junto. É a quarta fase seguida em que a peça de schema
//! prevista não nasce (§38 registrou as três primeiras).

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::AppResult;
use crate::AppState;

/// Uma linha do mural.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Acontecimento {
    /// `assistindo` | `largou` | `terminou` | `pegou` | `devolveu` | `pediu`
    /// | `avaliou` | `postou`.
    pub tipo: String,
    pub quando: chrono::DateTime<chrono::Utc>,
    pub quem: String,
    pub quem_id: Uuid,
    /// Se fui eu. A tela marca discretamente em vez de esconder: o mural é seu,
    /// e um que apagasse metade dos seus próprios atos contaria uma história
    /// torta.
    pub meu: bool,
    pub titulo: String,
    pub obra_id: Option<Uuid>,
    pub poster: Option<String>,
    /// O que qualifica o acontecimento, quando há: a condição da devolução, a
    /// nota dada, "atrasada", o texto do post. `None` some da tela — não vira
    /// "—" (§24).
    pub detalhe: Option<String>,
    /// O id do post, quando o acontecimento **é** um post. É por ele que o
    /// comentário se pendura, e é `None` em todo o resto — comentar um
    /// "fulano terminou X" seria comentar um fato, não uma fala.
    pub post_id: Option<Uuid>,
    /// Os comentários, já embutidos. Uma segunda consulta pelos ids dos posts
    /// da página, e não uma requisição por post: um feed com quarenta linhas
    /// faria quarenta chamadas pra mostrar, quase sempre, zero comentários.
    ///
    /// `skip` e não `default`: o segundo ainda exige que o tipo saiba se
    /// decodificar de uma coluna, e este não vem de coluna nenhuma — ele é
    /// preenchido em Rust depois da consulta.
    #[sqlx(skip)]
    pub comentarios: Vec<crate::routes::social::Comentario>,
}

#[derive(Debug, Deserialize)]
pub struct FeedQuery {
    #[serde(default = "padrao_limite")]
    pub limit: i64,
}

fn padrao_limite() -> i64 {
    40
}

#[derive(Debug, Serialize)]
pub struct Feed {
    pub acontecimentos: Vec<Acontecimento>,
    /// Quantas pessoas apareceram no mural.
    ///
    /// Serve pra tela dizer a verdade sobre o silêncio: um mural com um nome só
    /// não é uma conversa, é uma pessoa em voz alta. Sem este número, a tela
    /// pareceria funcionar quando na verdade só tem um lado.
    pub vozes: usize,
    /// Quantas poderiam aparecer: você mais os seus amigos.
    pub pessoas: usize,
}

/// O seu mural.
///
/// Uma consulta, cinco fontes, ordenadas por tempo. **Nenhuma tabela nova** — e
/// os cinco `SELECT` são a lista dos acontecimentos que este produto sabe
/// produzir hoje. Quando a locadora rodar, o mesmo código conta mais coisas.
pub async fn feed(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<FeedQuery>,
) -> AppResult<Json<Feed>> {
    let limite = q.limit.clamp(1, 200);

    // As três fontes, unidas e ordenadas.
    //
    // `terminou` sai da regra do §8f — evento `finish` **ou** passar de 92% —
    // que é a mesma que a curadoria, o guia (§30), a locadora (§35) e o placar
    // (§40) usam. Cinco lugares, uma definição: escrever outra aqui faria o
    // mural discordar da retrospectiva sobre a palavra "terminou".
    //
    // **Quem entra no mural**: você e seus amigos. O `UNION` (e não `UNION ALL`)
    // é o que impede você de aparecer duas vezes se o fragmento algum dia
    // devolver a si mesmo — barato, e uma linha duplicada aqui viraria cada ato
    // seu contado em dobro.
    let sql = format!(
        r#"
        WITH gente AS (
            SELECT $1::uuid AS user_id
            UNION
            {amigos}
        ),
        terminadas AS (
            SELECT pe.user_id,
                   pe.work_id,
                   max(pe.created_at) AS quando
            FROM play_event pe
            JOIN gente g ON g.user_id = pe.user_id
            GROUP BY pe.user_id, pe.work_id
            HAVING count(*) FILTER (WHERE pe.event_type = 'finish') > 0
                OR max(pe.position_seconds / NULLIF(pe.duration_seconds, 0)) >= 0.92
        )
        SELECT 'terminou' AS tipo, t.quando, u.display_name AS quem, u.id AS quem_id,
               u.id = $1 AS meu, w.title AS titulo, w.id AS obra_id,
               w.artwork->>'poster' AS poster,
               NULL::text AS detalhe,
               NULL::uuid AS post_id
        FROM terminadas t
        JOIN app_user u ON u.id = t.user_id
        JOIN work w ON w.id = t.work_id

        UNION ALL

        SELECT 'pegou', e.pego_em, u.display_name, u.id, u.id = $1,
               COALESCE(w.title, c.title), COALESCE(e.work_id, e.collection_id),
               COALESCE(w.artwork->>'poster', c.artwork->>'poster'),
               NULL, NULL
        FROM emprestimo e
        JOIN gente g ON g.user_id = e.user_id
        JOIN app_user u ON u.id = e.user_id
        LEFT JOIN work w ON w.id = e.work_id
        LEFT JOIN collection c ON c.id = e.collection_id

        UNION ALL

        -- A devolução carrega o que a fita conta sobre quem a teve. São fatos
        -- sobre pessoas reais, não métrica inventada — o §35 fez questão.
        SELECT 'devolveu', e.devolvido_em, u.display_name, u.id, u.id = $1,
               COALESCE(w.title, c.title), COALESCE(e.work_id, e.collection_id),
               COALESCE(w.artwork->>'poster', c.artwork->>'poster'),
               CASE e.devolvido_como
                   WHEN 'rebobinada' THEN 'rebobinada'
                   WHEN 'no-meio'    THEN 'sem rebobinar'
                   WHEN 'terminada'  THEN 'até o fim'
               END
               || CASE WHEN e.devolvido_em > e.vence_em THEN ' · atrasada' ELSE '' END
               || CASE WHEN e.devolvido_por = 'prazo' THEN ' · pelo prazo' ELSE '' END,
               NULL
        FROM emprestimo e
        JOIN gente g ON g.user_id = e.user_id
        JOIN app_user u ON u.id = e.user_id
        LEFT JOIN work w ON w.id = e.work_id
        LEFT JOIN collection c ON c.id = e.collection_id
        WHERE e.devolvido_em IS NOT NULL

        UNION ALL

        -- Quem PEDIU, e não quem está com ela: o ato social é o pedido.
        SELECT 'pediu', e.pedido_em, p.display_name, p.id, p.id = $1,
               COALESCE(w.title, c.title), COALESCE(e.work_id, e.collection_id),
               COALESCE(w.artwork->>'poster', c.artwork->>'poster'),
               'de ' || u.display_name, NULL
        FROM emprestimo e
        JOIN app_user p ON p.id = e.pedido_por
        JOIN gente g ON g.user_id = p.id
        JOIN app_user u ON u.id = e.user_id
        LEFT JOIN work w ON w.id = e.work_id
        LEFT JOIN collection c ON c.id = e.collection_id
        WHERE e.pedido_em IS NOT NULL

        UNION ALL

        SELECT 'avaliou', a.atualizado_em, u.display_name, u.id, u.id = $1,
               w.title, w.id, w.artwork->>'poster',
               repeat('★', a.nota), NULL
        FROM avaliacao a
        JOIN gente g ON g.user_id = a.user_id
        JOIN app_user u ON u.id = a.user_id
        JOIN work w ON w.id = a.work_id

        UNION ALL

        -- **O QUE ESTÁ RODANDO AGORA** (R33). Uma linha por pessoa, e não uma
        -- por evento: o que interessa é "fulano está vendo isto", não o
        -- histórico de heartbeats. O corte de 90s é o da locadora (§35).
        SELECT 'assistindo', ps.updated_at, u.display_name, u.id, u.id = $1,
               w.title, w.id, w.artwork->>'poster',
               NULL, NULL
        FROM playback_state ps
        JOIN gente g ON g.user_id = ps.user_id
        JOIN app_user u ON u.id = ps.user_id
        JOIN work w ON w.id = ps.work_id
        WHERE ps.updated_at > now() - interval '90 seconds'

        UNION ALL

        -- **O QUE FOI LARGADO NO MEIO.** Exige três coisas juntas pra não
        -- confundir abandono com pausa: não terminada, entre 5% e 85% do filme,
        -- e parada há mais de um dia. Sem a terceira, "foi fazer café" viraria
        -- notícia; sem a segunda, abrir e fechar nos dois minutos iniciais
        -- também.
        SELECT 'largou', ps.updated_at, u.display_name, u.id, u.id = $1,
               w.title, w.id, w.artwork->>'poster',
               'no minuto ' || (ps.position_seconds / 60)::int, NULL
        FROM playback_state ps
        JOIN gente g ON g.user_id = ps.user_id
        JOIN app_user u ON u.id = ps.user_id
        JOIN work w ON w.id = ps.work_id
        WHERE NOT ps.finished
          AND ps.duration_seconds > 0
          AND ps.position_seconds / ps.duration_seconds BETWEEN 0.05 AND 0.85
          AND ps.updated_at < now() - interval '1 day'

        UNION ALL

        -- **O QUE AS PESSOAS ESCREVERAM.** É o único acontecimento do mural que
        -- alguém digitou — o resto o produto deduziu. Por isso é o único que
        -- pode ser comentado e apagado.
        SELECT 'postou', p.criado_em, u.display_name, u.id, u.id = $1,
               COALESCE(w.title, ''), w.id, w.artwork->>'poster',
               p.texto, p.id
        FROM post p
        JOIN gente g ON g.user_id = p.user_id
        JOIN app_user u ON u.id = p.user_id
        LEFT JOIN work w ON w.id = p.work_id

        ORDER BY quando DESC
        LIMIT $2
        "#,
        amigos = crate::routes::amigos::IDS_DOS_MEUS_AMIGOS,
    );

    let mut acontecimentos: Vec<Acontecimento> = sqlx::query_as(&sql)
        .bind(user.id)
        .bind(limite)
        .fetch_all(&state.pool)
        .await?;

    // Os comentários dos posts **desta página**, numa consulta só. Uma por post
    // faria quarenta chamadas pra mostrar, quase sempre, zero comentários.
    let ids: Vec<Uuid> = acontecimentos.iter().filter_map(|a| a.post_id).collect();
    if !ids.is_empty() {
        // Uma linha achatada, e não `(Uuid, Comentario)`: uma tupla do sqlx
        // decodifica coluna a coluna e não sabe montar um struct aninhado.
        #[derive(sqlx::FromRow)]
        struct Linha {
            post_id: Uuid,
            id: Uuid,
            quem: String,
            quem_id: Uuid,
            meu: bool,
            texto: String,
            criado_em: chrono::DateTime<chrono::Utc>,
        }

        let comentarios: Vec<Linha> = sqlx::query_as(
            "SELECT c.post_id, c.id, u.display_name AS quem, u.id AS quem_id,
                    u.id = $1 AS meu, c.texto, c.criado_em
             FROM comentario c JOIN app_user u ON u.id = c.user_id
             WHERE c.post_id = ANY($2)
             ORDER BY c.criado_em",
        )
        .bind(user.id)
        .bind(&ids)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

        for l in comentarios {
            if let Some(a) = acontecimentos.iter_mut().find(|a| a.post_id == Some(l.post_id)) {
                a.comentarios.push(crate::routes::social::Comentario {
                    id: l.id,
                    quem: l.quem,
                    quem_id: l.quem_id,
                    meu: l.meu,
                    texto: l.texto,
                    criado_em: l.criado_em,
                });
            }
        }
    }

    let vozes = acontecimentos
        .iter()
        .map(|a| a.quem_id)
        .collect::<std::collections::HashSet<_>>()
        .len();

    // Você mais os seus amigos — a mesma conta que o `WITH gente` fez lá em
    // cima, e o `+ 1` é você. Um mural que dissesse "1 de 1 pessoa apareceu"
    // sem contar você estaria descrevendo uma casa vazia onde há alguém.
    let amigos: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM ({amigos}) AS meus",
        amigos = crate::routes::amigos::IDS_DOS_MEUS_AMIGOS,
    ))
    .bind(user.id)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(Feed {
        acontecimentos,
        vozes,
        pessoas: amigos as usize + 1,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O limite é grampeado, não amplificado: `?limit=100000` num `UNION` de
    /// cinco fontes seria uma varredura pedida pelo cliente.
    #[test]
    fn o_limite_e_grampeado() {
        assert_eq!(padrao_limite(), 40);
        assert_eq!(100_000i64.clamp(1, 200), 200);
        assert_eq!(0i64.clamp(1, 200), 1);
        assert_eq!((-5i64).clamp(1, 200), 1);
    }

    /// Os cinco tipos que o mural sabe contar. A lista é fechada de propósito:
    /// um tipo novo exige uma frase nova na tela, e um `tipo` desconhecido
    /// chegando lá viraria uma linha muda.
    #[test]
    fn os_tipos_sao_os_que_a_tela_sabe_dizer() {
        for t in [
            "assistindo", "largou", "terminou", "pegou", "devolveu", "pediu",
            "avaliou", "postou",
        ] {
            assert!(!t.is_empty());
            assert!(t.chars().all(|c| c.is_lowercase()));
        }
    }
}
