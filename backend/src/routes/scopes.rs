//! Identificação com a PASTA como unidade de decisão.
//!
//! A medição que justifica este módulo: num acervo real, 7.568 arquivos por
//! identificar estavam em apenas **578 diretórios**, e 97,3% dos diretórios que
//! já tinham episódios casados apontavam para exatamente uma série. O nome da
//! obra está na pasta mesmo quando o nome do arquivo é ilegível.
//!
//! Decidir arquivo por arquivo não é só lento — é pedir a mesma resposta 499
//! vezes.

use axum::extract::{Query, State};
use axum::Json;
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::AppResult;
use crate::metadata;
use crate::models::{
    ScopeIdentify, ScopeQuery, ScopeRecord, ScopeRow, ScopeSearch, SiblingMatch,
};
use crate::scanner::guess::{guess_from_filename, guess_from_path, Guess};
use crate::AppState;

/// Ordenação por whitelist — o único trecho montado por concatenação, igual ao
/// `works::order_by()`. Devolve `&'static str`: o que vai pra SQL é literal
/// escolhido aqui, nunca texto que veio do cliente.
fn ordem(sort: Option<&str>) -> &'static str {
    match sort {
        Some("path") => "dir_path ASC",
        // A ordenação do modo conferência, e ela precisa existir porque o
        // padrão é inútil lá: pasta já conferida tem zero pendentes, então no
        // `pendentes DESC` todas empatam e a ordem vira o desempate alfabético.
        Some("identificadas_recentes") => "identificada_em DESC NULLS LAST, dir_path ASC",
        _ => "pendentes DESC, dir_path ASC",
    }
}

/// Que pastas a listagem devolve. Mesma disciplina do `ordem`: literal.
fn recorte(mostrar: Option<&str>) -> &'static str {
    match mostrar {
        // NÃO é `pendentes = 0`. Uma pasta meio identificada é as duas coisas
        // ao mesmo tempo — trabalho a fazer e decisão a conferir — e tem que
        // aparecer nos dois modos. No acervo onde isto foi medido são 48
        // pastas nessa situação; o complemento perderia todas.
        Some("identificadas") => "ja_identificados > 0",
        Some("todas") => "TRUE",
        // `pendentes` e qualquer valor torto. O padrão é o comportamento de
        // sempre: a fila de trabalho é o uso normal, e um erro de digitação
        // não pode transformá-la em erro.
        _ => "pendentes > 0",
    }
}

