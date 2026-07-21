use std::collections::BTreeMap;

use rusqlite::params;
use rusqlite::{Connection, OptionalExtension};

use mpgs_steam_source::{
    APP_LIST_ADAPTER_VERSION, APP_LIST_SOURCE_NAME, AppCatalogProposal, AppListPage,
    AppListRequest, AppRelationProposal, AppTypeProposal, CcuProposal, DominantModeLabel,
    GOLDEN_SET_VERSION, GoldenGame, PopularReviewsProposal, RelationTypeProposal,
    ReleaseStateProposal, ReviewSummaryProposal, STORE_SEARCH_ADAPTER_VERSION,
    STORE_SEARCH_SOURCE_NAME, StoreDetailsProposal, StoreSearchPage, content_hash,
};

use crate::catalog::{self, upsert_app, upsert_relation};
use crate::curation::{
    has_active_override, insert_feature_evidence, insert_feature_evidence_with_document,
};
use crate::error::StorageResult;
use crate::util::{day_utc_from_ms, wilson_lower_bound};

pub fn ingest_app_catalog(
    conn: &Connection,
    proposal: &AppCatalogProposal,
    now_ms: i64,
) -> StorageResult<()> {
    let app_type = app_type_str(proposal.app_type);
    let source_modified = proposal.last_modified.map(|s| i64::from(s) * 1000);
    upsert_app(
        conn,
        proposal.app_id,
        app_type,
        &proposal.name,
        "unknown",
        None,
        None,
        source_modified,
        now_ms,
    )?;
    // Steam's canonical per-app header image is served from an approved CDN and
    // is stable enough to use as the card capsule fallback when a richer media
    // field is absent from the store response.
    let capsule_url = format!(
        "https://cdn.akamai.steamstatic.com/steam/apps/{}/header.jpg",
        proposal.app_id
    );
    conn.execute(
        "INSERT INTO app_media (app_id, capsule_url, source, updated_at_ms)
         VALUES (?1, ?2, 'steam_catalog', ?3)
         ON CONFLICT(app_id) DO UPDATE SET
             capsule_url = excluded.capsule_url,
             source = excluded.source,
             updated_at_ms = excluded.updated_at_ms",
        params![proposal.app_id, capsule_url, now_ms],
    )?;
    Ok(())
}

/// Persist one official `IStoreService/GetAppList` page and its audit record
/// in the same transaction. The request never contains the API key, so the
/// stored entity key is safe to expose through operations tooling.
pub fn ingest_app_list_page(
    conn: &Connection,
    request: &AppListRequest,
    page: &AppListPage,
    now_ms: i64,
) -> StorageResult<usize> {
    conn.execute(
        "INSERT INTO source_documents (
            source, entity_type, entity_key, content_type, content_hash,
            content_text, fetched_at_ms, parse_version
         ) VALUES (?1, 'app_list_page', ?2, 'application/json', ?3, NULL, ?4, ?5)",
        params![
            APP_LIST_SOURCE_NAME,
            format!(
                "last_appid={};if_modified_since={}",
                request.last_appid, request.if_modified_since
            ),
            page.content_hash,
            now_ms,
            APP_LIST_ADAPTER_VERSION,
        ],
    )?;

    for proposal in &page.proposals {
        ingest_app_catalog(conn, proposal, now_ms)?;
    }
    Ok(page.proposals.len())
}

pub fn ingest_review_summary(
    conn: &Connection,
    proposal: &ReviewSummaryProposal,
    now_ms: i64,
) -> StorageResult<()> {
    catalog::ensure_app_stub(
        conn,
        proposal.app_id,
        &format!("app-{}", proposal.app_id),
        now_ms,
    )?;
    let wilson = wilson_lower_bound(proposal.total_positive, proposal.total_reviews);
    conn.execute(
        "INSERT OR REPLACE INTO review_snapshots (
            app_id, region_scope, language_scope, captured_at_ms,
            total_positive, total_negative, total_reviews, review_score, review_score_desc,
            wilson_lower, filter_offtopic_activity, parameter_hash, content_hash, source
         ) VALUES (?1, 'all', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            proposal.app_id,
            proposal.language_scope,
            now_ms,
            proposal.total_positive,
            proposal.total_negative,
            proposal.total_reviews,
            proposal.review_score,
            proposal.review_score_desc,
            wilson,
            if proposal.filter_offtopic_activity {
                1
            } else {
                0
            },
            proposal.parameter_hash,
            proposal.content_hash,
            proposal.source,
        ],
    )?;
    Ok(())
}

