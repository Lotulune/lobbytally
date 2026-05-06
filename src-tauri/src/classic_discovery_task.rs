use crate::auto_scheduler;
use crate::db::{self, ClassicDiscoveryProgressPatch};
use crate::models::{
    AiAnalysisQueueSource, ClassicDiscoveryRejectCacheEntry, ClassicDiscoveryRunSnapshot,
    ClassicRejectReasonCode, DiscoveryRunStatus, StoreReleaseState,
};
use crate::state::AppState;
use crate::steam::{self, SteamGameSnapshotEnrichment};
use anyhow::Result;
use std::collections::HashSet;
use tauri::{AppHandle, Emitter, Manager};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClassicDiscoveryControl {
    #[default]
    None,
    CancelRequested,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassicDiscoveryRuntimeSnapshot {
    pub running: bool,
    pub active_run_id: Option<i64>,
}

#[derive(Debug, Default)]
pub struct ClassicDiscoveryRuntimeState {
    pub active_run_id: Option<i64>,
    pub control: ClassicDiscoveryControl,
}

impl ClassicDiscoveryRuntimeState {
    pub fn snapshot(&self) -> ClassicDiscoveryRuntimeSnapshot {
        ClassicDiscoveryRuntimeSnapshot {
            running: self.active_run_id.is_some(),
            active_run_id: self.active_run_id,
        }
    }
}

pub const CLASSIC_DISCOVERY_TASK_EVENT: &str = "classic-discovery-task-updated";

pub fn emit_snapshot(app: &AppHandle, snapshot: &ClassicDiscoveryRunSnapshot) {
    let _ = app.emit(CLASSIC_DISCOVERY_TASK_EVENT, snapshot.clone());
}

pub fn spawn_classic_discovery_worker(app: AppHandle, run_id: i64) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = run_classic_discovery_worker(app.clone(), run_id).await {
            eprintln!("classic discovery worker {run_id} failed: {error:#}");
            let _ = fail_run(&app, run_id, error.to_string());
        }
        auto_scheduler::kick(app);
    });
}

pub fn restore_classic_discovery_runtime(app: AppHandle) -> Result<()> {
    let latest = {
        let state = app.state::<AppState>();
        let conn = state
            .db
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        db::load_latest_classic_discovery_run(&conn)?
    };
    let Some(snapshot) = latest else {
        return Ok(());
    };
    if !snapshot.can_resume() {
        return Ok(());
    }
    let state = app.state::<AppState>();
    let mut runtime = state
        .classic_discovery
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    if runtime.active_run_id.is_some() {
        return Ok(());
    }
    runtime.active_run_id = Some(snapshot.id);
    runtime.control = ClassicDiscoveryControl::None;
    drop(runtime);
    spawn_classic_discovery_worker(app, snapshot.id);
    Ok(())
}

