use crate::ai_batch_refresh_task;
use crate::backfill_task;
use crate::classic_discovery_task;
use crate::commands;
use crate::db;
use crate::models::{
    ClassicDiscoveryTaskRequest, DiscoveryRunStatus, DiscoveryTaskRequest, SyncMode,
};
use crate::state::AppState;
use anyhow::Result;
use tauri::{AppHandle, Manager};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub const STARTUP_NEW_DISCOVERY_COOLDOWN_HOURS: i64 = 3;

pub fn kick(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = evaluate(app).await {
            eprintln!("auto scheduler failed: {error:#}");
        }
    });
}

async fn evaluate(app: AppHandle) -> Result<()> {
    {
        let state = app.state::<AppState>();
        let mut scheduler = state
            .auto_scheduler
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        if scheduler.evaluating {
            return Ok(());
        }
        scheduler.evaluating = true;
    }

    let result = evaluate_inner(&app).await;

    let state = app.state::<AppState>();
    let mut scheduler = state
        .auto_scheduler
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    scheduler.evaluating = false;

    result
}

async fn evaluate_inner(app: &AppHandle) -> Result<()> {
    let new_discovery_running = discovery_running(app)?;

    if should_resume_new_discovery(app)? {
        crate::discovery_task::restore_discovery_runtime(app.clone())?;
        let state = app.state::<AppState>();
        let mut scheduler = state
            .auto_scheduler
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        scheduler.startup_new_discovery_bootstrap_completed = true;
        return Ok(());
    }

    if should_start_startup_new_discovery(app)? {
        let _ = commands::start_discovery_task(
            app.clone(),
            DiscoveryTaskRequest {
                sync_mode: SyncMode::Full,
                target_added_games: crate::discovery::DISCOVERY_TASK_TARGET_ADDED_GAMES_DEFAULT,
                page_size: 100,
            },
        );
        let state = app.state::<AppState>();
        let mut scheduler = state
            .auto_scheduler
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        scheduler.startup_new_discovery_bootstrap_completed = true;
        return Ok(());
    }

    let backfill_active = backfill_pending_or_running(app)?;
    if backfill_active {
        backfill_task::restore_backfill_runtime(app.clone())?;
    }

    if ai_queue_ready(app)? {
        let concurrency = {
            let state = app.state::<AppState>();
            let conn = state
                .db
                .lock()
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            db::load_ai_batch_refresh_concurrency(&conn)?
        };
        ai_batch_refresh_task::start_ai_batch_refresh_worker_if_idle(app, concurrency)?;
    }

    if new_discovery_running {
        return Ok(());
    }

    if should_resume_classic_discovery(app)? {
        classic_discovery_task::restore_classic_discovery_runtime(app.clone())?;
        return Ok(());
    }

    if should_start_classic_discovery(app)? {
        let _ = commands::start_classic_discovery_task(
            app.clone(),
            ClassicDiscoveryTaskRequest {
                max_pages: Some(db::CLASSIC_DISCOVERY_MAX_PAGES_DEFAULT),
            },
        );
        return Ok(());
    }

    Ok(())
}

fn should_resume_new_discovery(app: &AppHandle) -> Result<bool> {
    if discovery_running(app)? || classic_running(app)? {
        return Ok(false);
    }
    let state = app.state::<AppState>();
    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let latest = db::load_latest_discovery_run(&conn)?;
    Ok(latest.as_ref().is_some_and(|run| run.can_resume()))
}

fn should_start_startup_new_discovery(app: &AppHandle) -> Result<bool> {
    let state = app.state::<AppState>();
    let scheduler = state
        .auto_scheduler
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    if scheduler.startup_new_discovery_bootstrap_completed {
        return Ok(false);
    }
    drop(scheduler);
    if discovery_running(app)? || classic_running(app)? {
        return Ok(false);
    }
    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let latest = db::load_latest_discovery_run(&conn)?;
    if matches!(
        latest.as_ref().map(|run| &run.status),
        Some(DiscoveryRunStatus::Running)
    ) {
        return Ok(false);
    }

    let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
    startup_new_discovery_is_due(latest.as_ref(), &now)
}

fn should_resume_classic_discovery(app: &AppHandle) -> Result<bool> {
    if !classic_discovery_prerequisites_met(
        classic_running(app)?,
        discovery_running(app)?,
        new_backfill_pending_or_running(app)?,
    ) {
        return Ok(false);
    }
    let state = app.state::<AppState>();
    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let latest = db::load_latest_classic_discovery_run(&conn)?;
    Ok(latest.as_ref().is_some_and(|run| run.can_resume()))
}

fn should_start_classic_discovery(app: &AppHandle) -> Result<bool> {
    if !classic_discovery_prerequisites_met(
        classic_running(app)?,
        discovery_running(app)?,
        new_backfill_pending_or_running(app)?,
    ) {
        return Ok(false);
    }
    let state = app.state::<AppState>();
    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
    db::classic_discovery_is_due(&conn, &now)
}

fn discovery_running(app: &AppHandle) -> Result<bool> {
    let state = app.state::<AppState>();
    let runtime = state
        .discovery
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok(runtime.active_run_id.is_some())
}

fn classic_running(app: &AppHandle) -> Result<bool> {
    let state = app.state::<AppState>();
    let runtime = state
        .classic_discovery
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok(runtime.active_run_id.is_some())
}

fn backfill_pending_or_running(app: &AppHandle) -> Result<bool> {
    let state = app.state::<AppState>();
    let runtime = state
        .backfill
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let snapshot = runtime.snapshot();
    if snapshot.running || snapshot.pending_count > 0 {
        return Ok(true);
    }
    drop(runtime);
    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok(db::count_metadata_backfill_jobs(&conn)? > 0)
}

