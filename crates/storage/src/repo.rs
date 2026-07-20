//! High-level repository façade over the SQLite database.

use crate::backup::{self, restore_from_backup};
use crate::catalog;
use crate::curation;
use crate::db::Database;
use crate::error::StorageResult;
use crate::ingest;
use crate::jobs;
use crate::models::{
    AppRecord, CreateOverrideRequest, CurationOverride, EffectiveFeatureValue, EnqueueJob,
    EnrichmentNeedFilter, EnrichmentTarget, JobRecord, M3CatalogCoverage, M7DataCoverage,
    MultiplayerProfile,
};
use crate::quality::{self, QualityFinding};
use mpgs_steam_source::{
    AppCatalogProposal, AppListPage, AppListRequest, AppRelationProposal, CcuProposal, GoldenGame,
    PopularReviewsProposal, ReviewSummaryProposal, StoreDetailsProposal, StoreSearchPage,
};
use std::path::Path;

pub const REVIEW_REFRESH_INTERVAL_MS: i64 = 24 * 60 * 60 * 1_000;
pub const CCU_REFRESH_INTERVAL_MS: i64 = 6 * 60 * 60 * 1_000;
pub const PRICE_REFRESH_INTERVAL_MS: i64 = 24 * 60 * 60 * 1_000;

#[derive(Clone)]
pub struct Repository {
    pub(crate) db: Database,
}