/// As pastas com trabalho pendente, mais o que ajuda a decidir sobre cada uma.
pub async fn list(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<ScopeQuery>,
) -> AppResult<Json<Value>> {
    #[derive(sqlx::FromRow)]
    struct Agg {
        dir_path: String,
        library_id: Uuid,
        library_name: String,
        pendentes: i64,
        unmatched: i64,
        needs_review: i64,
        ja_identificados: i64,
        identificada_em: Option<chrono::DateTime<chrono::Utc>>,
        exemplos: Vec<String>,
    }

    let order = ordem(params.sort.as_deref());
    let mostrar = recorte(params.mostrar.as_deref());

    let limit = params.limit.clamp(1, 500);

    // A CTE mora numa variável só, e as DUAS consultas abaixo — a da página e a
    // da contagem — são montadas a partir dela com o MESMO `{mostrar}`.
    //
    // Isso é estrutura, não capricho: antes a contagem era uma consulta escrita
    // à parte, com o `pendentes > 0` repetido na mão como um `HAVING`. Duas
    // cópias do mesmo predicado é uma que fica pra trás, e o sintoma dessa é a
    // paginação mentir — dizer 500 e desenhar 12.
    let por_pasta = r#"
        SELECT
            m.dir_path,
            m.library_id,
            l.name AS library_name,
            count(*) FILTER (WHERE w.match_state IN ('unmatched','needs_review')) AS pendentes,
            count(*) FILTER (WHERE w.match_state = 'unmatched')    AS unmatched,
            count(*) FILTER (WHERE w.match_state = 'needs_review') AS needs_review,
            count(*) FILTER (WHERE w.match_state IN ('auto','confirmed')) AS ja_identificados,
            max(w.matched_at) FILTER (WHERE w.match_state IN ('auto','confirmed'))
                AS identificada_em,
            -- O COALESCE não é defensivo, é obrigatório: `array_agg` FILTER
            -- devolve NULL quando nada casa o filtro, e `NULL[1:3]` continua
            -- NULL. Numa pasta já toda identificada não há pendente nenhum, e
            -- sem isto o sqlx estoura no decode de `Vec<String>` — 500 em todas
            -- as 163 pastas que o `mostrar=identificadas` trouxe.
            --
            -- Enquanto a listagem era fixa em `pendentes > 0` o caso não
            -- existia: toda pasta listada tinha ao menos um pendente. Foi o
            -- recorte novo que abriu a porta, não a coluna que mudou.
            --
            -- Vazio é a resposta certa, e não uma ausência: "exemplos do que
            -- falta identificar" numa pasta onde não falta nada é uma lista
            -- vazia mesmo.
            COALESCE(
                (array_agg(m.filename ORDER BY m.size_bytes DESC)
                    FILTER (WHERE w.match_state IN ('unmatched','needs_review')))[1:3],
                ARRAY[]::text[]
            ) AS exemplos
        FROM media_file m
        JOIN work w    ON w.id = m.work_id
        JOIN library l ON l.id = m.library_id
        WHERE m.status = 'probed'
          AND l.provider_hint <> 'none'
          AND ($1::uuid IS NULL OR m.library_id = $1)
          AND ($2::text IS NULL OR m.dir_path ILIKE '%' || $2 || '%')
        GROUP BY m.dir_path, m.library_id, l.name
    "#;

    let sql = format!(
        r#"
        WITH por_pasta AS ({por_pasta})
        SELECT * FROM por_pasta
        WHERE {mostrar}
        ORDER BY {order}
        LIMIT $3 OFFSET $4
        "#
    );

    let rows: Vec<Agg> = sqlx::query_as(&sql)
        .bind(params.library)
        .bind(params.q.as_deref())
        .bind(limit)
        .bind(params.offset.max(0))
        .fetch_all(&state.pool)
        .await?;

    // Mesma CTE, mesmo `{mostrar}`. O total conta exatamente o conjunto que a
    // página pagina.
    let total: i64 = sqlx::query_scalar(&format!(
        "WITH por_pasta AS ({por_pasta}) SELECT count(*) FROM por_pasta WHERE {mostrar}"
    ))
    .bind(params.library)
    .bind(params.q.as_deref())
    .fetch_one(&state.pool)
    .await?;

    let dirs: Vec<String> = rows.iter().map(|r| r.dir_path.clone()).collect();
    let siblings = siblings_for(&state, &dirs).await?;
    let escopos = scopes_for(&state, &dirs).await?;

    let items: Vec<ScopeRow> = rows
        .into_iter()
        .map(|r| {
            // O nome da pasta passa pelo mesmo parser dos arquivos: tira ano,
            // grupo de release e lixo de codec. "Naruto Shippuuden (2007)
            // [1080p]" vira "Naruto Shippuuden".
            let base = r.dir_path.rsplit('/').next().unwrap_or(&r.dir_path);
            let titulo_sugerido = guess_from_filename(base).title;

            ScopeRow {
                sibling_match: siblings.get(&r.dir_path).cloned(),
                escopo: escopos.get(&r.dir_path).cloned(),
                dir_path: r.dir_path,
                library_id: r.library_id,
                library_name: r.library_name,
                pendentes: r.pendentes,
                unmatched: r.unmatched,
                needs_review: r.needs_review,
                ja_identificados: r.ja_identificados,
                identificada_em: r.identificada_em,
                exemplos: r.exemplos,
                titulo_sugerido,
            }
        })
        .collect();

    Ok(Json(json!({
        "total": total,
        "limit": limit,
        "offset": params.offset.max(0),
        "items": items,
    })))
}

/// A obra que os arquivos JÁ identificados de cada pasta apontam.
///
/// Não é heurística de nome: é o que o próprio acervo já decidiu para vizinhos.
/// Quando existe, costuma ser a resposta — e economiza a busca no provider.
async fn siblings_for(
    state: &AppState,
    dirs: &[String],
) -> AppResult<HashMap<String, SiblingMatch>> {
    if dirs.is_empty() {
        return Ok(HashMap::new());
    }

    #[derive(sqlx::FromRow)]
    struct Row {
        dir_path: String,
        provider: String,
        provider_id: String,
        titulo: String,
        obras: i64,
    }

    // DISTINCT ON pega o provider mais frequente por pasta. O título vem da
    // coleção da série quando existe — é lá que o nome da SÉRIE mora (§8), e
    // não no título da obra, que num episódio é o nome do episódio.
    let rows: Vec<Row> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (t.dir_path)
            t.dir_path, t.provider, t.provider_id, t.titulo, t.obras
        FROM (
            SELECT
                m.dir_path,
                e.key   AS provider,
                e.value #>> '{}' AS provider_id,
                coalesce(
                    max(c.title) FILTER (WHERE c.kind = 'series'),
                    max(w.title)
                ) AS titulo,
                count(*) AS obras
            FROM media_file m
            JOIN work w ON w.id = m.work_id
            CROSS JOIN LATERAL jsonb_each(w.external_ids) e
            LEFT JOIN collection_item ci ON ci.work_id = w.id
            LEFT JOIN collection c       ON c.id = ci.collection_id AND c.kind = 'series'
            WHERE m.dir_path = ANY($1)
              AND w.match_state IN ('auto', 'confirmed')
            GROUP BY m.dir_path, e.key, e.value
        ) t
        ORDER BY t.dir_path, t.obras DESC
        "#,
    )
    .bind(dirs)
    .fetch_all(&state.pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.dir_path,
                SiblingMatch {
                    provider: r.provider,
                    provider_id: r.provider_id,
                    titulo: r.titulo,
                    obras: r.obras,
                },
            )
        })
        .collect())
}

