use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use mpgs_domain::ModeFamily;

use crate::pipeline::{RankedCandidate, SlotReason};
use crate::unit;

const MMR_WINDOW_SIZE: usize = 200;
const EXPLORE_START: usize = 4;
const EXPLORE_END_EXCLUSIVE: usize = 8;
const MAX_EXPLORE_SLOTS: usize = 2;
const EXPLORE_MIN_CONFIDENCE: f64 = 0.45;
const EXPLORE_RELEVANCE_MARGIN: f64 = 0.10;
const MODE_GUARDRAIL_TOP_K: usize = 20;
const MODE_SHARE_NUMERATOR: usize = 3;
const MODE_SHARE_DENOMINATOR: usize = 5;

#[derive(Debug)]
struct RemainingCandidate {
    item: RankedCandidate,
    baseline_rank: usize,
    max_similarity: f64,
}

/// Keeps a known mode family from occupying more than 60% of the visible
/// Top20. The constraint is enabled only when the re-rank window contains at
/// least three known families and there are enough candidates to satisfy it;
/// sparse pools therefore never lose candidates or deadlock selection.
#[derive(Debug)]
struct ModeGuardrail {
    enabled: bool,
    top_k: usize,
    max_per_mode: usize,
    selected_per_mode: HashMap<ModeFamily, usize>,
}

impl ModeGuardrail {
    fn new(items: &[RankedCandidate]) -> Self {
        let top_k = items.len().min(MODE_GUARDRAIL_TOP_K);
        let max_per_mode = top_k * MODE_SHARE_NUMERATOR / MODE_SHARE_DENOMINATOR;
        let mut available_per_mode = HashMap::<ModeFamily, usize>::new();
        let mut unknown_count = 0usize;
        for item in items {
            if let Some(mode) = known_mode(item) {
                *available_per_mode.entry(mode).or_default() += 1;
            } else {
                unknown_count += 1;
            }
        }

        let feasible_capacity = unknown_count
            + available_per_mode
                .values()
                .map(|count| (*count).min(max_per_mode))
                .sum::<usize>();
        let enabled = top_k > 0
            && max_per_mode > 0
            && available_per_mode.len() >= 3
            && feasible_capacity >= top_k;

        Self {
            enabled,
            top_k,
            max_per_mode,
            selected_per_mode: HashMap::new(),
        }
    }

    fn allows(&self, item: &RankedCandidate, selected_len: usize) -> bool {
        if !self.enabled || selected_len >= self.top_k {
            return true;
        }
        known_mode(item).is_none_or(|mode| {
            self.selected_per_mode.get(&mode).copied().unwrap_or(0) < self.max_per_mode
        })
    }

    fn record(&mut self, item: &RankedCandidate, selected_len: usize) {
        if !self.enabled || selected_len >= self.top_k {
            return;
        }
        if let Some(mode) = known_mode(item) {
            *self.selected_per_mode.entry(mode).or_default() += 1;
        }
    }
}

/// Maximal Marginal Relevance re-rank for relevance, structured content
/// diversity, and a small confidence-gated exploration window.
///
/// `lambda` closer to 1 prefers pure relevance; closer to 0 prefers novelty.
pub fn mmr_rerank(
    items: Vec<RankedCandidate>,
    lambda: f64,
    explore_slots: usize,
) -> Vec<RankedCandidate> {
    mmr_rerank_with_tie_seed(items, lambda, explore_slots, 0)
}

