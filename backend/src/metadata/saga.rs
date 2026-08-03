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
//!
//! ## R38 — a arte, que esta rodada esqueceu de baixar
//!
//! A primeira versão deste módulo gravou o `poster_path` **cru do TMDB** no
//! `artwork` da coleção. O pipeline de série chama `artwork::fetch` desde o M1;
//! aqui a chamada faltou, e o resultado foi medido: **131 sagas com caminho
//! remoto contra 113 séries com arquivo local**. O front prefixa `/artwork/`,
//! o `ServeDir` responde 404, e a moldura do guia fica vazia — os dois itens
//! "cartaz da semana quebrado" e "diversas capas quebradas" eram este bug.
//!
//! O conserto tem duas metades e as duas moram aqui: o job baixa a arte de toda
//! saga nova, e uma varredura conserta as que já existem. **A varredura não
//! custa uma chamada de API**: o caminho remoto que ficou guardado é o próprio
//! insumo do reparo, então ela só precisa remontar a URL e baixar.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    /// Capas baixadas pro cache local — as novas mais as reparadas (R38).
    pub capas: usize,
    /// Sagas cuja arte não desceu. Elas guardam o caminho remoto e voltam a ser
    /// alvo da varredura na rodada seguinte.
    pub capas_pendentes: usize,
    pub atual: Option<String>,
}