/// Busca a obra a partir do que a pasta diz.
///
/// Não persiste em `match_candidate`: isto é busca de SÉRIE pra uma pasta, não
/// tentativa de match de uma obra. Misturar as duas encheria o histórico de
/// auditoria de linhas que não correspondem a decisão nenhuma sobre a obra.
pub async fn search(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(body): Json<ScopeSearch>,
) -> AppResult<Json<Value>> {
    let base = body.dir_path.rsplit('/').next().unwrap_or(&body.dir_path);
    let termo = body
        .query
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(|| guess_from_filename(base).title);

    if termo.trim().is_empty() {
        return Err(crate::error::AppError::BadRequest(
            "não consegui deduzir um título da pasta — mande `query`".into(),
        ));
    }

    // Um guess sintético que representa a PASTA: episódio presente pra que a
    // busca vá em séries, que é o caso de 97% das pastas do backlog.
    let guess = Guess {
        title: termo.clone(),
        episode: Some(1),
        ..Default::default()
    };

    let hint = body.provider.as_deref().unwrap_or("auto");
    let candidatos = metadata::search(&state.providers, &guess, hint).await;

    Ok(Json(json!({ "consultado": termo, "candidatos": candidatos })))
}

/// Aplica uma decisão humana à pasta inteira.
///
/// A REGRA QUE PRESERVA A FILOSOFIA (§8b): um arquivo só vira `confirmed` se o
/// seu PRÓPRIO número de episódio resolver. Quando não resolve, ele recebe a
/// identidade da série, entra na coleção e fica em `needs_review` com a razão
/// gravada — nunca `confirmed` com título "Episódio N", que é afirmar uma
/// certeza que não existe.
///
/// # A prévia responde na hora; a escrita vira job — **R81**
///
/// Em 25/08/2026 esta rota morreu no ar aplicando o Popeye. O escopo foi
/// gravado às 05:37:57, os episódios foram confirmados **um por segundo** até
/// 05:40:01, e aí o túnel registrou
/// `Incoming request ended abruptly: context canceled`. São **124 segundos** —
/// e a Cloudflare, que fica entre o navegador e este servidor, corta resposta
/// de origem por volta dos 100. O navegador não recebeu nada e mostrou
/// `TypeError: NetworkError when attempting to fetch resource`, que não diz
/// nada sobre a causa.
///
/// Dos 215 episódios, 149 tinham sido processados e 66 nunca foram alcançados:
/// o `Drop` do axum cancelou o handler junto com a conexão. **Não é um limite
/// que dá pra empurrar** — a 1,2 obra por segundo (cada uma baixa a arte do
/// episódio), 100 segundos comportam ~120 arquivos, e a fila tem pastas de
/// 388, 331 e 313. Qualquer teto que a gente escolhesse seria o mesmo defeito
/// mais tarde.
///
/// Então a resposta deixa de esperar o trabalho. `dry_run` — que é a prévia, e
/// custou 0,28 s medidos em 103 obras — continua respondendo direto, porque é
/// dela que a tela precisa ANTES de confirmar. Só a escrita abre job.
pub async fn identify(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    Json(body): Json<ScopeIdentify>,
) -> AppResult<Json<Value>> {
    // A prévia é barata e é síncrona de propósito: quem está decidindo precisa
    // ver o que vai acontecer sem ter que ficar consultando job nenhum.
    if body.dry_run {
        return aplicar(&state, user.id, body, None).await.map(Json);
    }

    let pasta = body.dir_path.clone();
    let Some(job) = crate::jobs::Job::start(
        &state.pool,
        "scope_apply",
        json!({ "pasta": pasta, "provider_id": body.provider_id }),
        Some(user.id),
    )
    .await
    else {
        // Mesmo cuidado do §34: `Job::start` devolve `None` tanto para "já há
        // um rodando" quanto para "o banco recusou", e responder a primeira
        // coisa quando foi a segunda é errar em silêncio. Pergunta antes de
        // afirmar.
        let ativo = crate::jobs::latest(&state.pool, "scope_apply")
            .await
            .map(|j| j.state == "running")
            .unwrap_or(false);
        return Ok(Json(json!({
            "started": false,
            "reason": if ativo {
                "já há uma pasta sendo aplicada — só uma por vez, pra não atropelar o TMDB"
            } else {
                "o banco recusou abrir o job — confira o CHECK de job.kind"
            },
        })));
    };

    let id = job.id;
    let estado = state.clone();
    let no_job = pasta.clone();
    tokio::spawn(async move {
        let resultado = aplicar(&estado, user.id, body, Some(&job)).await;
        match resultado {
            Ok(resumo) => {
                let estado_final = if job.cancelled().await {
                    "cancelled"
                } else {
                    "succeeded"
                };
                job.finish(&resumo, estado_final, None).await;
            }
            // O erro fica NO JOB, não só no log: sem isto a tela veria o job
            // sumir da lista de ativos sem nunca dizer o que houve (§8b).
            Err(e) => {
                let erro = e.to_string();
                tracing::warn!(erro = %erro, pasta = %no_job, "escopo não aplicou");
                job.finish(&json!({ "pasta": no_job }), "failed", Some(erro))
                    .await;
            }
        }
    });

    Ok(Json(json!({
        "started": true,
        "job_id": id,
        "pasta": pasta,
        "acompanhe": format!("/api/jobs/{id}"),
    })))
}

