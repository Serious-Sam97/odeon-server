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
        exemplos: Vec<String>,
    }

    // Ordenação por whitelist — o único trecho montado por concatenação, igual
    // ao `works::order_by()`.
    let order = match params.sort.as_deref() {
        Some("path") => "dir_path ASC",
        _ => "pendentes DESC, dir_path ASC",
    };

    let limit = params.limit.clamp(1, 500);

    let sql = format!(
        r#"
        WITH por_pasta AS (
            SELECT
                m.dir_path,
                m.library_id,
                l.name AS library_name,
                count(*) FILTER (WHERE w.match_state IN ('unmatched','needs_review')) AS pendentes,
                count(*) FILTER (WHERE w.match_state = 'unmatched')    AS unmatched,
                count(*) FILTER (WHERE w.match_state = 'needs_review') AS needs_review,
                count(*) FILTER (WHERE w.match_state IN ('auto','confirmed')) AS ja_identificados,
                (array_agg(m.filename ORDER BY m.size_bytes DESC)
                    FILTER (WHERE w.match_state IN ('unmatched','needs_review')))[1:3] AS exemplos
            FROM media_file m
            JOIN work w    ON w.id = m.work_id
            JOIN library l ON l.id = m.library_id
            WHERE m.status = 'probed'
              AND l.provider_hint <> 'none'
              AND ($1::uuid IS NULL OR m.library_id = $1)
              AND ($2::text IS NULL OR m.dir_path ILIKE '%' || $2 || '%')
            GROUP BY m.dir_path, m.library_id, l.name
        )
        SELECT * FROM por_pasta
        WHERE pendentes > 0
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

    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*) FROM (
            SELECT m.dir_path
            FROM media_file m
            JOIN work w    ON w.id = m.work_id
            JOIN library l ON l.id = m.library_id
            WHERE m.status = 'probed'
              AND l.provider_hint <> 'none'
              AND ($1::uuid IS NULL OR m.library_id = $1)
              AND ($2::text IS NULL OR m.dir_path ILIKE '%' || $2 || '%')
            GROUP BY m.dir_path
            HAVING count(*) FILTER (WHERE w.match_state IN ('unmatched','needs_review')) > 0
        ) t
        "#,
    )
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
pub async fn identify(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    Json(body): Json<ScopeIdentify>,
) -> AppResult<Json<Value>> {
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
        return Ok(Json(resumo));
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
    .bind(user.id)
    .execute(&state.pool)
    .await?;

    let mut aplicados = 0usize;
    let mut falhas = Vec::new();

    for r in &resolvidos {
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
    }

    Ok(Json(json!({
        "dry_run": false,
        "pasta": body.dir_path,
        "aplicados": aplicados,
        "confirmados": confirmariam,
        "em_revisao": revisariam,
        "chamadas_de_temporada": necessarias.len(),
        "falhas": falhas,
    })))
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