async fn run_classic_discovery_worker(app: AppHandle, run_id: i64) -> Result<()> {
    let (http, country, language, mut snapshot, mut known_appids) = {
        let state = app.state::<AppState>();
        let conn = state
            .db
            .lock()
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        let config = db::public_config(&conn)?;
        let snapshot = db::load_classic_discovery_run(&conn, run_id)?
            .ok_or_else(|| anyhow::anyhow!("classic discovery run {run_id} not found"))?;
        (
            state.http.clone(),
            config.country,
            config.language,
            snapshot,
            db::list_game_appids(&conn)?
                .into_iter()
                .collect::<HashSet<_>>(),
        )
    };
    emit_snapshot(&app, &snapshot);
    let now = now_rfc3339()?;
    let mut resume_anchor = if snapshot.can_resume() {
        snapshot.last_appid
    } else {
        None
    };

    while snapshot.pages_processed < snapshot.max_pages {
        if should_yield_to_higher_priority(&app)? {
            snapshot.status = DiscoveryRunStatus::Interrupted;
            snapshot.current_appid = None;
            snapshot.finished_at = None;
            persist_snapshot(&app, run_id, &snapshot)?;
            clear_runtime_if_active(&app, run_id)?;
            return Ok(());
        }
        match current_control(&app)? {
            ClassicDiscoveryControl::CancelRequested => {
                snapshot.status = DiscoveryRunStatus::Cancelled;
                snapshot.current_appid = None;
                snapshot.finished_at = Some(now_rfc3339()?);
                snapshot.last_error = None;
                persist_snapshot(&app, run_id, &snapshot)?;
                clear_runtime_if_active(&app, run_id)?;
                return Ok(());
            }
            ClassicDiscoveryControl::None => {}
        }

        let start = snapshot.pages_processed.saturating_mul(snapshot.page_size);
        let mut preview =
            steam::fetch_classic_search_candidates(&http, start, snapshot.page_size, &language)
                .await?;
        let added_before_page = snapshot.added_games;

        let anchor_consumed;
        if let Some(anchor) = resume_anchor {
            let (trimmed, consumed) = trim_page_after_resume_anchor(preview.apps, anchor);
            preview.apps = trimmed;
            anchor_consumed = consumed;
            if consumed {
                resume_anchor = None;
            }
        } else {
            anchor_consumed = false;
        }

        if preview.apps.is_empty() {
            snapshot.consecutive_empty_pages = snapshot.consecutive_empty_pages.saturating_add(1);
            if anchor_consumed {
                snapshot.consecutive_empty_pages = 0;
            }
            persist_snapshot(&app, run_id, &snapshot)?;
            if !preview.have_more_results {
                break;
            }
            if snapshot.consecutive_empty_pages >= 2 {
                break;
            }
            continue;
        }

        for app_item in preview.apps {
            snapshot.current_appid = Some(app_item.appid);
            persist_snapshot(&app, run_id, &snapshot)?;

            if known_appids.contains(&app_item.appid) {
                snapshot.scanned_apps += 1;
                snapshot.skipped_existing += 1;
                snapshot.last_appid = Some(app_item.appid);
                snapshot.current_appid = None;
                persist_snapshot(&app, run_id, &snapshot)?;
                continue;
            }

            {
                let state = app.state::<AppState>();
                let conn = state
                    .db
                    .lock()
                    .map_err(|err| anyhow::anyhow!(err.to_string()))?;
                if !db::classic_reject_cache_allows_retry(&conn, app_item.appid, &now)? {
                    snapshot.scanned_apps += 1;
                    snapshot.skipped_rejected_cache += 1;
                    snapshot.last_appid = Some(app_item.appid);
                    snapshot.current_appid = None;
                    persist_snapshot(&app, run_id, &snapshot)?;
                    continue;
                }
            }

            snapshot.scanned_apps += 1;
            snapshot.considered_apps += 1;
            let fetched = steam::fetch_game_snapshot(
                &http,
                app_item.appid,
                &country,
                &language,
                SteamGameSnapshotEnrichment::Full,
            )
            .await;

            match fetched {
                Ok(full_snapshot) => {
                    if let Some(reason_code) = reject_reason(&full_snapshot) {
                        snapshot.rejected_games += 1;
                        let state = app.state::<AppState>();
                        let conn = state
                            .db
                            .lock()
                            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
                        db::save_classic_reject_cache_entry(
                            &conn,
                            &ClassicDiscoveryRejectCacheEntry {
                                appid: app_item.appid,
                                reason_code,
                                positive_review_pct: full_snapshot.positive_review_pct,
                                total_reviews: full_snapshot.total_reviews,
                                current_players: full_snapshot.current_players,
                                release_state: full_snapshot
                                    .release_state
                                    .unwrap_or(StoreReleaseState::Unknown),
                                release_date: full_snapshot.release_date.clone(),
                                checked_at: now.clone(),
                                rule_version: db::CLASSIC_DISCOVERY_RULE_VERSION.to_string(),
                            },
                        )?;
                    } else if let Some(mut card) = crate::discovery::build_discovered_game_card(
                        &app_item,
                        full_snapshot,
                        &crate::recommendation::today_iso_utc(),
                    ) {
                        if matches!(card.section.as_str(), "classic" | "classic_hidden") {
                            card.recommendation_score = db::score_card(&card);
                            let state = app.state::<AppState>();
                            let conn = state
                                .db
                                .lock()
                                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
                            db::upsert_game(&conn, &card)?;
                            db::enqueue_ai_analysis_jobs(
                                &conn,
                                AiAnalysisQueueSource::Classic,
                                [card.appid],
                            )?;
                            snapshot.added_games += 1;
                            known_appids.insert(card.appid);
                        } else {
                            snapshot.rejected_games += 1;
                        }
                    } else {
                        snapshot.rejected_games += 1;
                    }
                }
                Err(error) => {
                    snapshot.failed_games += 1;
                    snapshot.last_error = Some(error.to_string());
                }
            }

            snapshot.last_appid = Some(app_item.appid);
            snapshot.current_appid = None;
            persist_snapshot(&app, run_id, &snapshot)?;
        }

        snapshot.pages_processed = snapshot.pages_processed.saturating_add(1);

        if snapshot.added_games == added_before_page {
            snapshot.consecutive_empty_pages = snapshot.consecutive_empty_pages.saturating_add(1);
        } else {
            snapshot.consecutive_empty_pages = 0;
        }
        persist_snapshot(&app, run_id, &snapshot)?;

        if snapshot.consecutive_empty_pages >= 2 || !preview.have_more_results {
            break;
        }
    }

    snapshot.status = DiscoveryRunStatus::Completed;
    snapshot.current_appid = None;
    snapshot.finished_at = Some(now_rfc3339()?);
    snapshot.last_error = None;
    persist_snapshot(&app, run_id, &snapshot)?;
    clear_runtime_if_active(&app, run_id)?;
    Ok(())
}