/// O miolo do `identify`, chamável de dentro do servidor — **R76**.
///
/// A rota continua sendo a porta pra gente; esta função é a porta pro
/// `identificar-pastas`, que propõe uma série por pasta e aplica as que
/// passam do limiar. **Uma implementação só**: se a aplicação automática
/// usasse outro caminho, ela e a manual poderiam divergir sobre o que "aplicar
/// uma pasta" significa — e a manual é a que tem anos de regra escrita dentro.
///
/// `job` é `Some` quando quem chamou é a rota com `dry_run: false` (R81), e
/// `None` quando é o `identificar-pastas` — que já roda dentro do job dele e
/// não pode abrir outro, porque só cabe um ativo por `kind`.
pub async fn aplicar(
    state: &AppState,
    quem: Uuid,
    body: ScopeIdentify,
    job: Option<&crate::jobs::Job>,
) -> AppResult<Value> {
    let tmdb = state
        .providers
        .tmdb
        .clone()
        .ok_or_else(|| crate::error::AppError::BadRequest("TMDB_API_KEY não configurada".into()))?;

    if body.provider != "tmdb" {
        return Err(crate::error::AppError::BadRequest(
            "por ora o escopo por pasta só resolve episódio no TMDB".into(),
        ));
    }

    #[derive(sqlx::FromRow)]
    struct Alvo {
        id: Uuid,
        path: String,
        filename: String,
        root_path: String,
        default_kind: String,
        provider_hint: String,
    }

    // `recursive` decide se a subárvore entra: uma série com pastas de
    // temporada precisa, uma pasta solta não.
    let alvos: Vec<Alvo> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (w.id)
            w.id, m.path, m.filename, l.root_path, l.default_kind, l.provider_hint
        FROM work w
        JOIN media_file m ON m.work_id = w.id AND m.status = 'probed'
        JOIN library l    ON l.id = m.library_id
        WHERE m.library_id = $1
          AND ($3 = false AND m.dir_path = $2
            OR $3 = true  AND (m.dir_path = $2 OR m.dir_path LIKE $2 || '/%'))
          AND ($4 OR w.match_state IN ('unmatched', 'needs_review'))
        ORDER BY w.id, m.size_bytes DESC
        "#,
    )
    .bind(body.library_id)
    .bind(&body.dir_path)
    .bind(body.recursive)
    .bind(body.force)
    .fetch_all(&state.pool)
    .await?;

    // O índice de temporadas serve ao mapeamento absoluto. Uma chamada.
    let temporadas = if body.numbering == "absolute" {
        tmdb.series_seasons(&body.provider_id).await.unwrap_or_default()
    } else {
        Vec::new()
    };

    // Resolve o par (temporada, episódio) de cada arquivo ANTES de tocar na
    // rede de novo — assim as temporadas necessárias são buscadas uma vez cada,
    // em vez de uma vez por arquivo.
    struct Resolvido {
        alvo: Alvo,
        season: Option<i32>,
        episode: Option<i32>,
        motivos: Vec<String>,
    }

    let mut resolvidos = Vec::with_capacity(alvos.len());
    for alvo in alvos {
        let guess = guess_from_path(
            std::path::Path::new(&alvo.path),
            std::path::Path::new(&alvo.root_path),
            alvo.default_kind == "episode",
        );

        let mut motivos = vec![format!("escopo definido por você em {}", body.dir_path)];
        let (season, episode) = match body.numbering.as_str() {
            "none" => (None, None),
            "absolute" => {
                let bruto = guess.any_episode().map(|e| e + body.absolute_offset);
                match bruto.and_then(|a| metadata::absolute_to_seasonal(&temporadas, a)) {
                    Some((s, e)) => {
                        motivos.push(format!(
                            "absoluto {} → S{s:02}E{e:02} pelo índice cumulativo do TMDB",
                            bruto.unwrap()
                        ));
                        (Some(s), Some(e))
                    }
                    None => {
                        motivos.push(
                            "não consegui mapear a numeração absoluta pra temporada".into(),
                        );
                        (None, None)
                    }
                }
            }
            // seasonal
            _ => {
                let s = body.season_number.or(guess.season);
                let e = guess.any_episode();
                if e.is_none() {
                    motivos.push("o número do episódio não está neste arquivo".into());
                }
                (s, e)
            }
        };

        resolvidos.push(Resolvido {
            alvo,
            season,
            episode,
            motivos,
        });
    }

    // Uma chamada por TEMPORADA distinta, não por arquivo.
    let mut necessarias: Vec<i32> = resolvidos
        .iter()
        .filter(|r| r.episode.is_some())
        .filter_map(|r| r.season)
        .collect();
    necessarias.sort_unstable();
    necessarias.dedup();

    // A temporada inteira, guardada NA ORDEM em que o provider devolve.
    //
    // A ordem importa: o TMDB numera episódio de forma inconsistente entre
    // séries. Em Naruto Shippuden, por exemplo, a temporada 8 vem com
    // `episode_number` de 152 a 175 — numeração absoluta — e não de 1 a 24.
    // Procurar "o episódio 2 da temporada 8" pelo número não acha nada.
    let mut temporadas_carregadas: HashMap<i32, Vec<crate::metadata::tmdb::SeasonEpisode>> =
        HashMap::new();
    for numero in &necessarias {
        if let Ok(detalhe) = tmdb.season(&body.provider_id, *numero).await {
            temporadas_carregadas.insert(*numero, detalhe.episodes);
        }
    }

    // A busca em si vive em `metadata::find_episode` — é a mesma regra que o
    // `apply_candidate` usa, e duplicá-la foi justamente o que fez o preview
    // discordar da escrita.
    fn achar<'a>(
        temporadas: &'a HashMap<i32, Vec<crate::metadata::tmdb::SeasonEpisode>>,
        season: i32,
        numero: i32,
    ) -> Option<&'a crate::metadata::tmdb::SeasonEpisode> {
        metadata::find_episode(temporadas.get(&season)?, numero)
    }

    let mut preview = Vec::new();
    let mut confirmariam = 0usize;
    let mut revisariam = 0usize;

    for r in &resolvidos {
        let detalhe = r
            .season
            .zip(r.episode)
            .and_then(|(s, e)| achar(&temporadas_carregadas, s, e));

        // Só é `confirmed` quando o episódio REALMENTE resolveu no provider.
        let estado = if detalhe.is_some() { "confirmed" } else { "needs_review" };
        if estado == "confirmed" {
            confirmariam += 1;
        } else {
            revisariam += 1;
        }

        if preview.len() < 25 {
            preview.push(json!({
                "work_id": r.alvo.id,
                "arquivo": r.alvo.filename,
                "temporada": r.season,
                "episodio": r.episode,
                "titulo_resolvido": detalhe.and_then(|d| d.name.clone()),
                "estado": estado,
                "motivos": r.motivos,
            }));
        }
    }

    let resumo = json!({
        "dry_run": body.dry_run,
        "pasta": body.dir_path,
        "afetados": resolvidos.len(),
        "confirmariam": confirmariam,
        "ficariam_em_revisao": revisariam,
        "chamadas_de_temporada": necessarias.len(),
        "preview": preview,
    });

    if body.dry_run {
        return Ok(resumo);
    }

    // --- a partir daqui escreve -------------------------------------------

    let serie = tmdb.tv_by_id(&body.provider_id).await.map_err(|e| {
        crate::error::AppError::BadRequest(format!("não consegui carregar a série: {e}"))
    })?;

    sqlx::query(
        r#"
        INSERT INTO identification_scope
            (library_id, dir_path, recursive, provider, provider_id, provider_kind,
             season_number, numbering, absolute_offset, note, decided_by)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
        ON CONFLICT (library_id, dir_path) DO UPDATE SET
            recursive = EXCLUDED.recursive,
            provider = EXCLUDED.provider,
            provider_id = EXCLUDED.provider_id,
            provider_kind = EXCLUDED.provider_kind,
            season_number = EXCLUDED.season_number,
            numbering = EXCLUDED.numbering,
            absolute_offset = EXCLUDED.absolute_offset,
            note = EXCLUDED.note,
            decided_by = EXCLUDED.decided_by,
            decided_at = now()
        "#,
    )
    .bind(body.library_id)
    .bind(&body.dir_path)
    .bind(body.recursive)
    .bind(&body.provider)
    .bind(&body.provider_id)
    .bind(&body.provider_kind)
    .bind(body.season_number)
    .bind(&body.numbering)
    .bind(body.absolute_offset)
    .bind(&body.note)
    .bind(quem)
    .execute(&state.pool)
    .await?;

    let mut aplicados = 0usize;
    let mut falhas = Vec::new();
    let mut parou_em = None;

    // O `total` chega no job aqui, e não na resposta da rota: a rota devolve
    // antes de esta função ter olhado o disco, e prometer um número que ainda
    // não foi contado é o tipo de metadado inventado que o §18 proíbe.
    if let Some(j) = job {
        j.tick(
            &json!({ "pasta": body.dir_path, "aplicados": 0 }),
            0,
            Some(resolvidos.len() as i64),
        )
        .await;
    }

    for (i, r) in resolvidos.iter().enumerate() {
        // Entre itens, nunca no meio de um. Cancelar depois do
        // `persist_candidate` e antes do `apply_candidate` deixaria candidato
        // gravado sem obra aplicada — exatamente o estado pela metade que o
        // cancelamento cooperativo existe pra evitar.
        if let Some(j) = job {
            if j.cancelled().await {
                parou_em = Some(i);
                break;
            }
        }

        let resolveu = r
            .season
            .zip(r.episode)
            .and_then(|(s, e)| achar(&temporadas_carregadas, s, e))
            .is_some();

        let motivos = serde_json::to_value(&r.motivos).unwrap_or_else(|_| json!([]));

        // NÃO resolveu o episódio: não escreve metadado nenhum.
        //
        // Poderia gravar a identidade da série e deixar em revisão — mas isso
        // é escrever sob incerteza, e o §8b é explícito: quando o matcher não
        // sabe, ele marca e pergunta. O que muda em relação a antes é que agora
        // a pergunta vem com o motivo escrito.
        if !resolveu {
            let _ = sqlx::query(
                "UPDATE work SET match_state = 'needs_review', match_reasons = $2,
                                 updated_at = now()
                 WHERE id = $1 AND ($3 OR match_state IN ('unmatched','needs_review'))",
            )
            .bind(r.alvo.id)
            .bind(&motivos)
            .bind(body.force)
            .execute(&state.pool)
            .await;
            continue;
        }

        let work = crate::metadata::WorkToMatch {
            id: r.alvo.id,
            path: r.alvo.path.clone(),
            root_path: r.alvo.root_path.clone(),
            provider_hint: r.alvo.provider_hint.clone(),
            default_kind: r.alvo.default_kind.clone(),
            // O escopo já é conhecido aqui — não há o que consultar de novo.
            dir_path: None,
            library_id: None,
        };

        // O guess entregue ao `apply_candidate` carrega a numeração que o
        // ESCOPO resolveu — é assim que a decisão da pasta chega no episódio,
        // em vez de o `apply_candidate` re-derivar do caminho e ignorá-la.
        let mut guess = guess_from_path(
            std::path::Path::new(&r.alvo.path),
            std::path::Path::new(&r.alvo.root_path),
            r.alvo.default_kind == "episode",
        );
        guess.season = r.season;
        guess.episode = r.episode;
        guess.absolute_episode = None;

        // O candidato é gravado com score 1.0: a decisão foi humana, e o
        // histórico de auditoria precisa poder responder "quem escolheu isto".
        let pontuacao = crate::metadata::score::Score {
            value: 1.0,
            reasons: r.motivos.clone(),
        };

        let resultado = async {
            let candidate_id =
                crate::metadata::persist_candidate(&state.pool, r.alvo.id, &serie, &pontuacao)
                    .await?;
            crate::metadata::apply_candidate(
                &state.pool,
                &state.providers,
                &state.config.artwork_dir,
                &work,
                &guess,
                &serie,
                candidate_id,
                1.0,
                "confirmed",
            )
            .await?;
            sqlx::query("UPDATE work SET match_reasons = $2 WHERE id = $1")
                .bind(r.alvo.id)
                .bind(&motivos)
                .execute(&state.pool)
                .await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;

        match resultado {
            Ok(()) => aplicados += 1,
            Err(e) => {
                if falhas.len() < 20 {
                    falhas.push(json!({ "arquivo": r.alvo.filename, "erro": e.to_string() }));
                }
            }
        }

        // A cada 5, e não a cada um: a 1,2 obra por segundo, ticar sempre
        // dobraria o número de idas ao banco por item pra ganhar uma barra de
        // progresso 4 segundos mais fina.
        if let Some(j) = job {
            if (i + 1) % 5 == 0 || i + 1 == resolvidos.len() {
                j.tick(
                    &json!({
                        "pasta": body.dir_path,
                        "aplicados": aplicados,
                        "arquivo": r.alvo.filename,
                    }),
                    i as i64 + 1,
                    Some(resolvidos.len() as i64),
                )
                .await;
            }
        }
    }

    // `processados` existe por causa do Popeye: quando o trabalho para no meio,
    // "aplicados: 112" sozinho não deixa ninguém saber se faltaram 66 arquivos
    // ou se eram só esses. O número que responde isso é quantos foram VISTOS.
    let processados = parou_em.unwrap_or(resolvidos.len());

    Ok(json!({
        "dry_run": false,
        "pasta": body.dir_path,
        "aplicados": aplicados,
        "confirmados": confirmariam,
        "em_revisao": revisariam,
        "chamadas_de_temporada": necessarias.len(),
        "afetados": resolvidos.len(),
        "processados": processados,
        "cancelado": parou_em.is_some(),
        "falhas": falhas,
    }))
}

