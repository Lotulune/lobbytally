use crate::recommendation::DemoStatus;
use serde::{ser::SerializeStruct, Deserialize, Serialize, Serializer};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicConfig {
    pub steam_api_key_configured: bool,
    pub steam_api_key_validated: bool,
    pub llm_api_key_configured: bool,
    pub llm_config_validated: bool,
    pub llm_provider: LlmProvider,
    pub llm_base_url: String,
    pub llm_model: String,
    pub country: String,
    pub language: String,
    pub ai_batch_refresh_concurrency: u8,
    pub onboarding_completed: bool,
    pub onboarding_current_step: u8,
    pub onboarding_llm_provider_draft: LlmProvider,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveConfigRequest {
    pub steam_api_key: Option<String>,
    pub steam_api_key_validated: Option<bool>,
    pub clear_steam_api_key: Option<bool>,
    pub llm_api_key: Option<String>,
    pub llm_config_validated: Option<bool>,
    pub clear_llm_api_key: Option<bool>,
    pub llm_provider: Option<LlmProvider>,
    pub llm_base_url: Option<String>,
    pub llm_model: Option<String>,
    pub country: Option<String>,
    pub language: Option<String>,
    pub ai_batch_refresh_concurrency: Option<u8>,
    pub onboarding_completed: Option<bool>,
    pub onboarding_current_step: Option<u8>,
    pub onboarding_llm_provider_draft: Option<LlmProvider>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    Deepseek,
    Openai,
    Anthropic,
    Custom,
}

impl Default for LlmProvider {
    fn default() -> Self {
        Self::Deepseek
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidateSteamConfigRequest {
    pub steam_api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidateLlmConfigRequest {
    pub provider: LlmProvider,
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionValidationResult {
    pub success: bool,
    pub message: String,
    pub diagnostic: Option<String>,
    pub latency_ms: Option<u64>,
    pub provider: Option<LlmProvider>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub app_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardPayload {
    pub new_games: Vec<GameCard>,
    pub classics: Vec<GameCard>,
    pub hidden_games: Vec<GameCard>,
    pub upcoming: Vec<GameCard>,
    pub recent_discoveries: Vec<GameCard>,
    pub collections: UserCollections,
    pub ai_analysis_queue_failures: Vec<AiAnalysisQueueFailureItem>,
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
    pub classic_discovery_running: bool,
    pub classic_discovery_status: Option<DiscoveryRunStatus>,
    pub classic_discovery_current_appid: Option<u32>,
    pub classic_discovery_last_appid: Option<u32>,
    pub classic_discovery_scanned_apps: usize,
    pub classic_discovery_added_games: usize,
    pub classic_discovery_rejected_games: usize,
    pub classic_discovery_failed_games: usize,
    pub classic_discovery_skipped_existing: usize,
    pub classic_discovery_skipped_rejected_cache: usize,
    pub classic_discovery_last_completed_at: Option<String>,
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
    pub ai_batch_refresh_running: bool,
    pub ai_batch_refresh_concurrency: u8,
    pub ai_batch_refresh_pending_count: usize,
    pub ai_batch_refresh_active_count: usize,
    pub ai_batch_refresh_total_count: usize,
    pub ai_batch_refresh_processed_count: usize,
    pub ai_batch_refresh_updated_count: usize,
    pub ai_batch_refresh_failed_count: usize,
    pub ai_batch_refresh_failed_pending_review_count: usize,
    pub ai_batch_refresh_last_error: Option<String>,
    pub ai_batch_refresh_last_error_appid: Option<u32>,
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
    pub is_free: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiBatchRefreshReport {
    pub total_games: usize,
    pub updated_games: usize,
    pub failed_games: usize,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiAnalysisQueueSource {
    NewRelease,
    Classic,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRecommendationMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRecommendationRequest {
    pub prompt: String,
    #[serde(default)]
    pub context_messages: Vec<AiRecommendationMessage>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRecommendedGame {
    pub game: GameCard,
    pub match_score: f64,
    pub reason: String,
    pub matched_traits: Vec<String>,
    pub missing_traits: Vec<String>,
    pub caveats: Vec<String>,
    pub exact_match: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRecommendationResponse {
    pub reply: String,
    pub follow_up_question: Option<String>,
    pub exact_match_count: usize,
    pub source: AnalysisSource,
    pub llm_used: bool,
    pub diagnostic: Option<String>,
    pub items: Vec<AiRecommendedGame>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisSource {
    Hybrid,
    Rule,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisEvidenceKind {
    PositiveReviewPct,
    TotalReviews,
    CurrentPlayers,
    Tags,
    MultiplayerModes,
    ShortDescription,
    ReviewSnippet,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisReviewStance {
    Strength,
    Risk,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationPool {
    NewRelease,
    Evergreen,
    HiddenGem,
    FriendsParty,
    DemoPotential,
}

impl Default for RecommendationPool {
    fn default() -> Self {
        Self::Evergreen
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisDimensionScore {
    pub key: String,
    pub label: String,
    pub score: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisPoint {
    pub title: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisEvidenceItem {
    pub kind: AnalysisEvidenceKind,
    pub label: String,
    pub value: String,
    pub interpretation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisReviewEvidenceItem {
    pub stance: AnalysisReviewStance,
    pub quote: String,
    pub playtime_text: String,
    pub interpretation: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisRiskFlag {
    pub key: String,
    pub label: String,
    pub severity: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameAnalysisReport {
    pub appid: u32,
    pub generated_at: String,
    pub source: AnalysisSource,
    pub confidence: AnalysisConfidence,
    #[serde(default = "default_score_version")]
    pub score_version: String,
    #[serde(default)]
    pub quality_score: f64,
    #[serde(default)]
    pub recommendation_score: f64,
    #[serde(default)]
    pub confidence_score: f64,
    #[serde(default)]
    pub pool_type: RecommendationPool,
    #[serde(default)]
    pub risk_flags: Vec<AnalysisRiskFlag>,
    pub overall_score: f64,
    pub overview: String,
    pub dimension_scores: Vec<AnalysisDimensionScore>,
    pub strengths: Vec<AnalysisPoint>,
    pub risks: Vec<AnalysisPoint>,
    pub evidence: Vec<AnalysisEvidenceItem>,
    pub review_evidence: Vec<AnalysisReviewEvidenceItem>,
}

fn default_score_version() -> String {
    "v1_compat".to_string()
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryCompletionReason {
    TargetReached,
    PageBudgetReached,
    NoMoreResults,
    Paused,
    Cancelled,
    Failed,
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
pub struct ClassicDiscoveryTaskRequest {
    pub max_pages: Option<u32>,
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
    pub completion_reason: Option<DiscoveryCompletionReason>,
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
        let mut state = serializer.serialize_struct("DiscoveryRunSnapshot", 23)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("status", &self.status)?;
        state.serialize_field("completionReason", &self.completion_reason)?;
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClassicRejectReasonCode {
    NonMultiplayer,
    NotReleased,
    TooNew,
    LowReviewCount,
    LowPositiveReviewPct,
    LowCurrentPlayers,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassicDiscoveryRejectCacheEntry {
    pub appid: u32,
    pub reason_code: ClassicRejectReasonCode,
    pub positive_review_pct: Option<f64>,
    pub total_reviews: Option<u32>,
    pub current_players: Option<u32>,
    pub release_state: StoreReleaseState,
    pub release_date: Option<String>,
    pub checked_at: String,
    pub rule_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassicDiscoveryRunSnapshot {
    pub id: i64,
    pub status: DiscoveryRunStatus,
    pub max_pages: u32,
    pub page_size: u32,
    pub pages_processed: u32,
    pub scanned_apps: usize,
    pub considered_apps: usize,
    pub added_games: usize,
    pub rejected_games: usize,
    pub skipped_existing: usize,
    pub skipped_rejected_cache: usize,
    pub failed_games: usize,
    pub current_appid: Option<u32>,
    pub last_appid: Option<u32>,
    pub consecutive_empty_pages: u32,
    pub rule_version: String,
    pub started_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
    pub last_error: Option<String>,
}

impl ClassicDiscoveryRunSnapshot {
    pub fn can_resume(&self) -> bool {
        matches!(
            self.status,
            DiscoveryRunStatus::Paused | DiscoveryRunStatus::Interrupted
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAnalysisQueueFailureItem {
    pub appid: u32,
    pub attempt: u8,
    pub last_error: String,
    pub updated_at: String,
}
