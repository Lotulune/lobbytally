use crate::ai_batch_refresh_task;
use crate::backfill_task;
use crate::classic_discovery_task;
use crate::db;
use crate::discovery::{
    build_discovered_game_card, clamp_discovery_page_size, clamp_discovery_pages,
    clamp_discovery_target_added_games, store_search_start_for_page, SteamDiscoveryReport,
    DISCOVERY_CURSOR_CONFIG_KEY,
};
use crate::discovery_task::{emit_snapshot, spawn_discovery_worker, DiscoveryControl};
use crate::game_analysis;
use crate::llm::{self, LlmRuntimeConfig};
use crate::models::{
    AiAnalysisQueueSource, AiAssessment, AiBatchRefreshReport, ClassicDiscoveryRunSnapshot,
    ClassicDiscoveryTaskRequest, DashboardPayload, DiscoveryRunSnapshot, DiscoveryRunStatus,
    DiscoveryTaskRequest, GameAnalysisReport, PublicConfig, SaveConfigRequest, SyncMode,
    SyncReport, SyncRequest, UserCollections, UserGameState, UserGameStatePatch,
};
use crate::recommendation::{bucket_game, ReleaseBucket};
use crate::state::AppState;
use crate::steam::{self, SteamGameSnapshot};
use crate::sync_task;
use std::collections::HashSet;
use tauri::{AppHandle, Manager, State};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[cfg(test)]
use futures::{stream, StreamExt};

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
    let persisted_classic_status = payload.stats.classic_discovery_status.clone();
    let persisted_ai_batch_refresh_total_count = payload.stats.ai_batch_refresh_total_count;
    let persisted_ai_batch_refresh_processed_count = payload.stats.ai_batch_refresh_processed_count;
    let persisted_ai_batch_refresh_updated_count = payload.stats.ai_batch_refresh_updated_count;
    let persisted_ai_batch_refresh_failed_count = payload.stats.ai_batch_refresh_failed_count;
    let persisted_ai_batch_refresh_last_error = payload.stats.ai_batch_refresh_last_error.clone();
    let persisted_ai_batch_refresh_last_error_appid =
        payload.stats.ai_batch_refresh_last_error_appid;
    let persisted_ai_batch_refresh_concurrency = payload.stats.ai_batch_refresh_concurrency;
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
    let classic = state
        .classic_discovery
        .lock()
        .map_err(|err| err.to_string())?
        .snapshot();
    payload.stats.classic_discovery_running = classic.running;
    if classic.running {
        payload.stats.classic_discovery_status = Some(DiscoveryRunStatus::Running);
    } else {
        payload.stats.classic_discovery_status = persisted_classic_status;
    }
    let ai_batch_refresh = state
        .ai_batch_refresh
        .lock()
        .map_err(|err| err.to_string())?
        .snapshot();
    payload.stats.ai_batch_refresh_running = ai_batch_refresh.running;
    payload.stats.ai_batch_refresh_concurrency = visible_ai_batch_refresh_concurrency(
        &ai_batch_refresh,
        persisted_ai_batch_refresh_concurrency,
    );
    payload.stats.ai_batch_refresh_pending_count = ai_batch_refresh.pending_count;
    payload.stats.ai_batch_refresh_active_count = ai_batch_refresh.active_count;
    payload.stats.ai_batch_refresh_total_count =
        persisted_ai_batch_refresh_total_count.max(ai_batch_refresh.total_count);
    payload.stats.ai_batch_refresh_processed_count =
        persisted_ai_batch_refresh_processed_count.max(ai_batch_refresh.processed_count);
    payload.stats.ai_batch_refresh_updated_count =
        persisted_ai_batch_refresh_updated_count.max(ai_batch_refresh.updated_count);
    payload.stats.ai_batch_refresh_failed_count =
        persisted_ai_batch_refresh_failed_count.max(ai_batch_refresh.failed_count);
    payload.stats.ai_batch_refresh_last_error = ai_batch_refresh
        .last_error
        .clone()
        .or(persisted_ai_batch_refresh_last_error);
    payload.stats.ai_batch_refresh_last_error_appid = ai_batch_refresh
        .last_error_appid
        .or(persisted_ai_batch_refresh_last_error_appid);
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
    if let Some(value) = request.ai_batch_refresh_concurrency {
        db::set_config(
            &conn,
            "ai_batch_refresh_concurrency",
            &db::clamp_ai_batch_refresh_concurrency(value).to_string(),
        )
        .map_err(to_command_error)?;
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
    generate_assessment_from_report_pipeline(state.inner(), appid)
        .await
        .map_err(to_command_error)
}

#[tauri::command]
pub fn get_game_analysis(
    state: State<'_, AppState>,
    appid: u32,
) -> Result<Option<GameAnalysisReport>, String> {
    load_cached_game_analysis(state.inner(), appid).map_err(to_command_error)
}

#[tauri::command]
pub async fn generate_game_analysis(
    state: State<'_, AppState>,
    appid: u32,
    force_refresh: Option<bool>,
) -> Result<GameAnalysisReport, String> {
    generate_or_load_game_analysis(state.inner(), appid, force_refresh.unwrap_or(false))
        .await
        .map_err(to_command_error)
}

