#![forbid(unsafe_code)]

mod explain;
mod mmr;
mod personalize;
mod pipeline;

use mpgs_domain::{FeedSection, RankingSignals};
use serde::{Deserialize, Serialize};

pub use explain::{Explanation, explain};
pub use mmr::{mmr_rerank, mmr_rerank_with_tie_seed};
pub use mpgs_domain::friend_fit;
pub use personalize::{
    HardConstraints, apply_personalization, apply_personalization_with_constraints, hard_filter,
    hard_filter_with_constraints,
};
pub use pipeline::{
    RankedCandidate, RankingInput, SlotReason, rank_feed, rank_feed_configured,
    rank_feed_configured_with_constraints, rank_feed_configured_with_constraints_and_tie_seed,
    rank_feed_with_constraints,
};

pub const ALGORITHM_VERSION: &str = "rules-0.3.1";
const PERSONAL_WEIGHT: f64 = 0.45;
const AI_WEIGHT: f64 = 0.15;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AiAdjustment {
    pub fit: f64,
    pub confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub friend_fit: f64,
    pub section_score: f64,
    pub personalized_score: f64,
    #[serde(default)]
    pub group_fit: f64,
    #[serde(default)]
    pub mode_fit: f64,
    #[serde(default)]
    pub access_fit: f64,
    #[serde(default)]
    pub hosting_fit: f64,
    #[serde(default)]
    pub session_fit: f64,
    #[serde(default)]
    pub quality: f64,
    /// Confidence-shrunk activity level with a smaller trend contribution.
    #[serde(default)]
    pub activity: f64,
    #[serde(default)]
    pub freshness: f64,
    #[serde(default)]
    pub risk: f64,
    /// Continuous score used for ordering. Unlike the legacy display fields,
    /// this preserves negative risk-adjusted values instead of clipping ties at 0.
    #[serde(default)]
    pub relevance_score: f64,
    pub final_score: f64,
}

pub fn score(
    section: FeedSection,
    signals: &RankingSignals,
    ai: Option<AiAdjustment>,
) -> ScoreBreakdown {
    let friend_fit = friend_fit(&signals.multiplayer);
    let section_relevance = section_relevance(section, signals);
    let personalized_relevance = blend_personal_relevance(section_relevance, signals.personal_fit);
    let relevance_score = blend_ai_relevance(personalized_relevance, ai);

    ScoreBreakdown {
        friend_fit,
        section_score: unit(section_relevance),
        personalized_score: unit(personalized_relevance),
        group_fit: unit(signals.personal_components.group_fit),
        mode_fit: unit(signals.personal_components.mode_fit),
        access_fit: unit(signals.personal_components.access_fit),
        hosting_fit: unit(signals.personal_components.hosting_fit),
        session_fit: unit(signals.personal_components.session_fit),
        quality: unit(signals.quality),
        activity: unit(0.7 * unit(signals.popularity) + 0.3 * unit(signals.momentum)),
        freshness: unit(match section {
            FeedSection::Upcoming => signals.release_proximity,
            _ => signals.freshness,
        }),
        risk: unit(signals.risk),
        relevance_score,
        final_score: unit(relevance_score),
    }
}

pub fn section_score(section: FeedSection, signals: &RankingSignals, _friend_fit: f64) -> f64 {
    unit(section_relevance(section, signals))
}

/// Section-level, player-independent relevance. Positive weights within each
/// section sum to one; player fit enters once, later, through the 45% blend.
fn section_relevance(section: FeedSection, signals: &RankingSignals) -> f64 {
    let raw = match section {
        FeedSection::RecentRelease => {
            0.40 * unit(signals.quality)
                + 0.20 * unit(signals.popularity)
                + 0.15 * unit(signals.momentum)
                + 0.25 * unit(signals.freshness)
        }
        FeedSection::Upcoming => {
            0.35 * unit(signals.demo_playability)
                + 0.20 * unit(signals.release_date_confidence)
                + 0.25 * unit(signals.release_proximity)
                + 0.20 * unit(signals.studio_prior)
        }
        FeedSection::PopularLegacy => {
            0.40 * unit(signals.popularity)
                + 0.20 * unit(signals.momentum)
                + 0.30 * unit(signals.quality)
                + 0.10 * unit(signals.maintenance_health)
        }
        FeedSection::ClassicLegacy => {
            0.45 * unit(signals.quality)
                + 0.25 * unit(signals.longevity)
                + 0.15 * unit(signals.maintenance_health)
                + 0.15 * unit(signals.popularity)
        }
    };

    raw - 0.20 * unit(signals.risk)
}