fn reject_reason(snapshot: &steam::SteamGameSnapshot) -> Option<ClassicRejectReasonCode> {
    if snapshot.multiplayer_modes.is_empty() {
        return Some(ClassicRejectReasonCode::NonMultiplayer);
    }
    let release_state = snapshot
        .release_state
        .clone()
        .unwrap_or(StoreReleaseState::Unknown);
    if release_state != StoreReleaseState::Released {
        return Some(ClassicRejectReasonCode::NotReleased);
    }
    let release_date = snapshot.release_date.as_deref();
    let days_since_release = release_date
        .and_then(|date| {
            let today = crate::recommendation::today_iso_utc();
            let release = time::Date::parse(
                date,
                time::macros::format_description!("[year]-[month]-[day]"),
            )
            .ok()?;
            let today = time::Date::parse(
                &today,
                time::macros::format_description!("[year]-[month]-[day]"),
            )
            .ok()?;
            Some((today - release).whole_days())
        })
        .unwrap_or_default();
    if days_since_release <= 30 {
        return Some(ClassicRejectReasonCode::TooNew);
    }
    let total_reviews = snapshot.total_reviews.unwrap_or(0);
    let positive_review_pct = snapshot.positive_review_pct.unwrap_or(0.0);
    if total_reviews < 300 {
        return Some(ClassicRejectReasonCode::LowReviewCount);
    }
    if positive_review_pct < 60.0 {
        return Some(ClassicRejectReasonCode::LowPositiveReviewPct);
    }
    None
}

