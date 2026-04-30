use crate::commands;
use crate::db;
use crate::models::SyncMode;
use crate::state::AppState;
use crate::steam::{self, SteamGameSnapshotEnrichment};
use anyhow::{Context, Result};
use reqwest::StatusCode;
use std::collections::VecDeque;
use std::time::Duration;
use tauri::{AppHandle, Manager};

pub const SYNC_DELAY_BETWEEN_QUICK_GAMES: Duration = Duration::from_millis(350);
pub const SYNC_DELAY_BETWEEN_FULL_GAMES: Duration = Duration::from_millis(900);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncRuntimeSnapshot {
    pub running: bool,
    pub mode: Option<SyncMode>,
    pub pending_count: usize,
    pub current_appid: Option<u32>,
    pub total_count: usize,
    pub processed_count: usize,
    pub updated_count: usize,
    pub failed_count: usize,
    pub last_error: Option<String>,
    pub last_error_appid: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncJob {
    pub appid: u32,
    pub mode: SyncMode,
    pub attempt: u8,
}

#[derive(Debug, Default)]
pub struct SyncRuntimeState {
    pub active: bool,
    mode: Option<SyncMode>,
    pending: VecDeque<SyncJob>,
    current_job: Option<SyncJob>,
    total_count: usize,
    processed_count: usize,
    updated_count: usize,
    failed_count: usize,
    last_error: Option<String>,
    last_error_appid: Option<u32>,
}

impl SyncRuntimeState {
    pub fn start(&mut self, jobs: Vec<SyncJob>, mode: SyncMode) -> bool {
        if self.active {
            return false;
        }

        self.active = true;
        self.mode = Some(mode);
        self.pending = jobs.into();
        self.current_job = None;
        self.total_count = self.pending.len();
        self.processed_count = 0;
        self.updated_count = 0;
        self.failed_count = 0;
        self.last_error = None;
        self.last_error_appid = None;
        true
    }

    pub fn take_next_job(&mut self) -> Option<SyncJob> {
        let job = self.pending.pop_front()?;
        self.mode = Some(job.mode);
        self.current_job = Some(job);
        Some(job)
    }

    pub fn finish_current(&mut self, updated: bool, error: Option<String>) {
        let current_appid = self.current_job.take().map(|job| job.appid);
        self.processed_count += 1;

        if updated {
            self.updated_count += 1;
        }

        if let Some(error) = error {
            self.failed_count += 1;
            self.last_error = Some(error);
            self.last_error_appid = current_appid;
        }
    }

    pub fn finish_batch(&mut self) {
        self.active = false;
        self.pending.clear();
        self.current_job = None;
    }

    pub fn clear_active(&mut self) {
        self.active = false;
        self.mode = None;
        self.pending.clear();
        self.current_job = None;
        self.total_count = 0;
        self.processed_count = 0;
        self.updated_count = 0;
        self.failed_count = 0;
        self.last_error = None;
        self.last_error_appid = None;
    }

    pub fn snapshot(&self) -> SyncRuntimeSnapshot {
        SyncRuntimeSnapshot {
            running: self.active,
            mode: self.mode,
            pending_count: self.pending.len() + if self.current_job.is_some() { 1 } else { 0 },
            current_appid: self.current_job.map(|job| job.appid),
            total_count: self.total_count,
            processed_count: self.processed_count,
            updated_count: self.updated_count,
            failed_count: self.failed_count,
            last_error: self.last_error.clone(),
            last_error_appid: self.last_error_appid,
        }
    }
}

pub fn sync_jobs_from_records(records: Vec<db::SyncJobRecord>) -> Vec<SyncJob> {
    records
        .into_iter()
        .map(|record| SyncJob {
            appid: record.appid,
            mode: record.mode,
            attempt: record.attempt,
        })
        .collect()
}

pub fn spawn_sync_worker(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = run_sync_worker(app.clone()).await {
            eprintln!("steam sync worker failed: {error:#}");
            let _ = clear_worker_active(&app);
        }
    });
}

async fn run_sync_worker(app: AppHandle) -> Result<()> {
    loop {
        let maybe_job = {
            let state = app.state::<AppState>();
            let mut runtime = state
                .sync
                .lock()
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            runtime.take_next_job()
        };
        let Some(job) = maybe_job else {
            let state = app.state::<AppState>();
            let conn = state
                .db
                .lock()
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            db::mark_sync_complete(&conn)?;
            drop(conn);

            let mut runtime = state
                .sync
                .lock()
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            runtime.finish_batch();
            return Ok(());
        };

        let outcome = sync_one_game(&app, job.appid, job.mode).await;
        let should_resume_later = outcome
            .as_ref()
            .err()
            .map(should_preserve_sync_job)
            .unwrap_or(false);

        {
            let state = app.state::<AppState>();
            let conn = state
                .db
                .lock()
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            let mut runtime = state
                .sync
                .lock()
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;

            match outcome {
                Ok(updated) => {
                    db::delete_sync_job(&conn, job.appid)?;
                    runtime.finish_current(updated, None);
                }
                Err(error) => {
                    let summary = summarize_sync_error(job.appid, &error);
                    eprintln!(
                        "steam sync for app {} attempt {} failed: {error:#}",
                        job.appid, job.attempt
                    );

                    if should_resume_later {
                        db::update_sync_job(
                            &conn,
                            job.appid,
                            job.mode,
                            next_sync_attempt(job.attempt),
                            Some(&summary),
                        )?;
                        runtime.finish_current(false, Some(summary));
                        runtime.finish_batch();
                        return Ok(());
                    }

                    db::delete_sync_job(&conn, job.appid)?;
                    runtime.finish_current(false, Some(summary));
                }
            }
        }

        tokio::time::sleep(sync_delay_for_mode(job.mode)).await;
    }
}

