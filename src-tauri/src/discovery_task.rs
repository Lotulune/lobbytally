use crate::backfill_task;
use crate::db::{self, DiscoveryProgressPatch};
use crate::discovery::{
    build_discovered_game_card, store_search_reached_page_budget, store_search_start_for_page,
    DISCOVERY_CURSOR_CONFIG_KEY,
};
use crate::models::{DiscoveryRunSnapshot, DiscoveryRunStatus, SyncMode};
use crate::state::AppState;
use crate::steam;
use anyhow::{Context, Result};
use reqwest::Client;
use std::collections::HashSet;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub const DISCOVERY_TASK_EVENT: &str = "discovery-task-updated";
const DISCOVERY_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const DISCOVERY_HTTP_USER_AGENT: &str = "MPGS/0.1 (+https://local.app)";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiscoveryControl {
    #[default]
    None,
    PauseRequested,
    CancelRequested,
}

#[derive(Debug, Default)]
pub struct DiscoveryRuntimeState {
    pub active_run_id: Option<i64>,
    pub control: DiscoveryControl,
}

pub fn emit_snapshot(app: &AppHandle, snapshot: &DiscoveryRunSnapshot) {
    let _ = app.emit(DISCOVERY_TASK_EVENT, snapshot.clone());
}

pub fn spawn_discovery_worker(app: AppHandle, run_id: i64) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = run_discovery_worker(app.clone(), run_id).await {
            eprintln!("discovery worker {run_id} failed: {error:#}");
            let _ = fail_run_if_possible(&app, run_id, error.to_string());
        }
    });
}

