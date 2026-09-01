use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Library {
    pub id: Uuid,
    pub name: String,
    pub root_path: String,
    pub default_kind: String,
    pub provider_hint: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct NewLibrary {
    pub name: String,
    pub root_path: String,
    /// O que os arquivos soltos desta pasta são: `movie`, `episode`…
    #[serde(default = "default_kind")]
    pub default_kind: String,
    /// `auto` decide por heurística; `anilist` força anime; `none` não identifica.
    #[serde(default = "default_hint")]
    pub provider_hint: String,
}

fn default_hint() -> String {
    "auto".to_string()
}

fn default_kind() -> String {
    "movie".to_string()
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Work {
    pub id: Uuid,
    pub kind: String,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub runtime_seconds: Option<i32>,
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub external_ids: serde_json::Value,
    pub match_state: String,
    pub match_confidence: Option<f32>,
    pub artwork: serde_json::Value,
    pub dominant_color: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// O que a lista da biblioteca devolve: a obra + o suficiente pra tocar e
/// pra mostrar "continuar de onde parou", sem carregar o probe inteiro.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct WorkListItem {
    pub id: Uuid,
    pub kind: String,
    pub title: String,
    pub year: Option<i32>,
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub match_state: String,
    pub match_confidence: Option<f32>,
    pub dominant_color: Option<String>,
    /// A sinopse, na lista e não só na ficha (R63).
    ///
    /// É a linha da lista de episódios que responde "qual era esse mesmo?" sem
    /// abrir nada. Nula em 7.216 dos 14.844 episódios do acervo — e nula é a
    /// resposta certa: o cliente não desenha a linha em vez de inventá-la.
    pub overview: Option<String>,
    /// Caminho relativo servido em `/artwork/...`; None enquanto não identificado.
    pub poster: Option<String>,
    /// Arte larga da obra. Baixada desde o M1 junto com o pôster, mas só passou
    /// a sair daqui no redesenho: o herói do painel precisa de 16:9, e recortar
    /// um pôster 2:3 pra isso dá enquadramento ruim em toda obra.
    pub backdrop: Option<String>,
    /// Quadro do episódio, quando o provider tem. Melhor que o backdrop da
    /// série no painel — é a cena daquele episódio, não da série inteira.
    pub still: Option<String>,
    /// Nome da série, quando a obra é um episódio dentro de uma coleção.
    pub series_title: Option<String>,
    pub media_file_id: Option<Uuid>,
    pub duration_seconds: Option<f64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub container: Option<String>,
    pub size_bytes: Option<i64>,
    pub position_seconds: Option<f64>,
    pub finished: Option<bool>,
    /// Tags achatadas em `namespace:valor`, pra UI não precisar de outra volta.
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MediaFileSummary {
    pub id: Uuid,
    pub path: String,
    pub filename: String,
    pub size_bytes: i64,
    pub container: Option<String>,
    pub duration_seconds: Option<f64>,
    pub bitrate: Option<i64>,
    pub video_codec: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub frame_rate: Option<f64>,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<i32>,
    pub subtitle_langs: Vec<String>,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct WorkDetail {
    #[serde(flatten)]
    pub work: Work,
    pub files: Vec<MediaFileSummary>,
    pub position_seconds: f64,
    pub finished: bool,
    /// As três faces do grafo, do ponto de vista desta obra.
    pub tags: Vec<WorkTag>,
    pub collections: Vec<CollectionRow>,
    pub relations: Vec<RelationRow>,
    pub credits: Vec<crate::routes::people::CreditRow>,
}

#[derive(Debug, Deserialize)]
pub struct ProgressReport {
    pub position_seconds: f64,
    pub duration_seconds: Option<f64>,
    pub media_file_id: Option<Uuid>,
    #[serde(default = "default_event")]
    pub event_type: String,
    pub client: Option<String>,
    /// Identifica o aparelho, pra ele ignorar o próprio eco no SSE.
    pub device_id: Option<String>,
}

fn default_event() -> String {
    "progress".to_string()
}

// ------------------------------------------------------------------ M1: match

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MatchCandidateRow {
    pub id: Uuid,
    pub work_id: Uuid,
    pub provider: String,
    pub provider_id: String,
    pub provider_kind: String,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub score: f32,
    /// Os motivos legíveis do score. É isto que o Jellyfin nunca te mostra.
    pub reasons: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ReviewWork {
    pub id: Uuid,
    pub title: String,
    pub year: Option<i32>,
    pub kind: String,
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub match_state: String,
    pub match_confidence: Option<f32>,
    /// Por que a obra está na fila, quando o motivo não veio de um candidato —
    /// propagação de escopo, ou identificação desfeita por contradizer o
    /// provider. Ver a coluna homônima na migração 0008.
    pub match_reasons: serde_json::Value,
    pub filename: String,
    /// **Quando esta identificação foi feita** — o que separa "mexi nisto" de
    /// "decidi isto".
    ///
    /// Numa obra `confirmed` é o instante em que uma PESSOA confirmou: o
    /// matcher automático pula `confirmed` e `ignored`, então nada sobrescreve
    /// depois. Numa `auto`, é quando o matcher decidiu sozinho — que é
    /// exatamente o que a conferência quer revisitar, porque nunca passou por
    /// revisão nenhuma.
    ///
    /// `null` em obra nunca identificada, e também nas que foram identificadas
    /// antes de alguém ler esta coluna. Fica nulo: a tela mostra "sem data" em
    /// vez de inventar um instante.
    pub matched_at: Option<DateTime<Utc>>,
}

/// O que o parser entendeu do nome do arquivo — mostrado lado a lado com os
/// candidatos, pra ficar óbvio POR QUE o match ficou em dúvida.
#[derive(Debug, Clone, Serialize)]
pub struct GuessView {
    pub title: String,
    pub year: Option<i32>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub absolute_episode: Option<i32>,
    pub release_group: Option<String>,
    pub looks_like_anime: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewItem {
    pub work: ReviewWork,
    pub guess: GuessView,
    pub candidates: Vec<MatchCandidateRow>,
}

#[derive(Debug, Deserialize)]
pub struct ConfirmMatch {
    pub candidate_id: Uuid,

    /// Até onde a decisão vale:
    ///   `work`      — só esta obra (padrão, e o comportamento de sempre)
    ///   `directory` — as obras da MESMA pasta
    ///   `series`    — a subárvore da série (sobe uma pasta se esta é temporada)
    ///
    /// Sempre explícito: propagar por inferência é como se apaga uma biblioteca
    /// inteira sem perceber.
    #[serde(default = "so_esta")]
    pub apply_to: String,

    /// Mostra o que aconteceria sem escrever. A interface pede isto antes de
    /// oferecer o botão quando o escopo passa de uma obra.
    #[serde(default)]
    pub dry_run: bool,
}

fn so_esta() -> String {
    "work".to_string()
}

#[derive(Debug, Deserialize)]
pub struct BulkMatch {
    /// Pares (obra, candidato). Cada obra escolhe o SEU candidato — não há
    /// "aplicar o mesmo id a todas", porque candidato é por obra.
    pub items: Vec<BulkMatchItem>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Deserialize)]
pub struct BulkMatchItem {
    pub work_id: Uuid,
    pub candidate_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct BulkState {
    pub work_ids: Vec<Uuid>,
    /// `ignored` para material extra; `unmatched` para devolver à fila.
    pub state: String,
    /// Vira `match_reasons` — quem abrir a fila depois precisa saber por quê.
    pub reason: Option<String>,
}

// --- escopo de identificação por pasta ------------------------------------

#[derive(Debug, Deserialize)]
pub struct ScopeQuery {
    /// Só pastas desta biblioteca.
    pub library: Option<Uuid>,
    /// Substring do caminho — é como se acha "Naruto" entre 578 pastas.
    pub q: Option<String>,
    /// `files` (padrão): as pastas que resolvem mais arquivos primeiro. É a
    /// ordenação que faz a fila encolher rápido. `path` pra varrer em ordem.
    ///
    /// `identificadas_recentes` é a do modo conferência, e existe porque o
    /// padrão não serve lá: pasta já conferida tem zero pendentes, e no
    /// `pendentes DESC` todas empatariam.
    pub sort: Option<String>,
    /// **Que pastas listar** (R77). O padrão é o de sempre.
    ///
    /// | valor | o que devolve |
    /// |---|---|
    /// | `pendentes` (padrão) | só pasta com arquivo por identificar |
    /// | `identificadas` | pasta que já tem arquivo `auto` ou `confirmed` |
    /// | `todas` | sem filtro |
    ///
    /// **`identificadas` não é o complemento de `pendentes`.** Uma pasta com
    /// cinco arquivos identificados e cinco não aparece NOS DOIS modos — ela é
    /// ao mesmo tempo trabalho a fazer e decisão a conferir, e escondê-la de um
    /// dos lados perderia metade dela.
    ///
    /// Valor desconhecido cai no padrão, como o `sort` e o `tipo` do
    /// `works::list` — a fila de trabalho é o uso normal e não pode virar erro
    /// por causa de um erro de digitação.
    pub mostrar: Option<String>,
    #[serde(default = "cem")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn cem() -> i64 {
    100
}

/// Uma pasta com trabalho pendente, e tudo que ajuda a decidir sobre ela.
#[derive(Debug, Serialize)]
pub struct ScopeRow {
    pub dir_path: String,
    pub library_id: Uuid,
    pub library_name: String,
    /// Arquivos ainda não identificados aqui — o que esta decisão resolve.
    pub pendentes: i64,
    pub unmatched: i64,
    pub needs_review: i64,
    /// Já identificados na mesma pasta. Quando > 0, o `sibling_match` abaixo
    /// costuma ser a resposta.
    pub ja_identificados: i64,
    pub exemplos: Vec<String>,
    /// O que o parser entende do NOME DA PASTA — normalmente o nome da série.
    pub titulo_sugerido: String,
    /// **Quando esta pasta foi identificada** — o `matched_at` MAIS RECENTE
    /// entre os arquivos já identificados dela.
    ///
    /// O mais recente, e não o primeiro, porque é ele que responde a pergunta
    /// do modo conferência: *o que eu decidi por último?* Uma pasta mexida
    /// ontem e completada hoje é uma pasta de hoje.
    ///
    /// `null` quando não há arquivo identificado, ou quando os que há vêm de
    /// antes de a coluna passar a ser lida.
    pub identificada_em: Option<DateTime<Utc>>,
    /// A obra que os irmãos já casados apontam. Não é palpite: é o que o
    /// próprio acervo já decidiu para arquivos vizinhos.
    pub sibling_match: Option<SiblingMatch>,
    /// Decisão humana já registrada para esta pasta.
    pub escopo: Option<ScopeRecord>,
}

#[derive(Debug, Deserialize)]
pub struct ReviewQuery {
    /// Qualquer `match_state`, sozinho ou separado por vírgula. `needs_review`
    /// é o padrão.
    ///
    /// **Não há lista branca aqui, e é de propósito.** `auto,confirmed` é a
    /// consulta da conferência: identificar era porta de mão única — no
    /// instante em que a obra saía de `needs_review` ela sumia da tela e não
    /// havia filtro que a trouxesse de volta. Quem identificou errado, ou quem
    /// quer conferir o que o matcher decidiu sozinho, entra por aqui.
    pub state: Option<String>,
    pub library: Option<Uuid>,
    /// Prefixo de caminho — fatia a fila por pasta.
    pub dir: Option<String>,
    pub kind: Option<String>,
    /// Separa dois problemas que a fila hoje confunde: "o matcher achou opções
    /// e não sabe qual" de "o matcher não achou nada". São filas diferentes,
    /// com ações diferentes — escolher versus corrigir o nome.
    pub has_candidates: Option<bool>,
    /// Substring do nome do arquivo ou do título entendido.
    pub q: Option<String>,
    /// `confidence` (padrão) põe primeiro o que está mais perto do limiar, que
    /// é o mais barato de resolver.
    pub sort: Option<String>,
    #[serde(default = "cinquenta")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn cinquenta() -> i64 {
    50
}

#[derive(Debug, Deserialize)]
pub struct RepairParams {
    #[serde(default = "sim")]
    pub dry_run: bool,
    /// Devolve pra fila as obras cujo episódio o provider diz não existir.
    ///
    /// Separado do reparo de título de propósito: consertar um texto é
    /// inofensivo, desfazer uma identificação não é. Quem chama tem que pedir.
    #[serde(default)]
    pub requeue: bool,
}

#[derive(Debug, Deserialize)]
pub struct ScopeSearch {
    pub dir_path: String,
    /// Sobrescreve o título sugerido pela pasta.
    pub query: Option<String>,
    /// `auto` (padrão), `tmdb` ou `anilist`.
    pub provider: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ScopeIdentify {
    pub library_id: Uuid,
    pub dir_path: String,
    /// Vale para a subárvore — série com pastas de temporada dentro.
    #[serde(default)]
    pub recursive: bool,

    pub provider: String,
    pub provider_id: String,
    pub provider_kind: String,

    pub season_number: Option<i32>,
    #[serde(default = "sazonal")]
    pub numbering: String,
    #[serde(default)]
    pub absolute_offset: i32,
    pub note: Option<String>,

    /// Padrão `true`. Uma operação que escreve centenas de obras mostra o que
    /// vai fazer antes de fazer — §8b aplicado à escala.
    #[serde(default = "sim")]
    pub dry_run: bool,

    /// Inclui o que já está `auto`/`confirmed`.
    ///
    /// Desligado por padrão porque `confirmed` é decisão humana e a máquina não
    /// a desfaz (§8b). Existe para o caso legítimo de REDECIDIR a pasta — a
    /// série escolhida estava errada, ou a numeração era outra — e aí quem
    /// desfaz é a mesma pessoa que decidiu.
    #[serde(default)]
    pub force: bool,
}

fn sazonal() -> String {
    "seasonal".to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct SiblingMatch {
    pub provider: String,
    pub provider_id: String,
    pub titulo: String,
    pub obras: i64,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ScopeRecord {
    pub id: Uuid,
    pub library_id: Uuid,
    pub dir_path: String,
    pub recursive: bool,
    pub provider: String,
    pub provider_id: String,
    pub provider_kind: String,
    pub season_number: Option<i32>,
    pub numbering: String,
    pub absolute_offset: i32,
    pub note: Option<String>,
    pub decided_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct ManualSearch {
    pub query: String,
    pub year: Option<i32>,
    /// tmdb | anilist | auto
    #[serde(default = "default_provider")]
    pub provider: String,
}

fn default_provider() -> String {
    "auto".to_string()
}

// ------------------------------------------------------------------ M2: grafo

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TagRow {
    pub id: Uuid,
    pub namespace: String,
    pub value: String,
    pub color: Option<String>,
    /// Quantas obras carregam esta tag — pra UI ordenar e esconder as órfãs.
    pub work_count: i64,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct WorkTag {
    pub id: Uuid,
    pub namespace: String,
    pub value: String,
    pub color: Option<String>,
    /// manual | provider | inferred — quem colocou a tag aqui.
    pub source: String,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TagNamespace {
    pub namespace: String,
    pub label: String,
    pub color: Option<String>,
    pub position: i32,
}

#[derive(Debug, Deserialize)]
pub struct AttachTag {
    pub namespace: String,
    pub value: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CollectionRow {
    pub id: Uuid,
    pub kind: String,
    pub parent_id: Option<Uuid>,
    pub title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub description: Option<String>,
    pub position: Option<i32>,
    pub origin: String,
    pub provider_key: Option<String>,
    /// Arte da própria coleção, servida em `/artwork/...` (R63).
    ///
    /// Existe na tabela desde o M1 e não saía daqui. Numa temporada é o pôster
    /// dela; numa série, o da série. `None` enquanto o job de temporadas não
    /// passou — e aí o cliente cai no `still` do primeiro episódio, que é o
    /// que ele já faz.
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub dominant_color: Option<String>,
    /// Obras na **subárvore inteira**, não só filhos diretos.
    ///
    /// Contava só o nível de baixo, e como episódio mora na temporada, toda
    /// série aparecia como "0" na tela — com 70 episódios embaixo.
    pub item_count: i64,
    /// Quantas da subárvore **quem perguntou** já terminou (R63). É o que
    /// desenha a barra da temporada na ficha da série.
    pub finished_count: i64,
    /// Até quatro pôsteres da subárvore, pra capa empilhada do cartão.
    pub posters: Option<Vec<String>>,
}

/// Coleção com filhos — o grafo recursivo materializado pra UI.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionNode {
    #[serde(flatten)]
    pub collection: CollectionRow,
    pub children: Vec<CollectionNode>,
}

#[derive(Debug, Deserialize)]
pub struct NewCollection {
    pub kind: String,
    pub title: String,
    pub parent_id: Option<Uuid>,
    pub description: Option<String>,
    pub year: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateCollection {
    pub title: Option<String>,
    pub description: Option<String>,
    pub year: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct AddItem {
    pub work_id: Uuid,
    pub position: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct OrderEntry {
    pub work_id: Uuid,
    pub position: i32,
}

#[derive(Debug, Deserialize)]
pub struct ReorderItems {
    pub items: Vec<OrderEntry>,
}

/// Uma aresta obra↔obra, já resolvida com o título do outro lado.
/// `direction` diz se esta obra é a origem (`out`) ou o destino (`in`) —
/// "é sequência de" e "tem sequência" são a mesma linha vista de dois lados.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RelationRow {
    pub kind: String,
    pub label: Option<String>,
    pub position: Option<i32>,
    pub direction: String,
    pub other_id: Uuid,
    pub other_title: String,
    pub other_year: Option<i32>,
    pub other_poster: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NewRelation {
    pub to_work: Uuid,
    pub kind: String,
    pub label: Option<String>,
    pub position: Option<i32>,
}

#[derive(Debug, Deserialize, Default)]
pub struct MatchRequest {
    /// Refaz até o que já foi casado automaticamente. Nunca toca no confirmado.
    #[serde(default)]
    pub force: bool,
    /// **O alvo da rodada** — R74. `novas` (padrão), `pendentes` ou `tudo`.
    ///
    /// Existia só o interruptor `force`, e ele pulava de um extremo ao outro:
    ///
    /// | | o que entra |
    /// |---|---|
    /// | `force = false` | só `unmatched` |
    /// | `force = true` | `unmatched` + `needs_review` + **`auto`** |
    ///
    /// Faltava o meio, e é justamente ele que uma melhoria de parser ou de
    /// score pede: **o que está pendente**. Quando o `guess` passou a ler
    /// "Episódio" como não-título (R71) e o desempate por temporada entrou
    /// (R72), as 3.373 obras em revisão deixaram de estar certas — e nenhuma
    /// delas é alcançada por `force = false`. A saída era `force = true`, que
    /// refaz de quebra 4.689 identificações que já estavam boas: milhares de
    /// chamadas ao provider e de artes rebaixadas pra reconfirmar o que
    /// ninguém questionou.
    ///
    /// ⚠️ **Nenhum dos três toca `confirmed` nem `ignored`.** Decisão humana
    /// não é refeita pela máquina — é a regra do §8b, e ela não tem exceção
    /// aqui. `alvo=tudo` é o teto, e o teto para antes das pessoas.
    #[serde(default)]
    pub alvo: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReparseParams {
    /// Padrão `true`: uma operação que reescreve milhares de linhas mostra o
    /// que vai fazer antes de fazer. Escrever exige `?dry_run=false` explícito.
    #[serde(default = "sim")]
    pub dry_run: bool,
}

fn sim() -> bool {
    true
}
