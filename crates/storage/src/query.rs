//! Read models for feeds, search, calendar, and game detail.

use mpgs_domain::{
    CandidateAvailability, FeedSection, ModeFamily, MultiplayerSignals, RankingSignals,
    RecommendationConfig, SteamAppId,
};
use rusqlite::{Connection, OptionalExtension, params, types::Type};
use std::collections::HashMap;

use crate::error::{StorageError, StorageResult};
use crate::models::AppRecord;
use crate::util::sql_to_opt_bool;

#[derive(Debug, Clone, PartialEq)]
pub struct GameCandidateRow {
    pub app_id: SteamAppId,
    pub name: String,
    pub app_type: String,
    pub release_state: String,
    pub release_date: Option<String>,
    pub release_date_raw: Option<String>,
    pub release_date_precision: Option<String>,
    pub cover_url: Option<String>,
    pub cover_updated_at_ms: Option<i64>,
    pub short_description: Option<String>,
    pub dominant_mode: Option<String>,
    pub private_session: Option<bool>,
    pub online_coop: Option<bool>,
    pub self_hosted_server: Option<bool>,
    pub drop_in_out: Option<bool>,
    pub crossplay: Option<bool>,
    pub service_status: Option<String>,
    pub matchmaking_core: Option<bool>,
    pub matchmaking_core_confidence: Option<f64>,
    pub public_world_dependency: Option<bool>,
    pub public_world_dependency_confidence: Option<f64>,
    pub service_shutdown_risk: Option<bool>,
    pub service_shutdown_risk_confidence: Option<f64>,
    pub recommended_min: Option<u8>,
    pub recommended_max: Option<u8>,
    pub profile_confidence: Option<f64>,
    pub total_reviews: Option<u32>,
    pub total_positive: Option<u32>,
    pub latest_ccu: Option<u32>,
    pub wilson_lower: Option<f64>,
    pub typical_ccu_7d: Option<u32>,
    /// Activity percentile within the complete queried section cohort. This is
    /// populated by `list_candidates`; detail/search rows leave it unknown.
    pub activity_percentile: Option<f64>,
    /// Neutralized 10-day activity trend. `None` means fewer than seven
    /// observed days (or insufficient samples on one side of the comparison).
    pub activity_momentum: Option<f64>,
    pub taxonomy_tags: Vec<String>,
    pub publisher: Option<String>,
    pub platforms: Vec<String>,
    pub languages: Vec<String>,
    pub typical_session_minutes_min: Option<u32>,
    pub typical_session_minutes_max: Option<u32>,
    pub is_free: Option<bool>,
    pub final_price_minor: Option<i64>,
    pub price_currency: Option<String>,
    pub has_demo: bool,
    /// Source observation times used by the public freshness contract. Feed
    /// SQL already applies each feature's TTL, so a non-null value is fresh for
    /// that request snapshot rather than merely the newest historical row.
    pub profile_observed_at_ms: Option<i64>,
    pub reviews_observed_at_ms: Option<i64>,
    pub activity_observed_at_ms: Option<i64>,
    pub price_observed_at_ms: Option<i64>,
    pub release_observed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PopularReviewRow {
    pub recommendation_id: String,
    pub rank: u8,
    pub author_name: Option<String>,
    pub author_profile_url: Option<String>,
    pub review_text: String,
    pub voted_up: bool,
    pub votes_up: u32,
    pub votes_funny: u32,
    pub comment_count: u32,
    pub playtime_forever_minutes: Option<u32>,
    pub playtime_at_review_minutes: Option<u32>,
    pub created_at_ms: i64,
    pub written_during_early_access: bool,
}

/// One screenshot or trailer row from `app_media_assets` (detail API only).
#[derive(Debug, Clone, PartialEq)]
pub struct GameMediaAssetRow {
    pub kind: String,
    pub source_id: String,
    pub sort_order: u16,
    pub title: Option<String>,
    pub thumbnail_url: String,
    pub full_url: Option<String>,
    pub mp4_url: Option<String>,
    pub hls_h264_url: Option<String>,
    pub dash_h264_url: Option<String>,
    pub is_highlight: bool,
    pub updated_at_ms: i64,
}

impl GameCandidateRow {
    pub fn availability(&self) -> CandidateAvailability {
        CandidateAvailability {
            platforms: self.platforms.clone(),
            languages: self.languages.clone(),
            typical_session_minutes_min: self.typical_session_minutes_min,
            typical_session_minutes_max: self.typical_session_minutes_max,
            price_currency: self.price_currency.clone(),
            final_price_minor: self.final_price_minor,
            is_free: self.is_free,
        }
    }

    /// Prefer stored dominant_mode; fall back to online_coop so the UI is not
    /// stuck on 未知 when Steam only left a co-op bool.
    pub fn display_dominant_mode(&self) -> Option<String> {
        resolve_display_dominant_mode(self.dominant_mode.as_deref(), self.online_coop)
    }

    pub fn mode_family(&self) -> ModeFamily {
        let stored = self
            .dominant_mode
            .as_deref()
            .map(ModeFamily::from_alias)
            .unwrap_or(ModeFamily::Unknown);
        if stored != ModeFamily::Unknown {
            return stored;
        }
        if self.self_hosted_server == Some(true) {
            ModeFamily::SelfHosted
        } else if self.online_coop == Some(true) || self.private_session == Some(true) {
            ModeFamily::PrivateCoop
        } else if self.recommended_max.is_some_and(|max| max >= 2)
            || self.recommended_min.is_some_and(|min| min >= 2)
        {
            ModeFamily::GenericMultiplayer
        } else {
            ModeFamily::Unknown
        }
    }

    /// Value exposed by the evidence API for normalized matchmaking behavior.
    /// Explicit evidence wins; otherwise a canonical primary mode can provide a
    /// computed-profile fact without claiming that unknown modes are negative.
    pub fn resolved_matchmaking_core(&self) -> Option<bool> {
        self.matchmaking_core.or_else(|| match self.mode_family() {
            ModeFamily::MatchmadePvp => Some(true),
            _ => None,
        })
    }

    pub fn resolved_public_world_dependency(&self) -> Option<bool> {
        self.public_world_dependency
            .or_else(|| match self.mode_family() {
                ModeFamily::PublicWorld => Some(true),
                _ => None,
            })
    }

    pub fn resolved_service_shutdown_risk(&self) -> Option<f64> {
        self.service_shutdown_risk.map(bool_value).or_else(|| {
            self.service_status
                .as_deref()
                .map(|status| service_shutdown_risk(Some(status)))
        })
    }

    pub fn to_ranking_signals(&self) -> RankingSignals {
        self.to_ranking_signals_as_of(None)
    }

    /// Build ranking signals relative to the request snapshot date. Supplying
    /// an as-of day makes freshness, launch proximity and longevity continuous
    /// instead of assigning every title in a section the same constant.
    pub fn to_ranking_signals_at(&self, today: &str) -> RankingSignals {
        self.to_ranking_signals_as_of(Some(today))
    }

    fn to_ranking_signals_as_of(&self, today: Option<&str>) -> RankingSignals {
        let review_confidence = self
            .total_reviews
            .map(|reviews| ((1.0 + f64::from(reviews)).ln() / 10_001.0_f64.ln()).clamp(0.0, 1.0))
            .unwrap_or(0.0);
        let observed_quality = self
            .wilson_lower
            .filter(|value| value.is_finite())
            .unwrap_or_else(|| match (self.total_positive, self.total_reviews) {
                (Some(pos), Some(total)) if total > 0 => f64::from(pos) / f64::from(total),
                _ => 0.5,
            })
            .clamp(0.0, 1.0);
        let quality = shrink_to_prior(observed_quality, review_confidence, 0.5);
        let observed_popularity = self.activity_percentile.unwrap_or_else(|| {
            self.typical_ccu_7d
                .or(self.latest_ccu)
                .map(|ccu| (f64::from(ccu).ln_1p() / 12.0).clamp(0.0, 1.0))
                .unwrap_or(0.5)
        });
        let activity_confidence = if self.activity_momentum.is_some() {
            1.0
        } else if self.typical_ccu_7d.is_some() {
            0.8
        } else if self.latest_ccu.is_some() {
            0.5
        } else {
            0.0
        };
        let popularity = shrink_to_prior(observed_popularity, activity_confidence, 0.5);
        let confidence = self
            .profile_confidence
            .filter(|value| value.is_finite())
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        let mode = self.mode_family();
        let mode_matchmaking = matches!(mode, ModeFamily::MatchmadePvp);
        let mode_public_world = matches!(mode, ModeFamily::PublicWorld);
        let mode_mixed = matches!(mode, ModeFamily::Mixed);
        let (matchmaking, matchmaking_confidence) = match self.matchmaking_core {
            Some(value) => (
                bool_value(value),
                self.matchmaking_core_confidence.unwrap_or(confidence),
            ),
            None if mode_matchmaking => (1.0, confidence),
            None if mode_mixed => (0.5, confidence),
            None => (0.0, 0.0),
        };
        let (public_world, public_world_confidence) = match self.public_world_dependency {
            Some(value) => (
                bool_value(value),
                self.public_world_dependency_confidence
                    .unwrap_or(confidence),
            ),
            None if mode_public_world => (1.0, confidence),
            None if mode_mixed => (0.5, confidence),
            None => (0.0, 0.0),
        };
        let inferred_shutdown = service_shutdown_risk(self.service_status.as_deref());
        let (shutdown, shutdown_confidence) = match self.service_shutdown_risk {
            Some(value) => (
                bool_value(value),
                self.service_shutdown_risk_confidence.unwrap_or(confidence),
            ),
            None if self.service_status.is_some() => (inferred_shutdown, confidence),
            None => (0.0, 0.0),
        };
        let (private, private_confidence) = capability_signal(self.private_session, confidence);
        let (coop, coop_confidence) = capability_signal(self.online_coop, confidence);
        let (self_host, self_host_confidence) =
            capability_signal(self.self_hosted_server, confidence);
        let (drop_in_out, drop_in_out_confidence) = capability_signal(self.drop_in_out, confidence);
        let (crossplay, crossplay_confidence) = capability_signal(self.crossplay, confidence);
        let has_public_independent_path =
            self.private_session == Some(true) || self.self_hosted_server == Some(true);
        let explicitly_public_independent =
            self.matchmaking_core == Some(false) && self.public_world_dependency == Some(false);
        let (low_public_population_dependency, low_public_confidence) =
            if has_public_independent_path || explicitly_public_independent {
                (
                    1.0,
                    private_confidence
                        .max(self_host_confidence)
                        .max(matchmaking_confidence.min(public_world_confidence)),
                )
            } else if matchmaking >= 0.5 || public_world >= 0.5 {
                (0.0, matchmaking_confidence.max(public_world_confidence))
            } else {
                // Absence of public-dependency evidence is not positive proof that a
                // title remains playable without a public population.
                (0.5, 0.0)
            };
        let known_feature_count = [
            self.dominant_mode
                .as_deref()
                .is_some_and(|mode| ModeFamily::from_alias(mode) != ModeFamily::Unknown),
            self.private_session.is_some(),
            self.online_coop.is_some(),
            self.self_hosted_server.is_some(),
            self.drop_in_out.is_some(),
            self.crossplay.is_some(),
            self.recommended_min.is_some(),
            self.recommended_max.is_some(),
            self.service_status.is_some(),
            self.matchmaking_core.is_some(),
            self.public_world_dependency.is_some(),
            self.service_shutdown_risk.is_some(),
        ]
        .into_iter()
        .filter(|known| *known)
        .count();
        let evidence_coverage = known_feature_count as f64 / 12.0;
        let profile_evidence = (0.6 * confidence + 0.4 * evidence_coverage).clamp(0.0, 1.0);
        let release_confidence = release_date_confidence(
            self.release_date.as_deref(),
            self.release_date_precision.as_deref(),
        );
        let relative_days = today.and_then(|today| {
            let today = crate::util::iso_day_to_unix_days(today)?;
            let release = crate::util::iso_day_to_unix_days(self.release_date.as_deref()?)?;
            Some(release - today)
        });
        let freshness = relative_days
            .filter(|days| *days <= 0)
            .map(|days| (1.0 - (-days) as f64 / 365.0).clamp(0.0, 1.0))
            .unwrap_or_else(|| {
                if self.release_state == "released" {
                    0.5
                } else {
                    0.8
                }
            });
        let release_proximity = relative_days
            .filter(|days| *days >= 0)
            .map(|days| (1.0 - days as f64 / 30.0).clamp(0.0, 1.0))
            .unwrap_or(0.2);
        let longevity = relative_days
            .filter(|days| *days <= 0)
            .map(|days| ((-days) as f64 / 3_650.0).clamp(0.0, 1.0))
            .unwrap_or_else(|| {
                if self.release_state == "released" {
                    0.5
                } else {
                    0.0
                }
            });
        let maintenance_health = maintenance_health(self.service_status.as_deref());
        let data_confidence = (0.45 * profile_evidence
            + 0.25 * review_confidence
            + 0.20 * activity_confidence
            + 0.10 * release_confidence)
            .clamp(0.0, 1.0);

        RankingSignals {
            multiplayer: MultiplayerSignals {
                private_session: private,
                self_host_or_dedicated: self_host,
                online_coop: coop,
                group_size_fit: 0.5,
                low_public_population_dependency,
                drop_in_out,
                cross_platform_fit: crossplay,
                matchmaking_core: matchmaking,
                public_world_dependency: public_world,
                group_size_mismatch: 0.0,
                service_shutdown_risk: shutdown,
                external_account_friction: 0.0,
                platform_or_anticheat_restriction: 0.0,
            },
            multiplayer_confidence: MultiplayerSignals {
                private_session: private_confidence,
                self_host_or_dedicated: self_host_confidence,
                online_coop: coop_confidence,
                group_size_fit: f64::from(
                    self.recommended_min.is_some() || self.recommended_max.is_some(),
                ),
                low_public_population_dependency: low_public_confidence,
                drop_in_out: drop_in_out_confidence,
                cross_platform_fit: crossplay_confidence,
                matchmaking_core: matchmaking_confidence,
                public_world_dependency: public_world_confidence,
                group_size_mismatch: f64::from(
                    self.recommended_min.is_some() || self.recommended_max.is_some(),
                ),
                service_shutdown_risk: shutdown_confidence,
                external_account_friction: 0.0,
                platform_or_anticheat_restriction: 0.0,
            },
            has_multiplayer_confidence: true,
            quality,
            popularity,
            momentum: self.activity_momentum.unwrap_or(0.5).clamp(0.0, 1.0),
            evidence: evidence_coverage,
            freshness,
            data_confidence,
            demo_playability: if self.app_type == "demo" || self.has_demo {
                1.0
            } else {
                0.0
            },
            release_date_confidence: release_confidence,
            release_proximity,
            studio_prior: 0.5,
            longevity,
            maintenance_health,
            risk: shutdown * shutdown_confidence * 0.5
                + public_world * public_world_confidence * 0.2,
            personal_fit: 0.5,
            personal_components: Default::default(),
        }
    }
}

fn shrink_to_prior(observed: f64, confidence: f64, prior: f64) -> f64 {
    let confidence = confidence.clamp(0.0, 1.0);
    confidence * observed.clamp(0.0, 1.0) + (1.0 - confidence) * prior.clamp(0.0, 1.0)
}

fn release_date_confidence(date: Option<&str>, precision: Option<&str>) -> f64 {
    if date.is_none() {
        return 0.0;
    }
    match precision
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("day" | "exact") => 1.0,
        Some("month") => 0.65,
        Some("quarter") => 0.5,
        Some("year") => 0.35,
        _ => 0.8,
    }
}

