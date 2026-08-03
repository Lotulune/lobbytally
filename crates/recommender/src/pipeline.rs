use mpgs_domain::{
    CandidateAvailability, FeedSection, RankingSignals, RecommendationConfig, SteamAppId,
    UserPreferences,
};
use serde::{Deserialize, Serialize};

use crate::ALGORITHM_VERSION;
use crate::ScoreBreakdown;
use crate::explain::{Explanation, explain};
use crate::mmr::mmr_rerank_with_tie_seed;
use crate::personalize::{
    HardConstraints, apply_personalization_with_constraints, hard_filter_with_constraints,
};
use crate::score;

#[derive(Debug, Clone, PartialEq)]
pub struct RankingInput {
    pub app_id: SteamAppId,
    pub name: String,
    pub dominant_mode: Option<String>,
    pub taxonomy_tags: Vec<String>,
    pub publisher: Option<String>,
    pub series: Option<String>,
    pub recommended_min: Option<u8>,
    pub recommended_max: Option<u8>,
    pub availability: CandidateAvailability,
    pub signals: RankingSignals,
    pub personal_adjustment: f64,
    /// Community play-intent votes; boosts the final score (0 = no effect).
    pub play_intent_count: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotReason {
    #[default]
    Base,
    Diversity,
    Explore,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankedCandidate {
    pub app_id: SteamAppId,
    pub name: String,
    pub dominant_mode: Option<String>,
    #[serde(default)]
    pub taxonomy_tags: Vec<String>,
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub series: Option<String>,
    pub recommended_min: Option<u8>,
    pub recommended_max: Option<u8>,
    #[serde(default)]
    pub data_confidence: f64,
    pub score: ScoreBreakdown,
    pub explanation: Explanation,
    #[serde(default)]
    pub slot_reason: SlotReason,
    pub algorithm_version: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RankedFeed {
    pub section: FeedSection,
    pub algorithm_version: String,
    pub items: Vec<RankedCandidate>,
}

pub fn rank_feed(
    section: FeedSection,
    candidates: &[RankingInput],
    prefs: &UserPreferences,
    mmr_lambda: Option<f64>,
) -> RankedFeed {
    rank_feed_with_constraints(
        section,
        candidates,
        prefs,
        &HardConstraints::NONE,
        mmr_lambda,
    )
}

/// Rank with request-scoped hard constraints. Stored profile preferences remain
/// soft when the corresponding mask field is false.
pub fn rank_feed_with_constraints(
    section: FeedSection,
    candidates: &[RankingInput],
    prefs: &UserPreferences,
    constraints: &HardConstraints,
    mmr_lambda: Option<f64>,
) -> RankedFeed {
    let defaults = RecommendationConfig::default();
    rank_feed_inner(
        section,
        candidates,
        prefs,
        constraints,
        mmr_lambda.unwrap_or(defaults.mmr_lambda),
        defaults.effective_play_intent_weight(),
        defaults.play_intent_saturation,
        0,
        ALGORITHM_VERSION,
    )
}

pub fn rank_feed_configured(
    section: FeedSection,
    candidates: &[RankingInput],
    prefs: &UserPreferences,
    config: &RecommendationConfig,
    algorithm_version: &str,
) -> RankedFeed {
    rank_feed_configured_with_constraints(
        section,
        candidates,
        prefs,
        &HardConstraints::NONE,
        config,
        algorithm_version,
    )
}

/// Configured ranking variant for callers that have distinguished explicit
/// query constraints from long-term profile preferences.
pub fn rank_feed_configured_with_constraints(
    section: FeedSection,
    candidates: &[RankingInput],
    prefs: &UserPreferences,
    constraints: &HardConstraints,
    config: &RecommendationConfig,
    algorithm_version: &str,
) -> RankedFeed {
    rank_feed_configured_with_constraints_and_tie_seed(
        section,
        candidates,
        prefs,
        constraints,
        config,
        algorithm_version,
        0,
    )
}

/// Configured player-facing ranking with an explicit stable tie seed. The
/// caller owns seed derivation so account identifiers never enter this crate.
pub fn rank_feed_configured_with_constraints_and_tie_seed(
    section: FeedSection,
    candidates: &[RankingInput],
    prefs: &UserPreferences,
    constraints: &HardConstraints,
    config: &RecommendationConfig,
    algorithm_version: &str,
    tie_seed: u64,
) -> RankedFeed {
    rank_feed_inner(
        section,
        candidates,
        prefs,
        constraints,
        config.mmr_lambda,
        config.effective_play_intent_weight(),
        config.play_intent_saturation,
        tie_seed,
        algorithm_version,
    )
}

/// Conservative community play-intent lift. Five or fewer distinct voters are
/// treated as insufficient evidence; later votes approach, but never exceed, a
/// three-point lift. A zero legacy weight/saturation still disables the signal.
fn play_intent_boost(count: u32, weight: f64, saturation: u32) -> f64 {
    const MIN_VOTERS: u32 = 5;
    const HALF_LIFT_EXCESS_VOTERS: f64 = 20.0;
    const MAX_LIFT: f64 = 0.03;

    if count <= MIN_VOTERS || saturation == 0 || weight <= 0.0 {
        return 0.0;
    }
    let excess = f64::from(count - MIN_VOTERS);
    let norm = excess / (excess + HALF_LIFT_EXCESS_VOTERS);
    crate::unit(weight).min(MAX_LIFT) * norm
}

#[allow(clippy::too_many_arguments)]
fn rank_feed_inner(
    section: FeedSection,
    candidates: &[RankingInput],
    prefs: &UserPreferences,
    constraints: &HardConstraints,
    mmr_lambda: f64,
    play_intent_weight: f64,
    play_intent_saturation: u32,
    tie_seed: u64,
    algorithm_version: &str,
) -> RankedFeed {
    let mut scored = Vec::new();
    for candidate in candidates {
        if !hard_filter_with_constraints(
            prefs,
            candidate.recommended_min,
            candidate.recommended_max,
            candidate.dominant_mode.as_deref(),
            &candidate.signals,
            &candidate.availability,
            constraints,
        ) {
            continue;
        }
        let mut signals = candidate.signals;
        apply_personalization_with_constraints(
            prefs,
            &mut signals,
            candidate.recommended_min,
            candidate.recommended_max,
            &candidate.availability,
            constraints,
        );
        signals.personal_fit =
            crate::unit(signals.personal_fit + candidate.personal_adjustment.clamp(-0.5, 0.5));
        let mut breakdown = score(section, &signals, None);
        // Community demand nudges the ranked score upward, after deterministic
        // scoring and before diversity re-ranking.
        let boost = play_intent_boost(
            candidate.play_intent_count,
            play_intent_weight,
            play_intent_saturation,
        );
        if boost > 0.0 {
            breakdown.relevance_score += boost;
            breakdown.final_score = crate::unit(breakdown.relevance_score);
        }
        let explanation = explain(
            candidate.app_id,
            &signals,
            &breakdown,
            candidate.dominant_mode.as_deref(),
        );
        scored.push(RankedCandidate {
            app_id: candidate.app_id,
            name: candidate.name.clone(),
            dominant_mode: candidate.dominant_mode.clone(),
            taxonomy_tags: candidate.taxonomy_tags.clone(),
            publisher: candidate.publisher.clone(),
            series: candidate.series.clone(),
            recommended_min: candidate.recommended_min,
            recommended_max: candidate.recommended_max,
            data_confidence: crate::unit(signals.data_confidence),
            score: breakdown,
            explanation,
            slot_reason: SlotReason::Base,
            algorithm_version: algorithm_version.to_owned(),
        });
    }

    let items = mmr_rerank_with_tie_seed(scored, mmr_lambda, 2, tie_seed);
    RankedFeed {
        section,
        algorithm_version: algorithm_version.to_owned(),
        items,
    }
}