async fn scopes_for(state: &AppState, dirs: &[String]) -> AppResult<HashMap<String, ScopeRecord>> {
    if dirs.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<ScopeRecord> = sqlx::query_as(
        "SELECT id, library_id, dir_path, recursive, provider, provider_id, provider_kind,
                season_number, numbering, absolute_offset, note, decided_at
         FROM identification_scope WHERE dir_path = ANY($1)",
    )
    .bind(dirs)
    .fetch_all(&state.pool)
    .await?;

    Ok(rows.into_iter().map(|r| (r.dir_path.clone(), r)).collect())
}

#[cfg(test)]
mod tests {
    /// **R77 — `identificadas` não é o complemento de `pendentes`.**
    ///
    /// Uma pasta com cinco arquivos identificados e cinco não é as duas coisas
    /// ao mesmo tempo: trabalho a fazer e decisão a conferir. Escrever
    /// `pendentes = 0` aqui a esconderia do modo conferência — e é justamente a
    /// pasta meio feita que mais precisa ser conferida, porque foi nela que
    /// alguém parou no meio. No acervo onde isto foi medido são 48.
    #[test]
    fn identificadas_nao_e_o_complemento_de_pendentes() {
        assert_eq!(super::recorte(Some("identificadas")), "ja_identificados > 0");
        assert_eq!(super::recorte(Some("todas")), "TRUE");
        assert_eq!(super::recorte(Some("pendentes")), "pendentes > 0");
    }