/// Seeded variant used by player-facing feeds for stable per-player, per-day
/// tie resolution. A zero seed preserves the legacy App ID fallback so callers
/// that do not have a request identity keep their historical ordering.
pub fn mmr_rerank_with_tie_seed(
    items: Vec<RankedCandidate>,
    lambda: f64,
    explore_slots: usize,
    tie_seed: u64,
) -> Vec<RankedCandidate> {
    if items.len() <= 1 {
        return items;
    }
    let lambda = unit(lambda);
    let mut sorted = items;
    for item in &mut sorted {
        item.slot_reason = SlotReason::Base;
    }
    sorted.sort_by(|left, right| compare_baseline(left, right, tie_seed));

    // Candidate queries should already be unique, but keeping one deterministic
    // best occurrence prevents malformed pools from producing duplicate cards.
    let mut seen = HashSet::new();
    sorted.retain(|item| seen.insert(item.app_id));

    let tail = if sorted.len() > MMR_WINDOW_SIZE {
        sorted.split_off(MMR_WINDOW_SIZE)
    } else {
        Vec::new()
    };
    let mut mode_guardrail = ModeGuardrail::new(&sorted);
    let explore_cutoff = sorted.get(11).map(relevance);
    let mut remaining: Vec<_> = sorted
        .into_iter()
        .enumerate()
        .map(|(baseline_rank, item)| RemainingCandidate {
            item,
            baseline_rank,
            max_similarity: 0.0,
        })
        .collect();

    let mut selected: Vec<RankedCandidate> = Vec::with_capacity(remaining.len() + tail.len());
    let mut explored = 0usize;
    let explore_slots = explore_slots.min(MAX_EXPLORE_SLOTS);
    while !remaining.is_empty() {
        let in_explore_window = (EXPLORE_START..EXPLORE_END_EXCLUSIVE).contains(&selected.len());
        let explore_idx = if in_explore_window && explored < explore_slots {
            explore_candidate_index(
                &remaining,
                explore_cutoff,
                &mode_guardrail,
                selected.len(),
                tie_seed,
            )
        } else {
            None
        };

        let (best_idx, reason) = if let Some(index) = explore_idx {
            explored += 1;
            (index, SlotReason::Explore)
        } else {
            let index = best_mmr_index(
                &remaining,
                lambda,
                &mode_guardrail,
                selected.len(),
                tie_seed,
            );
            let reason = if index == 0 {
                SlotReason::Base
            } else {
                SlotReason::Diversity
            };
            (index, reason)
        };

        let mut chosen = remaining.remove(best_idx).item;
        chosen.slot_reason = reason;
        mode_guardrail.record(&chosen, selected.len());
        for candidate in &mut remaining {
            candidate.max_similarity = candidate
                .max_similarity
                .max(similarity(&chosen, &candidate.item));
        }
        selected.push(chosen);
    }
    selected.extend(tail);
    selected
}

fn best_mmr_index(
    remaining: &[RemainingCandidate],
    lambda: f64,
    mode_guardrail: &ModeGuardrail,
    selected_len: usize,
    tie_seed: u64,
) -> usize {
    let mut best: Option<(usize, f64)> = None;
    for (index, candidate) in remaining.iter().enumerate() {
        if !mode_guardrail.allows(&candidate.item, selected_len) {
            continue;
        }
        let value =
            lambda * relevance(&candidate.item) - (1.0 - lambda) * unit(candidate.max_similarity);
        let is_better = best.is_none_or(|(best_idx, best_value)| {
            value.total_cmp(&best_value).then_with(|| {
                compare_baseline(&candidate.item, &remaining[best_idx].item, tie_seed).reverse()
            }) == Ordering::Greater
        });
        if is_better {
            best = Some((index, value));
        }
    }
    // Feasibility is checked before the guardrail is enabled. Falling back to
    // the baseline head still keeps this function total if malformed metadata
    // reaches it.
    best.map_or(0, |(index, _)| index)
}

fn explore_candidate_index(
    remaining: &[RemainingCandidate],
    cutoff: Option<f64>,
    mode_guardrail: &ModeGuardrail,
    selected_len: usize,
    tie_seed: u64,
) -> Option<usize> {
    let cutoff = cutoff?;
    let margin = cutoff.abs().max(0.01) * EXPLORE_RELEVANCE_MARGIN;
    remaining
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            candidate.baseline_rank >= 12
                && mode_guardrail.allows(&candidate.item, selected_len)
                && unit(candidate.item.data_confidence) >= EXPLORE_MIN_CONFIDENCE
                && relevance(&candidate.item) >= cutoff - margin
                && has_diversity_metadata(&candidate.item)
                && 1.0 - unit(candidate.max_similarity) >= 0.25
        })
        .max_by(|(_, a), (_, b)| {
            let a_novelty = 1.0 - unit(a.max_similarity);
            let b_novelty = 1.0 - unit(b.max_similarity);
            a_novelty
                .total_cmp(&b_novelty)
                .then_with(|| compare_baseline(&b.item, &a.item, tie_seed))
        })
        .map(|(index, _)| index)
}

