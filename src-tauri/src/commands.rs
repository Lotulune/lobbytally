use crate::backfill_task;
use crate::db;
use crate::discovery::{
    build_discovered_game_card, clamp_discovery_page_size, clamp_discovery_pages,
    clamp_discovery_target_added_games, store_search_start_for_page, SteamDiscoveryReport,
    DISCOVERY_CURSOR_CONFIG_KEY,
};
use crate::discovery_task::{emit_snapshot, spawn_discovery_worker, DiscoveryControl};
use crate::llm::{self, LlmRuntimeConfig};
use crate::models::{
    AiAssessment, DashboardPayload, DiscoveryRunSnapshot, DiscoveryRunStatus, DiscoveryTaskRequest,
    PublicConfig, SaveConfigRequest, SyncMode, SyncReport, SyncRequest, UserCollections,
    UserGameState, UserGameStatePatch,
};
use crate::recommendation::{bucket_game, ReleaseBucket};
use crate::state::AppState;
use crate::steam::{self, SteamGameSnapshot};
use crate::sync_task;
use std::collections::HashSet;
use tauri::{AppHandle, Manager, State};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[tauri::command]
pub fn get_dashboard(state: State<'_, AppState>) -> Result<DashboardPayload, String> {
    let mut payload = {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        db::load_dashboard(&conn).map_err(to_command_error)?
    };
    let persisted_sync_pending_count = payload.stats.sync_pending_count;
    let persisted_sync_mode = payload.stats.sync_mode;
    let persisted_sync_total_count = payload.stats.sync_total_count;
    let persisted_sync_last_error = payload.stats.sync_last_error.clone();
    let persisted_sync_last_error_appid = payload.stats.sync_last_error_appid;
    let persisted_pending_count = payload.stats.backfill_pending_count;
    let sync = state.sync.lock().map_err(|err| err.to_string())?.snapshot();
    payload.stats.sync_running = sync.running;
    payload.stats.sync_pending_count = persisted_sync_pending_count.max(sync.pending_count);
    payload.stats.sync_mode = sync.mode.or(persisted_sync_mode);
    payload.stats.sync_current_appid = sync.current_appid;
    payload.stats.sync_total_count = if sync.total_count == 0 {
        persisted_sync_total_count
    } else {
        sync.total_count
    };
    payload.stats.sync_processed_count = sync.processed_count;
    payload.stats.sync_updated_count = sync.updated_count;
    payload.stats.sync_failed_count = sync.failed_count;
    payload.stats.sync_last_error = sync.last_error.or(persisted_sync_last_error);
    payload.stats.sync_last_error_appid = sync.last_error_appid.or(persisted_sync_last_error_appid);
    let backfill = state
        .backfill
        .lock()
        .map_err(|err| err.to_string())?
        .snapshot();
    payload.stats.backfill_pending_count = persisted_pending_count.max(backfill.pending_count);
    payload.stats.backfill_running = backfill.running;
    payload.stats.backfill_current_appid = backfill.current_appid;
    payload.stats.backfill_current_attempt = backfill.current_attempt;
    payload.stats.backfill_total_count = if backfill.total_count == 0 {
        payload.stats.backfill_pending_count
    } else {
        backfill.total_count
    };
    payload.stats.backfill_processed_count = backfill.processed_count;
    payload.stats.backfill_failed_count = backfill.failed_count;
    Ok(payload)
}

#[tauri::command]
pub fn save_config(
    state: State<'_, AppState>,
    request: SaveConfigRequest,
) -> Result<PublicConfig, String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;

    if let Some(value) = request
        .steam_api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        db::set_config(&conn, "steam_api_key", value).map_err(to_command_error)?;
    }
    if let Some(value) = request
        .llm_api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        db::set_config(&conn, "llm_api_key", value).map_err(to_command_error)?;
    }
    if let Some(value) = request
        .llm_base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        db::set_config(&conn, "llm_base_url", value).map_err(to_command_error)?;
    }
    if let Some(value) = request
        .llm_model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        db::set_config(&conn, "llm_model", value).map_err(to_command_error)?;
    }
    if let Some(value) = request
        .country
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        db::set_config(&conn, "country", value).map_err(to_command_error)?;
    }
    if let Some(value) = request
        .language
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        db::set_config(&conn, "language", value).map_err(to_command_error)?;
    }

    db::public_config(&conn).map_err(to_command_error)
}

