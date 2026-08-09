use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppRecord {
    pub app_id: u32,
    pub app_type: String,
    pub canonical_name: String,
    pub release_state: String,
    pub release_date: Option<String>,
    pub release_date_raw: Option<String>,
    pub release_date_precision: Option<String>,
    pub is_early_access: Option<bool>,
    pub current_data_confidence: Option<f64>,
    pub source_modified_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiplayerProfile {
    pub app_id: u32,
    pub dominant_mode: Option<String>,
    pub private_session: Option<bool>,
    pub online_coop: Option<bool>,
    pub self_hosted_server: Option<bool>,
    pub drop_in_out: Option<bool>,
    pub crossplay: Option<bool>,
    pub recommended_min_players: Option<i64>,
    pub recommended_max_players: Option<i64>,
    pub profile_confidence: Option<f64>,
    pub computed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurationOverride {
    pub override_id: i64,
    pub app_id: u32,
    pub feature_name: String,
    pub value_json: String,
    pub reason: String,
    pub external_evidence: Option<String>,
    pub operator: String,
    pub created_at_ms: i64,
    pub revoked_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateOverrideRequest {
    pub feature_name: String,
    pub value_json: serde_json::Value,
    pub reason: String,
    pub external_evidence: Option<String>,
    pub operator: String,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRecord {
    pub job_id: i64,
    pub source: String,
    pub task_type: String,
    pub entity_key: String,
    pub priority: i64,
    pub attempts: i64,
    pub max_attempts: i64,
    pub due_at_ms: i64,
    pub status: String,
    pub lease_owner: Option<String>,
    pub lease_expires_at_ms: Option<i64>,
    pub idempotency_key: String,
    pub completion_idempotency_key: Option<String>,
    pub payload_json: Option<String>,
    pub last_error_category: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnqueueJob {
    pub source: String,
    pub task_type: String,
    pub entity_key: String,
    pub priority: i64,
    pub due_at_ms: i64,
    pub idempotency_key: String,
    pub payload_json: Option<String>,
    pub max_attempts: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectiveFeatureValue {
    pub app_id: u32,
    pub feature_name: String,
    pub value_json: serde_json::Value,
    pub origin: FeatureOrigin,
    pub override_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureOrigin {
    HumanOverride,
    SourceEvidence,
    Missing,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct M3CatalogCoverage {
    pub normalized_multiplayer_candidates: i64,
    pub category_evidence_candidates: i64,
    pub recommendation_ready_profiles: i64,
    pub trusted_familiar_profiles: i64,
    pub with_platforms: i64,
    pub with_languages: i64,
    pub with_typical_session: i64,
    pub with_price: i64,
    pub with_reviews: i64,
    pub with_ccu: i64,
}

/// Release-gate coverage for the M7 real-data requirements.
///
/// The section counts use the active recommendation configuration and the
/// same eligibility rules as the public feed before per-user hard filters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct M7DataCoverage {
    pub normalized_multiplayer_candidates: i64,
    pub trusted_friend_multiplayer_profiles: i64,
    pub candidates_with_date: i64,
    pub candidates_with_cover: i64,
    pub upcoming_candidates: i64,
    pub recent_release_candidates: i64,
    pub popular_legacy_candidates: i64,
    pub classic_legacy_candidates: i64,
    pub trusted_profiles_with_seven_day_reviews: i64,
    pub trusted_profiles_with_seven_day_ccu: i64,
}

/// Multiplayer catalog row still missing automated enrichment dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichmentTarget {
    pub app_id: u32,
    pub needs_store_details: bool,
    pub needs_reviews: bool,
    pub needs_review_excerpts: bool,
    pub needs_ccu: bool,
    pub needs_price: bool,
    /// Missing gallery media and eligible for a bounded store re-fetch.
    pub needs_media_backfill: bool,
    /// Missing English display name for dual-name search.
    pub needs_english_name: bool,
}

/// Which enrichment dimensions should be selected and prioritized.
///
/// Used so store-only / skip-* passes do not LIMIT away the intended work when
/// other dimensions (for example CCU) rank higher in the default ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnrichmentNeedFilter {
    pub store: bool,
    pub reviews: bool,
    pub review_excerpts: bool,
    pub ccu: bool,
    pub price: bool,
    pub media_backfill: bool,
    pub english_name: bool,
}

impl EnrichmentNeedFilter {
    pub const ALL: Self = Self {
        store: true,
        reviews: true,
        review_excerpts: true,
        ccu: true,
        price: true,
        media_backfill: true,
        english_name: true,
    };

    /// Store/reviews/CCU/price only — used by classic `list_enrichment_targets`.
    pub const CLASSIC: Self = Self {
        store: true,
        reviews: true,
        review_excerpts: true,
        ccu: true,
        price: true,
        media_backfill: false,
        english_name: false,
    };

    pub fn any(self) -> bool {
        self.store
            || self.reviews
            || self.review_excerpts
            || self.ccu
            || self.price
            || self.media_backfill
            || self.english_name
    }
}

/// Policy for coverage-gated, attempt-limited media gallery backfill.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaBackfillPolicy {
    pub enabled: bool,
    /// When candidate media coverage is at or above this ratio, skip backfill.
    pub coverage_threshold: f64,
    pub max_attempts: u32,
    pub cooldown_ms: i64,
}

impl MediaBackfillPolicy {
    pub const DEFAULT: Self = Self {
        enabled: true,
        coverage_threshold: 0.95,
        max_attempts: 3,
        cooldown_ms: 6 * 60 * 60 * 1_000,
    };

    pub const DISABLED: Self = Self {
        enabled: false,
        coverage_threshold: 1.0,
        max_attempts: 0,
        cooldown_ms: i64::MAX,
    };
}

/// Snapshot of media gallery coverage among multiplayer enrichment candidates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MediaCoverageStats {
    pub candidate_apps: u32,
    pub apps_with_media: u32,
    pub coverage_ratio: f64,
}

impl EnrichmentTarget {
    pub fn filtered(mut self, filter: EnrichmentNeedFilter) -> Self {
        self.needs_store_details &= filter.store;
        self.needs_reviews &= filter.reviews;
        self.needs_review_excerpts &= filter.review_excerpts;
        self.needs_ccu &= filter.ccu;
        self.needs_price &= filter.price;
        self.needs_media_backfill &= filter.media_backfill;
        self.needs_english_name &= filter.english_name;
        self
    }

    pub fn needs_any(self) -> bool {
        self.needs_store_details
            || self.needs_reviews
            || self.needs_review_excerpts
            || self.needs_ccu
            || self.needs_price
            || self.needs_media_backfill
            || self.needs_english_name
    }

    pub fn needs_store_fetch(self) -> bool {
        self.needs_store_details || self.needs_price || self.needs_media_backfill
    }

    pub fn missing_count(self) -> u8 {
        u8::from(self.needs_store_details)
            + u8::from(self.needs_reviews)
            + u8::from(self.needs_review_excerpts)
            + u8::from(self.needs_ccu)
            + u8::from(self.needs_price)
            + u8::from(self.needs_media_backfill)
            + u8::from(self.needs_english_name)
    }

    pub fn matches_filter(self, filter: EnrichmentNeedFilter) -> bool {
        (filter.store && self.needs_store_details)
            || (filter.reviews && self.needs_reviews)
            || (filter.review_excerpts && self.needs_review_excerpts)
            || (filter.ccu && self.needs_ccu)
            || (filter.price && self.needs_price)
            || (filter.media_backfill && self.needs_media_backfill)
            || (filter.english_name && self.needs_english_name)
    }
}

/// Outcome recorded after a media backfill store attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaBackfillOutcome {
    /// At least one screenshot/movie row is present.
    Complete,
    /// Store succeeded but Steam returned no usable media; stop retrying.
    NoneAvailable,
    /// Transient failure; may retry until max attempts.
    Failed,
}

/// Observable state for each controlled M7 data-refresh task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataRefreshStatus {
    pub task_name: String,
    pub last_success_at_ms: Option<i64>,
    pub next_run_at_ms: Option<i64>,
    pub last_error_category: Option<String>,
    pub cursor_value: Option<String>,
    pub coverage_ratio: Option<f64>,
    pub updated_at_ms: i64,
}

/// Operator-facing inventory snapshot for the data pipeline dashboard.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PipelineInventory {
    pub apps_total: i64,
    pub multiplayer_profiles: i64,
    pub released_with_date: i64,
    pub released_last_14_days: i64,
    pub coming_soon_total: i64,
    pub coming_soon_dated: i64,
    pub unknown_named_stubs: i64,
    pub max_release_date: Option<String>,
    pub max_release_date_app_id: Option<u32>,
    pub max_release_date_name: Option<String>,
    pub jobs_pending: i64,
    pub jobs_leased: i64,
    pub jobs_dead: i64,
    pub jobs_dead_recent: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineDimensionCoverage {
    pub candidates: i64,
    pub store_details: i64,
    pub release_date: i64,
    pub reviews: i64,
    pub ccu: i64,
    pub price: i64,
    pub languages: i64,
    pub retrieval_index: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineRunStatus {
    pub task_type: String,
    pub status: String,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub request_count: i64,
    pub success_count: i64,
    pub error_category: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameIngestionQueueStatus {
    pub pending: i64,
    pub retry: i64,
    pub leased: i64,
    pub dead: i64,
    pub store_details: i64,
    pub review_summary: i64,
    pub popular_reviews: i64,
    pub ccu: i64,
    pub oldest_dead_at_ms: Option<i64>,
    pub dead_by_stage: Vec<GameIngestionDeadCount>,
    pub dead_by_category: Vec<GameIngestionDeadCount>,
    pub recent_dead: Vec<GameIngestionDeadSample>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameIngestionDeadCount {
    pub key: String,
    pub count: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameIngestionDeadSample {
    pub app_id: u32,
    pub stage: String,
    pub error_category: Option<String>,
    pub error_summary: Option<String>,
    pub dead_at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PipelineStatusSnapshot {
    pub generated_at_ms: i64,
    pub inventory: PipelineInventory,
    pub coverage: M3CatalogCoverage,
    pub m7_coverage: M7DataCoverage,
    pub dimension_coverage: PipelineDimensionCoverage,
    pub latest_runs: Vec<PipelineRunStatus>,
    #[serde(default)]
    pub integrated_ingestion: GameIngestionQueueStatus,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSearchIngestOutcome {
    pub ingested_candidates: usize,
    pub queued_new_app_ids: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameIngestionTask {
    pub app_id: u32,
    pub source: String,
    pub priority: i64,
    pub stage: String,
    pub status: String,
    pub stage_failure_attempts: i64,
    pub total_failure_attempts: i64,
    pub lease_count: i64,
    pub next_attempt_at_ms: i64,
    pub last_error_category: Option<String>,
    pub last_error_summary: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_expires_at_ms: Option<i64>,
    pub enrichment_profile: String,
    pub profile_version: i64,
    pub dead_at_ms: Option<i64>,
}
