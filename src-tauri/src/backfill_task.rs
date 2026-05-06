use crate::commands;
use crate::db;
use crate::models::AiAnalysisQueueSource;
use crate::state::AppState;
use crate::steam::{self, SteamGameSnapshotEnrichment};
use anyhow::{Context, Result};
use std::collections::{HashSet, VecDeque};
use std::time::Duration;
use tauri::{AppHandle, Manager};

// Keep a small gap between full snapshot requests so we speed up enrichment
// without stacking discovery traffic and backfill traffic at the same time.
pub const BACKFILL_DELAY_BETWEEN_GAMES: Duration = Duration::from_millis(500);
pub const BACKFILL_MAX_ATTEMPTS: u8 = 2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackfillRuntimeSnapshot {
    pub running: bool,
    pub pending_count: usize,
    pub current_appid: Option<u32>,
    pub current_attempt: Option<u8>,
    pub total_count: usize,
    pub processed_count: usize,
    pub failed_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackfillJob {
    pub appid: u32,
    pub attempt: u8,
}

#[derive(Debug, Default)]
pub struct BackfillRuntimeState {
    pub active: bool,
    pending: VecDeque<BackfillJob>,
    tracked_appids: HashSet<u32>,
    in_progress: Option<BackfillJob>,
    total_count: usize,
    processed_count: usize,
    failed_count: usize,
}

impl BackfillRuntimeState {
    fn reset_progress_for_new_batch(&mut self) {
        if self.pending.is_empty() && self.tracked_appids.is_empty() && self.in_progress.is_none() {
            self.total_count = 0;
            self.processed_count = 0;
            self.failed_count = 0;
        }
    }

    pub fn enqueue<I>(&mut self, appids: I) -> usize
    where
        I: IntoIterator<Item = u32>,
    {
        let mut added = 0usize;

        for appid in appids {
            if !self.tracked_appids.insert(appid) {
                continue;
            }

            if added == 0 {
                self.reset_progress_for_new_batch();
            }
            self.pending.push_back(BackfillJob { appid, attempt: 1 });
            self.total_count += 1;
            added += 1;
        }

        added
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn restore<I>(&mut self, jobs: I)
    where
        I: IntoIterator<Item = BackfillJob>,
    {
        let mut added = 0usize;

        for job in jobs {
            if !self.tracked_appids.insert(job.appid) {
                continue;
            }

            if added == 0 {
                self.reset_progress_for_new_batch();
            }
            self.pending.push_back(job);
            self.total_count += 1;
            added += 1;
        }
    }

    pub fn take_next_job(&mut self) -> Option<BackfillJob> {
        let job = self.pending.pop_front()?;
        self.in_progress = Some(job);
        Some(job)
    }

    pub fn finish_job(&mut self, job: BackfillJob, succeeded: bool) -> bool {
        self.in_progress = None;

        if succeeded {
            self.processed_count += 1;
            self.tracked_appids.remove(&job.appid);
            return true;
        }

        if job.attempt < BACKFILL_MAX_ATTEMPTS {
            self.pending.push_back(BackfillJob {
                appid: job.appid,
                attempt: job.attempt + 1,
            });
            return true;
        }

        self.processed_count += 1;
        self.failed_count += 1;
        self.tracked_appids.remove(&job.appid);
        false
    }

    pub fn snapshot(&self) -> BackfillRuntimeSnapshot {
        BackfillRuntimeSnapshot {
            running: self.active,
            pending_count: self.tracked_appids.len(),
            current_appid: self.in_progress.map(|job| job.appid),
            current_attempt: self.in_progress.map(|job| job.attempt),
            total_count: self.total_count,
            processed_count: self.processed_count,
            failed_count: self.failed_count,
        }
    }
}

pub fn enqueue_backfill(app: &AppHandle, appids: impl IntoIterator<Item = u32>) -> Result<usize> {
    let inserted_jobs = {
        let state = app.state::<AppState>();
        let conn = state
            .db
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        db::enqueue_metadata_backfill_jobs(&conn, appids)?
    };
    if inserted_jobs.is_empty() {
        return Ok(0);
    }

    let (added, should_spawn) = {
        let state = app.state::<AppState>();
        let mut runtime = state
            .backfill
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        let jobs = inserted_jobs
            .iter()
            .map(|job| BackfillJob {
                appid: job.appid,
                attempt: job.attempt,
            })
            .collect::<Vec<_>>();
        runtime.restore(jobs);
        let added = inserted_jobs.len();
        let should_spawn = added > 0 && !runtime.active;
        if should_spawn {
            runtime.active = true;
        }
        (added, should_spawn)
    };

    if should_spawn {
        spawn_backfill_worker(app.clone());
    }

    Ok(added)
}

pub fn restore_backfill_runtime(app: AppHandle) -> Result<()> {
    let records = {
        let state = app.state::<AppState>();
        let conn = state
            .db
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        db::list_metadata_backfill_jobs(&conn)?
    };
    if records.is_empty() {
        return Ok(());
    }

    let should_spawn = {
        let state = app.state::<AppState>();
        let mut runtime = state
            .backfill
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        runtime.restore(records.into_iter().map(|record| BackfillJob {
            appid: record.appid,
            attempt: record.attempt,
        }));
        if runtime.active {
            false
        } else {
            runtime.active = true;
            true
        }
    };

    if should_spawn {
        spawn_backfill_worker(app);
    }

    Ok(())
}

pub fn spawn_backfill_worker(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = run_backfill_worker(app.clone()).await {
            eprintln!("metadata backfill worker failed: {error:#}");
            let _ = clear_worker_active(&app);
        }
        crate::auto_scheduler::kick(app);
    });
}

