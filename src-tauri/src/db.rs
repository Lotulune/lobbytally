use crate::backfill_task::BACKFILL_MAX_ATTEMPTS;
use crate::discovery::{parse_saved_cursor, DISCOVERY_CURSOR_CONFIG_KEY};
use crate::llm::AnalysisNarrative;
use crate::models::{
    AiAnalysisQueueFailureItem, AiAnalysisQueueSource, ClassicDiscoveryRejectCacheEntry,
    ClassicDiscoveryRunSnapshot, ClassicRejectReasonCode, DashboardPayload, DashboardStats,
    DiscoveryCompletionReason, DiscoveryFailureItem, DiscoveryRunSnapshot, DiscoveryRunStatus,
    DiscoveryTaskRequest, GameAnalysisReport, GameCard, LlmProvider, PublicConfig,
    ReviewSnippet, StoreReleaseState, SyncMode, UserCollections, UserGameState,
    UserGameStatePatch,
};
use crate::recommendation::{
    bucket_game, compute_recommendation_score, today_iso_utc, DemoStatus, GameFacts, ReleaseBucket,
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::HashSet;
use std::path::Path;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub const DEFAULT_LLM_PROVIDER: LlmProvider = LlmProvider::Deepseek;
pub const DEFAULT_LLM_BASE_URL: &str = "https://api.deepseek.com";
pub const DEFAULT_LLM_MODEL: &str = "deepseek-v4-flash";
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4.1";
pub const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-20250514";
const STEAM_API_KEY_VALIDATED_CONFIG_KEY: &str = "steam_api_key_validated";
const LLM_CONFIG_VALIDATED_CONFIG_KEY: &str = "llm_config_validated";
const LLM_PROVIDER_CONFIG_KEY: &str = "llm_provider";
const ONBOARDING_COMPLETED_CONFIG_KEY: &str = "onboarding_completed";
const ONBOARDING_CURRENT_STEP_CONFIG_KEY: &str = "onboarding_current_step";
const ONBOARDING_LLM_PROVIDER_DRAFT_CONFIG_KEY: &str = "onboarding_llm_provider_draft";
const MAX_SQLITE_U32: i64 = u32::MAX as i64;
pub const DEFAULT_AI_BATCH_REFRESH_CONCURRENCY: u8 = 5;
pub const MIN_AI_BATCH_REFRESH_CONCURRENCY: u8 = 1;
pub const MAX_AI_BATCH_REFRESH_CONCURRENCY: u8 = 10;
const AI_BATCH_REFRESH_CONCURRENCY_CONFIG_KEY: &str = "ai_batch_refresh_concurrency";
pub const CLASSIC_DISCOVERY_RULE_VERSION: &str = "classic_v2";
pub const CLASSIC_DISCOVERY_MAX_PAGES_DEFAULT: u32 = 3;
pub const CLASSIC_DISCOVERY_PAGE_SIZE: u32 = 100;
pub const CLASSIC_DISCOVERY_COOLDOWN_HOURS: i64 = 24;
pub const CLASSIC_DISCOVERY_REJECT_CACHE_TTL_DAYS: i64 = 90;
pub const AI_ANALYSIS_QUEUE_MAX_ATTEMPTS: u8 = 3;
pub const CLASSIC_DISCOVERY_LAST_APPID_CONFIG_KEY: &str = "classic_discovery_last_appid";
pub const CLASSIC_DISCOVERY_LAST_OFFSET_CONFIG_KEY: &str = "classic_discovery_last_offset";
pub const CLASSIC_DISCOVERY_LAST_COMPLETED_AT_CONFIG_KEY: &str =
    "classic_discovery_last_completed_at";
pub const CLASSIC_DISCOVERY_LAST_STARTED_AT_CONFIG_KEY: &str = "classic_discovery_last_started_at";
pub const CLASSIC_DISCOVERY_LAST_RUN_STATUS_CONFIG_KEY: &str = "classic_discovery_last_run_status";
const CLASSIC_DISCOVERY_CLEANUP_VERSION_CONFIG_KEY: &str = "classic_discovery_cleanup_version";

#[derive(Debug, Clone)]
pub struct GameSeed {
    pub appid: u32,
    pub name: &'static str,
    pub release_date: &'static str,
    pub release_date_text: &'static str,
    pub demo_status: DemoStatus,
    pub positive_review_pct: f64,
    pub total_reviews: u32,
    pub current_players: u32,
    pub ai_score: f64,
    pub ai_summary: &'static str,
    pub tags: Vec<&'static str>,
    pub multiplayer_modes: Vec<&'static str>,
    pub section: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct DiscoveryProgressPatch {
    pub status: Option<DiscoveryRunStatus>,
    pub completion_reason: Option<Option<DiscoveryCompletionReason>>,
    pub current_appid: Option<Option<u32>>,
    pub last_appid: Option<Option<u32>>,
    pub pages_processed: Option<u32>,
    pub scanned_apps: Option<usize>,
    pub added_games: Option<usize>,
    pub added_new_games: Option<usize>,
    pub added_classic_games: Option<usize>,
    pub skipped_existing: Option<usize>,
    pub skipped_non_multiplayer: Option<usize>,
    pub failed_games: Option<usize>,
    pub have_more_results: Option<bool>,
    pub last_error: Option<Option<String>>,
    pub finished_at: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataBackfillJobRecord {
    pub appid: u32,
    pub attempt: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataBackfillErrorRecord {
    pub appid: u32,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncJobRecord {
    pub appid: u32,
    pub mode: SyncMode,
    pub attempt: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncQueueSummary {
    pub pending_count: usize,
    pub mode: SyncMode,
    pub last_error_appid: Option<u32>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiAnalysisQueueJobRecord {
    pub appid: u32,
    pub source: AiAnalysisQueueSource,
    pub attempt: u8,
    pub last_error: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct ClassicDiscoveryProgressPatch {
    pub status: Option<DiscoveryRunStatus>,
    pub pages_processed: Option<u32>,
    pub scanned_apps: Option<usize>,
    pub considered_apps: Option<usize>,
    pub added_games: Option<usize>,
    pub rejected_games: Option<usize>,
    pub skipped_existing: Option<usize>,
    pub skipped_rejected_cache: Option<usize>,
    pub failed_games: Option<usize>,
    pub current_appid: Option<Option<u32>>,
    pub last_appid: Option<Option<u32>>,
    pub consecutive_empty_pages: Option<u32>,
    pub last_error: Option<Option<String>>,
    pub finished_at: Option<Option<String>>,
}

pub fn open_database(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create app data dir {}", parent.display()))?;
    }

    let conn = Connection::open(path)
        .with_context(|| format!("open sqlite database {}", path.display()))?;
    migrate(&conn)?;
    purge_legacy_bootstrap_seed_games(&conn)?;
    Ok(conn)
}

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS app_config (
            key TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS games (
            appid INTEGER PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            short_description TEXT,
            section TEXT NOT NULL,
            release_date TEXT,
            release_date_text TEXT NOT NULL,
            release_state TEXT NOT NULL DEFAULT '"released"',
            demo_status TEXT NOT NULL,
            supported_languages_json TEXT NOT NULL DEFAULT '[]',
            is_adult_content INTEGER NOT NULL DEFAULT 0,
            is_free INTEGER NOT NULL DEFAULT 0,
            price_text TEXT,
            discount_percent INTEGER,
            positive_review_pct REAL,
            total_reviews INTEGER,
            current_players INTEGER,
            recommendation_score REAL NOT NULL,
            ai_score REAL,
            ai_summary TEXT NOT NULL,
            ai_analysis_report_json TEXT,
            ai_analysis_generated_at TEXT,
            capsule_url TEXT NOT NULL,
            store_screenshot_urls_json TEXT NOT NULL DEFAULT '[]',
            tags_json TEXT NOT NULL,
            multiplayer_modes_json TEXT NOT NULL,
            review_snippets_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS user_game_state (
            appid INTEGER PRIMARY KEY NOT NULL,
            favorite INTEGER NOT NULL DEFAULT 0,
            wishlist INTEGER NOT NULL DEFAULT 0,
            followed INTEGER NOT NULL DEFAULT 0,
            viewed INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL,
            FOREIGN KEY(appid) REFERENCES games(appid) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS discovery_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            status TEXT NOT NULL CHECK (
                status IN (
                    'running',
                    'paused',
                    'completed',
                    'failed',
                    'cancelled',
                    'interrupted'
                )
            ),
            sync_mode TEXT NOT NULL DEFAULT 'full' CHECK (
                sync_mode IN ('quick', 'full')
            ),
            completion_reason TEXT,
            target_added_games INTEGER NOT NULL
                CHECK (target_added_games >= 0 AND target_added_games <= 4294967295),
            page_size INTEGER NOT NULL
                CHECK (page_size > 0 AND page_size <= 4294967295),
            pages_processed INTEGER NOT NULL DEFAULT 0
                CHECK (pages_processed >= 0 AND pages_processed <= 4294967295),
            scanned_apps INTEGER NOT NULL DEFAULT 0 CHECK (scanned_apps >= 0),
            added_games INTEGER NOT NULL DEFAULT 0 CHECK (added_games >= 0),
            added_new_games INTEGER NOT NULL DEFAULT 0 CHECK (added_new_games >= 0),
            added_classic_games INTEGER NOT NULL DEFAULT 0 CHECK (added_classic_games >= 0),
            skipped_existing INTEGER NOT NULL DEFAULT 0 CHECK (skipped_existing >= 0),
            skipped_non_multiplayer INTEGER NOT NULL DEFAULT 0 CHECK (skipped_non_multiplayer >= 0),
            failed_games INTEGER NOT NULL DEFAULT 0 CHECK (failed_games >= 0),
            current_appid INTEGER
                CHECK (current_appid IS NULL OR (current_appid >= 0 AND current_appid <= 4294967295)),
            last_appid INTEGER
                CHECK (last_appid IS NULL OR (last_appid >= 0 AND last_appid <= 4294967295)),
            have_more_results INTEGER NOT NULL DEFAULT 1 CHECK (have_more_results IN (0, 1)),
            started_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            finished_at TEXT,
            last_error TEXT
        );

        CREATE TABLE IF NOT EXISTS discovery_failures (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id INTEGER NOT NULL,
            page_index INTEGER NOT NULL CHECK (page_index >= 0 AND page_index <= 4294967295),
            appid INTEGER CHECK (appid IS NULL OR (appid >= 0 AND appid <= 4294967295)),
            stage TEXT NOT NULL,
            reason TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY(run_id) REFERENCES discovery_runs(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS metadata_backfill_queue (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            appid INTEGER NOT NULL UNIQUE CHECK (appid >= 0 AND appid <= 4294967295),
            attempt INTEGER NOT NULL CHECK (attempt >= 1 AND attempt <= 255),
            last_error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS classic_discovery_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            status TEXT NOT NULL CHECK (
                status IN (
                    'running',
                    'paused',
                    'completed',
                    'failed',
                    'cancelled',
                    'interrupted'
                )
            ),
            max_pages INTEGER NOT NULL
                CHECK (max_pages >= 1 AND max_pages <= 4294967295),
            page_size INTEGER NOT NULL
                CHECK (page_size >= 1 AND page_size <= 4294967295),
            pages_processed INTEGER NOT NULL DEFAULT 0
                CHECK (pages_processed >= 0 AND pages_processed <= 4294967295),
            scanned_apps INTEGER NOT NULL DEFAULT 0 CHECK (scanned_apps >= 0),
            considered_apps INTEGER NOT NULL DEFAULT 0 CHECK (considered_apps >= 0),
            added_games INTEGER NOT NULL DEFAULT 0 CHECK (added_games >= 0),
            rejected_games INTEGER NOT NULL DEFAULT 0 CHECK (rejected_games >= 0),
            skipped_existing INTEGER NOT NULL DEFAULT 0 CHECK (skipped_existing >= 0),
            skipped_rejected_cache INTEGER NOT NULL DEFAULT 0 CHECK (skipped_rejected_cache >= 0),
            failed_games INTEGER NOT NULL DEFAULT 0 CHECK (failed_games >= 0),
            current_appid INTEGER
                CHECK (current_appid IS NULL OR (current_appid >= 0 AND current_appid <= 4294967295)),
            last_appid INTEGER
                CHECK (last_appid IS NULL OR (last_appid >= 0 AND last_appid <= 4294967295)),
            consecutive_empty_pages INTEGER NOT NULL DEFAULT 0
                CHECK (consecutive_empty_pages >= 0 AND consecutive_empty_pages <= 4294967295),
            rule_version TEXT NOT NULL,
            started_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            finished_at TEXT,
            last_error TEXT
        );

        CREATE TABLE IF NOT EXISTS classic_reject_cache (
            appid INTEGER PRIMARY KEY NOT NULL CHECK (appid >= 0 AND appid <= 4294967295),
            reason_code TEXT NOT NULL,
            positive_review_pct REAL,
            total_reviews INTEGER,
            current_players INTEGER,
            release_state TEXT NOT NULL,
            release_date TEXT,
            checked_at TEXT NOT NULL,
            rule_version TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS ai_analysis_queue (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            appid INTEGER NOT NULL UNIQUE CHECK (appid >= 0 AND appid <= 4294967295),
            source TEXT NOT NULL DEFAULT 'new_release'
                CHECK (source IN ('new_release', 'classic')),
            attempt INTEGER NOT NULL CHECK (attempt >= 1 AND attempt <= 255),
            last_error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sync_queue (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            appid INTEGER NOT NULL UNIQUE CHECK (appid >= 0 AND appid <= 4294967295),
            mode TEXT NOT NULL CHECK (mode IN ('quick', 'full')),
            attempt INTEGER NOT NULL CHECK (attempt >= 1 AND attempt <= 255),
            last_error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS analysis_narrative_cache (
            cache_key TEXT PRIMARY KEY NOT NULL,
            appid INTEGER NOT NULL CHECK (appid >= 0 AND appid <= 4294967295),
            score_version TEXT NOT NULL,
            base_url TEXT NOT NULL,
            model TEXT NOT NULL,
            narrative_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        "#,
    )?;
    ensure_games_metadata_columns(conn)?;
    ensure_discovery_run_columns(conn)?;
    ensure_classic_discovery_run_columns(conn)?;
    ensure_ai_analysis_queue_columns(conn)?;

    validate_discovery_run_rows(conn)?;
    validate_discovery_failure_rows(conn)?;

    set_config_if_missing(conn, "llm_base_url", DEFAULT_LLM_BASE_URL)?;
    set_config_if_missing(conn, "llm_model", DEFAULT_LLM_MODEL)?;
    set_config_if_missing(
        conn,
        LLM_PROVIDER_CONFIG_KEY,
        llm_provider_config_value(DEFAULT_LLM_PROVIDER),
    )?;
    set_config_if_missing(conn, STEAM_API_KEY_VALIDATED_CONFIG_KEY, "false")?;
    set_config_if_missing(conn, LLM_CONFIG_VALIDATED_CONFIG_KEY, "false")?;
    set_config_if_missing(conn, ONBOARDING_COMPLETED_CONFIG_KEY, "false")?;
    set_config_if_missing(conn, ONBOARDING_CURRENT_STEP_CONFIG_KEY, "1")?;
    set_config_if_missing(
        conn,
        ONBOARDING_LLM_PROVIDER_DRAFT_CONFIG_KEY,
        llm_provider_config_value(DEFAULT_LLM_PROVIDER),
    )?;
    set_config_if_missing(conn, "country", "US")?;
    set_config_if_missing(conn, "language", "schinese")?;
    set_config_if_missing(
        conn,
        AI_BATCH_REFRESH_CONCURRENCY_CONFIG_KEY,
        &DEFAULT_AI_BATCH_REFRESH_CONCURRENCY.to_string(),
    )?;
    migrate_default_language_to_schinese(conn)?;
    perform_classic_v2_cleanup(conn)?;
    Ok(())
}

pub fn seed_default_games(conn: &Connection) -> Result<()> {
    for seed in default_seeds() {
        let exists: Option<u32> = conn
            .query_row(
                "SELECT appid FROM games WHERE appid = ?1",
                params![seed.appid],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            upsert_seed(conn, &seed)?;
        }
    }
    Ok(())
}

fn purge_legacy_bootstrap_seed_games(conn: &Connection) -> Result<()> {
    if get_config(conn, "last_sync_at")?.is_some() {
        return Ok(());
    }

    let mut existing_appids = list_game_appids(conn)?;
    if existing_appids.is_empty() {
        return Ok(());
    }

    let mut seed_appids = default_seeds()
        .into_iter()
        .map(|seed| seed.appid)
        .collect::<Vec<_>>();
    existing_appids.sort_unstable();
    seed_appids.sort_unstable();

    if existing_appids != seed_appids {
        return Ok(());
    }

    conn.execute("DELETE FROM games", [])?;
    conn.execute(
        "DELETE FROM app_config WHERE key IN (?1, ?2)",
        params!["last_sync_at", DISCOVERY_CURSOR_CONFIG_KEY],
    )?;

    Ok(())
}

pub fn load_dashboard(conn: &Connection) -> Result<DashboardPayload> {
    let sync_queue_summary = sync_queue_summary(conn)?;
    let backfill_pending_count = count_metadata_backfill_jobs(conn)?;
    let backfill_last_error = latest_metadata_backfill_error(conn)?;
    let latest_classic_run = load_latest_classic_discovery_run(conn)?;
    let ai_queue_pending_count = count_ai_analysis_queue_jobs(conn)?;
    let ai_queue_failed_items = list_ai_analysis_queue_failures(conn)?;
    let sync_pending_count = sync_queue_summary
        .as_ref()
        .map(|summary| summary.pending_count)
        .unwrap_or(0);
    let sync_mode = sync_queue_summary.as_ref().map(|summary| summary.mode);
    let sync_last_error = sync_queue_summary
        .as_ref()
        .and_then(|summary| summary.last_error.clone());
    let sync_last_error_appid = sync_queue_summary
        .as_ref()
        .and_then(|summary| summary.last_error_appid);
    let mut stmt = conn.prepare(
        r#"
        SELECT appid, name, short_description, section, release_date, release_date_text,
               release_state, demo_status, supported_languages_json,
               is_adult_content, is_free, price_text, discount_percent, positive_review_pct,
               total_reviews, current_players, recommendation_score, ai_score,
               ai_summary, capsule_url, store_screenshot_urls_json, tags_json,
               multiplayer_modes_json, review_snippets_json, COALESCE(created_at, updated_at)
        FROM games
        ORDER BY recommendation_score DESC, positive_review_pct DESC
        "#,
    )?;

    let mut rows = stmt.query([])?;
    let mut new_games = Vec::new();
    let mut classics = Vec::new();
    let mut hidden_games = Vec::new();
    let mut upcoming = Vec::new();
    let mut cards_with_created_at = Vec::new();

    while let Some(row) = rows.next()? {
        let release_state: StoreReleaseState = serde_json::from_str(&row.get::<_, String>(6)?)?;
        let demo_status: DemoStatus = serde_json::from_str(&row.get::<_, String>(7)?)?;
        let supported_languages: Vec<String> = serde_json::from_str(&row.get::<_, String>(8)?)?;
        let store_screenshot_urls: Vec<String> = serde_json::from_str(&row.get::<_, String>(20)?)?;
        let tags: Vec<String> = serde_json::from_str(&row.get::<_, String>(21)?)?;
        let multiplayer_modes: Vec<String> = serde_json::from_str(&row.get::<_, String>(22)?)?;
        let review_snippets: Vec<ReviewSnippet> = serde_json::from_str(&row.get::<_, String>(23)?)?;
        let release_date: Option<String> = row.get(4)?;

        let created_at: String = row.get(24)?;
        let card = GameCard {
            appid: row.get(0)?,
            name: row.get(1)?,
            short_description: row.get(2)?,
            section: row.get(3)?,
            release_date,
            release_date_text: row.get(5)?,
            release_state,
            demo_status,
            supported_languages,
            is_adult_content: row.get(9)?,
            is_free: row.get(10)?,
            price_text: row.get(11)?,
            discount_percent: row.get(12)?,
            positive_review_pct: row.get(13)?,
            total_reviews: row.get(14)?,
            current_players: row.get(15)?,
            recommendation_score: row.get(16)?,
            ai_score: row.get(17)?,
            ai_summary: row.get(18)?,
            capsule_url: row.get(19)?,
            store_screenshot_urls,
            tags,
            multiplayer_modes,
            review_snippets,
            user_state: load_user_state(conn, row.get(0)?)?,
        };

        cards_with_created_at.push((created_at, card.clone()));
        match card.release_state {
            StoreReleaseState::Upcoming | StoreReleaseState::Tba => upcoming.push(card),
            _ if card.section == "new" => new_games.push(card),
            _ if card.section == "classic_hidden" => hidden_games.push(card),
            _ => classics.push(card),
        }
    }

    let new_games_count = new_games.len();
    let classic_games_count = classics.len();
    let total_games = new_games_count + classic_games_count + hidden_games.len() + upcoming.len();
    let last_discovery_appid = parse_saved_cursor(get_config(conn, DISCOVERY_CURSOR_CONFIG_KEY)?);
    cards_with_created_at.sort_by(|a, b| b.0.cmp(&a.0));
    let recent_discoveries = cards_with_created_at
        .into_iter()
        .map(|(_, card)| card)
        .collect();
    let collections = collections_from_games(load_all_games(conn)?);

    Ok(DashboardPayload {
        new_games,
        classics,
        hidden_games,
        upcoming,
        recent_discoveries,
        collections,
            ai_analysis_queue_failures: ai_queue_failed_items.clone(),
        stats: DashboardStats {
            last_sync_at: get_config(conn, "last_sync_at")?,
            seed_count: total_games,
            total_games,
            new_games_count,
            classic_games_count,
            last_discovery_appid,
            classic_discovery_running: false,
            classic_discovery_status: latest_classic_run.as_ref().map(|run| run.status.clone()),
            classic_discovery_current_appid: latest_classic_run
                .as_ref()
                .and_then(|run| run.current_appid),
            classic_discovery_last_appid: latest_classic_run
                .as_ref()
                .and_then(|run| run.last_appid)
                .or(parse_saved_cursor(get_config(
                    conn,
                    CLASSIC_DISCOVERY_LAST_APPID_CONFIG_KEY,
                )?)),
            classic_discovery_scanned_apps: latest_classic_run
                .as_ref()
                .map(|run| run.scanned_apps)
                .unwrap_or(0),
            classic_discovery_added_games: latest_classic_run
                .as_ref()
                .map(|run| run.added_games)
                .unwrap_or(0),
            classic_discovery_rejected_games: latest_classic_run
                .as_ref()
                .map(|run| run.rejected_games)
                .unwrap_or(0),
            classic_discovery_failed_games: latest_classic_run
                .as_ref()
                .map(|run| run.failed_games)
                .unwrap_or(0),
            classic_discovery_skipped_existing: latest_classic_run
                .as_ref()
                .map(|run| run.skipped_existing)
                .unwrap_or(0),
            classic_discovery_skipped_rejected_cache: latest_classic_run
                .as_ref()
                .map(|run| run.skipped_rejected_cache)
                .unwrap_or(0),
            classic_discovery_last_completed_at: get_config(
                conn,
                CLASSIC_DISCOVERY_LAST_COMPLETED_AT_CONFIG_KEY,
            )?,
            sync_running: false,
            sync_mode,
            sync_pending_count,
            sync_current_appid: None,
            sync_total_count: sync_pending_count,
            sync_processed_count: 0,
            sync_updated_count: 0,
            sync_failed_count: 0,
            sync_last_error,
            sync_last_error_appid,
            backfill_pending_count,
            backfill_running: false,
            backfill_current_appid: None,
            backfill_current_attempt: None,
            backfill_total_count: 0,
            backfill_processed_count: 0,
            backfill_failed_count: 0,
            backfill_max_attempts: BACKFILL_MAX_ATTEMPTS,
            backfill_last_error: backfill_last_error
                .as_ref()
                .map(|record| record.error.clone()),
            backfill_last_error_appid: backfill_last_error.map(|record| record.appid),
            ai_batch_refresh_running: false,
            ai_batch_refresh_concurrency: 0,
            ai_batch_refresh_pending_count: 0,
            ai_batch_refresh_active_count: 0,
            ai_batch_refresh_total_count: ai_queue_pending_count,
            ai_batch_refresh_processed_count: 0,
            ai_batch_refresh_updated_count: 0,
            ai_batch_refresh_failed_count: ai_queue_failed_items.len(),
            ai_batch_refresh_failed_pending_review_count: ai_queue_failed_items.len(),
            ai_batch_refresh_last_error: ai_queue_failed_items
                .first()
                .map(|item| item.last_error.clone()),
            ai_batch_refresh_last_error_appid: ai_queue_failed_items.first().map(|item| item.appid),
            data_source: if get_secret(conn, "steam_api_key")?.is_some() {
                if total_games == 0 {
                    "Steam API Key 已配置；当前库为空，可开始导入和发现多人游戏。".to_string()
                } else {
                    format!(
                        "Steam API Key 已配置；当前库共 {total_games} 个多人游戏，可继续同步和发现。"
                    )
                }
            } else {
                if total_games == 0 {
                    "当前库为空；请先配置 Steam API Key 后导入多人游戏。".to_string()
                } else {
                    format!(
                        "Steam API Key 未配置；当前库共 {total_games} 个多人游戏，配置后可继续同步和发现。"
                    )
                }
            },
        },
        config: public_config(conn)?,
    })
}

pub fn upsert_game(conn: &Connection, card: &GameCard) -> Result<()> {
    let now = now_rfc3339()?;
    conn.execute(
        r#"
        INSERT INTO games (
            appid, name, short_description, section, release_date, release_date_text,
            release_state, demo_status, supported_languages_json, is_adult_content,
            is_free, price_text, discount_percent, positive_review_pct, total_reviews,
            current_players, recommendation_score, ai_score, ai_summary, capsule_url,
            store_screenshot_urls_json, tags_json, multiplayer_modes_json,
            review_snippets_json, created_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)
        ON CONFLICT(appid) DO UPDATE SET
            name=excluded.name,
            short_description=excluded.short_description,
            section=excluded.section,
            release_date=excluded.release_date,
            release_date_text=excluded.release_date_text,
            release_state=excluded.release_state,
            demo_status=excluded.demo_status,
            supported_languages_json=excluded.supported_languages_json,
            is_adult_content=excluded.is_adult_content,
            is_free=excluded.is_free,
            price_text=excluded.price_text,
            discount_percent=excluded.discount_percent,
            positive_review_pct=excluded.positive_review_pct,
            total_reviews=excluded.total_reviews,
            current_players=excluded.current_players,
            recommendation_score=excluded.recommendation_score,
            ai_score=excluded.ai_score,
            ai_summary=excluded.ai_summary,
            capsule_url=excluded.capsule_url,
            store_screenshot_urls_json=excluded.store_screenshot_urls_json,
            tags_json=excluded.tags_json,
            multiplayer_modes_json=excluded.multiplayer_modes_json,
            review_snippets_json=excluded.review_snippets_json,
            updated_at=excluded.updated_at
        "#,
        params![
            card.appid,
            card.name,
            card.short_description,
            card.section,
            card.release_date,
            card.release_date_text,
            serde_json::to_string(&card.release_state)?,
            serde_json::to_string(&card.demo_status)?,
            serde_json::to_string(&card.supported_languages)?,
            card.is_adult_content,
            card.is_free,
            card.price_text,
            card.discount_percent,
            card.positive_review_pct,
            card.total_reviews,
            card.current_players,
            card.recommendation_score,
            card.ai_score,
            card.ai_summary,
            card.capsule_url,
            serde_json::to_string(&card.store_screenshot_urls)?,
            serde_json::to_string(&card.tags)?,
            serde_json::to_string(&card.multiplayer_modes)?,
            serde_json::to_string(&card.review_snippets)?,
            now.clone(),
            now,
        ],
    )?;
    Ok(())
}

pub fn set_game_user_state(
    conn: &Connection,
    appid: u32,
    patch: UserGameStatePatch,
) -> Result<UserGameState> {
    let now = now_rfc3339()?;
    let existing = load_user_state(conn, appid)?;
    let favorite = patch.favorite.unwrap_or(existing.favorite);
    let wishlist = patch.wishlist.unwrap_or(existing.wishlist);
    let followed = patch.followed.unwrap_or(existing.followed);
    let viewed = patch.viewed.unwrap_or(existing.viewed);

    conn.execute(
        r#"
        INSERT INTO user_game_state (appid, favorite, wishlist, followed, viewed, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(appid) DO UPDATE SET
            favorite=excluded.favorite,
            wishlist=excluded.wishlist,
            followed=excluded.followed,
            viewed=excluded.viewed,
            updated_at=excluded.updated_at
        "#,
        params![appid, favorite, wishlist, followed, viewed, now],
    )?;

    load_user_state(conn, appid)
}

pub fn load_user_state(conn: &Connection, appid: u32) -> Result<UserGameState> {
    Ok(conn
        .query_row(
            r#"
            SELECT favorite, wishlist, followed, viewed, updated_at
            FROM user_game_state
            WHERE appid = ?1
            "#,
            params![appid],
            |row| {
                Ok(UserGameState {
                    favorite: row.get::<_, bool>(0)?,
                    wishlist: row.get::<_, bool>(1)?,
                    followed: row.get::<_, bool>(2)?,
                    viewed: row.get::<_, bool>(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .optional()?
        .unwrap_or_default())
}

pub fn load_user_collections(conn: &Connection) -> Result<UserCollections> {
    Ok(load_dashboard(conn)?.collections)
}

pub fn load_max_game_appid(conn: &Connection) -> Result<Option<u32>> {
    conn.query_row("SELECT MAX(appid) FROM games", [], |row| row.get(0))
        .optional()
        .map(|value| value.flatten())
        .map_err(Into::into)
}

fn collections_from_games(games: Vec<GameCard>) -> UserCollections {
    UserCollections {
        favorites: games
            .iter()
            .filter(|game| game.user_state.favorite)
            .cloned()
            .collect(),
        wishlist: games
            .iter()
            .filter(|game| game.user_state.wishlist)
            .cloned()
            .collect(),
        followed: games
            .iter()
            .filter(|game| game.user_state.followed)
            .cloned()
            .collect(),
        history: games
            .into_iter()
            .filter(|game| game.user_state.viewed)
            .collect(),
    }
}

pub fn load_game(conn: &Connection, appid: u32) -> Result<Option<GameCard>> {
    Ok(load_all_games(conn)?
        .into_iter()
        .find(|game| game.appid == appid))
}

pub fn list_game_appids_with_sections(conn: &Connection) -> Result<Vec<(u32, String)>> {
    let mut stmt = conn.prepare("SELECT appid, section FROM games ORDER BY appid ASC")?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?)))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn load_game_analysis(conn: &Connection, appid: u32) -> Result<Option<GameAnalysisReport>> {
    let report_json = conn
        .query_row(
            "SELECT ai_analysis_report_json FROM games WHERE appid = ?1",
            params![appid],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();

    match report_json {
        Some(report_json) => {
            let mut report: GameAnalysisReport = serde_json::from_str(&report_json)?;
            if report.score_version.trim().is_empty() || report.score_version == "v1_compat" {
                report.score_version = "v1_compat".to_string();
                if report.quality_score <= 0.0 {
                    report.quality_score = report.overall_score;
                }
                if report.recommendation_score <= 0.0 {
                    report.recommendation_score = report.overall_score;
                }
                if report.confidence_score <= 0.0 {
                    report.confidence_score = match report.confidence.clone() {
                        crate::models::AnalysisConfidence::High => 0.8,
                        crate::models::AnalysisConfidence::Medium => 0.55,
                        crate::models::AnalysisConfidence::Low => 0.3,
                    };
                }
            }
            Ok(Some(report))
        }
        None => Ok(None),
    }
}

pub fn save_game_analysis(
    conn: &Connection,
    appid: u32,
    report: &GameAnalysisReport,
) -> Result<()> {
    ensure!(
        appid == report.appid,
        "report appid does not match target appid: target={appid}, report={}",
        report.appid
    );

    let updated_rows = conn.execute(
        r#"
        UPDATE games
        SET ai_analysis_report_json = ?2,
            ai_analysis_generated_at = ?3
        WHERE appid = ?1
        "#,
        params![appid, serde_json::to_string(report)?, report.generated_at],
    )?;

    ensure!(updated_rows == 1, "no game row found for appid {appid}");
    Ok(())
}

pub fn list_game_appids(conn: &Connection) -> Result<Vec<u32>> {
    let mut stmt =
        conn.prepare("SELECT appid FROM games ORDER BY section DESC, recommendation_score DESC")?;
    let rows = stmt.query_map([], |row| row.get::<_, u32>(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn enqueue_metadata_backfill(
    conn: &Connection,
    appids: impl IntoIterator<Item = u32>,
) -> Result<usize> {
    Ok(enqueue_metadata_backfill_jobs(conn, appids)?.len())
}

pub fn replace_sync_jobs(
    conn: &Connection,
    appids: impl IntoIterator<Item = u32>,
    mode: SyncMode,
) -> Result<usize> {
    let now = now_rfc3339()?;
    let mut seen = HashSet::new();
    let mut inserted = 0usize;

    conn.execute("DELETE FROM sync_queue", [])?;

    for appid in appids {
        if !seen.insert(appid) {
            continue;
        }

        conn.execute(
            r#"
            INSERT INTO sync_queue (
                appid, mode, attempt, last_error, created_at, updated_at
            )
            VALUES (?1, ?2, 1, NULL, ?3, ?3)
            "#,
            params![appid, sync_mode_as_str(mode), now],
        )?;
        inserted += 1;
    }

    Ok(inserted)
}

pub fn list_sync_jobs(conn: &Connection) -> Result<Vec<SyncJobRecord>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT appid, mode, attempt
        FROM sync_queue
        ORDER BY id ASC
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        let attempt = row.get::<_, i64>(2)?;
        let attempt = i64_to_u8(attempt).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?;
        let mode = sync_mode_from_str(&row.get::<_, String>(1)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?;
        Ok(SyncJobRecord {
            appid: row.get(0)?,
            mode,
            attempt,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn update_sync_job(
    conn: &Connection,
    appid: u32,
    mode: SyncMode,
    attempt: u8,
    last_error: Option<&str>,
) -> Result<()> {
    conn.execute(
        r#"
        UPDATE sync_queue
        SET mode = ?2, attempt = ?3, last_error = ?4, updated_at = ?5
        WHERE appid = ?1
        "#,
        params![
            appid,
            sync_mode_as_str(mode),
            i64::from(attempt),
            last_error,
            now_rfc3339()?
        ],
    )?;
    Ok(())
}

pub fn delete_sync_job(conn: &Connection, appid: u32) -> Result<()> {
    conn.execute("DELETE FROM sync_queue WHERE appid = ?1", params![appid])?;
    Ok(())
}

pub fn count_sync_jobs(conn: &Connection) -> Result<usize> {
    let count = conn.query_row("SELECT COUNT(*) FROM sync_queue", [], |row| {
        row.get::<_, i64>(0)
    })?;
    i64_to_usize(count)
}

pub fn update_all_sync_job_modes(conn: &Connection, mode: SyncMode) -> Result<()> {
    conn.execute(
        r#"
        UPDATE sync_queue
        SET mode = ?1, updated_at = ?2
        "#,
        params![sync_mode_as_str(mode), now_rfc3339()?],
    )?;
    Ok(())
}

pub fn sync_queue_summary(conn: &Connection) -> Result<Option<SyncQueueSummary>> {
    let (pending_count, mode_value) = conn.query_row(
        r#"
        SELECT
            COUNT(*),
            CASE
                WHEN SUM(CASE WHEN mode = 'full' THEN 1 ELSE 0 END) > 0 THEN 'full'
                ELSE 'quick'
            END
        FROM sync_queue
        "#,
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    )?;
    let pending_count = i64_to_usize(pending_count)?;
    if pending_count == 0 {
        return Ok(None);
    }

    let mode = sync_mode_from_str(&mode_value)?;
    let latest_error = conn
        .query_row(
            r#"
            SELECT appid, last_error
            FROM sync_queue
            WHERE last_error IS NOT NULL
            ORDER BY updated_at DESC, id DESC
            LIMIT 1
            "#,
            [],
            |row| Ok((Some(row.get::<_, u32>(0)?), Some(row.get::<_, String>(1)?))),
        )
        .optional()?
        .unwrap_or((None, None));

    Ok(Some(SyncQueueSummary {
        pending_count,
        mode,
        last_error_appid: latest_error.0,
        last_error: latest_error.1,
    }))
}

pub fn enqueue_metadata_backfill_jobs(
    conn: &Connection,
    appids: impl IntoIterator<Item = u32>,
) -> Result<Vec<MetadataBackfillJobRecord>> {
    let now = now_rfc3339()?;
    let mut inserted = Vec::new();

    for appid in appids {
        let changed = conn.execute(
            r#"
            INSERT OR IGNORE INTO metadata_backfill_queue (
                appid, attempt, last_error, created_at, updated_at
            )
            VALUES (?1, 1, NULL, ?2, ?2)
            "#,
            params![appid, now],
        )?;
        if changed > 0 {
            inserted.push(MetadataBackfillJobRecord { appid, attempt: 1 });
        }
    }

    Ok(inserted)
}

pub fn list_metadata_backfill_jobs(conn: &Connection) -> Result<Vec<MetadataBackfillJobRecord>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT appid, attempt
        FROM metadata_backfill_queue
        ORDER BY id ASC
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        let attempt = row.get::<_, i64>(1)?;
        let attempt = i64_to_u8(attempt).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?;
        Ok(MetadataBackfillJobRecord {
            appid: row.get(0)?,
            attempt,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn update_metadata_backfill_attempt(
    conn: &Connection,
    appid: u32,
    attempt: u8,
    last_error: Option<&str>,
) -> Result<()> {
    conn.execute(
        r#"
        UPDATE metadata_backfill_queue
        SET attempt = ?2, last_error = ?3, updated_at = ?4
        WHERE appid = ?1
        "#,
        params![appid, i64::from(attempt), last_error, now_rfc3339()?],
    )?;
    Ok(())
}

pub fn delete_metadata_backfill_job(conn: &Connection, appid: u32) -> Result<()> {
    conn.execute(
        "DELETE FROM metadata_backfill_queue WHERE appid = ?1",
        params![appid],
    )?;
    Ok(())
}

pub fn count_metadata_backfill_jobs(conn: &Connection) -> Result<usize> {
    let count = conn.query_row("SELECT COUNT(*) FROM metadata_backfill_queue", [], |row| {
        row.get::<_, i64>(0)
    })?;
    i64_to_usize(count)
}

pub fn latest_metadata_backfill_error(
    conn: &Connection,
) -> Result<Option<MetadataBackfillErrorRecord>> {
    conn.query_row(
        r#"
        SELECT appid, last_error
        FROM metadata_backfill_queue
        WHERE last_error IS NOT NULL
        ORDER BY updated_at DESC, id DESC
        LIMIT 1
        "#,
        [],
        |row| {
            Ok(MetadataBackfillErrorRecord {
                appid: row.get(0)?,
                error: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn create_classic_discovery_run(
    conn: &Connection,
    max_pages: u32,
    start_offset: u32,
) -> Result<ClassicDiscoveryRunSnapshot> {
    let now = now_rfc3339()?;
    conn.execute(
        r#"
        INSERT INTO classic_discovery_runs (
            status, max_pages, page_size, pages_processed, current_appid, last_appid, consecutive_empty_pages,
            rule_version, started_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, NULL, NULL, 0, ?5, ?6, ?6)
        "#,
        params![
            discovery_run_status_as_str(&DiscoveryRunStatus::Running),
            max_pages,
            CLASSIC_DISCOVERY_PAGE_SIZE,
            start_offset,
            CLASSIC_DISCOVERY_RULE_VERSION,
            now,
        ],
    )?;
    let run_id = conn.last_insert_rowid();
    set_config(conn, CLASSIC_DISCOVERY_LAST_STARTED_AT_CONFIG_KEY, &now)?;
    set_config(
        conn,
        CLASSIC_DISCOVERY_LAST_RUN_STATUS_CONFIG_KEY,
        discovery_run_status_as_str(&DiscoveryRunStatus::Running),
    )?;
    load_classic_discovery_run(conn, run_id)?.context("classic discovery run was just created")
}

pub fn update_classic_discovery_run_progress(
    conn: &Connection,
    run_id: i64,
    patch: ClassicDiscoveryProgressPatch,
) -> Result<()> {
    let existing = load_classic_discovery_run(conn, run_id)?
        .with_context(|| format!("classic discovery run {run_id} was not found"))?;
    let now = now_rfc3339()?;
    let status = patch.status.unwrap_or(existing.status);
    let pages_processed = patch.pages_processed.unwrap_or(existing.pages_processed);
    let scanned_apps = patch.scanned_apps.unwrap_or(existing.scanned_apps);
    let considered_apps = patch.considered_apps.unwrap_or(existing.considered_apps);
    let added_games = patch.added_games.unwrap_or(existing.added_games);
    let rejected_games = patch.rejected_games.unwrap_or(existing.rejected_games);
    let skipped_existing = patch.skipped_existing.unwrap_or(existing.skipped_existing);
    let skipped_rejected_cache = patch
        .skipped_rejected_cache
        .unwrap_or(existing.skipped_rejected_cache);
    let failed_games = patch.failed_games.unwrap_or(existing.failed_games);
    let current_appid = merge_nullable_patch(patch.current_appid, existing.current_appid);
    let last_appid = merge_nullable_patch(patch.last_appid, existing.last_appid);
    let consecutive_empty_pages = patch
        .consecutive_empty_pages
        .unwrap_or(existing.consecutive_empty_pages);
    let last_error = merge_nullable_patch(patch.last_error, existing.last_error);
    let finished_at = patch.finished_at.unwrap_or(existing.finished_at);

    conn.execute(
        r#"
        UPDATE classic_discovery_runs
        SET status = ?2,
            pages_processed = ?3,
            scanned_apps = ?4,
            considered_apps = ?5,
            added_games = ?6,
            rejected_games = ?7,
            skipped_existing = ?8,
            skipped_rejected_cache = ?9,
            failed_games = ?10,
            current_appid = ?11,
            last_appid = ?12,
            consecutive_empty_pages = ?13,
            updated_at = ?14,
            finished_at = ?15,
            last_error = ?16
        WHERE id = ?1
        "#,
        params![
            run_id,
            discovery_run_status_as_str(&status),
            pages_processed,
            usize_to_i64(scanned_apps)?,
            usize_to_i64(considered_apps)?,
            usize_to_i64(added_games)?,
            usize_to_i64(rejected_games)?,
            usize_to_i64(skipped_existing)?,
            usize_to_i64(skipped_rejected_cache)?,
            usize_to_i64(failed_games)?,
            current_appid,
            last_appid,
            consecutive_empty_pages,
            now,
            finished_at,
            last_error,
        ],
    )?;

    set_config(
        conn,
        CLASSIC_DISCOVERY_LAST_RUN_STATUS_CONFIG_KEY,
        discovery_run_status_as_str(&status),
    )?;
    if let Some(last_appid) = last_appid {
        set_config(
            conn,
            CLASSIC_DISCOVERY_LAST_APPID_CONFIG_KEY,
            &last_appid.to_string(),
        )?;
    }
    set_config(
        conn,
        CLASSIC_DISCOVERY_LAST_OFFSET_CONFIG_KEY,
        &pages_processed.to_string(),
    )?;
    if matches!(status, DiscoveryRunStatus::Completed) {
        set_config(conn, CLASSIC_DISCOVERY_LAST_COMPLETED_AT_CONFIG_KEY, &now)?;
    }

    Ok(())
}

pub fn load_classic_discovery_run(
    conn: &Connection,
    run_id: i64,
) -> Result<Option<ClassicDiscoveryRunSnapshot>> {
    conn.query_row(
        r#"
        SELECT id, status, max_pages, page_size, pages_processed, scanned_apps, considered_apps, added_games,
               rejected_games, skipped_existing, skipped_rejected_cache, failed_games,
               current_appid, last_appid, consecutive_empty_pages, rule_version, started_at, updated_at,
               finished_at, last_error
        FROM classic_discovery_runs
        WHERE id = ?1
        "#,
        params![run_id],
        map_classic_discovery_run_snapshot,
    )
    .optional()
    .map_err(Into::into)
}

pub fn load_latest_classic_discovery_run(
    conn: &Connection,
) -> Result<Option<ClassicDiscoveryRunSnapshot>> {
    let run_id = conn
        .query_row(
            "SELECT id FROM classic_discovery_runs ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;

    run_id
        .map(|run_id| load_classic_discovery_run(conn, run_id))
        .transpose()
        .map(|snapshot| snapshot.flatten())
}

pub fn list_classic_discovery_runs(conn: &Connection) -> Result<Vec<ClassicDiscoveryRunSnapshot>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, status, max_pages, page_size, pages_processed, scanned_apps, considered_apps, added_games,
               rejected_games, skipped_existing, skipped_rejected_cache, failed_games,
               current_appid, last_appid, consecutive_empty_pages, rule_version, started_at, updated_at,
               finished_at, last_error
        FROM classic_discovery_runs
        ORDER BY id DESC
        "#,
    )?;
    let rows = stmt.query_map([], map_classic_discovery_run_snapshot)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn mark_running_classic_discovery_runs_interrupted(conn: &Connection) -> Result<()> {
    conn.execute(
        r#"
        UPDATE classic_discovery_runs
        SET status = ?1, updated_at = ?2
        WHERE status = ?3
        "#,
        params![
            discovery_run_status_as_str(&DiscoveryRunStatus::Interrupted),
            now_rfc3339()?,
            discovery_run_status_as_str(&DiscoveryRunStatus::Running),
        ],
    )?;
    Ok(())
}

pub fn classic_discovery_is_due(conn: &Connection, now: &str) -> Result<bool> {
    let status = get_config(conn, CLASSIC_DISCOVERY_LAST_RUN_STATUS_CONFIG_KEY)?;
    let reference_time = match status.as_deref() {
        Some("cancelled" | "failed") => {
            get_config(conn, CLASSIC_DISCOVERY_LAST_STARTED_AT_CONFIG_KEY)?
        }
        _ => get_config(conn, CLASSIC_DISCOVERY_LAST_COMPLETED_AT_CONFIG_KEY)?,
    };
    let Some(reference_time) = reference_time else {
        return Ok(true);
    };
    let last_completed_at = parse_rfc3339_utc(&reference_time).ok();
    let now = parse_rfc3339_utc(now).ok();
    let Some((last_completed_at, now)) = last_completed_at.zip(now) else {
        return Ok(true);
    };
    Ok((now - last_completed_at).whole_hours() >= CLASSIC_DISCOVERY_COOLDOWN_HOURS)
}

pub fn save_classic_reject_cache_entry(
    conn: &Connection,
    entry: &ClassicDiscoveryRejectCacheEntry,
) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO classic_reject_cache (
            appid, reason_code, positive_review_pct, total_reviews,
            current_players, release_state, release_date, checked_at, rule_version
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT(appid) DO UPDATE SET
            reason_code = excluded.reason_code,
            positive_review_pct = excluded.positive_review_pct,
            total_reviews = excluded.total_reviews,
            current_players = excluded.current_players,
            release_state = excluded.release_state,
            release_date = excluded.release_date,
            checked_at = excluded.checked_at,
            rule_version = excluded.rule_version
        "#,
        params![
            entry.appid,
            classic_reject_reason_code_as_str(&entry.reason_code),
            entry.positive_review_pct,
            entry.total_reviews,
            entry.current_players,
            serde_json::to_string(&entry.release_state)?,
            entry.release_date,
            entry.checked_at,
            entry.rule_version,
        ],
    )?;
    Ok(())
}

pub fn load_classic_reject_cache_entry(
    conn: &Connection,
    appid: u32,
) -> Result<Option<ClassicDiscoveryRejectCacheEntry>> {
    conn.query_row(
        r#"
        SELECT appid, reason_code, positive_review_pct, total_reviews, current_players,
               release_state, release_date, checked_at, rule_version
        FROM classic_reject_cache
        WHERE appid = ?1
        "#,
        params![appid],
        |row| {
            let reason_code_value: String = row.get(1)?;
            let reason_code =
                classic_reject_reason_code_from_str(&reason_code_value).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
                    )
                })?;
            let release_state_raw: String = row.get(5)?;
            let release_state = serde_json::from_str(&release_state_raw).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            Ok(ClassicDiscoveryRejectCacheEntry {
                appid: row.get(0)?,
                reason_code,
                positive_review_pct: row.get(2)?,
                total_reviews: row.get(3)?,
                current_players: row.get(4)?,
                release_state,
                release_date: row.get(6)?,
                checked_at: row.get(7)?,
                rule_version: row.get(8)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn classic_reject_cache_allows_retry(conn: &Connection, appid: u32, now: &str) -> Result<bool> {
    let Some(entry) = load_classic_reject_cache_entry(conn, appid)? else {
        return Ok(true);
    };
    if entry.rule_version != CLASSIC_DISCOVERY_RULE_VERSION {
        return Ok(true);
    }
    let checked_at = parse_rfc3339_utc(&entry.checked_at).ok();
    let now = parse_rfc3339_utc(now).ok();
    let Some((checked_at, now)) = checked_at.zip(now) else {
        return Ok(true);
    };
    Ok((now - checked_at).whole_days() >= CLASSIC_DISCOVERY_REJECT_CACHE_TTL_DAYS)
}

pub fn enqueue_ai_analysis_jobs(
    conn: &Connection,
    source: AiAnalysisQueueSource,
    appids: impl IntoIterator<Item = u32>,
) -> Result<Vec<AiAnalysisQueueJobRecord>> {
    let now = now_rfc3339()?;
    let mut inserted = Vec::new();
    for appid in appids {
        let changed = conn.execute(
            r#"
            INSERT OR IGNORE INTO ai_analysis_queue (
                appid, source, attempt, last_error, created_at, updated_at
            )
            VALUES (?1, ?2, 1, NULL, ?3, ?3)
            "#,
            params![appid, ai_analysis_queue_source_as_str(source), now],
        )?;
        if changed > 0 {
            inserted.push(AiAnalysisQueueJobRecord {
                appid,
                source,
                attempt: 1,
                last_error: None,
                updated_at: now.clone(),
            });
        }
    }
    Ok(inserted)
}

pub fn list_ai_analysis_queue_jobs(conn: &Connection) -> Result<Vec<AiAnalysisQueueJobRecord>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT appid, source, attempt, last_error, updated_at
        FROM ai_analysis_queue
        ORDER BY
            CASE source WHEN 'new_release' THEN 0 ELSE 1 END ASC,
            CASE WHEN attempt >= ?1 AND last_error IS NOT NULL THEN 1 ELSE 0 END ASC,
            id ASC
        "#,
    )?;
    let rows = stmt.query_map(params![i64::from(AI_ANALYSIS_QUEUE_MAX_ATTEMPTS)], |row| {
        let attempt_raw = row.get::<_, i64>(2)?;
        let attempt = i64_to_u8(attempt_raw).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?;
        Ok(AiAnalysisQueueJobRecord {
            appid: row.get(0)?,
            source: ai_analysis_queue_source_from_str(row.get_ref(1)?.as_str()?).map_err(
                |err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
                    )
                },
            )?,
            attempt,
            last_error: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn list_ai_analysis_queue_ready_jobs(
    conn: &Connection,
) -> Result<Vec<AiAnalysisQueueJobRecord>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT appid, source, attempt, last_error, updated_at
        FROM ai_analysis_queue
        WHERE attempt <= ?1
        ORDER BY
            CASE source WHEN 'new_release' THEN 0 ELSE 1 END ASC,
            id ASC
        "#,
    )?;
    let rows = stmt.query_map(params![i64::from(AI_ANALYSIS_QUEUE_MAX_ATTEMPTS)], |row| {
        let attempt_raw = row.get::<_, i64>(2)?;
        let attempt = i64_to_u8(attempt_raw).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?;
        Ok(AiAnalysisQueueJobRecord {
            appid: row.get(0)?,
            source: ai_analysis_queue_source_from_str(row.get_ref(1)?.as_str()?).map_err(
                |err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
                    )
                },
            )?,
            attempt,
            last_error: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn list_ai_analysis_queue_ready_jobs_by_source(
    conn: &Connection,
    source: AiAnalysisQueueSource,
) -> Result<Vec<AiAnalysisQueueJobRecord>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT appid, source, attempt, last_error, updated_at
        FROM ai_analysis_queue
        WHERE attempt <= ?1 AND source = ?2
        ORDER BY id ASC
        "#,
    )?;
    let rows = stmt.query_map(
        params![
            i64::from(AI_ANALYSIS_QUEUE_MAX_ATTEMPTS),
            ai_analysis_queue_source_as_str(source)
        ],
        |row| {
            let attempt_raw = row.get::<_, i64>(2)?;
            let attempt = i64_to_u8(attempt_raw).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Integer,
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
                )
            })?;
            Ok(AiAnalysisQueueJobRecord {
                appid: row.get(0)?,
                source: ai_analysis_queue_source_from_str(row.get_ref(1)?.as_str()?).map_err(
                    |err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
                        )
                    },
                )?,
                attempt,
                last_error: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn load_ai_analysis_queue_job(
    conn: &Connection,
    appid: u32,
) -> Result<Option<AiAnalysisQueueJobRecord>> {
    conn.query_row(
        r#"
        SELECT appid, source, attempt, last_error, updated_at
        FROM ai_analysis_queue
        WHERE appid = ?1
        "#,
        params![appid],
        |row| {
            let attempt_raw = row.get::<_, i64>(2)?;
            let attempt = i64_to_u8(attempt_raw).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Integer,
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
                )
            })?;
            Ok(AiAnalysisQueueJobRecord {
                appid: row.get(0)?,
                source: ai_analysis_queue_source_from_str(row.get_ref(1)?.as_str()?).map_err(
                    |err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
                        )
                    },
                )?,
                attempt,
                last_error: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn update_ai_analysis_queue_job(
    conn: &Connection,
    appid: u32,
    attempt: u8,
    last_error: Option<&str>,
) -> Result<()> {
    conn.execute(
        r#"
        UPDATE ai_analysis_queue
        SET attempt = ?2, last_error = ?3, updated_at = ?4
        WHERE appid = ?1
        "#,
        params![appid, i64::from(attempt), last_error, now_rfc3339()?],
    )?;
    Ok(())
}

pub fn delete_ai_analysis_queue_job(conn: &Connection, appid: u32) -> Result<()> {
    conn.execute(
        "DELETE FROM ai_analysis_queue WHERE appid = ?1",
        params![appid],
    )?;
    Ok(())
}

pub fn count_ai_analysis_queue_jobs(conn: &Connection) -> Result<usize> {
    let count = conn.query_row("SELECT COUNT(*) FROM ai_analysis_queue", [], |row| {
        row.get::<_, i64>(0)
    })?;
    i64_to_usize(count)
}

pub fn count_ai_analysis_queue_ready_jobs(conn: &Connection) -> Result<usize> {
    let count = conn.query_row(
        "SELECT COUNT(*) FROM ai_analysis_queue WHERE attempt <= ?1",
        params![i64::from(AI_ANALYSIS_QUEUE_MAX_ATTEMPTS)],
        |row| row.get::<_, i64>(0),
    )?;
    i64_to_usize(count)
}

pub fn count_ai_analysis_queue_ready_jobs_by_source(
    conn: &Connection,
    source: AiAnalysisQueueSource,
) -> Result<usize> {
    let count = conn.query_row(
        "SELECT COUNT(*) FROM ai_analysis_queue WHERE attempt <= ?1 AND source = ?2",
        params![
            i64::from(AI_ANALYSIS_QUEUE_MAX_ATTEMPTS),
            ai_analysis_queue_source_as_str(source)
        ],
        |row| row.get::<_, i64>(0),
    )?;
    i64_to_usize(count)
}

pub fn list_ai_analysis_queue_failures(
    conn: &Connection,
) -> Result<Vec<AiAnalysisQueueFailureItem>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT appid, attempt, last_error, updated_at
        FROM ai_analysis_queue
        WHERE attempt > ?1 AND last_error IS NOT NULL
        ORDER BY updated_at DESC, id DESC
        "#,
    )?;
    let rows = stmt.query_map(params![i64::from(AI_ANALYSIS_QUEUE_MAX_ATTEMPTS)], |row| {
        let attempt_raw = row.get::<_, i64>(1)?;
        let attempt = i64_to_u8(attempt_raw).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?;
        Ok(AiAnalysisQueueFailureItem {
            appid: row.get(0)?,
            attempt: attempt.saturating_sub(1).max(1),
            last_error: row.get(2)?,
            updated_at: row.get(3)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn public_config(conn: &Connection) -> Result<PublicConfig> {
    let llm_provider = load_llm_provider(conn)?;
    let llm_provider_draft = load_onboarding_llm_provider_draft(conn)?;
    Ok(PublicConfig {
        steam_api_key_configured: get_secret(conn, "steam_api_key")?.is_some(),
        steam_api_key_validated: get_bool_config(conn, STEAM_API_KEY_VALIDATED_CONFIG_KEY)?
            .unwrap_or(false),
        llm_api_key_configured: get_secret(conn, "llm_api_key")?.is_some(),
        llm_config_validated: get_bool_config(conn, LLM_CONFIG_VALIDATED_CONFIG_KEY)?
            .unwrap_or(false),
        llm_provider,
        llm_base_url: get_config(conn, "llm_base_url")?
            .unwrap_or_else(|| default_llm_base_url(llm_provider).to_string()),
        llm_model: get_config(conn, "llm_model")?
            .unwrap_or_else(|| default_llm_model(llm_provider).to_string()),
        country: get_config(conn, "country")?.unwrap_or_else(|| "US".to_string()),
        language: get_config(conn, "language")?.unwrap_or_else(|| "schinese".to_string()),
        ai_batch_refresh_concurrency: load_ai_batch_refresh_concurrency(conn)?,
        onboarding_completed: get_bool_config(conn, ONBOARDING_COMPLETED_CONFIG_KEY)?
            .unwrap_or(false),
        onboarding_current_step: get_config(conn, ONBOARDING_CURRENT_STEP_CONFIG_KEY)?
            .as_deref()
            .and_then(|value| value.trim().parse::<u8>().ok())
            .map(|value| value.clamp(1, 5))
            .unwrap_or(1),
        onboarding_llm_provider_draft: llm_provider_draft,
    })
}

pub fn load_ai_batch_refresh_concurrency(conn: &Connection) -> Result<u8> {
    Ok(get_config(conn, AI_BATCH_REFRESH_CONCURRENCY_CONFIG_KEY)?
        .as_deref()
        .and_then(|value| value.trim().parse::<u8>().ok())
        .map(clamp_ai_batch_refresh_concurrency)
        .unwrap_or(DEFAULT_AI_BATCH_REFRESH_CONCURRENCY))
}

pub fn clamp_ai_batch_refresh_concurrency(value: u8) -> u8 {
    value.clamp(
        MIN_AI_BATCH_REFRESH_CONCURRENCY,
        MAX_AI_BATCH_REFRESH_CONCURRENCY,
    )
}

pub fn set_config(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO app_config (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn delete_config(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM app_config WHERE key = ?1", params![key])?;
    Ok(())
}

pub fn set_config_if_missing(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO app_config (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

pub fn set_bool_config(conn: &Connection, key: &str, value: bool) -> Result<()> {
    set_config(conn, key, if value { "true" } else { "false" })
}

pub fn get_config(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM app_config WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn get_bool_config(conn: &Connection, key: &str) -> Result<Option<bool>> {
    Ok(get_config(conn, key)?.map(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    }))
}

pub fn get_secret(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(get_config(conn, key)?.filter(|value| !value.trim().is_empty()))
}

pub fn default_llm_base_url(provider: LlmProvider) -> &'static str {
    match provider {
        LlmProvider::Deepseek => DEFAULT_LLM_BASE_URL,
        LlmProvider::Openai => DEFAULT_OPENAI_BASE_URL,
        LlmProvider::Anthropic => DEFAULT_ANTHROPIC_BASE_URL,
        LlmProvider::Custom => DEFAULT_LLM_BASE_URL,
    }
}

pub fn default_llm_model(provider: LlmProvider) -> &'static str {
    match provider {
        LlmProvider::Deepseek => DEFAULT_LLM_MODEL,
        LlmProvider::Openai => DEFAULT_OPENAI_MODEL,
        LlmProvider::Anthropic => DEFAULT_ANTHROPIC_MODEL,
        LlmProvider::Custom => DEFAULT_LLM_MODEL,
    }
}

pub fn load_llm_provider(conn: &Connection) -> Result<LlmProvider> {
    Ok(get_config(conn, LLM_PROVIDER_CONFIG_KEY)?
        .as_deref()
        .and_then(parse_llm_provider_config_value)
        .unwrap_or(DEFAULT_LLM_PROVIDER))
}

pub fn load_onboarding_llm_provider_draft(conn: &Connection) -> Result<LlmProvider> {
    Ok(get_config(conn, ONBOARDING_LLM_PROVIDER_DRAFT_CONFIG_KEY)?
        .as_deref()
        .and_then(parse_llm_provider_config_value)
        .unwrap_or_else(|| load_llm_provider(conn).unwrap_or(DEFAULT_LLM_PROVIDER)))
}

pub fn llm_provider_config_value(provider: LlmProvider) -> &'static str {
    match provider {
        LlmProvider::Deepseek => "deepseek",
        LlmProvider::Openai => "openai",
        LlmProvider::Anthropic => "anthropic",
        LlmProvider::Custom => "custom",
    }
}

pub fn parse_llm_provider_config_value(value: &str) -> Option<LlmProvider> {
    match value.trim().to_ascii_lowercase().as_str() {
        "deepseek" => Some(LlmProvider::Deepseek),
        "openai" => Some(LlmProvider::Openai),
        "anthropic" => Some(LlmProvider::Anthropic),
        "custom" => Some(LlmProvider::Custom),
        _ => None,
    }
}

pub fn facts_from_card(card: &GameCard) -> GameFacts {
    GameFacts {
        appid: card.appid,
        name: card.name.clone(),
        release_date: card.release_date.clone(),
        positive_review_pct: card.positive_review_pct,
        total_reviews: card.total_reviews,
        current_players: card.current_players,
        multiplayer_modes: card.multiplayer_modes.clone(),
        demo_status: card.demo_status.clone(),
        ai_score: card.ai_score,
    }
}

pub fn load_all_games(conn: &Connection) -> Result<Vec<GameCard>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT appid, name, short_description, section, release_date, release_date_text,
               release_state, demo_status, supported_languages_json,
               is_adult_content, is_free, price_text, discount_percent, positive_review_pct,
               total_reviews, current_players, recommendation_score, ai_score,
               ai_summary, capsule_url, store_screenshot_urls_json, tags_json,
               multiplayer_modes_json, review_snippets_json
        FROM games
        ORDER BY recommendation_score DESC, positive_review_pct DESC
        "#,
    )?;
    let mut rows = stmt.query([])?;
    let mut games = Vec::new();

    while let Some(row) = rows.next()? {
        let appid: u32 = row.get(0)?;
        let release_state: StoreReleaseState = serde_json::from_str(&row.get::<_, String>(6)?)?;
        let demo_status: DemoStatus = serde_json::from_str(&row.get::<_, String>(7)?)?;
        let supported_languages: Vec<String> = serde_json::from_str(&row.get::<_, String>(8)?)?;
        let store_screenshot_urls: Vec<String> = serde_json::from_str(&row.get::<_, String>(20)?)?;
        let tags: Vec<String> = serde_json::from_str(&row.get::<_, String>(21)?)?;
        let multiplayer_modes: Vec<String> = serde_json::from_str(&row.get::<_, String>(22)?)?;
        let review_snippets: Vec<ReviewSnippet> = serde_json::from_str(&row.get::<_, String>(23)?)?;
        games.push(GameCard {
            appid,
            name: row.get(1)?,
            short_description: row.get(2)?,
            section: row.get(3)?,
            release_date: row.get(4)?,
            release_date_text: row.get(5)?,
            release_state,
            demo_status,
            supported_languages,
            is_adult_content: row.get(9)?,
            is_free: row.get(10)?,
            price_text: row.get(11)?,
            discount_percent: row.get(12)?,
            positive_review_pct: row.get(13)?,
            total_reviews: row.get(14)?,
            current_players: row.get(15)?,
            recommendation_score: row.get(16)?,
            ai_score: row.get(17)?,
            ai_summary: row.get(18)?,
            capsule_url: row.get(19)?,
            store_screenshot_urls,
            tags,
            multiplayer_modes,
            review_snippets,
            user_state: load_user_state(conn, appid)?,
        });
    }

    Ok(games)
}

pub fn score_card(card: &GameCard) -> f64 {
    compute_recommendation_score(&facts_from_card(card), &today_iso_utc())
}

pub fn mark_sync_complete(conn: &Connection) -> Result<()> {
    set_config(conn, "last_sync_at", &now_rfc3339()?)
}

pub fn create_discovery_run(
    conn: &Connection,
    request: &DiscoveryTaskRequest,
    start_appid: Option<u32>,
) -> Result<DiscoveryRunSnapshot> {
    let now = now_rfc3339()?;
    conn.execute(
        r#"
        INSERT INTO discovery_runs (
            status, sync_mode, completion_reason, target_added_games, page_size, current_appid,
            last_appid, have_more_results, started_at, updated_at
        )
        VALUES (?1, ?2, NULL, ?3, ?4, NULL, ?5, 1, ?6, ?6)
        "#,
        params![
            discovery_run_status_as_str(&DiscoveryRunStatus::Running),
            sync_mode_as_str(request.sync_mode),
            request.target_added_games,
            request.page_size,
            start_appid,
            now,
        ],
    )?;

    let run_id = conn.last_insert_rowid();
    load_discovery_run(conn, run_id)?.context("discovery run was just created")
}

pub fn update_discovery_run_progress(
    conn: &Connection,
    run_id: i64,
    patch: DiscoveryProgressPatch,
) -> Result<()> {
    let existing = load_discovery_run(conn, run_id)?
        .with_context(|| format!("discovery run {run_id} was not found"))?;
    let now = now_rfc3339()?;
    let status = patch.status.unwrap_or(existing.status);
    let completion_reason =
        merge_nullable_patch(patch.completion_reason, existing.completion_reason);
    let current_appid = merge_nullable_patch(patch.current_appid, existing.current_appid);
    let last_appid = merge_nullable_patch(patch.last_appid, existing.last_appid);
    let pages_processed = patch.pages_processed.unwrap_or(existing.pages_processed);
    let scanned_apps = patch.scanned_apps.unwrap_or(existing.scanned_apps);
    let added_games = patch.added_games.unwrap_or(existing.added_games);
    let added_new_games = patch.added_new_games.unwrap_or(existing.added_new_games);
    let added_classic_games = patch
        .added_classic_games
        .unwrap_or(existing.added_classic_games);
    let skipped_existing = patch.skipped_existing.unwrap_or(existing.skipped_existing);
    let skipped_non_multiplayer = patch
        .skipped_non_multiplayer
        .unwrap_or(existing.skipped_non_multiplayer);
    let failed_games = patch.failed_games.unwrap_or(existing.failed_games);
    let have_more_results = patch
        .have_more_results
        .unwrap_or(existing.have_more_results);
    let last_error = merge_nullable_patch(patch.last_error, existing.last_error);
    let finished_at = patch.finished_at.unwrap_or(existing.finished_at);

    conn.execute(
        r#"
        UPDATE discovery_runs
        SET status = ?2,
            completion_reason = ?3,
            current_appid = ?4,
            last_appid = ?5,
            pages_processed = ?6,
            scanned_apps = ?7,
            added_games = ?8,
            added_new_games = ?9,
            added_classic_games = ?10,
            skipped_existing = ?11,
            skipped_non_multiplayer = ?12,
            failed_games = ?13,
            have_more_results = ?14,
            updated_at = ?15,
            finished_at = ?16,
            last_error = ?17
        WHERE id = ?1
        "#,
        params![
            run_id,
            discovery_run_status_as_str(&status),
            completion_reason
                .as_ref()
                .map(discovery_completion_reason_as_str),
            current_appid,
            last_appid,
            pages_processed,
            usize_to_i64(scanned_apps)?,
            usize_to_i64(added_games)?,
            usize_to_i64(added_new_games)?,
            usize_to_i64(added_classic_games)?,
            usize_to_i64(skipped_existing)?,
            usize_to_i64(skipped_non_multiplayer)?,
            usize_to_i64(failed_games)?,
            have_more_results,
            now,
            finished_at,
            last_error,
        ],
    )?;
    Ok(())
}

pub fn append_discovery_failure(
    conn: &Connection,
    run_id: i64,
    page_index: u32,
    appid: Option<u32>,
    stage: &str,
    reason: &str,
) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO discovery_failures (run_id, page_index, appid, stage, reason, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        "#,
        params![run_id, page_index, appid, stage, reason, now_rfc3339()?],
    )?;
    Ok(())
}

pub fn load_discovery_run(conn: &Connection, run_id: i64) -> Result<Option<DiscoveryRunSnapshot>> {
    let snapshot = conn
        .query_row(
            r#"
            SELECT id, status, sync_mode, completion_reason, target_added_games, page_size,
                   pages_processed, scanned_apps, added_games, added_new_games,
                   added_classic_games, skipped_existing, skipped_non_multiplayer, failed_games,
                   current_appid, last_appid, have_more_results, started_at, updated_at,
                   finished_at, last_error
            FROM discovery_runs
            WHERE id = ?1
            "#,
            params![run_id],
            map_discovery_run_snapshot,
        )
        .optional()?;

    snapshot
        .map(|snapshot| attach_discovery_failures(conn, snapshot))
        .transpose()
}

pub fn load_latest_discovery_run(conn: &Connection) -> Result<Option<DiscoveryRunSnapshot>> {
    let run_id = conn
        .query_row(
            "SELECT id FROM discovery_runs ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;

    run_id
        .map(|run_id| load_discovery_run(conn, run_id))
        .transpose()
        .map(|snapshot| snapshot.flatten())
}

pub fn list_discovery_runs(conn: &Connection) -> Result<Vec<DiscoveryRunSnapshot>> {
    let mut stmt = conn.prepare("SELECT id FROM discovery_runs ORDER BY id DESC")?;
    let run_ids = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    run_ids
        .into_iter()
        .map(|run_id| {
            load_discovery_run(conn, run_id)?
                .with_context(|| format!("discovery run {run_id} disappeared during listing"))
        })
        .collect()
}

pub fn mark_running_discovery_runs_interrupted(conn: &Connection) -> Result<()> {
    conn.execute(
        r#"
        UPDATE discovery_runs
        SET status = ?1, completion_reason = ?2, updated_at = ?3
        WHERE status = ?4
        "#,
        params![
            discovery_run_status_as_str(&DiscoveryRunStatus::Interrupted),
            discovery_completion_reason_as_str(&DiscoveryCompletionReason::Interrupted),
            now_rfc3339()?,
            discovery_run_status_as_str(&DiscoveryRunStatus::Running),
        ],
    )?;
    Ok(())
}

fn upsert_seed(conn: &Connection, seed: &GameSeed) -> Result<()> {
    let card = GameCard {
        appid: seed.appid,
        name: seed.name.to_string(),
        short_description: None,
        section: seed.section.to_string(),
        release_date: Some(seed.release_date.to_string()),
        release_date_text: seed.release_date_text.to_string(),
        release_state: StoreReleaseState::Released,
        demo_status: seed.demo_status.clone(),
        supported_languages: Vec::new(),
        is_adult_content: false,
        is_free: false,
        price_text: None,
        discount_percent: None,
        positive_review_pct: Some(seed.positive_review_pct),
        total_reviews: Some(seed.total_reviews),
        current_players: Some(seed.current_players),
        recommendation_score: 0.0,
        ai_score: Some(seed.ai_score),
        ai_summary: seed.ai_summary.to_string(),
        capsule_url: steam_header_url(seed.appid),
        store_screenshot_urls: Vec::new(),
        tags: seed.tags.iter().map(|tag| tag.to_string()).collect(),
        multiplayer_modes: seed
            .multiplayer_modes
            .iter()
            .map(|mode| mode.to_string())
            .collect(),
        review_snippets: Vec::new(),
        user_state: UserGameState::default(),
    };

    let mut scored = card;
    scored.recommendation_score = score_card(&scored);
    upsert_game(conn, &scored)
}

fn default_seeds() -> Vec<GameSeed> {
    vec![
        GameSeed {
            appid: 3_744_430,
            name: "Together Moon Escape",
            release_date: "2026-04-16",
            release_date_text: "2026.04 · Demo",
            demo_status: DemoStatus::DemoOnly,
            positive_review_pct: 97.0,
            total_reviews: 1245,
            current_players: 2893,
            ai_score: 92.0,
            ai_summary:
                "适合喜欢解谜和轻合作的 2-4 人队伍，卖点是沟通与分工，风险是内容体量仍需观察。",
            tags: vec!["解谜", "合作", "独立"],
            multiplayer_modes: vec!["Online Co-op", "Co-op"],
            section: "new",
        },
        GameSeed {
            appid: 3_087_930,
            name: "Pebble Knights",
            release_date: "2026-04-21",
            release_date_text: "2026.04 · Demo",
            demo_status: DemoStatus::DemoOnly,
            positive_review_pct: 95.0,
            total_reviews: 643,
            current_players: 1327,
            ai_score: 89.0,
            ai_summary: "动作节奏轻快，适合短局尝鲜。当前样本不算大，但口碑和合作定位都很干净。",
            tags: vec!["动作", "合作", "Roguelite"],
            multiplayer_modes: vec!["Online Co-op", "Shared/Split Screen Co-op"],
            section: "new",
        },
        GameSeed {
            appid: 3_844_970,
            name: "Burglin' Gnomes",
            release_date: "2026-04-18",
            release_date_text: "2026.04",
            demo_status: DemoStatus::Released,
            positive_review_pct: 92.0,
            total_reviews: 231,
            current_players: 945,
            ai_score: 88.0,
            ai_summary: "潜行与捣乱的组合很适合朋友互坑，属于小样本但辨识度强的新游。",
            tags: vec!["潜行", "策略", "合作"],
            multiplayer_modes: vec!["Online Co-op", "Multi-player"],
            section: "new",
        },
        GameSeed {
            appid: 1_063_420,
            name: "Void Crew",
            release_date: "2026-04-10",
            release_date_text: "2026.04 · Demo + 已发售",
            demo_status: DemoStatus::ReleasedWithDemo,
            positive_review_pct: 90.0,
            total_reviews: 512,
            current_players: 3102,
            ai_score: 87.0,
            ai_summary: "太空船员分工明确，适合固定车队。上手门槛略高，但合作戏剧性足。",
            tags: vec!["太空", "角色分工", "合作"],
            multiplayer_modes: vec!["Online Co-op", "Multi-player"],
            section: "new",
        },
        GameSeed {
            appid: 632_360,
            name: "Risk of Rain 2",
            release_date: "2020-08-11",
            release_date_text: "2020.08",
            demo_status: DemoStatus::Released,
            positive_review_pct: 96.0,
            total_reviews: 118_905,
            current_players: 8162,
            ai_score: 93.0,
            ai_summary: "强成长曲线、节奏紧凑，适合 2-4 人反复开局，是经典合作肉鸽基准线。",
            tags: vec!["肉鸽", "第三人称射击", "合作"],
            multiplayer_modes: vec!["Online Co-op", "Multi-player"],
            section: "classic",
        },
        GameSeed {
            appid: 413_150,
            name: "Stardew Valley",
            release_date: "2016-02-26",
            release_date_text: "2016.02",
            demo_status: DemoStatus::Released,
            positive_review_pct: 98.0,
            total_reviews: 547_470,
            current_players: 6301,
            ai_score: 92.0,
            ai_summary: "慢节奏合作经营的安全牌，适合长线朋友档，不适合只想打两局就下的人。",
            tags: vec!["农场模拟", "RPG", "合作"],
            multiplayer_modes: vec!["Online Co-op", "LAN Co-op"],
            section: "classic",
        },
        GameSeed {
            appid: 548_430,
            name: "Deep Rock Galactic",
            release_date: "2020-05-13",
            release_date_text: "2020.05",
            demo_status: DemoStatus::Released,
            positive_review_pct: 97.0,
            total_reviews: 212_384,
            current_players: 9678,
            ai_score: 95.0,
            ai_summary: "四人职业分工非常成熟，任务制让每晚开黑成本低，是老游区第一梯队。",
            tags: vec!["射击", "探索", "矮人"],
            multiplayer_modes: vec!["Online Co-op", "Co-op"],
            section: "classic",
        },
        GameSeed {
            appid: 550,
            name: "Left 4 Dead 2",
            release_date: "2009-11-17",
            release_date_text: "2009.11",
            demo_status: DemoStatus::Released,
            positive_review_pct: 97.0,
            total_reviews: 758_298,
            current_players: 7990,
            ai_score: 94.0,
            ai_summary: "老但依然硬，四人合作节奏和地图可读性极强；画面时代感是主要取舍。",
            tags: vec!["丧尸", "射击", "合作"],
            multiplayer_modes: vec!["Online Co-op", "Multi-player"],
            section: "classic",
        },
    ]
}

fn ensure_games_metadata_columns(conn: &Connection) -> Result<()> {
    add_games_column_if_missing(
        conn,
        "short_description",
        "ALTER TABLE games ADD COLUMN short_description TEXT",
    )?;
    add_games_column_if_missing(
        conn,
        "release_state",
        "ALTER TABLE games ADD COLUMN release_state TEXT NOT NULL DEFAULT '\"released\"'",
    )?;
    add_games_column_if_missing(
        conn,
        "supported_languages_json",
        "ALTER TABLE games ADD COLUMN supported_languages_json TEXT NOT NULL DEFAULT '[]'",
    )?;
    add_games_column_if_missing(
        conn,
        "is_adult_content",
        "ALTER TABLE games ADD COLUMN is_adult_content INTEGER NOT NULL DEFAULT 0",
    )?;
    add_games_column_if_missing(
        conn,
        "is_free",
        "ALTER TABLE games ADD COLUMN is_free INTEGER NOT NULL DEFAULT 0",
    )?;
    add_games_column_if_missing(
        conn,
        "price_text",
        "ALTER TABLE games ADD COLUMN price_text TEXT",
    )?;
    add_games_column_if_missing(
        conn,
        "discount_percent",
        "ALTER TABLE games ADD COLUMN discount_percent INTEGER",
    )?;
    add_games_column_if_missing(
        conn,
        "ai_analysis_report_json",
        "ALTER TABLE games ADD COLUMN ai_analysis_report_json TEXT",
    )?;
    add_games_column_if_missing(
        conn,
        "ai_analysis_generated_at",
        "ALTER TABLE games ADD COLUMN ai_analysis_generated_at TEXT",
    )?;
    add_games_column_if_missing(
        conn,
        "store_screenshot_urls_json",
        "ALTER TABLE games ADD COLUMN store_screenshot_urls_json TEXT NOT NULL DEFAULT '[]'",
    )?;
    add_games_column_if_missing(conn, "created_at", "ALTER TABLE games ADD COLUMN created_at TEXT")?;
    conn.execute(
        "UPDATE games SET created_at = updated_at WHERE created_at IS NULL OR TRIM(created_at) = ''",
        [],
    )?;
    Ok(())
}

fn ensure_discovery_run_columns(conn: &Connection) -> Result<()> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('discovery_runs') WHERE name = 'sync_mode' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();

    if !exists {
        conn.execute(
            "ALTER TABLE discovery_runs ADD COLUMN sync_mode TEXT NOT NULL DEFAULT 'full'",
            [],
        )?;
    }

    let completion_reason_exists = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('discovery_runs') WHERE name = 'completion_reason' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();

    if !completion_reason_exists {
        conn.execute(
            "ALTER TABLE discovery_runs ADD COLUMN completion_reason TEXT",
            [],
        )?;
    }

    Ok(())
}

fn ensure_classic_discovery_run_columns(conn: &Connection) -> Result<()> {
    let table_exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'classic_discovery_runs' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !table_exists {
        return Ok(());
    }

    let has_max_pages = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('classic_discovery_runs') WHERE name = 'max_pages' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !has_max_pages {
        conn.execute("DROP TABLE classic_discovery_runs", [])?;
        conn.execute(
            r#"
            CREATE TABLE classic_discovery_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                status TEXT NOT NULL CHECK (
                    status IN (
                        'running',
                        'paused',
                        'completed',
                        'failed',
                        'cancelled',
                        'interrupted'
                    )
                ),
                max_pages INTEGER NOT NULL
                    CHECK (max_pages >= 1 AND max_pages <= 4294967295),
                page_size INTEGER NOT NULL
                    CHECK (page_size >= 1 AND page_size <= 4294967295),
                pages_processed INTEGER NOT NULL DEFAULT 0
                    CHECK (pages_processed >= 0 AND pages_processed <= 4294967295),
                scanned_apps INTEGER NOT NULL DEFAULT 0 CHECK (scanned_apps >= 0),
                considered_apps INTEGER NOT NULL DEFAULT 0 CHECK (considered_apps >= 0),
                added_games INTEGER NOT NULL DEFAULT 0 CHECK (added_games >= 0),
                rejected_games INTEGER NOT NULL DEFAULT 0 CHECK (rejected_games >= 0),
                skipped_existing INTEGER NOT NULL DEFAULT 0 CHECK (skipped_existing >= 0),
                skipped_rejected_cache INTEGER NOT NULL DEFAULT 0 CHECK (skipped_rejected_cache >= 0),
                failed_games INTEGER NOT NULL DEFAULT 0 CHECK (failed_games >= 0),
                current_appid INTEGER
                    CHECK (current_appid IS NULL OR (current_appid >= 0 AND current_appid <= 4294967295)),
                last_appid INTEGER
                    CHECK (last_appid IS NULL OR (last_appid >= 0 AND last_appid <= 4294967295)),
                consecutive_empty_pages INTEGER NOT NULL DEFAULT 0
                    CHECK (consecutive_empty_pages >= 0 AND consecutive_empty_pages <= 4294967295),
                rule_version TEXT NOT NULL,
                started_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                finished_at TEXT,
                last_error TEXT
            )
            "#,
            [],
        )?;
    }

    Ok(())
}

fn ensure_ai_analysis_queue_columns(conn: &Connection) -> Result<()> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('ai_analysis_queue') WHERE name = 'source' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();

    if !exists {
        conn.execute(
            "ALTER TABLE ai_analysis_queue ADD COLUMN source TEXT NOT NULL DEFAULT 'new_release'",
            [],
        )?;
    }

    Ok(())
}

fn perform_classic_v2_cleanup(conn: &Connection) -> Result<()> {
    if get_config(conn, CLASSIC_DISCOVERY_CLEANUP_VERSION_CONFIG_KEY)?
        .as_deref()
        == Some(CLASSIC_DISCOVERY_RULE_VERSION)
    {
        return Ok(());
    }

    conn.execute("DELETE FROM classic_discovery_runs", [])?;
    conn.execute("DELETE FROM classic_reject_cache", [])?;
    conn.execute(
        "DELETE FROM app_config WHERE key IN (?1, ?2)",
        params![
            CLASSIC_DISCOVERY_LAST_APPID_CONFIG_KEY,
            CLASSIC_DISCOVERY_LAST_RUN_STATUS_CONFIG_KEY
        ],
    )?;

    let appids_to_delete = load_all_games(conn)?
        .into_iter()
        .filter(|game| should_delete_for_classic_cleanup(game))
        .map(|game| game.appid)
        .collect::<Vec<_>>();
    for appid in appids_to_delete {
        delete_game_and_related_state(conn, appid)?;
    }

    set_config(
        conn,
        CLASSIC_DISCOVERY_CLEANUP_VERSION_CONFIG_KEY,
        CLASSIC_DISCOVERY_RULE_VERSION,
    )?;
    Ok(())
}

fn should_delete_for_classic_cleanup(game: &GameCard) -> bool {
    if matches!(
        game.release_state,
        StoreReleaseState::Upcoming | StoreReleaseState::Tba
    ) {
        return false;
    }

    let facts = facts_from_card(game);
    match bucket_game(&facts, &today_iso_utc()) {
        ReleaseBucket::New => false,
        ReleaseBucket::Classic => false,
        ReleaseBucket::ClassicHidden => {
            let days = game
                .release_date
                .as_deref()
                .and_then(|date| {
                    let release = time::Date::parse(
                        date,
                        time::macros::format_description!("[year]-[month]-[day]"),
                    )
                    .ok()?;
                    let today = time::Date::parse(
                        &today_iso_utc(),
                        time::macros::format_description!("[year]-[month]-[day]"),
                    )
                    .ok()?;
                    Some((today - release).whole_days())
                })
                .unwrap_or_default();
            days > 30
                && (game.release_date.is_none()
                    || game.positive_review_pct.is_none()
                    || game.total_reviews.is_none()
                    || game.total_reviews.unwrap_or_default() < 300
                    || game.positive_review_pct.unwrap_or_default() < 60.0)
        }
    }
}

pub fn delete_game_and_related_state(conn: &Connection, appid: u32) -> Result<()> {
    conn.execute("DELETE FROM sync_queue WHERE appid = ?1", params![appid])?;
    conn.execute(
        "DELETE FROM metadata_backfill_queue WHERE appid = ?1",
        params![appid],
    )?;
    conn.execute("DELETE FROM ai_analysis_queue WHERE appid = ?1", params![appid])?;
    conn.execute(
        "DELETE FROM analysis_narrative_cache WHERE appid = ?1",
        params![appid],
    )?;
    conn.execute("DELETE FROM classic_reject_cache WHERE appid = ?1", params![appid])?;
    conn.execute("DELETE FROM games WHERE appid = ?1", params![appid])?;
    Ok(())
}

pub fn load_analysis_narrative_cache(
    conn: &Connection,
    cache_key: &str,
) -> Result<Option<AnalysisNarrative>> {
    let narrative_json = conn
        .query_row(
            "SELECT narrative_json FROM analysis_narrative_cache WHERE cache_key = ?1",
            params![cache_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;

    match narrative_json {
        Some(narrative_json) => Ok(Some(serde_json::from_str(&narrative_json)?)),
        None => Ok(None),
    }
}

pub fn save_analysis_narrative_cache(
    conn: &Connection,
    cache_key: &str,
    appid: u32,
    score_version: &str,
    base_url: &str,
    model: &str,
    narrative: &AnalysisNarrative,
) -> Result<()> {
    let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
    conn.execute(
        r#"
        INSERT INTO analysis_narrative_cache (
            cache_key,
            appid,
            score_version,
            base_url,
            model,
            narrative_json,
            created_at,
            updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
        ON CONFLICT(cache_key) DO UPDATE SET
            appid = excluded.appid,
            score_version = excluded.score_version,
            base_url = excluded.base_url,
            model = excluded.model,
            narrative_json = excluded.narrative_json,
            updated_at = excluded.updated_at
        "#,
        params![
            cache_key,
            appid,
            score_version,
            base_url,
            model,
            serde_json::to_string(narrative)?,
            now,
        ],
    )?;

    Ok(())
}

fn migrate_default_language_to_schinese(conn: &Connection) -> Result<()> {
    if get_config(conn, "language")?
        .as_deref()
        .is_some_and(|language| language.eq_ignore_ascii_case("english"))
    {
        set_config(conn, "language", "schinese")?;
    }

    Ok(())
}

fn add_games_column_if_missing(conn: &Connection, column: &str, sql: &str) -> Result<()> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('games') WHERE name = ?1 LIMIT 1",
            params![column],
            |_| Ok(()),
        )
        .optional()?
        .is_some();

    if !exists {
        conn.execute(sql, [])?;
    }

    Ok(())
}

fn steam_header_url(appid: u32) -> String {
    format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/header.jpg")
}

fn attach_discovery_failures(
    conn: &Connection,
    mut snapshot: DiscoveryRunSnapshot,
) -> Result<DiscoveryRunSnapshot> {
    snapshot.failures = load_discovery_failures(conn, snapshot.id)?;
    Ok(snapshot)
}

fn load_discovery_failures(conn: &Connection, run_id: i64) -> Result<Vec<DiscoveryFailureItem>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT page_index, appid, stage, reason, created_at
        FROM discovery_failures
        WHERE run_id = ?1
        ORDER BY page_index ASC, id ASC
        "#,
    )?;

    let rows = stmt.query_map(params![run_id], |row| {
        Ok(DiscoveryFailureItem {
            page_index: row.get(0)?,
            appid: row.get(1)?,
            stage: row.get(2)?,
            reason: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn validate_discovery_run_rows(conn: &Connection) -> Result<()> {
    let invalid_row = conn
        .query_row(
            r#"
            SELECT id, status, sync_mode, completion_reason, target_added_games, page_size,
                   pages_processed, scanned_apps, added_games, added_new_games,
                   added_classic_games, skipped_existing, skipped_non_multiplayer, failed_games,
                   current_appid, last_appid, have_more_results, started_at, updated_at
            FROM discovery_runs
            WHERE status IS NULL
               OR status NOT IN ('running', 'paused', 'completed', 'failed', 'cancelled', 'interrupted')
               OR sync_mode IS NULL
               OR sync_mode NOT IN ('quick', 'full')
               OR completion_reason NOT IN (
                    'target_reached',
                    'page_budget_reached',
                    'no_more_results',
                    'paused',
                    'cancelled',
                    'failed',
                    'interrupted'
               )
               OR target_added_games IS NULL
               OR target_added_games < 0
               OR target_added_games > 4294967295
               OR page_size IS NULL
               OR page_size <= 0
               OR page_size > 4294967295
               OR pages_processed IS NULL
               OR pages_processed < 0
               OR pages_processed > 4294967295
               OR scanned_apps IS NULL
               OR scanned_apps < 0
               OR added_games IS NULL
               OR added_games < 0
               OR added_new_games IS NULL
               OR added_new_games < 0
               OR added_classic_games IS NULL
               OR added_classic_games < 0
               OR skipped_existing IS NULL
               OR skipped_existing < 0
               OR skipped_non_multiplayer IS NULL
               OR skipped_non_multiplayer < 0
               OR failed_games IS NULL
               OR failed_games < 0
               OR current_appid < 0
               OR current_appid > 4294967295
               OR last_appid < 0
               OR last_appid > 4294967295
               OR have_more_results IS NULL
               OR have_more_results NOT IN (0, 1)
               OR started_at IS NULL
               OR updated_at IS NULL
            ORDER BY id ASC
            LIMIT 1
            "#,
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, Option<i64>>(14)?,
                    row.get::<_, Option<i64>>(15)?,
                    row.get::<_, Option<i64>>(16)?,
                    row.get::<_, Option<String>>(17)?,
                    row.get::<_, Option<String>>(18)?,
                ))
            },
        )
        .optional()?;

    if let Some((
        id,
        status,
        sync_mode,
        completion_reason,
        target_added_games,
        page_size,
        pages_processed,
        scanned_apps,
        added_games,
        added_new_games,
        added_classic_games,
        skipped_existing,
        skipped_non_multiplayer,
        failed_games,
        current_appid,
        last_appid,
        have_more_results,
        started_at,
        updated_at,
    )) = invalid_row
    {
        let reason = if status.is_none() {
            "status is NULL but must be NOT NULL".to_string()
        } else if !matches!(
            status.as_deref().expect("checked is_some"),
            "running" | "paused" | "completed" | "failed" | "cancelled" | "interrupted"
        ) {
            format!(
                "status='{}' is not allowed",
                status.as_deref().expect("checked is_some")
            )
        } else if sync_mode.is_none() {
            "sync_mode is NULL but must be NOT NULL".to_string()
        } else if !matches!(
            sync_mode.as_deref().expect("checked is_some"),
            "quick" | "full"
        ) {
            format!(
                "sync_mode='{}' is not allowed",
                sync_mode.as_deref().expect("checked is_some")
            )
        } else if completion_reason.as_deref().is_some_and(|value| {
            !matches!(
                value,
                "target_reached"
                    | "page_budget_reached"
                    | "no_more_results"
                    | "paused"
                    | "cancelled"
                    | "failed"
                    | "interrupted"
            )
        }) {
            format!(
                "completion_reason='{}' is not allowed",
                completion_reason.as_deref().expect("checked is_some")
            )
        } else if target_added_games.is_none() {
            "target_added_games is NULL but must be NOT NULL".to_string()
        } else if target_added_games.expect("checked is_some") < 0 {
            format!(
                "target_added_games={} is negative",
                target_added_games.expect("checked is_some")
            )
        } else if target_added_games.expect("checked is_some") > MAX_SQLITE_U32 {
            format!(
                "target_added_games={} exceeds u32 max {MAX_SQLITE_U32}",
                target_added_games.expect("checked is_some")
            )
        } else if page_size.is_none() {
            "page_size is NULL but must be NOT NULL".to_string()
        } else if page_size.expect("checked is_some") <= 0 {
            format!(
                "page_size={} must be > 0",
                page_size.expect("checked is_some")
            )
        } else if page_size.expect("checked is_some") > MAX_SQLITE_U32 {
            format!(
                "page_size={} exceeds u32 max {MAX_SQLITE_U32}",
                page_size.expect("checked is_some")
            )
        } else if pages_processed.is_none() {
            "pages_processed is NULL but must be NOT NULL".to_string()
        } else if pages_processed.expect("checked is_some") < 0 {
            format!(
                "pages_processed={} is negative",
                pages_processed.expect("checked is_some")
            )
        } else if pages_processed.expect("checked is_some") > MAX_SQLITE_U32 {
            format!(
                "pages_processed={} exceeds u32 max {MAX_SQLITE_U32}",
                pages_processed.expect("checked is_some")
            )
        } else if scanned_apps.is_none() {
            "scanned_apps is NULL but must be NOT NULL".to_string()
        } else if scanned_apps.expect("checked is_some") < 0 {
            format!(
                "scanned_apps={} is negative",
                scanned_apps.expect("checked is_some")
            )
        } else if added_games.is_none() {
            "added_games is NULL but must be NOT NULL".to_string()
        } else if added_games.expect("checked is_some") < 0 {
            format!(
                "added_games={} is negative",
                added_games.expect("checked is_some")
            )
        } else if added_new_games.is_none() {
            "added_new_games is NULL but must be NOT NULL".to_string()
        } else if added_new_games.expect("checked is_some") < 0 {
            format!(
                "added_new_games={} is negative",
                added_new_games.expect("checked is_some")
            )
        } else if added_classic_games.is_none() {
            "added_classic_games is NULL but must be NOT NULL".to_string()
        } else if added_classic_games.expect("checked is_some") < 0 {
            format!(
                "added_classic_games={} is negative",
                added_classic_games.expect("checked is_some")
            )
        } else if skipped_existing.is_none() {
            "skipped_existing is NULL but must be NOT NULL".to_string()
        } else if skipped_existing.expect("checked is_some") < 0 {
            format!(
                "skipped_existing={} is negative",
                skipped_existing.expect("checked is_some")
            )
        } else if skipped_non_multiplayer.is_none() {
            "skipped_non_multiplayer is NULL but must be NOT NULL".to_string()
        } else if skipped_non_multiplayer.expect("checked is_some") < 0 {
            format!(
                "skipped_non_multiplayer={} is negative",
                skipped_non_multiplayer.expect("checked is_some")
            )
        } else if failed_games.is_none() {
            "failed_games is NULL but must be NOT NULL".to_string()
        } else if failed_games.expect("checked is_some") < 0 {
            format!(
                "failed_games={} is negative",
                failed_games.expect("checked is_some")
            )
        } else if current_appid.is_some_and(|value| value < 0) {
            format!(
                "current_appid={} is negative",
                current_appid.expect("checked is_some")
            )
        } else if current_appid.is_some_and(|value| value > MAX_SQLITE_U32) {
            format!(
                "current_appid={} exceeds u32 max {MAX_SQLITE_U32}",
                current_appid.expect("checked is_some")
            )
        } else if last_appid.is_some_and(|value| value < 0) {
            format!(
                "last_appid={} is negative",
                last_appid.expect("checked is_some")
            )
        } else if last_appid.is_some_and(|value| value > MAX_SQLITE_U32) {
            format!(
                "last_appid={} exceeds u32 max {MAX_SQLITE_U32}",
                last_appid.expect("checked is_some")
            )
        } else if have_more_results.is_none() {
            "have_more_results is NULL but must be NOT NULL".to_string()
        } else if !matches!(have_more_results.expect("checked is_some"), 0 | 1) {
            format!(
                "have_more_results={} must be 0 or 1",
                have_more_results.expect("checked is_some")
            )
        } else if started_at.is_none() {
            "started_at is NULL but must be NOT NULL".to_string()
        } else if updated_at.is_none() {
            "updated_at is NULL but must be NOT NULL".to_string()
        } else {
            "unknown discovery_runs validation failure".to_string()
        };

        anyhow::bail!("invalid discovery_runs row detected during migration: id={id}, {reason}");
    }

    Ok(())
}

fn validate_discovery_failure_rows(conn: &Connection) -> Result<()> {
    let invalid_row = conn
        .query_row(
            r#"
            SELECT id, run_id, page_index, appid, stage, reason, created_at
            FROM discovery_failures
            WHERE run_id IS NULL
               OR page_index IS NULL
               OR page_index < 0
               OR page_index > 4294967295
               OR appid < 0
               OR appid > 4294967295
               OR stage IS NULL
               OR reason IS NULL
               OR created_at IS NULL
            ORDER BY id ASC
            LIMIT 1
            "#,
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()?;

    if let Some((id, run_id, page_index, appid, stage, reason_text, created_at)) = invalid_row {
        let reason = if run_id.is_none() {
            "run_id is NULL but must be NOT NULL".to_string()
        } else if page_index.is_none() {
            "page_index is NULL but must be NOT NULL".to_string()
        } else if page_index.expect("checked is_some") < 0 {
            format!(
                "page_index={} is negative",
                page_index.expect("checked is_some")
            )
        } else if page_index.expect("checked is_some") > MAX_SQLITE_U32 {
            format!(
                "page_index={} exceeds u32 max {MAX_SQLITE_U32}",
                page_index.expect("checked is_some")
            )
        } else if appid.is_some_and(|value| value < 0) {
            format!("appid={} is negative", appid.expect("checked is_some"))
        } else if appid.is_some_and(|value| value > MAX_SQLITE_U32) {
            format!(
                "appid={} exceeds u32 max {MAX_SQLITE_U32}",
                appid.expect("checked is_some")
            )
        } else if stage.is_none() {
            "stage is NULL but must be NOT NULL".to_string()
        } else if reason_text.is_none() {
            "reason is NULL but must be NOT NULL".to_string()
        } else if created_at.is_none() {
            "created_at is NULL but must be NOT NULL".to_string()
        } else {
            "unknown discovery_failures validation failure".to_string()
        };

        anyhow::bail!(
            "invalid discovery_failures row detected during migration: id={id}, run_id={run_id:?}, {reason}"
        );
    }

    Ok(())
}

fn map_discovery_run_snapshot(row: &Row<'_>) -> rusqlite::Result<DiscoveryRunSnapshot> {
    Ok(DiscoveryRunSnapshot {
        id: row.get(0)?,
        status: discovery_run_status_from_str(&row.get::<_, String>(1)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        sync_mode: sync_mode_from_str(&row.get::<_, String>(2)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        completion_reason: row
            .get::<_, Option<String>>(3)?
            .map(|value| {
                discovery_completion_reason_from_str(&value).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
                    )
                })
            })
            .transpose()?,
        target_added_games: row.get(4)?,
        page_size: row.get(5)?,
        pages_processed: row.get(6)?,
        scanned_apps: i64_to_usize(row.get(7)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        added_games: i64_to_usize(row.get(8)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        added_new_games: i64_to_usize(row.get(9)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        added_classic_games: i64_to_usize(row.get(10)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                10,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        skipped_existing: i64_to_usize(row.get(11)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                11,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        skipped_non_multiplayer: i64_to_usize(row.get(12)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                12,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        failed_games: i64_to_usize(row.get(13)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                13,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        current_appid: row.get(14)?,
        last_appid: row.get(15)?,
        have_more_results: row.get(16)?,
        started_at: row.get(17)?,
        updated_at: row.get(18)?,
        finished_at: row.get(19)?,
        last_error: row.get(20)?,
        failures: Vec::new(),
    })
}

fn map_classic_discovery_run_snapshot(
    row: &Row<'_>,
) -> rusqlite::Result<ClassicDiscoveryRunSnapshot> {
    Ok(ClassicDiscoveryRunSnapshot {
        id: row.get(0)?,
        status: discovery_run_status_from_str(&row.get::<_, String>(1)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        max_pages: row.get(2)?,
        page_size: row.get(3)?,
        pages_processed: row.get(4)?,
        scanned_apps: i64_to_usize(row.get(5)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        considered_apps: i64_to_usize(row.get(6)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        added_games: i64_to_usize(row.get(7)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        rejected_games: i64_to_usize(row.get(8)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        skipped_existing: i64_to_usize(row.get(9)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        skipped_rejected_cache: i64_to_usize(row.get(10)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                10,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        failed_games: i64_to_usize(row.get(11)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                11,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
            )
        })?,
        current_appid: row.get(12)?,
        last_appid: row.get(13)?,
        consecutive_empty_pages: row.get(14)?,
        rule_version: row.get(15)?,
        started_at: row.get(16)?,
        updated_at: row.get(17)?,
        finished_at: row.get(18)?,
        last_error: row.get(19)?,
    })
}

fn discovery_run_status_as_str(status: &DiscoveryRunStatus) -> &'static str {
    match status {
        DiscoveryRunStatus::Running => "running",
        DiscoveryRunStatus::Paused => "paused",
        DiscoveryRunStatus::Completed => "completed",
        DiscoveryRunStatus::Failed => "failed",
        DiscoveryRunStatus::Cancelled => "cancelled",
        DiscoveryRunStatus::Interrupted => "interrupted",
    }
}

fn discovery_completion_reason_as_str(reason: &DiscoveryCompletionReason) -> &'static str {
    match reason {
        DiscoveryCompletionReason::TargetReached => "target_reached",
        DiscoveryCompletionReason::PageBudgetReached => "page_budget_reached",
        DiscoveryCompletionReason::NoMoreResults => "no_more_results",
        DiscoveryCompletionReason::Paused => "paused",
        DiscoveryCompletionReason::Cancelled => "cancelled",
        DiscoveryCompletionReason::Failed => "failed",
        DiscoveryCompletionReason::Interrupted => "interrupted",
    }
}

fn discovery_completion_reason_from_str(value: &str) -> Result<DiscoveryCompletionReason> {
    match value {
        "target_reached" => Ok(DiscoveryCompletionReason::TargetReached),
        "page_budget_reached" => Ok(DiscoveryCompletionReason::PageBudgetReached),
        "no_more_results" => Ok(DiscoveryCompletionReason::NoMoreResults),
        "paused" => Ok(DiscoveryCompletionReason::Paused),
        "cancelled" => Ok(DiscoveryCompletionReason::Cancelled),
        "failed" => Ok(DiscoveryCompletionReason::Failed),
        "interrupted" => Ok(DiscoveryCompletionReason::Interrupted),
        _ => anyhow::bail!("unknown discovery completion reason: {value}"),
    }
}

fn discovery_run_status_from_str(value: &str) -> Result<DiscoveryRunStatus> {
    match value {
        "running" => Ok(DiscoveryRunStatus::Running),
        "paused" => Ok(DiscoveryRunStatus::Paused),
        "completed" => Ok(DiscoveryRunStatus::Completed),
        "failed" => Ok(DiscoveryRunStatus::Failed),
        "cancelled" => Ok(DiscoveryRunStatus::Cancelled),
        "interrupted" => Ok(DiscoveryRunStatus::Interrupted),
        _ => anyhow::bail!("unknown discovery run status: {value}"),
    }
}

fn classic_reject_reason_code_as_str(code: &ClassicRejectReasonCode) -> &'static str {
    match code {
        ClassicRejectReasonCode::NonMultiplayer => "non_multiplayer",
        ClassicRejectReasonCode::NotReleased => "not_released",
        ClassicRejectReasonCode::TooNew => "too_new",
        ClassicRejectReasonCode::LowReviewCount => "low_review_count",
        ClassicRejectReasonCode::LowPositiveReviewPct => "low_positive_review_pct",
        ClassicRejectReasonCode::LowCurrentPlayers => "low_current_players",
    }
}

fn classic_reject_reason_code_from_str(value: &str) -> Result<ClassicRejectReasonCode> {
    match value {
        "non_multiplayer" => Ok(ClassicRejectReasonCode::NonMultiplayer),
        "not_released" => Ok(ClassicRejectReasonCode::NotReleased),
        "too_new" => Ok(ClassicRejectReasonCode::TooNew),
        "low_review_count" => Ok(ClassicRejectReasonCode::LowReviewCount),
        "low_positive_review_pct" => Ok(ClassicRejectReasonCode::LowPositiveReviewPct),
        "low_current_players" => Ok(ClassicRejectReasonCode::LowCurrentPlayers),
        _ => anyhow::bail!("unknown classic reject reason code '{value}'"),
    }
}

fn sync_mode_as_str(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Quick => "quick",
        SyncMode::Full => "full",
    }
}

fn ai_analysis_queue_source_as_str(source: AiAnalysisQueueSource) -> &'static str {
    match source {
        AiAnalysisQueueSource::NewRelease => "new_release",
        AiAnalysisQueueSource::Classic => "classic",
    }
}

fn ai_analysis_queue_source_from_str(value: &str) -> Result<AiAnalysisQueueSource> {
    match value {
        "new_release" => Ok(AiAnalysisQueueSource::NewRelease),
        "classic" => Ok(AiAnalysisQueueSource::Classic),
        _ => anyhow::bail!("unknown ai analysis queue source '{value}'"),
    }
}

fn sync_mode_from_str(value: &str) -> Result<SyncMode> {
    match value {
        "quick" => Ok(SyncMode::Quick),
        "full" => Ok(SyncMode::Full),
        _ => anyhow::bail!("unknown sync mode: {value}"),
    }
}

fn merge_nullable_patch<T>(patch: Option<Option<T>>, existing: Option<T>) -> Option<T> {
    match patch {
        Some(value) => value,
        None => existing,
    }
}

fn usize_to_i64(value: usize) -> Result<i64> {
    i64::try_from(value).context("usize value exceeded sqlite integer range")
}

fn i64_to_usize(value: i64) -> Result<usize> {
    usize::try_from(value).context("sqlite integer was negative or exceeded usize range")
}

fn i64_to_u8(value: i64) -> Result<u8> {
    u8::try_from(value).context("sqlite integer was negative or exceeded u8 range")
}

fn parse_rfc3339_utc(value: &str) -> Result<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(Into::into)
}

fn now_rfc3339() -> Result<String> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

#[cfg(test)]
mod tests {
    use super::{
        classic_discovery_is_due, delete_game_and_related_state, enqueue_ai_analysis_jobs,
        get_config, load_dashboard, list_ai_analysis_queue_ready_jobs, migrate, public_config,
        set_config, upsert_game, CLASSIC_DISCOVERY_LAST_RUN_STATUS_CONFIG_KEY,
        CLASSIC_DISCOVERY_LAST_STARTED_AT_CONFIG_KEY, CLASSIC_DISCOVERY_LAST_OFFSET_CONFIG_KEY,
        CLASSIC_DISCOVERY_CLEANUP_VERSION_CONFIG_KEY,
        CLASSIC_DISCOVERY_RULE_VERSION,
    };
    use crate::models::{
        AiAnalysisQueueSource, GameCard, ReviewSnippet, StoreReleaseState, UserGameState,
    };
    use crate::recommendation::DemoStatus;
    use rusqlite::Connection;

    #[test]
    fn public_config_exposes_default_ai_batch_refresh_concurrency() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        migrate(&conn).expect("migrate");

        let config = serde_json::to_value(public_config(&conn).expect("load public config"))
            .expect("serialize public config");

        assert_eq!(config["aiBatchRefreshConcurrency"], 5);
        assert_eq!(config["llmProvider"], "deepseek");
        assert_eq!(config["steamApiKeyValidated"], false);
        assert_eq!(config["llmConfigValidated"], false);
        assert_eq!(config["onboardingCompleted"], false);
        assert_eq!(config["onboardingCurrentStep"], 1);
        assert_eq!(config["onboardingLlmProviderDraft"], "deepseek");
        assert_eq!(
            get_config(&conn, "ai_batch_refresh_concurrency").expect("load concurrency config"),
            Some("5".to_string())
        );
    }

    #[test]
    fn ai_analysis_queue_prioritizes_new_release_jobs() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        migrate(&conn).expect("migrate");

        enqueue_ai_analysis_jobs(&conn, AiAnalysisQueueSource::Classic, [2001, 2002])
            .expect("enqueue classic jobs");
        enqueue_ai_analysis_jobs(&conn, AiAnalysisQueueSource::NewRelease, [1001, 1002])
            .expect("enqueue new jobs");

        let jobs = list_ai_analysis_queue_ready_jobs(&conn).expect("list ready jobs");
        let ordered: Vec<(u32, AiAnalysisQueueSource)> =
            jobs.into_iter().map(|job| (job.appid, job.source)).collect();

        assert_eq!(
            ordered,
            vec![
                (1001, AiAnalysisQueueSource::NewRelease),
                (1002, AiAnalysisQueueSource::NewRelease),
                (2001, AiAnalysisQueueSource::Classic),
                (2002, AiAnalysisQueueSource::Classic),
            ]
        );
    }

    #[test]
    fn dashboard_keeps_classic_hidden_out_of_main_classics_but_in_collections() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        migrate(&conn).expect("migrate");
        let hidden = GameCard {
            appid: 700_001,
            name: "Hidden Candidate".to_string(),
            short_description: None,
            section: "classic_hidden".to_string(),
            release_date: Some("2025-01-01".to_string()),
            release_date_text: "2025.01".to_string(),
            release_state: StoreReleaseState::Released,
            demo_status: DemoStatus::Released,
            supported_languages: vec!["English".to_string()],
            is_adult_content: false,
            is_free: true,
            price_text: Some("Free To Play".to_string()),
            discount_percent: None,
            positive_review_pct: Some(68.0),
            total_reviews: Some(350),
            current_players: Some(32),
            recommendation_score: 50.0,
            ai_score: None,
            ai_summary: "hidden".to_string(),
            capsule_url: "https://example.com/hidden.jpg".to_string(),
            store_screenshot_urls: vec![],
            tags: vec!["Co-op".to_string()],
            multiplayer_modes: vec!["Online Co-op".to_string()],
            review_snippets: Vec::<ReviewSnippet>::new(),
            user_state: UserGameState {
                favorite: true,
                wishlist: false,
                followed: false,
                viewed: false,
                updated_at: None,
            },
        };
        upsert_game(&conn, &hidden).expect("insert hidden game");
        crate::db::set_game_user_state(
            &conn,
            hidden.appid,
            crate::models::UserGameStatePatch {
                favorite: Some(true),
                ..Default::default()
            },
        )
        .expect("set favorite state");

        let dashboard = load_dashboard(&conn).expect("load dashboard");

        assert!(dashboard.classics.is_empty());
        assert_eq!(dashboard.hidden_games.len(), 1);
        assert_eq!(dashboard.collections.favorites.len(), 1);
        assert_eq!(dashboard.collections.favorites[0].appid, hidden.appid);
    }

    #[test]
    fn delete_game_and_related_state_cleans_queues_and_cache() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        migrate(&conn).expect("migrate");
        let game = GameCard {
            appid: 700_002,
            name: "Delete Me".to_string(),
            short_description: None,
            section: "classic_hidden".to_string(),
            release_date: Some("2025-01-01".to_string()),
            release_date_text: "2025.01".to_string(),
            release_state: StoreReleaseState::Released,
            demo_status: DemoStatus::Released,
            supported_languages: vec!["English".to_string()],
            is_adult_content: false,
            is_free: false,
            price_text: Some("$9.99".to_string()),
            discount_percent: None,
            positive_review_pct: Some(58.0),
            total_reviews: Some(120),
            current_players: Some(8),
            recommendation_score: 10.0,
            ai_score: None,
            ai_summary: "delete".to_string(),
            capsule_url: "https://example.com/delete.jpg".to_string(),
            store_screenshot_urls: vec![],
            tags: vec!["Co-op".to_string()],
            multiplayer_modes: vec!["Online Co-op".to_string()],
            review_snippets: Vec::<ReviewSnippet>::new(),
            user_state: UserGameState::default(),
        };
        upsert_game(&conn, &game).expect("insert game");
        conn.execute(
            "INSERT INTO sync_queue (appid, mode, attempt, created_at, updated_at) VALUES (?1, 'full', 1, '2026-05-05T10:00:00Z', '2026-05-05T10:00:00Z')",
            [game.appid],
        )
        .expect("seed sync queue");
        conn.execute(
            "INSERT INTO metadata_backfill_queue (appid, attempt, created_at, updated_at) VALUES (?1, 1, '2026-05-05T10:00:00Z', '2026-05-05T10:00:00Z')",
            [game.appid],
        )
        .expect("seed backfill queue");
        conn.execute(
            "INSERT INTO ai_analysis_queue (appid, attempt, created_at, updated_at) VALUES (?1, 1, '2026-05-05T10:00:00Z', '2026-05-05T10:00:00Z')",
            [game.appid],
        )
        .expect("seed ai queue");
        conn.execute(
            "INSERT INTO analysis_narrative_cache (cache_key, appid, score_version, base_url, model, narrative_json, created_at, updated_at) VALUES ('k', ?1, 'v1', 'u', 'm', '{}', '2026-05-05T10:00:00Z', '2026-05-05T10:00:00Z')",
            [game.appid],
        )
        .expect("seed narrative cache");

        delete_game_and_related_state(&conn, game.appid).expect("delete related state");

        let games_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM games WHERE appid = ?1", [game.appid], |row| row.get(0))
            .expect("count games");
        let sync_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sync_queue WHERE appid = ?1", [game.appid], |row| row.get(0))
            .expect("count sync queue");
        let backfill_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_backfill_queue WHERE appid = ?1", [game.appid], |row| row.get(0))
            .expect("count backfill queue");
        let ai_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_analysis_queue WHERE appid = ?1", [game.appid], |row| row.get(0))
            .expect("count ai queue");
        let cache_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM analysis_narrative_cache WHERE appid = ?1", [game.appid], |row| row.get(0))
            .expect("count cache");

        assert_eq!(games_count, 0);
        assert_eq!(sync_count, 0);
        assert_eq!(backfill_count, 0);
        assert_eq!(ai_count, 0);
        assert_eq!(cache_count, 0);
    }

    #[test]
    fn migration_marks_classic_v2_cleanup_version() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        migrate(&conn).expect("migrate");

        assert_eq!(
            get_config(&conn, CLASSIC_DISCOVERY_CLEANUP_VERSION_CONFIG_KEY)
                .expect("load cleanup version"),
            Some(CLASSIC_DISCOVERY_RULE_VERSION.to_string())
        );
    }

    #[test]
    fn classic_discovery_due_respects_last_started_time_when_previous_run_never_completed() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        migrate(&conn).expect("migrate");
        set_config(
            &conn,
            CLASSIC_DISCOVERY_LAST_STARTED_AT_CONFIG_KEY,
            "2026-05-05T12:00:00Z",
        )
        .expect("seed last started");
        set_config(
            &conn,
            CLASSIC_DISCOVERY_LAST_RUN_STATUS_CONFIG_KEY,
            "cancelled",
        )
        .expect("seed last run status");

        let blocked = classic_discovery_is_due(&conn, "2026-05-06T11:59:59Z")
            .expect("evaluate blocked cooldown");
        let allowed = classic_discovery_is_due(&conn, "2026-05-06T12:00:00Z")
            .expect("evaluate allowed cooldown");

        assert!(!blocked);
        assert!(allowed);
    }

    #[test]
    fn classic_discovery_progress_persists_last_offset() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        migrate(&conn).expect("migrate");
        let snapshot = super::create_classic_discovery_run(&conn, 3, 2).expect("create run");

        super::update_classic_discovery_run_progress(
            &conn,
            snapshot.id,
            super::ClassicDiscoveryProgressPatch {
                pages_processed: Some(5),
                ..Default::default()
            },
        )
        .expect("update progress");

        assert_eq!(
            get_config(&conn, CLASSIC_DISCOVERY_LAST_OFFSET_CONFIG_KEY).expect("load last offset"),
            Some("5".to_string())
        );
    }
}
