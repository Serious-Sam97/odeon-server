//! R18 — o guia de cinema.
//!
//! A tese, numa frase: **um guia que qualquer site tem é a Wikipédia com passos
//! extras; um que cruza o cânone com o SEU acervo e o SEU histórico não existe
//! em lugar nenhum.**
//!
//! Por isso nenhuma resposta daqui é biografia. Toda pessoa vem com três coisas
//! ao mesmo tempo: quem é, **o que disso você tem**, e **o que você fez com
//! isso**. As duas últimas saem do `credit` (§8h) e do `playback_state` (M0) —
//! nenhuma tabela nova, nenhuma migração.
//!
//! Três cortes que definem o que aparece, todos medidos antes de escritos:
//!
//! * **Só obra identificada.** Mesma regra que a R15 (§26) aplicou à curadoria:
//!   recomendar — ou catalogar — exige saber o que se está mostrando. Material
//!   sem match se resolve na fila de revisão, que é onde ele mora.
//! * **Mínimo de obras por pessoa**, padrão 2. Página de uma obra só não é
//!   página. É o mesmo número da afinidade por pessoa do §8h, por um motivo
//!   diferente — lá, uma obra só faria o elenco inteiro virar gosto favorito;
//!   aqui, faria 9.634 páginas vazias. Medido: 418 diretores no acervo, **134
//!   com duas obras ou mais**.
//! * **Produção fica de fora dos eixos.** São 45.741 créditos contra 1.191 de
//!   direção; um eixo de produção enterraria os outros em assistente de efeitos.
//!   A allowlist do §8h já tinha tomado essa decisão uma vez.
//!
//! Gênero e década contam **filmes** (`kind = 'movie'`), não o acervo inteiro.
//! Num acervo com 14.657 episódios contra 635 filmes, contar tudo faria "Drama"
//! significar "uma série longa que eu tenho", e é um guia de cinema.

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::AppResult;
use crate::AppState;

/// Uma pessoa como o guia a vê: o que ela fez, o que disso está aqui, e o que
/// você fez com isso.
///
/// `terminadas` e `comecadas` são por usuário — duas pessoas da casa abrindo a
/// mesma página veem números diferentes, e é isso que faz a página ser sua.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PessoaDoGuia {
    pub id: Uuid,
    pub name: String,
    pub image_path: Option<String>,
    /// "Directing", "Acting"… o que a pessoa faz predominantemente (0007).
    pub known_for: Option<String>,
    /// Obras **desta pessoa neste acervo**, não a filmografia dela no mundo.
    pub obras: i64,
    pub terminadas: i64,
    pub comecadas: i64,
    /// Até quatro pôsteres pra capa empilhada, no mesmo espírito do cartão de
    /// coleção da R4.
    pub posters: Option<Vec<String>>,
    /// Quantas pessoas existem com este papel e este mínimo — pra paginação
    /// dizer "40 de 134" em vez de "40". Mesma correção da R3 (§14).
    pub total: i64,
}

/// Um eixo que não é pessoa: gênero ou década.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FaixaDoGuia {
    /// O que a UI mostra: "Ficção científica", "1990".
    pub rotulo: String,
    /// O que a UI manda pro filtro da biblioteca: `genre:Terror` ou o ano.
    pub chave: String,
    pub obras: i64,
    pub posters: Option<Vec<String>>,
}

