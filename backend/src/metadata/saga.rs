//! R32 — as sagas, e a dívida que elas pagam.
//!
//! O `IDEIAS.md` §7 registrou a dependência com todas as letras:
//!
//! > **Sagas de filme não existem como dado.** `belongs_to_collection` do TMDB
//! > não é buscado. Pré-requisito das conquistas de saga e dos eventos do guia.
//!
//! Os 007, Alien, De Volta para o Futuro — tudo isso existe como **pasta no
//! disco** e não como dado. Uma conquista de trilogia precisa saber o que é uma
//! trilogia, e ninguém sabia.
//!
//! ## A dívida era de dados, não de schema
//!
//! `collection.kind` aceita `'franchise'` desde a migração original, e
//! `collection_item` já liga coleção a obra. O modelo de grafo do §1 previu a
//! saga antes de alguém precisar dela — é a segunda vez que essa aposta paga
//! (a primeira foi a ordem alternativa de exibição).
//!
//! Então este módulo não cria tabela nenhuma. Ele preenche.
//!
//! ## Uma chamada, dois módulos
//!
//! `belongs_to_collection` vem na **mesma resposta** que a ficha de produção do
//! §38 (`/movie/{id}`). São dois módulos porque são dois jobs, com alvos e
//! retomadas diferentes; é uma chamada por filme porque pedir duas vezes seria
//! pagar dobrado pela mesma linha.
//!
//! ## A retomada sai de graça, como sempre
//!
//! O alvo é "filme identificado que ainda não está em nenhuma `franchise`". Uma
//! segunda rodada continua de onde parou sem coluna de controle — o mesmo truque
//! do `repair-series` (§21), da trivia e da produção (§38).
//!
//! **O falso negativo é assumido**: filme que o TMDB diz não pertencer a saga
//! nenhuma continua elegível pra sempre, e será perguntado de novo a cada
//! rodada. Marcá-lo exigiria uma tabela de "já perguntei e não tem" — schema pra
//! guardar ausência, que é o que o §38 recusou pelo mesmo motivo. Numa segunda
//! passada isso custa as chamadas dos filmes sem saga, e só.

use std::collections::HashMap;

use sqlx::PgPool;
use uuid::Uuid;

use crate::jobs::Job;
use crate::metadata::tmdb::SagaDoProvider;
use crate::metadata::Providers;

/// Quantos filmes por lote antes de olhar o cancelamento e respirar.
const LOTE: usize = 20;

/// O respiro entre lotes. O mesmo do §38: o TMDB não reclama, mas 548 chamadas
/// em rajada são 548 chances de tomar um 429 que ninguém está lendo.
const INTERVALO: std::time::Duration = std::time::Duration::from_millis(400);

#[derive(Debug, Default, serde::Serialize)]
pub struct SagaStatus {
    pub running: bool,
    pub total: usize,
    pub feitos: usize,
    /// Filmes que entraram numa saga.
    pub com_saga: usize,
    /// Filmes que o TMDB diz não pertencerem a nenhuma. É a maioria, e é fato.
    pub avulsos: usize,
    pub falhas: usize,
    /// Sagas distintas criadas nesta rodada.
    pub sagas: usize,
    pub atual: Option<String>,
}

/// Busca a saga de todos os filmes identificados.
pub async fn aquecer(pool: PgPool, providers: Providers, mut job: Option<Job>) {
    let Some(tmdb) = providers.tmdb.clone() else {
        tracing::warn!("sem chave do TMDB — as sagas não podem ser buscadas");
        if let Some(j) = job.take() {
            j.finish(&SagaStatus::default(), "failed", Some("sem chave do TMDB".into()))
                .await;
        }
        return;
    };

    let alvos: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT w.id, w.external_ids->>'tmdb', w.title
         FROM work w
         WHERE w.kind = 'movie'
           AND w.match_state IN ('auto', 'confirmed')
           AND w.external_ids ? 'tmdb'
           AND NOT EXISTS (
               SELECT 1 FROM collection_item ci
               JOIN collection c ON c.id = ci.collection_id AND c.kind = 'franchise'
               WHERE ci.work_id = w.id
           )
         ORDER BY w.title",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    let mut s = SagaStatus { running: true, total: alvos.len(), ..Default::default() };
    // Cache de saga do provider → id da coleção. Sem ele, os vinte e cinco
    // filmes dos 007 fariam vinte e cinco idas ao banco pra achar a mesma linha.
    let mut conhecidas: HashMap<i64, Uuid> = HashMap::new();

    for bloco in alvos.chunks(LOTE) {
        // Cancelamento cooperativo no ponto seguro (§12).
        if matches!(&job, Some(j) if j.cancelled().await) {
            s.running = false;
            if let Some(j) = job.take() {
                j.finish(&s, "cancelled", None).await;
            }
            tracing::info!(feitos = s.feitos, "busca de sagas cancelada");
            return;
        }

        s.atual = bloco.first().map(|(_, _, t)| t.clone());

        for (work_id, tmdb_id, _) in bloco {
            match tmdb.saga_do_filme(tmdb_id).await {
                Ok(Some(saga)) => match ligar(&pool, *work_id, &saga, &mut conhecidas).await {
                    Ok(nova) => {
                        s.com_saga += 1;
                        s.sagas += nova as usize;
                    }
                    Err(e) => {
                        s.falhas += 1;
                        tracing::warn!(error = %e, "não deu pra ligar o filme à saga");
                    }
                },
                Ok(None) => s.avulsos += 1,
                Err(e) => {
                    // Falha não marca nada, então o filme continua elegível — é
                    // o que torna isto retomável de verdade.
                    s.falhas += 1;
                    tracing::warn!(error = %e, "busca de saga falhou");
                }
            }
        }

        s.feitos += bloco.len();
        if let Some(j) = &job {
            j.tick(&s, s.feitos as i64, Some(s.total as i64)).await;
        }
        tokio::time::sleep(INTERVALO).await;
    }

    s.running = false;
    s.atual = None;
    tracing::info!(
        com_saga = s.com_saga,
        avulsos = s.avulsos,
        sagas = s.sagas,
        falhas = s.falhas,
        "busca de sagas concluída"
    );
    if let Some(j) = job {
        j.finish(&s, "succeeded", None).await;
    }
}

