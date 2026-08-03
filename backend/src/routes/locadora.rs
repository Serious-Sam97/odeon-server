//! A locadora: o estoque, e a fita.
//!
//! A locadora da R8 (§20) mostra 600 caixas e não guarda nada. Aqui ela ganha
//! as duas coisas que faltavam pra ser um lugar: **alguém está com a fita**, e
//! **ela volta em algum estado**.
//!
//! ## Uma loja, e ela é do servidor (R28)
//!
//! A R19 escreveu este módulo com escopo de **círculo** — um grupo fechado que
//! ninguém tinha pedido —, e a escassez valia dentro dele. A R28 desfez isso: há
//! **uma** locadora, a do servidor, e quem entra no Odeon entra nela. Uma cópia
//! por caixa passa a valer para todo mundo, que é o que "~40 caixas na loja
//! inteira" sempre quis dizer.
//!
//! O que sobra de "grupo" no produto é a **amizade** (`routes/amigos.rs`), e ela
//! não escopa estoque nenhum: escopa o que é social — o feed e as notas.
//!
//! ## O que este módulo não inventa
//!
//! A regra que atravessa o projeto é **não mentir com cara de metadado** (§18:
//! sem sufixo reconhecível, o idioma fica `None` em vez de virar "inglês"). Ela
//! decide duas coisas aqui:
//!
//! * **A condição da fita não é sorteada.** `playback_state` já sabe onde cada
//!   pessoa parou em cada obra. Quem assistiu até o minuto 47 e devolveu deixou
//!   a fita no minuto 47 — é literalmente verdade. Uma condição aleatória seria
//!   enfeite; esta é informação vestida de objeto.
//! * **A escassez não é uma regra inventada.** Ela barra porque a caixa está
//!   com outra pessoa do servidor, de verdade. O Odeon não vira DRM de
//!   mentirinha: ele vira o balcão que informa quem levou.
//!
//! ## Onde o bloqueio vale, e onde não vale
//!
//! O empréstimo barra **dentro da locadora**. A biblioteca, a busca e o
//! `▸ assistir` continuam abertos.
//!
//! Isso é decisão, não omissão. Barrar o player transformaria um morador em
//! porteiro do servidor do outro — e trancaria o dono fora do próprio arquivo,
//! que é o cenário que o §22 chama de regra inventada. A locadora é um lugar
//! com regra; o disco continua sendo seu. A escassez é honesta porque é social,
//! e uma escassez social se resolve socialmente: pedindo de volta.
//!
//! (Para o convidado a regra se inverte, e é `auth/acesso.rs` que a impõe: ele
//! só assiste o que pegou emprestado.)
//!
//! ## O impasse, e a válvula
//!
//! Bloqueio de verdade tem uma falha de modo: se a pessoa esquece de devolver,
//! a outra fica trancada pra sempre. Com gente que você não vê todo dia isso não
//! se resolve gritando pelo corredor — e a solução já é parte do tema, o que é
//! um bom sinal. **Uma locadora tem prazo.** Vencido, a fita volta sozinha
//! (`devolver_vencidos`).
//!
//! A varredura roda **na leitura**, não num daemon. É o padrão que a emissora
//! (§25) já usou pra programar três canais sem tabela e sem job: quando a
//! resposta é calculável na hora, um processo de fundo é uma peça a mais pra
//! quebrar. Aqui a varredura é um `UPDATE` indexado por `vence_em`.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::events::{self, AppEvent};
use crate::AppState;

/// O corte entre fita e disco.
///
/// O DVD chegou ao Brasil em 1998–99, mas a locadora só virou de verdade depois
/// de 2000. 1996 deixa o acervo em 111 fitas e 635 discos — a proporção certa:
/// a prateleira de VHS é o cantinho dos clássicos, não a loja inteira.
///
/// **Mora aqui e é servido ao cliente** em vez de existir dos dois lados. A R18
/// já pagou por essa lição uma vez: o mínimo de obras por pessoa estava escrito
/// em dois lugares, e o botão dizia *"ver as 644"* abrindo uma lista de 1.424
/// (§30). Um número de regra que a tela também precisa saber é servido, não
/// copiado.
pub const ULTIMO_ANO_VHS: i32 = 1996;

/// Quanto tempo sem heartbeat até considerar que ninguém está assistindo.
///
/// O player manda progresso a cada 10 s, então 90 s são nove batidas perdidas —
/// folgado o bastante pra sobreviver a uma aba em segundo plano e curto o
/// bastante pra não segurar uma fita vencida por causa de uma sessão que morreu.
const ASSISTINDO_AGORA: &str = "90 seconds";

/// A fração da duração a partir da qual a obra conta como terminada.
///
/// Não é um número novo: é o §8f, o mesmo que a curadoria e o guia (§30) usam.
/// Escrever 0.9 aqui faria três telas do mesmo produto discordarem sobre a
/// palavra "terminada", que é exatamente o defeito que a R18 desenterrou.
const RAZAO_TERMINADA: f64 = 0.92;

/// As estantes, na ordem em que reivindicam os títulos.
///
/// **Moraram no `Locadora.tsx` até a R20**, e mudaram de lado por uma razão de
/// correção, não de arrumação: reivindicar e sortear são a mesma decisão, e
/// decisão só pode morar num lugar.
///
/// A ordem importa porque cada título fica numa estante só, como numa loja de
/// verdade. Os gêneros distintivos vêm primeiro: se DRAMA viesse antes, ele
/// engoliria metade do acervo e as outras ficariam vazias.
///
/// **Desde a R29 a estante não é mais uma cota, é um endereço.** O sorteio
/// acontece na loja inteira e a estante é só onde cada caixa foi cair — então
/// uma semana pode ter nove de terror e nenhum faroeste, e a seguinte o
/// contrário. É por isso que `ESTANTES` continua sendo uma ordem de
/// reivindicação e deixou de ser uma divisão de prateleira.
///
/// Cada estante junta vários rótulos crus porque o acervo tem **dois**
/// vocabulários: o provider de filme responde em pt-BR ("Ficção científica") e
/// o de série em inglês ("Sci-Fi & Fantasy"). Sem a união, uma estante teria só
/// filmes e a outra só séries.
pub const ESTANTES: &[(&str, &[&str])] = &[
    ("Terror", &["Terror"]),
    ("Faroeste", &["Faroeste"]),
    ("Guerra", &["Guerra"]),
    ("Documentário", &["Documentário", "História"]),
    ("Animação", &["Animação"]),
    ("Infantil", &["Família", "Kids"]),
    (
        "Ficção científica",
        &["Ficção científica", "Sci-Fi & Fantasy", "Sci-Fi", "Fantasia"],
    ),
    (
        "Ação e aventura",
        &["Ação", "Aventura", "Action", "Adventure", "Action & Adventure", "Sports"],
    ),
    ("Crime e suspense", &["Crime", "Mistério", "Thriller"]),
    ("Comédia", &["Comédia", "Comedy"]),
    ("Romance", &["Romance", "Música"]),
    ("Drama", &["Drama"]),
];

/// As duas janelas que decidem a vitrine, e a diferença entre elas é a R29.
///
/// A primeira **não particiona**: ela ordena as 600 caixas pelo hash da semana
/// pra o corte acontecer na loja inteira. A segunda particiona, e roda antes do
/// corte — é o total do acervo por estante, o que faz a placa dizer "3 de 113"
/// sem que ninguém conclua que a loja tem três filmes de terror.
///
/// Mora numa constante pra ter teste. Um `PARTITION BY` a mais na primeira traz
/// a R20 de volta — 16 por estante, 166 caixas — **sem quebrar nada**: a loja
/// continua funcionando, só que com doze vezes mais caixas do que foi pedido.
const SORTEIO: &str = r#"
                   row_number() OVER (ORDER BY md5($2 || c.id::text)) AS pos,
                   count(*) OVER (PARTITION BY a.estante)             AS total,
                   count(*) OVER ()                                   AS no_acervo
"#;

/// As obras que compõem uma caixa de coleção.
///
/// A hierarquia real é série → temporada → obra, e `/api/library` agrupa por
/// `COALESCE(series.id, season.id)` — então o id de uma caixa é ou uma série
/// (que tem temporadas por filhas) ou uma temporada órfã. Os dois casos cabem
/// em `c.id = $ OR c.parent_id = $`, sem `WITH RECURSIVE`: a profundidade é
/// conhecida e é 2.
const OBRAS_DA_CAIXA: &str = r#"
    SELECT ci.work_id
    FROM collection_item ci
    JOIN collection c ON c.id = ci.collection_id
    WHERE c.id = $ALVO OR c.parent_id = $ALVO