async fn run_discovery_worker(app: AppHandle, run_id: i64) -> Result<()> {
    let http = build_discovery_http_client()?;
    let (mut snapshot, country, language, mut known_appids) = {
        let state = app.state::<AppState>();
        let conn = state
            .db
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        let snapshot = db::load_discovery_run(&conn, run_id)?
            .with_context(|| format!("discovery run {run_id} was not found"))?;
        let config = db::public_config(&conn)?;
        let known_appids = db::list_game_appids(&conn)?
            .into_iter()
            .collect::<HashSet<_>>();
        (snapshot, config.country, config.language, known_appids)
    };
    let today = crate::recommendation::today_iso_utc();
    emit_snapshot(&app, &snapshot);

    loop {
        match current_control(&app)? {
            DiscoveryControl::CancelRequested => {
                snapshot.status = DiscoveryRunStatus::Cancelled;
                snapshot.current_appid = None;
                snapshot.last_error = None;
                snapshot.finished_at = Some(now_rfc3339()?);
                persist_snapshot(&app, run_id, &snapshot, snapshot.last_appid.is_some(), true)?;
                clear_runtime_if_active(&app, run_id)?;
                return Ok(());
            }
            DiscoveryControl::PauseRequested => {
                snapshot.status = DiscoveryRunStatus::Paused;
                snapshot.current_appid = None;
                snapshot.last_error = None;
                snapshot.finished_at = None;
                persist_snapshot(
                    &app,
                    run_id,
                    &snapshot,
                    snapshot.last_appid.is_some(),
                    false,
                )?;
                clear_runtime_if_active(&app, run_id)?;
                return Ok(());
            }
            DiscoveryControl::None => {}
        }

        if snapshot.added_games >= snapshot.target_added_games as usize {
            snapshot.status = DiscoveryRunStatus::Completed;
            snapshot.current_appid = None;
            snapshot.last_error = None;
            snapshot.finished_at = Some(now_rfc3339()?);
            persist_snapshot(&app, run_id, &snapshot, snapshot.last_appid.is_some(), true)?;
            clear_runtime_if_active(&app, run_id)?;
            return Ok(());
        }

        if store_search_reached_page_budget(snapshot.pages_processed) {
            snapshot.status = DiscoveryRunStatus::Completed;
            snapshot.current_appid = None;
            snapshot.last_error = None;
            snapshot.have_more_results = true;
            snapshot.finished_at = Some(now_rfc3339()?);
            persist_snapshot(&app, run_id, &snapshot, snapshot.last_appid.is_some(), true)?;
            clear_runtime_if_active(&app, run_id)?;
            return Ok(());
        }

        let page_index = snapshot.pages_processed + 1;
        let page_start = store_search_start_for_page(snapshot.pages_processed, snapshot.page_size);
        let preview = match steam::fetch_store_search_candidates(
            &http,
            page_start,
            snapshot.page_size,
            &language,
        )
        .await
        {
            Ok(preview) => preview,
            Err(error) => {
                append_discovery_failure_in_place(
                    &app,
                    run_id,
                    page_index,
                    None,
                    "fetch_preview",
                    &error.to_string(),
                )?;
                snapshot.status = DiscoveryRunStatus::Failed;
                snapshot.current_appid = None;
                snapshot.last_error = Some(error.to_string());
                snapshot.finished_at = Some(now_rfc3339()?);
                persist_snapshot(&app, run_id, &snapshot, snapshot.last_appid.is_some(), true)?;
                clear_runtime_if_active(&app, run_id)?;
                return Ok(());
            }
        };
        let page_end_appid = preview.apps.last().map(|app| app.appid);
        let page_have_more_results = preview.have_more_results;

        snapshot.have_more_results = page_have_more_results;
        snapshot.last_error = None;
        snapshot.finished_at = None;

        for app_item in &preview.apps {
            match current_control(&app)? {
                DiscoveryControl::CancelRequested => {
                    snapshot.status = DiscoveryRunStatus::Cancelled;
                    snapshot.current_appid = None;
                    snapshot.last_error = None;
                    snapshot.finished_at = Some(now_rfc3339()?);
                    persist_snapshot(&app, run_id, &snapshot, snapshot.last_appid.is_some(), true)?;
                    clear_runtime_if_active(&app, run_id)?;
                    return Ok(());
                }
                DiscoveryControl::PauseRequested => {
                    snapshot.status = DiscoveryRunStatus::Paused;
                    snapshot.current_appid = None;
                    snapshot.last_error = None;
                    snapshot.finished_at = None;
                    persist_snapshot(
                        &app,
                        run_id,
                        &snapshot,
                        snapshot.last_appid.is_some(),
                        false,
                    )?;
                    clear_runtime_if_active(&app, run_id)?;
                    return Ok(());
                }
                DiscoveryControl::None => {}
            }

            snapshot.current_appid = Some(app_item.appid);
            snapshot.last_error = None;
            persist_snapshot(&app, run_id, &snapshot, false, false)?;

            if known_appids.contains(&app_item.appid) {
                snapshot.skipped_existing += 1;
                mark_processed_app(&mut snapshot, app_item.appid);
                persist_snapshot(&app, run_id, &snapshot, true, false)?;
                continue;
            }

            match steam::fetch_game_snapshot(
                &http,
                app_item.appid,
                &country,
                &language,
                steam::SteamGameSnapshotEnrichment::Discovery,
            )
            .await
            {
                Ok(game_snapshot) => {
                    if let Some(card) = build_discovered_game_card(app_item, game_snapshot, &today)
                    {
                        match card.section.as_str() {
                            "new" => snapshot.added_new_games += 1,
                            _ => snapshot.added_classic_games += 1,
                        }
                        {
                            let state = app.state::<AppState>();
                            let conn = state
                                .db
                                .lock()
                                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
                            db::upsert_game(&conn, &card)?;
                        }
                        if snapshot.sync_mode == SyncMode::Full {
                            backfill_task::enqueue_backfill(&app, [app_item.appid])?;
                        }
                        known_appids.insert(app_item.appid);
                        snapshot.added_games += 1;
                    } else {
                        snapshot.skipped_non_multiplayer += 1;
                    }
                }
                Err(error) => {
                    append_discovery_failure_in_place(
                        &app,
                        run_id,
                        page_index,
                        Some(app_item.appid),
                        "fetch_snapshot",
                        &error.to_string(),
                    )?;
                    snapshot.failed_games += 1;
                    snapshot.last_error = Some(error.to_string());
                }
            }

            mark_processed_app(&mut snapshot, app_item.appid);
            persist_snapshot(&app, run_id, &snapshot, true, false)?;

            if snapshot.added_games >= snapshot.target_added_games as usize {
                snapshot.status = DiscoveryRunStatus::Completed;
                snapshot.current_appid = None;
                snapshot.last_error = None;
                snapshot.finished_at = Some(now_rfc3339()?);
                persist_snapshot(&app, run_id, &snapshot, snapshot.last_appid.is_some(), true)?;
                clear_runtime_if_active(&app, run_id)?;
                return Ok(());
            }
        }

        snapshot.pages_processed = page_index;
        snapshot.current_appid = None;
        if let Some(page_end_appid) = page_end_appid {
            snapshot.last_appid = Some(page_end_appid);
        }
        persist_snapshot(
            &app,
            run_id,
            &snapshot,
            snapshot.last_appid.is_some(),
            false,
        )?;

        if !page_have_more_results || page_end_appid.is_none() {
            snapshot.status = DiscoveryRunStatus::Completed;
            snapshot.current_appid = None;
            snapshot.last_error = None;
            snapshot.have_more_results = false;
            snapshot.finished_at = Some(now_rfc3339()?);
            persist_snapshot(&app, run_id, &snapshot, snapshot.last_appid.is_some(), true)?;
            clear_runtime_if_active(&app, run_id)?;
            return Ok(());
        }
    }
}

