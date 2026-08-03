//! Deterministic catalog seed for local demos and M3 ranking tests.

use rusqlite::Connection;

use crate::catalog::{set_profile_bool_field, set_profile_text_field, upsert_app};
use crate::error::StorageResult;
use crate::util::wilson_lower_bound;
use rusqlite::params;

struct SeedGame {
    app_id: u32,
    name: &'static str,
    release_state: &'static str,
    release_date: Option<&'static str>,
    dominant_mode: &'static str,
    private_session: bool,
    online_coop: bool,
    self_host: bool,
    rec_min: i64,
    rec_max: i64,
    reviews: u32,
    positive: u32,
    ccu: u32,
    section_hint: &'static str,
}

const SEED: &[SeedGame] = &[
    SeedGame {
        app_id: 1623730,
        name: "Palworld",
        release_state: "released",
        release_date: Some("2024-01-19"),
        dominant_mode: "coop",
        private_session: true,
        online_coop: true,
        self_host: true,
        rec_min: 1,
        rec_max: 32,
        reviews: 200_000,
        positive: 180_000,
        ccu: 40_000,
        section_hint: "recent_release",
    },
    SeedGame {
        app_id: 346110,
        name: "ARK: Survival Evolved",
        release_state: "released",
        release_date: Some("2017-08-29"),
        dominant_mode: "coop",
        private_session: true,
        online_coop: true,
        self_host: true,
        rec_min: 1,
        rec_max: 70,
        reviews: 400_000,
        positive: 300_000,
        ccu: 25_000,
        section_hint: "popular_legacy",
    },
    SeedGame {
        app_id: 548430,
        name: "Deep Rock Galactic",
        release_state: "released",
        release_date: Some("2020-05-13"),
        dominant_mode: "private_coop",
        private_session: true,
        online_coop: true,
        self_host: false,
        rec_min: 1,
        rec_max: 4,
        reviews: 300_000,
        positive: 290_000,
        ccu: 900,
        section_hint: "classic_legacy",
    },
    SeedGame {
        app_id: 632360,
        name: "Risk of Rain 2",
        release_state: "released",
        release_date: Some("2020-08-11"),
        dominant_mode: "private_coop",
        private_session: true,
        online_coop: true,
        self_host: false,
        rec_min: 1,
        rec_max: 4,
        reviews: 200_000,
        positive: 190_000,
        ccu: 800,
        section_hint: "classic_legacy",
    },
    SeedGame {
        app_id: 892970,
        name: "Valheim",
        release_state: "released",
        release_date: Some("2021-02-02"),
        dominant_mode: "self_hosted_survival",
        private_session: true,
        online_coop: true,
        self_host: true,
        rec_min: 1,
        rec_max: 10,
        reviews: 400_000,
        positive: 380_000,
        ccu: 700,
        section_hint: "classic_legacy",
    },
    SeedGame {
        app_id: 730,
        name: "Counter-Strike 2",
        release_state: "released",
        release_date: Some("2023-09-27"),
        dominant_mode: "matchmaking_competitive",
        private_session: true,
        online_coop: false,
        self_host: true,
        rec_min: 2,
        rec_max: 10,
        reviews: 1_000_000,
        positive: 850_000,
        ccu: 900_000,
        section_hint: "popular_legacy",
    },
    SeedGame {
        app_id: 1172470,
        name: "Apex Legends",
        release_state: "released",
        release_date: Some("2020-11-04"),
        dominant_mode: "matchmaking_competitive",
        private_session: true,
        online_coop: false,
        self_host: false,
        rec_min: 1,
        rec_max: 3,
        reviews: 500_000,
        positive: 400_000,
        ccu: 100_000,
        section_hint: "popular_legacy",
    },
    SeedGame {
        app_id: 2500001,
        name: "Upcoming Co-op Demo Sample",
        release_state: "coming_soon",
        release_date: Some("2026-10-01"),
        dominant_mode: "private_coop",
        private_session: true,
        online_coop: true,
        self_host: false,
        rec_min: 2,
        rec_max: 4,
        reviews: 0,
        positive: 0,
        ccu: 0,
        section_hint: "upcoming",
    },
    SeedGame {
        app_id: 1966720,
        name: "Lethal Company",
        release_state: "released",
        release_date: Some("2023-10-23"),
        dominant_mode: "private_coop",
        private_session: true,
        online_coop: true,
        self_host: false,
        rec_min: 1,
        rec_max: 4,
        reviews: 350_000,
        positive: 340_000,
        ccu: 15_000,
        section_hint: "recent_release",
    },
    SeedGame {
        app_id: 553850,
        name: "HELLDIVERS 2",
        release_state: "released",
        release_date: Some("2024-02-08"),
        dominant_mode: "coop",
        private_session: true,
        online_coop: true,
        self_host: false,
        rec_min: 1,
        rec_max: 4,
        reviews: 500_000,
        positive: 400_000,
        ccu: 30_000,
        section_hint: "recent_release",
    },
    SeedGame {
        app_id: 2500002,
        name: "Recent Co-op Sample",
        release_state: "released",
        release_date: None,
        dominant_mode: "private_coop",
        private_session: true,
        online_coop: true,
        self_host: false,
        rec_min: 2,
        rec_max: 4,
        reviews: 5_000,
        positive: 4_500,
        ccu: 500,
        section_hint: "rolling_recent_sample",
    },
    SeedGame {
        app_id: 2500003,
        name: "Classic Co-op Sample",
        release_state: "released",
        release_date: Some("2019-06-01"),
        dominant_mode: "private_coop",
        private_session: true,
        online_coop: true,
        self_host: false,
        rec_min: 2,
        rec_max: 4,
        reviews: 8_000,
        positive: 7_200,
        ccu: 250,
        section_hint: "classic_legacy",
    },
];

