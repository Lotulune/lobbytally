//! Read models for feeds, search, calendar, and game detail.

use mpgs_domain::{
    CandidateAvailability, FeedSection, MultiplayerSignals, RankingSignals, RecommendationConfig,
    SteamAppId, friend_fit,
};
use rusqlite::{Connection, OptionalExtension, named_params, params, types::Type};

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
    pub recommended_min: Option<u8>,
    pub recommended_max: Option<u8>,
    pub profile_confidence: Option<f64>,
    pub total_reviews: Option<u32>,
    pub total_positive: Option<u32>,
    pub latest_ccu: Option<u32>,
    pub wilson_lower: Option<f64>,
    pub typical_ccu_7d: Option<u32>,
    pub platforms: Vec<String>,
    pub languages: Vec<String>,
    pub typical_session_minutes_min: Option<u32>,
    pub typical_session_minutes_max: Option<u32>,
    pub is_free: Option<bool>,
    pub final_price_minor: Option<i64>,
    pub price_currency: Option<String>,
    pub has_demo: bool,
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

    pub fn to_ranking_signals(&self) -> RankingSignals {
        let quality =
            self.wilson_lower
                .unwrap_or_else(|| match (self.total_positive, self.total_reviews) {
                    (Some(pos), Some(total)) if total > 0 => f64::from(pos) / f64::from(total),
                    _ => 0.5,
                });
        let popularity = self
            .typical_ccu_7d
            .or(self.latest_ccu)
            .map(|c| (1.0 + (c as f64).ln()).min(12.0) / 12.0)
            .unwrap_or(0.3);
        let confidence = self.profile_confidence.unwrap_or(0.4);
        let mode = self.display_dominant_mode().unwrap_or_default();
        let mode = mode.as_str();
        let matchmaking_core = mode.contains("match") || mode.contains("competitive");
        let public_world_mode = mode.contains("mmo") || mode.contains("public");
        // Dedicated servers for matchmaking-core titles do not count as friend-group self-host.
        let private = if matchmaking_core {
            bool01(self.private_session) * 0.25
        } else {
            bool01(self.private_session)
        };
        let coop = if matchmaking_core {
            bool01(self.online_coop) * 0.2
        } else {
            bool01(self.online_coop)
        };
        let self_host = if matchmaking_core {
            bool01(self.self_hosted_server) * 0.15
        } else {
            bool01(self.self_hosted_server)
        };
        let matchmaking = if matchmaking_core { 0.95 } else { 0.12 };
        let public_world = if public_world_mode {
            0.85
        } else if matchmaking_core {
            0.75
        } else {
            0.12
        };
        let shutdown = if mode.contains("live") || matchmaking_core {
            0.35
        } else {
            0.1
        };

        RankingSignals {
            multiplayer: MultiplayerSignals {
                private_session: private,
                self_host_or_dedicated: self_host,
                online_coop: coop,
                group_size_fit: 0.5,
                low_public_population_dependency: 1.0 - public_world,
                drop_in_out: 0.4,
                cross_platform_fit: 0.4,
                matchmaking_core: matchmaking,
                public_world_dependency: public_world,
                group_size_mismatch: 0.0,
                service_shutdown_risk: shutdown,
                external_account_friction: 0.1,
                platform_or_anticheat_restriction: 0.1,
            },
            quality,
            popularity,
            momentum: popularity * 0.7,
            evidence: confidence,
            freshness: if self.release_state == "released" {
                0.5
            } else {
                0.8
            },
            data_confidence: confidence,
            demo_playability: if self.app_type == "demo" || self.has_demo {
                0.9
            } else {
                0.2
            },
            release_date_confidence: if self.release_date.is_some() {
                0.8
            } else {
                0.3
            },
            release_proximity: if self.release_state == "coming_soon"
                || self.release_state == "upcoming"
            {
                0.7
            } else {
                0.2
            },
            studio_prior: 0.5,
            longevity: if self.release_state == "released" {
                0.7
            } else {
                0.2
            },
            maintenance_health: 0.6,
            risk: shutdown * 0.5 + public_world * 0.2,
            personal_fit: 0.5,
        }
    }
}

