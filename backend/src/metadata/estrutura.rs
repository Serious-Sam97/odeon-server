//! R75 — a estrutura vem do disco, e não do provider.
//!
//! ## O que estava conflado
//!
//! Uma `collection(series)` só nascia dentro do `apply_candidate`, ou seja,
//! **depois que o TMDB confirmou qual série é**. Medido em 20/08/2026:
//!
//! | | |
//! |---|---|
//! | pastas que claramente são série | **507** (14.141 arquivos numerados) |
//! | séries existentes no banco | **133** |
//! | pastas sem série nenhuma | **196** |
//!
//! Aqueles 196 viravam episódio solto na grade — um cartão por arquivo, com o
//! nome do episódio, sem a série que os junta. E a série existia: estava na
//! pasta, que é onde ela sempre esteve.
//!
//! "Isto é uma série com 14 temporadas" e "esta série é `tmdb/16183`" são
//! perguntas diferentes, e **só a segunda precisa de rede**. É a mesma
//! separação que a R64 fez com o formato.
//!
//! ## A comparação que fechou o argumento
//!
//! O Jellyfin da mesma casa mostra 187 séries e 16.416 episódios contra os
//! nossos 133 e 14.294 — e a diferença **não é identificação melhor**: dos
//! 16.416 dele, só 9.891 têm id de provider. Ele monta a estrutura a partir da
//! pasta e preenche o metadado quando dá; nós esperávamos o metadado pra montar
//! a estrutura.
//!
//! O que **não** se copia dele: escrever o palpite como se fosse fato. Aqui a
//! coleção nasce com `origin = 'estrutura'`, que é uma terceira palavra ao lado
//! de `manual` e `provider` e quer dizer com todas as letras *"isto veio do
//! disco, ninguém confirmou"*. A tela pode dizer isso; o Jellyfin não tem como.
//!
//! ## Por que é seguro
//!
//! Uma coleção de estrutura é **recriável de graça**: ela não guarda nada que
//! não esteja no caminho dos arquivos. Apagar e refazer não perde informação —
//! ao contrário de uma coleção `manual`, que é trabalho humano.
//!
//! E ela **cede a vez**: quando o provider identifica a pasta, o
//! `apply_candidate` cria a coleção dele e o vínculo de estrutura sai. Ver
//! `soltar_da_estrutura`.

use std::collections::HashMap;

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::jobs::Job;

/// Quantos arquivos numerados uma pasta precisa ter pra ser considerada série.
///
/// **Três, e não um.** Uma pasta com um arquivo numerado é um filme que o
/// parser leu torto muito mais vezes do que é uma série de um episódio; duas é
/// acaso comum — é o mesmo corte que a corroboração de vizinhos usa no score,
/// e pelo mesmo motivo.
const MINIMO_DE_EPISODIOS: i64 = 3;

#[derive(Debug, Default, Clone, Serialize)]
pub struct EstruturaStatus {
    pub running: bool,
    pub dry_run: bool,
    /// Pastas que parecem série ou canal.
    pub pastas: usize,
    /// Coleções criadas agora.
    pub criadas: usize,
    /// Pastas que já tinham coleção — do provider ou de uma passada anterior.
    pub ja_tinham: usize,
    /// Obras ligadas a uma coleção de estrutura.
    pub ligadas: usize,
    /// Coleções de estrutura que ficaram sem item e foram removidas.
    pub podadas: usize,
    pub amostra: Vec<serde_json::Value>,
}

/// Uma pasta que o disco diz ser série ou canal.
#[derive(Debug, sqlx::FromRow)]
struct Pasta {
    /// A pasta **da obra-mãe** — já subiu um nível quando o arquivo está numa
    /// pasta de temporada ou de extras.
    raiz: String,
    library_id: Uuid,
    /// `episode` ou `video`, do `default_kind` da biblioteca.
    tipo: String,
    arquivos: i64,
    numerados: i64,
    /// Quantas dessas obras já pertencem a uma coleção de provider.
    com_colecao: i64,
}