fn maintenance_health(status: Option<&str>) -> f64 {
    let Some(status) = status else {
        return 0.5;
    };
    let normalized = status.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    match normalized.as_str() {
        "active" | "online" | "operational" | "maintained" => 0.9,
        "degraded" | "maintenance" | "shutdown_announced" | "sunsetting" | "end_of_service" => 0.3,
        "shutdown" | "shut_down" | "offline" | "closed" | "discontinued" | "sunset" | "retired"
        | "terminated" => 0.0,
        _ => 0.5,
    }
}

fn capability_signal(value: Option<bool>, confidence: f64) -> (f64, f64) {
    match value {
        Some(true) => (1.0, confidence),
        Some(false) => (0.0, confidence),
        None => (0.5, 0.0),
    }
}

fn bool_value(value: bool) -> f64 {
    if value { 1.0 } else { 0.0 }
}

fn service_shutdown_risk(status: Option<&str>) -> f64 {
    let Some(status) = status else {
        return 0.0;
    };
    let normalized = status.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    match normalized.as_str() {
        "shutdown" | "shut_down" | "offline" | "closed" | "discontinued" | "sunset" | "retired"
        | "terminated" => 1.0,
        "shutdown_announced" | "sunsetting" | "end_of_service" | "degraded" => 0.75,
        _ => 0.0,
    }
}