"#;

/// As opções da loja.
///
/// Moravam no `circulo` até a R28, e não voltaram pro código quando ele caiu: o
/// argumento da 0021 continua valendo — *"um número de regra de negócio
/// escondido em `const` é um número que ninguém encontra"*. Com uma loja só, o
/// dono deles é o servidor, e a tabela `locadora_opcoes` é onde a fase 2 vai
/// pendurar o resto (tamanho do estoque, chave da escassez).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Opcoes {
    /// Quantas caixas ficam expostas na loja inteira por semana.
    ///
    /// **Na loja inteira, e não por estante** — é a escala do `IDEIAS.md` §3.2,
    /// e a diferença entre 40 e os 166 da R20 é a diferença entre uma mesa e uma
    /// parede.
    pub estoque: i32,
    pub prazo_dias: i32,
    pub limite_por_pessoa: i32,
    /// Ligada, uma cópia por caixa. Desligada, **só o bloqueio** sai: a loja
    /// continua curta, e ninguém barra ninguém.
    pub escassez: bool,
}

/// O que se pode mexer. Os mesmos campos, sem nada derivado — a rota de
/// gravação não aceita o que ela mesma calcula.
#[derive(Debug, Deserialize)]
pub struct NovasOpcoes {
    pub estoque: i32,
    pub prazo_dias: i32,
    pub limite_por_pessoa: i32,
    pub escassez: bool,
}

/// Alguém que frequenta a loja.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Pessoa {
    pub id: Uuid,
    pub display_name: String,
    /// Quantas caixas esta pessoa está segurando agora. É o que torna a loja
    /// habitada antes de existir qualquer feed: dá pra ver que o outro tem fita.
    pub na_mao: i64,

    /// **Quantas fitas dela alguém teve que rebobinar** (R30).
    ///
    /// É a reputação, e é o *"as pessoas saberem quem devolveu zoado"* virando
    /// número. Não é métrica inventada: cada unidade é uma vez em que outra
    /// pessoa encontrou a fita no meio e gastou os segundos pra voltar.
    pub zoadas: i64,
    /// E quantas ela rebobinou dos outros. O outro lado, e ele precisa existir:
    /// um placar que só conta o defeito faz de todo mundo réu.
    pub rebobinou: i64,
    /// Fitas que ela deixou no meio **agora**, esperando alguém.
    ///
    /// Diferente das duas de cima, esta é estado e não histórico: some no
    /// instante em que alguém rebobina. É a única das três que a pessoa pode
    /// consertar sozinha, e é por isso que ela aparece.
    pub no_meio: i64,
}

/// Uma caixa que está fora da prateleira.
///
/// A prateleira **não** devolve as 746 caixas com o estado de cada uma — devolve
/// só as que estão fora, que hoje são zero e num servidor de duas pessoas serão
/// meia dúzia. Quem cruza isso com a estante é a tela, que já tem as caixas na
/// mão. Mandar 746 linhas pra marcar 6 seria pagar a lista inteira pra dizer
/// quase nada.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Emprestada {
    pub id: i64,
    /// O id da caixa — obra avulsa ou coleção. É o mesmo id que
    /// `/api/library` devolve, e é por ele que a tela casa com a estante.
    pub caixa_id: Uuid,
    pub serie: bool,
    pub titulo: String,
    pub quem: Uuid,
    pub quem_nome: String,
    /// Se sou eu que estou com ela. A tela precisa saber sem comparar ids.
    pub meu: bool,
    pub pego_em: chrono::DateTime<chrono::Utc>,
    pub vence_em: chrono::DateTime<chrono::Utc>,
    pub pedido_em: Option<chrono::DateTime<chrono::Utc>>,
    pub pedido_por_nome: Option<String>,

    /// Se este empréstimo disputa a única cópia.
    ///
    /// **É o que decide se a caixa some da prateleira** (R29). Com a escassez
    /// ligada ela some — está com alguém, e não existe outra. Desligada, o
    /// empréstimo não tranca ninguém, então a caixa continua na estante pra
    /// quem quiser: sumir com ela ali seria inventar uma escassez que a opção
    /// acabou de desligar.
    pub exclusivo: bool,

    /// A arte da caixa, pra ela poder ser desenhada fora da estante.
    ///
    /// **Entrou por causa da rotação (§36).** Antes, uma caixa emprestada só
    /// aparecia como cinta sobre a caixa da estante — e a partir do momento em
    /// que a estante mostra 16 de 113, a fita que rudney levou pode
    /// simplesmente não estar exposta esta semana. Ela ficaria invisível, e com
    /// ela o "pedir de volta". A escassez precisa ser vista pra ter saída.
    pub poster: Option<String>,
    pub dominant_color: Option<String>,
    pub ano: Option<i32>,
}

/// Uma devolução que já aconteceu. É o que a caixa conta sobre a última pessoa
/// que a teve — e o que a R24 vai ler pra montar retrospectiva.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Devolvida {
    pub caixa_id: Uuid,
    pub titulo: String,
    pub quem_nome: String,
    pub devolvido_em: chrono::DateTime<chrono::Utc>,
    /// `rebobinada` | `no-meio` | `terminada`.
    pub devolvido_como: String,
    /// `membro` | `prazo`.
    pub devolvido_por: String,
    pub atrasada: bool,
}

#[derive(Debug, Serialize)]
pub struct Prateleira {
    pub opcoes: Opcoes,
    /// Quem mais frequenta a loja, e quanto cada um tem na mão.
    ///
    /// São as pessoas do **servidor**, e não as de um grupo: com estoque único,
    /// quem te barra pode ser qualquer uma delas, e o balcão que escondesse
    /// metade delas estaria escondendo metade das explicações.
    pub pessoas: Vec<Pessoa>,
    pub emprestadas: Vec<Emprestada>,
    pub devolvidas: Vec<Devolvida>,
    /// Quantas eu ainda posso pegar. Servido pronto porque o limite é do
    /// servidor e a conta é dele — a tela não deve refazer regra.
    pub posso_pegar: i64,
    pub ultimo_ano_vhs: i32,
}

/// As opções da loja.
///
/// Uma linha só, garantida pelo singleton da 0028 — então `fetch_one` é
/// honesto: se ela não estiver lá, o schema está quebrado e falhar alto é
/// melhor que servir um padrão inventado que ninguém configurou.
async fn opcoes(pool: &sqlx::PgPool) -> AppResult<Opcoes> {
    Ok(sqlx::query_as::<_, Opcoes>(
        "SELECT estoque, prazo_dias, limite_por_pessoa, escassez FROM locadora_opcoes",
    )
    .fetch_one(pool)
    .await?)
}

/// As opções, pra tela de administração. `AuthUser` e não `AdminUser`: qualquer
/// morador pode **ver** por quantos dias a fita é dele.
pub async fn ler_opcoes(
    State(state): State<AppState>,
    AuthUser(_): AuthUser,
) -> AppResult<Json<Opcoes>> {
    Ok(Json(opcoes(&state.pool).await?))
}

/// Trocar as opções da loja. Só quem administra — é configuração de servidor,
/// não preferência de conta.
///
/// **A validação é do banco.** Os quatro `CHECK` das migrações 0028 e 0029 já
/// dizem os intervalos, e repeti-los aqui criaria dois lugares pra discordar
/// sobre o que é um prazo válido. O que este handler faz é traduzir a violação
/// pra 400 em vez de deixar sair 500 — errar com o código errado é a versão
/// barulhenta de errar em silêncio (§8b).
pub async fn salvar_opcoes(
    State(state): State<AppState>,
    crate::auth::AdminUser(_): crate::auth::AdminUser,
    Json(novas): Json<NovasOpcoes>,
) -> AppResult<Json<Opcoes>> {
    let feito = sqlx::query(
        "UPDATE locadora_opcoes
         SET estoque = $1, prazo_dias = $2, limite_por_pessoa = $3, escassez = $4",
    )
    .bind(novas.estoque)
    .bind(novas.prazo_dias)
    .bind(novas.limite_por_pessoa)
    .bind(novas.escassez)
    .execute(&state.pool)
    .await;

    if let Err(e) = feito {
        // 23514 = check_violation.
        if matches!(&e, sqlx::Error::Database(db) if db.code().as_deref() == Some("23514")) {
            return Err(AppError::BadRequest(
                "número fora do que a loja aceita: estoque 1–1000, prazo 1–90 dias, limite 1–50"
                    .into(),
            ));
        }
        return Err(e.into());
    }

    // Devolve o que ficou gravado, e não o que foi mandado: a tela desenha a
    // verdade do banco, e uma coluna que um dia ganhe outro CHECK não faz a
    // tela mentir até alguém recarregar.
    let agora = opcoes(&state.pool).await?;
    tracing::info!(
        estoque = agora.estoque,
        prazo = agora.prazo_dias,
        limite = agora.limite_por_pessoa,
        escassez = agora.escassez,
        "opções da locadora"
    );
    Ok(Json(agora))
}