    /// **O padrão não pode mudar.** A fila de trabalho é o uso normal; a
    /// conferência é a exceção. Ausente e torto caem no mesmo lugar de sempre.
    #[test]
    fn mostrar_ausente_ou_torto_e_a_fila_de_sempre() {
        assert_eq!(super::recorte(None), "pendentes > 0");
        assert_eq!(super::recorte(Some("")), "pendentes > 0");
        assert_eq!(super::recorte(Some("identificades")), "pendentes > 0");
    }

    /// **A conferência ordena pelo que foi decidido, não pelo mais pendente.**
    ///
    /// Pasta já conferida tem zero pendentes: no `pendentes DESC` todas
    /// empatariam e a ordem viraria o desempate alfabético, que não diz nada
    /// sobre o que foi decidido por último.
    #[test]
    fn a_conferencia_ordena_por_quando_foi_identificada() {
        assert_eq!(
            super::ordem(Some("identificadas_recentes")),
            "identificada_em DESC NULLS LAST, dir_path ASC"
        );
        assert_eq!(super::ordem(None), "pendentes DESC, dir_path ASC");
        assert_eq!(super::ordem(Some("path")), "dir_path ASC");
    }

    /// **A contagem e a página usam o MESMO predicado.**
    ///
    /// Antes a contagem era uma consulta escrita à parte, com o `pendentes > 0`
    /// repetido na mão num `HAVING`. Duas cópias do mesmo predicado é uma que
    /// fica pra trás, e o sintoma é a paginação mentir: dizer 500 e desenhar
    /// 12. As duas consultas passaram a sair da mesma CTE e do mesmo
    /// `{mostrar}` — este teste é o que impede a separação de voltar.
    #[test]
    fn a_contagem_pagina_o_mesmo_conjunto_que_a_pagina() {
        let fonte = include_str!("scopes.rs");
        let inteiro = fonte
            .split_once("pub async fn list")
            .expect("list sumiu")
            .1
            .split_once("let dirs:")
            .expect("o corpo de list mudou de forma")
            .0;

        // Sem as linhas de comentário: elas FALAM do `HAVING` que não pode
        // voltar, e sem isto a guarda tropeçaria na própria explicação.
        let corpo: String = inteiro
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let corpo = corpo.as_str();

        assert_eq!(
            corpo.matches("WITH por_pasta AS ({por_pasta})").count(),
            2,
            "a pagina e a contagem deixaram de sair da mesma CTE"
        );
        assert_eq!(
            corpo.matches("WHERE {mostrar}").count(),
            2,
            "a contagem deixou de usar o mesmo predicado da pagina"
        );
        assert!(
            !corpo.contains("HAVING"),
            "a contagem voltou a ter predicado proprio"
        );
    }