async fn sync_one_game(app: &AppHandle, appid: u32, mode: SyncMode) -> Result<bool> {
    let (http, country, language) = {
        let state = app.state::<AppState>();
        let conn = state
            .db
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        let config = db::public_config(&conn)?;
        (state.http.clone(), config.country, config.language)
    };

    let snapshot =
        steam::fetch_game_snapshot(&http, appid, &country, &language, sync_enrichment(mode))
            .await
            .with_context(|| format!("fetch sync metadata snapshot for appid {appid}"))?;

    let state = app.state::<AppState>();
    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let Some(existing) = db::load_game(&conn, appid)? else {
        return Ok(false);
    };
    let merged = commands::merge_snapshot(existing, snapshot);
    db::upsert_game(&conn, &merged)?;
    Ok(true)
}

fn clear_worker_active(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let mut runtime = state
        .sync
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    runtime.clear_active();
    Ok(())
}

fn sync_enrichment(mode: SyncMode) -> SteamGameSnapshotEnrichment {
    match mode {
        SyncMode::Quick => SteamGameSnapshotEnrichment::Sync,
        SyncMode::Full => SteamGameSnapshotEnrichment::Full,
    }
}

fn sync_delay_for_mode(mode: SyncMode) -> Duration {
    match mode {
        SyncMode::Quick => SYNC_DELAY_BETWEEN_QUICK_GAMES,
        SyncMode::Full => SYNC_DELAY_BETWEEN_FULL_GAMES,
    }
}

fn next_sync_attempt(attempt: u8) -> u8 {
    attempt.saturating_add(1).max(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncErrorKind {
    RateLimited,
    Timeout,
    Network,
    RetryableRequest,
    Other,
}

fn should_preserve_sync_job(error: &anyhow::Error) -> bool {
    matches!(
        classify_sync_error(error),
        SyncErrorKind::RateLimited
            | SyncErrorKind::Timeout
            | SyncErrorKind::Network
            | SyncErrorKind::RetryableRequest
    )
}

fn summarize_sync_error(appid: u32, error: &anyhow::Error) -> String {
    match classify_sync_error(error) {
        SyncErrorKind::RateLimited => {
            format!("Steam 请求过于频繁（429）。AppID {appid} 已保留到待续同步队列，稍后继续即可。")
        }
        SyncErrorKind::Timeout => {
            format!("Steam 请求超时。AppID {appid} 已保留到待续同步队列，稍后继续即可。")
        }
        SyncErrorKind::Network => {
            format!("Steam 网络连接异常。AppID {appid} 已保留到待续同步队列，稍后继续即可。")
        }
        SyncErrorKind::RetryableRequest => {
            format!("Steam 临时响应异常。AppID {appid} 已保留到待续同步队列，稍后继续即可。")
        }
        SyncErrorKind::Other => {
            let message = error.root_cause().to_string().replace('\n', " ");
            format!("AppID {appid} 同步失败：{message}")
        }
    }
}

fn classify_sync_error(error: &anyhow::Error) -> SyncErrorKind {
    for cause in error.chain() {
        if let Some(reqwest_error) = cause.downcast_ref::<reqwest::Error>() {
            if reqwest_error.status() == Some(StatusCode::TOO_MANY_REQUESTS) {
                return SyncErrorKind::RateLimited;
            }
            if reqwest_error.is_timeout() {
                return SyncErrorKind::Timeout;
            }
            if reqwest_error.is_connect() {
                return SyncErrorKind::Network;
            }
            if reqwest_error.is_request() {
                return SyncErrorKind::RetryableRequest;
            }
        }
    }

    let lower = format!("{error:#}").to_ascii_lowercase();
    if lower.contains("http 429") || lower.contains("too many requests") {
        return SyncErrorKind::RateLimited;
    }
    if lower.contains("请求超时") || lower.contains("timeout") {
        return SyncErrorKind::Timeout;
    }
    if lower.contains("网络连接失败") || lower.contains("connection") {
        return SyncErrorKind::Network;
    }
    if lower.contains("steam returned http 5") || lower.contains("request failed") {
        return SyncErrorKind::RetryableRequest;
    }

    SyncErrorKind::Other
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn sync_runtime_snapshot_tracks_selected_mode() {
        let mut runtime = SyncRuntimeState::default();

        assert!(runtime.start(
            vec![
                SyncJob {
                    appid: 10,
                    mode: SyncMode::Quick,
                    attempt: 1,
                },
                SyncJob {
                    appid: 20,
                    mode: SyncMode::Quick,
                    attempt: 1,
                },
            ],
            SyncMode::Quick
        ));
        let snapshot = runtime.snapshot();

        assert_eq!(snapshot.mode, Some(SyncMode::Quick));
        assert_eq!(snapshot.total_count, 2);
        assert_eq!(snapshot.pending_count, 2);
    }

    #[test]
    fn sync_mode_maps_to_expected_snapshot_enrichment() {
        assert!(matches!(
            sync_enrichment(SyncMode::Quick),
            SteamGameSnapshotEnrichment::Sync
        ));
        assert!(matches!(
            sync_enrichment(SyncMode::Full),
            SteamGameSnapshotEnrichment::Full
        ));
    }

    #[test]
    fn sync_error_summary_compacts_rate_limit_failures() {
        let error = anyhow!(
            "fetch sync metadata snapshot for appid 2182680: fetch appdetails for appid 2182680: Steam appdetails for appid 2182680: Steam returned HTTP 429 Too Many Requests"
        );

        assert_eq!(
            summarize_sync_error(2182680, &error),
            "Steam 请求过于频繁（429）。AppID 2182680 已保留到待续同步队列，稍后继续即可。"
        );
    }
}