async fn run_backfill_worker(app: AppHandle) -> Result<()> {
    loop {
        let job = {
            let state = app.state::<AppState>();
            let mut runtime = state
                .backfill
                .lock()
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            let Some(job) = runtime.take_next_job() else {
                runtime.active = false;
                return Ok(());
            };
            job
        };

        let outcome = backfill_one_game(&app, job.appid).await;
        let error_text = outcome.as_ref().err().map(ToString::to_string);
        let will_retry = outcome.is_err() && job.attempt < BACKFILL_MAX_ATTEMPTS;
        {
            let state = app.state::<AppState>();
            let conn = state
                .db
                .lock()
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            let mut runtime = state
                .backfill
                .lock()
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            match error_text.as_deref() {
                None => {
                    db::delete_metadata_backfill_job(&conn, job.appid)?;
                    runtime.finish_job(job, true);
                }
                Some(error) => {
                    if will_retry {
                        db::update_metadata_backfill_attempt(
                            &conn,
                            job.appid,
                            job.attempt + 1,
                            Some(error),
                        )?;
                    } else {
                        db::delete_metadata_backfill_job(&conn, job.appid)?;
                    }
                    runtime.finish_job(job, false);
                }
            }
            if let Some(error) = error_text {
                eprintln!(
                    "metadata backfill for app {} attempt {} failed: {error:#}",
                    job.appid, job.attempt
                );
                if !will_retry {
                    eprintln!("metadata backfill for app {} exhausted retries", job.appid);
                }
            }
        }

        tokio::time::sleep(BACKFILL_DELAY_BETWEEN_GAMES).await;
    }
}

async fn backfill_one_game(app: &AppHandle, appid: u32) -> Result<()> {
    let (http, country, language) = {
        let state = app.state::<AppState>();
        let conn = state
            .db
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        let config = db::public_config(&conn)?;
        (state.http.clone(), config.country, config.language)
    };

    let snapshot = steam::fetch_game_snapshot(
        &http,
        appid,
        &country,
        &language,
        SteamGameSnapshotEnrichment::Full,
    )
    .await
    .with_context(|| format!("fetch full metadata snapshot for appid {appid}"))?;

    let state = app.state::<AppState>();
    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let Some(existing) = db::load_game(&conn, appid)? else {
        return Ok(());
    };
    let merged = commands::merge_snapshot(existing, snapshot);
    db::upsert_game(&conn, &merged)?;
    db::mark_sync_complete(&conn)?;
    db::enqueue_ai_analysis_jobs(&conn, AiAnalysisQueueSource::NewRelease, [appid])?;
    Ok(())
}

fn clear_worker_active(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let mut runtime = state
        .backfill
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    runtime.active = false;
    runtime.pending.clear();
    runtime.tracked_appids.clear();
    runtime.in_progress = None;
    Ok(())
}
