pub mod auth;
pub mod amigos;
pub mod avaliacao;
pub mod browse;
pub mod convite;
pub mod curation;
pub mod curiosidades;
pub mod feed;
pub mod graph;
pub mod junto;
pub mod guia;
pub mod live;
pub mod locadora;
pub mod menu;
pub mod metadata;
pub mod people;
pub mod perfil;
pub mod pareamento;
pub mod playback;
pub mod retrospectiva;
pub mod revista;
pub mod scopes;
pub mod social;
pub mod scrub;
pub mod stream;
pub mod works;

use axum::extract::{Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::auth::{AdminUser, AuthUser};
use crate::error::AppResult;
use crate::models::{Library, NewLibrary};
use crate::scanner;
use crate::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        // --- autenticação ---
        .route("/api/auth/status", get(auth::status))
        .route("/api/auth/setup", post(auth::setup))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        // R27: o token que vai no `?token=` das rotas de mídia. Curto, e sem
        // acesso a nada além de bytes — ver `auth/middleware.rs`.
        .route("/api/auth/media-token", post(auth::media_token))
        // R45: o token longo, só pra `/artwork/`. Ver `aceita_token_de_arte`.
        .route("/api/auth/artwork-token", post(auth::artwork_token))
        .route("/api/auth/password", post(auth::change_password))
        .route(
            "/api/auth/sessions",
            get(auth::sessions).delete(auth::revoke_all),
        )
        .route(
            "/api/auth/users",
            get(auth::list_users).post(auth::create_user),
        )
        .route(
            "/api/auth/users/{id}",
            delete(auth::delete_user).patch(auth::update_user),
        )
        .route("/api/auth/sessions/{id}", delete(auth::revoke_one))
        // --- R26: o convidado ---
        // `resgatar` é público como o login: quem troca o código por conta
        // ainda não tem sessão, e é o código que autentica.
        .route("/api/convites", get(convite::listar).post(convite::criar))
        .route("/api/convites/{para}", delete(convite::revogar))
        .route("/api/convites/resgatar", post(convite::resgatar))
        // R46: o celular pede o código, a TV o troca por sessão. `resgatar` é
        // pública pelo mesmo motivo da linha acima — do lado da TV ainda não há
        // sessão nenhuma, e o código é a credencial.
        .route("/api/pareamento", post(pareamento::criar))
        .route("/api/pareamento/resgatar", post(pareamento::resgatar))
        // R26: `root_path` é o caminho do SEU disco. A auditoria da §42
        // encontrou `/media/Movies` numa resposta 200 pra conta comum.
        .route("/api/libraries", get(list_libraries).post(create_library))
        .route("/api/libraries/{id}", delete(delete_library).patch(update_library))
        .route("/api/browse", get(browse::browse))
        .route("/api/scan", post(start_scan))
        .route("/api/scan/status", get(scan_status))
        // Histórico e controle das operações longas — ver o módulo `jobs`.
        .route("/api/jobs", get(list_jobs))
        .route("/api/jobs/{id}", get(get_job))
        .route("/api/jobs/{id}/cancel", post(cancel_job))
        .route("/api/works", get(works::list))
        // A biblioteca agrupada: série vira UMA entrada. Rota separada de
        // `/api/works` de propósito — dentro de uma coleção o que se quer é a
        // lista plana de episódios, e é `/api/works` que responde isso.
        .route("/api/library", get(works::library))
        .route("/api/works/{id}", get(works::detail).delete(works::delete_work))
        .route(
            "/api/works/{id}/progress",
            // R69: o `DELETE` é o desfazer do `POST` — mesma obra, mesma
            // pessoa, e some do mural.
            post(works::progress).delete(works::apagar_progresso),
        )
        .route("/api/continue", get(works::continue_watching))
        .route("/api/stream/{media_file_id}", get(stream::stream))
        // R80 — o mesmo arquivo, remuxado num mp4 só, pra levar no avião.
        .route("/api/stream/{media_file_id}/baixar", get(stream::baixar))
        // --- M1: identidade ---
        .route("/api/match", post(metadata::start))
        .route("/api/match/status", get(metadata::status))
        .route("/api/review", get(metadata::review))
        // Re-deriva o parse do que ainda não foi identificado. `dry_run` por
        // padrão — ver o comentário no handler.
        .route("/api/maintenance/reparse", post(metadata::reparse))
        .route(
            "/api/maintenance/repair-episode-titles",
            post(metadata::repair_episode_titles),
        )
        .route(
            "/api/maintenance/repair-series",
            post(metadata::repair_series),
        )
        .route(
            "/api/maintenance/artwork-orfao",
            post(metadata::limpar_artwork_orfao),
        )
        // A pasta como unidade de decisão — ver o módulo `scopes`.
        .route("/api/review/scopes", get(scopes::list))
        .route("/api/scopes/search", post(scopes::search))
        .route("/api/scopes/identify", post(scopes::identify))
        .route("/api/works/{id}/candidates", get(metadata::candidates))
        .route("/api/works/{id}/search", post(metadata::manual_search))
        .route("/api/works/{id}/match", post(metadata::confirm))
        // Lote. Ver os handlers: cada obra leva o SEU candidato, e mudança
        // de estado em massa só aceita os estados que não desfazem match bom.
        .route("/api/works/bulk/match", post(metadata::bulk_match))
        .route("/api/works/bulk/state", post(metadata::bulk_state))
        .route("/api/works/{id}/reset", post(metadata::reset))
        .route("/api/works/{id}/ignore", post(works::ignore_work))
        .route("/api/storage", get(works::storage))
        .route("/api/diagnostico", get(works::diagnostico))
        // R70: o número de sumidos é a única linha da saúde que é perda real —
        // e sem a lista ninguém pode agir sobre ela.
        .route("/api/diagnostico/sumidos", get(works::sumidos))
        // Correção humana do parse, persistida — ver o handler.
        .route(
            "/api/works/{id}/parse",
            post(metadata::set_parse).delete(metadata::clear_parse),
        )
        // --- M2: o grafo ---
        .route("/api/people", get(people::list))
        .route("/api/people/{id}", get(people::detail))
        // --- R18: o guia de cinema ---
        // Sem schema novo: é uma pergunta nova sobre `credit` (§8h) cruzada com
        // o `playback_state` do M0. Ver o cabeçalho do módulo.
        .route("/api/guia", get(guia::eixos))
        .route("/api/guia/pessoas", get(guia::pessoas))
        // Rota própria, e buscada depois que o cartaz já está na tela: são sete
        // consultas, e nenhuma delas vale atrasar a leitura da sinopse.
        .route(
            "/api/works/{id}/curiosidades",
            get(curiosidades::curiosidades),
        )
        // Busca a trivia de todos os filmes de uma vez. É `job` e não requisição
        // síncrona porque são 548 filmes com duas chamadas externas cada — a
        // dívida que o §21 registrou e que esta rota não repete.
        .route("/api/maintenance/aquecer-trivia", post(aquecer_trivia))
        // R22: a ficha de produção dos 548 filmes já casados. Também `job` —
        // e é a dívida que o `IDEIAS.md` §8 marcou como "pela terceira vez um
        // reparo de minutos vai correr dentro de um request".
        .route("/api/maintenance/aquecer-producao", post(aquecer_producao))
        // R32: as sagas. `belongs_to_collection` do TMDB, que o `IDEIAS.md` §7
        // registrava como dívida — pré-requisito das conquistas de trilogia.
        .route("/api/maintenance/aquecer-sagas", post(aquecer_sagas))
        // R75 — a estrutura que vem do disco: série e canal a partir da pasta,
        // sem provider nenhum. `?dry_run=false` pra valer.
        .route("/api/maintenance/estrutura", post(montar_estrutura))
        // R76 — a identidade da pasta: uma busca e uma ficha por pasta, não
        // por arquivo. Por ora só propõe; aplicar é o passo seguinte.
        .route("/api/maintenance/identificar-pastas", post(propor_pastas))
        // R63 — a ficha das temporadas (nome, sinopse e pôster próprio).
        .route("/api/maintenance/aquecer-temporadas", post(aquecer_temporadas))
        // --- R19: a locadora, e R28: o estoque é do servidor ---
        // A prateleira devolve só o que está FORA, não o estado das 746 caixas:
        // quem cruza com a estante é a tela, que já tem as caixas na mão.
        .route("/api/locadora/prateleira", get(locadora::prateleira))
        // R20: a loja da semana. Uma requisição no lugar das doze que a tela
        // fazia — porque reivindicar a estante e cortá-la são a mesma decisão.
        .route("/api/locadora/estantes", get(locadora::estantes))
        // R29: as opções da loja. Ler é de qualquer morador — saber por quantos
        // dias a fita é sua não é privilégio —, gravar é de administrador.
        .route(
            "/api/locadora/opcoes",
            get(locadora::ler_opcoes).put(locadora::salvar_opcoes),
        )
        .route("/api/locadora/alugar", post(locadora::alugar))
        .route("/api/locadora/devolver/{id}", post(locadora::devolver))
        .route("/api/locadora/pedir/{id}", post(locadora::pedir_de_volta))
        // Destrutivo: apaga o "continuar de onde parou". A confirmação é da
        // tela, mas o §22 exige que ela exista — ver o handler.
        .route("/api/locadora/rebobinar", post(locadora::rebobinar))
        // R30: onde está esta fita. Rota própria, chamada **no play** — a
        // estante não sabe, de propósito: você descobre quando põe pra tocar.
        .route("/api/locadora/fita/{id}", get(locadora::fita))
        // R50 — o que EU posso assistir agora, pra tela não oferecer o que a
        // validação vai negar (§53).
        .route("/api/locadora/liberadas", get(locadora::liberadas))
        // --- R21: o menu de DVD ---
        // O menu numa requisição; as cenas em outra, e só quando alguém entra
        // na tela de cenas — extrair doze quadros custa ~6s e ninguém deve
        // pagar isso por abrir o menu.
        .route("/api/works/{id}/menu", get(menu::menu))
        .route("/api/works/{id}/cenas", get(menu::cenas))
        // --- R23: a nota e a resenha ---
        // `PUT` porque avaliar de novo é trocar de ideia, não criar uma segunda
        // avaliação — a chave `(user_id, work_id)` já dizia isso.
        .route(
            "/api/works/{id}/avaliacao",
            get(avaliacao::listar)
                .put(avaliacao::salvar)
                .delete(avaliacao::apagar),
        )
        // --- R24: a retrospectiva ---
        //
        // O §40 fez dela e do placar duas rotas separadas "pra a decisão ser
        // reversível". Ela foi: o placar saiu na R32 apagando um arquivo e uma
        // linha, exatamente como previsto — e a retrospectiva ficou, porque
        // descrever quem você é continua sendo outra coisa que dar ponto.
        // --- R25: o mural, com o escopo que a R28 deu a ele ---
        // Nenhuma tabela nova: é um UNION sobre `play_event`, `emprestimo` e
        // `avaliacao`, agora escopado por **você e seus amigos**.
        .route("/api/feed", get(feed::feed))
        // --- R33: a rede social ---
        //
        // Presença, busca, post, comentário e conversa. O comentário é **uma
        // rota pros dois alvos**, espelhando a tabela: duas rotas quase iguais
        // seriam duas telas e duas chances de divergirem.
        .route("/api/presenca", get(social::presenca))
        .route("/api/pessoas", get(social::pessoas))
        .route("/api/posts", post(social::postar))
        .route("/api/posts/{id}", delete(social::apagar_post))
        .route("/api/comentarios", post(social::comentar))
        .route("/api/comentarios/{id}", delete(social::apagar_comentario))
        .route(
            "/api/avaliacao/{quem}/{obra}/comentarios",
            get(social::comentarios_da_review),
        )
        .route("/api/mensagens", get(social::conversas))
        .route(
            "/api/mensagens/{com}",
            get(social::conversa).post(social::mandar),
        )
        // --- R28: amigos ---
        // Três rotas pro que era um grupo inteiro. `POST` pede **ou** aceita, e
        // `DELETE` recusa, cancela ou desfaz: quem sabe em qual dos estados a
        // relação está é a linha do banco, não o cliente que carregou a tela
        // meio minuto atrás.
        .route("/api/amigos", get(amigos::listar))
        .route(
            "/api/amigos/{id}",
            post(amigos::pedir).delete(amigos::desfazer),
        )
        // --- R34: a revista da semana ---
        //
        // A capa do guia. O índice do §30 continua onde estava — ele virou a
        // parte de consulta, atrás da revista.
        .route("/api/guia/revista", get(revista::revista))
        .route("/api/retrospectiva", get(retrospectiva::retrospectiva))
        // --- R46: assistir junto ---
        //
        // O `junto` é uma sala, e as rotas dizem isso: criar, entrar, sair,
        // mandar o estado (só o host), avisar que travou, e conversar.
        .route("/api/junto", get(junto::atual).post(junto::criar))
        .route("/api/junto/abertas", get(junto::abertas))
        .route("/api/junto/{id}/entrar", post(junto::entrar))
        .route("/api/junto/{id}/sair", post(junto::sair))
        .route("/api/junto/{id}/estado", put(junto::estado))
        .route("/api/junto/{id}/pronto", put(junto::pronto))
        .route("/api/junto/{id}/recado", post(junto::recado))
        .route("/api/junto/{id}/membro/{quem}", delete(junto::expulsar))
        // --- R32: o perfil, e o placar que ele substitui ---
        //
        // O §40 pôs o placar numa aba própria "pra ser reversível", e o efeito
        // foi ele ficar escondido. A comparação com os amigos mora **dentro**
        // do perfil: é lá que alguém vai olhar.
        .route("/api/perfil", get(perfil::meu).put(perfil::salvar))
        .route("/api/perfil/{id}", get(perfil::de_alguem))
        // --- R35: os desafios ---
        //
        // Gerados na leitura, e idempotentes: o `UNIQUE` da 0034 faz a segunda
        // chamada na mesma janela não inserir nada. Sem job.
        .route("/api/desafios", get(perfil::desafios))
        .route("/api/desafios/cadencia", put(perfil::salvar_cadencia))
        .route("/api/works/{id}/credits", get(people::work_credits))
        .route("/api/tags", get(graph::list_tags))
        .route("/api/tag-namespaces", get(graph::list_namespaces))
        .route(
            "/api/works/{id}/tags",
            get(graph::work_tags).post(graph::attach_tag),
        )
        .route("/api/works/{id}/tags/{tag_id}", delete(graph::detach_tag))
        .route(
            "/api/collections",
            get(graph::list_collections).post(graph::create_collection),
        )
        .route("/api/collections/tree", get(graph::collection_tree))
        .route(
            "/api/collections/{id}",
            get(graph::collection_detail)
                .patch(graph::update_collection)
                .delete(graph::delete_collection),
        )
        .route("/api/collections/{id}/items", post(graph::add_item))
        .route(
            "/api/collections/{id}/items/{work_id}",
            delete(graph::remove_item),
        )
        .route("/api/collections/{id}/order", put(graph::reorder))
        .route(
            "/api/works/{id}/relations",
            get(graph::relations).post(graph::create_relation),
        )
        .route(
            "/api/works/{id}/relations/{other}/{kind}",
            delete(graph::delete_relation),
        )
        // --- M3: a alma ---
        .route("/api/scrub", post(scrub::start))
        .route("/api/scrub/status", get(scrub::status))
        .route("/api/media/{media_file_id}/scrub", get(scrub::info))
        .route("/api/events", get(crate::events::stream))
        // --- M5: curadoria ---
        .route("/api/curation/for-you", get(curation::for_you))
        .route("/api/curation/taste", get(curation::taste))
        .route("/api/curation/calibrar", get(curation::calibrar))
        .route("/api/curation/rebuild", post(curation::rebuild))
        .route("/api/curation/rebuild/status", get(curation::rebuild_status))
        .route("/api/works/{id}/similar", get(curation::similar))
        .route(
            "/api/works/{id}/feedback",
            post(curation::feedback).delete(curation::clear_feedback),
        )
        // --- M6: playback pesado ---
        .route("/api/playback/{media_file_id}/plan", get(playback::plan))
        .route("/api/playback/{media_file_id}/session", post(playback::start_session))
        .route("/api/hls/{session_id}/{filename}", get(playback::hls_file))
        .route("/api/hls/{session_id}", delete(playback::stop_session))
        .route("/api/transcode/capabilities", get(playback::capabilities))
        // --- R6: canais ao vivo ---
        .route("/api/live/channels", get(live::channels))
        .route("/api/live/guide", get(live::guide))
        .route("/api/live/odeon", get(live::odeon_guide))
        .route("/api/live/sources", get(live::sources).post(live::create_source))
        .route("/api/live/sources/{id}", delete(live::delete_source))
        .route("/api/live/import", post(live::import))
        .route("/api/live/{id}/watch", post(live::watch))
        .route("/api/live/reminders", get(live::reminders))
        .route(
            "/api/live/reminders/{programme_id}",
            post(live::create_reminder).delete(live::delete_reminder),
        )
        .route("/api/transcode/sessions", get(playback::sessions))
        .route("/api/media/{media_file_id}/subtitles", get(playback::list_subtitles))
        .route(
            "/api/media/{media_file_id}/subtitles/{index}",
            get(playback::subtitle_vtt),
        )
        .with_state(state)
}