/// O SQL que descobre as pastas. Vive numa `const` porque o ensaio e a
/// execução **têm** de olhar exatamente a mesma coisa — um ensaio que consulta
/// diferente do que escreve é pior que não ter ensaio.
///
/// A subida de nível usa a mesma lista de pastas-que-não-são-título que o
/// `guess_from_path` (temporada, especiais, extras): num acervo com
/// `Série/Temporada 3/arquivo`, a raiz é `Série`.
/// **A pasta-mãe de um arquivo.** Uma expressão só, usada nos dois lugares.
///
/// ⚠️ Ela precisa ser uma só porque quem **descobre** as pastas e quem
/// **pendura** as obras têm de concordar sobre onde cada arquivo mora. Não
/// concordavam na primeira versão: a descoberta calculava a raiz por arquivo e
/// a ligação varria `LIKE raiz || '/%'`, que é mais largo. Onde uma pasta de
/// série continha outra, o arquivo de dentro era pendurado nas duas —
/// **79 obras em dois grupos ao mesmo tempo**, medido.
///
/// ⚠️ **Canal é sempre o primeiro nível abaixo da raiz da biblioteca**, e isso
/// é convenção da casa, não heurística: `Youtube/<Canal>/<Playlist>/<vídeo>`.
/// Sem esta regra o job criava um canal por playlist — medido: *Vale Ou Não A
/// Pena Jogar*, *A Primeira Meia Hora* e *Sagas* viraram canais, e os três são
/// playlists do **Zangado**.
const RAIZ: &str = r#"
        CASE
            WHEN l.default_kind = 'video' THEN
                CASE WHEN mf.dir_path = l.root_path THEN l.root_path
                     ELSE l.root_path || '/' ||
                          split_part(right(mf.dir_path, -length(l.root_path) - 1), '/', 1)
                END
            -- Série: sobe um nível quando o arquivo está numa pasta de
            -- temporada ou de extras. É a mesma lista do `guess_from_path`.
            WHEN mf.dir_path ~* '/((season|temp(orada)?|s)[ ._-]*[0-9]+|especiais?|specials?|extras?|featurettes?|bonus|b[oô]nus|trailers?|making[ ._-]*of)[^/]*$'
                THEN regexp_replace(mf.dir_path, '/[^/]+$', '')
            ELSE mf.dir_path
        END
"#;

const PASTAS_SQL: &str = r#"
WITH arquivos AS (
    SELECT
        {RAIZ} AS raiz,
        mf.library_id,
        l.default_kind AS tipo,
        w.id AS work_id,
        (w.episode_number IS NOT NULL OR w.season_number IS NOT NULL) AS numerado,
        EXISTS (
            SELECT 1 FROM collection_item ci
            JOIN collection c ON c.id = ci.collection_id
            WHERE ci.work_id = w.id
              AND c.kind IN ('series', 'season', 'channel')
              AND c.origin <> 'estrutura'
        ) AS ja_tem
    FROM media_file mf
    JOIN work w ON w.id = mf.work_id
    JOIN library l ON l.id = mf.library_id
    WHERE mf.status = 'probed'
      AND w.match_state <> 'ignored'
      AND l.default_kind IN ('episode', 'video')
)
SELECT raiz, library_id, tipo,
       count(*) AS arquivos,
       count(*) FILTER (WHERE numerado) AS numerados,
       count(*) FILTER (WHERE ja_tem) AS com_colecao
FROM arquivos
GROUP BY 1, 2, 3
ORDER BY 4 DESC
"#;