pub fn ingest_popular_reviews(
    conn: &Connection,
    proposal: &PopularReviewsProposal,
    now_ms: i64,
) -> StorageResult<()> {
    catalog::ensure_app_stub(
        conn,
        proposal.app_id,
        &format!("app-{}", proposal.app_id),
        now_ms,
    )?;
    conn.execute(
        "DELETE FROM popular_reviews WHERE app_id = ?1",
        params![proposal.app_id],
    )?;
    for (index, review) in proposal.reviews.iter().take(10).enumerate() {
        conn.execute(
            "INSERT INTO popular_reviews (
                app_id, recommendation_id, rank, language, author_name, author_profile_url,
                review_text, voted_up, votes_up, votes_funny, comment_count,
                playtime_forever_minutes, playtime_at_review_minutes, created_at_s, updated_at_s,
                steam_purchase, received_for_free, written_during_early_access,
                parameter_hash, content_hash, source, captured_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                       ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
            params![
                proposal.app_id,
                review.recommendation_id,
                (index + 1) as i64,
                review.language,
                review.author_name,
                review.author_profile_url,
                review.review_text,
                i64::from(review.voted_up),
                review.votes_up,
                review.votes_funny,
                review.comment_count,
                review.playtime_forever_minutes,
                review.playtime_at_review_minutes,
                review.created_at_s,
                review.updated_at_s,
                i64::from(review.steam_purchase),
                i64::from(review.received_for_free),
                i64::from(review.written_during_early_access),
                proposal.parameter_hash,
                proposal.content_hash,
                proposal.source,
                now_ms,
            ],
        )?;
    }
    conn.execute(
        "INSERT INTO popular_review_refresh_state(app_id, captured_at_ms, result_count, source)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(app_id) DO UPDATE SET
             captured_at_ms = excluded.captured_at_ms,
             result_count = excluded.result_count,
             source = excluded.source",
        params![
            proposal.app_id,
            now_ms,
            proposal.reviews.len().min(10) as i64,
            proposal.source,
        ],
    )?;
    Ok(())
}

pub fn ingest_ccu(conn: &Connection, proposal: &CcuProposal, now_ms: i64) -> StorageResult<()> {
    catalog::ensure_app_stub(
        conn,
        proposal.app_id,
        &format!("app-{}", proposal.app_id),
        now_ms,
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO player_snapshots (
            app_id, captured_at_ms, player_count, result_code, missing_reason,
            content_hash, source, offline_players_excluded
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            proposal.app_id,
            now_ms,
            proposal.player_count,
            proposal.result_code,
            proposal.missing_reason,
            proposal.content_hash,
            proposal.source,
            if proposal.offline_players_excluded {
                1
            } else {
                0
            },
        ],
    )?;
    upsert_player_daily(conn, proposal.app_id, proposal.player_count, now_ms)?;
    Ok(())
}