/// Como a fita voltou, derivado do `playback_state` de quem estava com ela.
///
/// Devolve a expressão SQL, porque ela é usada em dois lugares — a devolução do
/// membro e a do prazo — e duas cópias divergindo significaria a mesma fita
/// contar duas histórias.
///
/// A obra relevante de uma caixa de série é a **mais recentemente mexida**: a
/// caixa voltou com o último episódio assistido em algum estado, que é como uma
/// caixa de box realmente volta. Exigir a série inteira terminada faria toda
/// caixa de série voltar "no meio" pra sempre.
fn condicao_sql(alias_emp: &str) -> String {
    let obras = OBRAS_DA_CAIXA.replace("$ALVO", &format!("{alias_emp}.collection_id"));
    format!(
        r#"
        SELECT
            -- **A fita manda** (R30). Ela é o objeto; o progresso de quem
            -- devolveu é a memória dessa pessoa. Só quando não há fita — e um
            -- DVD nunca tem — a pergunta volta a ser "até onde essa pessoa foi",
            -- que continua sendo a resposta certa pra um disco.
            COALESCE(ft.posicao_segundos, ps.position_seconds) AS position_seconds,
            COALESCE(ft.duracao_segundos, ps.duration_seconds) AS duration_seconds,
            -- `finished` é acumulativo (§31): responde "já terminei isto alguma
            -- vez", não "a fita está no fim". Ele só entra quando não há fita,
            -- pelo mesmo motivo — a fita não tem passado, tem posição.
            (ft.posicao_segundos IS NULL AND ps.finished) AS finished,
            -- **O relógio continua sendo o do heartbeat.** É ele que responde
            -- "tem alguém assistindo agora", e um DVD não tem fita pra
            -- responder — sem isto a devolução automática arrancaria o disco da
            -- mão de quem está no meio do filme.
            ps.updated_at
        FROM (SELECT 1) AS _
        LEFT JOIN LATERAL (
            SELECT p.position_seconds, p.duration_seconds, p.finished, p.updated_at
            FROM playback_state p
            WHERE p.user_id = {alias_emp}.user_id
              AND ({alias_emp}.work_id IS NOT NULL AND p.work_id = {alias_emp}.work_id
                   OR {alias_emp}.collection_id IS NOT NULL AND p.work_id IN ({obras}))
            ORDER BY p.updated_at DESC
            LIMIT 1
        ) ps ON true
        LEFT JOIN LATERAL (
            SELECT f.posicao_segundos, f.duracao_segundos
            FROM fita f
            WHERE ({alias_emp}.work_id IS NOT NULL AND f.work_id = {alias_emp}.work_id
                   OR {alias_emp}.collection_id IS NOT NULL AND f.work_id IN ({obras}))
            ORDER BY f.deixada_em DESC
            LIMIT 1
        ) ft ON true
        "#
    )
}

/// `rebobinada` | `no-meio` | `terminada`, a partir das colunas do LATERAL.
fn classificacao_sql(p: &str) -> String {
    format!(
        r#"CASE
             WHEN {p}.position_seconds IS NULL OR {p}.position_seconds <= 0 THEN 'rebobinada'
             WHEN {p}.finished
               OR ({p}.duration_seconds > 0
                   AND {p}.position_seconds / {p}.duration_seconds >= {RAZAO_TERMINADA})
               THEN 'terminada'
             ELSE 'no-meio'
           END"#
    )
}

/// Devolve sozinha toda fita vencida da loja — **menos a que alguém está
/// assistindo agora**.
///
/// A ressalva do meio da sessão não é gentileza: é a mesma escolha do
/// cancelamento cooperativo do §12, que espera o ponto seguro em vez de matar
/// no meio. Uma fita que vence às 21h04 com a pessoa no minuto 40 do filme
/// devolve quando a sessão acabar — e "a sessão acabou" tem sinal real, que é o
/// heartbeat de 10 s do player parar de chegar.
///
/// Roda a cada leitura da prateleira. É barato: o índice parcial
/// `emprestimo_vencendo_idx` cobre exatamente `devolvido_em IS NULL AND
/// vence_em <= now()`.
pub async fn devolver_vencidos(pool: &sqlx::PgPool, bus: &events::Bus) -> AppResult<u64> {
    let sql = format!(
        r#"
        WITH vencidos AS (
            SELECT e.id, e.user_id, e.work_id, e.collection_id
            FROM emprestimo e
            WHERE e.devolvido_em IS NULL
              AND e.vence_em <= now()
        ),
        com_estado AS (
            SELECT v.id, p.position_seconds, p.duration_seconds, p.finished, p.updated_at
            FROM vencidos v
            LEFT JOIN LATERAL ({condicao}) p ON true
        )
        UPDATE emprestimo e
        SET devolvido_em   = now(),
            devolvido_por  = 'prazo',
            devolvido_como = {classificacao}
        FROM com_estado ce
        WHERE e.id = ce.id
          -- A válvula da válvula: quem está no meio do filme não perde a fita.
          AND (ce.updated_at IS NULL OR ce.updated_at < now() - interval '{ASSISTINDO_AGORA}')
        "#,
        condicao = condicao_sql("v"),
        classificacao = classificacao_sql("ce"),
    );

    let devolvidas = sqlx::query(&sql).execute(pool).await?.rows_affected();

    if devolvidas > 0 {
        tracing::info!(devolvidas, "fitas devolvidas pelo prazo");
        // Um aviso só, e não um por fita: quem ouve recarrega a prateleira.
        events::publish(
            bus,
            AppEvent::Locadora {
                acao: "venceu".into(),
                caixa_id: None,
                titulo: None,
                quem_nome: None,
            },
        );
    }

    Ok(devolvidas)
}

