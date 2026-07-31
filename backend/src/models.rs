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
    /// Caminho relativo servido em `/artwork/...`; None enquanto não identificado.
    pub poster: Option<String>,
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
    pub filename: String,
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
    pub item_count: i64,
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
}