/// As pessoas de um papel, com o seu histórico junto.
///
/// O `LEFT JOIN` com `playback_state` é o que distingue este guia de uma lista
/// de créditos: sem ele a resposta seria a mesma pra todo mundo.
///
/// **A unidade é o título, não a obra** — e isso foi medido, não suposto. A
/// primeira versão contava `work` e o ranking de direção saiu assim:
///
/// ```text
/// Enrique Segoviano  221 obras   (Chaves e Chapolin)
/// Joseph Barbera     166 obras   (Tom & Jerry)
/// ```
///
/// São episódios. Um guia de cinema cujo diretor mais importante dirigiu 221
/// episódios de sitcom não é um guia de cinema — é a mesma falha que a R3 (§14)
/// corrigiu na biblioteca, aparecendo num eixo novo. O próprio dado denunciava:
/// quem tinha 221 obras tinha **um** pôster distinto, porque desde a R9 (§21) o
/// episódio herda a arte da série.
///
/// Então o rollup daqui é o mesmo de `/api/library` (episódio → temporada →
/// série), e vale a regra que a locadora já usava: **uma série é uma caixa, não
/// vinte e uma** (§20).
///
/// `terminados` conta título em que você terminou **alguma coisa** — o filme,
/// ou ao menos um episódio. Exigir a série inteira faria a coluna dizer zero
/// pra sempre em quem tem 221 episódios, e a pergunta que a tela faz é "eu
/// conheço o trabalho desta pessoa?", não "eu completei o box?".
///
/// **"Terminada" é a definição do M5, não o booleano.** A primeira versão lia
/// `playback_state.finished`, que está zerado nas 16 linhas deste acervo —
/// enquanto a tela "para você", ao lado, anunciava *2 obras terminadas*. Duas
/// telas do mesmo produto discordando sobre a mesma palavra é pior que as duas
/// erradas do mesmo jeito.
///
/// Quem manda é o §8f: terminada é `event_type = 'finish'` **ou** ter passado
/// de 92% da duração. Neste acervo não existe um único evento `finish` — os
/// dois casos vêm todos da razão, o que é exatamente por que o M5 tem as duas
/// metades. O booleano continua no OR: se um dia ele for gravado, vale.
const PESSOAS_SQL: &str = r#"
WITH visto AS (
    -- Agregado UMA vez pro usuário, não por crédito. O `play_event` deste
    -- acervo tem 116 linhas; correlacionar por obra dentro de 73.507 créditos
    -- de elenco custaria caro pra ler as mesmas 12 obras de novo.
    SELECT pe.work_id,
           max(pe.position_seconds / NULLIF(pe.duration_seconds, 0)) AS razao,
           count(*) FILTER (WHERE pe.event_type = 'finish')          AS finais
    FROM play_event pe
    WHERE pe.user_id = $2
    GROUP BY pe.work_id
),
titulo AS (
    SELECT
        c.person_id,
        COALESCE(g.grupo_id, w.id)                          AS titulo_id,
        -- O pôster é AGREGADO, nunca chave de agrupamento. Pô-lo no GROUP BY
        -- parte o título: uma série sem arte própria faz cada episódio cair no
        -- pôster dele mesmo, e um diretor de 43 episódios de UMA série voltava
        -- como 43 títulos. (A série de AniList é justamente a que a R9 não
        -- conseguiu enriquecer — §21 — então o caso existe neste acervo.)
        max(COALESCE(g.grupo_poster, w.artwork->>'poster')) AS poster,
        bool_or(
            COALESCE(ps.finished, false)
            OR COALESCE(v.finais, 0) > 0
            OR COALESCE(v.razao, 0) >= 0.92
        ) AS terminou,
        bool_or(
            COALESCE(ps.position_seconds, 0) > 0 OR COALESCE(v.razao, 0) > 0
        ) AS abriu
    FROM credit c
    JOIN work w ON w.id = c.work_id
    LEFT JOIN visto v ON v.work_id = w.id
    LEFT JOIN LATERAL (
        SELECT COALESCE(series.id, season.id)              AS grupo_id,
               COALESCE(series.artwork->>'poster',
                        season.artwork->>'poster')          AS grupo_poster
        FROM collection_item ci
        JOIN collection season ON season.id = ci.collection_id
        LEFT JOIN collection series ON series.id = season.parent_id
        WHERE ci.work_id = w.id AND season.kind IN ('season', 'series')
        LIMIT 1
    ) g ON true
    LEFT JOIN playback_state ps ON ps.work_id = w.id AND ps.user_id = $2
    WHERE c.role = $1
      AND w.match_state IN ('auto', 'confirmed')
    GROUP BY c.person_id, COALESCE(g.grupo_id, w.id)
)
SELECT
    p.id, p.name, p.image_path, p.known_for,
    count(*)                                   AS obras,
    count(*) FILTER (WHERE t.terminou)         AS terminadas,
    count(*) FILTER (WHERE t.abriu AND NOT t.terminou) AS comecadas,
    (array_remove(array_agg(DISTINCT t.poster), NULL))[1:4] AS posters,
    count(*) OVER ()                           AS total
