pub mod anilist;
pub mod producao;
pub mod regiao;
pub mod saga;
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

#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
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
    /// Onde o arquivo mora. É a chave do `identification_scope`: a decisão
    /// humana é por pasta, não por obra.
    #[sqlx(default)]
    pub dir_path: Option<String>,
    #[sqlx(default)]
    pub library_id: Option<Uuid>,
}

/// Roda a identificação sobre a biblioteca. `force = true` refaz até o que já
/// estava casado (menos o que um humano confirmou — isso nunca é sobrescrito).
pub async fn run_matching(
    pool: PgPool,
    providers: Providers,
    artwork_dir: PathBuf,
    status: SharedMatchStatus,
    force: bool,
    job: Option<crate::jobs::Job>,
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
            w.id, m.path, l.root_path, l.provider_hint, l.default_kind,
            m.dir_path, m.library_id
        FROM work w
        JOIN media_file m ON m.work_id = w.id AND m.status = 'probed'
        JOIN library l ON l.id = m.library_id
        WHERE l.provider_hint <> 'none'
          AND w.match_state NOT IN ('confirmed', 'ignored')
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

    // Os escopos já decididos, carregados uma vez.
    //
    // É isto que impede o backlog de voltar: a pasta que alguém já resolveu não
    // vira pergunta de novo quando aparecem arquivos novos nela. Sem esta
    // consulta, `identification_scope` seria só um registro histórico — a
    // decisão valeria uma vez e o próximo scan a ignoraria.
    let escopos: Vec<ScopeRule> = sqlx::query_as(
        "SELECT library_id, dir_path, recursive, provider, provider_id, provider_kind,
                season_number, numbering, absolute_offset
         FROM identification_scope",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    let total = pending.len();
    let mut cancelado = false;

    tracing::info!(
        total,
        escopos = escopos.len(),
        force,
        "identificação iniciada"
    );

    for work in pending {
        let guess = guess_from_path(
            Path::new(&work.path),
            Path::new(&work.root_path),
            work.default_kind == "episode",
        );

        {
            let mut s = status.lock().await;
            s.works_seen += 1;
            s.current = Some(guess.title.clone());
        }

        // Pasta com decisão humana não passa pelo matcher: a resposta já existe.
        // Além de acertar mais, poupa a busca no provider — e o `PROVIDER_DELAY`
        // que vem com ela.
        if let Some(regra) = escopo_de(&escopos, &work) {
            match aplicar_escopo(&pool, &providers, &artwork_dir, &work, &guess, regra).await {
                Ok(estado) => {
                    let mut s = status.lock().await;
                    match estado {
                        "auto" | "confirmed" => s.matched_auto += 1,
                        "needs_review" => s.needs_review += 1,
                        _ => s.still_unmatched += 1,
                    }
                }
                Err(e) => {
                    tracing::warn!(title = %guess.title, error = %e, "escopo falhou");
                    let mut s = status.lock().await;
                    s.still_unmatched += 1;
                    if s.errors.len() < 100 {
                        s.errors.push(format!("{} (escopo): {e}", guess.title));
                    }
                }
            }
            continue;
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

        // Ponto de parada seguro: a obra corrente terminou de gravar. Parar
        // aqui não deixa nada pela metade.
        if let Some(j) = &job {
            let vistas = status.lock().await.works_seen;
            if vistas % 25 == 0 {
                let atual = status.lock().await.clone();
                j.tick(&atual, vistas as i64, Some(total as i64)).await;
            }
            if j.cancelled().await {
                cancelado = true;
                tracing::info!(vistas, "identificação cancelada a pedido");
                break;
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
        cancelado,
        "identificação concluída"
    );
    if let Some(j) = job {
        let estado = if cancelado { "cancelled" } else { "succeeded" };
        j.finish(&*s, estado, None).await;
    }
    true
}

/// Para cada obra candidata, quantos VIZINHOS da mesma pasta já a escolheram.
///
/// Conta o melhor candidato de cada vizinho, não todos: um arquivo "aponta"
/// para uma obra só. Contar todos os candidatos faria oito palpites fracos
/// valerem tanto quanto oito escolhas fortes.
///
/// Falha em silêncio devolvendo vazio — a corroboração é um bônus, e perder o
/// bônus é melhor do que abortar a identificação por causa dele.
async fn irmaos_por_obra(
    pool: &PgPool,
    work: &WorkToMatch,
) -> std::collections::HashMap<(String, String), usize> {
    let Some(dir) = work.dir_path.as_deref() else {
        return Default::default();
    };

    let linhas: Vec<(String, String, i64)> = sqlx::query_as(
        r#"
        WITH melhores AS (
            SELECT DISTINCT ON (c.work_id) c.work_id, c.provider, c.provider_id
            FROM match_candidate c
            JOIN media_file m ON m.work_id = c.work_id
            WHERE m.dir_path = $1 AND c.work_id <> $2
            ORDER BY c.work_id, c.score DESC
        )
        SELECT provider, provider_id, count(*) FROM melhores GROUP BY 1, 2
        "#,
    )
    .bind(dir)
    .bind(work.id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    linhas
        .into_iter()
        .map(|(p, id, n)| ((p, id), n as usize))
        .collect()
}

/// Uma decisão humana sobre uma pasta, como o matcher a enxerga.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ScopeRule {
    pub library_id: Uuid,
    pub dir_path: String,
    pub recursive: bool,
    pub provider: String,
    pub provider_id: String,
    pub provider_kind: String,
    pub season_number: Option<i32>,
    pub numbering: String,
    pub absolute_offset: i32,
}

/// O escopo que vale para esta obra, se houver.
///
/// Quando mais de um casa (pasta e pasta-mãe recursiva), vence o mais
/// ESPECÍFICO — a decisão sobre `Serie/Temporada 2` é mais informada que a
/// decisão sobre `Serie`.
fn escopo_de<'a>(escopos: &'a [ScopeRule], work: &WorkToMatch) -> Option<&'a ScopeRule> {
    let (dir, lib) = (work.dir_path.as_deref()?, work.library_id?);
    escopos
        .iter()
        .filter(|e| e.library_id == lib)
        .filter(|e| e.dir_path == dir || (e.recursive && dir.starts_with(&format!("{}/", e.dir_path))))
        .max_by_key(|e| e.dir_path.len())
}

/// Aplica a decisão da pasta a uma obra.
///
/// Mesma regra do `scopes::identify`: só vira `confirmed` se o episódio DESTE
/// arquivo resolver no provider. Sem isso, receberia a série certa e um
/// episódio inventado — que é afirmar o que não se sabe (§8b).
async fn aplicar_escopo(
    pool: &PgPool,
    providers: &Providers,
    artwork_dir: &Path,
    work: &WorkToMatch,
    guess: &Guess,
    regra: &ScopeRule,
) -> anyhow::Result<&'static str> {
    // O AniList não tem endpoint de episódio (ver o comentário no matcher), então
    // o escopo com numeração ainda não sabe resolvê-lo. Falhar dizendo isso é
    // melhor do que aplicar meio certo e o usuário descobrir depois.
    if regra.provider != "tmdb" {
        anyhow::bail!(
            "escopo de pasta com provider `{}` ainda não resolve episódio",
            regra.provider
        );
    }
    let Some(tmdb) = &providers.tmdb else {
        anyhow::bail!("escopo aponta pro TMDB mas não há chave configurada");
    };

    let (season, episode) = match regra.numbering.as_str() {
        "none" => (None, None),
        "absolute" => {
            let bruto = guess.any_episode().map(|e| e + regra.absolute_offset);
            let temporadas = tmdb.series_seasons(&regra.provider_id).await.unwrap_or_default();
            match bruto.and_then(|a| absolute_to_seasonal(&temporadas, a)) {
                Some((s, e)) => (Some(s), Some(e)),
                None => (None, None),
            }
        }
        _ => (regra.season_number.or(guess.season), guess.any_episode()),
    };

    let motivo = format!("escopo definido por você em {}", regra.dir_path);

    let detalhe = match (season, episode) {
        (Some(s), Some(e)) => tmdb
            .season(&regra.provider_id, s)
            .await
            .ok()
            .and_then(|t| find_episode(&t.episodes, e).cloned()),
        _ => None,
    };

    if detalhe.is_none() && regra.provider_kind != "movie" {
        sqlx::query(
            "UPDATE work SET match_state = 'needs_review', match_reasons = $2, updated_at = now()
             WHERE id = $1 AND match_state NOT IN ('confirmed', 'ignored')",
        )
        .bind(work.id)
        .bind(serde_json::json!([
            motivo,
            "mas o número do episódio deste arquivo não resolveu no provider"
        ]))
        .execute(pool)
        .await?;
        return Ok("needs_review");
    }

    let candidate = tmdb.tv_by_id(&regra.provider_id).await?;
    let pontuacao = score::Score {
        value: 1.0,
        reasons: vec![motivo.clone()],
    };
    let candidate_id = persist_candidate(pool, work.id, &candidate, &pontuacao).await?;

    let mut ajustado = guess.clone();
    ajustado.season = season;
    ajustado.episode = episode;
    ajustado.absolute_episode = None;

    apply_candidate(
        pool,
        providers,
        artwork_dir,
        work,
        &ajustado,
        &candidate,
        candidate_id,
        1.0,
        "confirmed",
    )
    .await?;

    sqlx::query("UPDATE work SET match_reasons = $2 WHERE id = $1")
        .bind(work.id)
        .bind(serde_json::json!([motivo]))
        .execute(pool)
        .await?;

    Ok("confirmed")
}

async fn match_one(
    pool: &PgPool,
    providers: &Providers,
    artwork_dir: &Path,
    work: &WorkToMatch,
    guess: &Guess,
) -> anyhow::Result<&'static str> {
    let candidates = search(providers, guess, &work.provider_hint).await;

    // Quantos vizinhos já escolheram cada obra. Uma query só, não uma por
    // candidato.
    //
    // LIMITAÇÃO CONHECIDA, e ela é real: numa primeira execução os vizinhos
    // ainda não têm candidato, então a corroboração só aparece para os arquivos
    // processados DEPOIS deles — e só rende de fato numa segunda passada. O
    // score fica dependente da ordem de processamento.
    //
    // Isso é aceitável só porque o motivo é gravado junto ("outros N arquivos
    // desta pasta…"): a auditabilidade do §8b não pede que o número seja
    // idêntico entre execuções, pede que ele seja EXPLICÁVEL. Resolver de vez
    // exigiria uma segunda passada sobre a biblioteca inteira.
    let vizinhos = irmaos_por_obra(pool, work).await;

    let mut scored: Vec<(Candidate, score::Score)> = candidates
        .into_iter()
        .map(|c| {
            let evidencia = score::Evidence {
                siblings: vizinhos
                    .get(&(c.provider.clone(), c.provider_id.clone()))
                    .copied()
                    .unwrap_or(0),
            };
            let s = score::score_with_evidence(guess, &c, &evidencia);
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

/// Acha o episódio dentro de uma temporada já carregada.
///
/// Duas tentativas, nesta ordem, e a segunda é a que importa:
/// 1. pelo `episode_number` exato — o caso normal, e respeita temporada com
///    buraco na numeração ou que começa no zero;
/// 2. pela POSIÇÃO na lista — cobre a série cujo provider numera de forma
///    ABSOLUTA dentro da temporada.
///
/// O caso (2) não é exótico: em Naruto Shippuden a temporada 8 vem com
/// `episode_number` de 152 a 175, e pedir "o episódio 2" dela devolve 404. Sem
/// esta reserva, o título vira "Episódio 2" e a obra é gravada como se aquilo
/// fosse um fato conhecido.
pub fn find_episode<'a>(
    episodes: &'a [tmdb::SeasonEpisode],
    numero: i32,
) -> Option<&'a tmdb::SeasonEpisode> {
    episodes
        .iter()
        .find(|e| e.episode_number == numero)
        .or_else(|| episodes.get((numero - 1).max(0) as usize))
}

/// Numeração absoluta → (temporada, episódio), pelo índice cumulativo.
///
/// Fansub numera de 1 a N ignorando temporada; o provider numera dentro da
/// temporada. Somando `episode_count` até passar do número absoluto, chega-se
/// no par que o provider entende: com temporadas de 24+24+25, o absoluto 60
/// é S03E12.
///
/// **A temporada 0 fica de fora da soma.** Ela guarda especiais, que não entram
/// na contagem corrida de nenhum fansub.
///
/// Esta tradução é palpite fundamentado, não fato: as fronteiras de temporada do
/// provider divergem da numeração de fansub com frequência, sobretudo em anime
/// longo. Por isso ela sempre aparece no `dry_run` antes de virar escrita, e o
/// escopo tem `absolute_offset` pra corrigir sem precisar editar arquivo a
/// arquivo.
pub fn absolute_to_seasonal(
    seasons: &[tmdb::SeasonSummary],
    absolute: i32,
) -> Option<(i32, i32)> {
    if absolute < 1 {
        return None;
    }

    let mut ordenadas: Vec<&tmdb::SeasonSummary> =
        seasons.iter().filter(|s| s.season_number > 0).collect();
    ordenadas.sort_by_key(|s| s.season_number);

    let mut restante = absolute;
    for temporada in ordenadas {
        if temporada.episode_count <= 0 {
            continue;
        }
        if restante <= temporada.episode_count {
            return Some((temporada.season_number, restante));
        }
        restante -= temporada.episode_count;
    }
    // Passou do fim da série: melhor devolver nada do que inventar uma
    // temporada que não existe.
    None
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

    // Içado do bloco da coleção pra que a OBRA e a COLEÇÃO gravem o mesmo
    // número. Antes, `season_number` só existia dentro do `if is_episode` e as
    // colunas `work.season_number`/`episode_number` não eram escritas por
    // caminho nenhum depois da criação — ficavam com o parse do scanner para
    // sempre, divergindo da temporada em que a obra realmente foi filada.
    let season_number = is_episode.then(|| guess.season.unwrap_or(1));

    // --- a série vem antes da arte, porque a arte é dela ---------------------
    //
    // O pôster e o backdrop que o provider devolve para um episódio são os da
    // SÉRIE — arte por episódio é o `still`, que é outro campo. Baixá-los por
    // obra guardava a mesma imagem uma vez por episódio: 18.004 arquivos para
    // 1.429 imagens distintas, 2,19 GB onde caberiam 197 MB, uma delas salva
    // 553 vezes.
    //
    // Agora a coleção-série é dona do arquivo e o episódio aponta pra ele.
    let serie = if is_episode {
        Some(ensure_serie(pool, providers, artwork_dir, candidate).await?)
    } else {
        None
    };

    // --- artwork -----------------------------------------------------------
    let mut artwork_json = serde_json::Map::new();
    let mut dominant = candidate.accent_color.clone();

    if let Some(serie) = &serie {
        // Herdado da série: zero rede, zero disco.
        if let Some(poster) = &serie.poster {
            artwork_json.insert("poster".into(), poster.clone().into());
        }
        if let Some(backdrop) = &serie.backdrop {
            artwork_json.insert("backdrop".into(), backdrop.clone().into());
        }
        dominant = dominant.or_else(|| serie.dominant_color.clone());
    } else {
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
    }

    // --- título, sinopse, coleções -----------------------------------------
    let mut title = candidate.title.clone();
    let mut overview = candidate.overview.clone();

    if is_episode {
        // O título da OBRA é o do episódio; o nome da série mora na coleção.
        // É exatamente pra isso que o modelo em grafo existe.
        //
        // A série já foi criada e enriquecida lá em cima — ela precisa existir
        // antes do bloco de artwork, porque é dela que a arte vem.
        let serie = serie.as_ref().expect("episódio sempre resolve a série");
        let series_key = serie.provider_key.clone();
        let series_id = serie.id;

        let season_number = season_number.unwrap_or(1);
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
        //
        // Busca a TEMPORADA e acha o episódio dentro dela, em vez de pedir
        // `/season/{s}/episode/{e}` direto. Motivo medido: o TMDB numera
        // episódio de forma inconsistente entre séries. Em Naruto Shippuden a
        // temporada 8 vem com `episode_number` de 152 a 175 — numeração
        // absoluta — e pedir o episódio "2" dela devolve 404. O resultado era
        // o título virar "Episódio 2" com estado `confirmed`, ou seja, afirmar
        // certeza sobre um dado que não foi obtido.
        let episode_detail = match (&providers.tmdb, candidate.provider.as_str()) {
            (Some(tmdb), "tmdb") => {
                let numero = episode_number.unwrap_or(1);
                tmdb.season(&candidate.provider_id, season_number)
                    .await
                    .ok()
                    .and_then(|temporada| find_episode(&temporada.episodes, numero).cloned())
            }
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
            -- COALESCE pra não apagar o que o scanner acertou quando o
            -- candidato é um filme (season/episode chegam NULL aqui).
            season_number    = COALESCE($13, season_number),
            episode_number   = COALESCE($14, episode_number),
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
    .bind(season_number)
    .bind(episode_number)
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

    // A ficha de produção (R22, §38): país e idioma viram tags, e o filme que
    // acabou de casar já nasce com elas — o aquecimento existe pros 548 que
    // casaram antes, não pra ser o único caminho.
    //
    // Uma requisição a mais **por filme aceito**, e não por candidato avaliado:
    // `production_countries` não vem no resultado da busca, e buscá-la antes do
    // match seria pagar por candidato descartado. É a mesma forma que os
    // créditos logo acima já usam.
    if candidate.provider == "tmdb" && candidate.provider_kind == "movie" {
        if let Some(client) = &providers.tmdb {
            if let Err(e) =
                producao::aplicar(pool, client, work.id, &candidate.provider_id).await
            {
                // Sem ficha não é motivo pra desfazer um match bom.
                tracing::warn!(error = %e, "ficha de produção não veio do provider");
            }
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

/// O que a coleção-série sabe de si mesma, depois de garantida e enriquecida.
#[derive(Debug, Clone)]
pub struct SerieIdentificada {
    pub id: Uuid,
    pub provider_key: String,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub dominant_color: Option<String>,
}

/// Garante a coleção-série **e a enriquece**: sinopse, ids do provider e arte.
///
/// A coleção-série nascia como um agrupamento vazio e ficava assim para sempre
/// — o identificador enriquece obras, e a série nunca é uma obra. No acervo
/// isso deixou `overview` nulo nas 115 séries e `external_ids` vazio nas 115.
///
/// A arte é o outro lado da mesma moeda: o pôster que o provider devolve para
/// um episódio é o da série, e baixá-lo por obra guardava a mesma imagem uma
/// vez por episódio. Aqui ela é baixada **uma vez**, com o nome da coleção, e
/// o episódio herda o caminho.
///
/// A guarda de rede é precisa de propósito: só volta a buscar o que ainda
/// falta *e* que o candidato tem como preencher. Sem ela seriam dois downloads
/// por episódio identificado, para reescrever sempre a mesma coisa.
async fn ensure_serie(
    pool: &PgPool,
    providers: &Providers,
    artwork_dir: &Path,
    candidate: &Candidate,
) -> anyhow::Result<SerieIdentificada> {
    let provider_key = format!("{}:{}", candidate.provider, candidate.provider_id);
    let id = ensure_collection(
        pool,
        &provider_key,
        "series",
        &candidate.title,
        None,
        None,
        candidate.year,
    )
    .await?;

    let (overview, poster, backdrop, cor): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT overview, artwork->>'poster', artwork->>'backdrop', dominant_color
         FROM collection WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;

    let falta_texto = overview.is_none() && candidate.overview.is_some();
    let falta_arte = (poster.is_none() && candidate.poster_url.is_some())
        || (backdrop.is_none() && candidate.backdrop_url.is_some());

    if !falta_texto && !falta_arte {
        return Ok(SerieIdentificada {
            id,
            provider_key,
            poster,
            backdrop,
            dominant_color: cor,
        });
    }

    let mut arte = SerieIdentificada {
        id,
        provider_key,
        poster,
        backdrop,
        dominant_color: cor.or_else(|| candidate.accent_color.clone()),
    };

    if arte.poster.is_none() {
        if let Some(url) = &candidate.poster_url {
            match artwork::fetch(&providers.http, artwork_dir, id, "poster", url).await {
                Ok(stored) => {
                    arte.poster = Some(stored.path);
                    arte.dominant_color = arte.dominant_color.or(stored.dominant_color);
                }
                Err(e) => tracing::warn!(error = %e, "pôster da série não baixou"),
            }
        }
    }
    if arte.backdrop.is_none() {
        if let Some(url) = &candidate.backdrop_url {
            match artwork::fetch(&providers.http, artwork_dir, id, "backdrop", url).await {
                Ok(stored) => arte.backdrop = Some(stored.path),
                Err(e) => tracing::warn!(error = %e, "backdrop da série não baixou"),
            }
        }
    }

    grava_identidade_da_serie(
        pool,
        id,
        candidate.overview.as_deref(),
        &candidate.provider,
        &candidate.provider_id,
        arte.poster.as_deref(),
        arte.backdrop.as_deref(),
        arte.dominant_color.as_deref(),
    )
    .await?;

    Ok(arte)
}

/// O UPDATE em si, isolado porque o reparo das séries já existentes precisa do
/// mesmo comportamento sem passar pelo caminho de identificação de uma obra.
///
/// Nenhuma coluna é apagada: o provider preenche o que está faltando e não
/// derruba o que já está lá.
#[allow(clippy::too_many_arguments)]
pub async fn grava_identidade_da_serie(
    pool: &PgPool,
    collection_id: Uuid,
    overview: Option<&str>,
    provider: &str,
    provider_id: &str,
    poster: Option<&str>,
    backdrop: Option<&str>,
    dominant_color: Option<&str>,
) -> anyhow::Result<()> {
    let mut arte = serde_json::Map::new();
    if let Some(p) = poster {
        arte.insert("poster".into(), p.into());
    }
    if let Some(b) = backdrop {
        arte.insert("backdrop".into(), b.into());
    }

    let mut ids = serde_json::Map::new();
    ids.insert(
        provider.to_string(),
        serde_json::Value::String(provider_id.to_string()),
    );

    sqlx::query(
        "UPDATE collection SET
            overview       = COALESCE(overview, $2),
            external_ids   = collection.external_ids || $3,
            artwork        = collection.artwork || $4,
            dominant_color = COALESCE(dominant_color, $5)
         WHERE id = $1",
    )
    .bind(collection_id)
    .bind(overview)
    .bind(serde_json::Value::Object(ids))
    .bind(serde_json::Value::Object(arte))
    .bind(dominant_color)
    .execute(pool)
    .await?;
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
    // `origin = 'provider'`: quem cria isto é o matcher, não uma pessoa.
    //
    // A coluna nasce com default 'manual' e o INSERT não a preenchia, então
    // toda série e temporada criada automaticamente se apresentava como feita
    // à mão. Isso importa no `reset`: desfazer uma identificação tem que apagar
    // o que o provider trouxe e PRESERVAR playlist e ordem de exibição que
    // alguém montou — e sem a distinção não há como separar os dois.
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO collection (kind, title, parent_id, position, year, provider_key, origin)
         VALUES ($1, $2, $3, $4, $5, $6, 'provider')
         ON CONFLICT (provider_key) DO UPDATE SET title = EXCLUDED.title,
                                                  origin = 'provider'
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

pub(crate) async fn attach_tag(
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
            w.id, m.path, l.root_path, l.provider_hint, l.default_kind,
            m.dir_path, m.library_id
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

    let mut guess = guess_from_path(
        Path::new(&work.path),
        Path::new(&work.root_path),
        work.default_kind == "episode",
    );

    // A correção humana entra POR CIMA do parse do caminho.
    //
    // É aqui que ela passa a valer de verdade: `confirm` e `apply_candidate`
    // chamam esta função, então o que a pessoa ensinou chega até a busca do
    // episódio no provider — em vez de ser descartado no fim do handler de
    // busca manual, como acontecia.
    let override_json: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT parse_override FROM work WHERE id = $1")
            .bind(work_id)
            .fetch_optional(pool)
            .await?
            .flatten();

    if let Some(o) = override_json {
        apply_parse_override(&mut guess, &o);
    }

    Ok((work, guess))
}

/// Mescla o override campo a campo. O que não foi corrigido continua vindo do
/// caminho — corrigir a temporada não deveria apagar o título que já estava bom.
pub fn apply_parse_override(guess: &mut Guess, o: &serde_json::Value) {
    if let Some(t) = o.get("title").and_then(|v| v.as_str()) {
        if !t.trim().is_empty() {
            guess.title = t.trim().to_string();
        }
    }
    if let Some(v) = o.get("year") {
        guess.year = v.as_i64().map(|n| n as i32);
    }
    if let Some(v) = o.get("season") {
        guess.season = v.as_i64().map(|n| n as i32);
    }
    if let Some(v) = o.get("episode") {
        guess.episode = v.as_i64().map(|n| n as i32);
    }
    if let Some(v) = o.get("absolute_episode") {
        guess.absolute_episode = v.as_i64().map(|n| n as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::tmdb::SeasonSummary;

    fn temporadas(pares: &[(i32, i32)]) -> Vec<SeasonSummary> {
        pares
            .iter()
            .map(|(season_number, episode_count)| SeasonSummary {
                season_number: *season_number,
                episode_count: *episode_count,
            })
            .collect()
    }

    fn regra(dir: &str, recursive: bool) -> ScopeRule {
        ScopeRule {
            library_id: Uuid::nil(),
            dir_path: dir.into(),
            recursive,
            provider: "tmdb".into(),
            provider_id: "1".into(),
            provider_kind: "tv".into(),
            season_number: None,
            numbering: "seasonal".into(),
            absolute_offset: 0,
        }
    }

    fn obra(dir: &str) -> WorkToMatch {
        WorkToMatch {
            id: Uuid::nil(),
            path: format!("{dir}/x.mkv"),
            root_path: "/media".into(),
            provider_hint: "auto".into(),
            default_kind: "episode".into(),
            dir_path: Some(dir.into()),
            library_id: Some(Uuid::nil()),
        }
    }

    #[test]
    fn escopo_casa_a_propria_pasta() {
        let e = vec![regra("/media/Serie", false)];
        assert!(escopo_de(&e, &obra("/media/Serie")).is_some());
        // Sem `recursive`, a subpasta NÃO herda: decidir sobre a raiz não é
        // decidir sobre cada temporada.
        assert!(escopo_de(&e, &obra("/media/Serie/Temporada 2")).is_none());
    }

    #[test]
    fn escopo_recursivo_alcanca_a_subarvore() {
        let e = vec![regra("/media/Serie", true)];
        assert!(escopo_de(&e, &obra("/media/Serie/Temporada 2")).is_some());
        // Mas não pode vazar pra pasta irmã de nome parecido.
        assert!(escopo_de(&e, &obra("/media/Serie 2")).is_none());
    }

    #[test]
    fn o_escopo_mais_especifico_ganha() {
        // A decisão sobre a temporada é mais informada que a sobre a série.
        let e = vec![
            regra("/media/Serie", true),
            regra("/media/Serie/Temporada 2", false),
        ];
        let achado = escopo_de(&e, &obra("/media/Serie/Temporada 2")).unwrap();
        assert_eq!(achado.dir_path, "/media/Serie/Temporada 2");
    }

    #[test]
    fn escopo_de_outra_biblioteca_nao_vale() {
        let mut r = regra("/media/Serie", true);
        r.library_id = Uuid::from_u128(7);
        assert!(escopo_de(&[r], &obra("/media/Serie")).is_none());
    }

    #[test]
    fn absoluto_cai_na_primeira_temporada() {
        let t = temporadas(&[(1, 24), (2, 24), (3, 25)]);
        assert_eq!(absolute_to_seasonal(&t, 1), Some((1, 1)));
        assert_eq!(absolute_to_seasonal(&t, 24), Some((1, 24)));
    }

    #[test]
    fn absoluto_atravessa_a_fronteira_de_temporada() {
        let t = temporadas(&[(1, 24), (2, 24), (3, 25)]);
        assert_eq!(absolute_to_seasonal(&t, 25), Some((2, 1)));
        assert_eq!(absolute_to_seasonal(&t, 60), Some((3, 12)));
    }

    #[test]
    fn especiais_nao_entram_na_contagem() {
        // Temporada 0 é onde o TMDB guarda especiais. Fansub não os numera na
        // sequência corrida — incluí-la deslocaria a série inteira.
        let t = temporadas(&[(0, 10), (1, 24), (2, 24)]);
        assert_eq!(absolute_to_seasonal(&t, 1), Some((1, 1)));
        assert_eq!(absolute_to_seasonal(&t, 25), Some((2, 1)));
    }

    #[test]
    fn temporada_fora_de_ordem_nao_confunde() {
        let t = temporadas(&[(3, 25), (1, 24), (2, 24)]);
        assert_eq!(absolute_to_seasonal(&t, 25), Some((2, 1)));
    }

    #[test]
    fn passar_do_fim_devolve_nada_em_vez_de_inventar() {
        let t = temporadas(&[(1, 24), (2, 24)]);
        assert_eq!(absolute_to_seasonal(&t, 49), None);
        assert_eq!(absolute_to_seasonal(&t, 0), None);
    }

    #[test]
    fn temporada_sem_contagem_e_pulada() {
        // O TMDB às vezes devolve `episode_count: 0` numa temporada anunciada
        // mas ainda não catalogada.
        let t = temporadas(&[(1, 24), (2, 0), (3, 12)]);
        assert_eq!(absolute_to_seasonal(&t, 25), Some((3, 1)));
    }
}
