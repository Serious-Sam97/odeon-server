//! Canais ao vivo: o Odeon sintoniza, não programa.
//!
//! Uma fonte publica a lista de canais (M3U) e a grade (XMLTV); daqui pra
//! frente isto é leitura. Nada aqui toca o grafo — ver a migração 0013 pro
//! porquê de canal não ser `work` e programa não ser `collection`.

pub mod emissora;
pub mod parse;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sqlx::PgPool;
use uuid::Uuid;

use crate::jobs::Job;

/// Importa uma fonte: baixa M3U e XMLTV, e substitui o que havia.
///
/// **Substitui em vez de mesclar** no caso da grade: horário de programa muda
/// quando o provedor reprograma, e mesclar deixaria o antigo convivendo com o
/// novo — dois programas no ar ao mesmo tempo no mesmo canal. Os CANAIS, esses,
/// são mesclados por `provider_key`, pra não perder `hidden` nem `position` a
/// cada importação.
pub async fn importar(
    pool: &PgPool,
    http: &reqwest::Client,
    artwork_dir: &Path,
    source_id: Uuid,
    job: Option<Job>,
) -> anyhow::Result<(usize, usize)> {
    let fonte: (String, Option<String>) =
        sqlx::query_as("SELECT m3u_url, xmltv_url FROM channel_source WHERE id = $1")
            .bind(source_id)
            .fetch_one(pool)
            .await?;
    let (m3u_url, xmltv_url) = fonte;

    if let Some(j) = &job {
        j.tick(&serde_json::json!({ "etapa": "baixando a lista de canais" }), 0, None).await;
    }

    let lista = http.get(&m3u_url).send().await?.error_for_status()?.text().await?;
    let canais = parse::m3u(&lista);

    let mut tx = pool.begin().await?;
    for (i, canal) in canais.iter().enumerate() {
        // A URL do stream é reescrita AQUI, não na hora de tocar: assim o que
        // está no banco é o que funciona, e quem lê não precisa saber da regra.
        let stream = parse::reescreve_host(&canal.stream_url, &m3u_url);
        sqlx::query(
            "INSERT INTO channel
                (source_id, provider_key, name, number, logo_url, grupo, stream_url, position)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (source_id, provider_key) DO UPDATE SET
                name = EXCLUDED.name,
                number = EXCLUDED.number,
                logo_url = EXCLUDED.logo_url,
                grupo = EXCLUDED.grupo,
                stream_url = EXCLUDED.stream_url,
                position = EXCLUDED.position,
                updated_at = now()",
        )
        .bind(source_id)
        .bind(&canal.provider_key)
        .bind(&canal.name)
        .bind(&canal.number)
        .bind(&canal.logo_url)
        .bind(&canal.grupo)
        .bind(&stream)
        .bind(i as i32)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    let mut programas = 0usize;
    if let Some(url) = xmltv_url.filter(|u| !u.trim().is_empty()) {
        if let Some(j) = &job {
            j.tick(&serde_json::json!({ "etapa": "baixando a grade" }), 1, Some(2)).await;
        }
        let xml = http.get(&url).send().await?.error_for_status()?.text().await?;
        let grade = parse::xmltv(&xml);

        if let Some(j) = &job {
            j.tick(&serde_json::json!({ "etapa": "baixando a arte da grade" }), 1, Some(2)).await;
        }
        let artes = baixar_artes(pool, http, artwork_dir, &grade).await;

        programas = gravar_grade(pool, source_id, &grade, &artes).await?;
    }

    sqlx::query(
        "UPDATE channel_source SET last_import_at = now(), last_error = NULL WHERE id = $1",
    )
    .bind(source_id)
    .execute(pool)
    .await?;

    Ok((canais.len(), programas))
}

/// Traz pro disco do Odeon a arte que o XMLTV aponta. Devolve URL → caminho.
///
/// **Por que baixar em vez de apontar.** A URL que o ErsatzTV publica é um
/// endereço da bridge do Docker (`172.17.0.1:8409`): existe só nesta máquina.
/// Mandar isso pro navegador funcionaria aqui e em nenhum outro aparelho da
/// tailnet — e deixaria a grade dependendo do ErsatzTV estar de pé pra ter
/// capa. A imagem passa a ser do Odeon, como todo o resto do artwork.
///
/// **Três economias, e as três importam** porque isto roda a cada 6 horas:
///
/// 1. A mesma URL se repete muito na grade (um episódio reprisa no dia). São
///    342 imagens distintas pra 919 programas neste XMLTV — só as distintas
///    são baixadas.
/// 2. O que já está em disco não é baixado de novo. A grade é substituída
///    inteira a cada importação, então sem isto seriam 342 downloads a cada
///    6 horas, pra sempre.
/// 3. `fillHeight` sobe pra 720. O XMLTV pede 220 ou 440 de altura — bom pro
///    cartão de um guia, pobre pra um fundo de 46vh. O servidor de imagem
///    aceita o pedido maior, e a imagem chega em 1280×720.
///
/// Falha de download não derruba a importação: aquele programa fica sem arte,
/// que é o estado de antes.
async fn baixar_artes(
    pool: &PgPool,
    http: &reqwest::Client,
    artwork_dir: &Path,
    programas: &[parse::ProgramaBruto],
) -> HashMap<String, String> {
    let mut conhecidas: HashMap<String, String> = sqlx::query_as(
        "SELECT DISTINCT arte_url, arte FROM programme
          WHERE arte_url IS NOT NULL AND arte IS NOT NULL",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .collect();

    // Só vale reaproveitar o que ainda está em disco: a limpeza de artwork
    // órfão pode ter passado, e um caminho apontando pra arquivo que não
    // existe é pior que arte nenhuma — vira 404 na tela.
    conhecidas.retain(|_, caminho| artwork_dir.join(caminho.as_str()).exists());

    let mut baixadas = 0usize;
    let mut falhas = 0usize;

    for p in programas {
        let Some(url) = p.arte_url.as_deref() else { continue };
        if conhecidas.contains_key(url) {
            continue;
        }
        match buscar_arte(http, artwork_dir, url).await {
            Ok(caminho) => {
                conhecidas.insert(url.to_string(), caminho);
                baixadas += 1;
            }
            Err(e) => {
                falhas += 1;
                if falhas <= 3 {
                    tracing::warn!(url, erro = %e, "arte de programa não baixou");
                }
            }
        }
    }

    if baixadas > 0 || falhas > 0 {
        tracing::info!(baixadas, falhas, total = conhecidas.len(), "arte da grade");
    }
    conhecidas
}

/// Nome do arquivo derivado da URL: mesma imagem, mesmo nome, um arquivo só.
async fn buscar_arte(
    http: &reqwest::Client,
    artwork_dir: &Path,
    url: &str,
) -> anyhow::Result<String> {
    let pedido = maior(url);
    let resposta = http.get(&pedido).send().await?.error_for_status()?;
    let tipo = resposta
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let extensao = if tipo.contains("png") {
        "png"
    } else if tipo.contains("webp") {
        "webp"
    } else {
        "jpg"
    };
    let bytes = resposta.bytes().await?;
    if bytes.is_empty() {
        anyhow::bail!("resposta vazia");
    }

    let nome = format!("live-{}.{extensao}", impressao(url));
    let destino: PathBuf = artwork_dir.join(&nome);
    tokio::fs::create_dir_all(artwork_dir).await?;
    tokio::fs::write(&destino, &bytes).await?;
    Ok(nome)
}

/// Pede a imagem maior quando o servidor de arte aceita dizer o tamanho.
///
/// Conservador: só mexe se o parâmetro já estiver lá e pedir menos que 720.
/// URL de provedor que não conhecemos passa intacta.
fn maior(url: &str) -> String {
    let Some(i) = url.find("fillHeight=") else {
        return url.to_string();
    };
    let depois = &url[i + "fillHeight=".len()..];
    let fim = depois.find(|c: char| !c.is_ascii_digit()).unwrap_or(depois.len());
    match depois[..fim].parse::<u32>() {
        Ok(h) if h < 720 => format!("{}fillHeight=720{}", &url[..i], &depois[fim..]),
        _ => url.to_string(),
    }
}

/// Hash curto e estável da URL. Não é criptografia: é nome de arquivo.
fn impressao(url: &str) -> String {
    // FNV-1a de 64 bits — determinístico entre execuções, ao contrário do
    // `DefaultHasher` da std, que é semeado por processo e daria um nome novo
    // a cada reinício do servidor.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Regrava a grade desta fonte numa transação.
async fn gravar_grade(
    pool: &PgPool,
    source_id: Uuid,
    programas: &[parse::ProgramaBruto],
    artes: &HashMap<String, String>,
) -> anyhow::Result<usize> {
    let canais: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, provider_key FROM channel WHERE source_id = $1")
            .bind(source_id)
            .fetch_all(pool)
            .await?;
    let por_chave: std::collections::HashMap<&str, Uuid> =
        canais.iter().map(|(id, k)| (k.as_str(), *id)).collect();

    let mut tx = pool.begin().await?;

    // Os lembretes sobrevivem à troca da grade.
    //
    // `programme_reminder.programme_id` é `ON DELETE CASCADE`, e logo abaixo a
    // grade inteira desta fonte é apagada — ou seja, **toda importação
    // destruía silenciosamente os lembretes agendados**. Com importação manual
    // isso passava despercebido; com o vigia periódico, o agendamento do §17
    // deixaria de funcionar de vez.
    //
    // A identidade de um programa entre duas importações não é o `id` (que é
    // serial e se renova), é o trio canal + horário + título. É por ele que os
    // lembretes voltam. Programa que o provedor reprogramou para outro horário
    // perde o lembrete — e perder é o certo: o que a pessoa agendou não existe
    // mais naquele horário.
    sqlx::query(
        "CREATE TEMP TABLE lembretes_salvos ON COMMIT DROP AS
         SELECT r.user_id, p.channel_id, p.starts_at, p.title, r.created_at, r.notified_at
         FROM programme_reminder r
         JOIN programme p ON p.id = r.programme_id
         WHERE p.channel_id IN (SELECT id FROM channel WHERE source_id = $1)",
    )
    .bind(source_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "DELETE FROM programme WHERE channel_id IN (SELECT id FROM channel WHERE source_id = $1)",
    )
    .bind(source_id)
    .execute(&mut *tx)
    .await?;

    let mut gravados = 0usize;
    for p in programas {
        // Programa de canal que não está na lista é ruído do provedor: o XMLTV
        // costuma trazer a grade inteira dele, não só a dos seus canais.
        let Some(canal_id) = por_chave.get(p.canal.as_str()) else {
            continue;
        };
        sqlx::query(
            "INSERT INTO programme
                (channel_id, starts_at, ends_at, title, sub_title, description, year,
                 categoria, arte, arte_url)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(canal_id)
        .bind(p.starts_at)
        .bind(p.ends_at)
        .bind(&p.title)
        .bind(&p.sub_title)
        .bind(&p.description)
        .bind(p.year)
        .bind(&p.categoria)
        .bind(p.arte_url.as_deref().and_then(|u| artes.get(u)))
        .bind(&p.arte_url)
        .execute(&mut *tx)
        .await?;
        gravados += 1;
    }

    let voltaram = sqlx::query(
        "INSERT INTO programme_reminder (user_id, programme_id, created_at, notified_at)
         SELECT s.user_id, p.id, s.created_at, s.notified_at
         FROM lembretes_salvos s
         JOIN programme p
           ON p.channel_id = s.channel_id
          AND p.starts_at = s.starts_at
          AND p.title = s.title
         ON CONFLICT DO NOTHING",
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    if voltaram.rows_affected() > 0 {
        tracing::info!(lembretes = voltaram.rows_affected(), "lembretes preservados");
    }
    ligar_obras(pool, source_id).await?;
    Ok(gravados)
}

/// Liga programa → obra da biblioteca, pra o cartão poder mostrar a capa.
///
/// **Conservador de propósito.** Medido neste acervo: casar só por título
/// acharia obra pra 410 dos 899 programas, mas **734 títulos da biblioteca são
/// ambíguos** — em geral episódios que repetem o nome da série. Escolher um
/// deles no chute mostraria a capa errada, que é pior do que não mostrar capa.
///
/// Então só liga quando o título é **único** entre as obras com arte. O ano do
/// XMLTV desempata quando existe. Resultado real: 41% dos programas no ar
/// ganham arte — e a taxa sobe sozinha conforme você identifica o acervo.
///
/// "Sherlock" (a série, no ErsatzTV) NÃO casa com "Sherlock Holmes" (o filme,
/// na biblioteca), e é isso que se quer.
async fn ligar_obras(pool: &PgPool, source_id: Uuid) -> anyhow::Result<u64> {
    // --- 1. obra ----------------------------------------------------------
    //
    // **R68 — dois rips do mesmo filme não são ambiguidade.**
    //
    // O `count(*)` contava linhas, e o acervo tem 44 filmes em duas cópias.
    // *007 Contra Goldfinger* aparecia como "duas obras com o mesmo título" e
    // era descartado como homônimo — quando as duas são `tmdb 658`, o mesmo
    // filme. `count(DISTINCT grupo)` usa a mesma identificação que agrupa
    // versões na biblioteca (R47), conta a caixa da locadora (R60) e conta os
    // eixos do guia (R59). Uma regra, quatro lugares.
    //
    // **E ter capa deixou de ser exigência pra entrar na lista.**
    //
    // O `WHERE artwork ? 'poster'` estava lá pra o cartão poder mostrar alguma
    // coisa, e o efeito colateral era descartar o que não tem capa — clipe,
    // sobretudo. *Hans Zimmer Friends Diamond In The Desert* está no acervo
    // com o título **idêntico** ao do EPG, e não casava por ser um
    // `music_video` sem pôster. Casar sem capa ainda vale: o cartão abre a
    // ficha e o "ver desde o início" funciona.
    //
    // A capa vira **desempate** em vez de porta: `quantas = 1` aceita título
    // que resolve pra um grupo só, e `com_arte = 1` aceita quando entre os
    // homônimos existe **um** com arte — que é quase sempre o identificado,
    // enquanto os outros são nome de arquivo. Medido, nos 1.057 programas:
    //
    // | regra | programas que casam |
    // |---|---|
    // | exigindo capa (como era) | 329 |
    // | sem exigir capa | 373, e perde 4 que a capa desambiguava |
    // | **as duas juntas** | **377** |
    let obras = sqlx::query(
        r#"
        WITH com_grupo AS (
            SELECT id, year, kind, match_state,
                   artwork ? 'poster' AS tem_arte,
                   lower(btrim(title)) AS chave,
                   COALESCE(CASE WHEN kind = 'movie' THEN external_ids->>'tmdb' END,
                            id::text)  AS grupo
            FROM work
            WHERE match_state <> 'ignored'
        ),
        candidatas AS (
            SELECT chave,
                   count(DISTINCT grupo) AS quantas,
                   count(DISTINCT grupo) FILTER (WHERE tem_arte) AS com_arte,
                   (array_agg(id ORDER BY tem_arte DESC,
                                        (kind = 'movie') DESC,
                                        (match_state = 'confirmed') DESC))[1] AS work_id,
                   (array_agg(year ORDER BY tem_arte DESC,
                                         (kind = 'movie') DESC))[1] AS ano
            FROM com_grupo
            GROUP BY 1
        )
        UPDATE programme p
           SET work_id = c.work_id
          FROM candidatas c
         WHERE p.channel_id IN (SELECT id FROM channel WHERE source_id = $1)
           AND c.chave = lower(btrim(p.title))
           AND (c.quantas = 1 OR c.com_arte = 1)
           -- Ano discordante derruba o casamento: título igual e ano diferente
           -- quase sempre é remake ou homônimo.
           AND (p.year IS NULL OR c.ano IS NULL OR p.year = c.ano)
        "#,
    )
    .bind(source_id)
    .execute(pool)
    .await?;

    // --- 2. coleção -------------------------------------------------------
    //
    // **O EPG de uma série anuncia o nome da série**, e nome de série não é
    // título de obra nenhuma — é título de `collection`. Era isso que deixava
    // *The Walking Dead* e *Futurama* sem capa: as duas existem como série,
    // com pôster, e o casamento só olhava obras.
    //
    // Vem depois e só onde a obra não casou: apontar pro episódio certo é
    // melhor que apontar pra série, quando dá pra saber qual episódio é.
    //
    // O mesmo corte de ambiguidade vale aqui, e ele importa: uma temporada
    // órfã pode ter o mesmo nome da série.
    let colecoes = sqlx::query(
        r#"
        WITH candidatas AS (
            SELECT lower(btrim(title)) AS chave,
                   count(*) AS quantas,
                   (array_agg(id ORDER BY (kind = 'series') DESC))[1] AS collection_id
            FROM collection
            WHERE kind IN ('series', 'season')
              AND artwork ? 'poster'
            GROUP BY 1
        )
        UPDATE programme p
           SET collection_id = c.collection_id
          FROM candidatas c
         WHERE p.channel_id IN (SELECT id FROM channel WHERE source_id = $1)
           AND p.work_id IS NULL
           AND c.chave = lower(btrim(p.title))
           AND c.quantas = 1
        "#,
    )
    .bind(source_id)
    .execute(pool)
    .await?;

    tracing::info!(
        obras = obras.rows_affected(),
        colecoes = colecoes.rows_affected(),
        "programas ligados"
    );
    Ok(obras.rows_affected() + colecoes.rows_affected())
}

/// Guarda o motivo da falha na própria fonte: sem isto, "não importou" vira
/// adivinhação — exatamente o que o M1 combateu com os `reasons` do score.
pub async fn registrar_erro(pool: &PgPool, source_id: Uuid, erro: &str) {
    let _ = sqlx::query("UPDATE channel_source SET last_error = $2, last_import_at = now() WHERE id = $1")
        .bind(source_id)
        .bind(erro)
        .execute(pool)
        .await;
}

/// O vigia dos lembretes.
///
/// Roda a cada 30s procurando programa que começa dentro da janela e ainda não
/// foi avisado. Marca `notified_at` na MESMA consulta que seleciona
/// (`UPDATE ... RETURNING`), então duas passadas concorrentes não avisam duas
/// vezes — o banco resolve a corrida, não um mutex no processo.
pub fn vigiar_lembretes(pool: PgPool, bus: crate::events::Bus) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tick.tick().await;

            #[derive(sqlx::FromRow)]
            struct Aviso {
                programme_id: i64,
                channel_id: Uuid,
                channel_name: String,
                title: String,
                starts_at: chrono::DateTime<chrono::Utc>,
                user_id: Uuid,
            }

            let avisos: Vec<Aviso> = sqlx::query_as(
                r#"
                UPDATE programme_reminder r
                   SET notified_at = now()
                  FROM programme p JOIN channel c ON c.id = p.channel_id
                 WHERE r.programme_id = p.id
                   AND r.notified_at IS NULL
                   -- Janela: começa em até 2 min, ou começou há menos de 5.
                   -- O limite de trás existe porque o servidor pode ter ficado
                   -- fora do ar: avisar de algo que já acabou seria pior que
                   -- não avisar.
                   AND p.starts_at BETWEEN now() - interval '5 minutes'
                                       AND now() + interval '2 minutes'
             RETURNING r.programme_id, c.id AS channel_id, c.name AS channel_name,
                       p.title, p.starts_at, r.user_id
                "#,
            )
            .fetch_all(&pool)
            .await
            .unwrap_or_default();

            for a in avisos {
                tracing::info!(programa = %a.title, "avisando lembrete");
                crate::events::publish(
                    &bus,
                    crate::events::AppEvent::ProgrammeStarting {
                        programme_id: a.programme_id,
                        channel_id: a.channel_id,
                        channel_name: a.channel_name,
                        title: a.title,
                        starts_at: a.starts_at,
                        user_id: a.user_id,
                    },
                );
            }
        }
    });
}

/// O vigia da grade: reimporta as fontes antes de a programação acabar.
///
/// Sem ele o guia simplesmente termina. Medido neste servidor: a última
/// importação foi manual, e a grade cobria até **38 horas** à frente — passado
/// esse prazo a aba "ao vivo" fica sem programa nenhum, sem erro e sem aviso.
///
/// Duas condições disparam a reimportação, e as duas existem por motivos
/// diferentes:
///
///  - **cobertura futura abaixo de 24h** — é a que impede o guia de secar,
///    qualquer que seja o tamanho da grade que o provedor publica;
///  - **última importação há mais de 6h** — é a que traz reprogramação. O
///    provedor muda horário sem mudar o fim da grade, e sem isto o Odeon
///    mostraria a grade velha por dois dias.
///
/// E um piso de 55 minutos entre importações da mesma fonte: um provedor que
/// só publique 12h de grade cairia na primeira condição a cada passada, e o
/// vigia viraria um laço de download.
pub fn vigiar_grade(pool: PgPool, http: reqwest::Client, artwork_dir: PathBuf) {
    tokio::spawn(async move {
        let mut relogio = tokio::time::interval(std::time::Duration::from_secs(30 * 60));
        loop {
            relogio.tick().await;

            let fontes: Vec<Uuid> = sqlx::query_scalar(
                r#"
                SELECT s.id
                FROM channel_source s
                LEFT JOIN LATERAL (
                    SELECT max(p.starts_at) AS fim
                    FROM programme p
                    JOIN channel c ON c.id = p.channel_id
                    WHERE c.source_id = s.id
                ) g ON true
                WHERE s.xmltv_url IS NOT NULL AND btrim(s.xmltv_url) <> ''
                  AND (s.last_import_at IS NULL
                       OR s.last_import_at < now() - interval '55 minutes')
                  AND (g.fim IS NULL
                       OR g.fim < now() + interval '24 hours'
                       OR s.last_import_at IS NULL
                       OR s.last_import_at < now() - interval '6 hours')
                "#,
            )
            .fetch_all(&pool)
            .await
            .unwrap_or_default();

            for id in fontes {
                match importar(&pool, &http, &artwork_dir, id, None).await {
                    Ok((canais, programas)) => {
                        tracing::info!(%id, canais, programas, "grade reimportada pelo vigia");
                    }
                    Err(e) => {
                        tracing::warn!(%id, error = %e, "vigia não conseguiu reimportar");
                        registrar_erro(&pool, id, &e.to_string()).await;
                    }
                }
            }
        }
    });
}