/// Busca a saga de todos os filmes identificados, e conserta a arte das que já
/// existem (R38).
pub async fn aquecer(
    pool: PgPool,
    providers: Providers,
    artwork_dir: PathBuf,
    mut job: Option<Job>,
) {
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

    // As sagas que a R32 deixou com caminho remoto. A consulta é o próprio
    // estado errado, então a varredura é retomável pelo mesmo truque do resto do
    // módulo: o que já foi consertado deixa de aparecer.
    let quebradas: Vec<(Uuid, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, artwork->>'poster', artwork->>'backdrop'
         FROM collection
         WHERE kind = 'franchise'
           AND (artwork->>'poster' LIKE '/%' OR artwork->>'backdrop' LIKE '/%')
         ORDER BY title",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    // Um total só, porque é um trabalho só do ponto de vista de quem espera a
    // barra andar.
    let mut s = SagaStatus {
        running: true,
        total: alvos.len() + quebradas.len(),
        ..Default::default()
    };

    // O reparo vem antes da busca: ele não custa chamada de API, e a saga criada
    // logo adiante já nasce com a arte no lugar — não há trabalho repetido.
    for bloco in quebradas.chunks(LOTE) {
        if matches!(&job, Some(j) if j.cancelled().await) {
            s.running = false;
            if let Some(j) = job.take() {
                j.finish(&s, "cancelled", None).await;
            }
            tracing::info!(capas = s.capas, "conserto das capas cancelado");
            return;
        }

        s.atual = Some("consertando as capas das sagas".into());

        for (colecao, poster, backdrop) in bloco {
            // Só o que está remoto desce. Uma arte já local fica onde está.
            let baixadas = baixar_arte(
                &pool,
                &providers,
                &artwork_dir,
                *colecao,
                remoto(poster),
                remoto(backdrop),
            )
            .await;
            s.capas += baixadas.baixadas;
            s.capas_pendentes += baixadas.pendentes;
        }

        s.feitos += bloco.len();
        if let Some(j) = &job {
            j.tick(&s, s.feitos as i64, Some(s.total as i64)).await;
        }
        tokio::time::sleep(INTERVALO).await;
    }

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
                    Ok((colecao, nova)) => {
                        s.com_saga += 1;
                        s.sagas += nova as usize;
                        // A arte desce só quando a saga acaba de nascer: rodar de
                        // novo não rebaixa as 131 que a varredura já consertou.
                        if nova {
                            let baixadas = baixar_arte(
                                &pool,
                                &providers,
                                &artwork_dir,
                                colecao,
                                saga.poster_path.as_deref(),
                                saga.backdrop_path.as_deref(),
                            )
                            .await;
                            s.capas += baixadas.baixadas;
                            s.capas_pendentes += baixadas.pendentes;
                        }
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
        capas = s.capas,
        capas_pendentes = s.capas_pendentes,
        falhas = s.falhas,
        "busca de sagas concluída"
    );
    if let Some(j) = job {
        j.finish(&s, "succeeded", None).await;
    }
}

/// Garante a coleção da saga e pendura o filme nela.
///
/// Devolve a coleção e `true` quando ela foi criada agora — é o que conta "sagas
/// distintas" no status, e é o que diz a quem chama que a arte ainda precisa
/// descer.
///
/// **`provider_key` é a chave de identidade**, e a tabela já tem `UNIQUE` nela:
/// duas rodadas simultâneas não criam duas coleções dos 007, e o `ON CONFLICT`
/// devolve a que já existe em vez de estourar.
///
/// **A coleção nasce sem arte nenhuma** (R38). A versão anterior gravava aqui o
/// `poster_path` do TMDB, que é um caminho remoto que o `/artwork/` não serve —
/// guardar caminho quebrado é pior que guardar nada, porque a tela acredita nele
/// (§18). Quem baixa é o `baixar_arte`, e só ele escreve no `artwork`.
async fn ligar(
    pool: &PgPool,
    work_id: Uuid,
    saga: &SagaDoProvider,
    conhecidas: &mut HashMap<i64, Uuid>,
) -> Result<(Uuid, bool), sqlx::Error> {
    let mut nova = false;
    let colecao = match conhecidas.get(&saga.id) {
        Some(id) => *id,
        None => {
            let chave = format!("tmdb:collection:{}", saga.id);

            // `DO UPDATE` num campo inócuo, e não `DO NOTHING`: o segundo não
            // devolve linha no conflito, e aí a coleção que já existe voltaria
            // como "não encontrada" na primeira rodada em que ela já existisse.
            let (id, criada): (Uuid, bool) = sqlx::query_as(
                "INSERT INTO collection (kind, title, provider_key, external_ids, origin)
                 VALUES ('franchise', $1, $2, jsonb_build_object('tmdb', $3::text), 'provider')
                 ON CONFLICT (provider_key) DO UPDATE SET title = EXCLUDED.title
                 RETURNING id, (xmax = 0) AS criada",
            )
            .bind(&saga.name)
            .bind(&chave)
            .bind(saga.id.to_string())
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

    Ok((colecao, nova))
}

/// O caminho, se ele for do TMDB e não do cache local.
///
/// O arquivo local é `uuid-poster.jpg`; o remoto começa com `/`. É a mesma
/// distinção que o `WHERE ... LIKE '/%'` da varredura faz, e ela mora aqui pra
/// não ser reescrita em dois lugares.
fn remoto(caminho: &Option<String>) -> Option<&str> {
    caminho.as_deref().filter(|c| c.starts_with('/'))
}

#[derive(Default)]
struct ArteBaixada {
    baixadas: usize,
    pendentes: usize,
}

/// Baixa pôster e backdrop da saga pro cache e grava os caminhos locais.
///
/// O que entra é o caminho do TMDB (`/mv0MySTq….jpg`), venha ele da resposta da
/// API ou da linha que a R32 deixou no banco.
///
/// **A cor dominante vem junto e de graça** — é o mesmo `artwork::fetch` do M1,
/// e é por isso que as 133 sagas estão hoje com `dominant_color` nula enquanto
/// toda série tem a sua.
///
/// **Quando o download falha, o caminho remoto fica gravado.** Ele é o único
/// registro de onde a arte mora, e é o que faz a saga voltar a ser alvo da
/// varredura na próxima rodada. A alternativa — não gravar nada — perderia o
/// endereço e exigiria uma chamada de API pra reencontrá-lo.
async fn baixar_arte(
    pool: &PgPool,
    providers: &Providers,
    artwork_dir: &Path,
    colecao: Uuid,
    poster: Option<&str>,
    backdrop: Option<&str>,
) -> ArteBaixada {
    use crate::metadata::tmdb::{url_de_imagem, BACKDROP, POSTER};

    let mut contagem = ArteBaixada::default();
    let mut arte = serde_json::Map::new();
    let mut cor: Option<String> = None;

    for (tipo, caminho, tamanho) in [
        ("poster", poster, POSTER),
        ("backdrop", backdrop, BACKDROP),
    ] {
        let Some(caminho) = caminho else { continue };
        let url = url_de_imagem(caminho, tamanho);

        match crate::artwork::fetch(&providers.http, artwork_dir, colecao, tipo, &url).await {
            Ok(guardada) => {
                arte.insert(tipo.into(), guardada.path.into());
                // A cor sai do pôster, que é a moldura que a tela usa.
                if tipo == "poster" {
                    cor = guardada.dominant_color;
                }
                contagem.baixadas += 1;
            }
            Err(e) => {
                arte.insert(tipo.into(), caminho.into());
                contagem.pendentes += 1;
                tracing::warn!(error = %e, tipo, "arte da saga não baixou");
            }
        }
    }

    if arte.is_empty() {
        return contagem;
    }

    // `||` e não substituição: o que já estava no `artwork` e não foi baixado
    // agora continua lá. `COALESCE` na cor pela mesma razão do §21 — o provider
    // preenche o que falta e não derruba o que existe.
    let update = sqlx::query(
        "UPDATE collection
            SET artwork = artwork || $2,
                dominant_color = COALESCE(dominant_color, $3)
          WHERE id = $1",
    )
    .bind(colecao)
    .bind(serde_json::Value::Object(arte))
    .bind(cor.as_deref())
    .execute(pool)
    .await;

    if let Err(e) = update {
        tracing::warn!(error = %e, "não deu pra gravar a arte da saga");
    }

    contagem
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

    /// O arquivo do cache não é caminho remoto, e o do TMDB é.
    ///
    /// É a regra que separa "já consertado" de "ainda quebrado", e ela decide
    /// tanto o alvo da varredura quanto o que desce em cada saga.
    #[test]
    fn so_o_caminho_do_tmdb_e_remoto() {
        assert_eq!(
            super::remoto(&Some("/mv0MySTqfXg2ndthOkBg6bGwlnk.jpg".into())),
            Some("/mv0MySTqfXg2ndthOkBg6bGwlnk.jpg")
        );
        let local = Some("0d9f1b2c-3e4d-5a6b-7c8d-9e0f1a2b3c4d-poster.jpg".into());
        assert_eq!(super::remoto(&local), None);
        assert_eq!(super::remoto(&None), None);
    }

    /// **Regressão da R38.** O `INSERT` da saga não escreve arte nenhuma.
    ///
    /// Foi exatamente isto que quebrou 131 capas: `poster_path` gravado cru num
    /// campo que a tela lê como caminho servível. Quem escreve no `artwork` é o
    /// `baixar_arte`, depois do download — e só ele.
    #[test]
    fn o_insert_da_saga_nao_grava_arte() {
        let fonte = include_str!("saga.rs");
        let insert = fonte
            .split_once("INSERT INTO collection (")
            .expect("o INSERT da coleção sumiu")
            .1
            .split_once("RETURNING")
            .expect("o INSERT da coleção mudou de forma")
            .0;
        assert!(
            !insert.contains("artwork"),
            "a coleção da saga voltou a nascer com arte no INSERT"
        );
        assert!(
            !fonte.contains("\"poster\": saga.poster_path"),
            "o caminho remoto do TMDB voltou pro banco"
        );
    }

    /// A varredura mira o próprio estado errado, e é isso que a torna retomável
    /// sem coluna de controle — o mesmo truque do resto do módulo.
    #[test]
    fn a_varredura_mira_o_caminho_remoto() {
        let fonte = include_str!("saga.rs");
        assert!(fonte.contains("artwork->>'poster' LIKE '/%'"));
        assert!(fonte.contains("artwork->>'backdrop' LIKE '/%'"));
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