#[tauri::command]
pub fn sync_seed_games(
    app: AppHandle,
    state: State<'_, AppState>,
    request: Option<SyncRequest>,
) -> Result<SyncReport, String> {
    let requested_mode = request
        .map(|request| request.mode)
        .unwrap_or(SyncMode::Full);
    let (jobs, mode, resumed_queue, upgraded_queue) = {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        if let Some(summary) = db::sync_queue_summary(&conn).map_err(to_command_error)? {
            let upgraded_queue =
                summary.mode == SyncMode::Quick && requested_mode == SyncMode::Full;
            let mode = if upgraded_queue {
                db::update_all_sync_job_modes(&conn, SyncMode::Full).map_err(to_command_error)?;
                SyncMode::Full
            } else {
                summary.mode
            };
            (
                sync_task::sync_jobs_from_records(
                    db::list_sync_jobs(&conn).map_err(to_command_error)?,
                ),
                mode,
                true,
                upgraded_queue,
            )
        } else {
            let appids = db::list_game_appids(&conn).map_err(to_command_error)?;
            if appids.is_empty() {
                return Ok(SyncReport {
                    updated_games: 0,
                    failed_games: 0,
                    message: "当前库为空，没有可同步的游戏。".to_string(),
                });
            }

            db::replace_sync_jobs(&conn, appids, requested_mode).map_err(to_command_error)?;
            (
                sync_task::sync_jobs_from_records(
                    db::list_sync_jobs(&conn).map_err(to_command_error)?,
                ),
                requested_mode,
                false,
                false,
            )
        }
    };

    if jobs.is_empty() {
        return Ok(SyncReport {
            updated_games: 0,
            failed_games: 0,
            message: "当前库为空，没有可同步的游戏。".to_string(),
        });
    }

    let total_count = jobs.len();
    let (started, current_snapshot) = {
        let mut runtime = state.sync.lock().map_err(|err| err.to_string())?;
        let snapshot = runtime.snapshot();
        if snapshot.running {
            (false, snapshot)
        } else {
            let started = runtime.start(jobs, mode);
            (started, runtime.snapshot())
        }
    };

    if !started {
        return Ok(SyncReport {
            updated_games: current_snapshot.updated_count,
            failed_games: current_snapshot.failed_count,
            message: format!(
                "Steam {}任务已在运行：已处理 {}/{}，成功 {}，失败 {}。",
                sync_mode_label(current_snapshot.mode.unwrap_or(SyncMode::Full)),
                current_snapshot.processed_count,
                current_snapshot.total_count,
                current_snapshot.updated_count,
                current_snapshot.failed_count
            ),
        });
    }

    sync_task::spawn_sync_worker(app);

    Ok(SyncReport {
        updated_games: 0,
        failed_games: 0,
        message: format!(
            "{} Steam {}{}，当前共有 {total_count} 个库内游戏待处理。",
            if resumed_queue {
                "已继续"
            } else {
                "已启动"
            },
            sync_mode_label(mode),
            if upgraded_queue {
                "（已从快速同步升级）"
            } else {
                ""
            }
        ),
    })
}

#[tauri::command]
pub async fn assess_game_with_ai(
    state: State<'_, AppState>,
    appid: u32,
) -> Result<AiAssessment, String> {
    let (game, config) = {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        let game = db::load_game(&conn, appid)
            .map_err(to_command_error)?
            .ok_or_else(|| format!("未找到 Steam App {appid}"))?;
        let config = LlmRuntimeConfig {
            api_key: db::get_secret(&conn, "llm_api_key").map_err(to_command_error)?,
            base_url: db::get_config(&conn, "llm_base_url")
                .map_err(to_command_error)?
                .unwrap_or_else(|| "https://api.openai.com".to_string()),
            model: db::get_config(&conn, "llm_model")
                .map_err(to_command_error)?
                .unwrap_or_else(|| "gpt-4.1-mini".to_string()),
        };
        (game, config)
    };

    let assessment = llm::assess_game(&state.http, &config, &game)
        .await
        .map_err(to_command_error)?;

    {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        if let Some(mut existing) = db::load_game(&conn, appid).map_err(to_command_error)? {
            existing.ai_score = Some(assessment.score);
            existing.ai_summary = assessment.summary.clone();
            existing.recommendation_score = db::score_card(&existing);
            db::upsert_game(&conn, &existing).map_err(to_command_error)?;
        }
    }

    Ok(assessment)
}

