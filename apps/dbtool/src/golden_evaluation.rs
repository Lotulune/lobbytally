//! Offline, read-only evaluation for human recommendation judgments.
//!
//! The command accepts this deliberately explicit JSON schema. Every numeric
//! feature and baseline is normalized to `[0, 1]`; missing values must be
//! resolved while curating the matrix instead of silently becoming zero.
//!
//! ```json
//! {
//!   "schema_version": "recommendation_golden_labels_v1",
//!   "labels": [{
//!     "persona_id": "four-player-private-coop",
//!     "app_id": 892970,
//!     "section": "popular_legacy",
//!     "relevance": 3,
//!     "personal_fit": 0.95,
//!     "quality": 0.88,
//!     "activity": 0.76,
//!     "momentum": 0.52,
//!     "freshness": 0.10,
//!     "demo": 0.0,
//!     "date_confidence": 1.0,
//!     "studio_prior": 0.60,
//!     "longevity": 0.82,
//!     "maintenance": 0.90,
//!     "risk": 0.05,
//!     "ccu_baseline": 0.76,
//!     "review_baseline": 0.88
//!   }]
//! }
//! ```
//!
//! `relevance` is a human judgment from 0 (irrelevant) through 3 (excellent).
//! The two baseline fields are scores, not model inputs. The public command
//! rejects fewer than 200 validated, unique persona/game/section judgments.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use mpgs_domain::{FeedSection, RankingSignals};
use mpgs_recommender::{ALGORITHM_VERSION, score};
use serde::{Deserialize, Serialize};