fn persist_snapshot(
    app: &AppHandle,
    run_id: i64,
    snapshot: &ClassicDiscoveryRunSnapshot,
) -> Result<()> {
    let state = app.state::<AppState>();
    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    db::update_classic_discovery_run_progress(
        &conn,
        run_id,
        ClassicDiscoveryProgressPatch {
            status: Some(snapshot.status.clone()),
            pages_processed: Some(snapshot.pages_processed),
            scanned_apps: Some(snapshot.scanned_apps),
            considered_apps: Some(snapshot.considered_apps),
            added_games: Some(snapshot.added_games),
            rejected_games: Some(snapshot.rejected_games),
            skipped_existing: Some(snapshot.skipped_existing),
            skipped_rejected_cache: Some(snapshot.skipped_rejected_cache),
            failed_games: Some(snapshot.failed_games),
            current_appid: Some(snapshot.current_appid),
            last_appid: Some(snapshot.last_appid),
            consecutive_empty_pages: Some(snapshot.consecutive_empty_pages),
            last_error: Some(snapshot.last_error.clone()),
            finished_at: Some(snapshot.finished_at.clone()),
        },
    )?;
    let stored = db::load_classic_discovery_run(&conn, run_id)?
        .ok_or_else(|| anyhow::anyhow!("classic discovery run {run_id} disappeared"))?;
    drop(conn);
    emit_snapshot(app, &stored);
    Ok(())
}

fn fail_run(app: &AppHandle, run_id: i64, error: String) -> Result<()> {
    let state = app.state::<AppState>();
    let conn = state
        .db
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let Some(snapshot) = db::load_classic_discovery_run(&conn, run_id)? else {
        clear_runtime_if_active(app, run_id)?;
        return Ok(());
    };
    drop(conn);
    persist_snapshot(
        app,
        run_id,
        &ClassicDiscoveryRunSnapshot {
            status: DiscoveryRunStatus::Failed,
            current_appid: None,
            finished_at: Some(now_rfc3339()?),
            last_error: Some(error),
            ..snapshot
        },
    )?;
    clear_runtime_if_active(app, run_id)?;
    Ok(())
}

fn should_yield_to_higher_priority(app: &AppHandle) -> Result<bool> {
    let state = app.state::<AppState>();
    let discovery = state
        .discovery
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    if discovery.active_run_id.is_some() {
        return Ok(true);
    }
    drop(discovery);
    let backfill = state
        .backfill
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok(backfill.active || backfill.snapshot().pending_count > 0)
}

fn current_control(app: &AppHandle) -> Result<ClassicDiscoveryControl> {
    let state = app.state::<AppState>();
    let runtime = state
        .classic_discovery
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok(runtime.control)
}

fn clear_runtime_if_active(app: &AppHandle, run_id: i64) -> Result<()> {
    let state = app.state::<AppState>();
    let mut runtime = state
        .classic_discovery
        .lock()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    if runtime.active_run_id == Some(run_id) {
        runtime.active_run_id = None;
        runtime.control = ClassicDiscoveryControl::None;
    }
    Ok(())
}

fn now_rfc3339() -> Result<String> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

fn trim_page_after_resume_anchor(
    apps: Vec<steam::SteamAppListItem>,
    anchor_appid: u32,
) -> (Vec<steam::SteamAppListItem>, bool) {
    let Some(index) = apps.iter().position(|item| item.appid == anchor_appid) else {
        return (apps, false);
    };
    (apps.into_iter().skip(index + 1).collect(), true)
}

#[cfg(test)]
mod tests {
    use super::trim_page_after_resume_anchor;
    use crate::steam::SteamAppListItem;

    #[test]
    fn trim_page_after_resume_anchor_discards_entries_through_anchor() {
        let page = vec![
            SteamAppListItem {
                appid: 100,
                name: "A".to_string(),
            },
            SteamAppListItem {
                appid: 200,
                name: "B".to_string(),
            },
            SteamAppListItem {
                appid: 300,
                name: "C".to_string(),
            },
            SteamAppListItem {
                appid: 400,
                name: "D".to_string(),
            },
        ];

        let (remaining, anchor_consumed) = trim_page_after_resume_anchor(page, 300);

        assert!(anchor_consumed);
        assert_eq!(
            remaining.into_iter().map(|item| item.appid).collect::<Vec<_>>(),
            vec![400]
        );
    }
}