pub fn list_candidates(
    conn: &Connection,
    section: FeedSection,
    cutoff_date: &str,
    today: &str,
    budget_currency: &str,
    config: &RecommendationConfig,
    limit: i64,
) -> StorageResult<Vec<GameCandidateRow>> {
    // Currency alone is insufficient for regional Steam pricing. Use a price
    // only when the deployment's supported currency has one unambiguous store
    // country; otherwise leave price unknown and avoid a false budget filter.
    let budget_country = default_country_for_currency(budget_currency).unwrap_or("");
    // Section windows and feed presentation use the current store `apps.release_date`
    // (not the earliest historical date from release_events). "近期正式发售" is ordered
    // and filtered by that store day so 1.0 / re-dated launches surface by calendar day.
    // `section` is a trusted enum. Give SQLite one concrete predicate and sort
    // shape, then materialize that app scope before ranking snapshot history.
    // This preserves the full candidate limit without scanning unrelated
    // review, player, and daily rows for every feed request.
    let (scope_predicate, section_predicate, section_order) = match section {
        FeedSection::Upcoming => (
            "((a.release_state IN ('upcoming', 'coming_soon')
                 AND a.release_date IS NOT NULL
                 AND a.release_date >= :today
                 AND a.release_date <= date(:today, '+30 days'))
               OR a.app_type IN ('demo', 'playtest'))",
            "((a.release_state IN ('upcoming', 'coming_soon')
                 AND a.release_date IS NOT NULL
                 AND a.release_date >= :today
                 AND a.release_date <= date(:today, '+30 days'))
               OR a.app_type IN ('demo', 'playtest'))",
            "a.release_date DESC",
        ),
        FeedSection::RecentRelease => (
            "a.release_state = 'released'
             AND a.release_date IS NOT NULL
             AND a.release_date >= :cutoff
             AND a.release_date <= :today",
            "a.release_state = 'released'
             AND a.release_date IS NOT NULL
             AND a.release_date >= :cutoff
             AND a.release_date <= :today",
            "a.release_date DESC",
        ),
        FeedSection::PopularLegacy => (
            "a.release_state = 'released'
             AND a.release_date IS NOT NULL
             AND a.release_date < :cutoff",
            "a.release_state = 'released'
             AND a.release_date IS NOT NULL
             AND a.release_date < :cutoff
             AND COALESCE(d.typical_ccu, lp.player_count, 0) >= :popular_min_ccu
             AND COALESCE(r.wilson_lower, 0) >= CASE
                 WHEN COALESCE(d.typical_ccu, lp.player_count, 0) >= :popular_high_ccu
                     THEN :popular_high_ccu_min_wilson
                 ELSE :popular_min_wilson
             END",
            "COALESCE(d.typical_ccu, lp.player_count, 0) DESC",
        ),
        FeedSection::ClassicLegacy => (
            "a.release_state = 'released'
             AND a.release_date IS NOT NULL
             AND a.release_date < :cutoff",
            "a.release_state = 'released'
             AND a.release_date IS NOT NULL
             AND a.release_date < :cutoff
             AND COALESCE(r.total_reviews, 0) >= :classic_min_reviews
             AND COALESCE(r.wilson_lower, 0) >= :classic_min_wilson
             AND NOT (
                 COALESCE(d.typical_ccu, lp.player_count, 0) >= :popular_min_ccu
                 AND COALESCE(r.wilson_lower, 0) >= CASE
                     WHEN COALESCE(d.typical_ccu, lp.player_count, 0) >= :popular_high_ccu
                         THEN :popular_high_ccu_min_wilson
                     ELSE :popular_min_wilson
                 END
             )",
            "COALESCE(r.total_reviews, 0) DESC",
        ),
    };
    let classic_activity_percentiles = matches!(section, FeedSection::ClassicLegacy)
        .then(|| classic_activity_percentiles(conn, cutoff_date, today, limit))
        .transpose()?;
    let (
        latest_reviews_cte,
        ranked_scope_cte,
        activity_scope,
        final_scope,
        final_predicate,
        review_join,
    ) = if matches!(section, FeedSection::ClassicLegacy) {
        (
            CLASSIC_LATEST_REVIEWS_CTE,
            CLASSIC_RANKED_SCOPE_CTE,
            "ranked_candidate_scope",
            "classic_eligible_scope",
            "1=1",
            CLASSIC_REVIEW_JOIN,
        )
    } else {
        (
            "",
            "",
            "candidate_scope",
            "candidate_scope",
            section_predicate,
            CANDIDATE_REVIEW_JOIN,
        )
    };
    let eligible_scope_cte = if matches!(section, FeedSection::ClassicLegacy) {
        format!(
            ", classic_eligible_scope AS MATERIALIZED (
                 SELECT scope.app_id
                 FROM ranked_candidate_scope scope
                 JOIN apps a ON a.app_id = scope.app_id
                 LEFT JOIN latest_reviews r ON r.app_id = a.app_id
                 LEFT JOIN player_snapshots lp ON lp.rowid = (
                     SELECT latest_player.rowid
                     FROM player_snapshots latest_player
                     WHERE latest_player.app_id = a.app_id
                       AND latest_player.player_count IS NOT NULL
                       AND latest_player.captured_at_ms >= CAST(strftime('%s', date(:today, '-2 days')) AS INTEGER) * 1000
                     ORDER BY latest_player.captured_at_ms DESC
                     LIMIT 1
                 )
                 LEFT JOIN daily_typical d ON d.app_id = a.app_id
                 WHERE {section_predicate}
             )"
        )
    } else {
        String::new()
    };
    let sql = format!(
        "WITH {latest_reviews_cte}candidate_scope AS MATERIALIZED (
             SELECT a.app_id
             FROM apps a
             WHERE a.app_type IN ('game', 'demo', 'playtest', 'unknown')
               AND ({scope_predicate})
         ){ranked_scope_cte}, daily_window AS (
             SELECT daily.app_id, daily.day_utc, daily.mean_ccu
             FROM player_daily daily
             JOIN {activity_scope} scope ON scope.app_id = daily.app_id
             WHERE daily.mean_ccu IS NOT NULL
               AND daily.day_utc >= date(:today, '-9 days')
               AND daily.day_utc <= :today
         ), daily_activity AS (
             SELECT app_id,
                    CAST(AVG(CASE
                        WHEN day_utc >= date(:today, '-6 days') THEN mean_ccu
                    END) AS INTEGER) AS typical_ccu,
                    COUNT(DISTINCT day_utc) AS observed_days_10d,
                    SUM(CASE WHEN day_utc >= date(:today, '-2 days') THEN 1 ELSE 0 END)
                        AS recent_days,
                    SUM(CASE WHEN day_utc < date(:today, '-2 days') THEN 1 ELSE 0 END)
                        AS baseline_days,
                    AVG(CASE WHEN day_utc >= date(:today, '-2 days') THEN mean_ccu END)
                        AS recent_ccu,
                    AVG(CASE WHEN day_utc < date(:today, '-2 days') THEN mean_ccu END)
                        AS baseline_ccu,
                    MAX(day_utc) AS latest_day
             FROM daily_window
             GROUP BY app_id
         ), daily_typical AS (
             SELECT app_id, typical_ccu,
                    CAST(strftime('%s', latest_day) AS INTEGER) * 1000 AS observed_at_ms,
                    CASE
                        WHEN observed_days_10d >= 7 AND recent_days >= 2 AND baseline_days >= 4
                        THEN MIN(1.0, MAX(0.0,
                            0.5 + 0.25 * (recent_ccu - baseline_ccu)
                                / MAX(recent_ccu, baseline_ccu, 1.0)
                        ))
                    END AS momentum
             FROM daily_activity
         ){eligible_scope_cte}
         SELECT a.app_id, a.canonical_name, a.app_type, a.release_state,
                a.release_date,
                p.dominant_mode, p.private_session, p.online_coop, p.self_hosted_server,
                p.recommended_min_players, p.recommended_max_players, p.profile_confidence,
                r.total_reviews, r.total_positive, lp.player_count, r.wilson_lower,
                d.typical_ccu,
                COALESCE(v.platforms_json, '[]'), COALESCE(v.languages_json, '[]'),
                v.typical_session_minutes_min, v.typical_session_minutes_max, v.is_free,
                (
                    SELECT price.final_price_minor FROM price_snapshots price
                    WHERE price.app_id = a.app_id
                      AND price.currency = :currency
                      AND price.country_code = :country
                      AND price.captured_at_ms >= CAST(strftime('%s', date(:today, '-2 days')) AS INTEGER) * 1000
                    ORDER BY price.captured_at_ms DESC LIMIT 1
                ),
                :currency,
                (a.app_type IN ('demo', 'playtest') OR EXISTS (
                    SELECT 1 FROM app_relations demo_relation
                    WHERE demo_relation.target_app_id = a.app_id
                      AND demo_relation.relation_type IN ('demo_of', 'playtest_of')
                )),
                 a.release_date_raw, a.release_date_precision,
                 media.capsule_url, media.updated_at_ms, NULL,
                 p.drop_in_out, p.crossplay, p.service_status, d.momentum,
                 (
                    SELECT json_object(
                        'value', json(evidence.value_json),
                        'confidence', evidence.confidence,
                        'observed_at_ms', evidence.observed_at_ms
                    ) FROM feature_evidence evidence
                    WHERE evidence.app_id = a.app_id
                      AND evidence.feature_name = 'matchmaking_core'
                      AND evidence.is_active = 1
                      AND evidence.observed_at_ms >= CAST(strftime('%s', date(:today, '-180 days')) AS INTEGER) * 1000
                      AND (evidence.expires_at_ms IS NULL OR evidence.expires_at_ms >= CAST(strftime('%s', :today) AS INTEGER) * 1000)
                    ORDER BY evidence.observed_at_ms DESC, evidence.evidence_id DESC LIMIT 1
                 ),
                 (
                    SELECT json_object(
                        'value', json(evidence.value_json),
                        'confidence', evidence.confidence,
                        'observed_at_ms', evidence.observed_at_ms
                    ) FROM feature_evidence evidence
                    WHERE evidence.app_id = a.app_id
                      AND evidence.feature_name = 'public_world_dependency'
                      AND evidence.is_active = 1
                      AND evidence.observed_at_ms >= CAST(strftime('%s', date(:today, '-180 days')) AS INTEGER) * 1000
                      AND (evidence.expires_at_ms IS NULL OR evidence.expires_at_ms >= CAST(strftime('%s', :today) AS INTEGER) * 1000)
                    ORDER BY evidence.observed_at_ms DESC, evidence.evidence_id DESC LIMIT 1
                 ),
                 (
                    SELECT json_object(
                        'value', json(evidence.value_json),
                        'confidence', evidence.confidence,
                        'observed_at_ms', evidence.observed_at_ms
                    ) FROM feature_evidence evidence
                    WHERE evidence.app_id = a.app_id
                      AND evidence.feature_name = 'service_shutdown_risk'
                      AND evidence.is_active = 1
                      AND evidence.observed_at_ms >= CAST(strftime('%s', date(:today, '-180 days')) AS INTEGER) * 1000
                      AND (evidence.expires_at_ms IS NULL OR evidence.expires_at_ms >= CAST(strftime('%s', :today) AS INTEGER) * 1000)
                    ORDER BY evidence.observed_at_ms DESC, evidence.evidence_id DESC LIMIT 1
                 ),
                 (
                    SELECT evidence.value_json FROM feature_evidence evidence
                    WHERE evidence.app_id = a.app_id
                      AND evidence.feature_name = 'catalog_taxonomy'
                      AND evidence.is_active = 1
                      AND evidence.observed_at_ms >= CAST(strftime('%s', date(:today, '-180 days')) AS INTEGER) * 1000
                      AND (evidence.expires_at_ms IS NULL OR evidence.expires_at_ms >= CAST(strftime('%s', :today) AS INTEGER) * 1000)
                    ORDER BY evidence.observed_at_ms DESC, evidence.evidence_id DESC LIMIT 1
                 ),
                 p.computed_at_ms, r.captured_at_ms,
                 COALESCE(d.observed_at_ms, lp.captured_at_ms),
                 (
                    SELECT price.captured_at_ms FROM price_snapshots price
                    WHERE price.app_id = a.app_id
                      AND price.currency = :currency
                      AND price.country_code = :country
                      AND price.captured_at_ms >= CAST(strftime('%s', date(:today, '-2 days')) AS INTEGER) * 1000
                    ORDER BY price.captured_at_ms DESC LIMIT 1
                 ),
                 a.updated_at_ms
         FROM {final_scope} scope
         JOIN apps a ON a.app_id = scope.app_id
         LEFT JOIN multiplayer_profiles p ON p.app_id = a.app_id
             AND p.computed_at_ms >= CAST(strftime('%s', date(:today, '-180 days')) AS INTEGER) * 1000
         LEFT JOIN app_availability v ON v.app_id = a.app_id
         {review_join}
         LEFT JOIN player_snapshots lp ON lp.rowid = (
             SELECT latest_player.rowid
             FROM player_snapshots latest_player
             WHERE latest_player.app_id = a.app_id
               AND latest_player.player_count IS NOT NULL
               AND latest_player.captured_at_ms >= CAST(strftime('%s', date(:today, '-2 days')) AS INTEGER) * 1000
             ORDER BY latest_player.captured_at_ms DESC
             LIMIT 1
         )
         LEFT JOIN daily_typical d ON d.app_id = a.app_id
         LEFT JOIN app_media media ON media.app_id = a.app_id
         WHERE {final_predicate}
         ORDER BY
             {section_order},
             a.updated_at_ms DESC
         LIMIT :limit"
    );
    let mut stmt = conn.prepare(&sql)?;
    {
        let mut bind_if_present =
            |name: &str, value: &dyn rusqlite::ToSql| -> rusqlite::Result<()> {
                if let Some(index) = stmt.parameter_index(name)? {
                    stmt.raw_bind_parameter(index, value)?;
                }
                Ok(())
            };
        bind_if_present(":cutoff", &cutoff_date)?;
        bind_if_present(":today", &today)?;
        bind_if_present(":currency", &budget_currency)?;
        bind_if_present(":country", &budget_country)?;
        bind_if_present(":popular_min_ccu", &config.popular_min_ccu)?;
        bind_if_present(":popular_high_ccu", &config.popular_high_ccu)?;
        bind_if_present(":popular_min_wilson", &config.popular_min_wilson)?;
        bind_if_present(
            ":popular_high_ccu_min_wilson",
            &config.popular_high_ccu_min_wilson,
        )?;
        bind_if_present(":classic_min_reviews", &config.classic_min_reviews)?;
        bind_if_present(":classic_min_wilson", &config.classic_min_wilson)?;
        bind_if_present(":limit", &limit)?;
    }
    let mut rows = stmt.raw_query();
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(map_candidate(row)?);
    }
    if let Some(percentiles) = classic_activity_percentiles {
        for row in &mut out {
            row.activity_percentile = percentiles.get(&row.app_id).copied();
        }
    } else {
        assign_activity_percentiles(&mut out);
    }
    Ok(out)
}

const CLASSIC_LATEST_REVIEWS_CTE: &str = "latest_reviews AS MATERIALIZED (
    SELECT app_id, total_reviews, total_positive, wilson_lower, captured_at_ms
    FROM (
        SELECT app_id, total_reviews, total_positive, wilson_lower, captured_at_ms,
               ROW_NUMBER() OVER (
                   PARTITION BY app_id
                   ORDER BY captured_at_ms DESC, language_scope ASC
               ) AS snapshot_rank
        FROM review_snapshots
        WHERE captured_at_ms >= CAST(strftime('%s', date(:today, '-30 days')) AS INTEGER) * 1000
    )
    WHERE snapshot_rank = 1
), ";

const CLASSIC_REVIEW_JOIN: &str = "LEFT JOIN latest_reviews r ON r.app_id = a.app_id";

const CANDIDATE_REVIEW_JOIN: &str = "LEFT JOIN review_snapshots r ON r.rowid = (
    SELECT latest_review.rowid
    FROM review_snapshots latest_review
    WHERE latest_review.app_id = a.app_id
      AND latest_review.captured_at_ms >= CAST(strftime('%s', date(:today, '-30 days')) AS INTEGER) * 1000
    ORDER BY latest_review.captured_at_ms DESC, latest_review.language_scope ASC
    LIMIT 1
)";

const CLASSIC_RANKED_SCOPE_CTE: &str = ", ranked_candidate_scope AS MATERIALIZED (
    SELECT a.app_id
    FROM candidate_scope scope
    JOIN apps a ON a.app_id = scope.app_id
    LEFT JOIN latest_reviews r ON r.app_id = a.app_id
    ORDER BY COALESCE(r.total_reviews, 0) DESC, a.updated_at_ms DESC
    LIMIT :limit
)";

fn classic_activity_percentiles(
    conn: &Connection,
    cutoff_date: &str,
    today: &str,
    limit: i64,
) -> StorageResult<HashMap<SteamAppId, f64>> {
    let sql = format!(
        "WITH {CLASSIC_LATEST_REVIEWS_CTE}candidate_scope AS MATERIALIZED (
             SELECT a.app_id
             FROM apps a
             WHERE a.app_type IN ('game', 'demo', 'playtest', 'unknown')
               AND a.release_state = 'released'
               AND a.release_date IS NOT NULL
               AND a.release_date < :cutoff
         ){CLASSIC_RANKED_SCOPE_CTE}, daily_typical AS (
             SELECT daily.app_id,
                    CAST(AVG(CASE
                        WHEN daily.day_utc >= date(:today, '-6 days') THEN daily.mean_ccu
                    END) AS INTEGER) AS typical_ccu
             FROM player_daily daily
             JOIN ranked_candidate_scope scope ON scope.app_id = daily.app_id
             WHERE daily.mean_ccu IS NOT NULL
               AND daily.day_utc >= date(:today, '-9 days')
               AND daily.day_utc <= :today
             GROUP BY daily.app_id
         )
         SELECT scope.app_id,
                COALESCE(d.typical_ccu, (
                    SELECT latest_player.player_count
                    FROM player_snapshots latest_player
                    WHERE latest_player.app_id = scope.app_id
                      AND latest_player.player_count IS NOT NULL
                      AND latest_player.captured_at_ms >= CAST(strftime('%s', date(:today, '-2 days')) AS INTEGER) * 1000
                    ORDER BY latest_player.captured_at_ms DESC
                    LIMIT 1
                ))
         FROM ranked_candidate_scope scope
         LEFT JOIN daily_typical d ON d.app_id = scope.app_id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::named_params! {
            ":cutoff": cutoff_date,
            ":today": today,
            ":limit": limit,
        },
        |row| {
            Ok((
                row.get::<_, i64>(0)? as SteamAppId,
                row.get::<_, Option<i64>>(1)?.map(|value| value as u32),
            ))
        },
    )?;
    let activity = rows
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|(app_id, activity)| activity.map(|value| (app_id, value)))
        .collect();
    Ok(activity_percentiles(activity))
}

