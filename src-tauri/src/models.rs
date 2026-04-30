use crate::recommendation::DemoStatus;
use serde::{ser::SerializeStruct, Deserialize, Serialize, Serializer};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicConfig {
    pub steam_api_key_configured: bool,
    pub llm_api_key_configured: bool,
    pub llm_base_url: String,
    pub llm_model: String,
    pub country: String,
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveConfigRequest {
    pub steam_api_key: Option<String>,
    pub llm_api_key: Option<String>,
    pub llm_base_url: Option<String>,
    pub llm_model: Option<String>,
    pub country: Option<String>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardPayload {
    pub new_games: Vec<GameCard>,
    pub classics: Vec<GameCard>,
    pub upcoming: Vec<GameCard>,
    pub recent_discoveries: Vec<GameCard>,
    pub collections: UserCollections,
    pub stats: DashboardStats,
    pub config: PublicConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardStats {
    pub last_sync_at: Option<String>,
    pub seed_count: usize,
    pub total_games: usize,
    pub new_games_count: usize,
    pub classic_games_count: usize,
    pub last_discovery_appid: Option<u32>,
    pub sync_running: bool,
    pub sync_mode: Option<SyncMode>,
    pub sync_pending_count: usize,
    pub sync_current_appid: Option<u32>,
    pub sync_total_count: usize,
    pub sync_processed_count: usize,
    pub sync_updated_count: usize,
    pub sync_failed_count: usize,
    pub sync_last_error: Option<String>,
    pub sync_last_error_appid: Option<u32>,
    pub backfill_pending_count: usize,
    pub backfill_running: bool,
    pub backfill_current_appid: Option<u32>,
    pub backfill_current_attempt: Option<u8>,
    pub backfill_total_count: usize,
    pub backfill_processed_count: usize,
    pub backfill_failed_count: usize,
    pub backfill_max_attempts: u8,
    pub backfill_last_error: Option<String>,
    pub backfill_last_error_appid: Option<u32>,
    pub data_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameCard {
    pub appid: u32,
    pub name: String,
    pub short_description: Option<String>,
    pub section: String,
    pub release_date: Option<String>,
    pub release_date_text: String,
    pub release_state: StoreReleaseState,
    pub demo_status: DemoStatus,
    pub supported_languages: Vec<String>,
    pub is_adult_content: bool,
    pub price_text: Option<String>,
    pub discount_percent: Option<u32>,
    pub positive_review_pct: Option<f64>,
    pub total_reviews: Option<u32>,
    pub current_players: Option<u32>,
    pub recommendation_score: f64,
    pub ai_score: Option<f64>,
    pub ai_summary: String,
    pub capsule_url: String,
    pub store_screenshot_urls: Vec<String>,
    pub tags: Vec<String>,
    pub multiplayer_modes: Vec<String>,
    pub review_snippets: Vec<ReviewSnippet>,
    pub user_state: UserGameState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StoreReleaseState {
    Upcoming,
    Released,
    Tba,
    Unknown,
}

impl Default for StoreReleaseState {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserGameState {
    pub favorite: bool,
    pub wishlist: bool,
    pub followed: bool,
    pub viewed: bool,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserGameStatePatch {
    pub favorite: Option<bool>,
    pub wishlist: Option<bool>,
    pub followed: Option<bool>,
    pub viewed: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserCollections {
    pub favorites: Vec<GameCard>,
    pub wishlist: Vec<GameCard>,
    pub followed: Vec<GameCard>,
    pub history: Vec<GameCard>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSnippet {
    pub voted_up: bool,
    pub review: String,
    pub playtime_hours: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub updated_games: usize,
    pub failed_games: usize,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    Quick,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRequest {
    pub mode: SyncMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAssessment {
    pub appid: u32,
    pub score: f64,
    pub summary: String,
    pub best_for: Vec<String>,
    pub risks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryRunStatus {
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryTaskRequest {
    pub sync_mode: SyncMode,
    pub target_added_games: u32,
    pub page_size: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryFailureItem {
    pub page_index: u32,
    pub appid: Option<u32>,
    pub stage: String,
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryRunSnapshot {
    pub id: i64,
    pub status: DiscoveryRunStatus,
    pub sync_mode: SyncMode,
    pub target_added_games: u32,
    pub page_size: u32,
    pub pages_processed: u32,
    pub scanned_apps: usize,
    pub added_games: usize,
    pub added_new_games: usize,
    pub added_classic_games: usize,
    pub skipped_existing: usize,
    pub skipped_non_multiplayer: usize,
    pub failed_games: usize,
    pub current_appid: Option<u32>,
    pub last_appid: Option<u32>,
    pub have_more_results: bool,
    pub started_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
    pub last_error: Option<String>,
    pub failures: Vec<DiscoveryFailureItem>,
}

impl Serialize for DiscoveryRunSnapshot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DiscoveryRunSnapshot", 22)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("status", &self.status)?;
        state.serialize_field("syncMode", &self.sync_mode)?;
        state.serialize_field("targetAddedGames", &self.target_added_games)?;
        state.serialize_field("pageSize", &self.page_size)?;
        state.serialize_field("pagesProcessed", &self.pages_processed)?;
        state.serialize_field("scannedApps", &self.scanned_apps)?;
        state.serialize_field("addedGames", &self.added_games)?;
        state.serialize_field("addedNewGames", &self.added_new_games)?;
        state.serialize_field("addedClassicGames", &self.added_classic_games)?;
        state.serialize_field("skippedExisting", &self.skipped_existing)?;
        state.serialize_field("skippedNonMultiplayer", &self.skipped_non_multiplayer)?;
        state.serialize_field("failedGames", &self.failed_games)?;
        state.serialize_field("currentAppid", &self.current_appid)?;
        state.serialize_field("lastAppid", &self.last_appid)?;
        state.serialize_field("haveMoreResults", &self.have_more_results)?;
        state.serialize_field("startedAt", &self.started_at)?;
        state.serialize_field("updatedAt", &self.updated_at)?;
        state.serialize_field("finishedAt", &self.finished_at)?;
        state.serialize_field("lastError", &self.last_error)?;
        state.serialize_field("failures", &self.failures)?;
        state.serialize_field("progressPercent", &self.progress_percent())?;
        state.end()
    }
}

impl DiscoveryRunSnapshot {
    pub fn progress_percent(&self) -> u32 {
        if self.target_added_games == 0 {
            return 0;
        }

        let ratio = self.added_games as f64 / self.target_added_games as f64;
        (ratio.min(1.0) * 100.0).round() as u32
    }

    pub fn can_resume(&self) -> bool {
        matches!(
            self.status,
            DiscoveryRunStatus::Paused | DiscoveryRunStatus::Interrupted
        )
    }
}