fn seed_capsule_url(app_id: u32) -> Option<String> {
    // Synthetic examples intentionally keep the letter fallback, while known
    // Steam apps use the same approved CDN header convention as catalog sync.
    (app_id < 2_000_000)
        .then(|| format!("https://cdn.akamai.steamstatic.com/steam/apps/{app_id}/header.jpg"))
}

/// Seed a minimal multiplayer catalog if apps table is empty.
pub fn seed_demo_catalog_if_empty(conn: &Connection, now_ms: i64) -> StorageResult<usize> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM apps", [], |row| row.get(0))?;
    if count > 0 {
        return Ok(0);
    }
    seed_demo_catalog(conn, now_ms)
}

pub fn seed_demo_catalog(conn: &Connection, now_ms: i64) -> StorageResult<usize> {
    for game in SEED {
        let rolling_release_date = match game.section_hint {
            "rolling_recent_sample" => Some(crate::util::day_utc_from_ms(
                now_ms.saturating_sub(30_i64 * 24 * 60 * 60 * 1000),
            )),
            // Upcoming eligibility is intentionally limited to the next 30
            // days, so keep the local demo seed valid relative to its clock.
            "upcoming" => Some(crate::util::day_utc_from_ms(
                now_ms.saturating_add(14_i64 * 24 * 60 * 60 * 1000),
            )),
            _ => None,
        };
        let release_date = rolling_release_date.as_deref().or(game.release_date);
        upsert_app(
            conn,
            game.app_id,
            "game",
            game.name,
            game.release_state,
            release_date,
            release_date.map(|_| "day"),
            None,
            now_ms,
        )?;
        if let Some(capsule_url) = seed_capsule_url(game.app_id) {
            conn.execute(
                "INSERT INTO app_media (app_id, capsule_url, source, updated_at_ms)
                 VALUES (?1, ?2, 'demo_seed', ?3)
                 ON CONFLICT(app_id) DO UPDATE SET
                    capsule_url = excluded.capsule_url,
                    source = excluded.source,
                    updated_at_ms = excluded.updated_at_ms",
                params![game.app_id, capsule_url, now_ms],
            )?;
        }
        set_profile_text_field(
            conn,
            game.app_id,
            "dominant_mode",
            Some(game.dominant_mode),
            now_ms,
        )?;
        set_profile_bool_field(
            conn,
            game.app_id,
            "private_session",
            Some(game.private_session),
            now_ms,
        )?;
        set_profile_bool_field(
            conn,
            game.app_id,
            "online_coop",
            Some(game.online_coop),
            now_ms,
        )?;
        set_profile_bool_field(
            conn,
            game.app_id,
            "self_hosted_server",
            Some(game.self_host),
            now_ms,
        )?;
        conn.execute(
            "UPDATE multiplayer_profiles
             SET recommended_min_players = ?1, recommended_max_players = ?2,
                 profile_confidence = 0.85, computed_at_ms = ?3
             WHERE app_id = ?4",
            params![game.rec_min, game.rec_max, now_ms, game.app_id],
        )?;
        if game.reviews > 0 {
            let wilson = wilson_lower_bound(game.positive, game.reviews);
            conn.execute(
                "INSERT OR REPLACE INTO review_snapshots (
                    app_id, region_scope, language_scope, captured_at_ms,
                    total_positive, total_negative, total_reviews, review_score, review_score_desc,
                    wilson_lower, filter_offtopic_activity, parameter_hash, content_hash, source
                 ) VALUES (?1, 'all', 'all', ?2, ?3, ?4, ?5, 8, 'Very Positive', ?6, 1, 'seed', 'seed', 'seed')",
                params![
                    game.app_id,
                    now_ms,
                    game.positive,
                    game.reviews.saturating_sub(game.positive),
                    game.reviews,
                    wilson
                ],
            )?;
        }
        if game.ccu > 0 {
            conn.execute(
                "INSERT OR REPLACE INTO player_snapshots (
                    app_id, captured_at_ms, player_count, result_code, missing_reason,
                    content_hash, source, offline_players_excluded
                 ) VALUES (?1, ?2, ?3, 1, NULL, 'seed', 'seed', 1)",
                params![game.app_id, now_ms, game.ccu],
            )?;
        }
        let _ = game.section_hint;
    }
    Ok(SEED.len())
}