fn new_backfill_pending_or_running(app: &AppHandle) -> Result<bool> {
    let state = app.state::<AppState>();
    let runtime = state
        .backfill
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let snapshot = runtime.snapshot();
    if snapshot.running || snapshot.pending_count > 0 {
        return Ok(true);
    }
    drop(runtime);
    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok(db::count_metadata_backfill_jobs(&conn)? > 0)
}

fn ai_queue_ready(app: &AppHandle) -> Result<bool> {
    let state = app.state::<AppState>();
    let runtime = state
        .ai_batch_refresh
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    if runtime.active {
        return Ok(false);
    }
    drop(runtime);
    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok(db::count_ai_analysis_queue_ready_jobs(&conn)? > 0)
}

fn classic_discovery_prerequisites_met(
    classic_running: bool,
    discovery_running: bool,
    backfill_pending_or_running: bool,
) -> bool {
    !(classic_running || discovery_running || backfill_pending_or_running)
}

fn startup_new_discovery_is_due(
    latest: Option<&crate::models::DiscoveryRunSnapshot>,
    now: &str,
) -> Result<bool> {
    let Some(latest) = latest else {
        return Ok(true);
    };
    let reference_time = latest
        .finished_at
        .as_deref()
        .unwrap_or(latest.started_at.as_str());
    let last_run_at = parse_rfc3339_utc(reference_time).ok();
    let now = parse_rfc3339_utc(now).ok();
    let Some((last_run_at, now)) = last_run_at.zip(now) else {
        return Ok(true);
    };
    Ok((now - last_run_at).whole_hours() >= STARTUP_NEW_DISCOVERY_COOLDOWN_HOURS)
}

fn parse_rfc3339_utc(value: &str) -> Result<OffsetDateTime> {
    Ok(OffsetDateTime::parse(value, &Rfc3339)?)
}

#[cfg(test)]
mod tests {
    use super::{
        classic_discovery_prerequisites_met, startup_new_discovery_is_due,
        STARTUP_NEW_DISCOVERY_COOLDOWN_HOURS,
    };
    use crate::models::{
        DiscoveryCompletionReason, DiscoveryFailureItem, DiscoveryRunSnapshot, DiscoveryRunStatus,
        SyncMode,
    };

    fn discovery_run(
        status: DiscoveryRunStatus,
        started_at: &str,
        finished_at: Option<&str>,
    ) -> DiscoveryRunSnapshot {
        DiscoveryRunSnapshot {
            id: 1,
            status,
            completion_reason: Some(DiscoveryCompletionReason::PageBudgetReached),
            sync_mode: SyncMode::Full,
            target_added_games: 200,
            page_size: 100,
            pages_processed: 2,
            scanned_apps: 200,
            added_games: 0,
            added_new_games: 0,
            added_classic_games: 0,
            skipped_existing: 0,
            skipped_non_multiplayer: 0,
            failed_games: 0,
            current_appid: None,
            last_appid: None,
            have_more_results: false,
            started_at: started_at.to_string(),
            updated_at: started_at.to_string(),
            finished_at: finished_at.map(str::to_string),
            last_error: None,
            failures: Vec::<DiscoveryFailureItem>::new(),
        }
    }

    #[test]
    fn startup_new_discovery_is_due_without_previous_run() {
        let due = startup_new_discovery_is_due(None, "2026-05-05T12:00:00Z")
            .expect("evaluate due state");
        assert!(due);
    }

    #[test]
    fn startup_new_discovery_respects_three_hour_cooldown_from_finished_run() {
        let latest = discovery_run(
            DiscoveryRunStatus::Completed,
            "2026-05-05T08:00:00Z",
            Some("2026-05-05T10:30:00Z"),
        );

        let blocked = startup_new_discovery_is_due(
            Some(&latest),
            "2026-05-05T12:59:59Z",
        )
        .expect("evaluate blocked state");
        let allowed = startup_new_discovery_is_due(
            Some(&latest),
            "2026-05-05T13:30:00Z",
        )
        .expect("evaluate allowed state");

        assert!(!blocked);
        assert!(allowed);
    }

    #[test]
    fn startup_new_discovery_uses_started_at_when_run_is_unfinished() {
        let latest = discovery_run(
            DiscoveryRunStatus::Interrupted,
            "2026-05-05T09:00:00Z",
            None,
        );

        let blocked = startup_new_discovery_is_due(
            Some(&latest),
            "2026-05-05T11:59:59Z",
        )
        .expect("evaluate blocked state");
        let allowed = startup_new_discovery_is_due(
            Some(&latest),
            &format!(
                "2026-05-05T{:02}:00:00Z",
                9 + STARTUP_NEW_DISCOVERY_COOLDOWN_HOURS
            ),
        )
        .expect("evaluate allowed state");

        assert!(!blocked);
        assert!(allowed);
    }

    #[test]
    fn classic_discovery_waits_for_new_discovery_and_backfill_only() {
        assert!(!classic_discovery_prerequisites_met(true, false, false));
        assert!(!classic_discovery_prerequisites_met(false, true, false));
        assert!(!classic_discovery_prerequisites_met(false, false, true));
    }

    #[test]
    fn classic_discovery_can_start_only_when_new_pipeline_is_idle() {
        assert!(classic_discovery_prerequisites_met(false, false, false));
    }

    #[test]
    fn classic_discovery_is_not_blocked_by_new_ai_backlog_anymore() {
        assert!(classic_discovery_prerequisites_met(false, false, false));
    }
}