/// A prateleira: quem está com o quê, e o que voltou.
pub async fn prateleira(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Prateleira>> {
    let opcoes = opcoes(&state.pool).await?;

    // Antes de contar quem tem o quê, deixar o prazo agir. Ler primeiro
    // mostraria como emprestada uma fita que o próprio pedido já devolveria.
    devolver_vencidos(&state.pool, &state.events).await?;

    // Só quem está ativo: uma conta desativada não pega fita, e listá-la no
    // balcão seria mostrar um freguês que não entra mais na loja.
    // As três contas da fita saem de subconsultas e não de `JOIN`s: juntar
    // `emprestimo`, `rebobinada` e `fita` na mesma linha multiplicaria os
    // `count()` uns pelos outros — o defeito clássico do agregado com dois
    // caminhos, e ele sairia como uma reputação inflada em vez de um erro.
    let pessoas = sqlx::query_as::<_, Pessoa>(
        "SELECT u.id, u.display_name,
                (SELECT count(*) FROM emprestimo e
                  WHERE e.user_id = u.id AND e.devolvido_em IS NULL)   AS na_mao,
                -- `r.por <> u.id`: rebobinar a própria bagunça não é reputação,
                -- é arrumar a casa. O log guarda o fato; a leitura é que filtra.
                (SELECT count(*) FROM rebobinada r
                  WHERE r.de = u.id AND r.por IS DISTINCT FROM u.id)   AS zoadas,
                (SELECT count(*) FROM rebobinada r
                  WHERE r.por = u.id AND r.de IS DISTINCT FROM u.id)   AS rebobinou,
                (SELECT count(*) FROM fita f
                  WHERE f.deixada_por = u.id AND f.posicao_segundos > 0) AS no_meio
         FROM app_user u
         WHERE u.is_active
         ORDER BY u.display_name",
    )
    .fetch_all(&state.pool)
    .await?;

    // `COALESCE` no título porque a caixa é obra **ou** coleção — o mesmo
    // par de colunas do CHECK da 0021, lido do outro lado.
    let emprestadas = sqlx::query_as::<_, Emprestada>(
        "SELECT e.id,
                COALESCE(e.work_id, e.collection_id)   AS caixa_id,
                e.collection_id IS NOT NULL            AS serie,
                COALESCE(w.title, c.title)             AS titulo,
                e.user_id                              AS quem,
                u.display_name                         AS quem_nome,
                e.user_id = $1                         AS meu,
                e.pego_em, e.vence_em, e.pedido_em,
                p.display_name                         AS pedido_por_nome,
                e.exclusivo,
                COALESCE(w.artwork->>'poster', c.artwork->>'poster', arte.poster) AS poster,
                COALESCE(w.dominant_color, arte.cor)   AS dominant_color,
                COALESCE(w.year, c.year)               AS ano
         FROM emprestimo e
         JOIN app_user u ON u.id = e.user_id
         LEFT JOIN app_user p ON p.id = e.pedido_por
         LEFT JOIN work w       ON w.id = e.work_id
         LEFT JOIN collection c ON c.id = e.collection_id
         -- A coleção-série costuma vir sem arte própria: o scanner a cria e o
         -- identificador nunca a enriquece. Sem esta descida, toda caixa de
         -- série no balcão sairia sem capa.
         LEFT JOIN LATERAL (
             SELECT w2.artwork->>'poster' AS poster, w2.dominant_color AS cor
             FROM collection_item ci
             JOIN collection s ON s.id = ci.collection_id
             JOIN work w2 ON w2.id = ci.work_id
             WHERE (s.id = e.collection_id OR s.parent_id = e.collection_id)
               AND w2.artwork ? 'poster'
             LIMIT 1
         ) arte ON e.collection_id IS NOT NULL
         WHERE e.devolvido_em IS NULL
         ORDER BY e.vence_em",
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;

    // As últimas devoluções. É o balcão da locadora: a pilha do que voltou
    // hoje, com o estado em que voltou. Vira feed na R25 sem tabela nova.
    let devolvidas = sqlx::query_as::<_, Devolvida>(
        "SELECT COALESCE(e.work_id, e.collection_id) AS caixa_id,
                COALESCE(w.title, c.title)           AS titulo,
                u.display_name                       AS quem_nome,
                e.devolvido_em, e.devolvido_como, e.devolvido_por,
                e.devolvido_em > e.vence_em          AS atrasada
         FROM emprestimo e
         JOIN app_user u ON u.id = e.user_id
         LEFT JOIN work w       ON w.id = e.work_id
         LEFT JOIN collection c ON c.id = e.collection_id
         WHERE e.devolvido_em IS NOT NULL
         ORDER BY e.devolvido_em DESC
         LIMIT 8",
    )
    .fetch_all(&state.pool)
    .await?;

    let na_mao = pessoas.iter().find(|p| p.id == user.id).map_or(0, |p| p.na_mao);

    Ok(Json(Prateleira {
        posso_pegar: (opcoes.limite_por_pessoa as i64 - na_mao).max(0),
        ultimo_ano_vhs: ULTIMO_ANO_VHS,
        opcoes,
        pessoas,
        emprestadas,
        devolvidas,
    }))
}

// ----------------------------------------------------- R20: a loja da semana

/// Uma caixa exposta nesta semana.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CaixaExposta {
    pub id: Uuid,
    pub serie: bool,
    pub titulo: String,
    pub ano: Option<i32>,
    pub poster: String,
    pub dominant_color: Option<String>,
    pub temporadas: i64,
    pub media_file_id: Option<Uuid>,
    pub position_seconds: Option<f64>,
    /// Índice da estante em `ESTANTES`. A tela só agrupa; quem decide qual
    /// estante reivindica o quê é o servidor.
    pub estante: i32,
    /// Quantas caixas esta estante tem no acervo inteiro.
    ///
    /// Vem junto porque a placa precisa dizer **"3 de 113"**, e não "3". Um
    /// número que esconde o total é o "Biblioteca 300" que a R3 (§14) corrigiu:
    /// a pessoa não deve concluir que a loja tem três filmes de terror.
    pub total: i64,
    /// E quantas o acervo tem ao todo. Repetido em cada linha porque é uma
    /// janela sem partição — o custo é um `bigint` por caixa, e a alternativa é
    /// a tela somar os totais das estantes que apareceram, que dá outro número.
    #[serde(skip)]
    pub no_acervo: i64,
}

#[derive(Debug, Serialize)]
pub struct Estante {
    pub nome: String,
    pub total: i64,
    pub caixas: Vec<CaixaExposta>,
}

#[derive(Debug, Serialize)]
pub struct Loja {
    pub estantes: Vec<Estante>,
    /// Quantas caixas o acervo inteiro tem nas estantes.
    ///
    /// **Servido, e não somado pela tela.** A soma dos `total` das estantes
    /// devolvidas dá outro número: uma estante que não recebeu caixa nenhuma no
    /// sorteio não aparece, e o acervo dela some da conta junto. Na primeira
    /// semana em que isto rodou, a porta da loja disse *"597 no acervo"* de
    /// 600 — sumiram os 3 do faroeste, porque nenhum tinha sido sorteado.
    ///
    /// É o defeito do §14 outra vez, e ele volta sempre que a tela recalcula um
    /// número que o servidor já sabe: o botão que dizia "ver as 644" e abria
    /// 1.424 (§30) é o mesmo erro.
    pub no_acervo: i64,
    /// A segunda-feira desta rotação, em ISO. A tela diz "até segunda" com
    /// isso — e é o que torna a rotação uma promessa em vez de um sorteio.
    pub semana_de: chrono::NaiveDate,
    pub vira_em: chrono::DateTime<chrono::Utc>,
    pub ultimo_ano_vhs: i32,
}

/// A semana a que um instante pertence, e a segunda-feira seguinte.
///
/// Ancorada na **meia-noite local**, pelo mesmo `deslocamento()` da emissora
/// (§25) — que virou público exatamente pra isto. Duas leituras de fuso
/// divergindo fariam a loja virar num horário e a grade noutro.
///
/// Segunda-feira, e não "sete dias a partir de quando você entrou": uma loja
/// vira a vitrine num dia da semana, e uma janela deslizante por usuário faria
/// duas pessoas verem lojas diferentes no mesmo dia — que é justamente o que a
/// vitrine existe pra não fazer. A loja é a mesma pra todo mundo, e é por isso
/// que dá pra falar dela.
pub fn semana_e_virada(t: chrono::DateTime<chrono::Utc>) -> (chrono::NaiveDate, chrono::DateTime<chrono::Utc>) {
    use chrono::{Datelike, NaiveTime, TimeZone};

    let desl = crate::live::emissora::deslocamento();
    let local = t + desl;
    let hoje = local.date_naive();
    // `num_days_from_monday` dá 0 na segunda — subtrair volta pro começo da
    // semana sem depender de locale.
    let segunda = hoje - chrono::Duration::days(hoje.weekday().num_days_from_monday() as i64);
    let proxima = segunda + chrono::Duration::days(7);
    let vira = chrono::Utc.from_utc_datetime(&proxima.and_time(NaiveTime::MIN)) - desl;
    (segunda, vira)
}

/// A loja desta semana: as caixas sorteadas, e a estante em que cada uma caiu.
///
/// **Uma requisição, e não doze.** A tela pedia `/api/library` uma vez por
/// estante e juntava as respostas; agora a pergunta é uma só, porque a resposta
/// é uma só decisão.
///
/// ## O sorteio é na loja inteira (R29)
///
/// A R20 cortava **16 por estante**, o que dava 166 caixas de 600. A escala
/// pedida é outra:
///
/// > ~40 caixas na **loja inteira** por semana — não 40 por estante.
///
/// Então o `row_number()` deixou de ter `PARTITION BY estante`. Ele ordena as
/// 600 caixas pelo hash da semana e corta no `estoque`; a estante é só o
/// endereço de cada uma das que sobraram.
///
/// A consequência é a graça: **semana nenhuma tem a mesma forma.** Numa dá nove
/// de terror e nenhum faroeste, na outra o faroeste aparece e a ficção
/// científica lota. Estante que não recebeu nada não vira placa (§24), então a
/// loja muda até de silhueta. Cortar por estante dava a mesma vitrine com
/// conteúdo trocado — que é catálogo paginado, não loja.
///
/// **E o total de cada estante continua sendo o do acervo**, não o da amostra:
/// a janela que conta roda antes do corte, e é o que faz a placa dizer "3 de
/// 113" sem que ninguém conclua que a loja tem três filmes de terror.
///
/// **Sem tabela e sem daemon.** A rotação é `md5(semana || id)` calculada no
/// banco — o mesmo truque que a emissora (§25) usa pra programar três canais com
/// a semana no lugar do dia. Duas visitas na mesma semana veem a mesma loja, em
/// qualquer aparelho, sem nada pra sincronizar nem pra expirar; segunda-feira a
/// estante vira sozinha.
///
/// **A semente é só a semana** desde a R28. Ela tinha o círculo dentro, e era o
/// que dava ao círculo razão de existir antes de haver empréstimo nenhum: entrar
/// num círculo novo era entrar numa loja com outra vitrine. Com uma loja só, isso
/// vira o contrário do que a vitrine é para — **todo mundo vê a mesma
/// prateleira**, e é isso que faz a caixa da semana ser assunto em comum, do
/// mesmo jeito que o guia é igual pra todos (`IDEIAS.md` §2.4).
pub async fn estantes(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Loja>> {
    let opcoes = opcoes(&state.pool).await?;
    let (semana, vira_em) = semana_e_virada(chrono::Utc::now());
    let semente = semana.to_string();

    // As estantes viram uma tabela de valores literal. São 12 linhas de
    // constante do binário — nada aqui vem do cliente, então não há o que
    // escapar; mesmo assim a montagem só interpola o índice, e os gêneros vão
    // por bind.
    let valores: Vec<String> = (0..ESTANTES.len())
        .map(|i| format!("({i}, ${})", i + 3))
        .collect();

    let sql = format!(
        r#"
        WITH base AS (
            SELECT w.id AS work_id,
                   w.title, w.year, w.dominant_color,
                   w.artwork->>'poster' AS poster,
                   g.grupo_id, g.grupo_title, g.grupo_year, g.grupo_poster, g.season_id,
                   f.id AS media_file_id,
                   ps.position_seconds
            FROM work w
            LEFT JOIN LATERAL (
                SELECT COALESCE(series.id, season.id)       AS grupo_id,
                       COALESCE(series.title, season.title) AS grupo_title,
                       COALESCE(series.year, season.year)   AS grupo_year,
                       COALESCE(series.artwork->>'poster',
                                season.artwork->>'poster')  AS grupo_poster,
                       season.id                            AS season_id
                FROM collection_item ci
                JOIN collection season ON season.id = ci.collection_id
                LEFT JOIN collection series ON series.id = season.parent_id
                WHERE ci.work_id = w.id AND season.kind IN ('season', 'series')
                LIMIT 1
            ) g ON true
            LEFT JOIN LATERAL (
                SELECT m.id FROM media_file m
                WHERE m.work_id = w.id AND m.status = 'probed'
                ORDER BY m.size_bytes DESC LIMIT 1
            ) f ON true
            LEFT JOIN playback_state ps ON ps.work_id = w.id AND ps.user_id = $1
            -- Ignorada é obra que alguém descartou de propósito; ela não volta
            -- pela porta da locadora.
            WHERE w.match_state <> 'ignored'
        ),
        caixa AS (
            -- O rollup de `/api/library`: uma série é uma caixa, não vinte e
            -- uma fitas (§20). E **pôster é agregado, nunca chave** — pô-lo no
            -- GROUP BY parte o grupo quando a série não tem arte própria, que
            -- foi o defeito que a R18 pagou pra descobrir (§30).
            SELECT COALESCE(grupo_id, work_id)                  AS id,
                   grupo_id IS NOT NULL                         AS serie,
                   COALESCE(max(grupo_title), max(title))       AS titulo,
                   COALESCE(max(grupo_year), min(year))         AS ano,
                   COALESCE(max(grupo_poster), max(poster))     AS poster,
                   min(dominant_color)                          AS dominant_color,
                   count(DISTINCT season_id)                    AS temporadas,
                   -- `array_agg(...)[1]` e não `max(...)`: o Postgres não tem
                   -- `max(uuid)`. Aqui é exato, não um desempate arbitrário —
                   -- caixa avulsa é uma obra só, então o grupo tem uma linha.
                   -- `NOT (grupo_id IS NOT NULL)` e não `grupo_id IS NULL`:
                   -- o Postgres casa a expressão do GROUP BY por igualdade
                   -- sintática, e é `(grupo_id IS NOT NULL)` que está lá.
                   CASE WHEN NOT (grupo_id IS NOT NULL)
                        THEN (array_agg(media_file_id))[1] END       AS media_file_id,
                   max(position_seconds)                        AS position_seconds
            FROM base
            GROUP BY COALESCE(grupo_id, work_id), grupo_id IS NOT NULL
        ),
        genero AS (
            -- O gênero de uma caixa de série vem dos episódios: a coleção não
            -- carrega tag. Sem esta subida, nenhuma série entraria em estante
            -- nenhuma.
            SELECT DISTINCT COALESCE(b.grupo_id, b.work_id) AS caixa_id, t.value
            FROM base b
            JOIN work_tag wt ON wt.work_id = b.work_id
            JOIN tag t ON t.id = wt.tag_id AND t.namespace = 'genre'
        ),
        estante (idx, generos) AS (VALUES {valores}),
        atribuicao AS (
            -- `min(idx)`: a primeira estante que reivindica fica com o título.
            SELECT g.caixa_id, min(e.idx) AS estante
            FROM genero g
            JOIN estante e ON g.value = ANY(e.generos)
            GROUP BY g.caixa_id
        ),
        exposta AS (
            SELECT c.*, a.estante,
            {sorteio}
            FROM caixa c
            JOIN atribuicao a ON a.caixa_id = c.id
            -- Uma estante é feita de capas: o que não tem arte não entra (§20).
            WHERE c.poster IS NOT NULL
        )
        SELECT id, serie, titulo, ano, poster, dominant_color, temporadas,
               media_file_id, position_seconds, estante::int AS estante,
               total, no_acervo
        FROM exposta
        WHERE pos <= $ESTOQUE
        ORDER BY estante, pos
        "#,
        valores = valores.join(", "),
        sorteio = SORTEIO.trim_end(),
    );

    // O estoque entra por bind e não por interpolação. Ele vem de uma coluna
    // com `CHECK`, então não é entrada de cliente — mas é um número que alguém
    // digita numa tela, e número digitado não vira texto de SQL neste projeto.
    let sql = sql.replace("$ESTOQUE", &format!("${}", ESTANTES.len() + 3));

    let mut q = sqlx::query_as::<_, CaixaExposta>(&sql)
        .bind(user.id)
        .bind(&semente);
    for (_, generos) in ESTANTES {
        let g: Vec<String> = generos.iter().map(|s| s.to_string()).collect();
        q = q.bind(g);
    }
    let caixas = q.bind(opcoes.estoque as i64).fetch_all(&state.pool).await?;

    // Agrupar em Rust e não no JSON do Postgres: são no máximo 12 grupos, e o
    // nome da estante já está na constante daqui.
    let mut estantes: Vec<Estante> = Vec::new();
    for (i, (nome, _)) in ESTANTES.iter().enumerate() {
        let minhas: Vec<CaixaExposta> =
            caixas.iter().filter(|c| c.estante == i as i32).cloned().collect();
        if minhas.is_empty() {
            continue; // estante vazia não vira placa (§24)
        }
        estantes.push(Estante {
            nome: nome.to_string(),
            total: minhas[0].total,
            caixas: minhas,
        });
    }

    // O acervo inteiro vem na própria linha, pela mesma janela que conta o
    // total da estante — e pelo mesmo motivo: ela roda antes do corte. Zero
    // quando a loja está vazia, que é o único caso em que não há linha nenhuma
    // pra perguntar.
    let no_acervo = caixas.first().map_or(0, |c| c.no_acervo);

    Ok(Json(Loja {
        estantes,
        no_acervo,
        semana_de: semana,
        vira_em,
        ultimo_ano_vhs: ULTIMO_ANO_VHS,
    }))
}

#[derive(Debug, Deserialize)]
pub struct Alvo {
    /// Uma das duas, nunca as duas — a mesma regra do CHECK da 0021.
    pub work_id: Option<Uuid>,
    pub collection_id: Option<Uuid>,
}

impl Alvo {
    fn validar(&self) -> AppResult<()> {
        match (self.work_id, self.collection_id) {
            (Some(_), None) | (None, Some(_)) => Ok(()),
            _ => Err(AppError::BadRequest(
                "informe work_id ou collection_id, exatamente um".into(),
            )),
        }
    }
}

/// Pegar uma caixa.
pub async fn alugar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(alvo): Json<Alvo>,
) -> AppResult<Json<Value>> {
    alvo.validar()?;
    let opcoes = opcoes(&state.pool).await?;
    devolver_vencidos(&state.pool, &state.events).await?;

    let mut tx = state.pool.begin().await?;

    // **Serializa esta pessoa contra ela mesma.** Sem isto, dois cliques
    // simultâneos contam o limite antes de qualquer um inserir e os dois
    // passam. Travar a linha do usuário é mais barato que travar a tabela e é
    // suficiente: o limite é por pessoa.
    //
    // Era a linha de `circulo_membro` até a R28, e a troca não é só de tabela —
    // aquela linha também respondia "você está neste círculo?". Sem grupo, a
    // pergunta some: estar no servidor é estar na loja.
    //
    // O limite da *caixa* não precisa disto — quem o impõe é o índice único
    // parcial da 0028, e o banco não tem corrida consigo mesmo.
    sqlx::query("SELECT 1 FROM app_user WHERE id = $1 FOR UPDATE")
        .bind(user.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

    let na_mao: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM emprestimo WHERE user_id = $1 AND devolvido_em IS NULL",
    )
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await?;

    if na_mao >= opcoes.limite_por_pessoa as i64 {
        return Err(AppError::Forbidden(format!(
            "você já está com {na_mao} — devolva uma antes de pegar outra"
        )));
    }

    // **`exclusivo` é a chave de escassez, gravada na linha.** Ela não vira um
    // `if` que decide pular a inserção: o empréstimo nasce carregando o regime,
    // e quem recusa continua sendo o índice único parcial da 0029. A regra
    // permanece do banco, que é o que o §35 comprou e o §5 defende.
    let inserido = sqlx::query_as::<_, (i64,)>(
        "INSERT INTO emprestimo (user_id, work_id, collection_id, vence_em, exclusivo)
         VALUES ($1, $2, $3, now() + ($4 || ' days')::interval, $5)
         ON CONFLICT DO NOTHING
         RETURNING id",
    )
    .bind(user.id)
    .bind(alvo.work_id)
    .bind(alvo.collection_id)
    .bind(opcoes.prazo_dias.to_string())
    .bind(opcoes.escassez)
    .fetch_optional(&mut *tx)
    .await?;

    // `ON CONFLICT DO NOTHING` + nenhuma linha = um dos índices únicos recusou,
    // e agora são dois motivos possíveis. Perguntar qual, em vez de devolver
    // "falhou", é o que transforma o bloqueio em porta — a resposta traz o nome
    // pra tela poder oferecer "pedir de volta".
    let Some((id,)) = inserido else {
        tx.rollback().await?;

        // Quem está com ela. `meu` primeiro na ordenação porque, com a escassez
        // desligada, o único índice que ainda recusa é o de uma caixa por
        // pessoa — e aí quem está com ela é você. Dizer "fulano está com esta"
        // nesse caso mandaria pedir de volta a própria fita.
        let com_quem: Option<(String, bool)> = sqlx::query_as(
            "SELECT u.display_name, e.user_id = $1 AS meu
             FROM emprestimo e
             JOIN app_user u ON u.id = e.user_id
             WHERE e.devolvido_em IS NULL
               AND (e.work_id = $2 OR e.collection_id = $3)
             ORDER BY meu DESC
             LIMIT 1",
        )
        .bind(user.id)
        .bind(alvo.work_id)
        .bind(alvo.collection_id)
        .fetch_optional(&state.pool)
        .await?;

        return Err(AppError::Forbidden(match com_quem {
            Some((_, true)) => "esta já está com você".into(),
            Some((nome, false)) => format!("{nome} está com esta"),
            // Não deveria acontecer: o índice recusou e ninguém a tem. Dizer
            // "está alugada" aqui seria afirmar o que não se sabe (§8b).
            None => "não foi possível pegar esta caixa".into(),
        }));
    };

    let titulo = titulo_da_caixa(&mut tx, &alvo).await?;
    tx.commit().await?;

    events::publish(
        &state.events,
        AppEvent::Locadora {
            acao: "pegou".into(),
            caixa_id: alvo.work_id.or(alvo.collection_id),
            titulo: Some(titulo.clone()),
            quem_nome: Some(user.display_name.clone()),
        },
    );

    // R35: pegar uma caixa é um desafio possível.
    crate::desafios::conferir(&state.pool, user.id).await;

    Ok(Json(json!({
        "id": id,
        "titulo": titulo,
        "vence_em_dias": opcoes.prazo_dias,
    })))
}

async fn titulo_da_caixa(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    alvo: &Alvo,
) -> AppResult<String> {
    let titulo: Option<(String,)> = match (alvo.work_id, alvo.collection_id) {
        (Some(w), _) => sqlx::query_as("SELECT title FROM work WHERE id = $1")
            .bind(w)
            .fetch_optional(&mut **tx)
            .await?,
        (_, Some(c)) => sqlx::query_as("SELECT title FROM collection WHERE id = $1")
            .bind(c)
            .fetch_optional(&mut **tx)
            .await?,
        _ => None,
    };
    Ok(titulo.map(|t| t.0).unwrap_or_else(|| "a caixa".into()))
}

/// Devolver. Só quem está com ela.
pub async fn devolver(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    // A condição é lida e **congelada** agora, e não derivada na hora de exibir:
    // quem devolveu pode reassistir amanhã, e aí "voltou no meio" já teria sido
    // reescrito por um progresso posterior. O histórico não pode depender do
    // presente de outra pessoa.
    let sql = format!(
        r#"
        WITH alvo AS (
            SELECT e.id, e.user_id, e.work_id, e.collection_id
            FROM emprestimo e
            WHERE e.id = $1 AND e.user_id = $2
              AND e.devolvido_em IS NULL
        ),
        com_estado AS (
            SELECT a.id, p.position_seconds, p.duration_seconds, p.finished
            FROM alvo a
            LEFT JOIN LATERAL ({condicao}) p ON true
        )
        UPDATE emprestimo e
        SET devolvido_em   = now(),
            devolvido_por  = 'membro',
            devolvido_como = {classificacao}
        FROM com_estado ce
        WHERE e.id = ce.id
        RETURNING e.devolvido_como, e.devolvido_em > e.vence_em AS atrasada,
                  COALESCE(e.work_id, e.collection_id) AS caixa_id
        "#,
        condicao = classificacao_alvo(),
        classificacao = classificacao_sql("ce"),
    );

    let devolvida: Option<(String, bool, Uuid)> = sqlx::query_as(&sql)
        .bind(id)
        .bind(user.id)
        .fetch_optional(&state.pool)
        .await?;

    let Some((como, atrasada, caixa_id)) = devolvida else {
        return Err(AppError::NotFound);
    };

    events::publish(
        &state.events,
        AppEvent::Locadora {
            acao: "devolveu".into(),
            caixa_id: Some(caixa_id),
            titulo: None,
            quem_nome: Some(user.display_name.clone()),
        },
    );

    Ok(Json(json!({
        "devolvido_como": como,
        "atrasada": atrasada,
    })))
}

/// A mesma condição da varredura, com o alias que a devolução usa.
fn classificacao_alvo() -> String {
    condicao_sql("a")
}

/// Pedir de volta.
///
/// **Não encurta o prazo de ninguém.** É registro e aviso — e é a saída que
/// impede o bloqueio de ser parede. Dar a alguém poder sobre o prazo do outro
/// transformaria a locadora em disputa, e a decisão de barrar só é defensável
/// porque a escassez é social: a resposta a ela também é.
pub async fn pedir_de_volta(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<Value>> {
    // `pedido_em IS NULL` no WHERE: pedir duas vezes não reinicia o relógio do
    // pedido. Quem já pediu já pediu — e insistir não é feature.
    let pedido: Option<(String, Uuid)> = sqlx::query_as(
        "UPDATE emprestimo e
         SET pedido_em = now(), pedido_por = $2
         FROM app_user u
         WHERE e.id = $1
           AND e.devolvido_em IS NULL
           AND e.pedido_em IS NULL
           -- Pedir de volta o que está na sua própria mão não é pedir nada.
           AND e.user_id <> $2
           AND u.id = e.user_id
         RETURNING u.display_name, COALESCE(e.work_id, e.collection_id) AS caixa_id",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?;

    let Some((com_quem, caixa_id)) = pedido else {
        return Err(AppError::BadRequest(
            "nada a pedir: ou já foi pedida, ou já voltou, ou é sua".into(),
        ));
    };

    // Chega na hora — o barramento do M3 já entrega isso desde sempre, e é o
    // que faz o pedido ser uma conversa em vez de um bilhete no mural.
    events::publish(
        &state.events,
        AppEvent::Locadora {
            acao: "pediu".into(),
            caixa_id: Some(caixa_id),
            titulo: None,
            quem_nome: Some(user.display_name.clone()),
        },
    );

    Ok(Json(json!({ "pedido_a": com_quem })))
}

/// A fita, e quem a deixou assim (R30).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Fita {
    /// Onde a fita está. Zero é rebobinada.
    pub posicao_segundos: f64,
    pub duracao_segundos: Option<f64>,
    /// Quem deixou assim. `None` quando a fita nunca foi tocada, ou quando a
    /// conta de quem a deixou não existe mais — a fita sobrevive ao dono.
    pub deixada_por: Option<String>,
    pub deixada_em: Option<chrono::DateTime<chrono::Utc>>,
    /// Se fui **eu** que deixei assim.
    ///
    /// É o que decide se o rebobinar é obrigatório. Pausar o próprio filme e
    /// voltar amanhã é continuar de onde parou; encontrar a fita de outra pessoa
    /// no minuto 47 é outra coisa, e é a que a fase inteira existe pra fazer
    /// doer um pouco.
    pub minha: bool,
    /// Se isto é uma fita. DVD não é — ele lembra onde parou (§35).
    pub vhs: bool,
}

/// Onde está esta fita.
///
/// **Uma rota própria, chamada no play** — e não um campo da estante. A decisão
/// é do `IDEIAS.md` §3.9, e é cena a cena:
///
/// > *"você descobre quando põe pra tocar — não na estante, não antes"*
///
/// Mandar o estado junto com as 40 caixas da vitrine seria mais barato em
/// requisições e destruiria a única coisa que esta tela tem: a surpresa. A
/// estante não sabe, de propósito.
pub async fn fita(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(work_id): Path<Uuid>,
) -> AppResult<Json<Fita>> {
    let linha: Option<(Option<i32>, Option<f64>, Option<f64>, Option<String>, Option<chrono::DateTime<chrono::Utc>>, Option<Uuid>)> =
        sqlx::query_as(
            "SELECT w.year, f.posicao_segundos, f.duracao_segundos,
                    u.display_name, f.deixada_em, f.deixada_por
             FROM work w
             LEFT JOIN fita f      ON f.work_id = w.id
             LEFT JOIN app_user u  ON u.id = f.deixada_por
             WHERE w.id = $1",
        )
        .bind(work_id)
        .fetch_optional(&state.pool)
        .await?;

    let Some((ano, pos, dur, quem, quando, quem_id)) = linha else {
        return Err(AppError::NotFound);
    };

    Ok(Json(Fita {
        posicao_segundos: pos.unwrap_or(0.0),
        duracao_segundos: dur,
        deixada_por: quem,
        deixada_em: quando,
        minha: quem_id == Some(user.id),
        vhs: matches!(ano, Some(a) if a <= ULTIMO_ANO_VHS),
    }))
}

/// Rebobinar a fita.
///
/// ## O que mudou, e é a fase inteira (R30)
///
/// Isto zerava **o seu próprio `playback_state`**, e o §35 registrou a recusa de
/// deixar alguém rebobinar a fita de outra pessoa — chamando de *"a primeira
/// ação destrutiva entre usuários que este projeto teria"*.
///
/// A recusa estava certa e a modelagem é que estava errada. Enquanto a fita for
/// a memória de alguém, rebobinar apaga o "continuar de onde parou" dessa
/// pessoa, e aí realmente é destrutivo. Com a fita sendo **um objeto**
/// (`fita`, R30), rebobinar mexe no objeto e em ninguém: nenhum
/// `playback_state` é tocado aqui, e o de todo mundo continua intacto.
///
/// O que muda é só o que a próxima pessoa encontra ao pôr pra tocar. Que é o
/// que foi pedido desde a primeira anotação.
///
/// **E fica registrado quem teve o trabalho.** Cada rebobinada guarda dois
/// nomes — quem gastou os segundos e quem tinha deixado assim. É desta tabela
/// que sai a reputação no balcão, e é o *"as pessoas saberem quem devolveu
/// zoado"* virando dado.
///
/// **Só em VHS.** O DVD não rebobina — ele lembra onde parou, e é por isso que
/// ele tem menu. A diferença entre os formatos, que a R8 usou só como estética
/// (lombada de papel contra lombada de plástico), vira comportamento.
pub async fn rebobinar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(alvo): Json<Alvo>,
) -> AppResult<Json<Value>> {
    alvo.validar()?;

    let ano: Option<(Option<i32>,)> = match (alvo.work_id, alvo.collection_id) {
        (Some(w), _) => sqlx::query_as("SELECT year FROM work WHERE id = $1")
            .bind(w)
            .fetch_optional(&state.pool)
            .await?,
        (_, Some(c)) => sqlx::query_as("SELECT year FROM collection WHERE id = $1")
            .bind(c)
            .fetch_optional(&state.pool)
            .await?,
        _ => None,
    };
    let Some((ano,)) = ano else {
        return Err(AppError::NotFound);
    };
    if !matches!(ano, Some(a) if a <= ULTIMO_ANO_VHS) {
        return Err(AppError::BadRequest(
            "isto é um DVD — ele não rebobina, ele lembra onde parou".into(),
        ));
    }

    // **Nenhum `playback_state` é tocado aqui.** É a diferença entre a R19 e a
    // R30 numa linha: o que se rebobina é a fita.
    //
    // Uma consulta só, em três CTEs, e a ordem importa. `antes` **lê o estado
    // que está sendo desfeito** — `RETURNING` de um UPDATE devolve os valores
    // novos, então perguntar depois daria zero segundos e o nome errado no log.
    // O `FOR UPDATE` trava as linhas: dois rebobinares simultâneos da mesma
    // caixa serializam, e o segundo encontra a fita já no começo em vez de
    // escrever uma segunda linha de log dizendo que desfez o mesmo trabalho.
    let obras = OBRAS_DA_CAIXA.replace("$ALVO", "$2");
    let rebobinadas: i64 = sqlx::query_scalar(&format!(
        "WITH antes AS (
             SELECT work_id, posicao_segundos, deixada_por
             FROM fita
             WHERE posicao_segundos > 0
               AND ($1::uuid IS NOT NULL AND work_id = $1
                    OR $2::uuid IS NOT NULL AND work_id IN ({obras}))
             FOR UPDATE
         ),
         zerada AS (
             UPDATE fita f
             SET posicao_segundos = 0, deixada_por = $3, deixada_em = now()
             FROM antes a WHERE f.work_id = a.work_id
             RETURNING f.work_id
         ),
         -- O log guarda a verdade, inclusive quando a bagunça é sua: quem
         -- filtra 'rebobinei a minha própria' é quem lê a reputação, e não
         -- quem escreve o fato. Um log que mente por educação não serve de log.
         registrada AS (
             INSERT INTO rebobinada (work_id, por, de, segundos)
             SELECT a.work_id, $3, a.deixada_por, a.posicao_segundos FROM antes a
             RETURNING 1
         )
         SELECT count(*) FROM antes"
    ))
    .bind(alvo.work_id)
    .bind(alvo.collection_id)
    .bind(user.id)
    .fetch_one(&state.pool)
    .await?;

    if rebobinadas > 0 {
        tracing::info!(quem = %user.username, fitas = rebobinadas, "rebobinou");
    }

    // R35: rebobinar a fita de alguém também é um desafio possível.
    crate::desafios::conferir(&state.pool, user.id).await;

    Ok(Json(json!({ "rebobinadas": rebobinadas })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O alvo é obra **ou** coleção, e a checagem não pode divergir do CHECK
    /// `emprestimo_uma_caixa` — se divergir, um dos dois recusa o que o outro
    /// aceita e o erro sai como 500 em vez de 400.
    #[test]
    fn alvo_exige_exatamente_um() {
        let so_obra = Alvo { work_id: Some(Uuid::nil()), collection_id: None };
        let so_colecao = Alvo { work_id: None, collection_id: Some(Uuid::nil()) };
        let nenhum = Alvo { work_id: None, collection_id: None };
        let ambos = Alvo { work_id: Some(Uuid::nil()), collection_id: Some(Uuid::nil()) };

        assert!(so_obra.validar().is_ok());
        assert!(so_colecao.validar().is_ok());
        assert!(nenhum.validar().is_err());
        assert!(ambos.validar().is_err());
    }

    /// A classificação tem que citar o mesmo 0.92 do §8f. Um literal solto aqui
    /// faria a locadora discordar da curadoria e do guia sobre a palavra
    /// "terminada" — que é exatamente o defeito que a R18 desenterrou (§30).
    #[test]
    fn classificacao_usa_a_razao_do_m5() {
        let sql = classificacao_sql("ce");
        assert!(sql.contains("0.92"), "razão do §8f sumiu: {sql}");
        assert!(sql.contains("rebobinada"));
        assert!(sql.contains("terminada"));
        assert!(sql.contains("no-meio"));
    }

    /// `position_seconds <= 0` vem antes do teste de terminada, e a ordem
    /// importa: uma fita rebobinada depois de terminada tem `finished = true` e
    /// posição 0. Ela está **rebobinada** — é o que a próxima pessoa encontra.
    #[test]
    fn rebobinada_ganha_de_terminada() {
        let sql = classificacao_sql("ce");
        let i_rebobinada = sql.find("'rebobinada'").expect("sem rebobinada");
        let i_terminada = sql.find("'terminada'").expect("sem terminada");
        assert!(
            i_rebobinada < i_terminada,
            "a ordem do CASE inverteu: posição zero tem que decidir antes de finished"
        );
    }

    /// A caixa de série alcança a obra por série→temporada→obra, e o alcance é
    /// de dois níveis. Sem o `parent_id`, uma caixa de série nunca acharia
    /// episódio nenhum e toda devolução de box sairia 'rebobinada'.
    #[test]
    fn obras_da_caixa_alcanca_serie_e_temporada() {
        let sql = OBRAS_DA_CAIXA.replace("$ALVO", "$3");
        assert!(sql.contains("c.id = $3"), "temporada órfã fora: {sql}");
        assert!(sql.contains("c.parent_id = $3"), "série fora: {sql}");
    }

    /// As duas devoluções — a do membro e a do prazo — precisam classificar
    /// igual. Duas cópias divergindo fariam a mesma fita contar duas histórias
    /// dependendo de quem a devolveu.
    #[test]
    fn as_duas_devolucoes_compartilham_a_condicao() {
        assert_eq!(classificacao_alvo(), condicao_sql("a"));
        // O progresso de quem devolveu ainda é lido pelo mais recente: é ele
        // que responde "tem alguém assistindo agora".
        assert!(condicao_sql("v").contains("ORDER BY p.updated_at DESC"));
    }

    /// **A fita manda na condição, e o heartbeat manda no relógio** (R30).
    ///
    /// São duas perguntas diferentes lidas da mesma linha, e trocá-las quebra
    /// coisas distintas: a condição vinda do `playback_state` volta a dizer
    /// "até onde essa pessoa foi" em vez de "onde a fita está"; o relógio vindo
    /// da fita faz a devolução automática arrancar um **DVD** — que não tem
    /// fita — da mão de quem está no meio do filme.
    #[test]
    fn a_fita_manda_na_condicao_e_o_heartbeat_no_relogio() {
        let sql = condicao_sql("v");
        assert!(
            sql.contains("COALESCE(ft.posicao_segundos, ps.position_seconds)"),
            "a fita deixou de mandar na condição: {sql}"
        );
        assert!(
            sql.contains("ps.updated_at
"),
            "o relógio deixou de ser o do heartbeat: {sql}"
        );
        // E o `finished` acumulativo (§31) só vale quando não há fita: ele
        // responde "já terminei alguma vez", não "a fita está no fim".
        assert!(sql.contains("(ft.posicao_segundos IS NULL AND ps.finished)"));
    }

    // ------------------------------------------------- R20: a loja da semana

    fn instante(s: &str) -> chrono::DateTime<chrono::Utc> {
        s.parse().expect("data de teste inválida")
    }

    /// A semana começa na **segunda**, e a virada é a segunda seguinte.
    ///
    /// Uma janela deslizante ("sete dias desde que você entrou") faria duas
    /// pessoas verem lojas diferentes no mesmo dia — que é o oposto do que a
    /// vitrine existe pra fazer.
    #[test]
    fn a_semana_comeca_na_segunda() {
        use chrono::Datelike;
        // 2026-08-02 é um domingo; 2026-08-05, uma quarta.
        for t in ["2026-08-02T14:00:00Z", "2026-08-05T14:00:00Z"] {
            let (semana, vira) = semana_e_virada(instante(t));
            assert_eq!(
                semana.weekday(),
                chrono::Weekday::Mon,
                "{t} caiu numa semana que não começa na segunda: {semana}"
            );
            assert!(vira > instante(t), "{t}: a virada tem que estar no futuro");
        }
    }

    /// Domingo e a segunda seguinte são semanas **diferentes**, e é exatamente
    /// na virada da meia-noite local que a loja muda. Ancorar em UTC faria a
    /// vitrine virar às 21h de domingo aqui.
    #[test]
    fn domingo_e_segunda_sao_semanas_diferentes() {
        let (dom, vira_dom) = semana_e_virada(instante("2026-08-02T23:00:00Z"));
        let (seg, _) = semana_e_virada(instante("2026-08-03T23:00:00Z"));
        assert_ne!(dom, seg, "domingo e segunda caíram na mesma semana");
        assert_eq!(
            vira_dom,
            instante("2026-08-03T03:00:00Z"),
            "a virada não é meia-noite local (UTC-3 por padrão)"
        );
    }

    /// Duas leituras na mesma semana dão a mesma semente, e é isso que faz a
    /// loja ser a mesma em qualquer aparelho sem nada pra sincronizar.
    ///
    /// **E a semente não depende de mais nada.** Ela tinha o círculo dentro até
    /// a R28; se voltar a ter qualquer coisa por pessoa ou por grupo, duas
    /// pessoas passam a ver vitrines diferentes na mesma semana e a caixa da
    /// semana deixa de ser assunto em comum.
    #[test]
    fn a_semente_e_estavel_na_semana_e_muda_na_seguinte() {
        let a = semana_e_virada(instante("2026-08-04T09:00:00Z")).0;
        let b = semana_e_virada(instante("2026-08-07T22:00:00Z")).0;
        assert_eq!(a, b, "terça e sexta da mesma semana deram semanas diferentes");

        let prox = semana_e_virada(instante("2026-08-11T09:00:00Z")).0;
        assert_ne!(a.to_string(), prox.to_string(), "a semana seguinte não virou a vitrine");
    }

    /// **O sorteio é na loja inteira.** Esta é a linha que separa "40 caixas na
    /// loja" de "40 por estante", e ela é uma palavra: um `PARTITION BY` no
    /// `row_number()` traria a R20 de volta sem quebrar nenhum outro teste — a
    /// loja continuaria funcionando, só que com 12 vezes mais caixas.
    #[test]
    fn o_sorteio_nao_particiona_por_estante() {
        // A ordem das duas janelas importa: a que corta não particiona, a que
        // conta o acervo particiona. Trocá-las inverte a fase inteira.
        let corte = "row_number() OVER (ORDER BY md5(";
        let conta = "count(*) OVER (PARTITION BY a.estante)";
        assert!(SORTEIO.contains(corte), "o corte voltou a particionar: {SORTEIO}");
        assert!(SORTEIO.contains(conta), "o total da estante deixou de ser o do acervo");
    }

    /// Os `$n` da tabela de estantes começam em `$3` — `$1` é o usuário e `$2`
    /// a semente. Se isto desalinhar, o Postgres casa gênero com semente e a
    /// consulta falha ou, pior, devolve estante vazia em silêncio.
    #[test]
    fn os_placeholders_das_estantes_comecam_no_terceiro() {
        let valores: Vec<String> = (0..ESTANTES.len())
            .map(|i| format!("({i}, ${})", i + 3))
            .collect();
        assert_eq!(valores.first().unwrap(), "(0, $3)");
        assert_eq!(
            valores.last().unwrap(),
            &format!("({}, ${})", ESTANTES.len() - 1, ESTANTES.len() + 2)
        );
    }

    /// O estoque entra **depois** dos gêneros, e é o último `$n`. Se ele
    /// colidir com um gênero, a consulta compara título com número e a loja
    /// volta vazia — sem erro nenhum pra alguém notar.
    #[test]
    fn o_estoque_e_o_ultimo_placeholder() {
        assert_eq!(format!("${}", ESTANTES.len() + 3), "$15");
        // $1 usuário, $2 semente, $3..$14 os doze gêneros, $15 o estoque.
        assert_eq!(ESTANTES.len(), 12);
    }

    /// A ordem das estantes é a regra de reivindicação, e ela tem uma
    /// invariante: **Drama é a última.** Ele casa com meia biblioteca, e à
    /// frente de qualquer outra deixaria as demais vazias.
    #[test]
    fn drama_reivindica_por_ultimo() {
        assert_eq!(ESTANTES.last().unwrap().0, "Drama");
        assert!(ESTANTES.len() >= 2);
    }
}