/// O histórico das operações longas. É o que responde "quando foi a última
/// varredura?" e "o que estava rodando quando o servidor caiu?".
async fn list_jobs(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
) -> AppResult<Json<Value>> {
    Ok(Json(json!(crate::jobs::list(&state.pool, 50).await)))
}

/// Um job pelo id, para quem acabou de abrir um — **R81**.
///
/// `404` quando não existe, e não um corpo vazio com `200`: um job que sumiu e
/// um job que nunca existiu levam a telas diferentes, e devolver a mesma coisa
/// para os dois obrigaria o cliente a adivinhar (§8b).
async fn get_job(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> AppResult<Json<Value>> {
    crate::jobs::get(&state.pool, id)
        .await
        .map(|j| Json(json!(j)))
        .ok_or(crate::error::AppError::NotFound)
}

/// Pede o cancelamento. Não mata nada: marca, e o worker para no próximo ponto
/// seguro — interromper no meio de uma gravação deixaria estado pela metade.
async fn cancel_job(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> AppResult<Json<Value>> {
    let pedido = crate::jobs::request_cancel(&state.pool, id).await;
    Ok(Json(json!({
        "ok": pedido,
        "detalhe": if pedido {
            "vai parar no próximo item — o corrente termina de gravar"
        } else {
            "esse job não está rodando"
        }
    })))
}

/// Dispara o aquecimento do cache de trivia. Responde na hora com o id do job;
/// o acompanhamento é por `GET /api/jobs`, como as outras operações longas.
async fn aquecer_trivia(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> AppResult<Json<Value>> {
    let Some(job) = crate::jobs::Job::start(
        &state.pool,
        "trivia",
        json!({}),
        Some(user.id),
    ).await else {
        // `Job::start` devolve `None` tanto para "já existe um ativo" quanto
        // para "o INSERT falhou", e a diferença importa: a primeira versão
        // desta rota respondia "já há um aquecimento em andamento" quando na
        // verdade o CHECK de `job.kind` estava recusando o tipo novo (0020).
        // Um erro disfarçado de estado normal é o que o §8b chama de errar em
        // silêncio — então aqui a resposta pergunta ao banco antes de afirmar.
        let ativo = crate::jobs::latest(&state.pool, "trivia")
            .await
            .map(|j| j.state == "running")
            .unwrap_or(false);
        return Ok(Json(json!({
            "started": false,
            "reason": if ativo {
                "já há um aquecimento em andamento"
            } else {
                "o banco recusou abrir o job — confira o CHECK de job.kind"
            },
        })));
    };

    let id = job.id;
    let pool = state.pool.clone();
    let http = state.providers.http.clone();
    tokio::spawn(async move { crate::trivia::aquecer(pool, http, Some(job)).await });

    Ok(Json(json!({ "started": true, "job_id": id })))
}

/// Dispara o aquecimento da ficha de produção. Mesma forma do de trivia, e
/// pela mesma razão — inclusive a de perguntar ao banco antes de afirmar que
/// já há um em andamento (§34).
async fn aquecer_producao(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> AppResult<Json<Value>> {
    let Some(job) =
        crate::jobs::Job::start(&state.pool, "producao", json!({}), Some(user.id)).await
    else {
        let ativo = crate::jobs::latest(&state.pool, "producao")
            .await
            .map(|j| j.state == "running")
            .unwrap_or(false);
        return Ok(Json(json!({
            "started": false,
            "reason": if ativo {
                "já há um aquecimento em andamento"
            } else {
                "o banco recusou abrir o job — confira o CHECK de job.kind"
            },
        })));
    };

    let id = job.id;
    let pool = state.pool.clone();
    let providers = state.providers.clone();
    tokio::spawn(async move {
        crate::metadata::producao::aquecer(pool, providers, Some(job)).await
    });

    Ok(Json(json!({ "started": true, "job_id": id })))
}

/// Busca a saga de cada filme identificado, e conserta a arte das sagas que a
/// R32 gravou com caminho remoto (R38). Mesmo molde do aquecimento de produção
/// (§38): job, progresso visível, cancelamento e retomada pelo `WHERE`.
async fn aquecer_sagas(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> AppResult<Json<Value>> {
    let Some(job) = crate::jobs::Job::start(&state.pool, "saga", json!({}), Some(user.id)).await
    else {
        let ativo = crate::jobs::latest(&state.pool, "saga")
            .await
            .map(|j| j.state == "running")
            .unwrap_or(false);
        return Ok(Json(json!({
            "started": false,
            "reason": if ativo {
                "já há uma busca de sagas em andamento"
            } else {
                "o banco recusou abrir o job — confira o CHECK de job.kind"
            },
        })));
    };

    let id = job.id;
    let pool = state.pool.clone();
    let providers = state.providers.clone();
    let artwork_dir = state.config.artwork_dir.clone();
    tokio::spawn(
        async move { crate::metadata::saga::aquecer(pool, providers, artwork_dir, Some(job)).await },
    );

    Ok(Json(json!({ "started": true, "job_id": id })))
}

/// R63 — busca no TMDB a ficha de cada temporada: nome, sinopse e pôster.
///
/// Medido em 18/08/2026: **473 temporadas, zero com pôster e zero com
/// sinopse**. Uma chamada por série (120) e não por temporada (473), porque o
/// `/tv/{id}` devolve `seasons[]` inteiro — ver `metadata::temporada`.
async fn aquecer_temporadas(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> AppResult<Json<Value>> {
    let Some(job) = crate::jobs::Job::start(&state.pool, "temporada", json!({}), Some(user.id)).await
    else {
        let ativo = crate::jobs::latest(&state.pool, "temporada")
            .await
            .map(|j| j.state == "running")
            .unwrap_or(false);
        return Ok(Json(json!({
            "started": false,
            "reason": if ativo {
                "já há uma busca de temporadas em andamento"
            } else {
                "o banco recusou abrir o job — confira o CHECK de job.kind"
            },
        })));
    };

    let id = job.id;
    let pool = state.pool.clone();
    let providers = state.providers.clone();
    let artwork_dir = state.config.artwork_dir.clone();
    tokio::spawn(async move {
        crate::metadata::temporada::aquecer(pool, providers, artwork_dir, Some(job)).await
    });

    Ok(Json(json!({ "started": true, "job_id": id })))
}

/// R75 — monta série e canal a partir das pastas, sem tocar no provider.
///
/// Medido em 20/08/2026: 507 pastas do acervo são claramente série e só 133
/// séries existiam no banco. As outras viravam episódio solto na grade.
///
/// `dry_run` é o padrão, como em toda rota que escreve em lote aqui.
async fn montar_estrutura(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<crate::models::ReparseParams>,
) -> AppResult<Json<Value>> {
    // Sem job quando é ensaio: um ensaio não é trabalho, e abrir job pra ele
    // sujaria o histórico de manutenção com linhas que não mudaram nada.
    let job = if params.dry_run {
        None
    } else {
        crate::jobs::Job::start(&state.pool, "estrutura", json!({}), None).await
    };
    let s = crate::metadata::estrutura::montar(state.pool.clone(), params.dry_run, job).await;
    Ok(Json(serde_json::to_value(s).unwrap_or_else(|_| json!({}))))
}

#[derive(Debug, serde::Deserialize)]
struct PropostaParams {
    /// Quantas pastas visitar nesta chamada. Cada uma custa uma busca e uma
    /// ficha no provider, então o padrão é modesto e quem quiser mais pede.
    #[serde(default = "vinte")]
    limite: usize,
    /// Padrão `true`, como em toda rota que escreve em lote aqui — e mais
    /// ainda nesta: uma pasta aplicada mexe em até 485 obras de uma vez.
    #[serde(default = "sim")]
    dry_run: bool,
}

fn sim() -> bool {
    true
}

fn vinte() -> usize {
    20
}

/// R76 — propõe uma série pra cada pasta montada pela R75, e aplica as que
/// passam do limiar.
///
/// Quem aplica é o `scopes::aplicar` — o mesmo caminho da decisão manual, com
/// a regra do §8b dentro: arquivo cujo episódio não resolve entra na série e
/// fica em revisão, **nunca** `confirmed` com título "Episódio N".
async fn propor_pastas(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    Query(params): Query<PropostaParams>,
) -> AppResult<Json<Value>> {
    let s = crate::metadata::pasta::propor(
        &state,
        user.id,
        params.limite.clamp(1, 200),
        params.dry_run,
    )
    .await;
    Ok(Json(serde_json::to_value(s).unwrap_or_else(|_| json!({}))))
}

async fn health(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let one: i32 = sqlx::query_scalar("SELECT 1").fetch_one(&state.pool).await?;
    Ok(Json(json!({
        "status": "ok",
        "db": one == 1,
        "version": env!("CARGO_PKG_VERSION"),
    })))
}

/// As bibliotecas.
///
/// **O `root_path` some pra quem não é administrador** (R26, §42). A auditoria
/// encontrou `/media/Movies` e `/media2/Movies` numa resposta 200 pra conta
/// comum — o caminho do disco de outra pessoa é informação de dono.
///
/// A lista inteira não vira rota de admin porque ela alimenta o filtro por
/// biblioteca da tela: nome e id são legítimos pra quem navega, o caminho não.
/// Esconder a lista consertaria o vazamento quebrando a biblioteca, que é o
/// tipo de conserto que o §22 chama de regra inventada.
async fn list_libraries(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Vec<Value>>> {
    let libraries = sqlx::query_as::<_, Library>("SELECT * FROM library ORDER BY created_at")
        .fetch_all(&state.pool)
        .await?;

    let dono = crate::auth::acesso::e_morador(&user) && user.is_admin();
    Ok(Json(
        libraries
            .into_iter()
            .map(|l| {
                let mut v = json!({
                    "id": l.id,
                    "name": l.name,
                    "default_kind": l.default_kind,
                    "provider_hint": l.provider_hint,
                    "created_at": l.created_at,
                });
                if dono {
                    v["root_path"] = json!(l.root_path);
                }
                v
            })
            .collect(),
    ))
}

/// A regra da R75 que só morava dentro da migração 0048.
///
/// `provider_hint = 'none'` quer dizer "nada aqui vai ter ficha em provider
/// nenhum", e a R75 já respondeu o que isso faz do arquivo: é vídeo de canal.
/// O `Guess::kind` depende disso — a guarda dele (`if library_default ==
/// "video"`) é o que impede `001 - Payday 2` de nascer `episode` só pela
/// numeração, e ela nunca dispara se a biblioteca não disser `video`.
///
/// **Por que aqui e não só na migração.** A 0048 escreveu esta mesma regra como
/// um `UPDATE`, e num banco novo ela casou com zero linhas: migração roda no
/// boot, biblioteca se cria uma hora depois. Um `UPDATE` só alcança o que já
/// existe; a linha criada amanhã está fora do alcance de qualquer migração
/// escrita hoje. Só o caminho que **cria** a linha pode manter o invariante.
///
/// **Corrige em vez de recusar**, e isso é por causa da separação dos
/// repositórios: o cliente manda `default_kind` de um `<select>` que não
/// conhece esta regra, e um 400 aqui deixaria a aba Pastas sem conseguir criar
/// a biblioteca do YouTube até o outro lado ser atualizado — a dívida que o
/// README diz que a separação compra. A correção fica no log, que é onde ela
/// pode ser vista sem quebrar ninguém.
fn kind_do_canal(default_kind: &str, provider_hint: &str) -> String {
    if provider_hint == "none" {
        return "video".to_string();
    }
    default_kind.to_string()
}

async fn create_library(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(body): Json<NewLibrary>,
) -> AppResult<Json<Library>> {
    // Mesma checagem do navegador de pastas: sem isto a API aceitaria `/etc`
    // e o scanner sairia lendo o container inteiro.
    let path = std::path::Path::new(&body.root_path)
        .canonicalize()
        .map_err(|_| crate::error::AppError::BadRequest(
            format!("a pasta {} não existe no servidor", body.root_path)
        ))?;

    let inside = state.config.media_roots.iter().any(|root| {
        root.canonicalize().map(|r| path.starts_with(&r)).unwrap_or(false)
    });
    if !inside {
        return Err(crate::error::AppError::Forbidden(
            "essa pasta não está montada no servidor — veja MEDIA_PATH no .env".into(),
        ));
    }

    // Biblioteca aninhada dentro de outra é um estado que nunca funciona: um
    // arquivo pertence a UMA biblioteca (o `path` é UNIQUE), então a de dentro
    // nasce vazia e a pessoa fica achando que o scan quebrou.
    let existing: Vec<(uuid::Uuid, String, String)> =
        sqlx::query_as("SELECT id, name, root_path FROM library")
            .fetch_all(&state.pool)
            .await?;

    for (_, name, root) in &existing {
        let other = std::path::Path::new(root);
        if path.starts_with(other) {
            return Err(crate::error::AppError::BadRequest(format!(
                "essa pasta já está dentro da biblioteca \"{name}\" ({root}). \
                 Remova aquela primeiro, ou aponte esta pra outro lugar."
            )));
        }
        if other.starts_with(&path) {
            return Err(crate::error::AppError::BadRequest(format!(
                "essa pasta contém a biblioteca \"{name}\" ({root}). \
                 Remova aquela primeiro, ou escolha uma subpasta."
            )));
        }
    }

    let default_kind = kind_do_canal(&body.default_kind, &body.provider_hint);
    if default_kind != body.default_kind {
        tracing::info!(
            name = %body.name,
            pedido = %body.default_kind,
            "provider_hint = none: biblioteca de canal, default_kind corrigido pra video"
        );
    }

    let library = sqlx::query_as::<_, Library>(
        "INSERT INTO library (name, root_path, default_kind, provider_hint)
         VALUES ($1, $2, $3, $4) RETURNING *",
    )
    .bind(&body.name)
    .bind(path.to_string_lossy().to_string())
    .bind(&default_kind)
    .bind(&body.provider_hint)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            crate::error::AppError::BadRequest("já existe biblioteca nessa pasta".into())
        }
        _ => crate::error::AppError::Db(e),
    })?;
    Ok(Json(library))
}

/// Varre. Com `?then=match`, encadeia a identificação em seguida.
///
/// O encadeamento existe porque a sequência é sempre a mesma e a espera é
/// longa: varrer 17 mil arquivos leva uma hora, identificar leva mais, e sem
/// isto alguém precisa estar presente no meio pra apertar o segundo botão.
///
/// A identificação só começa se a varredura **terminou de verdade**. Depois de
/// um cancelamento ou de uma falha, encadear seria identificar sobre um
/// acervo pela metade.
///
/// **E as sagas vêm depois da identificação** (R40), pelo mesmo argumento: o
/// pedido era *"como dou refresh nas coleções para pegar filmes novos?"*, e
/// filme novo só vira alvo de saga depois de identificado — o alvo do job é
/// `match_state IN ('auto','confirmed')` com id do TMDB. Encadear logo após a
/// varredura acharia zero. Assim "achei filmes novos" e "as sagas deles
/// apareceram" são um gesto só.
async fn start_scan(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<ScanRequest>,
) -> AppResult<Json<Value>> {
    if state.scan.lock().await.running {
        return Ok(Json(json!({ "started": false, "reason": "scan já em andamento" })));
    }

    let pool = state.pool.clone();
    let status = state.scan.clone();
    let job = crate::jobs::Job::start(&state.pool, "scan", json!({}), None).await;
    let job_id = job.as_ref().map(|j| j.id);

    let encadear = params.then.as_deref() == Some("match");
    let apenas = kind_do_pedido(params.tipo.as_deref());
    let bus = state.events.clone();
    let depois = state.clone();

    tokio::spawn(async move {
        scanner::scan_kind(pool, status.clone(), job, apenas).await;
        let finished = status.lock().await.clone();
        crate::events::publish(
            &bus,
            crate::events::AppEvent::ScanFinished {
                added: finished.files_added,
                updated: finished.files_updated,
            },
        );

        if !encadear {
            return;
        }

        // Só encadeia se a varredura chegou ao fim. `finished_at` é carimbado
        // tanto no sucesso quanto no cancelamento, então quem responde por isso
        // é o ESTADO do job, não a existência da data.
        let concluiu = crate::jobs::latest(&depois.pool, "scan")
            .await
            .map(|j| j.state == "succeeded")
            .unwrap_or(false);
        if !concluiu {
            tracing::info!("varredura não concluiu — identificação não encadeada");
            return;
        }

        let job_match =
            crate::jobs::Job::start(&depois.pool, "match", json!({ "chained": true }), None).await;
        tracing::info!("varredura concluída — identificação encadeada");
        crate::metadata::run_matching_kind(
            depois.pool.clone(),
            depois.providers.clone(),
            depois.config.artwork_dir.clone(),
            depois.matching.clone(),
            // O encadeamento do `scan` busca **o que é novo** — quem quer
            // refazer o pendente chama o `/api/match?alvo=pendentes` (R74).
            "novas",
            job_match,
            apenas,
        )
        .await;
        let m = depois.matching.lock().await.clone();
        crate::events::publish(
            &bus,
            crate::events::AppEvent::MatchFinished {
                auto: m.matched_auto,
                needs_review: m.needs_review,
            },
        );

        // As sagas dos filmes recém-identificados (R40).
        //
        // Rodar de novo é barato e sempre seguro: o alvo é "filme sem
        // franquia", então uma segunda passada custa as chamadas dos avulsos e
        // mais nada — e desde a R38 a mesma rodada ainda conserta capa de saga
        // que tenha ficado com caminho remoto.
        //
        // `Job::start` devolvendo `None` aqui é o caso normal de "já há um
        // rodando", e não um erro: quem apertou o botão à mão ganha a rodada, e
        // o encadeamento não precisa de uma segunda.
        if let Some(job_saga) =
            crate::jobs::Job::start(&depois.pool, "saga", json!({ "chained": true }), None).await
        {
            tracing::info!("identificação concluída — sagas encadeadas");
            crate::metadata::saga::aquecer(
                depois.pool.clone(),
                depois.providers.clone(),
                depois.config.artwork_dir.clone(),
                Some(job_saga),
            )
            .await;
        } else {
            tracing::info!("sagas não encadeadas — já havia uma rodada em andamento");
        }
    });

    Ok(Json(json!({
        "started": true,
        "job_id": job_id,
        "then": if encadear { Some("match") } else { None },
    })))
}

#[derive(serde::Deserialize)]
struct ScanRequest {
    /// `match` encadeia a identificação depois da varredura.
    then: Option<String>,
    /// **O escopo da busca** — R73. `filme`, `serie`, ou ausente pra tudo.
    ///
    /// Os três gestos que o cliente pediu, no formato do Jellyfin:
    ///
    /// ```text
    /// POST /api/scan?tipo=filme&then=match     busca e identifica os filmes
    /// POST /api/scan?tipo=serie&then=match     busca e identifica as séries
    /// POST /api/scan?then=match                os dois, e o resto
    /// ```
    ///
    /// Um parâmetro e não três rotas porque as três compartilham a trava, o
    /// job e o encadeamento — três handlers seriam três lugares pra esquecer
    /// de segurar o mesmo cadeado.
    ///
    /// O corte é por `library.default_kind`, que é o que uma biblioteca
    /// **declara ser**. "Tudo" continua incluindo YouTube e Clipes na
    /// varredura; a identificação já os pula sozinha pelo `provider_hint`.
    tipo: Option<String>,
}

/// `filme` → `movie`, `serie` → `episode`. Qualquer outra coisa é tudo.
///
/// O vocabulário da API é o do produto, e o da coluna é o do schema —
/// traduzir aqui evita que o cliente precise saber que "série" se guarda como
/// `episode`.
fn kind_do_pedido(tipo: Option<&str>) -> Option<&'static str> {
    match tipo.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("filme") | Some("filmes") | Some("movie") | Some("movies") => Some("movie"),
        Some("serie") | Some("series") | Some("série") | Some("séries") | Some("episode") => {
            Some("episode")
        }
        _ => None,
    }
}

/// Apagar biblioteca leva junto arquivos e obras (cascade no schema). O
/// histórico de reprodução some com as obras — por isso a confirmação fica na
/// interface, e a API é explícita sobre o que removeu.
async fn delete_library(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> AppResult<Json<Value>> {
    let works: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT work_id) FROM media_file WHERE library_id = $1 AND work_id IS NOT NULL",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await?;

    let mut tx = state.pool.begin().await?;

    let result = sqlx::query("DELETE FROM library WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    // Desfaz na mão em vez de contar com o `Drop`. O `Drop` do sqlx também
    // desfaz, mas só quando a transação é destruída, e sem `await` — ele agenda
    // o rollback no pool. Fechar aqui devolve a conexão imediatamente e deixa a
    // intenção escrita: biblioteca inexistente não mexe em nada.
    if result.rows_affected() == 0 {
        tx.rollback().await?;
        return Err(crate::error::AppError::NotFound);
    }

    // O cascade leva os `media_file` junto, mas `work` não tem FK pra library —
    // sem isto as obras ficam órfãs, sem arquivo nenhum, e a biblioteca passa a
    // mostrar cartões que não tocam.
    let orphans = sqlx::query(
        "DELETE FROM work w WHERE NOT EXISTS (SELECT 1 FROM media_file m WHERE m.work_id = w.id)",
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(json!({
        "ok": true,
        "works_removed": orphans.rows_affected().max(works as u64),
    })))
}

#[derive(serde::Deserialize)]
struct UpdateLibrary {
    name: Option<String>,
    default_kind: Option<String>,
    provider_hint: Option<String>,
}

async fn update_library(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    Json(body): Json<UpdateLibrary>,
) -> AppResult<Json<Library>> {
    // O mesmo invariante do `create_library`, e ele precisa estar aqui também:
    // sem isto a combinação volta por PATCH, tanto trocando o `provider_hint`
    // pra `none` quanto mandando `default_kind` de novo numa biblioteca que já
    // é `none`. O `CASE` lê o `provider_hint` **de depois** do update — que é o
    // que o `COALESCE($4, provider_hint)` repetido está fazendo ali dentro.
    sqlx::query(
        "UPDATE library SET
            name = COALESCE($2, name),
            provider_hint = COALESCE($4, provider_hint),
            default_kind = CASE
                WHEN COALESCE($4, provider_hint) = 'none' THEN 'video'
                ELSE COALESCE($3, default_kind)
            END
         WHERE id = $1",
    )
    .bind(id)
    .bind(body.name)
    .bind(body.default_kind)
    .bind(body.provider_hint)
    .execute(&state.pool)
    .await?;

    sqlx::query_as::<_, Library>("SELECT * FROM library WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .map(Json)
        .ok_or(crate::error::AppError::NotFound)
}

/// Mesmo formato de sempre; a diferença é sobreviver ao restart.
///
/// Sem isto, o `systemctl stop docker` que matou uma varredura de 17 mil
/// arquivos deixava a interface dizendo "nunca rodou" — que foi exatamente o
/// que aconteceu na implantação deste servidor.
async fn scan_status(State(state): State<AppState>) -> Json<scanner::ScanStatus> {
    let current = state.scan.lock().await.clone();
    if current.started_at.is_some() {
        return Json(current);
    }

    if let Some(job) = crate::jobs::latest(&state.pool, "scan").await {
        if let Ok(mut anterior) = serde_json::from_value::<scanner::ScanStatus>(job.progress) {
            // O processo que executava aquilo não existe mais.
            anterior.running = false;
            return Json(anterior);
        }
    }
    Json(current)
}

#[cfg(test)]
mod tests {
    /// **R73 — o vocabulário da API é o do produto, não o do schema.**
    ///
    /// O cliente pede "filme" e "série"; a coluna guarda `movie` e `episode`.
    /// Se a tradução vazar pro cliente, ele passa a precisar saber que uma
    /// série se guarda como episódio — que é detalhe de dentro.
    #[test]
    fn o_tipo_da_busca_fala_a_lingua_do_produto() {
        assert_eq!(super::kind_do_pedido(Some("filme")), Some("movie"));
        assert_eq!(super::kind_do_pedido(Some("Filmes")), Some("movie"));
        assert_eq!(super::kind_do_pedido(Some("serie")), Some("episode"));
        assert_eq!(super::kind_do_pedido(Some("séries")), Some("episode"));
        // E o nome do schema também serve, pra quem já o conhece.
        assert_eq!(super::kind_do_pedido(Some("movie")), Some("movie"));
        assert_eq!(super::kind_do_pedido(Some("episode")), Some("episode"));
    }

    /// **Ausente e desconhecido são a mesma coisa: tudo.**
    ///
    /// Um `tipo=` com erro de digitação não pode varrer metade do acervo em
    /// silêncio — e não pode virar erro, porque "tudo" é o padrão que sempre
    /// existiu e é o que o cliente antigo manda.
    #[test]
    fn tipo_ausente_ou_torto_varre_tudo() {
        assert_eq!(super::kind_do_pedido(None), None);
        assert_eq!(super::kind_do_pedido(Some("")), None);
        assert_eq!(super::kind_do_pedido(Some("fimle")), None);
    }

    /// **As sagas vêm depois da identificação, e não depois da varredura.**
    ///
    /// O alvo do job de saga é filme com `match_state IN ('auto','confirmed')`
    /// e id do TMDB. Encadeá-lo logo após o `scan_kind` acharia zero filme novo,
    /// porque nenhum deles foi identificado ainda — e o defeito seria invisível:
    /// o job rodaria, terminaria bem e não faria nada.
    #[test]
    fn a_saga_e_encadeada_depois_do_match() {
        let fonte = include_str!("mod.rs");
        let corrente = fonte
            .split_once("async fn start_scan")
            .expect("start_scan sumiu")
            .1;
        let pos_match = corrente
            .find("run_matching")
            .expect("a identificação não é mais encadeada");
        let pos_saga = corrente
            .find("saga::aquecer")
            .expect("as sagas não são mais encadeadas");
        assert!(
            pos_match < pos_saga,
            "as sagas passaram a ser encadeadas antes da identificação"
        );
    }

    /// **R75 — biblioteca sem provider é biblioteca de canal.**
    ///
    /// O `<select>` do cliente oferece `other`, e foi com `other` que a
    /// biblioteca do YouTube nasceu: 2.511 arquivos sem `format:`, dos quais
    /// 2.180 caíram na aba de filmes (que filtra por negação, não por
    /// `format:filme`) e 331 na de séries. Nenhum dos dois é filme ou série.
    #[test]
    fn biblioteca_sem_provider_nasce_como_canal() {
        assert_eq!(super::kind_do_canal("other", "none"), "video");
        assert_eq!(super::kind_do_canal("movie", "none"), "video");
        assert_eq!(super::kind_do_canal("episode", "none"), "video");
        assert_eq!(super::kind_do_canal("video", "none"), "video");
    }

    /// **E a regra não vale pra mais ninguém.**
    ///
    /// `none` é a única declaração que responde *o que o arquivo é*. Uma
    /// biblioteca `auto` ou `anilist` continua sendo o que a pessoa escolheu —
    /// corrigir ali seria trocar a escolha dela por um palpite.
    #[test]
    fn biblioteca_com_provider_mantem_o_que_foi_escolhido() {
        assert_eq!(super::kind_do_canal("movie", "auto"), "movie");
        assert_eq!(super::kind_do_canal("episode", "auto"), "episode");
        assert_eq!(super::kind_do_canal("other", "auto"), "other");
        assert_eq!(super::kind_do_canal("episode", "anilist"), "episode");
    }

    /// **O PATCH tem que segurar o mesmo invariante que o POST.**
    ///
    /// Este é o buraco que a 0048 deixou aberto de outro jeito: adiantava pouco
    /// o `create_library` corrigir se `PATCH /api/libraries/{id}` pudesse pôr a
    /// combinação de volta — trocando o `provider_hint` pra `none`, ou mandando
    /// `default_kind` numa biblioteca que já é `none`. O `CASE` no SQL é o que
    /// fecha os dois casos, e ele tem de ler o `provider_hint` de **depois** do
    /// update, não o de antes.
    #[test]
    fn o_patch_nao_desfaz_a_regra_do_canal() {
        let fonte = include_str!("mod.rs");
        let corpo = fonte
            .split_once("async fn update_library")
            .expect("update_library sumiu")
            .1
            .split_once("WHERE id = $1")
            .expect("o UPDATE de update_library mudou de forma")
            .0;
        assert!(
            corpo.contains("WHEN COALESCE($4, provider_hint) = 'none' THEN 'video'"),
            "update_library voltou a aceitar default_kind livre com provider_hint = none"
        );
    }
}
