//! Deterministic, read-only recommendation quality audit.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use mpgs_domain::{FeedSection, ModeFamily, RecommendationConfig, UserPreferences};
use mpgs_recommender::{
    ALGORITHM_VERSION, HardConstraints, RankedCandidate, RankingInput, SlotReason, hard_filter,
    rank_feed_configured_with_constraints_and_tie_seed,
};
use mpgs_storage::query::{GameCandidateRow, list_candidates, section_matches};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

const DEFAULT_TOP: usize = 20;
const MIN_INDEX_POOL_SIZE: usize = 10;
const MIN_INDEX_DATA_CONFIDENCE: f64 = 0.45;
const MIN_INDEX_FEATURES: usize = 3;
const QUALITY_GATE_TOP: usize = 20;
const MIN_DISTINCT_TOP_EVIDENCE_VECTORS: usize = 12;
const MAX_CLAMP_RATE: f64 = 0.01;
const MIN_TOP_DISTINCT_INDICES: usize = 12;
const MAX_TOP_INDEX_BUCKET_SHARE: f64 = 0.20;
const MAX_TOP_CROSS_VECTOR_EXACT_TIE: usize = 2;
const MAX_TOP_MODE_SHARE: f64 = 0.60;
const MIN_MMR_INVERSION_INDEX_POINTS: u8 = 3;
const MAX_UNRESOLVED_EVIDENCE_ID_SAMPLES: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuditOptions {
    db_path: PathBuf,
    as_of: String,
    user_id: Option<String>,
    top: usize,
    json: bool,
    strict: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct RecommendationAudit {
    database: String,
    database_mode: &'static str,
    as_of: String,
    cutoff: String,
    algorithm_version: String,
    config_version: String,
    preference_source: String,
    top: usize,
    sections: Vec<SectionAudit>,
    quality_gates: QualityGateSummary,
    limitations: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct SectionAudit {
    section: &'static str,
    funnel: CandidateFunnel,
    scores: ScoreAudit,
    diversity: DiversityAudit,
    evidence: EvidenceAudit,
    quality_gates: Vec<QualityGate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct CandidateFunnel {
    queried: usize,
    section_eligible: usize,
    feedback_eligible: usize,
    hard_filter_eligible: usize,
    ranked: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct ScoreAudit {
    raw_relevance_min: Option<f64>,
    raw_relevance_median: Option<f64>,
    raw_relevance_max: Option<f64>,
    exact_tie_groups: usize,
    exact_tied_items: usize,
    largest_exact_tie: usize,
    raw_rounded_100_tie_groups: usize,
    raw_rounded_100_tied_items: usize,
    largest_raw_rounded_100_tie: usize,
    distinct_raw_rounded_100_scores: usize,
    clamp_count: usize,
    clamp_rate: f64,
    recommendation_index: RecommendationIndexAudit,
    quality_top20_recommendation_index: RecommendationIndexAudit,
    top20_exact_tie_groups: usize,
    top20_exact_tied_items: usize,
    top20_largest_exact_tie: usize,
    top20_distinct_evidence_vectors: usize,
    top20_distinct_visible_evidence_vectors: usize,
    top20_cross_vector_exact_tie_groups: usize,
    top20_largest_cross_vector_exact_tie: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct DiversityAudit {
    gate_top: usize,
    pool_known_mode_families: usize,
    top_known_mode_families: usize,
    top_known_mode_items: usize,
    top_largest_mode: Option<&'static str>,
    top_largest_mode_count: usize,
    top_largest_mode_share: f64,
    mmr_inversion_pairs_over_3_points: usize,
    mmr_inversion_promoted_items: usize,
    mmr_inversion_pairs_with_reason: usize,
    mmr_inversion_pairs_missing_reason: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct EvidenceAudit {
    referenced_count: usize,
    unique_referenced_count: usize,
    resolvable_count: usize,
    unresolved_count: usize,
    resolvability_rate: Option<f64>,
    unresolved_ids: Vec<String>,
    unresolved_ids_truncated: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum GateStatus {
    Pass,
    Fail,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct QualityGate {
    name: &'static str,
    status: GateStatus,
    observed: Option<f64>,
    requirement: &'static str,
    detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct QualityGateFailure {
    section: &'static str,
    gate: &'static str,
    detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct QualityGateSummary {
    strict_compatible: bool,
    evaluated: usize,
    passed: usize,
    failed: usize,
    not_applicable: usize,
    failures: Vec<QualityGateFailure>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct RecommendationIndexAudit {
    pool_size: usize,
    pool_eligible: bool,
    visible_count: usize,
    hidden_count: usize,
    distinct_indices: usize,
    largest_bucket: usize,
    top_count: usize,
    top_visible_count: usize,
    top_hidden_count: usize,
    top_distinct_indices: usize,
    top_largest_bucket: usize,
    top_largest_bucket_share: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct IndexEligibility {
    data_confidence: f64,
    effective_feature_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EffectiveFeatureFingerprint {
    score_component_bits: [u64; 11],
    mode: Option<&'static str>,
    taxonomy_tags: Vec<String>,
    publisher: Option<String>,
    series: Option<String>,
    recommended_min: Option<u8>,
    recommended_max: Option<u8>,
}

pub(crate) fn run_command(args: impl Iterator<Item = String>) -> Result<(), String> {
    let options = parse_options(args)?;
    let audit = audit_database(&options)?;
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&audit).map_err(|error| error.to_string())?
        );
    } else {
        print_text(&audit);
    }
    if options.strict && audit.quality_gates.failed > 0 {
        return Err(strict_failure_message(&audit.quality_gates));
    }
    Ok(())
}

fn parse_options(mut args: impl Iterator<Item = String>) -> Result<AuditOptions, String> {
    let db_path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| audit_usage().to_owned())?;
    let mut as_of = None;
    let mut user_id = None;
    let mut top = DEFAULT_TOP;
    let mut json = false;
    let mut strict = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--as-of" => {
                as_of = Some(
                    args.next()
                        .ok_or_else(|| "--as-of requires YYYY-MM-DD".to_owned())?,
                );
            }
            "--user-id" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--user-id requires a value".to_owned())?;
                if value.trim().is_empty() {
                    return Err("--user-id must not be empty".into());
                }
                user_id = Some(value);
            }
            "--top" => {
                top = args
                    .next()
                    .ok_or_else(|| "--top requires an integer".to_owned())?
                    .parse::<usize>()
                    .map_err(|_| "--top must be an integer".to_owned())?;
                if !(1..=100).contains(&top) {
                    return Err("--top must be between 1 and 100".into());
                }
            }
            "--json" => json = true,
            "--strict" => strict = true,
            _ => return Err(format!("unknown recommendation-audit option: {arg}")),
        }
    }

    let as_of = as_of.ok_or_else(|| "--as-of YYYY-MM-DD is required".to_owned())?;
    if !mpgs_storage::util::is_iso_day(&as_of) {
        return Err("--as-of must be a valid YYYY-MM-DD calendar date".into());
    }
    Ok(AuditOptions {
        db_path,
        as_of,
        user_id,
        top,
        json,
        strict,
    })
}

fn audit_database(options: &AuditOptions) -> Result<RecommendationAudit, String> {
    if !options.db_path.is_file() {
        return Err(format!(
            "database does not exist or is not a file: {}",
            options.db_path.display()
        ));
    }
    let conn = open_read_only(&options.db_path)?;
    let active = mpgs_storage::users::active_algorithm_config(&conn).map_err(err)?;
    let cutoff = cutoff_day(&options.as_of, &active.config)?;
    let (prefs, preference_source, feedback) = load_context(&conn, options.user_id.as_deref())?;
    let feedback_by_app: HashMap<u32, &str> = feedback
        .iter()
        .map(|item| (item.app_id, item.feedback_type.as_str()))
        .collect();
    let play_intent = mpgs_storage::play_intent::all_counts(&conn).map_err(err)?;
    let resolvable_evidence_ids = load_resolvable_evidence_ids(&conn)?;
    let tie_seed = recommendation_tie_seed(options.user_id.as_deref(), &options.as_of);

    let mut sections = Vec::with_capacity(FeedSection::ALL.len());
    for section in FeedSection::ALL {
        sections.push(audit_section(
            &conn,
            section,
            &cutoff,
            &options.as_of,
            &prefs,
            &active.config,
            ALGORITHM_VERSION,
            &feedback_by_app,
            &play_intent,
            &resolvable_evidence_ids,
            options.top,
            tie_seed,
        )?);
    }

    let quality_gates = summarize_quality_gates(&sections);

    Ok(RecommendationAudit {
        database: options.db_path.display().to_string(),
        database_mode: "read_only/query_only",
        as_of: options.as_of.clone(),
        cutoff,
        algorithm_version: ALGORITHM_VERSION.to_owned(),
        config_version: active.version,
        preference_source,
        top: options.top,
        sections,
        quality_gates,
        limitations: vec![
            "NDCG, pairwise relevance, and calibration quality are not evaluated without labeled judgments or attributed outcomes",
            "evidence resolvability covers deterministic rule-explanation IDs exposed by the local evidence API; it does not assess source truthfulness",
            "not_applicable gates are excluded from strict pass/fail counts and must not be interpreted as quality passes",
        ],
    })
}

fn open_read_only(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(err)?;
    conn.pragma_update(None, "query_only", "ON").map_err(err)?;
    let query_only: i64 = conn
        .query_row("PRAGMA query_only", [], |row| row.get(0))
        .map_err(err)?;
    if query_only != 1 {
        return Err("failed to enforce SQLite query_only mode".into());
    }
    Ok(conn)
}

fn load_resolvable_evidence_ids(conn: &Connection) -> Result<HashSet<String>, String> {
    let mut ids = HashSet::new();

    let mut feature_stmt = conn
        .prepare(
            "SELECT app_id, feature_name
             FROM feature_evidence
             WHERE is_active = 1",
        )
        .map_err(err)?;
    let feature_rows = feature_stmt
        .query_map([], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(err)?;
    for row in feature_rows {
        let (app_id, feature) = row.map_err(err)?;
        ids.insert(format!("feature:{feature}:{app_id}"));
    }

    let mut profile_stmt = conn
        .prepare(
            "SELECT app_id, 'private_session'
             FROM multiplayer_profiles WHERE private_session IS NOT NULL
             UNION ALL
             SELECT app_id, 'self_hosted_server'
             FROM multiplayer_profiles WHERE self_hosted_server IS NOT NULL
             UNION ALL
             SELECT app_id, 'online_coop'
             FROM multiplayer_profiles WHERE online_coop IS NOT NULL",
        )
        .map_err(err)?;
    let profile_rows = profile_stmt
        .query_map([], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(err)?;
    for row in profile_rows {
        let (app_id, feature) = row.map_err(err)?;
        ids.insert(format!("feature:{feature}:{app_id}"));
    }

    let mut computed_stmt = conn
        .prepare("SELECT app_id, dominant_mode, service_status FROM multiplayer_profiles")
        .map_err(err)?;
    let computed_rows = computed_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(err)?;
    for row in computed_rows {
        let (app_id, dominant_mode, service_status) = row.map_err(err)?;
        add_computed_profile_evidence_ids(
            &mut ids,
            app_id,
            dominant_mode.as_deref(),
            service_status.as_deref(),
        );
    }

    let mut review_stmt = conn
        .prepare("SELECT DISTINCT app_id FROM review_snapshots")
        .map_err(err)?;
    let review_rows = review_stmt
        .query_map([], |row| row.get::<_, u32>(0))
        .map_err(err)?;
    for app_id in review_rows {
        ids.insert(format!("review:{}:summary", app_id.map_err(err)?));
    }

    Ok(ids)
}

fn add_computed_profile_evidence_ids(
    ids: &mut HashSet<String>,
    app_id: u32,
    dominant_mode: Option<&str>,
    service_status: Option<&str>,
) {
    match dominant_mode.map(ModeFamily::from_alias) {
        Some(ModeFamily::MatchmadePvp) => {
            ids.insert(format!("feature:matchmaking_core:{app_id}"));
        }
        Some(ModeFamily::PublicWorld) => {
            ids.insert(format!("feature:public_world_dependency:{app_id}"));
        }
        _ => {}
    }
    if service_status.is_some_and(|status| !status.trim().is_empty()) {
        ids.insert(format!("feature:service_shutdown_risk:{app_id}"));
    }
}

fn cutoff_day(as_of: &str, config: &RecommendationConfig) -> Result<String, String> {
    let today_days = mpgs_storage::util::iso_day_to_unix_days(as_of)
        .ok_or_else(|| "invalid --as-of date".to_owned())?;
    let cutoff_days = today_days.saturating_sub(i64::from(config.recent_days));
    Ok(mpgs_storage::util::day_utc_from_ms(
        cutoff_days.saturating_mul(86_400_000),
    ))
}

fn load_context(
    conn: &Connection,
    user_id: Option<&str>,
) -> Result<
    (
        UserPreferences,
        String,
        Vec<mpgs_storage::feedback::ActiveFeedback>,
    ),
    String,
> {
    let Some(user_id) = user_id else {
        return Ok((UserPreferences::default(), "default".into(), Vec::new()));
    };
    let prefs = mpgs_storage::users::get_preferences(conn, user_id).map_err(err)?;
    let feedback = mpgs_storage::feedback::list_active_feedback(conn, user_id).map_err(err)?;
    Ok((prefs, format!("user:{user_id}"), feedback))
}

#[allow(clippy::too_many_arguments)]
fn audit_section(
    conn: &Connection,
    section: FeedSection,
    cutoff: &str,
    as_of: &str,
    prefs: &UserPreferences,
    config: &RecommendationConfig,
    algorithm_version: &str,
    feedback_by_app: &HashMap<u32, &str>,
    play_intent: &HashMap<u32, u32>,
    resolvable_evidence_ids: &HashSet<String>,
    top: usize,
    tie_seed: u64,
) -> Result<SectionAudit, String> {
    let rows = list_candidates(
        conn,
        section,
        cutoff,
        as_of,
        &prefs.budget_currency,
        config,
        i64::from(config.candidate_limit),
    )
    .map_err(err)?;
    let queried = rows.len();
    let mut section_eligible = 0;
    let mut feedback_eligible = 0;
    let mut hard_filter_eligible = 0;
    let mut inputs = Vec::new();
    let mut index_eligibility_by_app = HashMap::new();

    for row in rows {
        let signals = row.to_ranking_signals_at(as_of);
        if !section_matches(section, &row, &signals, cutoff, as_of, config) {
            continue;
        }
        section_eligible += 1;
        let feedback = feedback_by_app.get(&row.app_id).copied();
        if matches!(feedback, Some("not_interested" | "party_size_mismatch")) {
            continue;
        }
        feedback_eligible += 1;
        let dominant_mode = row.display_dominant_mode();
        if !hard_filter(
            prefs,
            row.recommended_min,
            row.recommended_max,
            dominant_mode.as_deref(),
            &signals,
            &row.availability(),
        ) {
            continue;
        }
        hard_filter_eligible += 1;
        index_eligibility_by_app.insert(
            row.app_id,
            IndexEligibility {
                data_confidence: signals.data_confidence.clamp(0.0, 1.0),
                effective_feature_count: effective_feature_count(&row),
            },
        );
        inputs.push(ranking_input(
            row,
            signals,
            dominant_mode,
            feedback,
            play_intent,
        ));
    }

    let ranked = rank_feed_configured_with_constraints_and_tie_seed(
        section,
        &inputs,
        prefs,
        &HardConstraints::NONE,
        config,
        algorithm_version,
        tie_seed,
    );
    let score_rows = ranked
        .items
        .iter()
        .map(|item| {
            (
                item.app_id,
                item.score.relevance_score,
                item.score.final_score,
            )
        })
        .collect::<Vec<_>>();
    let feature_fingerprints = ranked
        .items
        .iter()
        .map(|item| (item.app_id, score_feature_fingerprint(item)))
        .collect::<HashMap<_, _>>();
    let scores = summarize_scores(
        &score_rows,
        &index_eligibility_by_app,
        &feature_fingerprints,
        top,
    );
    let diversity = summarize_diversity(&ranked.items, &index_eligibility_by_app);
    let evidence = summarize_evidence(&ranked.items, resolvable_evidence_ids);
    let quality_gates = evaluate_section_gates(&scores, &diversity, &evidence);
    Ok(SectionAudit {
        section: section.as_str(),
        funnel: CandidateFunnel {
            queried,
            section_eligible,
            feedback_eligible,
            hard_filter_eligible,
            ranked: ranked.items.len(),
        },
        scores,
        diversity,
        evidence,
        quality_gates,
    })
}

fn effective_feature_count(row: &GameCandidateRow) -> usize {
    let has_known_multiplayer_dimension = row
        .dominant_mode
        .as_deref()
        .is_some_and(|mode| ModeFamily::from_alias(mode) != ModeFamily::Unknown)
        || row.private_session.is_some()
        || row.online_coop.is_some()
        || row.self_hosted_server.is_some()
        || row.drop_in_out.is_some()
        || row.crossplay.is_some()
        || row.matchmaking_core.is_some()
        || row.public_world_dependency.is_some()
        || row.recommended_min.is_some()
        || row.recommended_max.is_some();
    usize::from(
        row.total_reviews.is_some_and(|reviews| reviews > 0) && row.total_positive.is_some(),
    ) + usize::from(row.latest_ccu.is_some() || row.typical_ccu_7d.is_some())
        + usize::from(row.release_date.is_some())
        + usize::from(has_known_multiplayer_dimension)
        + usize::from(row.has_demo)
}

fn ranking_input(
    row: GameCandidateRow,
    signals: mpgs_domain::RankingSignals,
    dominant_mode: Option<String>,
    feedback: Option<&str>,
    play_intent: &HashMap<u32, u32>,
) -> RankingInput {
    let personal_adjustment =
        feedback_personal_adjustment(feedback, signals.multiplayer.matchmaking_core);
    let availability = row.availability();
    let taxonomy_tags = row.taxonomy_tags.clone();
    let publisher = row.publisher.clone();
    RankingInput {
        app_id: row.app_id,
        name: row.name,
        dominant_mode,
        taxonomy_tags,
        publisher,
        series: None,
        recommended_min: row.recommended_min,
        recommended_max: row.recommended_max,
        availability,
        signals,
        personal_adjustment,
        play_intent_count: play_intent.get(&row.app_id).copied().unwrap_or(0),
    }
}

fn score_feature_fingerprint(item: &RankedCandidate) -> EffectiveFeatureFingerprint {
    let mut taxonomy_tags = item
        .taxonomy_tags
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    taxonomy_tags.sort();
    taxonomy_tags.dedup();
    let normalized_text = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase)
    };
    let score = &item.score;
    EffectiveFeatureFingerprint {
        score_component_bits: [
            score.friend_fit.to_bits(),
            score.group_fit.to_bits(),
            score.mode_fit.to_bits(),
            score.access_fit.to_bits(),
            score.hosting_fit.to_bits(),
            score.session_fit.to_bits(),
            score.quality.to_bits(),
            score.activity.to_bits(),
            score.freshness.to_bits(),
            score.risk.to_bits(),
            item.data_confidence.to_bits(),
        ],
        mode: item
            .dominant_mode
            .as_deref()
            .map(ModeFamily::from_alias)
            .filter(|mode| *mode != ModeFamily::Unknown)
            .map(ModeFamily::as_str),
        taxonomy_tags,
        publisher: normalized_text(&item.publisher),
        series: normalized_text(&item.series),
        recommended_min: item.recommended_min,
        recommended_max: item.recommended_max,
    }
}

fn feedback_personal_adjustment(feedback: Option<&str>, matchmaking_core: f64) -> f64 {
    match feedback {
        Some("like") => 0.20,
        Some("played") => -0.10,
        Some("too_competitive") if matchmaking_core >= 0.5 => -0.30,
        Some("too_competitive" | "hosting_friction") => -0.15,
        _ => 0.0,
    }
}

fn summarize_scores(
    scores: &[(u32, f64, f64)],
    eligibility_by_app: &HashMap<u32, IndexEligibility>,
    feature_fingerprints: &HashMap<u32, EffectiveFeatureFingerprint>,
    top: usize,
) -> ScoreAudit {
    if scores.is_empty() {
        return ScoreAudit {
            raw_relevance_min: None,
            raw_relevance_median: None,
            raw_relevance_max: None,
            exact_tie_groups: 0,
            exact_tied_items: 0,
            largest_exact_tie: 0,
            raw_rounded_100_tie_groups: 0,
            raw_rounded_100_tied_items: 0,
            largest_raw_rounded_100_tie: 0,
            distinct_raw_rounded_100_scores: 0,
            clamp_count: 0,
            clamp_rate: 0.0,
            recommendation_index: summarize_recommendation_indices(scores, eligibility_by_app, top),
            quality_top20_recommendation_index: summarize_recommendation_indices(
                scores,
                eligibility_by_app,
                QUALITY_GATE_TOP,
            ),
            top20_exact_tie_groups: 0,
            top20_exact_tied_items: 0,
            top20_largest_exact_tie: 0,
            top20_distinct_evidence_vectors: 0,
            top20_distinct_visible_evidence_vectors: 0,
            top20_cross_vector_exact_tie_groups: 0,
            top20_largest_cross_vector_exact_tie: 0,
        };
    }

    let raw_scores: Vec<_> = scores.iter().map(|(_, relevance, _)| *relevance).collect();
    let mut sorted = raw_scores.clone();
    sorted.sort_by(f64::total_cmp);
    let median = if sorted.len().is_multiple_of(2) {
        (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2]) / 2.0
    } else {
        sorted[sorted.len() / 2]
    };
    let exact_counts = frequencies(raw_scores.iter().map(|score| score.to_bits()));
    let rounded_counts = frequencies(raw_scores.iter().map(|score| raw_rounded_100(*score)));
    let (exact_tie_groups, exact_tied_items, largest_exact_tie) = tie_stats(&exact_counts);
    let (raw_rounded_100_tie_groups, raw_rounded_100_tied_items, largest_raw_rounded_100_tie) =
        tie_stats(&rounded_counts);
    let clamp_count = scores
        .iter()
        .filter(|(_, relevance, _)| !relevance.is_finite() || !(0.0..=1.0).contains(relevance))
        .count();
    let top20 = &scores[..scores.len().min(QUALITY_GATE_TOP)];
    let top20_exact_counts = frequencies(top20.iter().map(|(_, score, _)| score.to_bits()));
    let (top20_exact_tie_groups, top20_exact_tied_items, top20_largest_exact_tie) =
        tie_stats(&top20_exact_counts);
    let top20_distinct_evidence_vectors = top20
        .iter()
        .filter_map(|(app_id, _, _)| feature_fingerprints.get(app_id))
        .collect::<HashSet<_>>()
        .len();
    let top20_indices = context_percentile_indices(
        top20
            .iter()
            .map(|(app_id, relevance, _)| (*app_id, *relevance)),
    );
    let top20_distinct_visible_evidence_vectors = top20
        .iter()
        .filter(|(app_id, _, _)| {
            visible_recommendation_index(*app_id, &top20_indices, eligibility_by_app).is_some()
        })
        .filter_map(|(app_id, _, _)| feature_fingerprints.get(app_id))
        .collect::<HashSet<_>>()
        .len();
    let mut vectors_by_score = BTreeMap::<u64, HashSet<&EffectiveFeatureFingerprint>>::new();
    for (app_id, relevance, _) in top20 {
        if visible_recommendation_index(*app_id, &top20_indices, eligibility_by_app).is_none() {
            continue;
        }
        if let Some(fingerprint) = feature_fingerprints.get(app_id) {
            vectors_by_score
                .entry(relevance.to_bits())
                .or_default()
                .insert(fingerprint);
        }
    }
    let cross_vector_counts = vectors_by_score
        .into_iter()
        .map(|(score, fingerprints)| (score, fingerprints.len()))
        .collect::<BTreeMap<_, _>>();
    let (top20_cross_vector_exact_tie_groups, _, top20_largest_cross_vector_exact_tie) =
        tie_stats(&cross_vector_counts);

    ScoreAudit {
        raw_relevance_min: sorted.first().copied(),
        raw_relevance_median: Some(median),
        raw_relevance_max: sorted.last().copied(),
        exact_tie_groups,
        exact_tied_items,
        largest_exact_tie,
        raw_rounded_100_tie_groups,
        raw_rounded_100_tied_items,
        largest_raw_rounded_100_tie,
        distinct_raw_rounded_100_scores: rounded_counts.len(),
        clamp_count,
        clamp_rate: clamp_count as f64 / scores.len() as f64,
        recommendation_index: summarize_recommendation_indices(scores, eligibility_by_app, top),
        quality_top20_recommendation_index: summarize_recommendation_indices(
            scores,
            eligibility_by_app,
            QUALITY_GATE_TOP,
        ),
        top20_exact_tie_groups,
        top20_exact_tied_items,
        top20_largest_exact_tie,
        top20_distinct_evidence_vectors,
        top20_distinct_visible_evidence_vectors,
        top20_cross_vector_exact_tie_groups,
        top20_largest_cross_vector_exact_tie,
    }
}

fn summarize_recommendation_indices(
    scores: &[(u32, f64, f64)],
    eligibility_by_app: &HashMap<u32, IndexEligibility>,
    top: usize,
) -> RecommendationIndexAudit {
    let window = &scores[..scores.len().min(top)];
    let indices = context_percentile_indices(
        window
            .iter()
            .map(|(app_id, relevance, _)| (*app_id, *relevance)),
    );
    let visible_indices: Vec<_> = window
        .iter()
        .filter_map(|(app_id, _, _)| {
            visible_recommendation_index(*app_id, &indices, eligibility_by_app)
        })
        .collect();
    let top_count = window.len();
    let top_indices: Vec<_> = window
        .iter()
        .filter_map(|(app_id, _, _)| {
            visible_recommendation_index(*app_id, &indices, eligibility_by_app)
        })
        .collect();
    let all_counts = frequencies(visible_indices.iter().copied());
    let top_counts = frequencies(top_indices.iter().copied());
    let largest_bucket = all_counts.values().copied().max().unwrap_or(0);
    let top_largest_bucket = top_counts.values().copied().max().unwrap_or(0);
    let top_visible_count = top_indices.len();

    RecommendationIndexAudit {
        pool_size: window.len(),
        pool_eligible: window.len() >= MIN_INDEX_POOL_SIZE,
        visible_count: visible_indices.len(),
        hidden_count: window.len().saturating_sub(visible_indices.len()),
        distinct_indices: all_counts.len(),
        largest_bucket,
        top_count,
        top_visible_count,
        top_hidden_count: top_count.saturating_sub(top_visible_count),
        top_distinct_indices: top_counts.len(),
        top_largest_bucket,
        top_largest_bucket_share: if top_visible_count == 0 {
            0.0
        } else {
            top_largest_bucket as f64 / top_visible_count as f64
        },
    }
}

fn summarize_diversity(
    items: &[RankedCandidate],
    eligibility_by_app: &HashMap<u32, IndexEligibility>,
) -> DiversityAudit {
    let known_mode = |item: &RankedCandidate| {
        item.dominant_mode
            .as_deref()
            .map(ModeFamily::from_alias)
            .filter(|mode| *mode != ModeFamily::Unknown)
    };
    let pool_modes = items.iter().filter_map(known_mode).collect::<HashSet<_>>();
    let top = &items[..items.len().min(QUALITY_GATE_TOP)];
    let mut top_mode_counts = BTreeMap::<&'static str, usize>::new();
    for mode in top.iter().filter_map(known_mode) {
        *top_mode_counts.entry(mode.as_str()).or_default() += 1;
    }
    let top_known_mode_items = top_mode_counts.values().sum();
    let (top_largest_mode, top_largest_mode_count) = top_mode_counts
        .iter()
        .max_by(|(left_mode, left_count), (right_mode, right_count)| {
            left_count
                .cmp(right_count)
                .then_with(|| right_mode.cmp(left_mode))
        })
        .map_or((None, 0), |(mode, count)| (Some(*mode), *count));

    let score_rows = top
        .iter()
        .map(|item| (item.app_id, item.score.relevance_score))
        .collect::<Vec<_>>();
    let indices = context_percentile_indices(score_rows);
    let mut inversion_pairs = 0usize;
    let mut inversion_pairs_with_reason = 0usize;
    let mut inversion_pairs_missing_reason = 0usize;
    let mut promoted_items = HashSet::new();
    for (left_index, promoted) in top.iter().enumerate() {
        let Some(promoted_score) =
            visible_recommendation_index(promoted.app_id, &indices, eligibility_by_app)
        else {
            continue;
        };
        for stronger in &top[left_index + 1..] {
            let Some(stronger_score) =
                visible_recommendation_index(stronger.app_id, &indices, eligibility_by_app)
            else {
                continue;
            };
            if stronger_score.saturating_sub(promoted_score) <= MIN_MMR_INVERSION_INDEX_POINTS {
                continue;
            }
            inversion_pairs += 1;
            promoted_items.insert(promoted.app_id);
            if matches!(
                promoted.slot_reason,
                SlotReason::Diversity | SlotReason::Explore
            ) {
                inversion_pairs_with_reason += 1;
            } else {
                inversion_pairs_missing_reason += 1;
            }
        }
    }

    DiversityAudit {
        gate_top: QUALITY_GATE_TOP,
        pool_known_mode_families: pool_modes.len(),
        top_known_mode_families: top_mode_counts.len(),
        top_known_mode_items,
        top_largest_mode,
        top_largest_mode_count,
        top_largest_mode_share: if top.is_empty() {
            0.0
        } else {
            top_largest_mode_count as f64 / top.len() as f64
        },
        mmr_inversion_pairs_over_3_points: inversion_pairs,
        mmr_inversion_promoted_items: promoted_items.len(),
        mmr_inversion_pairs_with_reason: inversion_pairs_with_reason,
        mmr_inversion_pairs_missing_reason: inversion_pairs_missing_reason,
    }
}

fn summarize_evidence(
    items: &[RankedCandidate],
    resolvable_evidence_ids: &HashSet<String>,
) -> EvidenceAudit {
    let referenced_count = items
        .iter()
        .map(|item| item.explanation.evidence_ids.len())
        .sum();
    let referenced = items
        .iter()
        .flat_map(|item| item.explanation.evidence_ids.iter().cloned())
        .collect::<HashSet<_>>();
    let mut unresolved_ids = referenced
        .difference(resolvable_evidence_ids)
        .cloned()
        .collect::<Vec<_>>();
    unresolved_ids.sort();
    let unique_referenced_count = referenced.len();
    let unresolved_count = unresolved_ids.len();
    let unresolved_ids_truncated =
        unresolved_count.saturating_sub(MAX_UNRESOLVED_EVIDENCE_ID_SAMPLES);
    unresolved_ids.truncate(MAX_UNRESOLVED_EVIDENCE_ID_SAMPLES);
    let resolvable_count = unique_referenced_count.saturating_sub(unresolved_count);
    EvidenceAudit {
        referenced_count,
        unique_referenced_count,
        resolvable_count,
        unresolved_count,
        resolvability_rate: (unique_referenced_count > 0)
            .then_some(resolvable_count as f64 / unique_referenced_count as f64),
        unresolved_ids,
        unresolved_ids_truncated,
    }
}

fn evaluate_section_gates(
    scores: &ScoreAudit,
    diversity: &DiversityAudit,
    evidence: &EvidenceAudit,
) -> Vec<QualityGate> {
    let top20_indices = &scores.quality_top20_recommendation_index;
    let full_top20 = top20_indices.top_count == QUALITY_GATE_TOP;
    let distinct_evidence =
        scores.top20_distinct_visible_evidence_vectors >= MIN_DISTINCT_TOP_EVIDENCE_VECTORS;
    let enough_visible_indices = top20_indices.top_visible_count >= MIN_TOP_DISTINCT_INDICES;
    let index_gate_applicable = full_top20 && distinct_evidence && enough_visible_indices;
    let exact_tie_gate_applicable = full_top20 && distinct_evidence;

    vec![
        gate(
            "clamp_rate",
            scores.recommendation_index.pool_size > 0,
            scores.clamp_rate <= MAX_CLAMP_RATE,
            Some(scores.clamp_rate),
            "<= 0.01",
            format!(
                "{} of {} ranked scores are clamped",
                scores.clamp_count, scores.recommendation_index.pool_size
            ),
        ),
        gate(
            "top20_distinct_recommendation_indices",
            index_gate_applicable,
            top20_indices.top_distinct_indices >= MIN_TOP_DISTINCT_INDICES,
            Some(top20_indices.top_distinct_indices as f64),
            ">= 12 when Top20 has >=12 distinct effective evidence vectors and >=12 visible indices",
            format!(
                "visible={}, distinct_vectors={}, distinct_visible_vectors={}, distinct_indices={}",
                top20_indices.top_visible_count,
                scores.top20_distinct_evidence_vectors,
                scores.top20_distinct_visible_evidence_vectors,
                top20_indices.top_distinct_indices
            ),
        ),
        gate(
            "top20_largest_recommendation_index_bucket_share",
            index_gate_applicable,
            top20_indices.top_largest_bucket_share <= MAX_TOP_INDEX_BUCKET_SHARE,
            Some(top20_indices.top_largest_bucket_share),
            "<= 0.20 when Top20 has >=12 distinct effective evidence vectors and >=12 visible indices",
            format!(
                "largest_bucket={} of {} visible",
                top20_indices.top_largest_bucket, top20_indices.top_visible_count
            ),
        ),
        gate(
            "top20_cross_vector_exact_tie_size",
            exact_tie_gate_applicable,
            scores.top20_largest_cross_vector_exact_tie <= MAX_TOP_CROSS_VECTOR_EXACT_TIE,
            Some(scores.top20_largest_cross_vector_exact_tie as f64),
            "<= 2 distinct effective evidence vectors per exact-score tie",
            format!(
                "exact_tie_groups={}, largest_exact_tie={}, cross_vector_groups={}, largest_cross_vector_tie={}",
                scores.top20_exact_tie_groups,
                scores.top20_largest_exact_tie,
                scores.top20_cross_vector_exact_tie_groups,
                scores.top20_largest_cross_vector_exact_tie
            ),
        ),
        gate(
            "top20_single_mode_share",
            full_top20 && diversity.pool_known_mode_families >= 3,
            diversity.top_largest_mode_share <= MAX_TOP_MODE_SHARE,
            Some(diversity.top_largest_mode_share),
            "<= 0.60 when the ranked pool contains >=3 known mode families",
            format!(
                "pool_modes={}, top_largest_mode={}, count={}/{}",
                diversity.pool_known_mode_families,
                diversity.top_largest_mode.unwrap_or("n/a"),
                diversity.top_largest_mode_count,
                top20_indices.top_count
            ),
        ),
        gate(
            "mmr_inversion_slot_reason",
            top20_indices.top_visible_count >= 2,
            diversity.mmr_inversion_pairs_missing_reason == 0,
            Some(diversity.mmr_inversion_pairs_missing_reason as f64),
            "0 inversions over 3 recommendation-index points without diversity/explore slot_reason",
            format!(
                "inversion_pairs={}, promoted_items={}, justified_pairs={}, missing_reason_pairs={}",
                diversity.mmr_inversion_pairs_over_3_points,
                diversity.mmr_inversion_promoted_items,
                diversity.mmr_inversion_pairs_with_reason,
                diversity.mmr_inversion_pairs_missing_reason
            ),
        ),
        gate(
            "evidence_id_resolvability",
            evidence.unique_referenced_count > 0,
            evidence.unresolved_count == 0,
            evidence.resolvability_rate,
            "100% of referenced deterministic explanation evidence IDs resolve locally",
            if evidence.unresolved_ids.is_empty() {
                format!(
                    "resolved {}/{} unique referenced IDs",
                    evidence.resolvable_count, evidence.unique_referenced_count
                )
            } else {
                format!(
                    "unresolved IDs: {}{}",
                    evidence.unresolved_ids.join(", "),
                    if evidence.unresolved_ids_truncated == 0 {
                        String::new()
                    } else {
                        format!(" (and {} more)", evidence.unresolved_ids_truncated)
                    }
                )
            },
        ),
    ]
}

fn gate(
    name: &'static str,
    applicable: bool,
    passed: bool,
    observed: Option<f64>,
    requirement: &'static str,
    detail: String,
) -> QualityGate {
    QualityGate {
        name,
        status: if !applicable {
            GateStatus::NotApplicable
        } else if passed {
            GateStatus::Pass
        } else {
            GateStatus::Fail
        },
        observed: applicable.then_some(observed).flatten(),
        requirement,
        detail,
    }
}

fn summarize_quality_gates(sections: &[SectionAudit]) -> QualityGateSummary {
    let mut evaluated = 0usize;
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut not_applicable = 0usize;
    let mut failures = Vec::new();
    for section in sections {
        for gate in &section.quality_gates {
            match gate.status {
                GateStatus::Pass => {
                    evaluated += 1;
                    passed += 1;
                }
                GateStatus::Fail => {
                    evaluated += 1;
                    failed += 1;
                    failures.push(QualityGateFailure {
                        section: section.section,
                        gate: gate.name,
                        detail: gate.detail.clone(),
                    });
                }
                GateStatus::NotApplicable => not_applicable += 1,
            }
        }
    }
    QualityGateSummary {
        strict_compatible: failed == 0,
        evaluated,
        passed,
        failed,
        not_applicable,
        failures,
    }
}

fn strict_failure_message(summary: &QualityGateSummary) -> String {
    let failures = summary
        .failures
        .iter()
        .map(|failure| format!("{}/{} ({})", failure.section, failure.gate, failure.detail))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "recommendation audit failed {} deterministic quality gate(s): {failures}",
        summary.failed
    )
}

fn context_percentile_indices(scores: impl IntoIterator<Item = (u32, f64)>) -> HashMap<u32, u8> {
    let mut scores: Vec<_> = scores
        .into_iter()
        .filter(|(_, score)| score.is_finite())
        .collect();
    if scores.len() < MIN_INDEX_POOL_SIZE {
        return HashMap::new();
    }

    scores.sort_by(|(left_id, left), (right_id, right)| {
        right.total_cmp(left).then_with(|| left_id.cmp(right_id))
    });
    let total = scores.len() as f64;
    let mut indices = HashMap::with_capacity(scores.len());
    let mut start = 0usize;
    while start < scores.len() {
        let score = scores[start].1;
        let mut end = start + 1;
        while end < scores.len() && scores[end].1.total_cmp(&score).is_eq() {
            end += 1;
        }
        let midrank = ((start + 1) as f64 + end as f64) / 2.0;
        let index = (100.0 * (total - midrank + 0.5) / total)
            .round()
            .clamp(0.0, 100.0) as u8;
        for (app_id, _) in &scores[start..end] {
            indices.insert(*app_id, index);
        }
        start = end;
    }
    indices
}

fn visible_recommendation_index(
    app_id: u32,
    indices: &HashMap<u32, u8>,
    eligibility_by_app: &HashMap<u32, IndexEligibility>,
) -> Option<u8> {
    let eligibility = eligibility_by_app.get(&app_id)?;
    (eligibility.data_confidence >= MIN_INDEX_DATA_CONFIDENCE
        && eligibility.effective_feature_count >= MIN_INDEX_FEATURES)
        .then(|| indices.get(&app_id).copied())
        .flatten()
}

fn frequencies<T: Ord>(values: impl Iterator<Item = T>) -> BTreeMap<T, usize> {
    let mut counts = BTreeMap::new();
    for value in values {
        *counts.entry(value).or_insert(0) += 1;
    }
    counts
}

fn tie_stats<T: Ord>(counts: &BTreeMap<T, usize>) -> (usize, usize, usize) {
    let mut groups = 0;
    let mut items = 0;
    let mut largest = 0;
    for count in counts.values().copied().filter(|count| *count > 1) {
        groups += 1;
        items += count;
        largest = largest.max(count);
    }
    (groups, items, largest)
}

fn raw_rounded_100(score: f64) -> i64 {
    (score * 100.0).round() as i64
}

fn print_text(audit: &RecommendationAudit) {
    println!("database={}", audit.database);
    println!("database_mode={}", audit.database_mode);
    println!("as_of={}", audit.as_of);
    println!("cutoff={}", audit.cutoff);
    println!("algorithm_version={}", audit.algorithm_version);
    println!("config_version={}", audit.config_version);
    println!("preference_source={}", audit.preference_source);
    println!("top={}", audit.top);
    for section in &audit.sections {
        let funnel = section.funnel;
        let scores = &section.scores;
        println!("section={}", section.section);
        println!(
            "  funnel queried={} section_eligible={} feedback_eligible={} hard_filter_eligible={} ranked={}",
            funnel.queried,
            funnel.section_eligible,
            funnel.feedback_eligible,
            funnel.hard_filter_eligible,
            funnel.ranked
        );
        println!(
            "  raw_relevance min={} median={} max={}",
            format_score(scores.raw_relevance_min),
            format_score(scores.raw_relevance_median),
            format_score(scores.raw_relevance_max)
        );
        println!(
            "  exact_ties groups={} items={} largest={}",
            scores.exact_tie_groups, scores.exact_tied_items, scores.largest_exact_tie
        );
        println!(
            "  raw_rounded_100_ties groups={} items={} largest={} distinct={}",
            scores.raw_rounded_100_tie_groups,
            scores.raw_rounded_100_tied_items,
            scores.largest_raw_rounded_100_tie,
            scores.distinct_raw_rounded_100_scores,
        );
        println!(
            "  clamp count={} rate={:.2}%",
            scores.clamp_count,
            scores.clamp_rate * 100.0
        );
        let indices = &scores.recommendation_index;
        println!(
            "  recommendation_index pool={} eligible={} visible={} hidden={} distinct={} largest_bucket={}",
            indices.pool_size,
            indices.pool_eligible,
            indices.visible_count,
            indices.hidden_count,
            indices.distinct_indices,
            indices.largest_bucket,
        );
        println!(
            "  top{}_recommendation_index count={} visible={} hidden={} distinct={} largest_bucket={} largest_bucket_share={:.2}%",
            audit.top,
            indices.top_count,
            indices.top_visible_count,
            indices.top_hidden_count,
            indices.top_distinct_indices,
            indices.top_largest_bucket,
            indices.top_largest_bucket_share * 100.0,
        );
        println!(
            "  top20_exact_ties groups={} items={} largest={} distinct_evidence_vectors={} distinct_visible_evidence_vectors={} cross_vector_groups={} largest_cross_vector_tie={}",
            scores.top20_exact_tie_groups,
            scores.top20_exact_tied_items,
            scores.top20_largest_exact_tie,
            scores.top20_distinct_evidence_vectors,
            scores.top20_distinct_visible_evidence_vectors,
            scores.top20_cross_vector_exact_tie_groups,
            scores.top20_largest_cross_vector_exact_tie,
        );
        let gate_indices = &scores.quality_top20_recommendation_index;
        println!(
            "  quality_top20_recommendation_index count={} visible={} hidden={} distinct={} largest_bucket={} largest_bucket_share={:.2}%",
            gate_indices.top_count,
            gate_indices.top_visible_count,
            gate_indices.top_hidden_count,
            gate_indices.top_distinct_indices,
            gate_indices.top_largest_bucket,
            gate_indices.top_largest_bucket_share * 100.0,
        );
        println!(
            "  diversity pool_modes={} top_modes={} top_known_items={} largest_mode={} largest_mode_count={} largest_mode_share={:.2}%",
            section.diversity.pool_known_mode_families,
            section.diversity.top_known_mode_families,
            section.diversity.top_known_mode_items,
            section.diversity.top_largest_mode.unwrap_or("n/a"),
            section.diversity.top_largest_mode_count,
            section.diversity.top_largest_mode_share * 100.0,
        );
        println!(
            "  mmr_inversions over_3_points={} promoted_items={} justified_pairs={} missing_reason_pairs={}",
            section.diversity.mmr_inversion_pairs_over_3_points,
            section.diversity.mmr_inversion_promoted_items,
            section.diversity.mmr_inversion_pairs_with_reason,
            section.diversity.mmr_inversion_pairs_missing_reason,
        );
        println!(
            "  evidence referenced={} unique={} resolvable={} unresolved={} unresolved_samples_truncated={} rate={}",
            section.evidence.referenced_count,
            section.evidence.unique_referenced_count,
            section.evidence.resolvable_count,
            section.evidence.unresolved_count,
            section.evidence.unresolved_ids_truncated,
            section
                .evidence
                .resolvability_rate
                .map_or_else(|| "n/a".into(), |rate| format!("{:.2}%", rate * 100.0)),
        );
        for gate in &section.quality_gates {
            println!(
                "  gate {} status={} requirement={} detail={}",
                gate.name,
                gate_status_str(gate.status),
                gate.requirement,
                gate.detail,
            );
        }
    }
    println!(
        "quality_gates strict_compatible={} evaluated={} passed={} failed={} not_applicable={}",
        audit.quality_gates.strict_compatible,
        audit.quality_gates.evaluated,
        audit.quality_gates.passed,
        audit.quality_gates.failed,
        audit.quality_gates.not_applicable,
    );
    for limitation in &audit.limitations {
        println!("limitation={limitation}");
    }
}

fn gate_status_str(status: GateStatus) -> &'static str {
    match status {
        GateStatus::Pass => "pass",
        GateStatus::Fail => "fail",
        GateStatus::NotApplicable => "not_applicable",
    }
}

fn format_score(score: Option<f64>) -> String {
    score.map_or_else(|| "n/a".into(), |score| format!("{score:.6}"))
}

fn audit_usage() -> &'static str {
    "recommendation-audit <db-path> --as-of YYYY-MM-DD [--user-id ID] [--top N] [--json] [--strict]"
}

fn recommendation_tie_seed(user_id: Option<&str>, utc_day: &str) -> u64 {
    let payload = format!(
        "recommendation-tie-v1|{}|{utc_day}",
        user_id.unwrap_or("public")
    );
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in payload.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mpgs_recommender::{Explanation, ScoreBreakdown};

    fn ranked_candidate(
        app_id: u32,
        relevance: f64,
        mode: &str,
        slot_reason: SlotReason,
        evidence_ids: &[&str],
    ) -> RankedCandidate {
        RankedCandidate {
            app_id,
            name: format!("game-{app_id}"),
            dominant_mode: Some(mode.into()),
            taxonomy_tags: vec![mode.into()],
            publisher: Some(format!("publisher-{}", app_id % 3)),
            series: None,
            recommended_min: Some(2),
            recommended_max: Some(4),
            data_confidence: 0.8,
            score: ScoreBreakdown {
                friend_fit: 0.6 + f64::from(app_id) / 10_000.0,
                section_score: relevance,
                personalized_score: relevance,
                group_fit: 0.8,
                mode_fit: 0.7,
                access_fit: 0.6,
                hosting_fit: 0.5,
                session_fit: 0.4,
                quality: 0.75 + f64::from(app_id) / 10_000.0,
                activity: 0.6,
                freshness: 0.5,
                risk: 0.1,
                relevance_score: relevance,
                final_score: relevance.clamp(0.0, 1.0),
            },
            explanation: Explanation {
                reasons: vec!["reason".into()],
                cautions: Vec::new(),
                evidence_ids: evidence_ids.iter().map(|value| (*value).into()).collect(),
            },
            slot_reason,
            algorithm_version: "test".into(),
        }
    }

    fn visible_eligibility(items: &[RankedCandidate]) -> HashMap<u32, IndexEligibility> {
        items
            .iter()
            .map(|item| {
                (
                    item.app_id,
                    IndexEligibility {
                        data_confidence: 0.8,
                        effective_feature_count: 3,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn options_require_a_valid_explicit_as_of_day() {
        assert!(parse_options(["data.db".into()].into_iter()).is_err());
        assert!(
            parse_options(["data.db".into(), "--as-of".into(), "2026-02-29".into()].into_iter())
                .is_err()
        );
        let parsed = parse_options(
            [
                "data.db".into(),
                "--as-of".into(),
                "2026-08-01".into(),
                "--top".into(),
                "12".into(),
                "--json".into(),
                "--strict".into(),
            ]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(parsed.as_of, "2026-08-01");
        assert_eq!(parsed.top, 12);
        assert!(parsed.json);
        assert!(parsed.strict);
    }

    #[test]
    fn score_summary_reports_raw_exact_rounded_and_clamp_stats() {
        let scores = [
            (1, 0.611_3, 0.611_3),
            (2, 0.611_3, 0.611_3),
            (3, 0.612_2, 0.612_2),
            (4, 0.781, 0.781),
            (5, 1.2, 1.0),
        ];
        let summary = summarize_scores(&scores, &HashMap::new(), &HashMap::new(), 4);
        assert_eq!(summary.raw_relevance_min, Some(0.611_3));
        assert_eq!(summary.raw_relevance_median, Some(0.612_2));
        assert_eq!(summary.raw_relevance_max, Some(1.2));
        assert_eq!(summary.exact_tie_groups, 1);
        assert_eq!(summary.exact_tied_items, 2);
        assert_eq!(summary.largest_exact_tie, 2);
        assert_eq!(summary.raw_rounded_100_tie_groups, 1);
        assert_eq!(summary.raw_rounded_100_tied_items, 3);
        assert_eq!(summary.largest_raw_rounded_100_tie, 3);
        assert_eq!(summary.distinct_raw_rounded_100_scores, 3);
        assert_eq!(summary.clamp_count, 1);
        assert_eq!(summary.clamp_rate, 0.2);
        assert!(!summary.recommendation_index.pool_eligible);
        assert_eq!(summary.recommendation_index.top_visible_count, 0);
        assert_eq!(summary.recommendation_index.top_hidden_count, 4);
    }

    #[test]
    fn clamp_rate_counts_out_of_range_raw_relevance_not_exact_boundaries() {
        let scores = [(1, 0.0, 0.0), (2, 1.0, 1.0), (3, -0.1, 0.0), (4, 1.1, 1.0)];
        let summary = summarize_scores(&scores, &HashMap::new(), &HashMap::new(), DEFAULT_TOP);
        assert_eq!(summary.clamp_count, 2);
        assert_eq!(summary.clamp_rate, 0.5);
    }

    #[test]
    fn context_percentile_uses_midrank_for_exact_ties() {
        let indices = context_percentile_indices([
            (1, 0.9),
            (2, 0.9),
            (3, 0.8),
            (4, 0.7),
            (5, 0.6),
            (6, 0.5),
            (7, 0.4),
            (8, 0.3),
            (9, 0.2),
            (10, 0.1),
        ]);
        assert_eq!(indices.get(&1), Some(&90));
        assert_eq!(indices.get(&2), Some(&90));
        assert_eq!(indices.get(&3), Some(&75));
        assert_eq!(indices.get(&10), Some(&5));
    }

    #[test]
    fn recommendation_index_summary_applies_public_visibility_rules() {
        let scores = [
            (1, 0.9, 0.9),
            (2, 0.9, 0.9),
            (3, 0.8, 0.8),
            (4, 0.7, 0.7),
            (5, 0.6, 0.6),
            (6, 0.5, 0.5),
            (7, 0.4, 0.4),
            (8, 0.3, 0.3),
            (9, 0.2, 0.2),
            (10, 0.1, 0.1),
        ];
        let mut eligibility = HashMap::new();
        for app_id in 1..=10 {
            eligibility.insert(
                app_id,
                IndexEligibility {
                    data_confidence: 0.8,
                    effective_feature_count: 3,
                },
            );
        }
        eligibility.get_mut(&3).unwrap().data_confidence = 0.44;
        eligibility.get_mut(&4).unwrap().effective_feature_count = 2;

        let summary =
            summarize_scores(&scores, &eligibility, &HashMap::new(), 10).recommendation_index;
        assert!(summary.pool_eligible);
        assert_eq!(summary.visible_count, 8);
        assert_eq!(summary.hidden_count, 2);
        assert_eq!(summary.distinct_indices, 7);
        assert_eq!(summary.largest_bucket, 2);
        assert_eq!(summary.top_count, 10);
        assert_eq!(summary.top_visible_count, 8);
        assert_eq!(summary.top_hidden_count, 2);
        assert_eq!(summary.top_distinct_indices, 7);
        assert_eq!(summary.top_largest_bucket, 2);
        assert_eq!(summary.top_largest_bucket_share, 0.25);
    }

    #[test]
    fn recommendation_index_summary_uses_the_returned_request_window() {
        let scores = (1_u32..=524)
            .map(|app_id| {
                let score = 1.0 - f64::from(app_id) / 1_000.0;
                (app_id, score, score)
            })
            .collect::<Vec<_>>();
        let eligibility = scores
            .iter()
            .map(|(app_id, _, _)| {
                (
                    *app_id,
                    IndexEligibility {
                        data_confidence: 0.8,
                        effective_feature_count: 3,
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        let summary = summarize_recommendation_indices(&scores, &eligibility, 20);

        assert_eq!(summary.pool_size, 20);
        assert_eq!(summary.visible_count, 20);
        assert_eq!(summary.distinct_indices, 20);
        assert_eq!(summary.top_largest_bucket_share, 0.05);
    }

    #[test]
    fn top20_exact_ties_distinguish_identical_scores_from_distinct_evidence_vectors() {
        let mut items = (1..=20)
            .map(|app_id| {
                ranked_candidate(
                    app_id,
                    1.0 - f64::from(app_id) / 100.0,
                    "private_coop",
                    SlotReason::Base,
                    &[],
                )
            })
            .collect::<Vec<_>>();
        for item in &mut items[..3] {
            item.score.relevance_score = 0.99;
            item.score.final_score = 0.99;
        }
        let scores = items
            .iter()
            .map(|item| {
                (
                    item.app_id,
                    item.score.relevance_score,
                    item.score.final_score,
                )
            })
            .collect::<Vec<_>>();
        let fingerprints = items
            .iter()
            .map(|item| (item.app_id, score_feature_fingerprint(item)))
            .collect();
        let summary = summarize_scores(
            &scores,
            &visible_eligibility(&items),
            &fingerprints,
            DEFAULT_TOP,
        );

        assert_eq!(summary.top20_exact_tie_groups, 1);
        assert_eq!(summary.top20_exact_tied_items, 3);
        assert_eq!(summary.top20_largest_exact_tie, 3);
        assert_eq!(summary.top20_distinct_evidence_vectors, 20);
        assert_eq!(summary.top20_distinct_visible_evidence_vectors, 20);
        assert_eq!(summary.top20_cross_vector_exact_tie_groups, 1);
        assert_eq!(summary.top20_largest_cross_vector_exact_tie, 3);
    }

    #[test]
    fn diversity_audit_requires_reasons_for_large_visible_mmr_inversions() {
        let mut items = Vec::new();
        items.push(ranked_candidate(
            20,
            0.20,
            "private_coop",
            SlotReason::Diversity,
            &[],
        ));
        for app_id in 1..=19 {
            let mode = match app_id {
                1..=11 => "private_coop",
                12..=15 => "matchmade_pvp",
                _ => "public_world",
            };
            items.push(ranked_candidate(
                app_id,
                1.0 - f64::from(app_id) / 100.0,
                mode,
                SlotReason::Base,
                &[],
            ));
        }
        let eligibility = visible_eligibility(&items);
        let diversity = summarize_diversity(&items, &eligibility);
        assert_eq!(diversity.pool_known_mode_families, 3);
        assert_eq!(diversity.top_largest_mode_count, 12);
        assert_eq!(diversity.top_largest_mode_share, 0.6);
        assert!(diversity.mmr_inversion_pairs_over_3_points > 0);
        assert_eq!(diversity.mmr_inversion_pairs_missing_reason, 0);
        assert_eq!(
            diversity.mmr_inversion_pairs_with_reason,
            diversity.mmr_inversion_pairs_over_3_points
        );

        items[0].slot_reason = SlotReason::Base;
        let missing_reason = summarize_diversity(&items, &eligibility);
        assert!(missing_reason.mmr_inversion_pairs_missing_reason > 0);
    }

    #[test]
    fn quality_gates_fail_a_top20_mode_share_above_sixty_percent() {
        let items = (1..=20)
            .map(|app_id| {
                let mode = match app_id {
                    1..=13 => "private_coop",
                    14..=17 => "matchmade_pvp",
                    _ => "public_world",
                };
                ranked_candidate(
                    app_id,
                    1.0 - f64::from(app_id) / 100.0,
                    mode,
                    SlotReason::Base,
                    &[],
                )
            })
            .collect::<Vec<_>>();
        let eligibility = visible_eligibility(&items);
        let score_rows = items
            .iter()
            .map(|item| {
                (
                    item.app_id,
                    item.score.relevance_score,
                    item.score.final_score,
                )
            })
            .collect::<Vec<_>>();
        let fingerprints = items
            .iter()
            .map(|item| (item.app_id, score_feature_fingerprint(item)))
            .collect();
        let scores = summarize_scores(&score_rows, &eligibility, &fingerprints, DEFAULT_TOP);
        let diversity = summarize_diversity(&items, &eligibility);
        let evidence = summarize_evidence(&items, &HashSet::new());
        let gates = evaluate_section_gates(&scores, &diversity, &evidence);
        let status = |name| gates.iter().find(|gate| gate.name == name).unwrap().status;

        assert_eq!(
            status("top20_distinct_recommendation_indices"),
            GateStatus::Pass
        );
        assert_eq!(
            status("top20_largest_recommendation_index_bucket_share"),
            GateStatus::Pass
        );
        assert_eq!(status("top20_single_mode_share"), GateStatus::Fail);
        assert_eq!(diversity.top_largest_mode_share, 0.65);
    }

    #[test]
    fn evidence_audit_reports_unique_unresolvable_ids() {
        let items = vec![
            ranked_candidate(
                1,
                0.9,
                "private_coop",
                SlotReason::Base,
                &["feature:private_session:1", "missing:1"],
            ),
            ranked_candidate(2, 0.8, "matchmade_pvp", SlotReason::Base, &["missing:1"]),
        ];
        let available = HashSet::from(["feature:private_session:1".to_owned()]);
        let evidence = summarize_evidence(&items, &available);
        assert_eq!(evidence.referenced_count, 3);
        assert_eq!(evidence.unique_referenced_count, 2);
        assert_eq!(evidence.resolvable_count, 1);
        assert_eq!(evidence.unresolved_count, 1);
        assert_eq!(evidence.unresolved_ids_truncated, 0);
        assert_eq!(evidence.resolvability_rate, Some(0.5));
        assert_eq!(evidence.unresolved_ids, vec!["missing:1"]);
    }

    #[test]
    fn computed_profile_resolver_matches_public_evidence_fallbacks() {
        let mut ids = HashSet::new();
        add_computed_profile_evidence_ids(&mut ids, 10, Some("competitive"), None);
        add_computed_profile_evidence_ids(&mut ids, 20, Some("mmo"), Some("active"));
        add_computed_profile_evidence_ids(&mut ids, 30, Some("private_coop"), Some("shutdown"));

        assert!(ids.contains("feature:matchmaking_core:10"));
        assert!(ids.contains("feature:public_world_dependency:20"));
        assert!(ids.contains("feature:service_shutdown_risk:20"));
        assert!(ids.contains("feature:service_shutdown_risk:30"));
        assert!(!ids.contains("feature:matchmaking_core:30"));
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn evidence_audit_bounds_unresolved_id_samples_without_hiding_failure_count() {
        let evidence_ids = (0..(MAX_UNRESOLVED_EVIDENCE_ID_SAMPLES + 3))
            .map(|index| format!("missing:{index}"))
            .collect::<Vec<_>>();
        let item = ranked_candidate(1, 0.9, "private_coop", SlotReason::Base, &[]);
        let mut item = item;
        item.explanation.evidence_ids = evidence_ids;
        let evidence = summarize_evidence(&[item], &HashSet::new());

        assert_eq!(
            evidence.unresolved_count,
            MAX_UNRESOLVED_EVIDENCE_ID_SAMPLES + 3
        );
        assert_eq!(
            evidence.unresolved_ids.len(),
            MAX_UNRESOLVED_EVIDENCE_ID_SAMPLES
        );
        assert_eq!(evidence.unresolved_ids_truncated, 3);
    }

    #[test]
    fn strict_failure_message_names_deterministic_failed_gates() {
        let summary = QualityGateSummary {
            strict_compatible: false,
            evaluated: 1,
            passed: 0,
            failed: 1,
            not_applicable: 2,
            failures: vec![QualityGateFailure {
                section: "popular",
                gate: "clamp_rate",
                detail: "rate=0.02".into(),
            }],
        };
        let message = strict_failure_message(&summary);
        assert!(message.contains("failed 1 deterministic quality gate"));
        assert!(message.contains("popular/clamp_rate"));
    }

    #[test]
    fn cutoff_uses_the_active_recent_window() {
        let config = RecommendationConfig {
            recent_days: 180,
            ..RecommendationConfig::default()
        };
        assert_eq!(cutoff_day("2026-08-01", &config).unwrap(), "2026-02-02");
    }
}