fn compare_baseline(a: &RankedCandidate, b: &RankedCandidate, tie_seed: u64) -> Ordering {
    relevance(b)
        .total_cmp(&relevance(a))
        .then_with(|| unit(b.data_confidence).total_cmp(&unit(a.data_confidence)))
        .then_with(|| unit(b.score.quality).total_cmp(&unit(a.score.quality)))
        // Upcoming uses this component for release proximity; other sections
        // use actual freshness. Together with data confidence it is the best
        // currently materialized proxy for date certainty.
        .then_with(|| unit(b.score.freshness).total_cmp(&unit(a.score.freshness)))
        .then_with(|| {
            if tie_seed != 0 {
                stable_tie_hash(tie_seed, a.app_id).cmp(&stable_tie_hash(tie_seed, b.app_id))
            } else {
                Ordering::Equal
            }
        })
        .then_with(|| a.app_id.cmp(&b.app_id))
}

/// SplitMix64 finalizer: deterministic across processes and Rust releases,
/// unlike `DefaultHasher`. It is ranking entropy only, not a privacy hash.
fn stable_tie_hash(seed: u64, app_id: u32) -> u64 {
    let mut value = seed ^ u64::from(app_id).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn relevance(item: &RankedCandidate) -> f64 {
    if item.score.relevance_score.is_finite() {
        item.score.relevance_score
    } else {
        f64::NEG_INFINITY
    }
}

fn similarity(a: &RankedCandidate, b: &RankedCandidate) -> f64 {
    if a.app_id == b.app_id {
        return 1.0;
    }
    let mut weighted_sum = 0.0;
    let mut known_weight = 0.0;

    if let Some(similarity) = tag_similarity(&a.taxonomy_tags, &b.taxonomy_tags) {
        weighted_sum += 0.45 * similarity;
        known_weight += 0.45;
    }
    if let (Some(a_mode), Some(b_mode)) = (&a.dominant_mode, &b.dominant_mode)
        && let Some(similarity) = mode_capability_similarity(a_mode, b_mode)
    {
        weighted_sum += 0.25 * similarity;
        known_weight += 0.25;
    }
    if let (Some(a_publisher), Some(b_publisher)) = (&a.publisher, &b.publisher)
        && let Some(similarity) = exact_text_similarity(a_publisher, b_publisher)
    {
        weighted_sum += 0.15 * similarity;
        known_weight += 0.15;
    }
    if let (Some(a_series), Some(b_series)) = (&a.series, &b.series)
        && let Some(similarity) = exact_text_similarity(a_series, b_series)
    {
        weighted_sum += 0.15 * similarity;
        known_weight += 0.15;
    }

    if known_weight == 0.0 {
        0.0
    } else {
        unit(weighted_sum / known_weight)
    }
}

fn tag_similarity(a: &[String], b: &[String]) -> Option<f64> {
    let a: HashSet<_> = a
        .iter()
        .map(|tag| normalize_text(tag))
        .filter(|tag| !tag.is_empty())
        .collect();
    let b: HashSet<_> = b
        .iter()
        .map(|tag| normalize_text(tag))
        .filter(|tag| !tag.is_empty())
        .collect();
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let intersection = a.intersection(&b).count();
    let union = a.union(&b).count();
    Some(intersection as f64 / union as f64)
}

fn mode_capability_similarity(a: &str, b: &str) -> Option<f64> {
    let a_family = ModeFamily::from_alias(a);
    let b_family = ModeFamily::from_alias(b);
    match (a_family, b_family) {
        (ModeFamily::Unknown, _) | (_, ModeFamily::Unknown) => None,
        (ModeFamily::GenericMultiplayer, ModeFamily::GenericMultiplayer) => Some(1.0),
        (ModeFamily::GenericMultiplayer, _) | (_, ModeFamily::GenericMultiplayer) => None,
        _ => {
            let a_mask = mode_capability_mask(a_family);
            let b_mask = mode_capability_mask(b_family);
            let intersection = (a_mask & b_mask).count_ones();
            let union = (a_mask | b_mask).count_ones();
            Some(f64::from(intersection) / f64::from(union))
        }
    }
}

fn mode_capability_mask(mode: ModeFamily) -> u8 {
    const COOP: u8 = 1 << 0;
    const PRIVATE_SESSION: u8 = 1 << 1;
    const SELF_HOSTED: u8 = 1 << 2;
    const MATCHMADE: u8 = 1 << 3;
    const PVP: u8 = 1 << 4;
    const PUBLIC_WORLD: u8 = 1 << 5;

    match mode {
        ModeFamily::PrivateCoop => COOP | PRIVATE_SESSION,
        ModeFamily::SelfHosted => PRIVATE_SESSION | SELF_HOSTED,
        ModeFamily::MatchmadePvp => MATCHMADE | PVP,
        ModeFamily::PublicWorld => PUBLIC_WORLD,
        ModeFamily::Mixed => COOP | PRIVATE_SESSION | SELF_HOSTED | MATCHMADE | PVP | PUBLIC_WORLD,
        ModeFamily::GenericMultiplayer | ModeFamily::Unknown => 0,
    }
}

fn exact_text_similarity(a: &str, b: &str) -> Option<f64> {
    let a = normalize_text(a);
    let b = normalize_text(b);
    if a.is_empty() || b.is_empty() {
        None
    } else {
        Some(f64::from(a == b))
    }
}

fn normalize_text(value: &str) -> String {
    value.trim().to_lowercase()
}

fn known_mode(item: &RankedCandidate) -> Option<ModeFamily> {
    let mode = ModeFamily::from_alias(item.dominant_mode.as_deref()?);
    (mode != ModeFamily::Unknown).then_some(mode)
}

fn has_diversity_metadata(item: &RankedCandidate) -> bool {
    item.taxonomy_tags
        .iter()
        .any(|tag| !normalize_text(tag).is_empty())
        || known_mode(item).is_some()
        || item
            .publisher
            .as_deref()
            .is_some_and(|value| !normalize_text(value).is_empty())
        || item
            .series
            .as_deref()
            .is_some_and(|value| !normalize_text(value).is_empty())
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::{
        MMR_WINDOW_SIZE, mmr_rerank, mmr_rerank_with_tie_seed, mode_capability_similarity,
        similarity,
    };
    use crate::{Explanation, RankedCandidate, ScoreBreakdown, SlotReason};

    fn candidate(
        app_id: u32,
        relevance: f64,
        dominant_mode: Option<&str>,
        taxonomy_tags: &[&str],
    ) -> RankedCandidate {
        RankedCandidate {
            app_id,
            name: format!("game-{app_id}"),
            dominant_mode: dominant_mode.map(str::to_owned),
            taxonomy_tags: taxonomy_tags.iter().map(|tag| (*tag).to_owned()).collect(),
            publisher: None,
            series: None,
            recommended_min: Some(1),
            recommended_max: Some(4),
            data_confidence: 0.8,
            score: ScoreBreakdown {
                friend_fit: 0.8,
                section_score: relevance.clamp(0.0, 1.0),
                personalized_score: relevance.clamp(0.0, 1.0),
                group_fit: 0.8,
                mode_fit: 0.8,
                access_fit: 0.8,
                hosting_fit: 0.8,
                session_fit: 0.8,
                quality: 0.8,
                activity: 0.8,
                freshness: 0.8,
                risk: 0.0,
                relevance_score: relevance,
                final_score: relevance.clamp(0.0, 1.0),
            },
            explanation: Explanation {
                reasons: vec!["reason".into()],
                cautions: Vec::new(),
                evidence_ids: Vec::new(),
            },
            slot_reason: SlotReason::Base,
            algorithm_version: "test".into(),
        }
    }

    #[test]
    fn structured_content_diversity_can_separate_near_duplicate_titles() {
        let first = candidate(1, 1.0, Some("private_coop"), &["action", "coop"]);
        let duplicate = candidate(2, 0.99, Some("private_coop"), &["action", "coop"]);
        let different = candidate(3, 0.98, Some("matchmade_pvp"), &["strategy", "pvp"]);

        let reranked = mmr_rerank(vec![duplicate, different, first], 0.5, 0);

        assert_eq!(
            reranked.iter().map(|item| item.app_id).collect::<Vec<_>>(),
            vec![1, 3, 2]
        );
        assert_eq!(reranked[1].slot_reason, SlotReason::Diversity);
    }

    #[test]
    fn pure_relevance_order_and_ties_are_deterministic() {
        let inputs = vec![
            candidate(30, 0.8, Some("coop"), &["coop"]),
            candidate(10, 0.9, Some("pvp"), &["pvp"]),
            candidate(20, 0.9, Some("strategy"), &["strategy"]),
        ];

        let forward = mmr_rerank(inputs.clone(), 1.0, 0);
        let reverse = mmr_rerank(inputs.into_iter().rev().collect(), 1.0, 0);
        let ids =
            |items: &[RankedCandidate]| items.iter().map(|item| item.app_id).collect::<Vec<_>>();

        assert_eq!(ids(&forward), vec![10, 20, 30]);
        assert_eq!(ids(&forward), ids(&reverse));
        assert!(
            forward
                .iter()
                .all(|item| item.slot_reason == SlotReason::Base)
        );
    }

    #[test]
    fn seeded_ties_are_input_order_invariant_and_can_rotate_by_seed() {
        let inputs = vec![
            candidate(10, 0.9, Some("private_coop"), &["coop"]),
            candidate(20, 0.9, Some("private_coop"), &["coop"]),
            candidate(30, 0.9, Some("private_coop"), &["coop"]),
            candidate(40, 0.9, Some("private_coop"), &["coop"]),
        ];
        let ids =
            |items: &[RankedCandidate]| items.iter().map(|item| item.app_id).collect::<Vec<_>>();

        let first = mmr_rerank_with_tie_seed(inputs.clone(), 1.0, 0, 7);
        let reversed = mmr_rerank_with_tie_seed(inputs.iter().cloned().rev().collect(), 1.0, 0, 7);
        assert_eq!(ids(&first), ids(&reversed));

        let rotated = (8..128)
            .map(|seed| mmr_rerank_with_tie_seed(inputs.clone(), 1.0, 0, seed))
            .find(|items| ids(items) != ids(&first))
            .expect("at least one daily seed should rotate an exact tie group");
        assert_ne!(ids(&first), ids(&rotated));
    }

    #[test]
    fn confidence_quality_and_freshness_dominate_the_seeded_tie_hash() {
        let mut high_confidence = candidate(10, 0.9, Some("private_coop"), &["coop"]);
        high_confidence.data_confidence = 0.9;
        high_confidence.score.quality = 0.1;
        let mut low_confidence = candidate(20, 0.9, Some("private_coop"), &["coop"]);
        low_confidence.data_confidence = 0.8;
        low_confidence.score.quality = 1.0;
        let ranked =
            mmr_rerank_with_tie_seed(vec![low_confidence, high_confidence], 1.0, 0, u64::MAX);
        assert_eq!(ranked[0].app_id, 10);

        let mut high_quality = candidate(30, 0.9, Some("private_coop"), &["coop"]);
        high_quality.score.quality = 0.9;
        high_quality.score.freshness = 0.1;
        let mut low_quality = candidate(40, 0.9, Some("private_coop"), &["coop"]);
        low_quality.score.quality = 0.8;
        low_quality.score.freshness = 1.0;
        let ranked = mmr_rerank_with_tie_seed(vec![low_quality, high_quality], 1.0, 0, u64::MAX);
        assert_eq!(ranked[0].app_id, 30);

        let mut fresh = candidate(50, 0.9, Some("private_coop"), &["coop"]);
        fresh.score.freshness = 0.9;
        let mut stale = candidate(60, 0.9, Some("private_coop"), &["coop"]);
        stale.score.freshness = 0.8;
        let ranked = mmr_rerank_with_tie_seed(vec![stale, fresh], 1.0, 0, u64::MAX);
        assert_eq!(ranked[0].app_id, 50);
    }

    #[test]
    fn relevance_guardrail_keeps_a_much_weaker_novel_title_back() {
        let first = candidate(1, 1.0, Some("coop"), &["action", "coop"]);
        let close_duplicate = candidate(2, 0.9, Some("coop"), &["action", "coop"]);
        let weak_novel = candidate(3, 0.6, Some("pvp"), &["strategy", "pvp"]);

        let reranked = mmr_rerank(vec![weak_novel, close_duplicate, first], 0.85, 0);

        assert_eq!(
            reranked.iter().map(|item| item.app_id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn localized_explanation_text_never_changes_ranking() {
        let mut translated = vec![
            candidate(1, 0.9, Some("coop"), &["coop"]),
            candidate(2, 0.89, Some("coop"), &["coop"]),
            candidate(3, 0.88, Some("pvp"), &["pvp"]),
        ];
        let original = mmr_rerank(translated.clone(), 0.75, 0);
        translated[0].explanation.reasons = vec!["早期数据".into()];
        translated[1].explanation.cautions = vec!["置信度偏低".into()];
        translated[2].explanation.reasons = vec!["early data".into()];

        let after_translation = mmr_rerank(translated, 0.75, 0);

        assert_eq!(
            original.iter().map(|item| item.app_id).collect::<Vec<_>>(),
            after_translation
                .iter()
                .map(|item| item.app_id)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn similarity_uses_known_dimensions_and_mode_capability_overlap() {
        let mut private_coop = candidate(1, 0.9, Some("private_coop"), &["Action", "Co-op"]);
        private_coop.publisher = Some("Studio".into());
        private_coop.series = Some("Alpha".into());
        let mut self_hosted = candidate(2, 0.89, Some("self_hosted"), &["action", "co-op"]);
        self_hosted.publisher = Some(" studio ".into());
        self_hosted.series = Some("Beta".into());

        // private_coop={coop,private}; self_hosted={private,self_hosted}
        assert_eq!(
            mode_capability_similarity("private_coop", "self_hosted"),
            Some(1.0 / 3.0)
        );
        // tags=1.0, mode=1/3, publisher=1.0, franchise=0.0, with the
        // configured 0.45/0.25/0.15/0.15 known-dimension weights.
        let expected = 0.45 + 0.25 / 3.0 + 0.15;
        assert!((similarity(&private_coop, &self_hosted) - expected).abs() < 1e-12);

        // Unknown/generic-vs-specific values do not fabricate a negative mode
        // comparison; the remaining known dimensions are renormalized.
        assert_eq!(mode_capability_similarity("unknown", "pvp"), None);
        assert_eq!(
            mode_capability_similarity("generic_multiplayer", "pvp"),
            None
        );
    }

    #[test]
    fn high_confidence_near_cutoff_candidate_can_fill_a_structured_explore_slot() {
        let mut items: Vec<_> = (0..12)
            .map(|index| {
                candidate(
                    index,
                    0.99 - f64::from(index) * 0.001,
                    Some("coop"),
                    &["action", "coop"],
                )
            })
            .collect();
        items.push(candidate(12, 0.978, Some("pvp"), &["strategy", "pvp"]));

        let reranked = mmr_rerank(items, 1.0, 2);

        assert_eq!(reranked[4].app_id, 12);
        assert_eq!(reranked[4].slot_reason, SlotReason::Explore);
    }

    #[test]
    fn low_confidence_candidate_is_not_forced_into_exploration() {
        let mut items: Vec<_> = (0..12)
            .map(|index| {
                candidate(
                    index,
                    0.99 - f64::from(index) * 0.001,
                    Some("coop"),
                    &["action", "coop"],
                )
            })
            .collect();
        let mut low_confidence = candidate(12, 0.978, Some("pvp"), &["strategy", "pvp"]);
        low_confidence.data_confidence = 0.44;
        items.push(low_confidence);

        let reranked = mmr_rerank(items, 1.0, 2);

        assert_eq!(reranked[12].app_id, 12);
        assert_ne!(reranked[12].slot_reason, SlotReason::Explore);
    }

    #[test]
    fn exploration_is_capped_at_two_and_only_uses_positions_five_through_eight() {
        let mut items: Vec<_> = (0..12)
            .map(|index| {
                candidate(
                    index,
                    1.0 - f64::from(index) * 0.001,
                    Some("private_coop"),
                    &["action", "coop"],
                )
            })
            .collect();
        items.push(candidate(
            12,
            0.988,
            Some("matchmade_pvp"),
            &["strategy", "pvp"],
        ));
        items.push(candidate(13, 0.987, Some("public_world"), &["rpg", "mmo"]));
        items.push(candidate(
            14,
            0.986,
            Some("self_hosted"),
            &["survival", "sandbox"],
        ));

        let reranked = mmr_rerank(items, 1.0, usize::MAX);
        let explore_positions: Vec<_> = reranked
            .iter()
            .enumerate()
            .filter_map(|(index, item)| (item.slot_reason == SlotReason::Explore).then_some(index))
            .collect();

        assert_eq!(explore_positions.len(), 2);
        assert!(explore_positions.iter().all(|index| (4..8).contains(index)));
    }

    #[test]
    fn candidate_outside_top_twelve_margin_is_not_an_explore_slot() {
        let mut items: Vec<_> = (0..12)
            .map(|index| {
                candidate(
                    index,
                    1.0 - f64::from(index) * 0.001,
                    Some("private_coop"),
                    &["action", "coop"],
                )
            })
            .collect();
        items.push(candidate(
            12,
            0.80,
            Some("matchmade_pvp"),
            &["strategy", "pvp"],
        ));

        let reranked = mmr_rerank(items, 1.0, 2);

        assert!(
            reranked
                .iter()
                .all(|item| item.slot_reason != SlotReason::Explore)
        );
    }

    #[test]
    fn feasible_three_mode_pool_caps_a_single_mode_at_sixty_percent_of_top_twenty() {
        let mut items = Vec::new();
        for index in 0..20 {
            items.push(candidate(
                index,
                1.0 - f64::from(index) * 0.001,
                Some("private_coop"),
                &["coop"],
            ));
        }
        for index in 20..25 {
            items.push(candidate(
                index,
                0.75 - f64::from(index - 20) * 0.001,
                Some("matchmade_pvp"),
                &["pvp"],
            ));
        }
        for index in 25..30 {
            items.push(candidate(
                index,
                0.70 - f64::from(index - 25) * 0.001,
                Some("public_world"),
                &["mmo"],
            ));
        }

        let reranked = mmr_rerank(items, 1.0, 0);
        let mut mode_counts = HashMap::new();
        for item in reranked.iter().take(20) {
            *mode_counts
                .entry(item.dominant_mode.as_deref().unwrap())
                .or_insert(0usize) += 1;
        }

        assert_eq!(reranked.len(), 30);
        assert_eq!(mode_counts.get("private_coop"), Some(&12));
        assert!(mode_counts.values().all(|count| *count <= 12));
    }

    #[test]
    fn rerank_is_input_order_invariant_and_preserves_every_unique_candidate_once() {
        let inputs: Vec<_> = (0..30)
            .map(|index| {
                let (mode, tag) = match index % 3 {
                    0 => ("private_coop", "coop"),
                    1 => ("matchmade_pvp", "pvp"),
                    _ => ("public_world", "mmo"),
                };
                candidate(index, 1.0 - f64::from(index) * 0.005, Some(mode), &[tag])
            })
            .collect();
        let mut permuted = inputs.clone();
        permuted.rotate_left(11);
        permuted.reverse();

        let forward = mmr_rerank(inputs, 0.85, 2);
        let reordered = mmr_rerank(permuted, 0.85, 2);
        let ids =
            |items: &[RankedCandidate]| items.iter().map(|item| item.app_id).collect::<Vec<_>>();
        let unique: HashSet<_> = forward.iter().map(|item| item.app_id).collect();

        assert_eq!(ids(&forward), ids(&reordered));
        assert_eq!(forward.len(), 30);
        assert_eq!(unique.len(), forward.len());
    }

    #[test]
    fn duplicate_app_ids_collapse_to_the_highest_relevance_item() {
        let low = candidate(1, 0.2, Some("coop"), &["coop"]);
        let high = candidate(1, 0.9, Some("coop"), &["coop"]);
        let other = candidate(2, 0.8, Some("pvp"), &["pvp"]);

        let reranked = mmr_rerank(vec![low, other, high], 1.0, 0);

        assert_eq!(reranked.len(), 2);
        assert_eq!(reranked[0].app_id, 1);
        assert_eq!(reranked[0].score.relevance_score, 0.9);
    }

    #[test]
    fn large_tail_is_preserved_in_relevance_order() {
        let items: Vec<_> = (0..(MMR_WINDOW_SIZE + 25))
            .map(|index| {
                candidate(
                    index as u32,
                    1.0 - index as f64 / 1_000.0,
                    Some("coop"),
                    &["coop"],
                )
            })
            .collect();
        let reranked = mmr_rerank(items, 0.75, 2);
        assert_eq!(reranked.len(), MMR_WINDOW_SIZE + 25);
        let tail_ids: Vec<_> = reranked[MMR_WINDOW_SIZE..]
            .iter()
            .map(|item| item.app_id)
            .collect();
        assert_eq!(
            tail_ids,
            (MMR_WINDOW_SIZE as u32..(MMR_WINDOW_SIZE + 25) as u32).collect::<Vec<_>>()
        );
    }
}