#[tauri::command]
pub async fn preview_steam_app_list(
    state: State<'_, AppState>,
    max_results: Option<u32>,
    last_appid: Option<u32>,
) -> Result<steam::SteamAppListPreview, String> {
    let key = {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        db::get_secret(&conn, "steam_api_key")
            .map_err(to_command_error)?
            .ok_or_else(|| "请先在设置中配置 Steam Web API Key。".to_string())?
    };

    steam::fetch_app_list_preview(
        &state.http,
        &key,
        max_results.unwrap_or(50).clamp(1, 500),
        last_appid,
    )
    .await
    .map_err(to_command_error)
}

#[tauri::command]
pub async fn discover_steam_games(
    app: AppHandle,
    state: State<'_, AppState>,
    max_pages: Option<u32>,
    page_size: Option<u32>,
    start_appid: Option<u32>,
) -> Result<SteamDiscoveryReport, String> {
    let max_pages = clamp_discovery_pages(max_pages);
    let page_size = clamp_discovery_page_size(page_size);
    let _legacy_start_appid = start_appid;
    let today = crate::recommendation::today_iso_utc();

    let (country, language, existing_appids) = {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        let config = db::public_config(&conn).map_err(to_command_error)?;
        let existing_appids = db::list_game_appids(&conn).map_err(to_command_error)?;
        (config.country, config.language, existing_appids)
    };

    let mut known_appids = existing_appids.into_iter().collect::<HashSet<_>>();
    let mut report = SteamDiscoveryReport::new();
    let mut backfill_appids = Vec::new();

    for page in 0..max_pages {
        let start = store_search_start_for_page(page, page_size);
        let preview =
            steam::fetch_store_search_candidates(&state.http, start, page_size, &language)
                .await
                .map_err(to_command_error)?;

        report.scanned_apps += preview.apps.len();
        report.have_more_results = preview.have_more_results;

        for app in &preview.apps {
            if known_appids.contains(&app.appid) {
                report.skipped_existing += 1;
                continue;
            }

            match steam::fetch_game_snapshot(
                &state.http,
                app.appid,
                &country,
                &language,
                steam::SteamGameSnapshotEnrichment::Discovery,
            )
            .await
            {
                Ok(snapshot) => {
                    if let Some(card) = build_discovered_game_card(app, snapshot, &today) {
                        match card.section.as_str() {
                            "new" => report.added_new_games += 1,
                            _ => report.added_classic_games += 1,
                        }
                        {
                            let conn = state.db.lock().map_err(|err| err.to_string())?;
                            db::upsert_game(&conn, &card).map_err(to_command_error)?;
                        }
                        known_appids.insert(app.appid);
                        backfill_appids.push(app.appid);
                        report.added_games += 1;
                    } else {
                        report.skipped_non_multiplayer += 1;
                    }
                }
                Err(_) => {
                    report.failed_games += 1;
                }
            }
        }

        report.last_appid = preview.apps.last().map(|app| app.appid);
        if !report.have_more_results || preview.apps.is_empty() {
            break;
        }
    }

    {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        if let Some(last_appid) = report.last_appid {
            db::set_config(&conn, DISCOVERY_CURSOR_CONFIG_KEY, &last_appid.to_string())
                .map_err(to_command_error)?;
        }
        db::mark_sync_complete(&conn).map_err(to_command_error)?;
    }
    if !backfill_appids.is_empty() {
        backfill_task::enqueue_backfill(&app, backfill_appids).map_err(to_command_error)?;
    }

    report.finish_message();
    Ok(report)
}

#[tauri::command]
pub fn get_discovery_task_snapshot(app: AppHandle) -> Result<Option<DiscoveryRunSnapshot>, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    db::load_latest_discovery_run(&conn).map_err(to_command_error)
}

#[tauri::command]
pub fn list_discovery_task_history(
    app: AppHandle,
    limit: Option<u32>,
) -> Result<Vec<DiscoveryRunSnapshot>, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    let mut runs = db::list_discovery_runs(&conn).map_err(to_command_error)?;
    let limit = limit.unwrap_or(8) as usize;
    if runs.len() > limit {
        runs.truncate(limit);
    }
    Ok(runs)
}