pub fn ingest_store_details(
    conn: &Connection,
    details: &StoreDetailsProposal,
    relations: &[AppRelationProposal],
    now_ms: i64,
) -> StorageResult<()> {
    let app_type = app_type_str(details.app_type);
    let proposed_release_state = release_state_str(details.release_state);
    let name = details
        .name
        .clone()
        .unwrap_or_else(|| format!("app-{}", details.app_id));

    // Capture prior release state/date for event log.
    let prior = catalog::get_app(conn, details.app_id)?;
    let release_state = if details.release_date_observed {
        proposed_release_state
    } else {
        prior
            .as_ref()
            .map_or(proposed_release_state, |app| app.release_state.as_str())
    }
    .to_owned();
    upsert_app(
        conn,
        details.app_id,
        app_type,
        &name,
        &release_state,
        details.release_date.as_deref(),
        details.release_date_precision.as_deref(),
        None,
        now_ms,
    )?;
    conn.execute(
        "INSERT INTO store_detail_refresh_state(
             app_id, country_code, language, captured_at_ms, status, source
         ) VALUES (?1, ?2, ?3, ?4, 'succeeded', ?5)
         ON CONFLICT(app_id, country_code, language) DO UPDATE SET
             captured_at_ms = excluded.captured_at_ms,
             status = excluded.status,
             source = excluded.source",
        params![
            details.app_id,
            details.country_code.trim().to_ascii_uppercase(),
            details.language.trim().to_ascii_lowercase(),
            now_ms,
            details.source
        ],
    )?;
    catalog::upsert_app_localization(
        conn,
        details.app_id,
        &details.language,
        details.name.as_deref(),
        details.short_description.as_deref(),
        details.source,
        now_ms,
    )?;
    if let Some(header_image_url) = details.header_image_url.as_deref() {
        conn.execute(
            "INSERT INTO app_media (app_id, capsule_url, source, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(app_id) DO UPDATE SET
                 capsule_url = excluded.capsule_url,
                 source = excluded.source,
                 updated_at_ms = excluded.updated_at_ms",
            params![details.app_id, header_image_url, details.source, now_ms],
        )?;
    }
    // Store adapters distinguish an explicit unknown date from a temporarily absent field.
    if details.release_date_observed {
        conn.execute(
            "UPDATE apps
             SET release_date = ?1, release_date_raw = ?2, release_date_precision = ?3,
                 updated_at_ms = ?4
             WHERE app_id = ?5",
            params![
                details.release_date,
                details.release_date_raw,
                details.release_date_precision,
                now_ms,
                details.app_id
            ],
        )?;
    }

    if details.release_date_observed
        && let Some(prev) = prior
        && (prev.release_state != release_state
            || prev.release_date != details.release_date
            || prev.release_date_raw != details.release_date_raw
            || prev.release_date_precision != details.release_date_precision)
    {
        conn.execute(
            "INSERT INTO release_events (
                app_id, old_release_date, new_release_date, old_precision, new_precision,
                old_release_state, new_release_state, source, observed_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                details.app_id,
                prev.release_date.or(prev.release_date_raw),
                details
                    .release_date
                    .as_ref()
                    .or(details.release_date_raw.as_ref()),
                prev.release_date_precision,
                details.release_date_precision,
                prev.release_state,
                release_state,
                details.source,
                now_ms
            ],
        )?;
    }

    if let Some(platforms) = details
        .platforms
        .as_ref()
        .filter(|values| !values.is_empty())
    {
        insert_feature_evidence(
            conn,
            details.app_id,
            "platforms",
            &serde_json::json!(platforms),
            "steam_store",
            details.source,
            0.9,
            now_ms,
        )?;
    }
    if let Some(languages) = details
        .supported_languages
        .as_ref()
        .filter(|values| !values.is_empty())
    {
        insert_feature_evidence(
            conn,
            details.app_id,
            "languages",
            &serde_json::json!(languages),
            "steam_store",
            details.source,
            0.8,
            now_ms,
        )?;
    }
    let platforms = if has_active_override(conn, details.app_id, "platforms")? {
        None
    } else {
        details
            .platforms
            .as_deref()
            .filter(|values| !values.is_empty())
    };
    let languages = if has_active_override(conn, details.app_id, "languages")? {
        None
    } else {
        details
            .supported_languages
            .as_deref()
            .filter(|values| !values.is_empty())
    };
    catalog::upsert_app_availability(
        conn,
        details.app_id,
        platforms,
        languages,
        details.is_free,
        now_ms,
    )?;

    for relation in relations {
        let rel = relation_type_str(&relation.relation_type);
        upsert_relation(
            conn,
            relation.source_app_id,
            relation.target_app_id,
            rel,
            relation.confidence,
            false,
            now_ms,
        )?;
    }

    // Store all category hints as one atomic observation. Writing one row per
    // label would deactivate the previous label because evidence replacement
    // is scoped by (app, feature, source_type).
    if !details.multiplayer_category_hints.is_empty() {
        insert_feature_evidence(
            conn,
            details.app_id,
            "category_hint",
            &serde_json::json!(details.multiplayer_category_hints),
            "store_category",
            details.source,
            0.3,
            now_ms,
        )?;
    }
    materialize_store_category_profile(
        conn,
        details.app_id,
        &details.multiplayer_category_hints,
        now_ms,
    )?;

    if let Some(price) = &details.price {
        conn.execute(
            "INSERT OR REPLACE INTO price_snapshots (
                app_id, country_code, currency, captured_at_ms,
                initial_price_minor, final_price_minor, discount_percent,
                is_purchasable, package_id, source
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                details.app_id,
                price.country_code,
                price.currency,
                now_ms,
                price.initial_price_minor,
                price.final_price_minor,
                price.discount_percent,
                match price.is_purchasable {
                    Some(true) => Some(1_i64),
                    Some(false) => Some(0_i64),
                    None => None,
                },
                price.package_id,
                details.source,
            ],
        )?;
    }
    Ok(())
}

const STORE_CATEGORY_PROFILE_CONFIDENCE: f64 = 0.3;