fn bool01(value: Option<bool>) -> f64 {
    match value {
        Some(true) => 1.0,
        Some(false) => 0.0,
        None => 0.35,
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
    let mut stmt = conn.prepare(
        "WITH release_date_values AS (
             SELECT app_id, release_date AS value
             FROM apps WHERE release_date IS NOT NULL
             UNION ALL
             SELECT app_id, old_release_date
             FROM release_events WHERE old_release_date IS NOT NULL
             UNION ALL
             SELECT app_id, new_release_date
             FROM release_events WHERE new_release_date IS NOT NULL
         ), classification_dates AS (
             SELECT app_id, MIN(value) AS first_release_date
             FROM release_date_values
             WHERE value GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'
             GROUP BY app_id
         ), ranked_reviews AS (
             SELECT app_id, total_reviews, total_positive, wilson_lower,
                    ROW_NUMBER() OVER (
                        PARTITION BY app_id ORDER BY captured_at_ms DESC, language_scope ASC
                    ) AS row_num
             FROM review_snapshots
         ), latest_reviews AS (
             SELECT app_id, total_reviews, total_positive, wilson_lower
             FROM ranked_reviews WHERE row_num = 1
         ), ranked_players AS (
             SELECT app_id, player_count,
                    ROW_NUMBER() OVER (PARTITION BY app_id ORDER BY captured_at_ms DESC) AS row_num
             FROM player_snapshots WHERE player_count IS NOT NULL
         ), latest_players AS (
             SELECT app_id, player_count FROM ranked_players WHERE row_num = 1
         ), ranked_daily AS (
             SELECT app_id, median_approx_ccu,
                    ROW_NUMBER() OVER (PARTITION BY app_id ORDER BY day_utc DESC) AS row_num
             FROM player_daily WHERE median_approx_ccu IS NOT NULL
         ), daily_typical AS (
             SELECT app_id, CAST(AVG(median_approx_ccu) AS INTEGER) AS typical_ccu
             FROM ranked_daily WHERE row_num <= 7 GROUP BY app_id
         )
         SELECT a.app_id, a.canonical_name, a.app_type, a.release_state,
                COALESCE(cd.first_release_date, a.release_date),
                p.dominant_mode, p.private_session, p.online_coop, p.self_hosted_server,
                p.recommended_min_players, p.recommended_max_players, p.profile_confidence,
                r.total_reviews, r.total_positive, lp.player_count, r.wilson_lower,
                d.typical_ccu,
                COALESCE(v.platforms_json, '[]'), COALESCE(v.languages_json, '[]'),
                v.typical_session_minutes_min, v.typical_session_minutes_max, v.is_free,
                (
                    SELECT price.final_price_minor FROM price_snapshots price
                    WHERE price.app_id = a.app_id AND price.currency = :currency
                    ORDER BY price.captured_at_ms DESC LIMIT 1
                ),
                :currency,
                (a.app_type IN ('demo', 'playtest') OR EXISTS (
                    SELECT 1 FROM app_relations demo_relation
                    WHERE demo_relation.target_app_id = a.app_id
                      AND demo_relation.relation_type IN ('demo_of', 'playtest_of')
                )),
                 a.release_date_raw, a.release_date_precision,
                 media.capsule_url, media.updated_at_ms, NULL
         FROM apps a
         LEFT JOIN classification_dates cd ON cd.app_id = a.app_id
         LEFT JOIN multiplayer_profiles p ON p.app_id = a.app_id
         LEFT JOIN app_availability v ON v.app_id = a.app_id
         LEFT JOIN latest_reviews r ON r.app_id = a.app_id
         LEFT JOIN latest_players lp ON lp.app_id = a.app_id
         LEFT JOIN daily_typical d ON d.app_id = a.app_id
         LEFT JOIN app_media media ON media.app_id = a.app_id
         WHERE a.app_type IN ('game', 'demo', 'playtest', 'unknown')
           AND (
               (:section = 'upcoming' AND (
                   a.release_state IN ('upcoming', 'coming_soon')
                   OR (a.app_type IN ('demo', 'playtest') AND EXISTS (
                       SELECT 1
                       FROM app_relations relation
                       JOIN apps parent ON parent.app_id = relation.target_app_id
                       WHERE relation.source_app_id = a.app_id
                         AND relation.relation_type IN ('demo_of', 'playtest_of')
                         AND parent.release_state IN ('upcoming', 'coming_soon')
                   ))
               ))
               OR (:section = 'recent_release' AND a.release_state = 'released'
                   AND COALESCE(cd.first_release_date, a.release_date) >= :cutoff
                   AND COALESCE(cd.first_release_date, a.release_date) <= :today)
               OR (:section IN ('popular_legacy', 'classic_legacy') AND a.release_state = 'released'
                   AND COALESCE(cd.first_release_date, a.release_date) < :cutoff)
           )
           AND (
               :section <> 'popular_legacy'
               OR (
                   COALESCE(d.typical_ccu, lp.player_count, 0) >= :popular_min_ccu
                   AND COALESCE(r.wilson_lower, 0) >= CASE
                       WHEN COALESCE(d.typical_ccu, lp.player_count, 0) >= :popular_high_ccu
                           THEN :popular_high_ccu_min_wilson
                       ELSE :popular_min_wilson
                   END
               )
           )
         ORDER BY
             CASE WHEN :section = 'popular_legacy' THEN COALESCE(d.typical_ccu, lp.player_count, 0) END DESC,
             CASE WHEN :section = 'classic_legacy' THEN COALESCE(r.total_reviews, 0) END DESC,
             CASE WHEN :section IN ('recent_release', 'upcoming')
                  THEN COALESCE(cd.first_release_date, a.release_date) END DESC,
             a.updated_at_ms DESC
         LIMIT :limit",
    )?;
    let rows = stmt.query_map(
        named_params! {
            ":section": section.as_str(),
            ":cutoff": cutoff_date,
            ":today": today,
            ":currency": budget_currency,
            ":popular_min_ccu": config.popular_min_ccu,
            ":popular_high_ccu": config.popular_high_ccu,
            ":popular_min_wilson": config.popular_min_wilson,
            ":popular_high_ccu_min_wilson": config.popular_high_ccu_min_wilson,
            ":limit": limit,
        },
        map_candidate,
    )?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
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
    let friend_score = friend_fit(&signals.multiplayer);
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
            .is_some_and(|value| value >= popular_quality_floor)
        && friend_score >= config.popular_min_friend_fit;
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
            let has_multiplayer_evidence = row.dominant_mode.is_some()
                || row.private_session == Some(true)
                || row.online_coop == Some(true)
                || row.self_hosted_server == Some(true)
                || row.recommended_min.is_some()
                || row.recommended_max.is_some()
                || row.profile_confidence.is_some_and(|value| value >= 0.2);
            (row.release_state == "upcoming"
                || row.release_state == "coming_soon"
                || row.app_type == "demo"
                || row.app_type == "playtest")
                && has_multiplayer_evidence
        }
        FeedSection::RecentRelease => {
            row.release_state == "released"
                && date.is_some_and(|value| value >= cutoff_date && value <= today)
                && friend_score >= config.recent_min_friend_fit
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
                && friend_score >= config.classic_min_friend_fit
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
                 media.capsule_url, media.updated_at_ms, NULL
         FROM apps a
         LEFT JOIN multiplayer_profiles p ON p.app_id = a.app_id
         LEFT JOIN app_availability v ON v.app_id = a.app_id
         LEFT JOIN app_media media ON media.app_id = a.app_id
         WHERE a.canonical_name LIKE ?1 ESCAPE '\\' COLLATE NOCASE
            OR EXISTS (
                SELECT 1
                FROM app_localizations loc
                WHERE loc.app_id = a.app_id
                  AND lower(loc.language) IN ('schinese', 'english', 'en')
                  AND loc.name IS NOT NULL
                  AND trim(loc.name) != ''
                  AND loc.name LIKE ?1 ESCAPE '\\' COLLATE NOCASE
            )
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
                    SELECT CAST(d.median_approx_ccu AS INTEGER) FROM player_daily d
                    WHERE d.app_id = a.app_id AND d.median_approx_ccu IS NOT NULL
                    ORDER BY d.day_utc DESC LIMIT 1
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
                 media.capsule_url, media.updated_at_ms, loc.short_description
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

pub fn list_calendar(
    conn: &Connection,
    from_date: &str,
    to_date: &str,
    state: &str,
) -> StorageResult<(Vec<AppRecord>, Vec<AppRecord>)> {
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
    let mut stmt = conn.prepare(
        "SELECT app_id, app_type, canonical_name, release_state, release_date,
                release_date_raw, release_date_precision, is_early_access,
                current_data_confidence, source_modified_at_ms, created_at_ms, updated_at_ms
         FROM apps
         WHERE (
               (?3 = 'upcoming' AND release_state IN ('upcoming', 'coming_soon')
                   AND (release_date IS NULL OR (release_date >= ?1 AND release_date <= ?2)))
               OR
               (?3 = 'recent' AND release_state = 'released'
                   AND release_date >= ?1 AND release_date <= ?2)
         )
         ORDER BY release_date IS NULL, release_date, canonical_name",
    )?;
    let rows = stmt.query_map(params![from_date, to_date, state], |row| {
        Ok(AppRecord {
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
        })
    })?;
    for row in rows {
        let app = row?;
        match &app.release_date {
            Some(d) if d.as_str() >= from_date && d.as_str() <= to_date => {
                dated.push(app);
            }
            Some(_) | None => undated.push(app),
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
    if let Some(raw) = stored.map(str::trim).filter(|s| !s.is_empty())
        && !raw.eq_ignore_ascii_case("unknown")
    {
        let normalized = match raw {
            "competitive" | "versus" | "vs" => "pvp",
            other => other,
        };
        return Some(normalized.to_owned());
    }
    if online_coop == Some(true) {
        return Some("coop".to_owned());
    }
    None
}

fn map_candidate(row: &rusqlite::Row<'_>) -> rusqlite::Result<GameCandidateRow> {
    let platforms_json: String = row.get(17)?;
    let languages_json: String = row.get(18)?;
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
        platforms: parse_string_list(17, &platforms_json)?,
        languages: parse_string_list(18, &languages_json)?,
        typical_session_minutes_min: row.get::<_, Option<i64>>(19)?.map(|v| v as u32),
        typical_session_minutes_max: row.get::<_, Option<i64>>(20)?.map(|v| v as u32),
        is_free: sql_to_opt_bool(row.get(21)?),
        final_price_minor: row.get(22)?,
        price_currency: row.get(23)?,
        has_demo: row.get::<_, i64>(24)? != 0,
    })
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
             SELECT MAX(updated_at_ms) AS updated_at_ms FROM apps
             UNION ALL SELECT MAX(computed_at_ms) FROM multiplayer_profiles
             UNION ALL SELECT MAX(updated_at_ms) FROM app_availability
             UNION ALL SELECT MAX(captured_at_ms) FROM price_snapshots
             UNION ALL SELECT MAX(captured_at_ms) FROM review_snapshots
              UNION ALL SELECT MAX(captured_at_ms) FROM popular_reviews
              UNION ALL SELECT MAX(captured_at_ms) FROM popular_review_refresh_state
              UNION ALL SELECT MAX(captured_at_ms) FROM store_detail_refresh_state
             UNION ALL SELECT MAX(captured_at_ms) FROM player_snapshots
             UNION ALL SELECT MAX(observed_at_ms) FROM feature_evidence
             UNION ALL SELECT MAX(MAX(created_at_ms, COALESCE(revoked_at_ms, 0))) FROM curation_overrides
              UNION ALL SELECT MAX(observed_at_ms) FROM release_events
              UNION ALL SELECT MAX(updated_at_ms) FROM app_media
              UNION ALL SELECT MAX(updated_at_ms) FROM app_media_assets
              UNION ALL SELECT MAX(updated_at_ms) FROM app_localizations
          )",
        [],
        |row| row.get(0),
    )?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{GameCandidateRow, section_matches};
    use mpgs_domain::{
        FeedSection, MultiplayerSignals, RankingSignals, RecommendationConfig, friend_fit,
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
            recommended_min: Some(1),
            recommended_max: Some(4),
            profile_confidence: Some(0.9),
            total_reviews: Some(3_000),
            total_positive: Some(2_800),
            latest_ccu: None,
            wilson_lower: Some(0.82),
            typical_ccu_7d: None,
            platforms: vec!["windows".into()],
            languages: vec!["schinese".into()],
            typical_session_minutes_min: None,
            typical_session_minutes_max: None,
            is_free: Some(false),
            final_price_minor: None,
            price_currency: None,
            has_demo: false,
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
    fn recent_release_enforces_configured_friend_fit_threshold() {
        let row = candidate("2026-07-01");
        let signals = strong_friend_signals();
        let score = friend_fit(&signals.multiplayer);
        let mut config = RecommendationConfig {
            recent_min_friend_fit: score,
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

        config.recent_min_friend_fit = (score + 0.001).min(1.0);
        assert!(!section_matches(
            FeedSection::RecentRelease,
            &row,
            &signals,
            CUTOFF,
            TODAY,
            &config,
        ));
    }

    #[test]
    fn classic_legacy_enforces_review_wilson_and_friend_fit_thresholds() {
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
        assert!(!section_matches(
            FeedSection::ClassicLegacy,
            &row,
            &signals,
            CUTOFF,
            TODAY,
            &config,
        ));
        config.classic_min_friend_fit = RecommendationConfig::default().classic_min_friend_fit;

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
}