/// Monta série e canal a partir das pastas.
///
/// `dry_run` mostra o que faria — e é o padrão em toda rota que escreve em
/// lote neste servidor.
pub async fn montar(pool: PgPool, dry_run: bool, mut job: Option<Job>) -> EstruturaStatus {
    let mut s = EstruturaStatus {
        running: true,
        dry_run,
        ..Default::default()
    };

    let pastas: Vec<Pasta> = sqlx::query_as(&PASTAS_SQL.replace("{RAIZ}", RAIZ))
        .fetch_all(&pool)
        .await
        .unwrap_or_default();

    // Canal entra sempre; série exige numeração. Um canal é um canal mesmo com
    // vídeos que ninguém numerou — é a pasta que o define, e não o nome do
    // arquivo.
    let alvos: Vec<&Pasta> = pastas
        .iter()
        .filter(|p| p.tipo == "video" || p.numerados >= MINIMO_DE_EPISODIOS)
        .collect();
    s.pastas = alvos.len();

    for pasta in alvos {
        // Pasta já resolvida pelo provider não é assunto daqui. `com_colecao`
        // conta obras que pertencem a uma coleção de verdade; se **todas**
        // pertencem, não há o que montar.
        if pasta.com_colecao >= pasta.arquivos {
            s.ja_tinham += 1;
            continue;
        }

        let nome = nome_da_pasta(&pasta.raiz);
        let kind = if pasta.tipo == "video" { "channel" } else { "series" };
        let chave = format!("estrutura:{}", pasta.raiz);

        if s.amostra.len() < 40 {
            s.amostra.push(serde_json::json!({
                "pasta": pasta.raiz,
                "vira": kind,
                "titulo": nome,
                "arquivos": pasta.arquivos,
                "numerados": pasta.numerados,
                "ja_no_provider": pasta.com_colecao,
            }));
        }

        if dry_run {
            s.criadas += 1;
            s.ligadas += (pasta.arquivos - pasta.com_colecao).max(0) as usize;
            continue;
        }

        match montar_uma(&pool, &chave, kind, &nome, pasta).await {
            Ok(ligadas) => {
                s.criadas += 1;
                s.ligadas += ligadas as usize;
            }
            Err(e) => tracing::warn!(error = %e, pasta = pasta.raiz, "estrutura não montou"),
        }

        if let Some(j) = &job {
            j.tick(&s, s.criadas as i64, Some(s.pastas as i64)).await;
        }
    }

    if !dry_run {
        s.podadas = podar(&pool).await;
    }

    s.running = false;
    tracing::info!(
        pastas = s.pastas,
        criadas = s.criadas,
        ligadas = s.ligadas,
        podadas = s.podadas,
        dry_run,
        "estrutura montada"
    );
    if let Some(j) = job.take() {
        j.finish(&s, "succeeded", None).await;
    }
    s
}

/// O último pedaço do caminho, que é o nome da série ou do canal.
fn nome_da_pasta(caminho: &str) -> String {
    caminho
        .rsplit('/')
        .find(|p| !p.trim().is_empty())
        .unwrap_or(caminho)
        .trim()
        .to_string()
}

