use crate::auto_scheduler;
use crate::commands;
use crate::db;
use crate::models::AiAnalysisQueueSource;
use crate::state::AppState;
use anyhow::Result;
use futures::stream::{FuturesUnordered, StreamExt};
use std::collections::{HashSet, VecDeque};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AiBatchRefreshRuntimeSnapshot {
    pub running: bool,
    pub concurrency: u8,
    pub pending_count: usize,
    pub active_count: usize,
    pub total_count: usize,
    pub processed_count: usize,
    pub updated_count: usize,
    pub failed_count: usize,
    pub last_error: Option<String>,
    pub last_error_appid: Option<u32>,
}

#[derive(Debug, Default)]
pub struct AiBatchRefreshRuntimeState {
    pub active: bool,
    concurrency: u8,
    pending: VecDeque<u32>,
    tracked_appids: HashSet<u32>,
    in_progress: HashSet<u32>,
    pending_count: usize,
    active_count: usize,
    total_count: usize,
    processed_count: usize,
    updated_count: usize,
    failed_count: usize,
    last_error: Option<String>,
    last_error_appid: Option<u32>,
}

impl AiBatchRefreshRuntimeState {
    fn reset_state_for_new_batch(&mut self, concurrency: u8) {
        self.active = true;
        self.concurrency = concurrency;
        self.pending.clear();
        self.tracked_appids.clear();
        self.in_progress.clear();
        self.pending_count = 0;
        self.active_count = 0;
        self.total_count = 0;
        self.processed_count = 0;
        self.updated_count = 0;
        self.failed_count = 0;
        self.last_error = None;
        self.last_error_appid = None;
    }

    pub fn start(&mut self, total_count: usize, concurrency: u8) -> bool {
        if self.active {
            return false;
        }

        self.reset_state_for_new_batch(concurrency);
        self.total_count = total_count;
        self.pending_count = total_count;
        true
    }

    pub fn activate_placeholder(&mut self, concurrency: u8) -> bool {
        if self.active {
            return false;
        }

        self.reset_state_for_new_batch(concurrency);
        true
    }

    pub fn load_pending_jobs(&mut self, appids: impl IntoIterator<Item = u32>, concurrency: u8) {
        self.reset_state_for_new_batch(concurrency);
        self.replace_pending(appids);
    }

    pub fn replace_pending(&mut self, appids: impl IntoIterator<Item = u32>) {
        self.pending.clear();
        self.tracked_appids.clear();
        self.in_progress.clear();
        for appid in appids {
            if self.tracked_appids.insert(appid) {
                self.pending.push_back(appid);
            }
        }
        self.pending_count = self.pending.len();
        self.active_count = 0;
        self.total_count = self.pending.len();
    }

    pub fn take_next_job(&mut self) -> Option<u32> {
        let appid = self.pending.pop_front()?;
        self.in_progress.insert(appid);
        self.pending_count = self.pending.len();
        self.active_count += 1;
        Some(appid)
    }

    pub fn mark_job_started(&mut self) {
        if self.pending_count > 0 {
            self.pending_count -= 1;
        }
        self.active_count += 1;
    }

    pub fn finish_job(&mut self, appid: u32, updated: bool, error: Option<String>) {
        self.in_progress.remove(&appid);
        self.tracked_appids.remove(&appid);
        if self.active_count > 0 {
            self.active_count -= 1;
        }
        self.processed_count += 1;
        if updated {
            self.updated_count += 1;
        }
        if let Some(error) = error {
            self.failed_count += 1;
            self.last_error = Some(error);
            self.last_error_appid = Some(appid);
        }
    }

    pub fn finish_batch(&mut self) {
        self.active = false;
        self.pending.clear();
        self.tracked_appids.clear();
        self.in_progress.clear();
        self.pending_count = 0;
        self.active_count = 0;
    }

    pub fn fail_batch(&mut self, error: String) {
        self.active = false;
        self.pending.clear();
        self.tracked_appids.clear();
        self.in_progress.clear();
        self.pending_count = 0;
        self.active_count = 0;
        self.last_error = Some(error);
    }

    pub fn snapshot(&self) -> AiBatchRefreshRuntimeSnapshot {
        AiBatchRefreshRuntimeSnapshot {
            running: self.active,
            concurrency: self.concurrency,
            pending_count: self.pending_count,
            active_count: self.active_count,
            total_count: self.total_count,
            processed_count: self.processed_count,
            updated_count: self.updated_count,
            failed_count: self.failed_count,
            last_error: self.last_error.clone(),
            last_error_appid: self.last_error_appid,
        }
    }
}

pub fn restore_ai_batch_refresh_runtime(app: AppHandle) -> Result<()> {
    let concurrency = {
        let state = app.state::<AppState>();
        let conn = state
            .db
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        db::load_ai_batch_refresh_concurrency(&conn)?
    };
    start_ai_batch_refresh_worker_if_idle(&app, concurrency)
}

pub fn start_ai_batch_refresh_worker_if_idle(app: &AppHandle, concurrency: u8) -> Result<()> {
    let should_spawn = {
        let state = app.state::<AppState>();
        let mut runtime = state
            .ai_batch_refresh
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        if runtime.active {
            false
        } else {
            runtime.activate_placeholder(db::clamp_ai_batch_refresh_concurrency(concurrency));
            true
        }
    };

    if should_spawn {
        spawn_ai_batch_refresh_worker(app.clone(), concurrency);
    }
    Ok(())
}

pub fn spawn_ai_batch_refresh_worker(app: AppHandle, concurrency: u8) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = run_ai_batch_refresh_worker(app.clone(), concurrency).await {
            eprintln!("ai batch refresh worker failed: {error:#}");
            let _ = fail_worker(&app, error.to_string());
        }
        auto_scheduler::kick(app);
    });
}