FROM person p
JOIN titulo t ON t.person_id = p.id
WHERE ($3::text IS NULL OR p.name ILIKE '%' || $3 || '%')
GROUP BY p.id
HAVING count(*) >= $4
ORDER BY obras DESC, p.name
LIMIT $5 OFFSET $6
"#;

async fn buscar_pessoas(
    state: &AppState,
    user_id: Uuid,
    role: &str,
    q: Option<&str>,
    minimo: i64,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<PessoaDoGuia>> {
    Ok(sqlx::query_as::<_, PessoaDoGuia>(PESSOAS_SQL)
        .bind(role)
        .bind(user_id)
        .bind(q)
        .bind(minimo)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.pool)
        .await?)
}

#[derive(Debug, Deserialize)]
pub struct GuiaQuery {
    /// `director` (padrão), `actor`, `composer`, `writer`…
    pub role: Option<String>,
    pub q: Option<String>,
    /// Sobrescreve o mínimo do papel. Sem isto, vale `minimo_de`.
    pub min: Option<i64>,
    #[serde(default = "quarenta")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn quarenta() -> i64 {
    40
}

/// Quantas obras uma pessoa precisa ter pra sustentar uma página.
///
/// **Mora aqui e em nenhum outro lugar.** A primeira versão tinha o número
/// escrito duas vezes — 3 na capa e 2 na lista — e o efeito foi um botão que
/// mentia: ele dizia *"ver as 644"* e abria uma lista de **1.424**. Um total
/// que muda ao atravessar o próprio botão é pior que um total ausente.
///
/// Elenco pede 3 porque é o eixo com mais gente por natureza (73.507 créditos,
/// contra 1.191 de direção): com 2, a lista enche de quem apareceu em dois
/// episódios. Os demais ficam em 2, que é o mínimo pra existir página — ver o
/// cabeçalho do módulo.
fn minimo_de(role: &str) -> i64 {
    match role {
        "actor" => 3,
        _ => 2,
    }
}

/// A lista de um eixo de pessoa, paginada.
pub async fn pessoas(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<GuiaQuery>,
) -> AppResult<Json<Vec<PessoaDoGuia>>> {
    let role = params.role.as_deref().unwrap_or("director");
    let q = params.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    Ok(Json(
        buscar_pessoas(
            &state,
            user.id(),
            role,
            q,
            params.min.unwrap_or_else(|| minimo_de(role)).clamp(1, 50),
            params.limit.clamp(1, 200),
            params.offset.max(0),
        )
        .await?,
    ))
}

const GENEROS_SQL: &str = r#"
SELECT
    t.value AS rotulo,
    t.namespace || ':' || t.value AS chave,
    count(DISTINCT w.id) AS obras,
    (array_remove(array_agg(DISTINCT w.artwork->>'poster'), NULL))[1:4] AS posters
FROM tag t
JOIN work_tag wt ON wt.tag_id = t.id
JOIN work w ON w.id = wt.work_id
WHERE t.namespace = 'genre'
  AND w.kind = 'movie'
  AND w.match_state IN ('auto', 'confirmed')
GROUP BY t.id, t.namespace, t.value
ORDER BY obras DESC, rotulo
"#;

/// O eixo que a R18 não pôde ter, e a R22 (§38) destravou.
///
/// A §30 registrou: *"Região. Conferido em `metadata/tmdb.rs`: país, idioma,
/// empresa e orçamento não têm coluna."* Agora têm — não coluna, **tag**, o
/// que é melhor: este SQL é o de gênero com um `namespace` diferente, e o
/// filtro de `/api/works` já sabia resolvê-lo desde o M2.
///
/// **O corte de 2 obras é o mesmo do §8h**, e ele importa aqui: dos 33 países,
/// 10 têm um filme só. Um país com uma obra não é uma prateleira, é uma linha
/// de tabela — e dez delas empurrariam pra fora os 23 que rendem.
///
/// O que este eixo **não** faz é esconder o topo. Estados Unidos tem 491 dos
/// 548, e omiti-lo pra "melhorar" o eixo seria mentir por omissão. Quem diz a
/// verdade sobre a forma do acervo é a legenda da tela, que traz o número que
/// faz o eixo valer a pena: **54 filmes fora dos Estados Unidos**.
const PAISES_SQL: &str = r#"
SELECT
    t.value AS rotulo,
    t.namespace || ':' || t.value AS chave,
    count(DISTINCT w.id) AS obras,
    (array_remove(array_agg(DISTINCT w.artwork->>'poster'), NULL))[1:4] AS posters