#[tauri::command]
pub fn refresh_all_game_analyses(
    app: AppHandle,
    state: State<'_, AppState>,
    concurrency: Option<u8>,
) -> Result<AiBatchRefreshReport, String> {
    let concurrency =
        resolve_batch_refresh_concurrency(state.inner(), concurrency).map_err(to_command_error)?;

    let appids = {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        db::list_game_appids(&conn).map_err(to_command_error)?
    };
    if appids.is_empty() {
        return Ok(AiBatchRefreshReport {
            total_games: 0,
            updated_games: 0,
            failed_games: 0,
            message: "当前库为空，没有可重算的 AI 评分。".to_string(),
        });
    }

    if let Some(existing_snapshot) =
        start_ai_batch_refresh_runtime(state.inner(), 0, concurrency).map_err(to_command_error)?
    {
        return Ok(running_ai_batch_refresh_report(&existing_snapshot));
    }
    {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        enqueue_full_refresh_ai_jobs(&conn).map_err(to_command_error)?;
    }
    ai_batch_refresh_task::spawn_ai_batch_refresh_worker(app, concurrency);

    Ok(AiBatchRefreshReport {
        total_games: appids.len(),
        updated_games: 0,
        failed_games: 0,
        message: format!(
            "已启动 AI 批量重算：共 {} 款游戏，当前并发 {}。",
            appids.len(),
            concurrency
        ),
    })
}

#[tauri::command]
pub fn retry_ai_analysis_job(
    app: AppHandle,
    state: State<'_, AppState>,
    appid: u32,
) -> Result<AiBatchRefreshReport, String> {
    {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        let source = db::load_game(&conn, appid)
            .map_err(to_command_error)?
            .map(|game| ai_analysis_source_for_section(game.section.as_str()))
            .unwrap_or(AiAnalysisQueueSource::Classic);
        if db::load_ai_analysis_queue_job(&conn, appid)
            .map_err(to_command_error)?
            .is_none()
        {
            db::enqueue_ai_analysis_jobs(&conn, source, [appid]).map_err(to_command_error)?;
        }
        db::update_ai_analysis_queue_job(&conn, appid, 1, None).map_err(to_command_error)?;
    }
    let concurrency =
        resolve_batch_refresh_concurrency(state.inner(), None).map_err(to_command_error)?;
    ai_batch_refresh_task::start_ai_batch_refresh_worker_if_idle(&app, concurrency)
        .map_err(to_command_error)?;
    Ok(AiBatchRefreshReport {
        total_games: 1,
        updated_games: 0,
        failed_games: 0,
        message: format!("已重新加入 AppID {appid} 的 AI 分析队列。"),
    })
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
pub fn get_classic_discovery_task_snapshot(
    app: AppHandle,
) -> Result<Option<ClassicDiscoveryRunSnapshot>, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    db::load_latest_classic_discovery_run(&conn).map_err(to_command_error)
}

#[tauri::command]
pub fn list_classic_discovery_task_history(
    app: AppHandle,
    limit: Option<u32>,
) -> Result<Vec<ClassicDiscoveryRunSnapshot>, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    let mut runs = db::list_classic_discovery_runs(&conn).map_err(to_command_error)?;
    let limit = limit.unwrap_or(8) as usize;
    if runs.len() > limit {
        runs.truncate(limit);
    }
    Ok(runs)
}

#[tauri::command]
pub fn start_classic_discovery_task(
    app: AppHandle,
    request: ClassicDiscoveryTaskRequest,
) -> Result<ClassicDiscoveryRunSnapshot, String> {
    let state = app.state::<AppState>();
    let mut runtime = state
        .classic_discovery
        .lock()
        .map_err(|err| err.to_string())?;
    if runtime.active_run_id.is_some() {
        return Err("当前已有精品老游补库任务正在运行。".to_string());
    }

    let conn = state.db.lock().map_err(|err| err.to_string())?;
    if let Some(latest) = db::load_latest_classic_discovery_run(&conn).map_err(to_command_error)? {
        if latest.status == DiscoveryRunStatus::Running {
            return Err("当前已有精品老游补库任务正在运行。".to_string());
        }
    }
    let max_pages = request
        .max_pages
        .unwrap_or(db::CLASSIC_DISCOVERY_MAX_PAGES_DEFAULT)
        .clamp(1, db::CLASSIC_DISCOVERY_MAX_PAGES_DEFAULT);
    let start_offset =
        resolve_manual_classic_discovery_start_offset(&conn).map_err(to_command_error)?;
    let end_page = start_offset.saturating_add(max_pages);
    let snapshot = db::create_classic_discovery_run(&conn, end_page, start_offset)
        .map_err(to_command_error)?;
    runtime.active_run_id = Some(snapshot.id);
    runtime.control = classic_discovery_task::ClassicDiscoveryControl::None;
    drop(conn);
    drop(runtime);

    classic_discovery_task::emit_snapshot(&app, &snapshot);
    classic_discovery_task::spawn_classic_discovery_worker(app, snapshot.id);
    Ok(snapshot)
}

fn resolve_manual_classic_discovery_start_offset(
    conn: &rusqlite::Connection,
) -> anyhow::Result<u32> {
    Ok(db::get_config(conn, db::CLASSIC_DISCOVERY_LAST_OFFSET_CONFIG_KEY)?
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0))
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
    if let Some(is_free) = snapshot.is_free {
        existing.is_free = is_free;
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
        ReleaseBucket::ClassicHidden => {
            if existing.section == "classic" {
                "classic".to_string()
            } else {
                "classic_hidden".to_string()
            }
        }
    };
    existing.recommendation_score = db::score_card(&existing);
    existing
}

fn to_command_error(error: anyhow::Error) -> String {
    error.to_string()
}

fn ai_analysis_source_for_section(section: &str) -> AiAnalysisQueueSource {
    if section == "new" {
        AiAnalysisQueueSource::NewRelease
    } else {
        AiAnalysisQueueSource::Classic
    }
}