/// Derive a coarse dominant mode from Steam store multiplayer category labels.
/// Returns (mode, has_coop, has_competitive, has_online_coop_flag, has_crossplay).
fn derive_mode_from_category_labels(
    labels: &[String],
) -> (Option<&'static str>, bool, bool, bool, bool) {
    let has_coop = labels.iter().any(|label| {
        label.contains("co-op")
            || label.contains("coop")
            || label.contains("cooperative")
            || label.contains("co operative")
    });
    // Friend-group co-op signal: any co-op category (private or online).
    let has_online_coop = has_coop
        || labels.iter().any(|label| {
            label.contains("online co-op")
                || label.contains("online coop")
                || label.contains("shared/split screen co-op")
                || label.contains("shared/split-screen co-op")
        });
    let has_competitive = labels.iter().any(|label| {
        label.contains("pvp")
            || label.contains("competitive")
            || label.contains("versus")
            || label.contains(" vs ")
            || label.ends_with(" vs")
            || label.starts_with("vs ")
    });
    let has_crossplay = labels.iter().any(|label| {
        label.contains("cross-platform multiplayer")
            || label.contains("crossplay")
            || label.contains("cross-play")
    });
    let has_multiplayer = labels.iter().any(|label| {
        label.contains("multi-player")
            || label.contains("multiplayer")
            || label.contains("multi player")
            || label.contains("online pvp")
            || label.contains("online multiplayer")
    });
    // mixed = both coop and competitive/PvP → UI shows 合作/对抗
    let mode = match (has_coop, has_competitive) {
        (true, true) => Some("mixed"),
        (true, false) => Some("coop"),
        (false, true) => Some("pvp"),
        // Multiplayer-only catalog filter without coop/pvp tags: still not "未知".
        (false, false) if has_multiplayer || has_online_coop => Some("multiplayer"),
        (false, false) => None,
    };
    (
        mode,
        has_coop,
        has_competitive,
        has_online_coop,
        has_crossplay,
    )
}

fn materialize_store_category_profile(
    conn: &Connection,
    app_id: u32,
    hints: &[String],
    now_ms: i64,
) -> StorageResult<bool> {
    let labels: Vec<String> = hints
        .iter()
        .map(|hint| hint.trim().to_ascii_lowercase())
        .filter(|hint| !hint.is_empty())
        .collect();
    if labels.is_empty() {
        return Ok(false);
    }
    let (mode, _has_coop, _has_competitive, has_online_coop, has_crossplay) =
        derive_mode_from_category_labels(&labels);
    let source_ref = format!("steam_store_categories:{}", hints.join("|"));

    conn.execute(
        "INSERT INTO multiplayer_profiles (app_id, computed_at_ms)
         VALUES (?1, ?2) ON CONFLICT(app_id) DO NOTHING",
        params![app_id, now_ms],
    )?;
    let mut applied = false;

    for (feature, column, value) in [
        ("online_coop", "online_coop", has_online_coop),
        ("crossplay", "crossplay", has_crossplay),
    ] {
        if !value {
            continue;
        }
        insert_feature_evidence(
            conn,
            app_id,
            feature,
            &serde_json::json!(true),
            "steam_store_profile_derived",
            &source_ref,
            STORE_CATEGORY_PROFILE_CONFIDENCE,
            now_ms,
        )?;
        if !has_active_override(conn, app_id, feature)? {
            let sql = format!(
                "UPDATE multiplayer_profiles SET {column} = 1, computed_at_ms = ?1
                 WHERE app_id = ?2 AND (
                     {column} IS NULL
                     OR ({column} <> 1 AND COALESCE(profile_confidence, 0) <= ?3)
                 )"
            );
            applied |= conn.execute(
                &sql,
                params![now_ms, app_id, STORE_CATEGORY_PROFILE_CONFIDENCE],
            )? > 0;
        }
    }

    if let Some(mode) = mode {
        insert_feature_evidence(
            conn,
            app_id,
            "dominant_mode",
            &serde_json::json!(mode),
            "steam_store_profile_derived",
            &source_ref,
            STORE_CATEGORY_PROFILE_CONFIDENCE,
            now_ms,
        )?;
        if !has_active_override(conn, app_id, "dominant_mode")? {
            applied |= conn.execute(
                "UPDATE multiplayer_profiles
                 SET dominant_mode = ?1, computed_at_ms = ?2
                 WHERE app_id = ?3
                   AND (
                       dominant_mode IS NULL
                       OR (dominant_mode <> ?1 AND COALESCE(profile_confidence, 0) <= ?4)
                   )",
                params![mode, now_ms, app_id, STORE_CATEGORY_PROFILE_CONFIDENCE],
            )? > 0;
        }
    }

    insert_feature_evidence(
        conn,
        app_id,
        "recommended_min_players",
        &serde_json::json!(2),
        "steam_store_profile_derived",
        &source_ref,
        STORE_CATEGORY_PROFILE_CONFIDENCE,
        now_ms,
    )?;
    applied |= conn.execute(
        "UPDATE multiplayer_profiles
         SET recommended_min_players = 2, computed_at_ms = ?1
         WHERE app_id = ?2 AND recommended_min_players IS NULL",
        params![now_ms, app_id],
    )? > 0;
    conn.execute(
        "UPDATE multiplayer_profiles
         SET profile_confidence = MAX(COALESCE(profile_confidence, 0), ?1), computed_at_ms = ?2
         WHERE app_id = ?3",
        params![STORE_CATEGORY_PROFILE_CONFIDENCE, now_ms, app_id],
    )?;
    Ok(applied)
}