#[tauri::command]
pub fn start_discovery_task(
    app: AppHandle,
    request: DiscoveryTaskRequest,
) -> Result<DiscoveryRunSnapshot, String> {
    let state = app.state::<AppState>();
    let mut runtime = state.discovery.lock().map_err(|err| err.to_string())?;
    if runtime.active_run_id.is_some() {
        return Err("当前已有发现任务正在运行。".to_string());
    }

    let conn = state.db.lock().map_err(|err| err.to_string())?;
    if let Some(latest) = db::load_latest_discovery_run(&conn).map_err(to_command_error)? {
        if latest.status == DiscoveryRunStatus::Running {
            return Err("当前已有发现任务正在运行。".to_string());
        }
        if latest.can_resume() {
            db::update_discovery_run_progress(
                &conn,
                latest.id,
                db::DiscoveryProgressPatch {
                    status: Some(DiscoveryRunStatus::Cancelled),
                    current_appid: Some(None),
                    last_error: Some(None),
                    finished_at: Some(Some(now_rfc3339().map_err(to_command_error)?)),
                    ..Default::default()
                },
            )
            .map_err(to_command_error)?;
        }
    }

    let normalized_request = DiscoveryTaskRequest {
        sync_mode: request.sync_mode,
        target_added_games: clamp_discovery_target_added_games(Some(request.target_added_games)),
        page_size: clamp_discovery_page_size(Some(request.page_size)),
    };
    let snapshot =
        db::create_discovery_run(&conn, &normalized_request, None).map_err(to_command_error)?;
    runtime.active_run_id = Some(snapshot.id);
    runtime.control = DiscoveryControl::None;
    drop(conn);
    drop(runtime);

    emit_snapshot(&app, &snapshot);
    spawn_discovery_worker(app, snapshot.id);
    Ok(snapshot)
}

#[tauri::command]
pub fn pause_discovery_task(app: AppHandle) -> Result<DiscoveryRunSnapshot, String> {
    let state = app.state::<AppState>();
    let mut runtime = state.discovery.lock().map_err(|err| err.to_string())?;
    let run_id = runtime
        .active_run_id
        .ok_or_else(|| "当前没有正在运行的发现任务。".to_string())?;
    runtime.control = DiscoveryControl::PauseRequested;

    let conn = state.db.lock().map_err(|err| err.to_string())?;
    let snapshot = db::load_discovery_run(&conn, run_id)
        .map_err(to_command_error)?
        .ok_or_else(|| "当前没有可暂停的发现任务。".to_string())?;
    Ok(snapshot)
}

#[tauri::command]
pub fn resume_discovery_task(app: AppHandle) -> Result<DiscoveryRunSnapshot, String> {
    let state = app.state::<AppState>();
    let mut runtime = state.discovery.lock().map_err(|err| err.to_string())?;
    if runtime.active_run_id.is_some() {
        return Err("当前已有发现任务正在运行。".to_string());
    }

    let conn = state.db.lock().map_err(|err| err.to_string())?;
    let latest = db::load_latest_discovery_run(&conn)
        .map_err(to_command_error)?
        .ok_or_else(|| "当前没有可恢复的发现任务。".to_string())?;
    if !latest.can_resume() {
        return Err("最近一次发现任务不处于可恢复状态。".to_string());
    }

    db::update_discovery_run_progress(
        &conn,
        latest.id,
        db::DiscoveryProgressPatch {
            status: Some(DiscoveryRunStatus::Running),
            current_appid: Some(None),
            last_error: Some(None),
            finished_at: Some(None),
            ..Default::default()
        },
    )
    .map_err(to_command_error)?;
    let snapshot = db::load_discovery_run(&conn, latest.id)
        .map_err(to_command_error)?
        .ok_or_else(|| "发现任务恢复后无法重新载入。".to_string())?;
    runtime.active_run_id = Some(snapshot.id);
    runtime.control = DiscoveryControl::None;
    drop(conn);
    drop(runtime);

    emit_snapshot(&app, &snapshot);
    spawn_discovery_worker(app, snapshot.id);
    Ok(snapshot)
}