impl Repository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    pub fn database(&self) -> &Database {
        &self.db
    }

    pub fn migrate(&self) -> StorageResult<i64> {
        self.db.migrate()
    }

    pub fn assert_ready(&self) -> StorageResult<()> {
        self.db.assert_ready()
    }

    pub fn readiness_check(&self) -> StorageResult<()> {
        self.db.readiness_check()?;
        self.db.with_conn(|conn| {
            let active_algorithms: i64 = conn.query_row(
                "SELECT COUNT(*) FROM algorithm_configs WHERE status = 'active'",
                [],
                |row| row.get(0),
            )?;
            let apps: i64 = conn.query_row("SELECT COUNT(*) FROM apps", [], |row| row.get(0))?;
            if active_algorithms != 1 {
                return Err(crate::StorageError::migration(
                    "exactly one active algorithm config is required",
                ));
            }
            if apps == 0 {
                return Err(crate::StorageError::migration(
                    "catalog has no app snapshot yet",
                ));
            }
            crate::users::active_algorithm_config(conn)?;
            Ok(())
        })
    }

    pub fn upsert_catalog(&self, proposal: &AppCatalogProposal) -> StorageResult<()> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            ingest::ingest_app_catalog(&tx, proposal, now)?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn ingest_app_list_page(
        &self,
        request: &AppListRequest,
        page: &AppListPage,
    ) -> StorageResult<usize> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            let ingested = ingest::ingest_app_list_page(&tx, request, page, now)?;
            tx.commit()?;
            Ok(ingested)
        })
    }

    pub fn ingest_review(&self, proposal: &ReviewSummaryProposal) -> StorageResult<()> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            ingest::ingest_review_summary(&tx, proposal, now)?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn ingest_popular_reviews(&self, proposal: &PopularReviewsProposal) -> StorageResult<()> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            ingest::ingest_popular_reviews(&tx, proposal, now)?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn ingest_ccu(&self, proposal: &CcuProposal) -> StorageResult<()> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            ingest::ingest_ccu(&tx, proposal, now)?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn ingest_store_details(
        &self,
        details: &StoreDetailsProposal,
        relations: &[AppRelationProposal],
    ) -> StorageResult<()> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            ingest::ingest_store_details(&tx, details, relations, now)?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn record_store_details_not_found(
        &self,
        app_id: u32,
        country_code: &str,
        language: &str,
    ) -> StorageResult<()> {
        let now = self.db.now_ms();
        let country_code = country_code.trim().to_ascii_uppercase();
        let language = language.trim().to_ascii_lowercase();
        self.db.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO store_detail_refresh_state(
                    app_id, country_code, language, captured_at_ms, status, source
                 ) VALUES (?1, ?2, ?3, ?4, 'not_found', 'steam_store_appdetails')
                 ON CONFLICT(app_id, country_code, language) DO UPDATE SET
                    captured_at_ms = excluded.captured_at_ms,
                    status = excluded.status,
                    source = excluded.source",
                rusqlite::params![app_id, country_code, language, now],
            )?;
            Ok(())
        })
    }

    pub fn ingest_store_search_page(&self, page: &StoreSearchPage) -> StorageResult<usize> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            let ingested = ingest::ingest_store_search_page(&tx, page, now)?;
            tx.commit()?;
            Ok(ingested)
        })
    }

    pub fn materialize_store_category_profiles(&self) -> StorageResult<usize> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            let applied = ingest::materialize_store_category_profiles(&tx, now)?;
            tx.commit()?;
            Ok(applied)
        })
    }

    pub fn restore_empty_availability_from_evidence(&self) -> StorageResult<usize> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| catalog::restore_empty_availability_from_evidence(conn, now))
    }

    pub fn ingest_multiplayer_bool(
        &self,
        app_id: u32,
        feature_name: &str,
        value: bool,
        source_type: &str,
        source_ref: &str,
        confidence: f64,
    ) -> StorageResult<bool> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            let applied = ingest::ingest_multiplayer_bool(
                &tx,
                app_id,
                feature_name,
                value,
                source_type,
                source_ref,
                confidence,
                now,
            )?;
            tx.commit()?;
            Ok(applied)
        })
    }

    pub fn get_app(&self, app_id: u32) -> StorageResult<Option<AppRecord>> {
        self.db.with_conn(|conn| catalog::get_app(conn, app_id))
    }

    pub fn get_profile(&self, app_id: u32) -> StorageResult<Option<MultiplayerProfile>> {
        self.db
            .with_conn(|conn| catalog::get_multiplayer_profile(conn, app_id))
    }

    pub fn count_apps(&self) -> StorageResult<i64> {
        self.db.with_conn(catalog::count_apps)
    }

    /// Multiplayer candidates due for enrichment in the default China storefront.
    pub fn list_enrichment_targets(&self, limit: u32) -> StorageResult<Vec<EnrichmentTarget>> {
        self.list_enrichment_targets_after(limit, None, "CN", "schinese")
    }

    /// Select due targets after a rotating app-id cursor, wrapping at the end.
    /// Dynamic snapshots become due again after their refresh interval.
    pub fn list_enrichment_targets_after(
        &self,
        limit: u32,
        after_app_id: Option<u32>,
        country_code: &str,
        language: &str,
    ) -> StorageResult<Vec<EnrichmentTarget>> {
        self.list_enrichment_targets_after_filtered(
            limit,
            after_app_id,
            country_code,
            language,
            EnrichmentNeedFilter::ALL,
        )
    }

    /// Like [`list_enrichment_targets_after`], but only dimensions enabled in
    /// `filter` participate in the WHERE clause and priority score. This keeps
    /// store-only / skip-* passes from returning empty after post-filtering.
    pub fn list_enrichment_targets_after_filtered(
        &self,
        limit: u32,
        after_app_id: Option<u32>,
        country_code: &str,
        language: &str,
        filter: EnrichmentNeedFilter,
    ) -> StorageResult<Vec<EnrichmentTarget>> {
        if !filter.any() {
            return Ok(Vec::new());
        }
        let limit = i64::from(limit.max(1));
        let now = self.db.now_ms();
        let review_cutoff = now.saturating_sub(REVIEW_REFRESH_INTERVAL_MS);
        let ccu_cutoff = now.saturating_sub(CCU_REFRESH_INTERVAL_MS);
        let price_cutoff = now.saturating_sub(PRICE_REFRESH_INTERVAL_MS);
        let after_app_id = i64::from(after_app_id.unwrap_or(0));
        let country_code = country_code.trim().to_ascii_uppercase();
        let language = language.trim().to_ascii_lowercase();
        let want_store = i64::from(filter.store);
        let want_reviews = i64::from(filter.reviews);
        let want_excerpts = i64::from(filter.review_excerpts);
        let want_ccu = i64::from(filter.ccu);
        let want_price = i64::from(filter.price);
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "WITH candidates AS (
                     SELECT a.app_id
                     FROM apps a
                     WHERE a.app_type IN ('game', 'demo', 'playtest')
                       AND (
                           EXISTS (
                               SELECT 1 FROM feature_evidence e
                               WHERE e.app_id = a.app_id
                                 AND e.feature_name = 'category_hint'
                                 AND e.is_active = 1
                                 AND e.confidence >= 0.3
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
                 ), due AS (
                     SELECT
                         candidates.app_id,
                         CASE WHEN (
                                    COALESCE(v.platforms_json, '[]') = '[]'
                                    OR COALESCE(v.languages_json, '[]') = '[]'
                                    OR NOT EXISTS (
                                        SELECT 1 FROM app_localizations localization
                                        WHERE localization.app_id = candidates.app_id
                                          AND localization.language = ?7
                                    )
                                  ) AND NOT EXISTS (
                                      SELECT 1 FROM store_detail_refresh_state refresh
                                      WHERE refresh.app_id = candidates.app_id
                                        AND refresh.country_code = ?5
                                        AND refresh.language = ?7
                                        AND refresh.status IN ('succeeded', 'not_found')
                                        AND refresh.captured_at_ms >= ?4
                                  )
                              THEN 1 ELSE 0 END AS needs_store_details,
                         CASE WHEN NOT EXISTS (
                             SELECT 1 FROM review_snapshots review
                             WHERE review.app_id = candidates.app_id
                               AND review.captured_at_ms >= ?2
                         ) THEN 1 ELSE 0 END AS needs_reviews,
                         CASE WHEN NOT EXISTS (
                             SELECT 1 FROM popular_review_refresh_state review
                             WHERE review.app_id = candidates.app_id
                               AND review.captured_at_ms >= ?2
                         ) THEN 1 ELSE 0 END AS needs_review_excerpts,
                         CASE WHEN NOT EXISTS (
                             SELECT 1 FROM player_snapshots player
                             WHERE player.app_id = candidates.app_id
                               AND player.captured_at_ms >= ?3
                         ) THEN 1 ELSE 0 END AS needs_ccu,
                         CASE WHEN NOT EXISTS (
                             SELECT 1 FROM price_snapshots price
                             WHERE price.app_id = candidates.app_id
                               AND price.country_code = ?5
                               AND price.final_price_minor IS NOT NULL
                               AND price.captured_at_ms >= ?4
                         ) AND NOT EXISTS (
                             SELECT 1 FROM store_detail_refresh_state refresh
                             WHERE refresh.app_id = candidates.app_id
                               AND refresh.country_code = ?5
                               AND refresh.language = ?7
                               AND refresh.status IN ('succeeded', 'not_found')
                               AND refresh.captured_at_ms >= ?4
                         ) THEN 1 ELSE 0 END AS needs_price
                     FROM candidates
                     LEFT JOIN app_availability v ON v.app_id = candidates.app_id
                 )
                 SELECT
                     app_id, needs_store_details, needs_reviews, needs_review_excerpts,
                     needs_ccu, needs_price
                 FROM due
                 WHERE (needs_store_details = 1 AND ?8 = 1)
                    OR (needs_reviews = 1 AND ?9 = 1)
                    OR (needs_review_excerpts = 1 AND ?10 = 1)
                    OR (needs_ccu = 1 AND ?11 = 1)
                    OR (needs_price = 1 AND ?12 = 1)
                 ORDER BY
                     (
                         CASE WHEN needs_store_details = 1 AND ?8 = 1 THEN 3 ELSE 0 END
                       + CASE WHEN needs_price = 1 AND ?12 = 1 THEN 3 ELSE 0 END
                       + CASE WHEN needs_reviews = 1 AND ?9 = 1 THEN 4 ELSE 0 END
                       + CASE WHEN needs_review_excerpts = 1 AND ?10 = 1 THEN 1 ELSE 0 END
                       + CASE WHEN needs_ccu = 1 AND ?11 = 1 THEN 2 ELSE 0 END
                     ) DESC,
                     CASE WHEN app_id > ?6 THEN 0 ELSE 1 END,
                     app_id ASC
                 LIMIT ?1",
            )?;
            let rows = stmt.query_map(
                rusqlite::params![
                    limit,
                    review_cutoff,
                    ccu_cutoff,
                    price_cutoff,
                    country_code,
                    after_app_id,
                    language,
                    want_store,
                    want_reviews,
                    want_excerpts,
                    want_ccu,
                    want_price,
                ],
                |row| {
                    Ok(EnrichmentTarget {
                        app_id: row.get::<_, i64>(0)? as u32,
                        needs_store_details: row.get::<_, i64>(1)? != 0,
                        needs_reviews: row.get::<_, i64>(2)? != 0,
                        needs_review_excerpts: row.get::<_, i64>(3)? != 0,
                        needs_ccu: row.get::<_, i64>(4)? != 0,
                        needs_price: row.get::<_, i64>(5)? != 0,
                    })
                },
            )?;
            let mut targets = Vec::new();
            for row in rows {
                targets.push(row?);
            }
            Ok(targets)
        })
    }

    pub fn import_golden_multiplayer_profile(&self, game: &GoldenGame) -> StorageResult<bool> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            let applied = ingest::import_golden_multiplayer_profile(&tx, game, now)?;
            tx.commit()?;
            Ok(applied)
        })
    }

    pub fn m3_catalog_coverage(&self) -> StorageResult<M3CatalogCoverage> {
        self.db.with_conn(|conn| {
            conn.query_row(
                "WITH candidates AS (
                     SELECT a.app_id
                     FROM apps a
                     WHERE a.app_type IN ('game', 'demo', 'playtest')
                       AND (
                           EXISTS (
                               SELECT 1 FROM feature_evidence e
                               WHERE e.app_id = a.app_id
                                 AND e.feature_name = 'category_hint'
                                 AND e.is_active = 1
                                 AND e.confidence >= 0.3
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
                 )
                 SELECT
                     COUNT(*),
                     COALESCE(SUM(CASE WHEN EXISTS (
                         SELECT 1 FROM feature_evidence evidence
                         WHERE evidence.app_id = candidates.app_id
                           AND evidence.feature_name = 'category_hint'
                           AND evidence.is_active = 1
                           AND evidence.confidence >= 0.3
                     ) THEN 1 ELSE 0 END), 0),
                     COALESCE(SUM(CASE WHEN
                         profile.dominant_mode IS NOT NULL
                         OR profile.private_session IS NOT NULL
                         OR profile.online_coop IS NOT NULL
                         OR profile.self_hosted_server IS NOT NULL
                         OR profile.drop_in_out IS NOT NULL
                         OR profile.crossplay IS NOT NULL
                         OR profile.recommended_max_players IS NOT NULL
                     THEN 1 ELSE 0 END), 0),
                     COALESCE(SUM(CASE WHEN profile.profile_confidence >= 0.8 AND EXISTS (
                         SELECT 1 FROM feature_evidence trusted
                         WHERE trusted.app_id = candidates.app_id
                           AND trusted.source_type = 'human_golden'
                           AND trusted.confidence >= 0.8
                           AND trusted.is_active = 1
                     ) AND (
                         profile.dominant_mode IS NOT NULL
                         OR profile.private_session = 1
                         OR profile.online_coop = 1
                         OR profile.self_hosted_server = 1
                     ) THEN 1 ELSE 0 END), 0),
                     COALESCE(SUM(CASE WHEN COALESCE(v.platforms_json, '[]') <> '[]' THEN 1 ELSE 0 END), 0),
                     COALESCE(SUM(CASE WHEN COALESCE(v.languages_json, '[]') <> '[]' THEN 1 ELSE 0 END), 0),
                     COALESCE(SUM(CASE WHEN v.typical_session_minutes_min IS NOT NULL
                                       AND v.typical_session_minutes_max IS NOT NULL
                                  THEN 1 ELSE 0 END), 0),
                     COALESCE(SUM(CASE WHEN EXISTS (
                         SELECT 1 FROM price_snapshots price
                         WHERE price.app_id = candidates.app_id
                           AND price.final_price_minor IS NOT NULL
                     ) THEN 1 ELSE 0 END), 0),
                     COALESCE(SUM(CASE WHEN EXISTS (
                         SELECT 1 FROM review_snapshots review
                         WHERE review.app_id = candidates.app_id
                     ) THEN 1 ELSE 0 END), 0),
                     COALESCE(SUM(CASE WHEN EXISTS (
                         SELECT 1 FROM player_snapshots player
                         WHERE player.app_id = candidates.app_id
                           AND player.player_count IS NOT NULL
                           AND player.result_code = 1
                     ) THEN 1 ELSE 0 END), 0)
                 FROM candidates
                 LEFT JOIN multiplayer_profiles profile ON profile.app_id = candidates.app_id
                 LEFT JOIN app_availability v ON v.app_id = candidates.app_id",
                [],
                |row| {
                    Ok(M3CatalogCoverage {
                        normalized_multiplayer_candidates: row.get(0)?,
                        category_evidence_candidates: row.get(1)?,
                        recommendation_ready_profiles: row.get(2)?,
                        trusted_familiar_profiles: row.get(3)?,
                        with_platforms: row.get(4)?,
                        with_languages: row.get(5)?,
                        with_typical_session: row.get(6)?,
                        with_price: row.get(7)?,
                        with_reviews: row.get(8)?,
                        with_ccu: row.get(9)?,
                    })
                },
            )
            .map_err(Into::into)
        })
    }

    /// Coverage used by the M7 real-data release gate.
    ///
    /// The aggregate portion is read directly from authoritative snapshots.
    /// Section counts then use the same post-query eligibility predicate as
    /// the public feed, before individual user preference filters apply.
    pub fn m7_data_coverage(
        &self,
        config: &mpgs_domain::RecommendationConfig,
    ) -> StorageResult<M7DataCoverage> {
        let now_ms = self.db.now_ms();
        let today = crate::util::day_utc_from_ms(now_ms);
        let cutoff = crate::util::day_utc_from_ms(
            now_ms.saturating_sub(i64::from(config.recent_days) * 24 * 60 * 60 * 1_000),
        );
        let mut coverage = self.db.with_conn(|conn| {
            conn.query_row(
                "WITH candidates AS (
                     SELECT a.app_id
                     FROM apps a
                     WHERE a.app_type IN ('game', 'demo', 'playtest')
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
                 ), focus AS (
                     SELECT candidates.app_id
                     FROM candidates
                     JOIN multiplayer_profiles profile ON profile.app_id = candidates.app_id
                     WHERE COALESCE(profile.profile_confidence, 0.0) >= 0.70
                       AND (
                           profile.private_session = 1
                           OR profile.online_coop = 1
                           OR profile.self_hosted_server = 1
                       )
                 ), review_days AS (
                     SELECT DISTINCT review.app_id,
                            date(review.captured_at_ms / 1000, 'unixepoch') AS day_utc
                     FROM review_snapshots review
                     JOIN focus ON focus.app_id = review.app_id
                 ), review_numbered AS (
                     SELECT app_id, day_utc,
                            julianday(day_utc) - ROW_NUMBER() OVER (
                                PARTITION BY app_id ORDER BY day_utc
                            ) AS streak_key
                     FROM review_days
                 ), review_streaks AS (
                     SELECT app_id, COUNT(*) AS day_count
                     FROM review_numbered
                     GROUP BY app_id, streak_key
                 ), ccu_days AS (
                     SELECT DISTINCT daily.app_id, daily.day_utc
                     FROM player_daily daily
                     JOIN focus ON focus.app_id = daily.app_id
                     WHERE daily.sample_count > 0
                       AND daily.median_approx_ccu IS NOT NULL
                 ), ccu_numbered AS (
                     SELECT app_id, day_utc,
                            julianday(day_utc) - ROW_NUMBER() OVER (
                                PARTITION BY app_id ORDER BY day_utc
                            ) AS streak_key
                     FROM ccu_days
                 ), ccu_streaks AS (
                     SELECT app_id, COUNT(*) AS day_count
                     FROM ccu_numbered
                     GROUP BY app_id, streak_key
                 )
                 SELECT
                     COUNT(*),
                     (SELECT COUNT(*) FROM focus),
                     COALESCE(SUM(CASE WHEN
                         NULLIF(TRIM(COALESCE(app.release_date, '')), '') IS NOT NULL
                         OR NULLIF(TRIM(COALESCE(app.release_date_raw, '')), '') IS NOT NULL
                     THEN 1 ELSE 0 END), 0),
                     COALESCE(SUM(CASE WHEN
                         NULLIF(TRIM(COALESCE(media.capsule_url, '')), '') IS NOT NULL
                     THEN 1 ELSE 0 END), 0),
                     (SELECT COUNT(*) FROM focus WHERE EXISTS (
                         SELECT 1 FROM review_streaks
                         WHERE review_streaks.app_id = focus.app_id
                           AND review_streaks.day_count >= 7
                     )),
                     (SELECT COUNT(*) FROM focus WHERE EXISTS (
                         SELECT 1 FROM ccu_streaks
                         WHERE ccu_streaks.app_id = focus.app_id
                           AND ccu_streaks.day_count >= 7
                     ))
                 FROM candidates
                 JOIN apps app ON app.app_id = candidates.app_id
                 LEFT JOIN app_media media ON media.app_id = candidates.app_id",
                [],
                |row| {
                    Ok(M7DataCoverage {
                        normalized_multiplayer_candidates: row.get(0)?,
                        trusted_friend_multiplayer_profiles: row.get(1)?,
                        candidates_with_date: row.get(2)?,
                        candidates_with_cover: row.get(3)?,
                        upcoming_candidates: 0,
                        recent_release_candidates: 0,
                        popular_legacy_candidates: 0,
                        classic_legacy_candidates: 0,
                        trusted_profiles_with_seven_day_reviews: row.get(4)?,
                        trusted_profiles_with_seven_day_ccu: row.get(5)?,
                    })
                },
            )
            .map_err(Into::into)
        })?;

        let limit = i64::from(config.candidate_limit);
        for section in mpgs_domain::FeedSection::ALL {
            let count = self
                .list_candidates(section, &cutoff, &today, "CNY", config, limit)?
                .into_iter()
                .filter(|row| {
                    let signals = row.to_ranking_signals();
                    crate::query::section_matches(section, row, &signals, &cutoff, &today, config)
                })
                .count() as i64;
            match section {
                mpgs_domain::FeedSection::Upcoming => coverage.upcoming_candidates = count,
                mpgs_domain::FeedSection::RecentRelease => {
                    coverage.recent_release_candidates = count
                }
                mpgs_domain::FeedSection::PopularLegacy => {
                    coverage.popular_legacy_candidates = count
                }
                mpgs_domain::FeedSection::ClassicLegacy => {
                    coverage.classic_legacy_candidates = count
                }
            }
        }
        Ok(coverage)
    }

    pub fn source_cursor(&self, cursor_key: &str) -> StorageResult<Option<String>> {
        self.db
            .with_conn(|conn| crate::source_state::load_cursor(conn, cursor_key))
    }

    pub fn save_source_cursor(
        &self,
        cursor_key: &str,
        source: &str,
        cursor: &serde_json::Value,
    ) -> StorageResult<()> {
        let now = self.db.now_ms();
        let cursor_json = serde_json::to_string(cursor)?;
        self.db.with_conn_mut(|conn| {
            crate::source_state::save_cursor(conn, cursor_key, source, &cursor_json, now)
        })
    }

    pub fn start_source_run(
        &self,
        source: &str,
        task_type: &str,
        parser_version: &str,
        notes: Option<&str>,
    ) -> StorageResult<i64> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            crate::source_state::start_run(conn, source, task_type, parser_version, notes, now)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finish_source_run(
        &self,
        run_id: i64,
        status: &str,
        request_count: i64,
        success_count: i64,
        error_category: Option<&str>,
        notes: Option<&str>,
    ) -> StorageResult<()> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            crate::source_state::finish_run(
                conn,
                run_id,
                status,
                request_count,
                success_count,
                error_category,
                notes,
                now,
            )
        })
    }

    pub fn create_override(
        &self,
        app_id: u32,
        request: &CreateOverrideRequest,
    ) -> StorageResult<CurationOverride> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| curation::create_override(conn, app_id, request, now))
    }

    pub fn revoke_override(
        &self,
        override_id: i64,
        operator: &str,
        reason: &str,
        request_id: Option<&str>,
    ) -> StorageResult<CurationOverride> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            curation::revoke_override(conn, override_id, operator, reason, request_id, now)
        })
    }

    pub fn resolve_feature(
        &self,
        app_id: u32,
        feature_name: &str,
    ) -> StorageResult<EffectiveFeatureValue> {
        self.db
            .with_conn(|conn| curation::resolve_effective_feature(conn, app_id, feature_name))
    }

    pub fn enqueue_job(&self, job: &EnqueueJob) -> StorageResult<i64> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| jobs::enqueue_job(conn, job, now))
    }

    pub fn has_active_job(
        &self,
        source: &str,
        task_type: &str,
        entity_key: &str,
    ) -> StorageResult<bool> {
        self.db
            .with_conn(|conn| jobs::has_active_job(conn, source, task_type, entity_key))
    }

    pub fn lease_jobs(
        &self,
        owner: &str,
        limit: i64,
        lease_ms: i64,
        source_filter: Option<&str>,
    ) -> StorageResult<Vec<JobRecord>> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            jobs::lease_jobs(conn, owner, limit, lease_ms, now, source_filter)
        })
    }

    pub fn complete_job(
        &self,
        job_id: i64,
        owner: &str,
        idempotency_key: &str,
    ) -> StorageResult<JobRecord> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| jobs::complete_job(conn, job_id, owner, idempotency_key, now))
    }

    pub fn fail_job(
        &self,
        job_id: i64,
        owner: &str,
        error_category: &str,
        retry_delay_ms: i64,
    ) -> StorageResult<JobRecord> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            jobs::fail_job(conn, job_id, owner, error_category, retry_delay_ms, now)
        })
    }

    pub fn run_quality_checks(&self) -> StorageResult<Vec<QualityFinding>> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| quality::run_quality_checks(conn, now))
    }

    pub fn backup_to(&self, dest: impl AsRef<Path>) -> StorageResult<()> {
        backup::backup_to_path(&self.db, dest)
    }

    pub fn restore_backup(
        backup_path: impl AsRef<Path>,
        dest_path: impl AsRef<Path>,
        now_ms: i64,
    ) -> StorageResult<Self> {
        let db = restore_from_backup(backup_path, dest_path, now_ms)?;
        Ok(Self::new(db))
    }

    pub fn seed_demo_if_empty(&self) -> StorageResult<usize> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            let seeded = crate::seed::seed_demo_catalog_if_empty(&tx, now)?;
            tx.commit()?;
            Ok(seeded)
        })
    }

    pub fn ensure_runtime_defaults(&self) -> StorageResult<()> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            crate::users::ensure_algorithm_config(conn, now)?;
            crate::source_state::ensure_data_refresh_tasks(conn, now)?;
            Ok(())
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_data_refresh_status(
        &self,
        task_name: &str,
        last_success_at_ms: Option<i64>,
        next_run_at_ms: Option<i64>,
        last_error_category: Option<&str>,
        cursor_value: Option<&str>,
        coverage_ratio: Option<f64>,
    ) -> StorageResult<()> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            crate::source_state::update_data_refresh_status(
                conn,
                task_name,
                last_success_at_ms,
                next_run_at_ms,
                last_error_category,
                cursor_value,
                coverage_ratio,
                now,
            )
        })
    }

    pub fn data_refresh_status(&self) -> StorageResult<Vec<crate::models::DataRefreshStatus>> {
        self.db.with_conn(crate::source_state::data_refresh_status)
    }

    pub fn create_anonymous_session(&self) -> StorageResult<crate::users::SessionTokens> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| crate::users::create_anonymous_session(conn, now))
    }

    pub fn refresh_anonymous_session(
        &self,
        refresh_token: &str,
    ) -> StorageResult<crate::users::SessionTokens> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| crate::users::refresh_anonymous_session(conn, refresh_token, now))
    }

    pub fn resolve_access_token(&self, token: &str) -> StorageResult<String> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| crate::users::resolve_user_id(conn, token, now))
    }

    // --- M7 account identity ---

    pub fn register_account(
        &self,
        input: &crate::accounts::RegisterAccount,
        anonymous_user_id: Option<&str>,
    ) -> StorageResult<crate::accounts::AccountSessionTokens> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            crate::accounts::register_account(conn, input, anonymous_user_id, now)
        })
    }

    pub fn login_account(
        &self,
        input: &crate::accounts::LoginAccount,
        anonymous_user_id: Option<&str>,
    ) -> StorageResult<crate::accounts::AccountSessionTokens> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            crate::accounts::login_account(conn, input, anonymous_user_id, now)
        })
    }

    pub fn refresh_account_session(
        &self,
        refresh_token: &str,
    ) -> StorageResult<crate::accounts::AccountSessionTokens> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            crate::accounts::refresh_account_session(conn, refresh_token, now)
        })
    }

    pub fn resolve_account_access_token(&self, token: &str) -> StorageResult<String> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| crate::accounts::resolve_account_user_id(conn, token, now))
    }

    pub fn resolve_anonymous_access_token(&self, token: &str) -> StorageResult<String> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| crate::accounts::resolve_anonymous_user_id(conn, token, now))
    }

    pub fn account_profile(&self, user_id: &str) -> StorageResult<crate::accounts::AccountProfile> {
        self.db
            .with_conn(|conn| crate::accounts::account_profile(conn, user_id))
    }

    pub fn update_account_display_name(
        &self,
        user_id: &str,
        display_name: &str,
    ) -> StorageResult<crate::accounts::AccountProfile> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            crate::accounts::update_display_name(conn, user_id, display_name, now)
        })
    }

    pub fn change_account_password(
        &self,
        user_id: &str,
        current_access_token: &str,
        old_password: &str,
        new_password: &str,
    ) -> StorageResult<()> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            crate::accounts::change_password(
                conn,
                user_id,
                current_access_token,
                old_password,
                new_password,
                now,
            )
        })
    }

    pub fn revoke_current_account_session(&self, access_token: &str) -> StorageResult<()> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| crate::accounts::revoke_current_session(conn, access_token, now))
    }

    pub fn revoke_all_account_sessions(&self, user_id: &str) -> StorageResult<()> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| crate::accounts::revoke_all_sessions(conn, user_id, now))
    }

    pub fn delete_account(
        &self,
        user_id: &str,
    ) -> StorageResult<Option<crate::accounts::AvatarMetadata>> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| crate::accounts::delete_account(conn, user_id, now))
    }

    pub fn set_account_avatar_metadata(
        &self,
        user_id: &str,
        content_hash: &str,
        storage_key: &str,
    ) -> StorageResult<crate::accounts::AvatarMetadata> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            crate::accounts::set_avatar_metadata(conn, user_id, content_hash, storage_key, now)
        })
    }

    pub fn delete_account_avatar_metadata(
        &self,
        user_id: &str,
    ) -> StorageResult<Option<crate::accounts::AvatarMetadata>> {
        self.db
            .with_conn_mut(|conn| crate::accounts::delete_avatar_metadata(conn, user_id))
    }

    pub fn set_account_avatar_moderation(
        &self,
        user_id: &str,
        actor: &str,
        reason: &str,
        blocked: bool,
    ) -> StorageResult<()> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            crate::accounts::set_avatar_moderation(conn, user_id, actor, reason, blocked, now)
        })
    }

    pub fn account_avatar_by_public_id(
        &self,
        public_id: &str,
    ) -> StorageResult<crate::accounts::AvatarLookup> {
        self.db
            .with_conn(|conn| crate::accounts::avatar_by_public_id(conn, public_id))
    }

    pub fn account_ai_settings(&self, user_id: &str) -> StorageResult<crate::accounts::AiSettings> {
        self.db
            .with_conn(|conn| crate::accounts::account_ai_settings(conn, user_id))
    }

    pub fn put_account_ai_settings(
        &self,
        user_id: &str,
        input: &crate::accounts::PutAiSettings,
        cipher: Option<&crate::accounts::CredentialCipher>,
    ) -> StorageResult<crate::accounts::AiSettings> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            crate::accounts::put_account_ai_settings(conn, user_id, input, cipher, now)
        })
    }

    pub fn delete_custom_ai_key(
        &self,
        user_id: &str,
    ) -> StorageResult<crate::accounts::AiSettings> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| crate::accounts::delete_custom_ai_key(conn, user_id, now))
    }

    pub fn account_ai_daily_usage(&self, user_id: &str, day_utc: i64) -> StorageResult<u32> {
        self.db
            .with_conn(|conn| crate::accounts::account_ai_daily_usage(conn, user_id, day_utc))
    }

    pub fn consume_account_ai_quota(
        &self,
        user_id: &str,
        day_utc: i64,
        daily_limit: u32,
    ) -> StorageResult<Option<u32>> {
        self.db.with_conn_mut(|conn| {
            crate::accounts::consume_account_ai_quota(conn, user_id, day_utc, daily_limit)
        })
    }

    pub fn custom_ai_credential(
        &self,
        user_id: &str,
        cipher: Option<&crate::accounts::CredentialCipher>,
    ) -> StorageResult<Option<crate::accounts::CustomAiCredential>> {
        self.db
            .with_conn(|conn| crate::accounts::custom_ai_credential(conn, user_id, cipher))
    }

    pub fn get_preferences(&self, user_id: &str) -> StorageResult<mpgs_domain::UserPreferences> {
        self.db
            .with_conn(|conn| crate::users::get_preferences(conn, user_id))
    }

    pub fn put_preferences(
        &self,
        user_id: &str,
        prefs: &mpgs_domain::UserPreferences,
    ) -> StorageResult<mpgs_domain::UserPreferences> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| crate::users::put_preferences(conn, user_id, prefs, now))
    }

    pub fn active_algorithm_config(&self) -> StorageResult<crate::users::ActiveAlgorithmConfig> {
        self.db.with_conn(crate::users::active_algorithm_config)
    }

    pub fn list_candidates(
        &self,
        section: mpgs_domain::FeedSection,
        cutoff_date: &str,
        today: &str,
        budget_currency: &str,
        config: &mpgs_domain::RecommendationConfig,
        limit: i64,
    ) -> StorageResult<Vec<crate::query::GameCandidateRow>> {
        self.db.with_conn(|conn| {
            crate::query::list_candidates(
                conn,
                section,
                cutoff_date,
                today,
                budget_currency,
                config,
                limit,
            )
        })
    }

    pub fn search_games(
        &self,
        q: &str,
        limit: i64,
    ) -> StorageResult<Vec<crate::query::GameCandidateRow>> {
        self.db
            .with_conn(|conn| crate::query::search_by_name(conn, q, limit))
    }

    pub fn game_detail(
        &self,
        app_id: u32,
    ) -> StorageResult<Option<crate::query::GameCandidateRow>> {
        self.db
            .with_conn(|conn| crate::query::get_game_detail(conn, app_id))
    }

    pub fn popular_reviews(
        &self,
        app_id: u32,
    ) -> StorageResult<Vec<crate::query::PopularReviewRow>> {
        self.db
            .with_conn(|conn| crate::query::list_popular_reviews(conn, app_id))
    }

    pub fn list_evidence(
        &self,
        app_id: u32,
        feature: Option<&str>,
    ) -> StorageResult<Vec<crate::query::EvidenceRow>> {
        self.db
            .with_conn(|conn| crate::query::list_evidence(conn, app_id, feature))
    }

    pub fn list_calendar(
        &self,
        from: &str,
        to: &str,
        state: &str,
    ) -> StorageResult<(Vec<AppRecord>, Vec<AppRecord>)> {
        self.db
            .with_conn(|conn| crate::query::list_calendar(conn, from, to, state))
    }

    pub fn data_updated_at_ms(&self) -> StorageResult<i64> {
        self.db.with_conn(crate::query::data_updated_at_ms)
    }

    pub fn create_feedback(
        &self,
        user_id: &str,
        app_id: u32,
        feedback_type: mpgs_domain::FeedbackType,
        recommendation_run_id: Option<&str>,
        idempotency_key: &str,
        client_created_at_ms: Option<i64>,
    ) -> StorageResult<crate::feedback::FeedbackRecord> {
        let now = self.db.now_ms();
        let fingerprint = crate::feedback::request_fingerprint(
            app_id,
            feedback_type,
            recommendation_run_id,
            client_created_at_ms,
        )?;
        self.db.with_conn_mut(|conn| {
            crate::feedback::create_feedback(
                conn,
                user_id,
                app_id,
                feedback_type,
                recommendation_run_id,
                idempotency_key,
                client_created_at_ms,
                &fingerprint,
                now,
            )
        })
    }

    pub fn undo_feedback(
        &self,
        user_id: &str,
        feedback_id: i64,
    ) -> StorageResult<crate::feedback::FeedbackRecord> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| crate::feedback::undo_feedback(conn, user_id, feedback_id, now))
    }

    pub fn list_active_feedback(
        &self,
        user_id: &str,
    ) -> StorageResult<Vec<crate::feedback::ActiveFeedback>> {
        self.db
            .with_conn(|conn| crate::feedback::list_active_feedback(conn, user_id))
    }

    // --- play-intent votes ---

    pub fn set_play_intent(
        &self,
        user_id: &str,
        app_id: u32,
        intent: bool,
    ) -> StorageResult<crate::play_intent::PlayIntentState> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            crate::play_intent::set_play_intent(conn, user_id, app_id, intent, now)
        })
    }

    pub fn play_intent_counts(&self) -> StorageResult<std::collections::HashMap<u32, u32>> {
        self.db.with_conn(crate::play_intent::all_counts)
    }

    pub fn play_intent_count(&self, app_id: u32) -> StorageResult<u32> {
        self.db
            .with_conn(|conn| crate::play_intent::count_for(conn, app_id))
    }

    pub fn user_play_intents(
        &self,
        user_id: &str,
    ) -> StorageResult<std::collections::HashSet<u32>> {
        self.db
            .with_conn(|conn| crate::play_intent::user_votes(conn, user_id))
    }

    pub fn has_play_intent(&self, user_id: &str, app_id: u32) -> StorageResult<bool> {
        self.db
            .with_conn(|conn| crate::play_intent::has_voted(conn, user_id, app_id))
    }

    pub fn play_intent_epoch(&self) -> StorageResult<crate::play_intent::PlayIntentEpoch> {
        self.db.with_conn(crate::play_intent::epoch)
    }

    pub fn play_intent_feed_snapshot(
        &self,
        user_id: Option<&str>,
    ) -> StorageResult<crate::play_intent::PlayIntentFeedSnapshot> {
        self.db
            .with_conn(|conn| crate::play_intent::feed_snapshot(conn, user_id))
    }

    pub fn play_intent_game_snapshot(
        &self,
        user_id: Option<&str>,
        app_id: u32,
    ) -> StorageResult<crate::play_intent::PlayIntentGameSnapshot> {
        self.db
            .with_conn(|conn| crate::play_intent::game_snapshot(conn, user_id, app_id))
    }

    pub fn community_play_intents(
        &self,
        user_id: Option<&str>,
        sort: crate::community::CommunitySort,
        filters: &crate::community::CommunityFilters,
        limit: usize,
        offset: usize,
    ) -> StorageResult<crate::community::CommunityPlayIntentPage> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            crate::community::page(conn, user_id, sort, filters, limit, offset, now)
        })
    }

    pub fn community_game_previews(
        &self,
        app_id: u32,
        user_id: Option<&str>,
    ) -> StorageResult<(
        crate::play_intent::PlayIntentEpoch,
        u32,
        bool,
        Vec<crate::community::CommunityVoterPreview>,
        u32,
    )> {
        self.db
            .with_conn_mut(|conn| crate::community::previews_for_game(conn, app_id, user_id))
    }

    // --- M8 progressive AI + dual-channel storage ---

    pub fn insert_progressive_analysis(
        &self,
        row: &crate::ai_m8::InsertProgressiveAnalysis,
    ) -> StorageResult<()> {
        self.db
            .with_conn_mut(|conn| crate::ai_m8::insert_progressive_analysis(conn, row))
    }

    pub fn get_progressive_analysis(
        &self,
        analysis_id: &str,
        now_ms: i64,
    ) -> StorageResult<Option<crate::ai_m8::ProgressiveAnalysis>> {
        self.db
            .with_conn(|conn| crate::ai_m8::get_progressive_analysis(conn, analysis_id, now_ms))
    }

    pub fn complete_progressive_analysis(
        &self,
        update: &crate::ai_m8::CompleteProgressiveAnalysis,
    ) -> StorageResult<()> {
        self.db
            .with_conn_mut(|conn| crate::ai_m8::complete_progressive_analysis(conn, update))
    }

    pub fn insert_web_discovery_evidence(
        &self,
        row: &crate::ai_m8::InsertWebDiscoveryEvidence,
    ) -> StorageResult<bool> {
        self.db
            .with_conn_mut(|conn| crate::ai_m8::insert_web_discovery_evidence(conn, row))
    }

    pub fn list_web_discovery_for_app(
        &self,
        app_id: u32,
        limit: i64,
    ) -> StorageResult<Vec<crate::ai_m8::WebDiscoveryEvidence>> {
        self.db
            .with_conn(|conn| crate::ai_m8::list_web_discovery_for_app(conn, app_id, limit))
    }

    pub fn insert_field_proposal(
        &self,
        row: &crate::ai_m8::InsertFieldProposal,
    ) -> StorageResult<()> {
        self.db
            .with_conn_mut(|conn| crate::ai_m8::insert_field_proposal(conn, row))
    }

    pub fn put_bootstrap_state(&self, key: &str, value_json: &str) -> StorageResult<()> {
        let now = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| crate::ai_m8::put_bootstrap_state(conn, key, value_json, now))
    }

    pub fn get_bootstrap_state(&self, key: &str) -> StorageResult<Option<String>> {
        self.db
            .with_conn(|conn| crate::ai_m8::get_bootstrap_state(conn, key))
    }
}