pub fn materialize_store_category_profiles(conn: &Connection, now_ms: i64) -> StorageResult<usize> {
    let mut stmt = conn.prepare(
        "SELECT app_id, value_json, source_type FROM feature_evidence
         WHERE feature_name = 'category_hint'
           AND source_type IN ('store_category', 'store_search_category')
           AND is_active = 1
         ORDER BY app_id, evidence_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)? as u32,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut grouped: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for row in rows {
        let (app_id, value_json, source_type) = row?;
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&value_json) {
            match value {
                serde_json::Value::String(label) => {
                    grouped.entry(app_id).or_default().push(label);
                }
                serde_json::Value::Array(labels) => {
                    grouped.entry(app_id).or_default().extend(
                        labels
                            .into_iter()
                            .filter_map(|label| label.as_str().map(str::to_owned)),
                    );
                }
                _ if source_type == "store_search_category" => {
                    grouped
                        .entry(app_id)
                        .or_default()
                        .push("Multi-player".into());
                }
                _ => {}
            }
        }
    }
    drop(stmt);

    let mut applied = 0_usize;
    for (app_id, hints) in grouped {
        if materialize_store_category_profile(conn, app_id, &hints, now_ms)? {
            applied += 1;
        }
    }
    Ok(applied)
}

pub fn ingest_store_search_page(
    conn: &Connection,
    page: &StoreSearchPage,
    now_ms: i64,
) -> StorageResult<usize> {
    conn.execute(
        "INSERT INTO source_documents (
            source, entity_type, entity_key, content_type, content_hash,
            content_text, fetched_at_ms, parse_version
         ) VALUES (?1, 'search_page', ?2, 'application/json', ?3, NULL, ?4, ?5)",
        params![
            STORE_SEARCH_SOURCE_NAME,
            format!("multiplayer:reviews_desc:{}", page.start),
            page.content_hash,
            now_ms,
            STORE_SEARCH_ADAPTER_VERSION,
        ],
    )?;
    let document_id = conn.last_insert_rowid();
    let source_ref = format!(
        "steam_store_search:category2=1;sort=Reviews_DESC;start={};sha256={}",
        page.start, page.content_hash
    );
    let evidence_value = serde_json::json!({
        "category": "Multi-player",
        "filter": "category2=1",
        "sort": "Reviews_DESC"
    });

    for candidate in &page.candidates {
        ingest_app_catalog(conn, &candidate.catalog_proposal(), now_ms)?;
        insert_feature_evidence_with_document(
            conn,
            candidate.app_id,
            "category_hint",
            &evidence_value,
            "store_search_category",
            &source_ref,
            Some(document_id),
            0.3,
            now_ms,
        )?;
    }
    Ok(page.candidates.len())
}

/// Apply a source-derived multiplayer boolean without clobbering human overrides.
#[allow(clippy::too_many_arguments)]
pub fn ingest_multiplayer_bool(
    conn: &Connection,
    app_id: u32,
    feature_name: &str,
    value: bool,
    source_type: &str,
    source_ref: &str,
    confidence: f64,
    now_ms: i64,
) -> StorageResult<bool> {
    insert_feature_evidence(
        conn,
        app_id,
        feature_name,
        &serde_json::json!(value),
        source_type,
        source_ref,
        confidence,
        now_ms,
    )?;

    if has_active_override(conn, app_id, feature_name)? {
        return Ok(false);
    }

    catalog::set_profile_bool_field(conn, app_id, feature_name, Some(value), now_ms)?;
    Ok(true)
}