fn activity_percentiles(mut activity: Vec<(SteamAppId, u32)>) -> HashMap<SteamAppId, f64> {
    activity.sort_by(|(left_id, left), (right_id, right)| {
        left.cmp(right).then_with(|| left_id.cmp(right_id))
    });
    if activity.is_empty() {
        return HashMap::new();
    }
    if activity.len() == 1 {
        return HashMap::from([(activity[0].0, 0.5)]);
    }

    let denominator = (activity.len() - 1) as f64;
    let mut percentiles = HashMap::with_capacity(activity.len());
    let mut start = 0usize;
    while start < activity.len() {
        let value = activity[start].1;
        let mut end = start + 1;
        while end < activity.len() && activity[end].1 == value {
            end += 1;
        }
        let midrank_zero_based = (start as f64 + (end - 1) as f64) / 2.0;
        let percentile = midrank_zero_based / denominator;
        for (app_id, _) in &activity[start..end] {
            percentiles.insert(*app_id, percentile);
        }
        start = end;
    }
    percentiles
}

fn assign_activity_percentiles(rows: &mut [GameCandidateRow]) {
    let mut activity: Vec<(usize, u32)> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            row.typical_ccu_7d
                .or(row.latest_ccu)
                .map(|ccu| (index, ccu))
        })
        .collect();
    activity.sort_by(|(left_index, left), (right_index, right)| {
        left.cmp(right).then_with(|| left_index.cmp(right_index))
    });
    if activity.is_empty() {
        return;
    }
    if activity.len() == 1 {
        rows[activity[0].0].activity_percentile = Some(0.5);
        return;
    }
    let denominator = (activity.len() - 1) as f64;
    let mut start = 0usize;
    while start < activity.len() {
        let value = activity[start].1;
        let mut end = start + 1;
        while end < activity.len() && activity[end].1 == value {
            end += 1;
        }
        let midrank_zero_based = (start as f64 + (end - 1) as f64) / 2.0;
        let percentile = midrank_zero_based / denominator;
        for (row_index, _) in &activity[start..end] {
            rows[*row_index].activity_percentile = Some(percentile);
        }
        start = end;
    }
}

fn default_country_for_currency(currency: &str) -> Option<&'static str> {
    match currency.trim().to_ascii_uppercase().as_str() {
        "CNY" => Some("CN"),
        "USD" => Some("US"),
        "GBP" => Some("GB"),
        "JPY" => Some("JP"),
        "KRW" => Some("KR"),
        "CAD" => Some("CA"),
        "AUD" => Some("AU"),
        "BRL" => Some("BR"),
        "INR" => Some("IN"),
        // EUR and other shared/unsupported currencies require an explicit
        // country in a future public preference schema.
        _ => None,
    }
}

/// Shared feed eligibility after source-level candidate selection and before
/// user-specific hard filters. Keeping it here makes the API and release audit
/// evaluate the same section rules.
pub fn section_matches(
    section: FeedSection,
    row: &GameCandidateRow,
    signals: &RankingSignals,
    cutoff_date: &str,
    today: &str,
    config: &RecommendationConfig,
) -> bool {
    let activity = row.typical_ccu_7d.or(row.latest_ccu).unwrap_or(0);
    let date = row.release_date.as_deref();
    let popular_quality_floor = if activity >= config.popular_high_ccu {
        config.popular_high_ccu_min_wilson
    } else {
        config.popular_min_wilson
    };
    let is_popular_legacy = row.release_state == "released"
        && date.is_some_and(|value| value < cutoff_date)
        && activity >= config.popular_min_ccu
        && row
            .wilson_lower
            .is_some_and(|value| value >= popular_quality_floor);
    let multiplayer = &signals.multiplayer;
    let depends_on_public_population =
        multiplayer.matchmaking_core >= 0.5 || multiplayer.public_world_dependency >= 0.5;
    let has_public_independent_path = row.private_session == Some(true)
        || row.self_hosted_server == Some(true)
        || multiplayer.private_session >= 0.5
        || multiplayer.self_host_or_dedicated >= 0.5
        || multiplayer.low_public_population_dependency >= 0.5;
    let classic_activity_sufficient = !depends_on_public_population
        || has_public_independent_path
        || activity >= config.classic_public_min_ccu;
    match section {
        FeedSection::Upcoming => {
            // Store-search candidates often only materialize a safe min party size
            // (recommended_min=2) before full store details fill mode flags. Treat
            // that conservative signal as enough multiplayer evidence for upcoming.
            let has_multiplayer_evidence = row.mode_family() != ModeFamily::Unknown
                || row.private_session == Some(true)
                || row.online_coop == Some(true)
                || row.self_hosted_server == Some(true)
                || row.drop_in_out == Some(true)
                || row.crossplay == Some(true)
                || row.recommended_min.is_some_and(|min| min >= 2)
                || row.recommended_max.is_some_and(|max| max >= 2);
            let release_within_30_days = date.is_some_and(|release_date| {
                let Some(today_day) = crate::util::iso_day_to_unix_days(today) else {
                    return false;
                };
                let Some(release_day) = crate::util::iso_day_to_unix_days(release_date) else {
                    return false;
                };
                (0..=30).contains(&(release_day - today_day))
            });
            ((matches!(row.release_state.as_str(), "upcoming" | "coming_soon")
                && release_within_30_days)
                || matches!(row.app_type.as_str(), "demo" | "playtest"))
                && has_multiplayer_evidence
        }
        FeedSection::RecentRelease => {
            row.release_state == "released"
                && date.is_some_and(|value| value >= cutoff_date && value <= today)
        }
        FeedSection::PopularLegacy => is_popular_legacy,
        FeedSection::ClassicLegacy => {
            row.release_state == "released"
                && date.is_some_and(|value| value < cutoff_date)
                && !is_popular_legacy
                && row
                    .total_reviews
                    .is_some_and(|value| value >= config.classic_min_reviews)
                && row
                    .wilson_lower
                    .is_some_and(|value| value >= config.classic_min_wilson)
                && classic_activity_sufficient
        }
    }
}