/// Cria (ou reusa) a coleção da pasta e pendura o que ainda está solto.
///
/// ⚠️ **Só entra obra que não tem coleção de provider.** Uma série
/// parcialmente identificada — metade casada, metade na fila — fica com a
/// metade casada onde já estava e a outra metade na estrutura, e as duas
/// aparecem. É feio e é honesto; fundir as duas seria afirmar que são a mesma
/// série, que é justamente o que ninguém confirmou ainda.
async fn montar_uma(
    pool: &PgPool,
    chave: &str,
    kind: &str,
    nome: &str,
    pasta: &Pasta,
) -> anyhow::Result<u64> {
    let colecao: Uuid = sqlx::query_scalar(
        "INSERT INTO collection (kind, title, provider_key, origin)
         VALUES ($1, $2, $3, 'estrutura')
         ON CONFLICT (provider_key) DO UPDATE SET title = EXCLUDED.title
         RETURNING id",
    )
    .bind(kind)
    .bind(nome)
    .bind(chave)
    .fetch_one(pool)
    .await?;

    // A temporada continua sendo `season` mesmo dentro de um canal: o modelo
    // do grafo não precisa de uma palavra nova pra "playlist com número", e a
    // biblioteca já sabe subir episódio → temporada → série por esse caminho.
    // A chave da sub-coleção: número da temporada na série, **nome da pasta**
    // no canal. Um canal não tem temporada — tem playlist, e o nome dela está
    // na pasta. Medido: os 331 vídeos dos Irmãos Piologo viraram "Temporada 1"
    // porque o parser leu a numeração como temporada, quando a pasta se chama
    // *Irmãos Piologo Games*.
    let mut filhas: HashMap<String, Uuid> = HashMap::new();
    let obras: Vec<(Uuid, Option<i32>, Option<i32>, String)> = sqlx::query_as(
        &r#"
        SELECT w.id, w.season_number, w.episode_number, mf.dir_path
        FROM media_file mf
        JOIN work w ON w.id = mf.work_id
        JOIN library l ON l.id = mf.library_id
        WHERE mf.status = 'probed'
          AND w.match_state <> 'ignored'
          AND mf.library_id = $2
          -- A MESMA raiz da descoberta, e não um `LIKE` mais largo.
          AND ({RAIZ}) = $1
          AND NOT EXISTS (
              SELECT 1 FROM collection_item ci
              JOIN collection c ON c.id = ci.collection_id
              WHERE ci.work_id = w.id
                AND c.kind IN ('series', 'season', 'channel')
                AND c.origin <> 'estrutura')
        "#
        .replace("{RAIZ}", RAIZ),
    )
    .bind(&pasta.raiz)
    .bind(pasta.library_id)
    .fetch_all(pool)
    .await?;

    let mut ligadas = 0u64;
    for (work_id, temporada, episodio, dir_path) in obras {
        // No canal, a sub-coleção é a **pasta**; na série, a temporada.
        let sub = if kind == "channel" {
            (dir_path != pasta.raiz)
                .then(|| (nome_da_pasta(&dir_path), format!("{chave}:{dir_path}"), None))
        } else {
            temporada.map(|n| (format!("Temporada {n}"), format!("{chave}:s{n}"), Some(n)))
        };

        let destino = match sub {
            Some((titulo, chave_filha, posicao)) => match filhas.get(&chave_filha) {
                Some(id) => *id,
                None => {
                    let id: Uuid = sqlx::query_scalar(
                        "INSERT INTO collection (kind, title, parent_id, position,
                                                 provider_key, origin)
                         VALUES ('season', $1, $2, $3, $4, 'estrutura')
                         ON CONFLICT (provider_key) DO UPDATE SET title = EXCLUDED.title
                         RETURNING id",
                    )
                    .bind(&titulo)
                    .bind(colecao)
                    .bind(posicao)
                    .bind(&chave_filha)
                    .fetch_one(pool)
                    .await?;
                    filhas.insert(chave_filha, id);
                    id
                }
            },
            // Sem sub-pasta e sem temporada, o item pendura direto na série ou
            // no canal. Inventar "Temporada 1" seria afirmar o que o disco não
            // diz — e foi exatamente o que aconteceu antes desta linha existir.
            None => colecao,
        };

        let r = sqlx::query(
            "INSERT INTO collection_item (collection_id, work_id, position)
             VALUES ($1, $2, $3)
             ON CONFLICT (collection_id, work_id) DO UPDATE SET position = EXCLUDED.position",
        )
        .bind(destino)
        .bind(work_id)
        .bind(episodio)
        .execute(pool)
        .await?;
        ligadas += r.rows_affected();
    }

    Ok(ligadas)
}