/// Import a human golden multiplayer profile without clobbering human overrides.
///
/// Writes feature evidence for every known label and applies profile fields when
/// no active override exists. Does not invent Steam store metrics (reviews/CCU).
pub fn import_golden_multiplayer_profile(
    conn: &Connection,
    game: &GoldenGame,
    now_ms: i64,
) -> StorageResult<bool> {
    catalog::ensure_app_stub(conn, game.app_id, &game.name, now_ms)?;
    // Prefer curated names and promote stubs so golden titles enter game candidates.
    if let Some(app) = catalog::get_app(conn, game.app_id)? {
        let needs_name = app.canonical_name.starts_with("app-") || app.canonical_name.is_empty();
        let needs_type = app.app_type == "unknown";
        if needs_name || needs_type {
            catalog::upsert_app(
                conn,
                game.app_id,
                if needs_type {
                    "game"
                } else {
                    app.app_type.as_str()
                },
                if needs_name {
                    game.name.as_str()
                } else {
                    app.canonical_name.as_str()
                },
                app.release_state.as_str(),
                app.release_date.as_deref(),
                app.release_date_precision.as_deref(),
                app.source_modified_at_ms,
                now_ms,
            )?;
        }
    }

    let content_text = serde_json::to_string(game)?;
    let hash = content_hash(content_text.as_bytes());
    let source_ref = format!(
        "golden:{GOLDEN_SET_VERSION}:app={}:sha256={hash}",
        game.app_id
    );
    let document_id = golden_source_document(conn, game.app_id, &hash, &content_text, now_ms)?;
    // Only second-pass labels qualify for the >= 0.8 trusted-data gate.
    let confidence = if game.dual_reviewed { 0.85 } else { 0.65 };
    let mut applied_any = false;

    let mp = &game.multiplayer;
    for (feature, value) in [
        ("private_session", mp.private_session),
        ("self_hosted_server", mp.self_host_or_dedicated),
        ("online_coop", mp.online_coop),
        ("drop_in_out", mp.drop_in_out),
        ("crossplay", mp.crossplay),
    ] {
        if let Some(flag) = value {
            let evidence_changed = insert_golden_evidence(
                conn,
                game.app_id,
                feature,
                &serde_json::json!(flag),
                &source_ref,
                document_id,
                confidence,
                now_ms,
            )?;
            if evidence_changed {
                applied_any = true;
            }
            if evidence_changed && !has_active_override(conn, game.app_id, feature)? {
                catalog::set_profile_bool_field(conn, game.app_id, feature, Some(flag), now_ms)?;
            }
        }
    }

    // Evidence-only labels (not first-class multiplayer_profiles columns).
    for (feature, value) in [
        ("matchmaking_core", mp.matchmaking_core),
        ("public_world_dependency", mp.public_world_dependency),
        ("service_shutdown_risk", mp.service_shutdown_risk),
    ] {
        if let Some(flag) = value
            && insert_golden_evidence(
                conn,
                game.app_id,
                feature,
                &serde_json::json!(flag),
                &source_ref,
                document_id,
                confidence,
                now_ms,
            )?
        {
            applied_any = true;
        }
    }

    if !matches!(mp.dominant_mode, DominantModeLabel::Unknown) {
        let mode = dominant_mode_label(mp.dominant_mode);
        let evidence_changed = insert_golden_evidence(
            conn,
            game.app_id,
            "dominant_mode",
            &serde_json::json!(mode),
            &source_ref,
            document_id,
            confidence,
            now_ms,
        )?;
        if evidence_changed && !has_active_override(conn, game.app_id, "dominant_mode")? {
            catalog::set_profile_text_field(
                conn,
                game.app_id,
                "dominant_mode",
                Some(mode),
                now_ms,
            )?;
        }
        applied_any |= evidence_changed;
    }

    // Ensure profile row exists before bounds/confidence updates.
    conn.execute(
        "INSERT INTO multiplayer_profiles (app_id, computed_at_ms) VALUES (?1, ?2)
         ON CONFLICT(app_id) DO NOTHING",
        params![game.app_id, now_ms],
    )?;

    if mp.recommended_min_players.is_some() || mp.recommended_max_players.is_some() {
        let changed = conn.execute(
            "UPDATE multiplayer_profiles
             SET recommended_min_players = COALESCE(?1, recommended_min_players),
                 recommended_max_players = COALESCE(?2, recommended_max_players),
                 computed_at_ms = ?3
             WHERE app_id = ?4
               AND (recommended_min_players IS NOT COALESCE(?1, recommended_min_players)
                    OR recommended_max_players IS NOT COALESCE(?2, recommended_max_players))",
            params![
                mp.recommended_min_players.map(i64::from),
                mp.recommended_max_players.map(i64::from),
                now_ms,
                game.app_id
            ],
        )?;
        applied_any |= changed > 0;
    }

    let confidence_changed = conn.execute(
        "UPDATE multiplayer_profiles
         SET profile_confidence = MAX(COALESCE(profile_confidence, 0), ?1),
             computed_at_ms = ?2
         WHERE app_id = ?3 AND COALESCE(profile_confidence, 0) < ?1",
        params![confidence, now_ms, game.app_id],
    )?;
    applied_any |= confidence_changed > 0;

    Ok(applied_any)
}