async fn run_ai_batch_refresh_worker(app: AppHandle, concurrency: u8) -> Result<()> {
    let concurrency = usize::from(db::clamp_ai_batch_refresh_concurrency(concurrency));
    let classic_running = {
        let state = app.state::<AppState>();
        let runtime = state
            .classic_discovery
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        runtime.active_run_id.is_some()
    };
    let ready_jobs = {
        let state = app.state::<AppState>();
        let conn = state
            .db
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        if classic_running {
            db::list_ai_analysis_queue_ready_jobs_by_source(&conn, AiAnalysisQueueSource::NewRelease)?
        } else {
            db::list_ai_analysis_queue_ready_jobs(&conn)?
        }
    };

    if ready_jobs.is_empty() {
        let state = app.state::<AppState>();
        let mut runtime = state
            .ai_batch_refresh
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        runtime.finish_batch();
        return Ok(());
    }

    {
        let state = app.state::<AppState>();
        let mut runtime = state
            .ai_batch_refresh
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        runtime.load_pending_jobs(ready_jobs.iter().map(|job| job.appid), concurrency as u8);
    }

    let mut in_flight = FuturesUnordered::new();

    loop {
        while in_flight.len() < concurrency {
            let Some(appid) = take_next_appid(&app)? else {
                break;
            };
            let app_handle = app.clone();
            in_flight.push(async move {
                let state = app_handle.state::<AppState>();
                let result =
                    commands::generate_or_load_game_analysis(state.inner(), appid, true).await;
                (appid, result)
            });
        }

        let Some((appid, outcome)) = in_flight.next().await else {
            break;
        };

        let error_text = outcome.as_ref().err().map(|error| error.to_string());
        {
            let state = app.state::<AppState>();
            let conn = state
                .db
                .lock()
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            match error_text.as_deref() {
                None => db::delete_ai_analysis_queue_job(&conn, appid)?,
                Some(error) => {
                    let current_attempt = db::load_ai_analysis_queue_job(&conn, appid)?
                        .map(|job| job.attempt)
                        .unwrap_or(1);
                    let next_attempt = current_attempt.saturating_add(1);
                    db::update_ai_analysis_queue_job(&conn, appid, next_attempt, Some(error))?;
                }
            }
        }
        let state = app.state::<AppState>();
        let mut runtime = state
            .ai_batch_refresh
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        runtime.finish_job(appid, error_text.is_none(), error_text);
    }

    let state = app.state::<AppState>();
    let mut runtime = state
        .ai_batch_refresh
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    runtime.finish_batch();
    Ok(())
}

fn fail_worker(app: &AppHandle, error: String) -> Result<()> {
    let state = app.state::<AppState>();
    let mut runtime = state
        .ai_batch_refresh
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    runtime.fail_batch(error);
    Ok(())
}

fn take_next_appid(app: &AppHandle) -> Result<Option<u32>> {
    let state = app.state::<AppState>();
    let mut runtime = state
        .ai_batch_refresh
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok(runtime.take_next_job())
}

#[cfg(test)]
mod tests {
    use super::AiBatchRefreshRuntimeState;

    #[test]
    fn snapshot_tracks_batch_refresh_progress() {
        let mut runtime = AiBatchRefreshRuntimeState::default();
        assert!(runtime.start(4, 5));

        runtime.mark_job_started();
        runtime.mark_job_started();
        runtime.finish_job(101, true, None);
        runtime.finish_job(202, false, Some("timeout".to_string()));
        runtime.finish_batch();

        let snapshot = runtime.snapshot();
        assert!(!snapshot.running);
        assert_eq!(snapshot.concurrency, 5);
        assert_eq!(snapshot.total_count, 4);
        assert_eq!(snapshot.processed_count, 2);
        assert_eq!(snapshot.updated_count, 1);
        assert_eq!(snapshot.failed_count, 1);
        assert_eq!(snapshot.last_error.as_deref(), Some("timeout"));
        assert_eq!(snapshot.last_error_appid, Some(202));
    }

    #[test]
    fn placeholder_activation_clears_stale_counters_before_worker_loads_jobs() {
        let mut runtime = AiBatchRefreshRuntimeState::default();
        assert!(runtime.start(4, 5));
        runtime.mark_job_started();
        runtime.finish_job(202, false, Some("timeout".to_string()));
        runtime.finish_batch();

        assert!(runtime.activate_placeholder(3));

        let snapshot = runtime.snapshot();
        assert!(snapshot.running);
        assert_eq!(snapshot.concurrency, 3);
        assert_eq!(snapshot.total_count, 0);
        assert_eq!(snapshot.processed_count, 0);
        assert_eq!(snapshot.updated_count, 0);
        assert_eq!(snapshot.failed_count, 0);
        assert_eq!(snapshot.last_error, None);
        assert_eq!(snapshot.last_error_appid, None);
    }

    #[test]
    fn loading_pending_jobs_resets_previous_progress_even_when_runtime_is_active() {
        let mut runtime = AiBatchRefreshRuntimeState::default();
        assert!(runtime.start(4, 5));
        runtime.mark_job_started();
        runtime.finish_job(202, false, Some("timeout".to_string()));

        runtime.load_pending_jobs([303], 2);

        let snapshot = runtime.snapshot();
        assert!(snapshot.running);
        assert_eq!(snapshot.concurrency, 2);
        assert_eq!(snapshot.pending_count, 1);
        assert_eq!(snapshot.total_count, 1);
        assert_eq!(snapshot.processed_count, 0);
        assert_eq!(snapshot.updated_count, 0);
        assert_eq!(snapshot.failed_count, 0);
        assert_eq!(snapshot.last_error, None);
        assert_eq!(snapshot.last_error_appid, None);
    }
}