    /// **A query string chega mesmo no predicado.**
    ///
    /// Este é o elo que os outros testes não cobriam: eles provam que
    /// `recorte("identificadas")` devolve o predicado certo, mas não que o
    /// `mostrar=` da URL vira esse `Some("identificadas")`. Se o campo sumir da
    /// `ScopeQuery`, o serde ignora em silêncio, o endpoint responde 200 como
    /// se nada tivesse sido pedido, e a listagem volta a ser a de sempre — que
    /// é um defeito sem mensagem de erro nenhuma.
    ///
    /// Vai pelo `Query::try_from_uri`, o mesmo caminho do extrator do axum.
    #[test]
    fn a_query_string_chega_no_predicado() {
        use axum::extract::Query;
        let uri: axum::http::Uri =
            "/api/review/scopes?mostrar=identificadas&sort=identificadas_recentes"
                .parse()
                .unwrap();
        let Query(p): Query<crate::models::ScopeQuery> =
            Query::try_from_uri(&uri).expect("ScopeQuery nao aceitou a query string");

        assert_eq!(p.mostrar.as_deref(), Some("identificadas"));
        assert_eq!(p.sort.as_deref(), Some("identificadas_recentes"));
        assert_eq!(super::recorte(p.mostrar.as_deref()), "ja_identificados > 0");
        assert_eq!(
            super::ordem(p.sort.as_deref()),
            "identificada_em DESC NULLS LAST, dir_path ASC"
        );
    }