fn enqueue_full_refresh_ai_jobs(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    let games = db::list_game_appids_with_sections(conn)?;
    let mut new_release = Vec::new();
    let mut classic = Vec::new();

    conn.execute("DELETE FROM ai_analysis_queue", [])?;

    for (appid, section) in games {
        match ai_analysis_source_for_section(section.as_str()) {
            AiAnalysisQueueSource::NewRelease => new_release.push(appid),
            AiAnalysisQueueSource::Classic => classic.push(appid),
        }
    }

    if !new_release.is_empty() {
        db::enqueue_ai_analysis_jobs(conn, AiAnalysisQueueSource::NewRelease, new_release)?;
    }
    if !classic.is_empty() {
        db::enqueue_ai_analysis_jobs(conn, AiAnalysisQueueSource::Classic, classic)?;
    }

    Ok(())
}

fn load_cached_game_analysis(
    state: &AppState,
    appid: u32,
) -> anyhow::Result<Option<GameAnalysisReport>> {
    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    db::load_game_analysis(&conn, appid)
}

fn summarize_analysis_report_as_assessment(report: GameAnalysisReport) -> AiAssessment {
    game_analysis::summarize_report_as_assessment(&report)
}

async fn generate_assessment_from_report_pipeline(
    state: &AppState,
    appid: u32,
) -> anyhow::Result<AiAssessment> {
    let report = generate_or_load_game_analysis(state, appid, true).await?;
    Ok(summarize_analysis_report_as_assessment(report))
}

async fn generate_game_analysis_with_narrative_cache(
    state: &AppState,
    config: &LlmRuntimeConfig,
    game: &crate::models::GameCard,
    generated_at: String,
) -> anyhow::Result<GameAnalysisReport> {
    let rule_report = game_analysis::build_rule_report(game, generated_at)?;
    if config.api_key.is_none() {
        return Ok(rule_report);
    }

    let cache_key = llm::build_analysis_narrative_cache_key(config, game, &rule_report);
    if let Some(cached_narrative) = {
        let conn = state
            .db
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        db::load_analysis_narrative_cache(&conn, &cache_key)?
    } {
        return Ok(game_analysis::apply_narrative_patch(
            rule_report,
            cached_narrative,
        ));
    }

    match llm::generate_analysis_narrative(&state.http, config, game, &rule_report).await {
        Ok(narrative) => {
            let conn = state
                .db
                .lock()
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            db::save_analysis_narrative_cache(
                &conn,
                &cache_key,
                game.appid,
                &rule_report.score_version,
                config.base_url.trim(),
                config.model.trim(),
                &narrative,
            )?;
            Ok(game_analysis::apply_narrative_patch(rule_report, narrative))
        }
        Err(_) => Ok(rule_report),
    }
}

pub(crate) async fn generate_or_load_game_analysis(
    state: &AppState,
    appid: u32,
    force_refresh: bool,
) -> anyhow::Result<GameAnalysisReport> {
    let (game, config, cached_report) = {
        let conn = state
            .db
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        let game = db::load_game(&conn, appid)?
            .ok_or_else(|| anyhow::anyhow!("未找到 Steam App {appid}"))?;
        let config = load_llm_runtime_config(&conn)?;
        let cached_report = if force_refresh {
            None
        } else {
            db::load_game_analysis(&conn, appid)?
        };
        (game, config, cached_report)
    };

    if let Some(cached_report) = cached_report {
        return Ok(cached_report);
    }

    let report =
        generate_game_analysis_with_narrative_cache(state, &config, &game, now_rfc3339()?).await?;

    let mut updated_game = game;
    updated_game.ai_score = Some(report.recommendation_score);
    updated_game.ai_summary = report.overview.clone();
    updated_game.recommendation_score = report.recommendation_score;

    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    db::save_game_analysis(&conn, appid, &report)?;
    db::upsert_game(&conn, &updated_game)?;

    Ok(report)
}

#[cfg(test)]
async fn refresh_all_game_analyses_pipeline(
    state: &AppState,
    concurrency: u8,
) -> anyhow::Result<AiBatchRefreshReport> {
    let appids = {
        let conn = state
            .db
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        db::list_game_appids(&conn)?
    };

    if appids.is_empty() {
        return Ok(AiBatchRefreshReport {
            total_games: 0,
            updated_games: 0,
            failed_games: 0,
            message: "当前库为空，没有可重算的 AI 评分。".to_string(),
        });
    }

    let concurrency = db::clamp_ai_batch_refresh_concurrency(concurrency);
    let total_games = appids.len();
    let mut updated_games = 0usize;
    let mut failed_games = 0usize;
    let mut failure_samples = Vec::new();

    let mut refreshes = stream::iter(appids)
        .map(|appid| async move {
            (
                appid,
                generate_or_load_game_analysis(state, appid, true).await,
            )
        })
        .buffer_unordered(usize::from(concurrency));

    while let Some((appid, outcome)) = refreshes.next().await {
        match outcome {
            Ok(_) => updated_games += 1,
            Err(error) => {
                failed_games += 1;
                if failure_samples.len() < 3 {
                    failure_samples.push(format!("{appid}: {error}"));
                }
            }
        }
    }

    let message = if failed_games == 0 {
        format!("已按 {concurrency} 路并发重算 {updated_games} 款游戏的 AI 评分。")
    } else {
        let detail = failure_samples.join("；");
        format!(
            "已按 {concurrency} 路并发重算 {updated_games}/{total_games} 款游戏，失败 {failed_games} 款。{}",
            if detail.is_empty() {
                "请查看日志定位失败原因。".to_string()
            } else {
                format!("失败示例：{detail}")
            }
        )
    };

    Ok(AiBatchRefreshReport {
        total_games,
        updated_games,
        failed_games,
        message,
    })
}

