pub mod anilist;
pub mod score;
pub mod tmdb;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::artwork;
use crate::scanner::guess::{guess_from_path, Guess};

/// Intervalo entre obras. O TMDB aguenta muito mais, mas não há pressa e não
/// custa ser educado com API de graça.
const PROVIDER_DELAY: Duration = Duration::from_millis(200);

/// Resultado normalizado de qualquer provider.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub provider: String,
    pub provider_id: String,
    /// movie | tv | anime
    pub provider_kind: String,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    /// Gêneros do provider — viram tags no namespace `genre`.
    pub genres: Vec<String>,
    /// O AniList entrega isso de graça; no TMDB sai do pôster baixado.
    pub accent_color: Option<String>,
    pub popularity: f32,
    pub raw: serde_json::Value,
}

#[derive(Clone)]
pub struct Providers {
    pub tmdb: Option<tmdb::Tmdb>,
    pub anilist: anilist::AniList,
    pub http: reqwest::Client,
}

impl Providers {
    pub fn new(http: reqwest::Client, tmdb_key: Option<String>) -> Self {
        let tmdb = tmdb_key.map(|key| tmdb::Tmdb::new(http.clone(), key));
        if tmdb.is_none() {
            tracing::warn!(
                "TMDB_API_KEY não definida — filmes e séries não serão identificados. \
                 O AniList (anime) segue funcionando, não precisa de chave."
            );
        }
        Self {
            anilist: anilist::AniList::new(http.clone()),
            tmdb,
            http,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MatchStatus {
    pub running: bool,
    pub tmdb_enabled: bool,
    pub current: Option<String>,
    pub works_seen: u64,
    pub matched_auto: u64,
    pub needs_review: u64,
    pub still_unmatched: u64,
    pub errors: Vec<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

pub type SharedMatchStatus = Arc<Mutex<MatchStatus>>;

#[derive(Debug, sqlx::FromRow)]
pub struct WorkToMatch {
    pub id: Uuid,
    pub path: String,
    pub root_path: String,
    pub provider_hint: String,
    pub default_kind: String,
}

/// Roda a identificação sobre a biblioteca. `force = true` refaz até o que já
/// estava casado (menos o que um humano confirmou — isso nunca é sobrescrito).
pub async fn run_matching(
    pool: PgPool,
    providers: Providers,
    artwork_dir: PathBuf,
    status: SharedMatchStatus,
    force: bool,
) -> bool {
    {
        let mut s = status.lock().await;
        if s.running {
            return false;
        }
        *s = MatchStatus {
            running: true,
            tmdb_enabled: providers.tmdb.is_some(),
            started_at: Some(Utc::now()),
            ..Default::default()
        };
    }

    let pending: Vec<WorkToMatch> = match sqlx::query_as(
        r#"
        SELECT DISTINCT ON (w.id)
            w.id, m.path, l.root_path, l.provider_hint, l.default_kind
        FROM work w
        JOIN media_file m ON m.work_id = w.id AND m.status = 'probed'
        JOIN library l ON l.id = m.library_id
        WHERE l.provider_hint <> 'none'
          AND w.match_state <> 'confirmed'
          AND ($1 OR w.match_state = 'unmatched')
        ORDER BY w.id, m.size_bytes DESC
        "#,
    )
    .bind(force)
    .fetch_all(&pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            let mut s = status.lock().await;
            s.errors.push(format!("falha ao listar obras: {e}"));
            s.running = false;
            s.finished_at = Some(Utc::now());
            return true;
        }
    };

    tracing::info!(total = pending.len(), force, "identificação iniciada");

    for work in pending {
        let guess = guess_from_path(Path::new(&work.path), Path::new(&work.root_path));

        {
            let mut s = status.lock().await;
            s.works_seen += 1;
            s.current = Some(guess.title.clone());
        }

        match match_one(&pool, &providers, &artwork_dir, &work, &guess).await {
            Ok(state) => {
                let mut s = status.lock().await;
                match state {
                    "auto" => s.matched_auto += 1,
                    "needs_review" => s.needs_review += 1,
                    _ => s.still_unmatched += 1,
                }
            }
            Err(e) => {
                tracing::warn!(title = %guess.title, error = %e, "identificação falhou");
                let mut s = status.lock().await;
                s.still_unmatched += 1;
                if s.errors.len() < 100 {
                    s.errors.push(format!("{}: {e}", guess.title));
                }
            }
        }

        tokio::time::sleep(PROVIDER_DELAY).await;
    }

    let mut s = status.lock().await;
    s.running = false;
    s.current = None;
    s.finished_at = Some(Utc::now());
    tracing::info!(
        auto = s.matched_auto,
        revisar = s.needs_review,
        sem_match = s.still_unmatched,
        "identificação concluída"
    );
    true
}

async fn match_one(
    pool: &PgPool,
    providers: &Providers,
    artwork_dir: &Path,
    work: &WorkToMatch,
    guess: &Guess,
) -> anyhow::Result<&'static str> {
    let candidates = search(providers, guess, &work.provider_hint).await;

    let mut scored: Vec<(Candidate, score::Score)> = candidates
        .into_iter()
        .map(|c| {
            let s = score::score_candidate(guess, &c);
            (c, s)
        })
        .collect();

    scored.sort_by(|a, b| b.1.value.total_cmp(&a.1.value));
    scored.truncate(8);

    // Toda tentativa fica gravada, mesmo a que perdeu. É o que permite auditar
    // depois "por que ele achou que era isso?".
    let mut candidate_ids = Vec::new();
    for (candidate, s) in &scored {
        let id = persist_candidate(pool, work.id, candidate, s).await?;
        candidate_ids.push(id);
    }

    let Some(((best, best_score), best_id)) = scored.first().zip(candidate_ids.first()) else {
        sqlx::query(
            "UPDATE work SET match_state = 'unmatched', match_confidence = NULL,
                             matched_at = now(), updated_at = now()
             WHERE id = $1",
        )
        .bind(work.id)
        .execute(pool)
        .await?;
        return Ok("unmatched");
    };

    let state = score::state_for(best_score.value);

    if state == "auto" {
        apply_candidate(
            pool,
            providers,
            artwork_dir,
            work,
            guess,
            best,
            *best_id,
            best_score.value,
            "auto",
        )
        .await?;
    } else {
        // Sem confiança suficiente: marca e para. Nada de sobrescrever o título
        // com um palpite — é exatamente esse o erro do Jellyfin.
        sqlx::query(
            "UPDATE work SET match_state = $2, match_confidence = $3,
                             matched_candidate_id = $4, matched_at = now(), updated_at = now()
             WHERE id = $1",
        )
        .bind(work.id)
        .bind(state)
        .bind(best_score.value)
        .bind(best_id)
        .execute(pool)
        .await?;
    }

    Ok(state)
}

pub async fn search(providers: &Providers, guess: &Guess, hint: &str) -> Vec<Candidate> {
    let mut out = Vec::new();
    let serial = guess.any_episode().is_some();

    let use_tmdb = matches!(hint, "auto" | "tmdb");
    let use_anilist = hint == "anilist" || (hint == "auto" && guess.looks_like_anime);

    if use_tmdb {
        if let Some(tmdb) = &providers.tmdb {
            // Busca primeiro o formato que o arquivo sugere; se vier vazio,
            // tenta o outro — nome de arquivo mente com frequência.
            let primary = if serial {
                tmdb.search_tv(&guess.title, guess.year).await
            } else {
                tmdb.search_movie(&guess.title, guess.year).await
            };

            match primary {
                Ok(hits) if !hits.is_empty() => out.extend(hits),
                Ok(_) => {
                    let fallback = if serial {
                        tmdb.search_movie(&guess.title, guess.year).await
                    } else {
                        tmdb.search_tv(&guess.title, guess.year).await
                    };
                    match fallback {
                        Ok(hits) => out.extend(hits),
                        Err(e) => tracing::warn!(error = %e, "TMDB (fallback) falhou"),
                    }
                }
                Err(e) => tracing::warn!(error = %e, "TMDB falhou"),
            }
        }
    }

    if use_anilist {
        match providers.anilist.search(&guess.title).await {
            Ok(hits) => out.extend(hits),
            Err(e) => tracing::warn!(error = %e, "AniList falhou"),
        }
    }

    out
}

pub async fn persist_candidate(
    pool: &PgPool,
    work_id: Uuid,
    candidate: &Candidate,
    scored: &score::Score,
) -> anyhow::Result<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO match_candidate (
            work_id, provider, provider_id, provider_kind, title, original_title,
            year, overview, poster_url, backdrop_url, accent_color, popularity,
            score, reasons, raw
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
        ON CONFLICT (work_id, provider, provider_id) DO UPDATE SET
            title = EXCLUDED.title, original_title = EXCLUDED.original_title,
            year = EXCLUDED.year, overview = EXCLUDED.overview,
            poster_url = EXCLUDED.poster_url, backdrop_url = EXCLUDED.backdrop_url,
            accent_color = EXCLUDED.accent_color, popularity = EXCLUDED.popularity,
            score = EXCLUDED.score, reasons = EXCLUDED.reasons
        RETURNING id
        "#,
    )
    .bind(work_id)
    .bind(candidate.provider.clone())
    .bind(candidate.provider_id.clone())
    .bind(candidate.provider_kind.clone())
    .bind(candidate.title.clone())
    .bind(candidate.original_title.clone())
    .bind(candidate.year)
    .bind(candidate.overview.clone())
    .bind(candidate.poster_url.clone())
    .bind(candidate.backdrop_url.clone())
    .bind(candidate.accent_color.clone())
    .bind(candidate.popularity)
    .bind(scored.value)
    .bind(serde_json::to_value(&scored.reasons)?)
    .bind(candidate.raw.clone())
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Aplica um candidato à obra: metadata, artwork, coleções (série/temporada) e
/// tags. Usado tanto pelo match automático quanto pela confirmação manual.
#[allow(clippy::too_many_arguments)]
pub async fn apply_candidate(
    pool: &PgPool,
    providers: &Providers,
    artwork_dir: &Path,
    work: &WorkToMatch,
    guess: &Guess,
    candidate: &Candidate,
    candidate_id: Uuid,
    confidence: f32,
    state: &str,
) -> anyhow::Result<()> {
    let episode_number = guess.any_episode();
    let is_episode = episode_number.is_some();

    // --- artwork -----------------------------------------------------------
    let mut artwork_json = serde_json::Map::new();
    let mut dominant = candidate.accent_color.clone();

    if let Some(url) = &candidate.poster_url {
        match artwork::fetch(&providers.http, artwork_dir, work.id, "poster", url).await {
            Ok(stored) => {
                artwork_json.insert("poster".into(), stored.path.clone().into());
                // A cor do AniList é curada; a extraída do pôster é o fallback.
                dominant = dominant.or(stored.dominant_color);
            }
            Err(e) => tracing::warn!(error = %e, "pôster não baixou"),
        }
    }
    if let Some(url) = &candidate.backdrop_url {
        match artwork::fetch(&providers.http, artwork_dir, work.id, "backdrop", url).await {
            Ok(stored) => {
                artwork_json.insert("backdrop".into(), stored.path.into());
            }
            Err(e) => tracing::warn!(error = %e, "backdrop não baixou"),
        }
    }

    // --- título, sinopse, coleções -----------------------------------------
    let mut title = candidate.title.clone();
    let mut overview = candidate.overview.clone();

    if is_episode {
        // O título da OBRA é o do episódio; o nome da série mora na coleção.
        // É exatamente pra isso que o modelo em grafo existe.
        let series_key = format!("{}:{}", candidate.provider, candidate.provider_id);
        let series_id = ensure_collection(
            pool,
            &series_key,
            "series",
            &candidate.title,
            None,
            None,
            candidate.year,
        )
        .await?;

        let season_number = guess.season.unwrap_or(1);
        let season_id = ensure_collection(
            pool,
            &format!("{series_key}:s{season_number}"),
            "season",
            &format!("Temporada {season_number}"),
            Some(series_id),
            Some(season_number),
            None,
        )
        .await?;

        sqlx::query(
            "INSERT INTO collection_item (collection_id, work_id, position)
             VALUES ($1, $2, $3)
             ON CONFLICT (collection_id, work_id) DO UPDATE SET position = EXCLUDED.position",
        )
        .bind(season_id)
        .bind(work.id)
        .bind(episode_number)
        .execute(pool)
        .await?;

        // O TMDB tem detalhe por episódio; o AniList não, então lá fica o número.
        let episode_detail = match (&providers.tmdb, candidate.provider.as_str()) {
            (Some(tmdb), "tmdb") => tmdb
                .episode(&candidate.provider_id, season_number, episode_number.unwrap_or(1))
                .await
                .ok(),
            _ => None,
        };

        match episode_detail {
            Some(detail) => {
                title = detail
                    .name
                    .clone()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| format!("Episódio {}", episode_number.unwrap_or(1)));
                overview = detail.overview.clone().filter(|o| !o.trim().is_empty());

                if let Some(still) = detail.still_url() {
                    if let Ok(stored) =
                        artwork::fetch(&providers.http, artwork_dir, work.id, "still", &still).await
                    {
                        artwork_json.insert("still".into(), stored.path.into());
                    }
                }
            }
            None => {
                title = format!("Episódio {}", episode_number.unwrap_or(1));
            }
        }
    }

    let kind = if is_episode {
        "episode".to_string()
    } else {
        work.default_kind.clone()
    };

    let mut external_ids = serde_json::Map::new();
    external_ids.insert(
        candidate.provider.clone(),
        serde_json::Value::String(candidate.provider_id.clone()),
    );

    sqlx::query(
        r#"
        UPDATE work SET
            title            = $2,
            original_title   = $3,
            year             = COALESCE($4, year),
            overview         = COALESCE($5, overview),
            kind             = $6,
            external_ids     = work.external_ids || $7,
            artwork          = $8,
            dominant_color   = COALESCE($9, dominant_color),
            match_state      = $10,
            match_confidence = $11,
            matched_candidate_id = $12,
            matched_at       = now(),
            updated_at       = now()
        WHERE id = $1
        "#,
    )
    .bind(work.id)
    .bind(title.clone())
    .bind(candidate.original_title.clone())
    .bind(candidate.year)
    .bind(overview.clone())
    .bind(kind.clone())
    .bind(serde_json::Value::Object(external_ids))
    .bind(serde_json::Value::Object(artwork_json))
    .bind(dominant.clone())
    .bind(state)
    .bind(confidence)
    .bind(candidate_id)
    .execute(pool)
    .await?;

    // 'anime' é TAG, não `kind`. Um episódio de anime continua kind='episode'.
    let format_tag = match (candidate.provider.as_str(), candidate.provider_kind.as_str()) {
        ("anilist", _) => "anime",
        (_, "movie") => "filme",
        _ => "série",
    };
    // --- elenco e equipe -----------------------------------------------
    // Só agora, com o match aceito: buscar créditos de candidato descartado
    // seria requisição jogada fora.
    match fetch_credits(providers, candidate).await {
        Ok(people) if !people.is_empty() => {
            if let Err(e) = store_credits(pool, providers, artwork_dir, work.id, &people).await {
                tracing::warn!(error = %e, "créditos não gravaram");
            }
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "créditos não vieram do provider"),
    }

    attach_tag(pool, work.id, "format", format_tag).await?;

    // Gêneros do provider viram tags no namespace `genre`. É isso que dá massa
    // crítica pra taxonomia — sem eles o sistema de tags nasce vazio.
    for genre in &candidate.genres {
        let trimmed = genre.trim();
        if !trimmed.is_empty() {
            attach_tag(pool, work.id, "genre", trimmed).await?;
        }
    }

    Ok(())
}

async fn fetch_credits(
    providers: &Providers,
    candidate: &Candidate,
) -> anyhow::Result<Vec<tmdb::CreditPerson>> {
    match candidate.provider.as_str() {
        "tmdb" => match &providers.tmdb {
            Some(client) => {
                client
                    .credits(&candidate.provider_kind, &candidate.provider_id)
                    .await
            }
            None => Ok(Vec::new()),
        },
        "anilist" => providers.anilist.credits(&candidate.provider_id).await,
        _ => Ok(Vec::new()),
    }
}

/// Grava pessoas e vínculos.
///
/// `provider_key` é o que impede a mesma pessoa virar uma linha nova a cada
/// filme — sem ele, "tudo do Villeneuve" devolveria um filme só.
async fn store_credits(
    pool: &PgPool,
    providers: &Providers,
    artwork_dir: &Path,
    work_id: Uuid,
    people: &[tmdb::CreditPerson],
) -> anyhow::Result<()> {
    // Substitui em bloco: refazer o match não pode deixar elenco antigo pra trás.
    sqlx::query("DELETE FROM credit WHERE work_id = $1")
        .bind(work_id)
        .execute(pool)
        .await?;

    for person in people {
        let person_id: Uuid = sqlx::query_scalar(
            "INSERT INTO person (name, provider_key, image_url, known_for)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (provider_key) DO UPDATE SET
                name = EXCLUDED.name,
                image_url = COALESCE(EXCLUDED.image_url, person.image_url),
                known_for = COALESCE(EXCLUDED.known_for, person.known_for),
                updated_at = now()
             RETURNING id",
        )
        .bind(&person.name)
        .bind(&person.provider_key)
        .bind(&person.image_url)
        .bind(&person.known_for)
        .fetch_one(pool)
        .await?;

        // Retrato em cache local, igual ao artwork do M1: a biblioteca segue
        // funcionando com a internet fora. Baixa uma vez por pessoa.
        let has_image: Option<String> =
            sqlx::query_scalar("SELECT image_path FROM person WHERE id = $1")
                .bind(person_id)
                .fetch_one(pool)
                .await?;

        if has_image.is_none() {
            if let Some(url) = &person.image_url {
                if let Ok(stored) =
                    artwork::fetch(&providers.http, artwork_dir, person_id, "person", url).await
                {
                    let _ = sqlx::query("UPDATE person SET image_path = $2 WHERE id = $1")
                        .bind(person_id)
                        .bind(&stored.path)
                        .execute(pool)
                        .await;
                }
            }
        }

        sqlx::query(
            "INSERT INTO credit (work_id, person_id, role, character_name, position)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (work_id, person_id, role) DO UPDATE SET
                character_name = COALESCE(EXCLUDED.character_name, credit.character_name),
                position = LEAST(credit.position, EXCLUDED.position)",
        )
        .bind(work_id)
        .bind(person_id)
        .bind(&person.role)
        .bind(&person.character_name)
        .bind(person.position)
        .execute(pool)
        .await?;
    }

    Ok(())
}

async fn ensure_collection(
    pool: &PgPool,
    provider_key: &str,
    kind: &str,
    title: &str,
    parent_id: Option<Uuid>,
    position: Option<i32>,
    year: Option<i32>,
) -> anyhow::Result<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO collection (kind, title, parent_id, position, year, provider_key)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (provider_key) DO UPDATE SET title = EXCLUDED.title
         RETURNING id",
    )
    .bind(kind)
    .bind(title)
    .bind(parent_id)
    .bind(position)
    .bind(year)
    .bind(provider_key)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