#[tauri::command]
pub fn cancel_discovery_task(app: AppHandle) -> Result<DiscoveryRunSnapshot, String> {
    let state = app.state::<AppState>();
    let mut runtime = state.discovery.lock().map_err(|err| err.to_string())?;

    if let Some(run_id) = runtime.active_run_id {
        runtime.control = DiscoveryControl::CancelRequested;
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        let snapshot = db::load_discovery_run(&conn, run_id)
            .map_err(to_command_error)?
            .ok_or_else(|| "当前没有可取消的发现任务。".to_string())?;
        return Ok(snapshot);
    }

    let conn = state.db.lock().map_err(|err| err.to_string())?;
    let latest = db::load_latest_discovery_run(&conn)
        .map_err(to_command_error)?
        .ok_or_else(|| "当前没有可取消的发现任务。".to_string())?;
    if !matches!(
        latest.status,
        DiscoveryRunStatus::Paused | DiscoveryRunStatus::Interrupted
    ) {
        return Err("最近一次发现任务不处于可取消状态。".to_string());
    }

    let finished_at = now_rfc3339().map_err(to_command_error)?;
    db::update_discovery_run_progress(
        &conn,
        latest.id,
        db::DiscoveryProgressPatch {
            status: Some(DiscoveryRunStatus::Cancelled),
            current_appid: Some(None),
            last_error: Some(None),
            finished_at: Some(Some(finished_at)),
            ..Default::default()
        },
    )
    .map_err(to_command_error)?;
    db::mark_sync_complete(&conn).map_err(to_command_error)?;
    let snapshot = db::load_discovery_run(&conn, latest.id)
        .map_err(to_command_error)?
        .ok_or_else(|| "发现任务取消后无法重新载入。".to_string())?;
    runtime.active_run_id = None;
    runtime.control = DiscoveryControl::None;
    drop(conn);
    drop(runtime);

    emit_snapshot(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
pub fn set_game_user_state(
    state: State<'_, AppState>,
    appid: u32,
    patch: UserGameStatePatch,
) -> Result<UserGameState, String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    db::set_game_user_state(&conn, appid, patch).map_err(to_command_error)
}

#[tauri::command]
pub fn get_user_collections(state: State<'_, AppState>) -> Result<UserCollections, String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    db::load_user_collections(&conn).map_err(to_command_error)
}

pub(crate) fn merge_snapshot(
    mut existing: crate::models::GameCard,
    snapshot: SteamGameSnapshot,
) -> crate::models::GameCard {
    if let Some(name) = snapshot.name {
        existing.name = name;
    }
    if let Some(short_description) = snapshot
        .short_description
        .filter(|text| !text.trim().is_empty())
    {
        existing.short_description = Some(short_description);
    }
    if let Some(release_date) = snapshot.release_date {
        existing.release_date = Some(release_date);
    }
    if let Some(release_date_text) = snapshot.release_date_text {
        existing.release_date_text = release_date_text;
    }
    if let Some(release_state) = snapshot.release_state {
        existing.release_state = release_state;
    }
    existing.demo_status = snapshot.demo_status;
    if let Some(supported_languages) = snapshot.supported_languages {
        existing.supported_languages = supported_languages;
    }
    if let Some(is_adult_content) = snapshot.is_adult_content {
        existing.is_adult_content = is_adult_content;
    }
    if let Some(price_text) = snapshot.price_text.filter(|text| !text.trim().is_empty()) {
        existing.price_text = Some(price_text);
    }
    if let Some(discount_percent) = snapshot.discount_percent {
        existing.discount_percent = Some(discount_percent);
    }
    existing.positive_review_pct = snapshot
        .positive_review_pct
        .or(existing.positive_review_pct);
    existing.total_reviews = snapshot.total_reviews.or(existing.total_reviews);
    existing.current_players = snapshot.current_players.or(existing.current_players);
    if let Some(capsule_url) = snapshot.capsule_url {
        existing.capsule_url = capsule_url;
    }
    if !snapshot.store_screenshot_urls.is_empty() {
        existing.store_screenshot_urls = snapshot.store_screenshot_urls;
    }
    if !snapshot.tags.is_empty() {
        existing.tags = snapshot.tags;
    }
    if !snapshot.multiplayer_modes.is_empty() {
        existing.multiplayer_modes = snapshot.multiplayer_modes;
    }
    if !snapshot.review_snippets.is_empty() {
        existing.review_snippets = snapshot.review_snippets;
    }

    let facts = db::facts_from_card(&existing);
    existing.section = match bucket_game(&facts, &crate::recommendation::today_iso_utc()) {
        ReleaseBucket::New => "new".to_string(),
        ReleaseBucket::Classic => "classic".to_string(),
    };
    existing.recommendation_score = db::score_card(&existing);
    existing
}

fn to_command_error(error: anyhow::Error) -> String {
    error.to_string()
}

fn now_rfc3339() -> anyhow::Result<String> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

fn sync_mode_label(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Quick => "快速同步",
        SyncMode::Full => "完整同步",
    }
}