fn golden_source_document(
    conn: &Connection,
    app_id: u32,
    hash: &str,
    content_text: &str,
    now_ms: i64,
) -> StorageResult<i64> {
    let entity_key = format!("{GOLDEN_SET_VERSION}:{app_id}");
    let existing = conn
        .query_row(
            "SELECT document_id FROM source_documents
             WHERE source = 'human_golden' AND entity_type = 'golden_game'
               AND entity_key = ?1 AND content_hash = ?2
             ORDER BY document_id DESC LIMIT 1",
            params![entity_key, hash],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(document_id) = existing {
        return Ok(document_id);
    }
    conn.execute(
        "INSERT INTO source_documents (
            source, entity_type, entity_key, content_type, content_hash,
            content_text, fetched_at_ms, parse_version
         ) VALUES ('human_golden', 'golden_game', ?1, 'application/json', ?2, ?3, ?4, ?5)",
        params![entity_key, hash, content_text, now_ms, GOLDEN_SET_VERSION],
    )?;
    Ok(conn.last_insert_rowid())
}

#[allow(clippy::too_many_arguments)]
fn insert_golden_evidence(
    conn: &Connection,
    app_id: u32,
    feature_name: &str,
    value: &serde_json::Value,
    source_ref: &str,
    document_id: i64,
    confidence: f64,
    now_ms: i64,
) -> StorageResult<bool> {
    let value_json = serde_json::to_string(value)?;
    let exists: bool = conn.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM feature_evidence
             WHERE app_id = ?1 AND feature_name = ?2
               AND source_type = 'human_golden' AND source_ref = ?3
               AND value_json = ?4 AND confidence = ?5 AND is_active = 1
         )",
        params![app_id, feature_name, source_ref, value_json, confidence],
        |row| row.get(0),
    )?;
    if exists {
        return Ok(false);
    }
    insert_feature_evidence_with_document(
        conn,
        app_id,
        feature_name,
        value,
        "human_golden",
        source_ref,
        Some(document_id),
        confidence,
        now_ms,
    )?;
    Ok(true)
}

fn dominant_mode_label(mode: DominantModeLabel) -> &'static str {
    match mode {
        DominantModeLabel::Coop => "coop",
        DominantModeLabel::Competitive => "competitive",
        DominantModeLabel::Mixed => "mixed",
        DominantModeLabel::Mmo => "mmo",
        DominantModeLabel::SinglePrimary => "single_primary",
        DominantModeLabel::Unknown => "unknown",
    }
}

struct DailyAgg {
    min_ccu: Option<i64>,
    max_ccu: Option<i64>,
    mean_ccu: Option<f64>,
    sample_count: i64,
    missing_rate: f64,
}

