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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl EnrichmentNeedFilter {
    pub const ALL: Self = Self {
        store: true,
        reviews: true,
        review_excerpts: true,
        ccu: true,
        price: true,
        media_backfill: true,
    };

    pub fn any(self) -> bool {
        self.store
            || self.reviews
            || self.review_excerpts
            || self.ccu
            || self.price
            || self.media_backfill
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
    pub fn needs_any(self) -> bool {
        self.needs_store_details
            || self.needs_reviews
            || self.needs_review_excerpts
            || self.needs_ccu
            || self.needs_price
            || self.needs_media_backfill
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
    }

    pub fn matches_filter(self, filter: EnrichmentNeedFilter) -> bool {
        (filter.store && self.needs_store_details)
            || (filter.reviews && self.needs_reviews)
            || (filter.review_excerpts && self.needs_review_excerpts)
            || (filter.ccu && self.needs_ccu)
            || (filter.price && self.needs_price)
            || (filter.media_backfill && self.needs_media_backfill)
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