FROM tag t
JOIN work_tag wt ON wt.tag_id = t.id
JOIN work w ON w.id = wt.work_id
WHERE t.namespace = 'country'
  AND w.kind = 'movie'
  AND w.match_state IN ('auto', 'confirmed')
GROUP BY t.id, t.namespace, t.value
HAVING count(DISTINCT w.id) >= 2
ORDER BY obras DESC, rotulo
"#;

/// Década é `(ano / 10) * 10` — divisão inteira no Postgres, sem `date_trunc`,
/// porque `work.year` é um `int` e não uma data.
const DECADAS_SQL: &str = r#"
SELECT
    ((w.year / 10) * 10)::text AS rotulo,
    ((w.year / 10) * 10)::text AS chave,
    count(*) AS obras,
    (array_remove(array_agg(DISTINCT w.artwork->>'poster'), NULL))[1:4] AS posters
FROM work w
WHERE w.year IS NOT NULL
  AND w.kind = 'movie'
  AND w.match_state IN ('auto', 'confirmed')
GROUP BY 1, 2
ORDER BY rotulo DESC
"#;

/// A capa do guia: um punhado de cada eixo, com o suficiente pra desenhar as
/// prateleiras sem uma segunda ida ao servidor.
///
/// Uma requisição e não seis: a locadora da R8 já paga 12 requisições por
/// visita (§20), e repetir aquilo aqui seria repetir um custo que o próprio doc
/// registrou como o teto do aceitável.
pub async fn eixos(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Value>> {
    let uid = user.id();

    // Direção primeiro porque é onde a cobertura é total: 548 dos 548 filmes
    // identificados têm diretor. Trilha vem junto porque são 5.879 créditos de
    // compositor que nunca tiveram tela nenhuma.
    // O mínimo vem de `minimo_de` nos três, e é o mesmo que a lista completa
    // usa — senão o "ver as N" da capa abriria uma lista de outro tamanho.
    let mut eixos = Vec::new();
    for role in ["director", "actor", "composer"] {
        eixos.push(buscar_pessoas(&state, uid, role, None, minimo_de(role), 12, 0).await?);
    }
    let trilha = eixos.pop().unwrap();
    let elenco = eixos.pop().unwrap();
    let direcao = eixos.pop().unwrap();

    let generos = sqlx::query_as::<_, FaixaDoGuia>(GENEROS_SQL)
        .fetch_all(&state.pool)
        .await?;
    let decadas = sqlx::query_as::<_, FaixaDoGuia>(DECADAS_SQL)
        .fetch_all(&state.pool)
        .await?;
    let paises = sqlx::query_as::<_, FaixaDoGuia>(PAISES_SQL)
        .fetch_all(&state.pool)
        .await?;

    // O número que faz o eixo de região valer a pena.
    //
    // Sozinha, a lista de países diz "Estados Unidos 491" e o resto vira
    // rodapé. Este contador é a pergunta que ninguém conseguia fazer antes da
    // R22 — *"o que eu tenho que não é de Hollywood?"* — e ele fica na legenda
    // da seção, ao lado da lista, em vez de escondê-la.
    let fora_de_hollywood: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM work w
         WHERE w.kind = 'movie' AND w.match_state IN ('auto', 'confirmed')
           AND EXISTS (SELECT 1 FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
                        WHERE wt.work_id = w.id AND t.namespace = 'country')
           AND NOT EXISTS (SELECT 1 FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
                            WHERE wt.work_id = w.id AND t.namespace = 'country'
                              AND t.value = 'Estados Unidos')",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);

    Ok(Json(json!({
        "direcao": direcao,
        "elenco": elenco,
        "trilha": trilha,
        "generos": generos,
        "decadas": decadas,
        "paises": paises,
        "fora_de_hollywood": fora_de_hollywood,
    })))
}