fn resolve_batch_refresh_concurrency(
    state: &AppState,
    requested: Option<u8>,
) -> anyhow::Result<u8> {
    if let Some(value) = requested {
        return Ok(db::clamp_ai_batch_refresh_concurrency(value));
    }

    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    db::load_ai_batch_refresh_concurrency(&conn)
}

fn visible_ai_batch_refresh_concurrency(
    snapshot: &crate::ai_batch_refresh_task::AiBatchRefreshRuntimeSnapshot,
    persisted_concurrency: u8,
) -> u8 {
    if snapshot.concurrency > 0 {
        snapshot.concurrency
    } else if persisted_concurrency > 0 {
        persisted_concurrency
    } else {
        db::DEFAULT_AI_BATCH_REFRESH_CONCURRENCY
    }
}

fn start_ai_batch_refresh_runtime(
    state: &AppState,
    total_count: usize,
    concurrency: u8,
) -> anyhow::Result<Option<crate::ai_batch_refresh_task::AiBatchRefreshRuntimeSnapshot>> {
    let mut runtime = state
        .ai_batch_refresh
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;

    if runtime.start(total_count, concurrency) {
        Ok(None)
    } else {
        Ok(Some(runtime.snapshot()))
    }
}

fn running_ai_batch_refresh_report(
    snapshot: &crate::ai_batch_refresh_task::AiBatchRefreshRuntimeSnapshot,
) -> AiBatchRefreshReport {
    AiBatchRefreshReport {
        total_games: snapshot.total_count,
        updated_games: snapshot.updated_count,
        failed_games: snapshot.failed_count,
        message: format!(
            "AI 批量重算正在进行：已处理 {}/{}，成功 {}，失败 {}，并发 {}。",
            snapshot.processed_count,
            snapshot.total_count,
            snapshot.updated_count,
            snapshot.failed_count,
            snapshot.concurrency
        ),
    }
}