pub fn search_by_name(
    conn: &Connection,
    query: &str,
    limit: i64,
) -> StorageResult<Vec<GameCandidateRow>> {
    let escaped = query
        .trim()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    if escaped.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = format!("%{escaped}%");
    // Match both the list/display canonical string and CN/EN localization names.
    // Other languages are intentionally not included yet.
    let mut stmt = conn.prepare(
        "SELECT a.app_id, a.canonical_name, a.app_type, a.release_state, a.release_date,
                p.dominant_mode, p.private_session, p.online_coop, p.self_hosted_server,
                p.recommended_min_players, p.recommended_max_players, p.profile_confidence,
                NULL, NULL, NULL, NULL, NULL,
                COALESCE(v.platforms_json, '[]'), COALESCE(v.languages_json, '[]'),
                v.typical_session_minutes_min, v.typical_session_minutes_max, v.is_free,
                NULL, NULL,
                (a.app_type IN ('demo', 'playtest') OR EXISTS (
                    SELECT 1 FROM app_relations demo_relation
                    WHERE demo_relation.target_app_id = a.app_id
                      AND demo_relation.relation_type IN ('demo_of', 'playtest_of')
                )),
                 a.release_date_raw, a.release_date_precision,
                 media.capsule_url, media.updated_at_ms, NULL,
                 p.drop_in_out, p.crossplay, p.service_status, NULL,
                 (
                    SELECT json_object(
                        'value', json(evidence.value_json),
                        'confidence', evidence.confidence,
                        'observed_at_ms', evidence.observed_at_ms
                    ) FROM feature_evidence evidence
                    WHERE evidence.app_id = a.app_id
                      AND evidence.feature_name = 'matchmaking_core'
                      AND evidence.is_active = 1
                    ORDER BY evidence.observed_at_ms DESC, evidence.evidence_id DESC LIMIT 1
                 ),
                 (
                    SELECT json_object(
                        'value', json(evidence.value_json),
                        'confidence', evidence.confidence,
                        'observed_at_ms', evidence.observed_at_ms
                    ) FROM feature_evidence evidence
                    WHERE evidence.app_id = a.app_id
                      AND evidence.feature_name = 'public_world_dependency'
                      AND evidence.is_active = 1
                    ORDER BY evidence.observed_at_ms DESC, evidence.evidence_id DESC LIMIT 1
                 ),
                 (
                    SELECT json_object(
                        'value', json(evidence.value_json),
                        'confidence', evidence.confidence,
                        'observed_at_ms', evidence.observed_at_ms
                    ) FROM feature_evidence evidence
                    WHERE evidence.app_id = a.app_id
                      AND evidence.feature_name = 'service_shutdown_risk'
                      AND evidence.is_active = 1
                    ORDER BY evidence.observed_at_ms DESC, evidence.evidence_id DESC LIMIT 1
                 ),
                 (
                    SELECT evidence.value_json FROM feature_evidence evidence
                    WHERE evidence.app_id = a.app_id
                      AND evidence.feature_name = 'catalog_taxonomy'
                      AND evidence.is_active = 1
                    ORDER BY evidence.observed_at_ms DESC, evidence.evidence_id DESC LIMIT 1
                 ),
                 p.computed_at_ms, NULL, NULL, NULL, a.updated_at_ms
         FROM (
             SELECT app_id
             FROM apps
             WHERE canonical_name LIKE ?1 ESCAPE '\\' COLLATE NOCASE
             UNION
             SELECT app_id
             FROM app_localizations
             WHERE lower(language) IN ('schinese', 'english', 'en')
               AND name IS NOT NULL
               AND trim(name) != ''
               AND name LIKE ?1 ESCAPE '\\' COLLATE NOCASE
         ) matches
         JOIN apps a ON a.app_id = matches.app_id
         LEFT JOIN multiplayer_profiles p ON p.app_id = a.app_id
         LEFT JOIN app_availability v ON v.app_id = a.app_id
         LEFT JOIN app_media media ON media.app_id = a.app_id
         ORDER BY a.canonical_name
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, limit], map_candidate)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Load gallery assets for one app in stable sort order (no N+1).
pub fn list_game_media_assets(
    conn: &Connection,
    app_id: u32,
) -> StorageResult<Vec<GameMediaAssetRow>> {
    let mut stmt = conn.prepare(
        "SELECT kind, source_id, sort_order, title, thumbnail_url, full_url,
                mp4_url, hls_h264_url, dash_h264_url, is_highlight, updated_at_ms
         FROM app_media_assets
         WHERE app_id = ?1
         ORDER BY kind ASC, sort_order ASC, source_id ASC",
    )?;
    let rows = stmt.query_map(params![app_id], |row| {
        Ok(GameMediaAssetRow {
            kind: row.get(0)?,
            source_id: row.get(1)?,
            sort_order: row.get::<_, i64>(2)?.clamp(0, i64::from(u16::MAX)) as u16,
            title: row.get(3)?,
            thumbnail_url: row.get(4)?,
            full_url: row.get(5)?,
            mp4_url: row.get(6)?,
            hls_h264_url: row.get(7)?,
            dash_h264_url: row.get(8)?,
            is_highlight: row.get::<_, i64>(9)? != 0,
            updated_at_ms: row.get(10)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn get_game_detail(conn: &Connection, app_id: u32) -> StorageResult<Option<GameCandidateRow>> {
    conn.query_row(
        "SELECT a.app_id, a.canonical_name, a.app_type, a.release_state, a.release_date,
                p.dominant_mode, p.private_session, p.online_coop, p.self_hosted_server,
                p.recommended_min_players, p.recommended_max_players, p.profile_confidence,
                (
                    SELECT r.total_reviews FROM review_snapshots r
                    WHERE r.app_id = a.app_id
                    ORDER BY r.captured_at_ms DESC LIMIT 1
                ),
                (
                    SELECT r.total_positive FROM review_snapshots r
                    WHERE r.app_id = a.app_id
                    ORDER BY r.captured_at_ms DESC LIMIT 1
                ),
                (
                    SELECT s.player_count FROM player_snapshots s
                    WHERE s.app_id = a.app_id AND s.player_count IS NOT NULL
                    ORDER BY s.captured_at_ms DESC LIMIT 1
                ),
                (
                    SELECT r.wilson_lower FROM review_snapshots r
                    WHERE r.app_id = a.app_id
                    ORDER BY r.captured_at_ms DESC LIMIT 1
                ),
                (
                    SELECT CAST(AVG(d.mean_ccu) AS INTEGER) FROM player_daily d
                    WHERE d.app_id = a.app_id AND d.mean_ccu IS NOT NULL
                      AND d.day_utc >= date((
                          SELECT MAX(anchor.day_utc) FROM player_daily anchor
                          WHERE anchor.app_id = a.app_id AND anchor.mean_ccu IS NOT NULL
                      ), '-6 days')
                ),
                COALESCE(v.platforms_json, '[]'), COALESCE(v.languages_json, '[]'),
                v.typical_session_minutes_min, v.typical_session_minutes_max, v.is_free,
                (
                    SELECT price.final_price_minor FROM price_snapshots price
                    WHERE price.app_id = a.app_id
                    ORDER BY price.captured_at_ms DESC, price.currency ASC LIMIT 1
                ),
                (
                    SELECT price.currency FROM price_snapshots price
                    WHERE price.app_id = a.app_id
                    ORDER BY price.captured_at_ms DESC, price.currency ASC LIMIT 1
                ),
                (a.app_type IN ('demo', 'playtest') OR EXISTS (
                    SELECT 1 FROM app_relations demo_relation
                    WHERE demo_relation.target_app_id = a.app_id
                      AND demo_relation.relation_type IN ('demo_of', 'playtest_of')
                )),
                 a.release_date_raw, a.release_date_precision,
                 media.capsule_url, media.updated_at_ms, loc.short_description,
                 p.drop_in_out, p.crossplay, p.service_status, NULL,
                 (
                    SELECT json_object(
                        'value', json(evidence.value_json),
                        'confidence', evidence.confidence,
                        'observed_at_ms', evidence.observed_at_ms
                    ) FROM feature_evidence evidence
                    WHERE evidence.app_id = a.app_id
                      AND evidence.feature_name = 'matchmaking_core'
                      AND evidence.is_active = 1
                    ORDER BY evidence.observed_at_ms DESC, evidence.evidence_id DESC LIMIT 1
                 ),
                 (
                    SELECT json_object(
                        'value', json(evidence.value_json),
                        'confidence', evidence.confidence,
                        'observed_at_ms', evidence.observed_at_ms
                    ) FROM feature_evidence evidence
                    WHERE evidence.app_id = a.app_id
                      AND evidence.feature_name = 'public_world_dependency'
                      AND evidence.is_active = 1
                    ORDER BY evidence.observed_at_ms DESC, evidence.evidence_id DESC LIMIT 1
                 ),
                 (
                    SELECT json_object(
                        'value', json(evidence.value_json),
                        'confidence', evidence.confidence,
                        'observed_at_ms', evidence.observed_at_ms
                    ) FROM feature_evidence evidence
                    WHERE evidence.app_id = a.app_id
                      AND evidence.feature_name = 'service_shutdown_risk'
                      AND evidence.is_active = 1
                    ORDER BY evidence.observed_at_ms DESC, evidence.evidence_id DESC LIMIT 1
                 ),
                 (
                    SELECT evidence.value_json FROM feature_evidence evidence
                    WHERE evidence.app_id = a.app_id
                      AND evidence.feature_name = 'catalog_taxonomy'
                      AND evidence.is_active = 1
                    ORDER BY evidence.observed_at_ms DESC, evidence.evidence_id DESC LIMIT 1
                 ),
                 p.computed_at_ms,
                 (
                    SELECT r.captured_at_ms FROM review_snapshots r
                    WHERE r.app_id = a.app_id
                    ORDER BY r.captured_at_ms DESC LIMIT 1
                 ),
                 COALESCE(
                    (
                       SELECT CAST(strftime('%s', MAX(d.day_utc)) AS INTEGER) * 1000
                       FROM player_daily d
                       WHERE d.app_id = a.app_id AND d.mean_ccu IS NOT NULL
                    ),
                    (
                       SELECT s.captured_at_ms FROM player_snapshots s
                       WHERE s.app_id = a.app_id AND s.player_count IS NOT NULL
                       ORDER BY s.captured_at_ms DESC LIMIT 1
                    )
                 ),
                 (
                    SELECT price.captured_at_ms FROM price_snapshots price
                    WHERE price.app_id = a.app_id
                    ORDER BY price.captured_at_ms DESC, price.currency ASC LIMIT 1
                 ),
                 a.updated_at_ms
          FROM apps a
          LEFT JOIN multiplayer_profiles p ON p.app_id = a.app_id
          LEFT JOIN app_availability v ON v.app_id = a.app_id
          LEFT JOIN app_media media ON media.app_id = a.app_id
          LEFT JOIN app_localizations loc ON loc.app_id = a.app_id AND loc.language = (
              SELECT language FROM app_localizations l2
              WHERE l2.app_id = a.app_id
              ORDER BY CASE l2.language
                  WHEN 'schinese' THEN 0
                  WHEN 'english' THEN 1
                  WHEN 'en' THEN 2
                  ELSE 9
              END
              LIMIT 1
          )
          WHERE a.app_id = ?1",
        params![app_id],
        map_candidate,
    )
    .optional()
    .map_err(StorageError::from)
}

pub fn list_popular_reviews(
    conn: &Connection,
    app_id: u32,
) -> StorageResult<Vec<PopularReviewRow>> {
    let mut stmt = conn.prepare(
        "SELECT recommendation_id, rank, author_name, author_profile_url, review_text,
                voted_up, votes_up, votes_funny, comment_count, playtime_forever_minutes,
                playtime_at_review_minutes, created_at_s, written_during_early_access
         FROM popular_reviews
         WHERE app_id = ?1
         ORDER BY rank ASC
         LIMIT 10",
    )?;
    let rows = stmt.query_map(params![app_id], |row| {
        Ok(PopularReviewRow {
            recommendation_id: row.get(0)?,
            rank: row.get::<_, i64>(1)?.clamp(1, 10) as u8,
            author_name: row.get(2)?,
            author_profile_url: row.get(3)?,
            review_text: row.get(4)?,
            voted_up: row.get::<_, i64>(5)? != 0,
            votes_up: row.get::<_, i64>(6)?.max(0) as u32,
            votes_funny: row.get::<_, i64>(7)?.max(0) as u32,
            comment_count: row.get::<_, i64>(8)?.max(0) as u32,
            playtime_forever_minutes: row
                .get::<_, Option<i64>>(9)?
                .map(|value| value.max(0) as u32),
            playtime_at_review_minutes: row
                .get::<_, Option<i64>>(10)?
                .map(|value| value.max(0) as u32),
            created_at_ms: row.get::<_, i64>(11)?.saturating_mul(1_000),
            written_during_early_access: row.get::<_, i64>(12)? != 0,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn list_evidence(
    conn: &Connection,
    app_id: u32,
    feature: Option<&str>,
) -> StorageResult<Vec<EvidenceRow>> {
    if let Some(feature) = feature {
        let mut stmt = conn.prepare(
            "SELECT evidence_id, feature_name, value_json, source_type, source_ref, confidence, observed_at_ms
             FROM feature_evidence
             WHERE app_id = ?1 AND feature_name = ?2 AND is_active = 1
             ORDER BY observed_at_ms DESC LIMIT 50",
        )?;
        let rows = stmt.query_map(params![app_id, feature], map_evidence)?;
        return rows.collect::<Result<Vec<_>, _>>().map_err(Into::into);
    }
    let mut stmt = conn.prepare(
        "SELECT evidence_id, feature_name, value_json, source_type, source_ref, confidence, observed_at_ms
         FROM feature_evidence
         WHERE app_id = ?1 AND is_active = 1
         ORDER BY observed_at_ms DESC LIMIT 50",
    )?;
    let rows = stmt.query_map(params![app_id], map_evidence)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceRow {
    pub evidence_id: i64,
    pub feature_name: String,
    pub value_json: String,
    pub source_type: String,
    pub source_ref: String,
    pub confidence: f64,
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalendarItemRow {
    pub app: AppRecord,
    pub review_total: Option<u32>,
    pub cover_url: Option<String>,
}

const CALENDAR_UNDATED_LIMIT: i64 = 100;

pub fn list_calendar(
    conn: &Connection,
    from_date: &str,
    to_date: &str,
    state: &str,
) -> StorageResult<(Vec<CalendarItemRow>, Vec<CalendarItemRow>)> {
    if !crate::util::is_iso_day(from_date) || !crate::util::is_iso_day(to_date) {
        return Err(StorageError::validation(
            "calendar dates must use valid YYYY-MM-DD values",
        ));
    }
    if from_date > to_date {
        return Err(StorageError::validation(
            "calendar from date must not be after to date",
        ));
    }
    if !matches!(state, "upcoming" | "recent") {
        return Err(StorageError::validation(
            "calendar state must be upcoming or recent",
        ));
    }
    let from_day = crate::util::iso_day_to_unix_days(from_date).expect("validated above");
    let to_day = crate::util::iso_day_to_unix_days(to_date).expect("validated above");
    if to_day - from_day > 366 {
        return Err(StorageError::validation(
            "calendar date range must not exceed one year",
        ));
    }
    let mut dated = Vec::new();
    let mut undated = Vec::new();
    // Build the candidate scope before joining review/media snapshots. Upcoming
    // entries without a date are useful for an explicit "undated" bucket, but
    // returning every stale catalog row made the calendar both noisy and slow.
    let state_predicate = if state == "upcoming" {
        "a.release_state IN ('upcoming', 'coming_soon')"
    } else {
        "a.release_state = 'released'"
    };
    let undated_predicate = if state == "upcoming" {
        "a.release_date IS NULL"
    } else {
        "0"
    };
    let sql = format!(
        "WITH eligible_apps AS MATERIALIZED (
             SELECT a.app_id
             FROM apps a
             WHERE {state_predicate}
               AND a.app_type IN ('game', 'demo', 'playtest')
               AND (
                   EXISTS (
                       SELECT 1 FROM feature_evidence evidence
                       WHERE evidence.app_id = a.app_id
                         AND evidence.feature_name = 'category_hint'
                         AND evidence.is_active = 1
                         AND evidence.confidence >= 0.3
                   )
                   OR EXISTS (
                       SELECT 1 FROM multiplayer_profiles profile
                       WHERE profile.app_id = a.app_id
                         AND (
                             profile.dominant_mode IS NOT NULL
                             OR profile.private_session IS NOT NULL
                             OR profile.online_coop IS NOT NULL
                             OR profile.self_hosted_server IS NOT NULL
                             OR profile.drop_in_out IS NOT NULL
                             OR profile.crossplay IS NOT NULL
                             OR profile.recommended_max_players IS NOT NULL
                         )
                   )
               )
         ),
         dated_scope AS MATERIALIZED (
             SELECT a.app_id
             FROM eligible_apps eligible
             JOIN apps a ON a.app_id = eligible.app_id
             WHERE a.release_date >= ?1 AND a.release_date <= ?2
         ),
         undated_scope AS MATERIALIZED (
             SELECT a.app_id
             FROM eligible_apps eligible
             JOIN apps a ON a.app_id = eligible.app_id
             WHERE {undated_predicate}
             ORDER BY a.updated_at_ms DESC, a.canonical_name ASC
             LIMIT {CALENDAR_UNDATED_LIMIT}
         ),
         calendar_scope AS (
             SELECT app_id FROM dated_scope
             UNION ALL
             SELECT app_id FROM undated_scope
         )
         SELECT a.app_id, a.app_type, a.canonical_name, a.release_state, a.release_date,
                a.release_date_raw, a.release_date_precision, a.is_early_access,
                a.current_data_confidence, a.source_modified_at_ms,
                a.created_at_ms, a.updated_at_ms,
                (
                    SELECT review.total_reviews
                    FROM review_snapshots review
                    WHERE review.app_id = a.app_id
                    ORDER BY review.captured_at_ms DESC, review.language_scope ASC
                    LIMIT 1
                ),
                media.capsule_url
         FROM calendar_scope scope
         JOIN apps a ON a.app_id = scope.app_id
         LEFT JOIN app_media media ON media.app_id = a.app_id
         ORDER BY a.release_date IS NULL, a.release_date, a.canonical_name"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![from_date, to_date], |row| {
        Ok(CalendarItemRow {
            app: AppRecord {
                app_id: row.get::<_, i64>(0)? as u32,
                app_type: row.get(1)?,
                canonical_name: row.get(2)?,
                release_state: row.get(3)?,
                release_date: row.get(4)?,
                release_date_raw: row.get(5)?,
                release_date_precision: row.get(6)?,
                is_early_access: sql_to_opt_bool(row.get(7)?),
                current_data_confidence: row.get(8)?,
                source_modified_at_ms: row.get(9)?,
                created_at_ms: row.get(10)?,
                updated_at_ms: row.get(11)?,
            },
            review_total: row.get(12)?,
            cover_url: row.get(13)?,
        })
    })?;
    for row in rows {
        let item = row?;
        match &item.app.release_date {
            Some(d) if d.as_str() >= from_date && d.as_str() <= to_date => {
                dated.push(item);
            }
            Some(_) | None => undated.push(item),
        }
    }
    Ok((dated, undated))
}

/// Resolve the mode string shown in feeds/detail chips.
/// - Prefer an explicit profile mode (not empty / "unknown")
/// - Map competitive → pvp for consistent UI labels
/// - If only online_coop is known, treat as coop
pub fn resolve_display_dominant_mode(
    stored: Option<&str>,
    online_coop: Option<bool>,
) -> Option<String> {
    if let Some(raw) = stored.map(str::trim).filter(|s| !s.is_empty()) {
        return match ModeFamily::from_alias(raw) {
            ModeFamily::MatchmadePvp => Some("pvp".to_owned()),
            ModeFamily::Unknown => online_coop
                .is_some_and(|online| online)
                .then(|| "coop".to_owned()),
            _ => Some(raw.to_ascii_lowercase()),
        };
    }
    if online_coop == Some(true) {
        return Some("coop".to_owned());
    }
    None
}

fn map_candidate(row: &rusqlite::Row<'_>) -> rusqlite::Result<GameCandidateRow> {
    let platforms_json: String = row.get(17)?;
    let languages_json: String = row.get(18)?;
    let final_price_minor: Option<i64> = row.get(22)?;
    // Currency without a matching regional price is not a usable observation.
    // Keeping it `None` prevents callers from treating an unknown price as a
    // known zero-cost or in-budget result.
    let price_currency: Option<String> = if final_price_minor.is_some() {
        row.get(23)?
    } else {
        None
    };
    let matchmaking_json: Option<String> = row.get(34)?;
    let public_world_json: Option<String> = row.get(35)?;
    let shutdown_json: Option<String> = row.get(36)?;
    let taxonomy_json: Option<String> = row.get(37)?;
    let (matchmaking_core, matchmaking_core_confidence, matchmaking_observed_at_ms) =
        parse_json_bool_signal(matchmaking_json.as_deref());
    let (public_world_dependency, public_world_dependency_confidence, public_world_observed_at_ms) =
        parse_json_bool_signal(public_world_json.as_deref());
    let (service_shutdown_risk, service_shutdown_risk_confidence, shutdown_observed_at_ms) =
        parse_json_bool_signal(shutdown_json.as_deref());
    let (taxonomy_tags, publisher) = parse_catalog_taxonomy(taxonomy_json.as_deref());
    let profile_observed_at_ms = [
        row.get::<_, Option<i64>>(38)?,
        matchmaking_observed_at_ms,
        public_world_observed_at_ms,
        shutdown_observed_at_ms,
    ]
    .into_iter()
    .flatten()
    .max();
    Ok(GameCandidateRow {
        app_id: row.get::<_, i64>(0)? as u32,
        name: row.get(1)?,
        app_type: row.get(2)?,
        release_state: row.get(3)?,
        release_date: row.get(4)?,
        release_date_raw: row.get(25)?,
        release_date_precision: row.get(26)?,
        cover_url: row.get(27)?,
        cover_updated_at_ms: row.get(28)?,
        short_description: row.get(29)?,
        dominant_mode: row.get(5)?,
        private_session: sql_to_opt_bool(row.get(6)?),
        online_coop: sql_to_opt_bool(row.get(7)?),
        self_hosted_server: sql_to_opt_bool(row.get(8)?),
        drop_in_out: sql_to_opt_bool(row.get(30)?),
        crossplay: sql_to_opt_bool(row.get(31)?),
        service_status: row.get(32)?,
        matchmaking_core,
        matchmaking_core_confidence,
        public_world_dependency,
        public_world_dependency_confidence,
        service_shutdown_risk,
        service_shutdown_risk_confidence,
        recommended_min: row.get::<_, Option<i64>>(9)?.map(|v| v.clamp(0, 255) as u8),
        recommended_max: row
            .get::<_, Option<i64>>(10)?
            .map(|v| v.clamp(0, 255) as u8),
        profile_confidence: row.get(11)?,
        total_reviews: row.get::<_, Option<i64>>(12)?.map(|v| v as u32),
        total_positive: row.get::<_, Option<i64>>(13)?.map(|v| v as u32),
        latest_ccu: row.get::<_, Option<i64>>(14)?.map(|v| v as u32),
        wilson_lower: row.get(15)?,
        typical_ccu_7d: row.get::<_, Option<i64>>(16)?.map(|v| v as u32),
        activity_percentile: None,
        activity_momentum: row.get(33)?,
        taxonomy_tags,
        publisher,
        platforms: parse_string_list(17, &platforms_json)?,
        languages: parse_string_list(18, &languages_json)?,
        typical_session_minutes_min: row.get::<_, Option<i64>>(19)?.map(|v| v as u32),
        typical_session_minutes_max: row.get::<_, Option<i64>>(20)?.map(|v| v as u32),
        is_free: sql_to_opt_bool(row.get(21)?),
        final_price_minor,
        price_currency,
        has_demo: row.get::<_, i64>(24)? != 0,
        profile_observed_at_ms,
        reviews_observed_at_ms: row.get(39)?,
        activity_observed_at_ms: row.get(40)?,
        price_observed_at_ms: row.get(41)?,
        release_observed_at_ms: row.get(42)?,
    })
}

fn parse_json_bool_signal(value: Option<&str>) -> (Option<bool>, Option<f64>, Option<i64>) {
    let Some(parsed) = value.and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
    else {
        return (None, None, None);
    };
    let (value, confidence, observed_at_ms) = match parsed {
        serde_json::Value::Object(object) => {
            let value = object
                .get("value")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let confidence = object
                .get("confidence")
                .and_then(serde_json::Value::as_f64)
                .map(|value| value.clamp(0.0, 1.0));
            let observed_at_ms = object
                .get("observed_at_ms")
                .and_then(serde_json::Value::as_i64);
            (value, confidence, observed_at_ms)
        }
        value => (value, None, None),
    };
    let value = match value {
        serde_json::Value::Bool(value) => Some(value),
        serde_json::Value::Number(value) if value.as_i64() == Some(1) => Some(true),
        serde_json::Value::Number(value) if value.as_i64() == Some(0) => Some(false),
        serde_json::Value::String(value) if value.eq_ignore_ascii_case("true") => Some(true),
        serde_json::Value::String(value) if value.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    };
    let observed_at_ms = value.and(observed_at_ms);
    (value, confidence, observed_at_ms)
}

fn parse_catalog_taxonomy(value: Option<&str>) -> (Vec<String>, Option<String>) {
    let Some(serde_json::Value::Object(object)) =
        value.and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
    else {
        return (Vec::new(), None);
    };

    let mut tags = Vec::new();
    for key in ["categories", "genres"] {
        if let Some(serde_json::Value::Array(values)) = object.get(key) {
            tags.extend(values.iter().filter_map(|value| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_ascii_lowercase)
            }));
        }
    }
    tags.sort_unstable();
    tags.dedup();
    let publisher = object
        .get("publishers")
        .and_then(serde_json::Value::as_array)
        .and_then(|values| values.iter().find_map(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    (tags, publisher)
}

fn parse_string_list(index: usize, value: &str) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

fn map_evidence(row: &rusqlite::Row<'_>) -> rusqlite::Result<EvidenceRow> {
    Ok(EvidenceRow {
        evidence_id: row.get(0)?,
        feature_name: row.get(1)?,
        value_json: row.get(2)?,
        source_type: row.get(3)?,
        source_ref: row.get(4)?,
        confidence: row.get(5)?,
        observed_at_ms: row.get(6)?,
    })
}

pub fn data_updated_at_ms(conn: &Connection) -> StorageResult<i64> {
    let value = conn.query_row(
        "SELECT COALESCE(MAX(updated_at_ms), 0)
         FROM (
             -- Collection commands bracket writes with a source run, and the
             -- worker records completion in data_refresh_state. Reading those
             -- durable watermarks avoids full scans of every snapshot table.
             SELECT MAX(updated_at_ms) AS updated_at_ms FROM data_refresh_state
             UNION ALL SELECT MAX(updated_at_ms) FROM apps
             UNION ALL SELECT MAX(started_at_ms, COALESCE(finished_at_ms, 0))
                 FROM source_runs
                 WHERE run_id = (SELECT MAX(run_id) FROM source_runs)
             UNION ALL SELECT MAX(MAX(created_at_ms, COALESCE(revoked_at_ms, 0))) FROM curation_overrides
          )",
        [],
        |row| row.get(0),
    )?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{
        CALENDAR_UNDATED_LIMIT, GameCandidateRow, list_calendar, list_candidates,
        resolve_display_dominant_mode, section_matches,
    };
    use crate::Database;
    use mpgs_domain::{
        FeedSection, ModeFamily, MultiplayerSignals, RankingSignals, RecommendationConfig,
    };

    const CUTOFF: &str = "2026-01-01";
    const TODAY: &str = "2026-07-28";

    fn candidate(release_date: &str) -> GameCandidateRow {
        GameCandidateRow {
            app_id: 1,
            name: "test-game".into(),
            app_type: "game".into(),
            release_state: "released".into(),
            release_date: Some(release_date.into()),
            release_date_raw: None,
            release_date_precision: None,
            cover_url: None,
            cover_updated_at_ms: None,
            short_description: None,
            dominant_mode: Some("coop".into()),
            private_session: Some(true),
            online_coop: Some(true),
            self_hosted_server: Some(true),
            drop_in_out: Some(true),
            crossplay: Some(false),
            service_status: Some("active".into()),
            matchmaking_core: Some(false),
            matchmaking_core_confidence: Some(0.9),
            public_world_dependency: Some(false),
            public_world_dependency_confidence: Some(0.9),
            service_shutdown_risk: Some(false),
            service_shutdown_risk_confidence: Some(0.9),
            recommended_min: Some(1),
            recommended_max: Some(4),
            profile_confidence: Some(0.9),
            total_reviews: Some(3_000),
            total_positive: Some(2_800),
            latest_ccu: None,
            wilson_lower: Some(0.82),
            typical_ccu_7d: None,
            activity_percentile: None,
            activity_momentum: None,
            taxonomy_tags: Vec::new(),
            publisher: None,
            platforms: vec!["windows".into()],
            languages: vec!["schinese".into()],
            typical_session_minutes_min: None,
            typical_session_minutes_max: None,
            is_free: Some(false),
            final_price_minor: None,
            price_currency: None,
            has_demo: false,
            profile_observed_at_ms: None,
            reviews_observed_at_ms: None,
            activity_observed_at_ms: None,
            price_observed_at_ms: None,
            release_observed_at_ms: None,
        }
    }

    fn strong_friend_signals() -> RankingSignals {
        RankingSignals {
            multiplayer: MultiplayerSignals {
                private_session: 1.0,
                self_host_or_dedicated: 1.0,
                online_coop: 1.0,
                group_size_fit: 1.0,
                low_public_population_dependency: 1.0,
                drop_in_out: 0.8,
                cross_platform_fit: 0.5,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn isolated_classic_config() -> RecommendationConfig {
        RecommendationConfig {
            // Keep this fixture out of PopularLegacy so ClassicLegacy gates are isolated.
            popular_min_ccu: 10_000,
            ..RecommendationConfig::default()
        }
    }

    #[test]
    fn recent_release_does_not_pre_gate_on_friend_fit() {
        let row = candidate("2026-07-01");
        let signals = RankingSignals::default();
        let config = RecommendationConfig {
            recent_min_friend_fit: 1.0,
            ..RecommendationConfig::default()
        };

        assert!(section_matches(
            FeedSection::RecentRelease,
            &row,
            &signals,
            CUTOFF,
            TODAY,
            &config,
        ));
    }

    #[test]
    fn classic_legacy_enforces_review_and_wilson_but_not_friend_fit() {
        let mut row = candidate("2020-01-01");
        let signals = strong_friend_signals();
        let mut config = isolated_classic_config();
        assert!(section_matches(
            FeedSection::ClassicLegacy,
            &row,
            &signals,
            CUTOFF,
            TODAY,
            &config,
        ));

        config.classic_min_reviews = 3_001;
        assert!(!section_matches(
            FeedSection::ClassicLegacy,
            &row,
            &signals,
            CUTOFF,
            TODAY,
            &config,
        ));
        config.classic_min_reviews = 3_000;

        config.classic_min_wilson = 0.83;
        assert!(!section_matches(
            FeedSection::ClassicLegacy,
            &row,
            &signals,
            CUTOFF,
            TODAY,
            &config,
        ));
        config.classic_min_wilson = 0.82;

        config.classic_min_friend_fit = 1.0;
        assert!(section_matches(
            FeedSection::ClassicLegacy,
            &row,
            &signals,
            CUTOFF,
            TODAY,
            &config,
        ));
        row.total_reviews = None;
        assert!(!section_matches(
            FeedSection::ClassicLegacy,
            &row,
            &signals,
            CUTOFF,
            TODAY,
            &config,
        ));
        row.total_reviews = Some(3_000);
        row.wilson_lower = None;
        assert!(!section_matches(
            FeedSection::ClassicLegacy,
            &row,
            &signals,
            CUTOFF,
            TODAY,
            &config,
        ));
    }

    #[test]
    fn classic_public_activity_gate_preserves_unknown_and_private_path_semantics() {
        let mut row = candidate("2020-01-01");
        row.private_session = Some(false);
        row.online_coop = Some(false);
        row.self_hosted_server = Some(false);
        let mut config = isolated_classic_config();
        config.classic_min_friend_fit = 0.0;
        config.classic_public_min_ccu = 1_000;

        let unknown_dependency = RankingSignals::default();
        assert!(section_matches(
            FeedSection::ClassicLegacy,
            &row,
            &unknown_dependency,
            CUTOFF,
            TODAY,
            &config,
        ));

        let public_dependency = RankingSignals {
            multiplayer: MultiplayerSignals {
                matchmaking_core: 0.6,
                public_world_dependency: 0.6,
                ..Default::default()
            },
            ..Default::default()
        };
        row.latest_ccu = Some(999);
        assert!(!section_matches(
            FeedSection::ClassicLegacy,
            &row,
            &public_dependency,
            CUTOFF,
            TODAY,
            &config,
        ));

        row.latest_ccu = Some(1_000);
        assert!(section_matches(
            FeedSection::ClassicLegacy,
            &row,
            &public_dependency,
            CUTOFF,
            TODAY,
            &config,
        ));

        row.latest_ccu = None;
        row.private_session = Some(true);
        assert!(section_matches(
            FeedSection::ClassicLegacy,
            &row,
            &public_dependency,
            CUTOFF,
            TODAY,
            &config,
        ));
    }

    #[test]
    fn mode_aliases_share_one_typed_ranking_family() {
        for alias in ["competitive", "versus", "pvp", "pvp_only"] {
            let mut row = candidate("2020-01-01");
            row.dominant_mode = Some(alias.into());
            row.matchmaking_core = None;
            assert_eq!(row.mode_family(), ModeFamily::MatchmadePvp);
            assert_eq!(row.resolved_matchmaking_core(), Some(true));
            assert_eq!(row.to_ranking_signals().multiplayer.matchmaking_core, 1.0);
            assert_eq!(
                resolve_display_dominant_mode(Some(alias), None).as_deref(),
                Some("pvp")
            );
        }
    }

    #[test]
    fn ranking_signals_use_stored_capabilities_and_neutral_missing_momentum() {
        let mut row = candidate("2020-01-01");
        row.drop_in_out = Some(true);
        row.crossplay = Some(true);
        row.service_status = Some("shutdown_announced".into());
        row.service_shutdown_risk = None;
        row.activity_momentum = None;
        let signals = row.to_ranking_signals();

        assert_eq!(signals.multiplayer.drop_in_out, 1.0);
        assert_eq!(signals.multiplayer.cross_platform_fit, 1.0);
        assert_eq!(signals.multiplayer.service_shutdown_risk, 0.75);
        assert_eq!(signals.multiplayer_confidence.drop_in_out, 0.9);
        assert!(signals.has_multiplayer_confidence);
        assert_eq!(signals.momentum, 0.5);
        assert!(signals.evidence > 0.5);
    }

    #[test]
    fn unknown_profile_values_are_not_positive_capability_proof() {
        let mut row = candidate("2020-01-01");
        row.dominant_mode = None;
        row.private_session = None;
        row.online_coop = None;
        row.self_hosted_server = None;
        row.drop_in_out = None;
        row.crossplay = None;
        row.service_status = None;
        row.matchmaking_core = None;
        row.public_world_dependency = None;
        row.service_shutdown_risk = None;
        row.recommended_min = None;
        row.recommended_max = None;
        row.profile_confidence = None;
        row.latest_ccu = Some(0);
        let signals = row.to_ranking_signals();

        assert_eq!(row.mode_family(), ModeFamily::Unknown);
        assert_eq!(signals.multiplayer.private_session, 0.5);
        assert_eq!(signals.multiplayer.drop_in_out, 0.5);
        assert_eq!(signals.multiplayer.cross_platform_fit, 0.5);
        assert_eq!(signals.multiplayer_confidence.private_session, 0.0);
        assert_eq!(signals.multiplayer_confidence.drop_in_out, 0.0);
        assert!(signals.data_confidence > 0.0);
        assert_eq!(signals.evidence, 0.0);
        assert_eq!(
            signals.popularity, 0.25,
            "one low-confidence zero-CCU sample shrinks toward the neutral prior"
        );
    }

    #[test]
    fn date_signals_are_continuous_and_respect_release_precision() {
        let mut fresh = candidate("2026-07-28");
        fresh.release_date_precision = Some("day".into());
        let fresh_signals = fresh.to_ranking_signals_at(TODAY);
        assert_eq!(fresh_signals.freshness, 1.0);
        assert_eq!(fresh_signals.release_date_confidence, 1.0);

        let mut older = candidate("2026-01-28");
        older.release_date_precision = Some("month".into());
        let older_signals = older.to_ranking_signals_at(TODAY);
        assert!(older_signals.freshness < fresh_signals.freshness);
        assert_eq!(older_signals.release_date_confidence, 0.65);
        assert!(older_signals.longevity > fresh_signals.longevity);

        let mut upcoming = candidate("2026-08-27");
        upcoming.release_state = "upcoming".into();
        let far = upcoming.to_ranking_signals_at(TODAY);
        upcoming.release_date = Some("2026-08-02".into());
        let near = upcoming.to_ranking_signals_at(TODAY);
        assert!(near.release_proximity > far.release_proximity);
    }

    #[test]
    fn activity_percentiles_use_midrank_and_leave_missing_values_unknown() {
        let mut rows = vec![
            candidate("2020-01-01"),
            candidate("2020-01-02"),
            candidate("2020-01-03"),
            candidate("2020-01-04"),
        ];
        rows[0].typical_ccu_7d = Some(100);
        rows[1].typical_ccu_7d = Some(100);
        rows[2].typical_ccu_7d = Some(1_000);
        super::assign_activity_percentiles(&mut rows);

        assert_eq!(rows[0].activity_percentile, Some(0.25));
        assert_eq!(rows[1].activity_percentile, Some(0.25));
        assert_eq!(rows[2].activity_percentile, Some(1.0));
        assert_eq!(rows[3].activity_percentile, None);
    }

    #[test]
    fn upcoming_requires_known_release_within_30_days() {
        let signals = strong_friend_signals();
        let config = RecommendationConfig::default();
        let mut row = candidate("2026-08-27");
        row.release_state = "upcoming".into();
        assert!(section_matches(
            FeedSection::Upcoming,
            &row,
            &signals,
            CUTOFF,
            TODAY,
            &config,
        ));

        row.release_date = Some("2026-08-28".into());
        assert!(!section_matches(
            FeedSection::Upcoming,
            &row,
            &signals,
            CUTOFF,
            TODAY,
            &config,
        ));
        row.release_date = None;
        assert!(!section_matches(
            FeedSection::Upcoming,
            &row,
            &signals,
            CUTOFF,
            TODAY,
            &config,
        ));
    }

    #[test]
    fn playable_demo_remains_eligible_without_parent_release_date() {
        let signals = strong_friend_signals();
        let config = RecommendationConfig::default();
        let mut row = candidate("2020-01-01");
        row.app_type = "demo".into();
        row.release_state = "released".into();
        row.release_date = None;
        assert!(section_matches(
            FeedSection::Upcoming,
            &row,
            &signals,
            CUTOFF,
            TODAY,
            &config,
        ));
    }

    #[test]
    fn calendar_only_returns_multiplayer_games_and_batches_cover_data() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn(|conn| {
            for (app_id, app_type, name, date) in [
                (101, "game", "multiplayer-game", Some("2026-08-10")),
                (102, "dlc", "multiplayer-dlc", Some("2026-08-11")),
                (103, "game", "single-player", Some("2026-08-12")),
                (104, "demo", "undated-demo", None),
            ] {
                crate::catalog::upsert_app(
                    conn,
                    app_id,
                    app_type,
                    name,
                    "coming_soon",
                    date,
                    date.map(|_| "day"),
                    None,
                    1,
                )?;
            }
            for app_id in [101, 102, 104] {
                conn.execute(
                    "INSERT INTO multiplayer_profiles (
                         app_id, dominant_mode, recommended_min_players,
                         profile_confidence, computed_at_ms
                     ) VALUES (?1, 'generic_multiplayer', 2, 0.3, 1)",
                    [app_id],
                )?;
            }
            conn.execute(
                "INSERT INTO app_media(app_id, capsule_url, source, updated_at_ms)
                 VALUES (101, 'https://example.test/101.jpg', 'test', 1)",
                [],
            )?;

            let (dated, undated) = list_calendar(conn, "2026-08-01", "2026-08-31", "upcoming")?;
            assert_eq!(
                dated.iter().map(|row| row.app.app_id).collect::<Vec<_>>(),
                vec![101]
            );
            assert_eq!(
                dated[0].cover_url.as_deref(),
                Some("https://example.test/101.jpg")
            );
            assert_eq!(
                undated.iter().map(|row| row.app.app_id).collect::<Vec<_>>(),
                vec![104]
            );
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn calendar_bounds_undated_upcoming_items() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn(|conn| {
            for app_id in 200..350 {
                crate::catalog::upsert_app(
                    conn,
                    app_id,
                    "game",
                    &format!("undated-{app_id}"),
                    "coming_soon",
                    None,
                    None,
                    None,
                    i64::from(app_id),
                )?;
                conn.execute(
                    "INSERT INTO multiplayer_profiles (
                         app_id, dominant_mode, recommended_min_players,
                         profile_confidence, computed_at_ms
                     ) VALUES (?1, 'generic_multiplayer', 2, 0.3, ?2)",
                    (app_id, app_id),
                )?;
            }

            let (dated, undated) = list_calendar(conn, "2026-08-01", "2026-08-31", "upcoming")?;
            assert!(dated.is_empty());
            assert_eq!(undated.len(), CALENDAR_UNDATED_LIMIT as usize);
            assert_eq!(undated.first().map(|row| row.app.app_id), Some(250));
            assert_eq!(undated.last().map(|row| row.app.app_id), Some(349));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn popular_section_does_not_pre_gate_competitive_titles_on_friend_fit() {
        let mut row = candidate("2020-01-01");
        row.typical_ccu_7d = Some(2_000);
        let signals = RankingSignals {
            multiplayer: MultiplayerSignals {
                matchmaking_core: 1.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let config = RecommendationConfig {
            popular_min_friend_fit: 1.0,
            ..RecommendationConfig::default()
        };
        assert!(section_matches(
            FeedSection::PopularLegacy,
            &row,
            &signals,
            CUTOFF,
            TODAY,
            &config,
        ));
    }

    #[test]
    fn candidate_query_uses_mean_ccu_and_requires_seven_of_ten_days_for_momentum() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn(|conn| {
            crate::catalog::upsert_app(
                conn,
                42,
                "game",
                "activity-test",
                "released",
                Some("2026-07-01"),
                Some("day"),
                None,
                1,
            )?;
            conn.execute(
                "INSERT INTO multiplayer_profiles (
                    app_id, dominant_mode, online_coop, recommended_min_players,
                    recommended_max_players, profile_confidence, computed_at_ms
                 ) VALUES (42, 'coop', 1, 2, 4, 0.8, 1)",
                [],
            )?;
            for day in 19..=28 {
                let mean = if day >= 26 { 200.0 } else { 100.0 };
                conn.execute(
                    "INSERT INTO player_daily (
                        app_id, day_utc, mean_ccu, median_approx_ccu,
                        sample_count, missing_rate, updated_at_ms
                     ) VALUES (?1, ?2, ?3, 999999, 3, 0, 1)",
                    rusqlite::params![42, format!("2026-07-{day:02}"), mean],
                )?;
            }

            let rows = list_candidates(
                conn,
                FeedSection::RecentRelease,
                CUTOFF,
                TODAY,
                "CNY",
                &RecommendationConfig::default(),
                10,
            )?;
            assert_eq!(rows.len(), 1);
            // Last seven calendar days: 4 * 100 + 3 * 200 = 1000 / 7.
            assert_eq!(rows[0].typical_ccu_7d, Some(142));
            assert_eq!(rows[0].activity_momentum, Some(0.625));
            assert!(rows[0].activity_observed_at_ms.is_some());
            assert_eq!(rows[0].release_observed_at_ms, Some(1));

            conn.execute(
                "DELETE FROM player_daily WHERE app_id = 42 AND day_utc < '2026-07-23'",
                [],
            )?;
            let rows = list_candidates(
                conn,
                FeedSection::RecentRelease,
                CUTOFF,
                TODAY,
                "CNY",
                &RecommendationConfig::default(),
                10,
            )?;
            assert_eq!(rows[0].activity_momentum, None);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn classic_prefilter_keeps_activity_percentiles_from_the_full_ranked_scope() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn(|conn| {
            for (app_id, name) in [(501, "eligible-classic"), (502, "low-review-title")] {
                crate::catalog::upsert_app(
                    conn,
                    app_id,
                    "game",
                    name,
                    "released",
                    Some("2020-01-01"),
                    Some("day"),
                    None,
                    1,
                )?;
            }
            conn.execute_batch(
                "INSERT INTO review_snapshots (
                     app_id, region_scope, language_scope, captured_at_ms,
                     total_positive, total_negative, total_reviews, wilson_lower,
                     filter_offtopic_activity, parameter_hash, content_hash, source
                 ) VALUES
                 (501, 'all', 'all', CAST(strftime('%s', '2026-07-28') AS INTEGER) * 1000,
                    2800, 200, 3000, 0.83, 1, 'p1', 'r1', 'test'),
                 (502, 'all', 'all', CAST(strftime('%s', '2026-07-28') AS INTEGER) * 1000,
                    90, 10, 100, 0.90, 1, 'p2', 'r2', 'test');
                 INSERT INTO player_snapshots (
                     app_id, captured_at_ms, player_count, result_code,
                     content_hash, source
                 ) VALUES
                 (501, CAST(strftime('%s', '2026-07-28') AS INTEGER) * 1000,
                    100, 1, 'c1', 'test'),
                 (502, CAST(strftime('%s', '2026-07-28') AS INTEGER) * 1000,
                    1000, 1, 'c2', 'test');",
            )?;

            let rows = list_candidates(
                conn,
                FeedSection::ClassicLegacy,
                CUTOFF,
                TODAY,
                "CNY",
                &RecommendationConfig::default(),
                10,
            )?;

            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].app_id, 501);
            assert_eq!(rows[0].activity_percentile, Some(0.0));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn candidate_query_respects_signal_ttl_and_regional_price_identity() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn(|conn| {
            crate::catalog::upsert_app(
                conn,
                77,
                "game",
                "freshness-test",
                "released",
                Some("2026-07-01"),
                Some("day"),
                None,
                1,
            )?;
            conn.execute(
                "INSERT INTO multiplayer_profiles (
                    app_id, dominant_mode, private_session, online_coop,
                    recommended_min_players, recommended_max_players,
                    profile_confidence, computed_at_ms
                 ) VALUES (
                    77, 'coop', 1, 1, 2, 4, 0.8,
                    CAST(strftime('%s', '2026-07-01') AS INTEGER) * 1000
                 )",
                [],
            )?;
            conn.execute_batch(
                "INSERT INTO feature_evidence (
                    app_id, feature_name, value_json, source_type, source_ref,
                    confidence, observed_at_ms, expires_at_ms, is_active
                 ) VALUES
                 (77, 'matchmaking_core', 'true', 'test', 'stale', 0.95,
                    CAST(strftime('%s', '2025-01-01') AS INTEGER) * 1000, NULL, 1),
                 (77, 'public_world_dependency', 'false', 'test', 'fresh', 0.72,
                    CAST(strftime('%s', '2026-07-27') AS INTEGER) * 1000, NULL, 1),
                 (77, 'service_shutdown_risk', 'true', 'test', 'expired', 0.99,
                    CAST(strftime('%s', '2026-07-27') AS INTEGER) * 1000,
                    CAST(strftime('%s', '2026-07-27') AS INTEGER) * 1000, 1);
                 INSERT INTO price_snapshots (
                    app_id, country_code, currency, captured_at_ms,
                    initial_price_minor, final_price_minor, discount_percent,
                    is_purchasable, package_id, source
                 ) VALUES
                 (77, 'US', 'USD', CAST(strftime('%s', '2026-07-27') AS INTEGER) * 1000,
                    3000, 2500, 17, 1, NULL, 'test'),
                 (77, 'CA', 'USD', CAST(strftime('%s', '2026-07-28') AS INTEGER) * 1000,
                    1500, 1200, 20, 1, NULL, 'test'),
                 (77, 'CN', 'CNY', CAST(strftime('%s', '2026-07-20') AS INTEGER) * 1000,
                    1000, 800, 20, 1, NULL, 'test');",
            )?;

            let usd = list_candidates(
                conn,
                FeedSection::RecentRelease,
                CUTOFF,
                TODAY,
                "USD",
                &RecommendationConfig::default(),
                10,
            )?;
            assert_eq!(usd.len(), 1);
            assert_eq!(usd[0].final_price_minor, Some(2_500));
            assert_eq!(usd[0].price_currency.as_deref(), Some("USD"));
            assert!(usd[0].profile_observed_at_ms.is_some());
            assert!(usd[0].price_observed_at_ms.is_some());
            assert_eq!(usd[0].reviews_observed_at_ms, None);
            assert_eq!(usd[0].activity_observed_at_ms, None);
            assert_eq!(usd[0].release_observed_at_ms, Some(1));
            assert_eq!(usd[0].matchmaking_core, None, "stale evidence is unknown");
            assert_eq!(usd[0].public_world_dependency, Some(false));
            assert_eq!(usd[0].public_world_dependency_confidence, Some(0.72));
            assert_eq!(
                usd[0].service_shutdown_risk, None,
                "expired evidence is unknown"
            );

            let cny = list_candidates(
                conn,
                FeedSection::RecentRelease,
                CUTOFF,
                TODAY,
                "CNY",
                &RecommendationConfig::default(),
                10,
            )?;
            assert_eq!(cny[0].final_price_minor, None, "stale price is unknown");
            assert_eq!(cny[0].price_currency, None);
            assert_eq!(cny[0].price_observed_at_ms, None);

            let eur = list_candidates(
                conn,
                FeedSection::RecentRelease,
                CUTOFF,
                TODAY,
                "EUR",
                &RecommendationConfig::default(),
                10,
            )?;
            assert_eq!(eur[0].final_price_minor, None);
            assert_eq!(eur[0].price_currency, None);

            conn.execute(
                "UPDATE multiplayer_profiles
                 SET computed_at_ms = CAST(strftime('%s', '2026-01-01') AS INTEGER) * 1000
                 WHERE app_id = 77",
                [],
            )?;
            let stale_profile = list_candidates(
                conn,
                FeedSection::RecentRelease,
                CUTOFF,
                TODAY,
                "USD",
                &RecommendationConfig::default(),
                10,
            )?;
            assert_eq!(stale_profile[0].dominant_mode, None);
            assert_eq!(stale_profile[0].private_session, None);
            assert_eq!(stale_profile[0].profile_confidence, None);
            assert!(
                stale_profile[0].profile_observed_at_ms.is_some(),
                "fresh standalone multiplayer evidence still supplies freshness"
            );
            Ok(())
        })
        .unwrap();
    }
}