fn upsert_player_daily(
    conn: &Connection,
    app_id: u32,
    player_count: Option<u32>,
    now_ms: i64,
) -> StorageResult<()> {
    let day = day_utc_from_ms(now_ms);
    let existing: Option<DailyAgg> = conn
        .query_row(
            "SELECT min_ccu, max_ccu, mean_ccu, sample_count, missing_rate
             FROM player_daily WHERE app_id = ?1 AND day_utc = ?2",
            params![app_id, day],
            |row| {
                Ok(DailyAgg {
                    min_ccu: row.get(0)?,
                    max_ccu: row.get(1)?,
                    mean_ccu: row.get(2)?,
                    sample_count: row.get(3)?,
                    missing_rate: row.get(4)?,
                })
            },
        )
        .optional_compat()?;

    match (existing, player_count) {
        (None, Some(count)) => {
            let c = i64::from(count);
            conn.execute(
                "INSERT INTO player_daily (
                    app_id, day_utc, min_ccu, max_ccu, mean_ccu, median_approx_ccu,
                    sample_count, missing_rate, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?3, ?3, ?3, 1, 0, ?4)",
                params![app_id, day, c, now_ms],
            )?;
        }
        (None, None) => {
            conn.execute(
                "INSERT INTO player_daily (
                    app_id, day_utc, min_ccu, max_ccu, mean_ccu, median_approx_ccu,
                    sample_count, missing_rate, updated_at_ms
                 ) VALUES (?1, ?2, NULL, NULL, NULL, NULL, 1, 1, ?3)",
                params![app_id, day, now_ms],
            )?;
        }
        (Some(agg), Some(count)) => {
            let c = i64::from(count);
            let old_total = agg.sample_count.max(0);
            let old_missing = (agg.missing_rate * old_total as f64)
                .round()
                .clamp(0.0, old_total as f64) as i64;
            let old_valid = old_total - old_missing;
            let sample = old_total + 1;
            let valid_samples = old_valid + 1;
            let min_v = Some(agg.min_ccu.map_or(c, |m| m.min(c)));
            let max_v = Some(agg.max_ccu.map_or(c, |m| m.max(c)));
            let mean_v = agg.mean_ccu.map_or(c as f64, |m| {
                (m * old_valid as f64 + c as f64) / valid_samples as f64
            });
            let missing_rate = old_missing as f64 / sample as f64;
            conn.execute(
                "UPDATE player_daily SET
                    min_ccu = ?1, max_ccu = ?2, mean_ccu = ?3, median_approx_ccu = ?3,
                    sample_count = ?4, missing_rate = ?5, updated_at_ms = ?6
                 WHERE app_id = ?7 AND day_utc = ?8",
                params![
                    min_v,
                    max_v,
                    mean_v,
                    sample,
                    missing_rate,
                    now_ms,
                    app_id,
                    day
                ],
            )?;
        }
        (Some(agg), None) => {
            let old_total = agg.sample_count.max(0);
            let old_missing = (agg.missing_rate * old_total as f64)
                .round()
                .clamp(0.0, old_total as f64) as i64;
            let total_slots = old_total + 1;
            let missing = (old_missing + 1) as f64 / total_slots as f64;
            conn.execute(
                "UPDATE player_daily SET
                    sample_count = ?1, missing_rate = ?2, updated_at_ms = ?3
                 WHERE app_id = ?4 AND day_utc = ?5",
                params![total_slots, missing, now_ms, app_id, day],
            )?;
        }
    }
    Ok(())
}

trait OptionalCompat<T> {
    fn optional_compat(self) -> StorageResult<Option<T>>;
}

impl<T> OptionalCompat<T> for Result<T, rusqlite::Error> {
    fn optional_compat(self) -> StorageResult<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

fn app_type_str(value: AppTypeProposal) -> &'static str {
    match value {
        AppTypeProposal::Game => "game",
        AppTypeProposal::Demo => "demo",
        AppTypeProposal::Playtest => "playtest",
        AppTypeProposal::Dlc => "dlc",
        AppTypeProposal::Tool => "tool",
        AppTypeProposal::Application => "application",
        AppTypeProposal::Music => "music",
        AppTypeProposal::Video => "video",
        AppTypeProposal::Series => "series",
        AppTypeProposal::Comic => "comic",
        AppTypeProposal::Advertising => "advertising",
        AppTypeProposal::Mod => "mod",
        AppTypeProposal::Hardware => "hardware",
        AppTypeProposal::Unknown => "unknown",
    }
}

fn release_state_str(value: ReleaseStateProposal) -> &'static str {
    match value {
        ReleaseStateProposal::Released => "released",
        ReleaseStateProposal::Upcoming => "upcoming",
        ReleaseStateProposal::ComingSoon => "coming_soon",
        ReleaseStateProposal::Retired => "retired",
        ReleaseStateProposal::Unknown => "unknown",
    }
}

fn relation_type_str(value: &RelationTypeProposal) -> &'static str {
    match value {
        RelationTypeProposal::DemoOf => "demo_of",
        RelationTypeProposal::PlaytestOf => "playtest_of",
        RelationTypeProposal::DedicatedServerFor => "dedicated_server_for",
        RelationTypeProposal::EditionOf => "edition_of",
        RelationTypeProposal::Replaces => "replaces",
    }
}