fn load_llm_runtime_config(conn: &rusqlite::Connection) -> anyhow::Result<LlmRuntimeConfig> {
    Ok(LlmRuntimeConfig {
        api_key: db::get_secret(conn, "llm_api_key")?,
        base_url: db::get_config(conn, "llm_base_url")?
            .unwrap_or_else(|| "https://api.deepseek.com".to_string()),
        model: db::get_config(conn, "llm_model")?
            .unwrap_or_else(|| "deepseek-v4-flash".to_string()),
    })
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

#[cfg(test)]
mod tests {
    use super::{
        enqueue_full_refresh_ai_jobs,
        generate_assessment_from_report_pipeline, generate_or_load_game_analysis,
        load_cached_game_analysis, refresh_all_game_analyses_pipeline,
        merge_snapshot, resolve_manual_classic_discovery_start_offset,
        start_ai_batch_refresh_runtime, visible_ai_batch_refresh_concurrency,
    };
    use crate::backfill_task::BackfillRuntimeState;
    use crate::db;
    use crate::discovery_task::DiscoveryRuntimeState;
    use crate::models::{
        AnalysisConfidence, AnalysisDimensionScore, AnalysisEvidenceItem, AnalysisEvidenceKind,
        AnalysisPoint, AnalysisReviewEvidenceItem, AnalysisReviewStance, AnalysisSource,
        GameAnalysisReport, GameCard, ReviewSnippet, StoreReleaseState, UserGameState,
    };
    use crate::recommendation::DemoStatus;
    use crate::steam::SteamGameSnapshot;
    use crate::state::AppState;
    use crate::sync_task::SyncRuntimeState;
    use reqwest::Client;
    use rusqlite::Connection;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::thread;
    use std::time::Duration;

    fn seeded_state() -> AppState {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        db::migrate(&conn).expect("migrate");
        let card = GameCard {
            appid: 7_301,
            name: "Harbor Crew".to_string(),
            short_description: Some(
                "A cooperative harbor sim with short-session runs.".to_string(),
            ),
            section: "new".to_string(),
            release_date: Some("2026-03-18".to_string()),
            release_date_text: "2026-03-18".to_string(),
            release_state: StoreReleaseState::Released,
            demo_status: DemoStatus::ReleasedWithDemo,
            supported_languages: vec!["English".to_string(), "Simplified Chinese".to_string()],
            is_adult_content: false,
            is_free: false,
            price_text: Some("$19.99".to_string()),
            discount_percent: Some(15),
            positive_review_pct: Some(91.0),
            total_reviews: Some(1248),
            current_players: Some(1860),
            recommendation_score: 86.0,
            ai_score: Some(88.0),
            ai_summary: "seeded summary".to_string(),
            capsule_url: "https://example.com/capsule.jpg".to_string(),
            store_screenshot_urls: vec!["https://example.com/shot-1.jpg".to_string()],
            tags: vec![
                "Co-op".to_string(),
                "Simulation".to_string(),
                "Casual".to_string(),
            ],
            multiplayer_modes: vec!["Online Co-op".to_string(), "LAN Co-op".to_string()],
            review_snippets: vec![
                ReviewSnippet {
                    voted_up: true,
                    review: "Great with friends and easy to teach.".to_string(),
                    playtime_hours: Some(18.4),
                },
                ReviewSnippet {
                    voted_up: false,
                    review: "Late-game variety is still a bit thin.".to_string(),
                    playtime_hours: Some(14.7),
                },
            ],
            user_state: UserGameState::default(),
        };
        db::upsert_game(&conn, &card).expect("seed game");

        AppState {
            db: Mutex::new(conn),
            http: Client::builder().build().expect("build test client"),
            discovery: Mutex::new(DiscoveryRuntimeState::default()),
            classic_discovery: Mutex::new(
                crate::classic_discovery_task::ClassicDiscoveryRuntimeState::default(),
            ),
            backfill: Mutex::new(BackfillRuntimeState::default()),
            sync: Mutex::new(SyncRuntimeState::default()),
            ai_batch_refresh: Mutex::new(
                crate::ai_batch_refresh_task::AiBatchRefreshRuntimeState::default(),
            ),
            auto_scheduler: Mutex::new(crate::state::AutoSchedulerRuntimeState::default()),
        }
    }

    fn second_seeded_card() -> GameCard {
        GameCard {
            appid: 8_402,
            name: "Quiet Orbit".to_string(),
            short_description: Some(
                "A compact co-op survival loop for weeknight squads.".to_string(),
            ),
            section: "classic".to_string(),
            release_date: Some("2025-08-10".to_string()),
            release_date_text: "2025-08-10".to_string(),
            release_state: StoreReleaseState::Released,
            demo_status: DemoStatus::Released,
            supported_languages: vec!["English".to_string(), "Simplified Chinese".to_string()],
            is_adult_content: false,
            is_free: false,
            price_text: Some("$14.99".to_string()),
            discount_percent: None,
            positive_review_pct: Some(94.0),
            total_reviews: Some(612),
            current_players: Some(402),
            recommendation_score: 74.0,
            ai_score: Some(73.0),
            ai_summary: "second seeded summary".to_string(),
            capsule_url: "https://example.com/orbit.jpg".to_string(),
            store_screenshot_urls: vec!["https://example.com/orbit-shot.jpg".to_string()],
            tags: vec!["Co-op".to_string(), "Survival".to_string()],
            multiplayer_modes: vec!["Multi-player".to_string(), "Online Co-op".to_string()],
            review_snippets: vec![
                ReviewSnippet {
                    voted_up: true,
                    review: "Great for a fixed co-op group.".to_string(),
                    playtime_hours: Some(16.0),
                },
                ReviewSnippet {
                    voted_up: false,
                    review: "Late-game content still needs more variety.".to_string(),
                    playtime_hours: Some(11.5),
                },
            ],
            user_state: UserGameState::default(),
        }
    }

    #[test]
    fn manual_classic_discovery_start_offset_defaults_to_zero_without_saved_progress() {
        let state = seeded_state();
        let conn = state.db.lock().expect("lock db");
        let start_offset =
            resolve_manual_classic_discovery_start_offset(&conn).expect("resolve start offset");

        assert_eq!(start_offset, 0);
    }

    #[test]
    fn manual_classic_discovery_start_offset_uses_saved_progress() {
        let state = seeded_state();
        let conn = state.db.lock().expect("lock db");

        db::set_config(&conn, db::CLASSIC_DISCOVERY_LAST_OFFSET_CONFIG_KEY, "7")
            .expect("seed last classic offset");

        let start_offset =
            resolve_manual_classic_discovery_start_offset(&conn).expect("resolve start offset");

        assert_eq!(start_offset, 7);
    }

    #[test]
    fn full_refresh_enqueue_preserves_real_ai_sources_per_game() {
        let state = seeded_state();
        let conn = state.db.lock().expect("lock db");

        db::upsert_game(
            &conn,
            &GameCard {
                appid: 8_402,
                name: "Quiet Orbit".to_string(),
                short_description: Some("classic".to_string()),
                section: "classic".to_string(),
                release_date: Some("2025-08-10".to_string()),
                release_date_text: "2025-08-10".to_string(),
                release_state: StoreReleaseState::Released,
                demo_status: DemoStatus::Released,
                supported_languages: vec!["English".to_string()],
                is_adult_content: false,
                is_free: false,
                price_text: None,
                discount_percent: None,
                positive_review_pct: Some(82.0),
                total_reviews: Some(1500),
                current_players: Some(320),
                recommendation_score: 70.0,
                ai_score: None,
                ai_summary: "classic".to_string(),
                capsule_url: "https://example.com/classic.jpg".to_string(),
                store_screenshot_urls: vec![],
                tags: vec!["Co-op".to_string()],
                multiplayer_modes: vec!["Online Co-op".to_string()],
                review_snippets: vec![],
                user_state: UserGameState::default(),
            },
        )
        .expect("seed classic game");
        db::upsert_game(
            &conn,
            &GameCard {
                appid: 9_503,
                name: "Hidden Orbit".to_string(),
                short_description: Some("hidden".to_string()),
                section: "classic_hidden".to_string(),
                release_date: Some("2024-08-10".to_string()),
                release_date_text: "2024-08-10".to_string(),
                release_state: StoreReleaseState::Released,
                demo_status: DemoStatus::Released,
                supported_languages: vec!["English".to_string()],
                is_adult_content: false,
                is_free: true,
                price_text: Some("Free To Play".to_string()),
                discount_percent: None,
                positive_review_pct: Some(68.0),
                total_reviews: Some(350),
                current_players: Some(21),
                recommendation_score: 40.0,
                ai_score: None,
                ai_summary: "hidden".to_string(),
                capsule_url: "https://example.com/hidden.jpg".to_string(),
                store_screenshot_urls: vec![],
                tags: vec!["Co-op".to_string()],
                multiplayer_modes: vec!["Online Co-op".to_string()],
                review_snippets: vec![],
                user_state: UserGameState::default(),
            },
        )
        .expect("seed hidden classic game");

        enqueue_full_refresh_ai_jobs(&conn).expect("enqueue full refresh jobs");

        let jobs = db::list_ai_analysis_queue_jobs(&conn).expect("list queued jobs");
        let ordered = jobs
            .into_iter()
            .map(|job| (job.appid, job.source))
            .collect::<Vec<_>>();

        assert_eq!(
            ordered,
            vec![
                (7_301, crate::models::AiAnalysisQueueSource::NewRelease),
                (8_402, crate::models::AiAnalysisQueueSource::Classic),
                (9_503, crate::models::AiAnalysisQueueSource::Classic),
            ]
        );
    }

    #[test]
    fn merge_snapshot_moves_older_mid_quality_new_game_into_classic_hidden() {
        let existing = GameCard {
            appid: 91_001,
            name: "Borderline Squad".to_string(),
            short_description: Some("before update".to_string()),
            section: "new".to_string(),
            release_date: Some("2026-03-01".to_string()),
            release_date_text: "2026-03-01".to_string(),
            release_state: StoreReleaseState::Released,
            demo_status: DemoStatus::Released,
            supported_languages: vec!["English".to_string()],
            is_adult_content: false,
            is_free: false,
            price_text: Some("$14.99".to_string()),
            discount_percent: None,
            positive_review_pct: Some(72.0),
            total_reviews: Some(280),
            current_players: Some(40),
            recommendation_score: 50.0,
            ai_score: None,
            ai_summary: "summary".to_string(),
            capsule_url: "https://example.com/borderline.jpg".to_string(),
            store_screenshot_urls: vec![],
            tags: vec!["Co-op".to_string()],
            multiplayer_modes: vec!["Online Co-op".to_string()],
            review_snippets: vec![],
            user_state: UserGameState::default(),
        };
        let snapshot = SteamGameSnapshot {
            name: None,
            short_description: None,
            release_date: Some("2026-03-01".to_string()),
            release_date_text: Some("2026-03-01".to_string()),
            release_state: Some(StoreReleaseState::Released),
            demo_status: DemoStatus::Released,
            supported_languages: None,
            is_adult_content: None,
            is_free: Some(false),
            price_text: Some("$14.99".to_string()),
            discount_percent: None,
            positive_review_pct: Some(68.0),
            total_reviews: Some(320),
            current_players: Some(55),
            capsule_url: None,
            store_screenshot_urls: vec![],
            tags: vec!["Co-op".to_string()],
            multiplayer_modes: vec!["Online Co-op".to_string()],
            review_snippets: vec![],
        };

        let merged = merge_snapshot(existing, snapshot);

        assert_eq!(merged.section, "classic_hidden");
    }

    #[test]
    fn merge_snapshot_promotes_classic_hidden_to_classic_when_quality_threshold_is_met() {
        let existing = GameCard {
            appid: 91_002,
            name: "Sleeper Hit".to_string(),
            short_description: Some("before update".to_string()),
            section: "classic_hidden".to_string(),
            release_date: Some("2025-01-01".to_string()),
            release_date_text: "2025-01-01".to_string(),
            release_state: StoreReleaseState::Released,
            demo_status: DemoStatus::Released,
            supported_languages: vec!["English".to_string()],
            is_adult_content: false,
            is_free: true,
            price_text: Some("Free To Play".to_string()),
            discount_percent: None,
            positive_review_pct: Some(69.0),
            total_reviews: Some(380),
            current_players: Some(120),
            recommendation_score: 40.0,
            ai_score: None,
            ai_summary: "summary".to_string(),
            capsule_url: "https://example.com/sleeper.jpg".to_string(),
            store_screenshot_urls: vec![],
            tags: vec!["Co-op".to_string()],
            multiplayer_modes: vec!["Online Co-op".to_string()],
            review_snippets: vec![],
            user_state: UserGameState::default(),
        };
        let snapshot = SteamGameSnapshot {
            name: None,
            short_description: None,
            release_date: Some("2025-01-01".to_string()),
            release_date_text: Some("2025-01-01".to_string()),
            release_state: Some(StoreReleaseState::Released),
            demo_status: DemoStatus::Released,
            supported_languages: None,
            is_adult_content: None,
            is_free: Some(true),
            price_text: Some("Free To Play".to_string()),
            discount_percent: None,
            positive_review_pct: Some(83.0),
            total_reviews: Some(1_240),
            current_players: Some(180),
            capsule_url: None,
            store_screenshot_urls: vec![],
            tags: vec!["Co-op".to_string()],
            multiplayer_modes: vec!["Online Co-op".to_string()],
            review_snippets: vec![],
        };

        let merged = merge_snapshot(existing, snapshot);

        assert_eq!(merged.section, "classic");
    }

    fn spawn_single_use_chat_completion_server(body: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
        let address = listener
            .local_addr()
            .expect("read local test server address");
        let body = body.to_string();

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0_u8; 16_384];
                let _ = stream.read(&mut buffer);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write test response");
                let _ = stream.flush();
            }
        });

        format!("http://{}", address)
    }

    fn cached_report(
        appid: u32,
        generated_at: &str,
        summary: &str,
        score: f64,
    ) -> GameAnalysisReport {
        GameAnalysisReport {
            appid,
            generated_at: generated_at.to_string(),
            source: AnalysisSource::Hybrid,
            confidence: AnalysisConfidence::High,
            score_version: "v2".to_string(),
            quality_score: score - 4.0,
            recommendation_score: score,
            confidence_score: 0.82,
            pool_type: crate::models::RecommendationPool::Evergreen,
            risk_flags: vec![],
            overall_score: score,
            overview: summary.to_string(),
            dimension_scores: vec![
                AnalysisDimensionScore {
                    key: "review_quality".to_string(),
                    label: "口碑质量".to_string(),
                    score,
                    reason: "cached".to_string(),
                },
                AnalysisDimensionScore {
                    key: "multiplayer_fit".to_string(),
                    label: "联机适配度".to_string(),
                    score,
                    reason: "cached".to_string(),
                },
                AnalysisDimensionScore {
                    key: "activity_health".to_string(),
                    label: "活跃健康度".to_string(),
                    score,
                    reason: "cached".to_string(),
                },
                AnalysisDimensionScore {
                    key: "content_depth".to_string(),
                    label: "内容深度".to_string(),
                    score,
                    reason: "cached".to_string(),
                },
                AnalysisDimensionScore {
                    key: "accessibility".to_string(),
                    label: "上手与本地化".to_string(),
                    score,
                    reason: "cached".to_string(),
                },
                AnalysisDimensionScore {
                    key: "discovery_value".to_string(),
                    label: "发现价值".to_string(),
                    score,
                    reason: "cached".to_string(),
                },
            ],
            strengths: vec![AnalysisPoint {
                title: "缓存优势".to_string(),
                reason: "cached".to_string(),
            }],
            risks: vec![AnalysisPoint {
                title: "缓存风险".to_string(),
                reason: "cached".to_string(),
            }],
            evidence: vec![AnalysisEvidenceItem {
                kind: AnalysisEvidenceKind::PositiveReviewPct,
                label: "好评率".to_string(),
                value: "91%".to_string(),
                interpretation: "cached".to_string(),
            }],
            review_evidence: vec![AnalysisReviewEvidenceItem {
                stance: AnalysisReviewStance::Strength,
                quote: "cached".to_string(),
                playtime_text: "10.0h".to_string(),
                interpretation: "cached".to_string(),
            }],
        }
    }

    #[test]
    fn cached_read_path_returns_none_before_generation() {
        let state = seeded_state();

        let cached = load_cached_game_analysis(&state, 7_301).expect("load cached report");

        assert!(cached.is_none());
        let conn = state.db.lock().expect("lock db");
        assert!(db::load_game_analysis(&conn, 7_301)
            .expect("reload cached report")
            .is_none());
    }

    #[tokio::test]
    async fn generate_path_without_force_refresh_returns_cached_report_when_present() {
        let state = seeded_state();
        let report = cached_report(7_301, "2026-04-30T10:00:00Z", "cached summary", 77.0);
        {
            let conn = state.db.lock().expect("lock db");
            db::save_game_analysis(&conn, 7_301, &report).expect("save cached report");
        }

        let generated = generate_or_load_game_analysis(&state, 7_301, false)
            .await
            .expect("load report");

        assert_eq!(generated.generated_at, "2026-04-30T10:00:00Z");
        assert_eq!(generated.overview, "cached summary");
        assert_eq!(generated.source, AnalysisSource::Hybrid);
    }

    #[tokio::test]
    async fn generate_path_with_force_refresh_regenerates_and_overwrites_cache() {
        let state = seeded_state();
        let stale = cached_report(7_301, "2026-04-30T10:00:00Z", "stale cached summary", 61.0);
        {
            let conn = state.db.lock().expect("lock db");
            db::save_game_analysis(&conn, 7_301, &stale).expect("save stale report");
        }

        let generated = generate_or_load_game_analysis(&state, 7_301, true)
            .await
            .expect("regenerate report");

        assert_ne!(generated.generated_at, stale.generated_at);
        assert_ne!(generated.overview, stale.overview);
        let conn = state.db.lock().expect("lock db");
        let saved = db::load_game_analysis(&conn, 7_301)
            .expect("load saved report")
            .expect("saved report exists");
        assert_eq!(saved.generated_at, generated.generated_at);
        assert_eq!(saved.overview, generated.overview);
        assert_eq!(saved.overall_score, generated.overall_score);
    }

    #[tokio::test]
    async fn force_refresh_reuses_cached_narrative_when_only_generated_at_changes() {
        let state = seeded_state();
        let base_url = spawn_single_use_chat_completion_server(
            r#"{"choices":[{"message":{"content":"{\"overview\":\"联机亮点明确，适合固定好友队反复开黑。\",\"strengths\":[{\"title\":\"朋友局体验稳\",\"reason\":\"在线协作信号和近期口碑都够强。\"}],\"risks\":[{\"title\":\"后期深度一般\",\"reason\":\"差评主要集中在内容消耗后的重复感。\"}],\"dimensionReasons\":[[\"content_depth\",\"后期内容延展性一般，但不影响短中期组局体验。\"]]}"}}]}"#,
        );
        {
            let conn = state.db.lock().expect("lock db");
            db::set_config(&conn, "llm_api_key", "test-key").expect("set llm api key");
            db::set_config(&conn, "llm_base_url", &base_url).expect("set llm base url");
            db::set_config(&conn, "llm_model", "deepseek-v4-flash").expect("set llm model");
        }

        let first = generate_or_load_game_analysis(&state, 7_301, true)
            .await
            .expect("generate first hybrid report");
        thread::sleep(Duration::from_millis(1_100));
        let second = generate_or_load_game_analysis(&state, 7_301, true)
            .await
            .expect("reuse cached narrative on force refresh");

        assert_eq!(first.source, AnalysisSource::Hybrid);
        assert_eq!(second.source, AnalysisSource::Hybrid);
        assert_ne!(first.generated_at, second.generated_at);
        assert_eq!(first.overview, second.overview);

        let conn = state.db.lock().expect("lock db");
        let cache_entries: i64 = conn
            .query_row("SELECT COUNT(*) FROM analysis_narrative_cache", [], |row| {
                row.get(0)
            })
            .expect("count narrative cache entries");
        assert_eq!(cache_entries, 1);
    }

    #[tokio::test]
    async fn batch_refresh_pipeline_regenerates_all_cached_reports_and_updates_scores() {
        let state = seeded_state();
        {
            let conn = state.db.lock().expect("lock db");
            db::upsert_game(&conn, &second_seeded_card()).expect("seed second game");
            db::save_game_analysis(
                &conn,
                7_301,
                &cached_report(7_301, "2026-04-30T10:00:00Z", "stale first summary", 61.0),
            )
            .expect("save first stale report");
            db::save_game_analysis(
                &conn,
                8_402,
                &cached_report(8_402, "2026-04-30T10:00:00Z", "stale second summary", 58.0),
            )
            .expect("save second stale report");
        }

        let result = refresh_all_game_analyses_pipeline(&state, 2)
            .await
            .expect("refresh all analyses");

        assert_eq!(result.total_games, 2);
        assert_eq!(result.updated_games, 2);
        assert_eq!(result.failed_games, 0);

        let conn = state.db.lock().expect("lock db");
        let first = db::load_game_analysis(&conn, 7_301)
            .expect("load first refreshed report")
            .expect("first refreshed report exists");
        let second = db::load_game_analysis(&conn, 8_402)
            .expect("load second refreshed report")
            .expect("second refreshed report exists");
        assert_ne!(first.overview, "stale first summary");
        assert_ne!(second.overview, "stale second summary");
        assert_ne!(first.generated_at, "2026-04-30T10:00:00Z");
        assert_ne!(second.generated_at, "2026-04-30T10:00:00Z");

        let first_game = db::load_game(&conn, 7_301)
            .expect("load first game")
            .expect("first game exists");
        let second_game = db::load_game(&conn, 8_402)
            .expect("load second game")
            .expect("second game exists");
        assert_eq!(first_game.ai_score, Some(first.recommendation_score));
        assert_eq!(second_game.ai_score, Some(second.recommendation_score));
    }

    #[tokio::test]
    async fn batch_refresh_pipeline_clamps_requested_concurrency_and_reports_it() {
        let state = seeded_state();
        {
            let conn = state.db.lock().expect("lock db");
            db::upsert_game(&conn, &second_seeded_card()).expect("seed second game");
        }

        let lowered = refresh_all_game_analyses_pipeline(&state, 0)
            .await
            .expect("refresh analyses with lowered concurrency");
        let raised = refresh_all_game_analyses_pipeline(&state, 99)
            .await
            .expect("refresh analyses with raised concurrency");

        assert!(lowered.message.contains("1 路并发"));
        assert!(raised.message.contains("10 路并发"));
    }

    #[test]
    fn visible_batch_refresh_concurrency_prefers_runtime_value_while_running() {
        let running = crate::ai_batch_refresh_task::AiBatchRefreshRuntimeSnapshot {
            running: true,
            concurrency: 5,
            pending_count: 4,
            active_count: 5,
            total_count: 9,
            processed_count: 0,
            updated_count: 0,
            failed_count: 0,
            last_error: None,
            last_error_appid: None,
        };
        let idle = crate::ai_batch_refresh_task::AiBatchRefreshRuntimeSnapshot::default();

        assert_eq!(visible_ai_batch_refresh_concurrency(&running, 10), 5);
        assert_eq!(visible_ai_batch_refresh_concurrency(&idle, 10), 10);
        assert_eq!(
            visible_ai_batch_refresh_concurrency(&idle, 0),
            db::DEFAULT_AI_BATCH_REFRESH_CONCURRENCY
        );
    }

    #[test]
    fn start_batch_refresh_runtime_returns_existing_snapshot_when_already_running() {
        let state = seeded_state();
        {
            let mut runtime = state.ai_batch_refresh.lock().expect("lock runtime");
            assert!(runtime.start(12, 4));
            runtime.mark_job_started();
        }

        let existing = start_ai_batch_refresh_runtime(&state, 30, 9)
            .expect("start runtime")
            .expect("existing snapshot");

        assert!(existing.running);
        assert_eq!(existing.total_count, 12);
        assert_eq!(existing.concurrency, 4);
        assert_eq!(existing.pending_count, 11);
    }

    #[tokio::test]
    async fn legacy_assess_path_adapts_from_report_pipeline() {
        let state = seeded_state();
        let stale = cached_report(7_301, "2026-04-30T10:00:00Z", "stale cached summary", 61.0);
        {
            let conn = state.db.lock().expect("lock db");
            db::save_game_analysis(&conn, 7_301, &stale).expect("save stale report");
        }

        let assessment = generate_assessment_from_report_pipeline(&state, 7_301)
            .await
            .expect("generate assessment");
        let conn = state.db.lock().expect("lock db");
        let saved = db::load_game_analysis(&conn, 7_301)
            .expect("load saved report")
            .expect("saved report exists");

        assert_eq!(assessment.appid, saved.appid);
        assert_eq!(assessment.summary, saved.overview);
        assert_eq!(assessment.score, saved.overall_score);
        assert_eq!(
            assessment.best_for,
            saved
                .strengths
                .iter()
                .map(|item| item.title.clone())
                .take(3)
                .collect::<Vec<_>>()
        );
        assert_ne!(assessment.summary, stale.overview);
    }
}