pub fn blend_personal_fit(base: f64, personal_fit: f64) -> f64 {
    unit(blend_personal_relevance(unit(base), personal_fit))
}

fn blend_personal_relevance(base: f64, personal_fit: f64) -> f64 {
    (1.0 - PERSONAL_WEIGHT) * base + PERSONAL_WEIGHT * unit(personal_fit)
}

pub fn blend_ai(base: f64, ai: Option<AiAdjustment>) -> f64 {
    blend_ai_relevance(base, ai)
}

fn blend_ai_relevance(base: f64, ai: Option<AiAdjustment>) -> f64 {
    let Some(ai) = ai else {
        return base;
    };

    let confidence = unit(ai.confidence);
    let confidence_adjusted_target = confidence * unit(ai.fit) + (1.0 - confidence) * base;
    (1.0 - AI_WEIGHT) * base + AI_WEIGHT * confidence_adjusted_target
}

pub(crate) fn unit(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ALGORITHM_VERSION, AiAdjustment, HardConstraints, RankingInput, blend_ai, friend_fit,
        rank_feed, rank_feed_with_constraints, score,
    };
    use mpgs_domain::{
        CandidateAvailability, FeedSection, MultiplayerSignals, RankingSignals, SteamAppId,
        UserPreferences,
    };

    fn cooperative_signals() -> MultiplayerSignals {
        MultiplayerSignals {
            private_session: 1.0,
            self_host_or_dedicated: 0.8,
            online_coop: 1.0,
            group_size_fit: 1.0,
            low_public_population_dependency: 1.0,
            drop_in_out: 0.8,
            cross_platform_fit: 0.5,
            ..Default::default()
        }
    }

    fn matchmaking_signals() -> MultiplayerSignals {
        MultiplayerSignals {
            private_session: 0.2,
            online_coop: 0.0,
            group_size_fit: 0.8,
            low_public_population_dependency: 0.0,
            matchmaking_core: 1.0,
            public_world_dependency: 0.8,
            ..Default::default()
        }
    }

    fn ranking(app_id: SteamAppId, multiplayer: MultiplayerSignals) -> RankingInput {
        RankingInput {
            app_id,
            name: format!("app-{app_id}"),
            dominant_mode: None,
            taxonomy_tags: Vec::new(),
            publisher: None,
            series: None,
            recommended_min: Some(1),
            recommended_max: Some(4),
            availability: Default::default(),
            personal_adjustment: 0.0,
            play_intent_count: 0,
            signals: RankingSignals {
                multiplayer,
                quality: 0.85,
                popularity: 0.7,
                momentum: 0.5,
                evidence: 0.85,
                data_confidence: 0.9,
                longevity: 0.8,
                maintenance_health: 0.8,
                personal_fit: 0.5,
                ..Default::default()
            },
        }
    }

    #[test]
    fn private_coop_outranks_matchmaking_for_friend_fit() {
        assert!(friend_fit(&cooperative_signals()) > friend_fit(&matchmaking_signals()));
    }

    #[test]
    fn ai_adjustment_is_bounded() {
        let base = 0.2;
        let adjusted = blend_ai(
            base,
            Some(AiAdjustment {
                fit: 1.0,
                confidence: 1.0,
            }),
        );
        assert!((adjusted - base).abs() <= 0.15);

        let unbounded_base = -0.3;
        let adjusted = blend_ai(
            unbounded_base,
            Some(AiAdjustment {
                fit: 1.0,
                confidence: 1.0,
            }),
        );
        assert!((adjusted - (-0.105)).abs() < 1e-12);
        assert_eq!(blend_ai(1.4, None), 1.4);
    }

    #[test]
    fn invalid_signal_values_are_clamped() {
        let signals = RankingSignals {
            multiplayer: cooperative_signals(),
            quality: 10.0,
            popularity: f64::NAN,
            momentum: -5.0,
            evidence: 2.0,
            freshness: 2.0,
            data_confidence: 2.0,
            personal_fit: 5.0,
            ..Default::default()
        };
        let result = score(FeedSection::RecentRelease, &signals, None);
        assert!((0.0..=1.0).contains(&result.final_score));
    }

    #[test]
    fn relevance_score_preserves_information_beyond_legacy_display_bounds() {
        let signals = RankingSignals {
            risk: 1.0,
            personal_fit: 0.0,
            ..Default::default()
        };

        let result = score(FeedSection::ClassicLegacy, &signals, None);

        assert_eq!(result.final_score, 0.0);
        assert!(result.relevance_score < 0.0);
    }

    #[test]
    fn evidence_confidence_is_not_a_direct_popularity_bonus() {
        let low_confidence = RankingSignals {
            quality: 0.7,
            popularity: 0.6,
            momentum: 0.4,
            evidence: 0.0,
            data_confidence: 0.0,
            freshness: 0.5,
            longevity: 0.6,
            maintenance_health: 0.7,
            ..Default::default()
        };
        let high_confidence = RankingSignals {
            evidence: 1.0,
            data_confidence: 1.0,
            ..low_confidence
        };

        for section in [
            FeedSection::RecentRelease,
            FeedSection::PopularLegacy,
            FeedSection::ClassicLegacy,
        ] {
            let low = super::section_score(section, &low_confidence, 0.6);
            let high = super::section_score(section, &high_confidence, 0.6);
            assert_eq!(low, high, "{section:?} must not reward confidence itself");
        }
    }

    #[test]
    fn personal_fit_has_the_rules_0_3_seed_weight() {
        assert!((super::blend_personal_fit(0.0, 1.0) - 0.45).abs() < f64::EPSILON);
    }

    #[test]
    fn objective_section_score_does_not_double_count_friend_fit() {
        let common = RankingSignals {
            quality: 0.85,
            popularity: 0.85,
            momentum: 0.5,
            evidence: 0.8,
            data_confidence: 0.9,
            longevity: 0.8,
            maintenance_health: 0.8,
            personal_fit: 0.8,
            ..Default::default()
        };
        let cooperative = RankingSignals {
            multiplayer: cooperative_signals(),
            ..common
        };
        let matchmaking = RankingSignals {
            multiplayer: matchmaking_signals(),
            ..common
        };
        let cooperative_score = score(FeedSection::ClassicLegacy, &cooperative, None);
        let matchmaking_score = score(FeedSection::ClassicLegacy, &matchmaking, None);
        assert_eq!(
            cooperative_score.section_score,
            matchmaking_score.section_score
        );
        assert_eq!(cooperative_score.final_score, matchmaking_score.final_score);
    }

    #[test]
    fn prd_default_sort_coop_self_host_above_matchmaking_core() {
        // PRD: 帕鲁/方舟/深岩/雨中冒险2 熟人适配应高于 CS2 类匹配核心。
        let prefs = UserPreferences {
            // This assertion represents an explicitly confirmed co-op profile;
            // untouched onboarding defaults intentionally shrink to neutral.
            preference_confidence: 1.0,
            ..UserPreferences::default()
        };
        let coop_ids = [1623730u32, 346110, 548430, 632360]; // Palworld, ARK, DRG, RoR2
        let match_ids = [730u32, 1172470]; // CS2, Apex

        let mut candidates = Vec::new();
        for id in coop_ids {
            candidates.push(ranking(id, cooperative_signals()));
        }
        for id in match_ids {
            candidates.push(ranking(id, matchmaking_signals()));
        }

        let ranked = rank_feed(FeedSection::ClassicLegacy, &candidates, &prefs, None);
        assert_eq!(ranked.algorithm_version, ALGORITHM_VERSION);
        assert_eq!(ranked.items.len(), 6);

        let positions: Vec<_> = ranked.items.iter().map(|i| i.app_id).collect();
        let first_match = positions
            .iter()
            .position(|id| match_ids.contains(id))
            .unwrap();
        let last_coop = positions
            .iter()
            .rposition(|id| coop_ids.contains(id))
            .unwrap();
        assert!(
            last_coop < first_match,
            "coop titles should outrank matchmaking cores: {positions:?}"
        );
    }

    #[test]
    fn play_intent_votes_lift_ranking() {
        let prefs = UserPreferences::default();
        // Two identical cooperative candidates; only the vote count differs.
        let mut low = ranking(1, cooperative_signals());
        low.play_intent_count = 0;
        let mut high = ranking(2, cooperative_signals());
        high.play_intent_count = 500;

        let ranked = rank_feed(
            FeedSection::ClassicLegacy,
            &[low.clone(), high.clone()],
            &prefs,
            None,
        );
        let positions: Vec<_> = ranked.items.iter().map(|i| i.app_id).collect();
        assert_eq!(
            positions.first(),
            Some(&2u32),
            "heavily-voted game should rank first: {positions:?}"
        );
        let high_score = ranked.items.iter().find(|i| i.app_id == 2).unwrap();
        let low_score = ranked.items.iter().find(|i| i.app_id == 1).unwrap();
        assert!(high_score.score.final_score > low_score.score.final_score);
        assert!((0.0..=1.0).contains(&high_score.score.final_score));
    }

    #[test]
    fn play_intent_requires_community_evidence_and_has_a_three_point_cap() {
        let prefs = UserPreferences::default();
        let relevance_for = |count| {
            let mut candidate = ranking(1, cooperative_signals());
            candidate.play_intent_count = count;
            rank_feed(FeedSection::ClassicLegacy, &[candidate], &prefs, None).items[0]
                .score
                .relevance_score
        };

        let baseline = relevance_for(0);
        assert_eq!(relevance_for(4), baseline);
        assert_eq!(relevance_for(5), baseline);

        let first_lift = relevance_for(6) - baseline;
        assert!((first_lift - 0.03 / 4.0).abs() < 1e-12);
        assert!((relevance_for(8) - baseline - 0.015).abs() < 1e-12);

        let maximum_lift = relevance_for(u32::MAX) - baseline;
        assert!(maximum_lift > first_lift);
        assert!(maximum_lift <= 0.03);
    }

    #[test]
    fn competitive_preference_can_lift_matchmaking() {
        let prefs = UserPreferences {
            coop_competitive: 0.9,
            self_hosting_willingness: 0.1,
            ..Default::default()
        };

        let candidates = vec![
            ranking(548430, cooperative_signals()),
            ranking(730, matchmaking_signals()),
        ];
        let ranked = rank_feed(FeedSection::PopularLegacy, &candidates, &prefs, None);
        // With high competitive preference, CS2 should not be forced below coop always,
        // but still appear with cautions about public matchmaking.
        let cs = ranked.items.iter().find(|i| i.app_id == 730).unwrap();
        assert!(!cs.explanation.cautions.is_empty() || cs.score.friend_fit < 0.5);
    }

    #[test]
    fn known_platform_language_session_and_budget_mismatches_are_hard_filters() {
        let prefs = UserPreferences::default();
        let base = ranking(548430, cooperative_signals());
        let mismatches = [
            CandidateAvailability {
                platforms: vec!["linux".into()],
                ..Default::default()
            },
            CandidateAvailability {
                platforms: vec!["windows".into()],
                languages: vec!["japanese".into()],
                ..Default::default()
            },
            CandidateAvailability {
                typical_session_minutes_min: Some(240),
                typical_session_minutes_max: Some(360),
                ..Default::default()
            },
            CandidateAvailability {
                price_currency: Some("CNY".into()),
                final_price_minor: Some(20_000),
                is_free: Some(false),
                ..Default::default()
            },
        ];
        for availability in mismatches {
            let candidate = RankingInput {
                availability,
                ..base.clone()
            };
            let ranked = rank_feed_with_constraints(
                FeedSection::ClassicLegacy,
                &[candidate],
                &prefs,
                &HardConstraints::ALL,
                None,
            );
            assert!(ranked.items.is_empty());
        }

        let unknown = rank_feed(FeedSection::ClassicLegacy, &[base], &prefs, None);
        assert_eq!(unknown.items.len(), 1, "unknown facts must remain eligible");
    }

    #[test]
    fn legacy_macos_platform_alias_matches_canonical_mac_preference() {
        let prefs = UserPreferences {
            platforms: vec!["mac".into()],
            ..Default::default()
        };
        let candidate = RankingInput {
            availability: CandidateAvailability {
                platforms: vec!["macos".into()],
                ..Default::default()
            },
            ..ranking(548430, cooperative_signals())
        };
        let ranked = rank_feed(FeedSection::ClassicLegacy, &[candidate], &prefs, None);
        assert_eq!(ranked.items.len(), 1);
    }
}