fn current_control(app: &AppHandle) -> Result<DiscoveryControl> {
    let state = app.state::<AppState>();
    let runtime = state
        .discovery
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok(runtime.control)
}

fn clear_runtime_if_active(app: &AppHandle, run_id: i64) -> Result<()> {
    let state = app.state::<AppState>();
    let mut runtime = state
        .discovery
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    if runtime.active_run_id == Some(run_id) {
        runtime.active_run_id = None;
        runtime.control = DiscoveryControl::None;
    }
    Ok(())
}

fn persist_snapshot(
    app: &AppHandle,
    run_id: i64,
    snapshot: &DiscoveryRunSnapshot,
    persist_cursor: bool,
    mark_sync_complete: bool,
) -> Result<DiscoveryRunSnapshot> {
    let state = app.state::<AppState>();
    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    db::update_discovery_run_progress(&conn, run_id, snapshot_to_patch(snapshot))?;
    if persist_cursor {
        if let Some(last_appid) = snapshot.last_appid {
            db::set_config(&conn, DISCOVERY_CURSOR_CONFIG_KEY, &last_appid.to_string())?;
        }
    }
    if mark_sync_complete {
        db::mark_sync_complete(&conn)?;
    }
    let stored = db::load_discovery_run(&conn, run_id)?
        .with_context(|| format!("discovery run {run_id} disappeared after persistence"))?;
    drop(conn);
    emit_snapshot(app, &stored);
    Ok(stored)
}

fn fail_run_if_possible(app: &AppHandle, run_id: i64, error: String) -> Result<()> {
    let mut snapshot = {
        let state = app.state::<AppState>();
        let conn = state
            .db
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        db::load_discovery_run(&conn, run_id)?
            .with_context(|| format!("discovery run {run_id} was not found"))?
    };

    if matches!(
        snapshot.status,
        DiscoveryRunStatus::Completed | DiscoveryRunStatus::Cancelled
    ) {
        clear_runtime_if_active(app, run_id)?;
        return Ok(());
    }

    append_discovery_failure_in_place(
        app,
        run_id,
        snapshot.pages_processed + 1,
        snapshot.current_appid,
        "worker",
        &error,
    )?;
    snapshot.status = DiscoveryRunStatus::Failed;
    snapshot.current_appid = None;
    snapshot.last_error = Some(error);
    snapshot.finished_at = Some(now_rfc3339()?);
    persist_snapshot(app, run_id, &snapshot, snapshot.last_appid.is_some(), true)?;
    clear_runtime_if_active(app, run_id)?;
    Ok(())
}

fn snapshot_to_patch(snapshot: &DiscoveryRunSnapshot) -> DiscoveryProgressPatch {
    DiscoveryProgressPatch {
        status: Some(snapshot.status.clone()),
        current_appid: Some(snapshot.current_appid),
        last_appid: Some(snapshot.last_appid),
        pages_processed: Some(snapshot.pages_processed),
        scanned_apps: Some(snapshot.scanned_apps),
        added_games: Some(snapshot.added_games),
        added_new_games: Some(snapshot.added_new_games),
        added_classic_games: Some(snapshot.added_classic_games),
        skipped_existing: Some(snapshot.skipped_existing),
        skipped_non_multiplayer: Some(snapshot.skipped_non_multiplayer),
        failed_games: Some(snapshot.failed_games),
        have_more_results: Some(snapshot.have_more_results),
        last_error: Some(snapshot.last_error.clone()),
        finished_at: Some(snapshot.finished_at.clone()),
    }
}

fn mark_processed_app(snapshot: &mut DiscoveryRunSnapshot, appid: u32) {
    snapshot.scanned_apps += 1;
    snapshot.current_appid = None;
    snapshot.last_appid = Some(appid);
}

fn now_rfc3339() -> Result<String> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

fn build_discovery_http_client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(DISCOVERY_HTTP_USER_AGENT)
        .timeout(DISCOVERY_HTTP_TIMEOUT)
        .build()?)
}

fn append_discovery_failure_in_place(
    app: &AppHandle,
    run_id: i64,
    page_index: u32,
    appid: Option<u32>,
    stage: &str,
    reason: &str,
) -> Result<()> {
    let state = app.state::<AppState>();
    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    db::append_discovery_failure(&conn, run_id, page_index, appid, stage, reason)?;
    Ok(())
}