const SCHEMA_VERSION: &str = "recommendation_golden_labels_v1";
const MIN_PUBLIC_LABELS: usize = 200;
const FOLD_COUNT: usize = 5;
const TOP_K: usize = 20;
const DETERMINISTIC_SEED: u64 = 0x4c6f_6262_7954_616c;
const FEATURE_COUNT: usize = 11;
const PERSONAL: usize = 0;
const QUALITY: usize = 1;
const ACTIVITY: usize = 2;
const MOMENTUM: usize = 3;
const FRESHNESS: usize = 4;
const DEMO: usize = 5;
const DATE_CONFIDENCE: usize = 6;
const STUDIO_PRIOR: usize = 7;
const LONGEVITY: usize = 8;
const MAINTENANCE: usize = 9;
const RISK: usize = 10;
const L2: f64 = 0.02;
const TRAINING_EPOCHS: usize = 400;
const INITIAL_STEP: f64 = 0.20;
const MIN_NDCG: f64 = 0.80;
const MIN_PAIR_ACCURACY: f64 = 0.90;
const MIN_RELATIVE_LIFT: f64 = 0.05;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoldenDataset {
    schema_version: String,
    labels: Vec<GoldenLabel>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoldenLabel {
    persona_id: String,
    app_id: u32,
    section: FeedSection,
    relevance: u8,
    personal_fit: f64,
    quality: f64,
    activity: f64,
    momentum: f64,
    freshness: f64,
    demo: f64,
    date_confidence: f64,
    studio_prior: f64,
    longevity: f64,
    maintenance: f64,
    risk: f64,
    ccu_baseline: f64,
    review_baseline: f64,
}

impl GoldenLabel {
    fn features(&self) -> [f64; FEATURE_COUNT] {
        [
            self.personal_fit,
            self.quality,
            self.activity,
            self.momentum,
            self.freshness,
            self.demo,
            self.date_confidence,
            self.studio_prior,
            self.longevity,
            self.maintenance,
            self.risk,
        ]
    }
}

#[derive(Debug, Clone, Copy)]
struct Model {
    weights: [f64; FEATURE_COUNT],
}

#[derive(Debug, Clone)]
struct Pair {
    difference: [f64; FEATURE_COUNT],
}

#[derive(Debug, Clone, Copy)]
enum Holdout {
    Persona,
    Game,
}

#[derive(Debug, Clone, Copy, Default)]
struct MetricTotals {
    ndcg_sum: f64,
    queries: usize,
    correct_pairs: f64,
    pairs: usize,
}

impl MetricTotals {
    fn merge(&mut self, other: Self) {
        self.ndcg_sum += other.ndcg_sum;
        self.queries += other.queries;
        self.correct_pairs += other.correct_pairs;
        self.pairs += other.pairs;
    }

    fn report(self) -> MetricReport {
        MetricReport {
            ndcg_at_20: (self.queries > 0).then(|| self.ndcg_sum / self.queries as f64),
            pair_direction_accuracy: (self.pairs > 0)
                .then(|| self.correct_pairs / self.pairs as f64),
            query_count: self.queries,
            pair_count: self.pairs,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct GoldenEvaluationReport {
    schema_version: &'static str,
    label_count: usize,
    persona_count: usize,
    game_count: usize,
    deterministic_seed: u64,
    fold_count: usize,
    current_rule_baseline_version: &'static str,
    training: TrainingConfig,
    section_models: Vec<SectionModelReport>,
    persona_holdout: CrossValidationReport,
    game_holdout: CrossValidationReport,
    freeze_gate: FreezeGateReport,
    freeze_eligible: bool,
    limitations: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct TrainingConfig {
    objective: &'static str,
    l2: f64,
    epochs: usize,
    personal_weight_range: [f64; 2],
    positive_weight_sum: f64,
    risk_weight_max: f64,
}

#[derive(Debug, Clone, Serialize)]
struct SectionModelReport {
    section: &'static str,
    label_count: usize,
    pair_count: usize,
    trained: bool,
    weights: WeightReport,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct WeightReport {
    personal_fit: f64,
    quality: f64,
    activity: f64,
    momentum: f64,
    freshness: f64,
    demo: f64,
    date_confidence: f64,
    studio_prior: f64,
    longevity: f64,
    maintenance: f64,
    risk: f64,
}

impl From<[f64; FEATURE_COUNT]> for WeightReport {
    fn from(value: [f64; FEATURE_COUNT]) -> Self {
        Self {
            personal_fit: value[PERSONAL],
            quality: value[QUALITY],
            activity: value[ACTIVITY],
            momentum: value[MOMENTUM],
            freshness: value[FRESHNESS],
            demo: value[DEMO],
            date_confidence: value[DATE_CONFIDENCE],
            studio_prior: value[STUDIO_PRIOR],
            longevity: value[LONGEVITY],
            maintenance: value[MAINTENANCE],
            risk: value[RISK],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct CrossValidationReport {
    strategy: &'static str,
    folds: Vec<FoldReport>,
    aggregate: ComparisonReport,
}

#[derive(Debug, Clone, Serialize)]
struct FoldReport {
    fold: usize,
    training_labels: usize,
    evaluation_labels: usize,
    metrics: ComparisonReport,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ComparisonReport {
    learned: MetricReport,
    current_rule: MetricReport,
    ccu_baseline: MetricReport,
    review_baseline: MetricReport,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct MetricReport {
    ndcg_at_20: Option<f64>,
    pair_direction_accuracy: Option<f64>,
    query_count: usize,
    pair_count: usize,
}

#[derive(Debug, Clone, Serialize)]
struct FreezeGateReport {
    required_ndcg_at_20: f64,
    required_pair_direction_accuracy: f64,
    required_relative_lift: f64,
    all_sections_trainable: bool,
    persona_holdout: HoldoutGate,
    game_holdout: HoldoutGate,
}

#[derive(Debug, Clone, Serialize)]
struct HoldoutGate {
    all_folds_evaluable: bool,
    learned_ndcg_at_20: Option<f64>,
    pair_direction_accuracy: Option<f64>,
    relative_lift_over_current_rule: Option<f64>,
    relative_lift_over_ccu: Option<f64>,
    relative_lift_over_reviews: Option<f64>,
    passed: bool,
}

pub(crate) fn run_command(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let labels_path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| evaluation_usage().to_owned())?;
    let mut json = false;
    for option in args {
        match option.as_str() {
            "--json" => json = true,
            _ => {
                return Err(format!(
                    "unknown recommendation-golden-evaluate option: {option}"
                ));
            }
        }
    }

    if !labels_path.is_file() {
        return Err(format!(
            "labels file does not exist or is not a file: {}",
            labels_path.display()
        ));
    }
    let source = fs::read_to_string(&labels_path)
        .map_err(|error| format!("failed to read {}: {error}", labels_path.display()))?;
    let dataset: GoldenDataset = serde_json::from_str(&source)
        .map_err(|error| format!("invalid golden label JSON: {error}"))?;
    validate_dataset(&dataset, MIN_PUBLIC_LABELS)?;
    let report = evaluate(&dataset.labels)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
    } else {
        print_text(&report);
    }
    Ok(())
}

fn evaluation_usage() -> &'static str {
    "recommendation-golden-evaluate <labels.json> [--json]"
}

fn validate_dataset(dataset: &GoldenDataset, minimum_labels: usize) -> Result<(), String> {
    if dataset.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "schema_version must be {SCHEMA_VERSION}, found {}",
            dataset.schema_version
        ));
    }
    let mut unique = HashSet::with_capacity(dataset.labels.len());
    let mut personas = HashSet::new();
    let mut games = HashSet::new();
    for (index, label) in dataset.labels.iter().enumerate() {
        let row = index + 1;
        if label.persona_id.trim().is_empty() {
            return Err(format!("label {row}: persona_id must not be empty"));
        }
        if label.app_id == 0 {
            return Err(format!("label {row}: app_id must be non-zero"));
        }
        if label.relevance > 3 {
            return Err(format!("label {row}: relevance must be between 0 and 3"));
        }
        for (name, value) in scalar_fields(label) {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(format!(
                    "label {row}: {name} must be a finite number between 0 and 1"
                ));
            }
        }
        if !unique.insert((label.persona_id.clone(), label.app_id, label.section)) {
            return Err(format!(
                "label {row}: duplicate persona_id/app_id/section judgment"
            ));
        }
        personas.insert(label.persona_id.as_str());
        games.insert(label.app_id);
    }
    if unique.len() < minimum_labels {
        return Err(format!(
            "at least {minimum_labels} valid labels are required; found {}",
            unique.len()
        ));
    }
    if personas.len() < FOLD_COUNT {
        return Err(format!(
            "at least {FOLD_COUNT} distinct personas are required for five-fold persona holdout"
        ));
    }
    if games.len() < FOLD_COUNT {
        return Err(format!(
            "at least {FOLD_COUNT} distinct games are required for five-fold game holdout"
        ));
    }
    Ok(())
}

fn scalar_fields(label: &GoldenLabel) -> [(&'static str, f64); 13] {
    [
        ("personal_fit", label.personal_fit),
        ("quality", label.quality),
        ("activity", label.activity),
        ("momentum", label.momentum),
        ("freshness", label.freshness),
        ("demo", label.demo),
        ("date_confidence", label.date_confidence),
        ("studio_prior", label.studio_prior),
        ("longevity", label.longevity),
        ("maintenance", label.maintenance),
        ("risk", label.risk),
        ("ccu_baseline", label.ccu_baseline),
        ("review_baseline", label.review_baseline),
    ]
}

fn evaluate(labels: &[GoldenLabel]) -> Result<GoldenEvaluationReport, String> {
    let persona_count = labels
        .iter()
        .map(|label| label.persona_id.as_str())
        .collect::<HashSet<_>>()
        .len();
    let game_count = labels
        .iter()
        .map(|label| label.app_id)
        .collect::<HashSet<_>>()
        .len();
    if persona_count < FOLD_COUNT || game_count < FOLD_COUNT {
        return Err("five-fold evaluation requires at least five personas and games".into());
    }

    let all = labels.iter().collect::<Vec<_>>();
    let (_, section_models) = train_section_models(&all);
    let persona_holdout = cross_validate(labels, Holdout::Persona);
    let game_holdout = cross_validate(labels, Holdout::Game);
    let all_sections_trainable = section_models.iter().all(|model| model.trained);
    let persona_gate = holdout_gate(&persona_holdout);
    let game_gate = holdout_gate(&game_holdout);
    let freeze_eligible = all_sections_trainable && persona_gate.passed && game_gate.passed;

    Ok(GoldenEvaluationReport {
        schema_version: SCHEMA_VERSION,
        label_count: labels.len(),
        persona_count,
        game_count,
        deterministic_seed: DETERMINISTIC_SEED,
        fold_count: FOLD_COUNT,
        current_rule_baseline_version: ALGORITHM_VERSION,
        training: TrainingConfig {
            objective: "per_section_pairwise_logistic",
            l2: L2,
            epochs: TRAINING_EPOCHS,
            personal_weight_range: [0.35, 0.55],
            positive_weight_sum: 1.0,
            risk_weight_max: 0.0,
        },
        section_models,
        persona_holdout,
        game_holdout,
        freeze_gate: FreezeGateReport {
            required_ndcg_at_20: MIN_NDCG,
            required_pair_direction_accuracy: MIN_PAIR_ACCURACY,
            required_relative_lift: MIN_RELATIVE_LIFT,
            all_sections_trainable,
            persona_holdout: persona_gate,
            game_holdout: game_gate,
        },
        freeze_eligible,
        limitations: vec![
            "weights are offline candidates and are not written to configuration or the database",
            "freeze eligibility depends on human labels; synthetic fixtures are only used by unit tests",
            "position-bias calibration remains gated on attributed production outcomes",
        ],
    })
}

fn train_section_models(labels: &[&GoldenLabel]) -> ([Model; 4], Vec<SectionModelReport>) {
    let mut models = [Model {
        weights: [0.0; FEATURE_COUNT],
    }; 4];
    let mut reports = Vec::with_capacity(4);
    for section in FeedSection::ALL {
        let section_labels = labels
            .iter()
            .copied()
            .filter(|label| label.section == section)
            .collect::<Vec<_>>();
        let pairs = build_pairs(&section_labels);
        let initial = current_rule_weights(section);
        let weights = if pairs.is_empty() {
            initial
        } else {
            train_pairwise(&pairs, initial)
        };
        models[section_index(section)] = Model { weights };
        reports.push(SectionModelReport {
            section: section.as_str(),
            label_count: section_labels.len(),
            pair_count: pairs.len(),
            trained: !pairs.is_empty(),
            weights: weights.into(),
        });
    }
    (models, reports)
}

fn build_pairs(labels: &[&GoldenLabel]) -> Vec<Pair> {
    let mut by_persona = BTreeMap::<&str, Vec<&GoldenLabel>>::new();
    for label in labels {
        by_persona
            .entry(label.persona_id.as_str())
            .or_default()
            .push(label);
    }

    let mut pairs = Vec::new();
    for group in by_persona.values_mut() {
        group.sort_by_key(|label| label.app_id);
        for left in 0..group.len() {
            for right in (left + 1)..group.len() {
                if group[left].relevance == group[right].relevance {
                    continue;
                }
                let (preferred, rejected) = if group[left].relevance > group[right].relevance {
                    (group[left], group[right])
                } else {
                    (group[right], group[left])
                };
                let preferred = preferred.features();
                let rejected = rejected.features();
                let mut difference = [0.0; FEATURE_COUNT];
                for index in 0..FEATURE_COUNT {
                    difference[index] = preferred[index] - rejected[index];
                }
                pairs.push(Pair { difference });
            }
        }
    }
    pairs
}

fn train_pairwise(pairs: &[Pair], initial: [f64; FEATURE_COUNT]) -> [f64; FEATURE_COUNT] {
    let mut weights = initial;
    project_weights(&mut weights);
    for epoch in 0..TRAINING_EPOCHS {
        let mut gradient = weights.map(|weight| L2 * weight);
        for pair in pairs {
            let margin = dot(weights, pair.difference);
            let coefficient = -sigmoid_negative_margin(margin) / pairs.len() as f64;
            for (gradient_slot, difference) in gradient.iter_mut().zip(pair.difference.iter()) {
                *gradient_slot += coefficient * difference;
            }
        }
        let step = INITIAL_STEP / (1.0 + epoch as f64 / 50.0).sqrt();
        for (weight, gradient_slot) in weights.iter_mut().zip(gradient.iter()) {
            *weight -= step * gradient_slot;
        }
        project_weights(&mut weights);
    }
    weights
}

fn sigmoid_negative_margin(margin: f64) -> f64 {
    if margin >= 0.0 {
        let exp = (-margin).exp();
        exp / (1.0 + exp)
    } else {
        1.0 / (1.0 + margin.exp())
    }
}

fn project_weights(weights: &mut [f64; FEATURE_COUNT]) {
    weights[PERSONAL] = weights[PERSONAL].clamp(0.35, 0.55);
    weights[RISK] = weights[RISK].min(0.0);
    for weight in &mut weights[1..RISK] {
        *weight = weight.max(0.0);
    }
    let remaining = 1.0 - weights[PERSONAL];
    let other_sum = weights[1..RISK].iter().sum::<f64>();
    if other_sum > f64::EPSILON {
        for weight in &mut weights[1..RISK] {
            *weight *= remaining / other_sum;
        }
    } else {
        let equal = remaining / (RISK - 1) as f64;
        weights[1..RISK].fill(equal);
    }
}

fn cross_validate(labels: &[GoldenLabel], holdout: Holdout) -> CrossValidationReport {
    let assignments = fold_assignments(labels, holdout);
    let mut folds = Vec::with_capacity(FOLD_COUNT);
    let mut learned_total = MetricTotals::default();
    let mut current_total = MetricTotals::default();
    let mut ccu_total = MetricTotals::default();
    let mut review_total = MetricTotals::default();

    for fold in 0..FOLD_COUNT {
        let mut training = Vec::new();
        let mut evaluation = Vec::new();
        for (index, label) in labels.iter().enumerate() {
            if assignments[index] == fold {
                evaluation.push(label);
            } else {
                training.push(label);
            }
        }
        let (models, _) = train_section_models(&training);
        let learned = evaluate_predictor(&evaluation, |label| {
            dot(
                models[section_index(label.section)].weights,
                label.features(),
            )
        });
        let current = evaluate_predictor(&evaluation, current_rule_score);
        let ccu = evaluate_predictor(&evaluation, |label| label.ccu_baseline);
        let review = evaluate_predictor(&evaluation, |label| label.review_baseline);

        learned_total.merge(learned);
        current_total.merge(current);
        ccu_total.merge(ccu);
        review_total.merge(review);
        folds.push(FoldReport {
            fold: fold + 1,
            training_labels: training.len(),
            evaluation_labels: evaluation.len(),
            metrics: ComparisonReport {
                learned: learned.report(),
                current_rule: current.report(),
                ccu_baseline: ccu.report(),
                review_baseline: review.report(),
            },
        });
    }

    CrossValidationReport {
        strategy: match holdout {
            Holdout::Persona => "persona_holdout",
            Holdout::Game => "game_holdout",
        },
        folds,
        aggregate: ComparisonReport {
            learned: learned_total.report(),
            current_rule: current_total.report(),
            ccu_baseline: ccu_total.report(),
            review_baseline: review_total.report(),
        },
    }
}

fn fold_assignments(labels: &[GoldenLabel], holdout: Holdout) -> Vec<usize> {
    let mut keys = labels
        .iter()
        .map(|label| match holdout {
            Holdout::Persona => format!("persona:{}", label.persona_id),
            Holdout::Game => format!("game:{}", label.app_id),
        })
        .collect::<Vec<_>>();
    keys.sort();
    keys.dedup();
    keys.sort_by(|left, right| {
        stable_hash(left)
            .cmp(&stable_hash(right))
            .then_with(|| left.cmp(right))
    });
    let key_folds = keys
        .into_iter()
        .enumerate()
        .map(|(index, key)| (key, index % FOLD_COUNT))
        .collect::<HashMap<_, _>>();

    labels
        .iter()
        .map(|label| {
            let key = match holdout {
                Holdout::Persona => format!("persona:{}", label.persona_id),
                Holdout::Game => format!("game:{}", label.app_id),
            };
            key_folds[&key]
        })
        .collect()
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64 ^ DETERMINISTIC_SEED;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn evaluate_predictor(
    labels: &[&GoldenLabel],
    predict: impl Fn(&GoldenLabel) -> f64,
) -> MetricTotals {
    let mut groups = BTreeMap::<(String, &'static str), Vec<&GoldenLabel>>::new();
    for label in labels {
        groups
            .entry((label.persona_id.clone(), label.section.as_str()))
            .or_default()
            .push(label);
    }

    let mut totals = MetricTotals::default();
    for group in groups.values() {
        if group.len() < 2 {
            continue;
        }
        let mut predicted = group
            .iter()
            .map(|label| (*label, predict(label)))
            .collect::<Vec<_>>();
        predicted.sort_by(|(left_label, left_score), (right_label, right_score)| {
            right_score
                .total_cmp(left_score)
                .then_with(|| left_label.app_id.cmp(&right_label.app_id))
        });
        let mut ideal = group.to_vec();
        ideal.sort_by(|left, right| {
            right
                .relevance
                .cmp(&left.relevance)
                .then_with(|| left.app_id.cmp(&right.app_id))
        });
        let ideal_dcg = dcg(ideal.iter().map(|label| label.relevance));
        if ideal_dcg > 0.0 {
            // Exact-score ties share their occupied ranks. App ID is only a
            // deterministic storage order and never manufactures metric lift.
            let actual_dcg = tie_aware_dcg(&predicted);
            totals.ndcg_sum += actual_dcg / ideal_dcg;
            totals.queries += 1;
        }

        for left in 0..group.len() {
            for right in (left + 1)..group.len() {
                if group[left].relevance == group[right].relevance {
                    continue;
                }
                let expected = group[left].relevance.cmp(&group[right].relevance);
                let score_order = predict(group[left]).total_cmp(&predict(group[right]));
                totals.correct_pairs += if score_order == expected {
                    1.0
                } else if score_order.is_eq() {
                    0.5
                } else {
                    0.0
                };
                totals.pairs += 1;
            }
        }
    }
    totals
}

fn dcg(relevances: impl Iterator<Item = u8>) -> f64 {
    relevances
        .take(TOP_K)
        .enumerate()
        .map(|(index, relevance)| {
            let gain = 2_f64.powi(i32::from(relevance)) - 1.0;
            gain / (index as f64 + 2.0).log2()
        })
        .sum()
}

fn tie_aware_dcg(ranked: &[(&GoldenLabel, f64)]) -> f64 {
    let mut total = 0.0;
    let mut start = 0;
    while start < ranked.len() && start < TOP_K {
        let mut end = start + 1;
        while end < ranked.len() && ranked[end].1.total_cmp(&ranked[start].1).is_eq() {
            end += 1;
        }
        let average_gain = ranked[start..end]
            .iter()
            .map(|(label, _)| 2_f64.powi(i32::from(label.relevance)) - 1.0)
            .sum::<f64>()
            / (end - start) as f64;
        total += (start..end.min(TOP_K))
            .map(|position| average_gain / (position as f64 + 2.0).log2())
            .sum::<f64>();
        start = end;
    }
    total
}

fn holdout_gate(report: &CrossValidationReport) -> HoldoutGate {
    let comparison = &report.aggregate;
    let all_folds_evaluable = report.folds.iter().all(|fold| {
        fold.metrics.learned.ndcg_at_20.is_some()
            && fold.metrics.learned.pair_direction_accuracy.is_some()
    });
    let learned = comparison.learned.ndcg_at_20;
    let pair_accuracy = comparison.learned.pair_direction_accuracy;
    let current_lift = relative_lift(learned, comparison.current_rule.ndcg_at_20);
    let ccu_lift = relative_lift(learned, comparison.ccu_baseline.ndcg_at_20);
    let review_lift = relative_lift(learned, comparison.review_baseline.ndcg_at_20);
    let passed = all_folds_evaluable
        && learned.is_some_and(|value| value >= MIN_NDCG)
        && pair_accuracy.is_some_and(|value| value >= MIN_PAIR_ACCURACY)
        && [current_lift, ccu_lift, review_lift]
            .into_iter()
            .all(|lift| lift.is_some_and(|value| value >= MIN_RELATIVE_LIFT));
    HoldoutGate {
        all_folds_evaluable,
        learned_ndcg_at_20: learned,
        pair_direction_accuracy: pair_accuracy,
        relative_lift_over_current_rule: current_lift,
        relative_lift_over_ccu: ccu_lift,
        relative_lift_over_reviews: review_lift,
        passed,
    }
}

fn relative_lift(candidate: Option<f64>, baseline: Option<f64>) -> Option<f64> {
    let (candidate, baseline) = (candidate?, baseline?);
    if baseline <= f64::EPSILON {
        return Some(if candidate > baseline { 1.0 } else { 0.0 });
    }
    Some((candidate - baseline) / baseline)
}

fn current_rule_weights(section: FeedSection) -> [f64; FEATURE_COUNT] {
    let mut weights = [0.0; FEATURE_COUNT];
    weights[PERSONAL] = 0.45;
    weights[RISK] = -0.11;
    match section {
        FeedSection::RecentRelease => {
            weights[QUALITY] = 0.22;
            weights[ACTIVITY] = 0.11;
            weights[MOMENTUM] = 0.0825;
            weights[FRESHNESS] = 0.1375;
        }
        FeedSection::Upcoming => {
            weights[DEMO] = 0.1925;
            weights[DATE_CONFIDENCE] = 0.11;
            weights[FRESHNESS] = 0.1375;
            weights[STUDIO_PRIOR] = 0.11;
        }
        FeedSection::PopularLegacy => {
            weights[ACTIVITY] = 0.22;
            weights[MOMENTUM] = 0.11;
            weights[QUALITY] = 0.165;
            weights[MAINTENANCE] = 0.055;
        }
        FeedSection::ClassicLegacy => {
            weights[QUALITY] = 0.2475;
            weights[LONGEVITY] = 0.1375;
            weights[MAINTENANCE] = 0.0825;
            weights[ACTIVITY] = 0.0825;
        }
    }
    weights
}

fn current_rule_score(label: &GoldenLabel) -> f64 {
    score(
        label.section,
        &RankingSignals {
            quality: label.quality,
            popularity: label.activity,
            momentum: label.momentum,
            freshness: label.freshness,
            demo_playability: label.demo,
            release_date_confidence: label.date_confidence,
            release_proximity: label.freshness,
            studio_prior: label.studio_prior,
            longevity: label.longevity,
            maintenance_health: label.maintenance,
            risk: label.risk,
            personal_fit: label.personal_fit,
            ..Default::default()
        },
        None,
    )
    .relevance_score
}

fn section_index(section: FeedSection) -> usize {
    match section {
        FeedSection::RecentRelease => 0,
        FeedSection::Upcoming => 1,
        FeedSection::PopularLegacy => 2,
        FeedSection::ClassicLegacy => 3,
    }
}

fn dot(left: [f64; FEATURE_COUNT], right: [f64; FEATURE_COUNT]) -> f64 {
    left.into_iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}

fn print_text(report: &GoldenEvaluationReport) {
    println!("schema_version={}", report.schema_version);
    println!("labels={}", report.label_count);
    println!("personas={}", report.persona_count);
    println!("games={}", report.game_count);
    println!(
        "current_rule_baseline_version={}",
        report.current_rule_baseline_version
    );
    print_holdout(&report.persona_holdout);
    print_holdout(&report.game_holdout);
    for model in &report.section_models {
        println!(
            "section={} labels={} pairs={} trained={} weights={}",
            model.section,
            model.label_count,
            model.pair_count,
            model.trained,
            serde_json::to_string(&model.weights).unwrap_or_else(|_| "{}".into())
        );
    }
    println!("freeze_eligible={}", report.freeze_eligible);
    if !report.freeze_eligible {
        println!(
            "freeze_blocked=requires both five-fold holdouts to reach NDCG@20 >= {:.2}, pair accuracy >= {:.2}, and >= {:.0}% lift over every baseline",
            MIN_NDCG,
            MIN_PAIR_ACCURACY,
            MIN_RELATIVE_LIFT * 100.0
        );
    }
}

fn print_holdout(report: &CrossValidationReport) {
    let metrics = report.aggregate;
    println!(
        "{} learned_ndcg_at_20={} pair_accuracy={} current_rule_ndcg_at_20={} ccu_ndcg_at_20={} review_ndcg_at_20={}",
        report.strategy,
        display_metric(metrics.learned.ndcg_at_20),
        display_metric(metrics.learned.pair_direction_accuracy),
        display_metric(metrics.current_rule.ndcg_at_20),
        display_metric(metrics.ccu_baseline.ndcg_at_20),
        display_metric(metrics.review_baseline.ndcg_at_20),
    );
}

fn display_metric(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".into(), |value| format!("{value:.4}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(persona: usize, game: usize, relevance: u8) -> GoldenLabel {
        let signal = f64::from(relevance) / 3.0;
        GoldenLabel {
            persona_id: format!("persona-{persona}"),
            app_id: game as u32 + 1,
            section: FeedSection::RecentRelease,
            relevance,
            personal_fit: signal,
            quality: signal,
            activity: 1.0 - signal,
            momentum: 0.5,
            freshness: 0.5,
            demo: 0.0,
            date_confidence: 1.0,
            studio_prior: 0.5,
            longevity: 0.5,
            maintenance: 0.5,
            risk: 1.0 - signal,
            ccu_baseline: 1.0 - signal,
            review_baseline: 1.0 - signal,
        }
    }

    fn matrix(personas: usize, games: usize) -> Vec<GoldenLabel> {
        (0..personas)
            .flat_map(|persona| (0..games).map(move |game| label(persona, game, (game % 4) as u8)))
            .collect()
    }

    #[test]
    fn public_validation_refuses_fewer_than_two_hundred_labels() {
        let dataset = GoldenDataset {
            schema_version: SCHEMA_VERSION.into(),
            labels: matrix(9, 22),
        };
        let error = validate_dataset(&dataset, MIN_PUBLIC_LABELS).unwrap_err();
        assert!(error.contains("at least 200 valid labels"));

        let boundary = GoldenDataset {
            schema_version: SCHEMA_VERSION.into(),
            labels: matrix(10, 20),
        };
        validate_dataset(&boundary, MIN_PUBLIC_LABELS).unwrap();
    }

    #[test]
    fn pairwise_training_obeys_weight_constraints() {
        let labels = matrix(6, 8);
        let refs = labels.iter().collect::<Vec<_>>();
        let pairs = build_pairs(&refs);
        let weights = train_pairwise(&pairs, current_rule_weights(FeedSection::RecentRelease));
        assert!((0.35..=0.55).contains(&weights[PERSONAL]));
        assert!(weights[1..RISK].iter().all(|weight| *weight >= 0.0));
        assert!((weights[..RISK].iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(weights[RISK] <= 0.0);
    }

    #[test]
    fn current_rule_seed_matches_the_compiled_baseline() {
        for section in FeedSection::ALL {
            let mut row = label(0, section_index(section), 2);
            row.section = section;
            let expected = dot(current_rule_weights(section), row.features());
            assert!((current_rule_score(&row) - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn deterministic_folds_are_balanced_and_keep_entities_together() {
        let labels = matrix(11, 12);
        let first = fold_assignments(&labels, Holdout::Persona);
        let second = fold_assignments(&labels, Holdout::Persona);
        assert_eq!(first, second);
        let mut persona_folds = HashMap::new();
        for (label, fold) in labels.iter().zip(first) {
            assert_eq!(
                persona_folds.entry(&label.persona_id).or_insert(fold),
                &fold
            );
        }
        assert_eq!(
            persona_folds
                .values()
                .copied()
                .collect::<HashSet<_>>()
                .len(),
            5
        );
    }

    #[test]
    fn ndcg_and_pair_accuracy_reward_the_correct_direction() {
        let labels = (0..8)
            .map(|game| label(0, game, (game % 4) as u8))
            .collect::<Vec<_>>();
        let refs = labels.iter().collect::<Vec<_>>();
        let good = evaluate_predictor(&refs, |row| f64::from(row.relevance)).report();
        let bad = evaluate_predictor(&refs, |row| -f64::from(row.relevance)).report();
        assert!((good.ndcg_at_20.unwrap() - 1.0).abs() < 1e-12);
        assert!((good.pair_direction_accuracy.unwrap() - 1.0).abs() < 1e-12);
        assert!(bad.ndcg_at_20.unwrap() < good.ndcg_at_20.unwrap());
        assert_eq!(bad.pair_direction_accuracy, Some(0.0));
    }

    #[test]
    fn exact_score_ties_do_not_gain_from_app_id_order() {
        let low_id_high_relevance = label(0, 0, 3);
        let high_id_low_relevance = label(0, 1, 0);
        let first = [(&low_id_high_relevance, 0.5), (&high_id_low_relevance, 0.5)];
        let second = [(&high_id_low_relevance, 0.5), (&low_id_high_relevance, 0.5)];
        assert!((tie_aware_dcg(&first) - tie_aware_dcg(&second)).abs() < 1e-12);
    }

    #[test]
    fn full_evaluation_does_not_freeze_when_current_rule_is_not_beaten() {
        let labels = matrix(10, 20);
        let report = evaluate(&labels).unwrap();
        assert_eq!(report.persona_holdout.folds.len(), 5);
        assert_eq!(report.game_holdout.folds.len(), 5);
        assert!(!report.freeze_eligible);
    }

    #[test]
    fn json_schema_rejects_implicit_missing_features() {
        let source = r#"{
            "schema_version":"recommendation_golden_labels_v1",
            "labels":[{"persona_id":"p","app_id":1,"section":"recent_release","relevance":3}]
        }"#;
        assert!(serde_json::from_str::<GoldenDataset>(source).is_err());
    }
}