    /// E sem parâmetro nenhum, o de sempre.
    #[test]
    fn sem_parametro_a_query_string_da_a_fila_de_sempre() {
        use axum::extract::Query;
        let uri: axum::http::Uri = "/api/review/scopes".parse().unwrap();
        let Query(p): Query<crate::models::ScopeQuery> = Query::try_from_uri(&uri).unwrap();
        assert_eq!(p.mostrar, None);
        assert_eq!(super::recorte(p.mostrar.as_deref()), "pendentes > 0");
        assert_eq!(super::ordem(p.sort.as_deref()), "pendentes DESC, dir_path ASC");
    }

    /// **`exemplos` não pode voltar nulo.**
    ///
    /// `array_agg(...) FILTER (...)` devolve NULL quando nada casa o filtro, e
    /// `NULL[1:3]` continua NULL. O campo é `Vec<String>`, então NULL vira erro
    /// de decode e 500 — não uma lista vazia.
    ///
    /// Enquanto a listagem era fixa em `pendentes > 0`, o caso era impossível:
    /// toda pasta listada tinha ao menos um pendente pra virar exemplo. Foi o
    /// `mostrar=identificadas` que abriu 163 pastas com zero pendentes, e cada
    /// uma delas derrubava o endpoint. É o preço de afrouxar um filtro que
    /// segurava um invariante sem dizer que segurava.
    #[test]
    fn exemplos_nunca_volta_nulo() {
        let fonte = include_str!("scopes.rs");
        let cte = fonte
            .split_once("let por_pasta = r#\"")
            .expect("a CTE sumiu")
            .1
            .split_once("\"#;")
            .expect("a CTE mudou de forma")
            .0;

        let exemplos = cte
            .split_once("AS exemplos")
            .expect("a coluna exemplos sumiu")
            .0;
        assert!(
            exemplos.contains("COALESCE(") && exemplos.contains("ARRAY[]::text[]"),
            "exemplos voltou a poder ser NULL — pasta sem pendente derruba o endpoint"
        );
    }
}