async fn attach_tag(
    pool: &PgPool,
    work_id: Uuid,
    namespace: &str,
    value: &str,
) -> anyhow::Result<()> {
    let tag_id: Uuid = sqlx::query_scalar(
        "INSERT INTO tag (namespace, value) VALUES ($1, $2)
         ON CONFLICT (namespace, value) DO UPDATE SET value = EXCLUDED.value
         RETURNING id",
    )
    .bind(namespace)
    .bind(value)
    .fetch_one(pool)
    .await?;

    sqlx::query(
        "INSERT INTO work_tag (work_id, tag_id, source) VALUES ($1, $2, 'provider')
         ON CONFLICT (work_id, tag_id) DO NOTHING",
    )
    .bind(work_id)
    .bind(tag_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Carrega o contexto de uma obra pra confirmação manual (rota de revisão).
pub async fn load_work_context(pool: &PgPool, work_id: Uuid) -> anyhow::Result<(WorkToMatch, Guess)> {
    let work: WorkToMatch = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (w.id)
            w.id, m.path, l.root_path, l.provider_hint, l.default_kind
        FROM work w
        JOIN media_file m ON m.work_id = w.id
        JOIN library l ON l.id = m.library_id
        WHERE w.id = $1
        ORDER BY w.id, m.size_bytes DESC
        "#,
    )
    .bind(work_id)
    .fetch_one(pool)
    .await?;

    let guess = guess_from_path(Path::new(&work.path), Path::new(&work.root_path));
    Ok((work, guess))
}