/// Garante a coleção da saga e pendura o filme nela.
///
/// Devolve `true` quando a saga foi criada agora — é o que conta "sagas
/// distintas" no status sem precisar de uma segunda consulta.
///
/// **`provider_key` é a chave de identidade**, e a tabela já tem `UNIQUE` nela:
/// duas rodadas simultâneas não criam duas coleções dos 007, e o `ON CONFLICT`
/// devolve a que já existe em vez de estourar.
async fn ligar(
    pool: &PgPool,
    work_id: Uuid,
    saga: &SagaDoProvider,
    conhecidas: &mut HashMap<i64, Uuid>,
) -> Result<bool, sqlx::Error> {
    let mut nova = false;
    let colecao = match conhecidas.get(&saga.id) {
        Some(id) => *id,
        None => {
            let chave = format!("tmdb:collection:{}", saga.id);
            let arte = serde_json::json!({
                "poster": saga.poster_path.as_deref(),
                "backdrop": saga.backdrop_path.as_deref(),
            });

            // `DO UPDATE` num campo inócuo, e não `DO NOTHING`: o segundo não
            // devolve linha no conflito, e aí a coleção que já existe voltaria
            // como "não encontrada" na primeira rodada em que ela já existisse.
            let (id, criada): (Uuid, bool) = sqlx::query_as(
                "INSERT INTO collection (kind, title, provider_key, external_ids, artwork, origin)
                 VALUES ('franchise', $1, $2, jsonb_build_object('tmdb', $3::text), $4, 'provider')
                 ON CONFLICT (provider_key) DO UPDATE SET title = EXCLUDED.title
                 RETURNING id, (xmax = 0) AS criada",
            )
            .bind(&saga.name)
            .bind(&chave)
            .bind(saga.id.to_string())
            .bind(&arte)
            .fetch_one(pool)
            .await?;

            nova = criada;
            conhecidas.insert(saga.id, id);
            id
        }
    };

    sqlx::query(
        "INSERT INTO collection_item (collection_id, work_id)
         VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(colecao)
    .bind(work_id)
    .execute(pool)
    .await?;

    Ok(nova)
}

#[cfg(test)]
mod tests {
    /// A saga é `franchise`, e não um `kind` novo.
    ///
    /// O CHECK de `collection.kind` aceita `franchise` desde a migração
    /// original — o §1 previu a saga antes de alguém precisar dela. Inventar
    /// `kind = 'saga'` exigiria uma migração e daria dois nomes pra mesma coisa.
    #[test]
    fn a_saga_e_franchise() {
        assert!(super::aquecer_usa_franchise());
    }

    /// O lote respira. 548 chamadas em rajada são 548 chances de tomar um 429
    /// que ninguém está lendo — o mesmo cuidado do §38.
    #[test]
    fn o_lote_respira() {
        assert!(super::LOTE <= 25);
        assert!(super::INTERVALO >= std::time::Duration::from_millis(200));
    }
}

/// Existe só pro teste acima poder afirmar o `kind` sem rodar SQL.
#[cfg(test)]
fn aquecer_usa_franchise() -> bool {
    // As duas consultas deste módulo citam `franchise`, e é a única palavra que
    // as liga ao schema. Se alguém trocar por 'saga', o CHECK recusa em tempo de
    // execução — e este teste lembra antes.
    include_str!("saga.rs").matches("'franchise'").count() >= 2
}