/// Tira a obra da estrutura quando o provider deu a ela um lugar de verdade.
///
/// Chamada pelo `apply_candidate`. É o que faz a coleção de estrutura **ceder
/// a vez** em vez de duplicar o cartão: sem isto, uma obra identificada
/// pertenceria à série do TMDB e à pasta ao mesmo tempo, e a biblioteca
/// escolheria uma das duas por sorte (`LIMIT 1`).
pub async fn soltar_da_estrutura(pool: &PgPool, work_id: Uuid) {
    let _ = sqlx::query(
        "DELETE FROM collection_item ci
          USING collection c
          WHERE c.id = ci.collection_id
            AND ci.work_id = $1
            AND c.origin = 'estrutura'",
    )
    .bind(work_id)
    .execute(pool)
    .await;
}

/// Remove coleção de estrutura que ficou vazia.
///
/// Só `origin = 'estrutura'`: uma playlist manual vazia é uma playlist que
/// alguém criou e ainda não encheu, e apagá-la seria apagar trabalho.
async fn podar(pool: &PgPool) -> usize {
    let r = sqlx::query(
        "DELETE FROM collection c
          WHERE c.origin = 'estrutura'
            AND NOT EXISTS (SELECT 1 FROM collection_item ci WHERE ci.collection_id = c.id)
            AND NOT EXISTS (SELECT 1 FROM collection f WHERE f.parent_id = c.id)",
    )
    .execute(pool)
    .await;
    r.map(|r| r.rows_affected() as usize).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_nome_e_o_ultimo_pedaco_do_caminho() {
        assert_eq!(nome_da_pasta("/media2/TV Show/ALF"), "ALF");
        assert_eq!(nome_da_pasta("/media2/Youtube/Zangado"), "Zangado");
        // Barra sobrando não vira nome vazio.
        assert_eq!(nome_da_pasta("/media2/TV Show/ALF/"), "ALF");
    }

    /// O código deste arquivo **sem os testes**.
    ///
    /// Ler o próprio módulo é a única rede possível pra defeito que é texto
    /// virando SQL — mas sem este corte o teste encontra as próprias agulhas
    /// e passa (ou falha) por si mesmo. Já aconteceu duas vezes.
    fn codigo() -> &'static str {
        let fonte = include_str!("estrutura.rs");
        fonte.split_once("\n#[cfg(test)]").map(|(a, _)| a).unwrap_or(fonte)
    }

    /// ⚠️ A consulta que **descobre** as pastas é uma só. Um ensaio que olha
    /// diferente do que a execução escreve é pior que não ter ensaio.
    #[test]
    fn o_ensaio_e_a_execucao_leem_a_mesma_consulta() {
        assert_eq!(
            codigo().matches("const PASTAS_SQL").count(),
            1,
            "apareceu uma segunda consulta de descoberta"
        );
        // E ela é usada uma vez só, no `montar`.
        assert_eq!(codigo().matches("PASTAS_SQL.replace").count(), 1);
        // E a raiz é uma expressão só, compartilhada com a ligação.
        assert_eq!(codigo().matches("const RAIZ").count(), 1);
        assert_eq!(codigo().matches(r#"replace("{RAIZ}", RAIZ)"#).count(), 2);
    }

    /// A coleção de estrutura **nunca** se apresenta como decisão de alguém.
    #[test]
    fn estrutura_nao_se_disfarca_de_manual_nem_de_provider() {
        let codigo = codigo();
        assert!(codigo.contains("'estrutura'"));
        for alheia in ["'provider'", "'manual'"] {
            assert!(
                !codigo.contains(&format!("origin) VALUES ($1, $2, $3, {alheia}")),
                "este módulo passou a escrever origem que não é dele: {alheia}"
            );
        }
    }

    /// A poda só alcança o que este módulo criou.
    #[test]
    fn a_poda_nao_encosta_em_playlist_de_gente() {
        let poda = codigo().split_once("async fn podar").expect("podar sumiu").1;
        assert!(poda.contains("c.origin = 'estrutura'"));
    }
}
