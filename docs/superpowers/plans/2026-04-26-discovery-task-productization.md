# Discovery Task Productization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the current one-shot Steam AppList scan into a resumable, observable discovery task with progress, failure logs, and history inside the existing Tauri desktop app.

**Architecture:** Keep `preview_steam_app_list` and `build_discovered_game_card` as the pure discovery helpers, but introduce a persisted `discovery_runs` / `discovery_failures` model in SQLite plus a single active background worker in Rust. The worker emits Tauri 2 events to the React frontend, which renders a dedicated discovery console inside Settings and refreshes the dashboard when a run reaches a terminal state.

**Tech Stack:** Tauri 2, Rust, rusqlite, reqwest, React 19, TypeScript, Vite, Vitest, `@tauri-apps/api/event`

---

## Scope Check

The roadmap spec in [docs/2026-04-26-mpgs-current-state-roadmap-spec.md](/D:/AI%20Coding/mpgs/docs/2026-04-26-mpgs-current-state-roadmap-spec.md) covers multiple independent subsystems. This plan intentionally covers only **Phase 1: discovery task productization**.

Out of scope for this plan:

- Multiplayer rule-engine improvements
- Batch AI assessment
- Personalized ranking
- Steam profile / wishlist / friends integration
- Home page information architecture changes beyond showing refreshed data after a task completes

## File Structure

**Backend**

- Modify: `src-tauri/src/models.rs`
  Add discovery request, status, snapshot, and failure payloads shared with the frontend.
- Modify: `src-tauri/src/db.rs`
  Add migration SQL plus helpers for creating, updating, resuming, and listing discovery runs.
- Create: `src-tauri/src/discovery_task.rs`
  Own the background worker, event name constant, progress helpers, and runtime control logic.
- Modify: `src-tauri/src/state.rs`
  Add in-memory single-run control state (`active_run_id`, pause/cancel flag).
- Modify: `src-tauri/src/commands.rs`
  Expose start / pause / resume / cancel / snapshot / history commands.
- Modify: `src-tauri/src/lib.rs`
  Register the new module and commands, and mark stale running jobs as interrupted during startup.
- Create: `src-tauri/tests/discovery_task_tests.rs`
  Cover DB persistence, interruption recovery, progress math, and resumable status rules.

**Frontend**

- Modify: `src/types.ts`
  Mirror the new discovery task payloads for the React app.
- Modify: `src/api/client.ts`
  Add typed task commands, mock-mode fallbacks, and keep `previewSteamAppList()` intact.
- Create: `src/features/discovery/useDiscoveryTask.ts`
  Subscribe to live Tauri events and provide start / pause / resume / cancel actions.
- Create: `src/features/discovery/DiscoveryTaskPanel.tsx`
  Render the discovery console, progress bar, control buttons, failure list, and history.
- Create: `src/features/discovery/DiscoveryTaskPanel.test.tsx`
  Verify paused / running / completed rendering and button enablement in jsdom.
- Modify: `src/App.tsx`
  Replace the one-shot scan form inside `SettingsPanel` with the new discovery console and provide dashboard refresh callbacks.
- Modify: `src/App.css`
  Add discovery console, progress, history, and failure-list styles.

## Task 1: Persist Discovery Runs and Failures in SQLite

**Files:**

- Create: `src-tauri/tests/discovery_task_tests.rs`
- Modify: `src-tauri/src/models.rs`
- Modify: `src-tauri/src/db.rs`

- [ ] **Step 1: Write the failing Rust persistence tests**

```rust
use rusqlite::Connection;
use tauri_app_lib::db;
use tauri_app_lib::models::{DiscoveryRunStatus, DiscoveryTaskRequest};

fn seeded_memory_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    db::migrate(&conn).expect("migrate");
    db::seed_default_games(&conn).expect("seed");
    conn
}

#[test]
fn discovery_run_snapshot_round_trips_with_failures() {
    let conn = seeded_memory_db();

    let run = db::create_discovery_run(
        &conn,
        &DiscoveryTaskRequest {
            target_added_games: 6,
            page_size: 25,
        },
        Some(321_000),
    )
    .expect("create run");

    db::update_discovery_run_progress(
        &conn,
        run.id,
        db::DiscoveryProgressPatch {
            status: Some(DiscoveryRunStatus::Running),
            current_appid: Some(321_012),
            last_appid: Some(321_025),
            pages_processed: Some(1),
            scanned_apps: Some(25),
            added_games: Some(4),
            added_new_games: Some(1),
            added_classic_games: Some(3),
            skipped_existing: Some(8),
            skipped_non_multiplayer: Some(11),
            failed_games: Some(2),
            have_more_results: Some(true),
            last_error: None,
            finished_at: None,
        },
    )
    .expect("update progress");

    db::append_discovery_failure(
        &conn,
        run.id,
        1,
        Some(321_017),
        "fetch_snapshot",
        "steam timeout",
    )
    .expect("append failure");

    let loaded = db::load_discovery_run(&conn, run.id)
        .expect("load run")
        .expect("run exists");

    assert_eq!(loaded.status, DiscoveryRunStatus::Running);
    assert_eq!(loaded.target_added_games, 6);
    assert_eq!(loaded.page_size, 25);
    assert_eq!(loaded.current_appid, Some(321_012));
    assert_eq!(loaded.last_appid, Some(321_025));
    assert_eq!(loaded.added_games, 4);
    assert_eq!(loaded.failures.len(), 1);
    assert_eq!(loaded.failures[0].stage, "fetch_snapshot");
    assert_eq!(loaded.failures[0].appid, Some(321_017));
}

#[test]
fn mark_running_discovery_runs_interrupted_on_startup() {
    let conn = seeded_memory_db();

    let run = db::create_discovery_run(
        &conn,
        &DiscoveryTaskRequest {
            target_added_games: 10,
            page_size: 30,
        },
        Some(555_000),
    )
    .expect("create run");

    db::update_discovery_run_progress(
        &conn,
        run.id,
        db::DiscoveryProgressPatch {
            status: Some(DiscoveryRunStatus::Running),
            current_appid: Some(555_012),
            last_appid: Some(555_030),
            pages_processed: Some(2),
            scanned_apps: Some(60),
            added_games: Some(5),
            added_new_games: Some(2),
            added_classic_games: Some(3),
            skipped_existing: Some(20),
            skipped_non_multiplayer: Some(33),
            failed_games: Some(2),
            have_more_results: Some(true),
            last_error: None,
            finished_at: None,
        },
    )
    .expect("mark running");

    db::mark_running_discovery_runs_interrupted(&conn).expect("interrupt stale runs");

    let interrupted = db::load_discovery_run(&conn, run.id)
        .expect("load interrupted")
        .expect("run exists");

    assert_eq!(interrupted.status, DiscoveryRunStatus::Interrupted);
    assert_eq!(interrupted.added_games, 5);
    assert_eq!(interrupted.last_appid, Some(555_030));
}
```

- [ ] **Step 2: Run the new Rust tests and confirm they fail**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --test discovery_task_tests
```

Expected: FAIL with unresolved items such as `DiscoveryTaskRequest`, `DiscoveryRunStatus`, `create_discovery_run`, `DiscoveryProgressPatch`, and `mark_running_discovery_runs_interrupted`.

- [ ] **Step 3: Implement the shared models and DB helpers**

Add the new shared payloads in `src-tauri/src/models.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryRunStatus {
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryTaskRequest {
    pub target_added_games: u32,
    pub page_size: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryFailureItem {
    pub page_index: u32,
    pub appid: Option<u32>,
    pub stage: String,
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryRunSnapshot {
    pub id: i64,
    pub status: DiscoveryRunStatus,
    pub target_added_games: u32,
    pub page_size: u32,
    pub pages_processed: u32,
    pub scanned_apps: usize,
    pub added_games: usize,
    pub added_new_games: usize,
    pub added_classic_games: usize,
    pub skipped_existing: usize,
    pub skipped_non_multiplayer: usize,
    pub failed_games: usize,
    pub current_appid: Option<u32>,
    pub last_appid: Option<u32>,
    pub have_more_results: bool,
    pub started_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
    pub last_error: Option<String>,
    pub failures: Vec<DiscoveryFailureItem>,
}

impl DiscoveryRunSnapshot {
    pub fn progress_percent(&self) -> u32 {
        if self.target_added_games == 0 {
            return 0;
        }
        let ratio = self.added_games as f64 / self.target_added_games as f64;
        (ratio.min(1.0) * 100.0).round() as u32
    }

    pub fn can_resume(&self) -> bool {
        matches!(
            self.status,
            DiscoveryRunStatus::Paused | DiscoveryRunStatus::Interrupted
        )
    }
}
```

Extend `src-tauri/src/db.rs` with the migration and helpers:

```rust
CREATE TABLE IF NOT EXISTS discovery_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    status TEXT NOT NULL,
    target_added_games INTEGER NOT NULL,
    page_size INTEGER NOT NULL,
    pages_processed INTEGER NOT NULL DEFAULT 0,
    scanned_apps INTEGER NOT NULL DEFAULT 0,
    added_games INTEGER NOT NULL DEFAULT 0,
    added_new_games INTEGER NOT NULL DEFAULT 0,
    added_classic_games INTEGER NOT NULL DEFAULT 0,
    skipped_existing INTEGER NOT NULL DEFAULT 0,
    skipped_non_multiplayer INTEGER NOT NULL DEFAULT 0,
    failed_games INTEGER NOT NULL DEFAULT 0,
    current_appid INTEGER,
    last_appid INTEGER,
    have_more_results INTEGER NOT NULL DEFAULT 1,
    started_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    finished_at TEXT,
    last_error TEXT
);

CREATE TABLE IF NOT EXISTS discovery_failures (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id INTEGER NOT NULL,
    page_index INTEGER NOT NULL,
    appid INTEGER,
    stage TEXT NOT NULL,
    reason TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY(run_id) REFERENCES discovery_runs(id) ON DELETE CASCADE
);
```

```rust
#[derive(Debug, Clone, Default)]
pub struct DiscoveryProgressPatch {
    pub status: Option<DiscoveryRunStatus>,
    pub current_appid: Option<u32>,
    pub last_appid: Option<u32>,
    pub pages_processed: Option<u32>,
    pub scanned_apps: Option<usize>,
    pub added_games: Option<usize>,
    pub added_new_games: Option<usize>,
    pub added_classic_games: Option<usize>,
    pub skipped_existing: Option<usize>,
    pub skipped_non_multiplayer: Option<usize>,
    pub failed_games: Option<usize>,
    pub have_more_results: Option<bool>,
    pub last_error: Option<String>,
    pub finished_at: Option<Option<String>>,
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
            status, target_added_games, page_size, current_appid, last_appid,
            have_more_results, started_at, updated_at
        )
        VALUES (?1, ?2, ?3, NULL, ?4, 1, ?5, ?5)
        "#,
        params![
            "running",
            request.target_added_games,
            request.page_size,
            start_appid,
            now,
        ],
    )?;

    let run_id = conn.last_insert_rowid();
    load_discovery_run(conn, run_id)?.context("discovery run was just created")
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

pub fn mark_running_discovery_runs_interrupted(conn: &Connection) -> Result<()> {
    conn.execute(
        r#"
        UPDATE discovery_runs
        SET status = 'interrupted', updated_at = ?1
        WHERE status = 'running'
        "#,
        params![now_rfc3339()?],
    )?;
    Ok(())
}
```

- [ ] **Step 4: Re-run the Rust tests**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --test discovery_task_tests
```

Expected: PASS for the new persistence and interruption tests.

- [ ] **Step 5: Commit the persistence layer**

```bash
git add src-tauri/src/models.rs src-tauri/src/db.rs src-tauri/tests/discovery_task_tests.rs
git commit -m "feat: persist discovery task runs"
```

## Task 2: Add the Background Worker, Runtime Control, and Tauri Commands

**Files:**

- Create: `src-tauri/src/discovery_task.rs`
- Modify: `src-tauri/src/state.rs`
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/tests/discovery_task_tests.rs`

- [ ] **Step 1: Add failing tests for progress math and resumable status rules**

Append these tests to `src-tauri/tests/discovery_task_tests.rs`:

```rust
use tauri_app_lib::models::{DiscoveryFailureItem, DiscoveryRunSnapshot, DiscoveryRunStatus};

fn sample_snapshot(status: DiscoveryRunStatus, added_games: usize) -> DiscoveryRunSnapshot {
    DiscoveryRunSnapshot {
        id: 1,
        status,
        target_added_games: 10,
        page_size: 25,
        pages_processed: 2,
        scanned_apps: 50,
        added_games,
        added_new_games: 2,
        added_classic_games: added_games.saturating_sub(2),
        skipped_existing: 20,
        skipped_non_multiplayer: 25,
        failed_games: 3,
        current_appid: Some(600_123),
        last_appid: Some(600_150),
        have_more_results: true,
        started_at: "2026-04-26T12:00:00Z".to_string(),
        updated_at: "2026-04-26T12:02:00Z".to_string(),
        finished_at: None,
        last_error: None,
        failures: Vec::<DiscoveryFailureItem>::new(),
    }
}

#[test]
fn discovery_snapshot_progress_percent_uses_target_added_games() {
    assert_eq!(sample_snapshot(DiscoveryRunStatus::Running, 4).progress_percent(), 40);
    assert_eq!(sample_snapshot(DiscoveryRunStatus::Running, 10).progress_percent(), 100);
}

#[test]
fn only_paused_or_interrupted_runs_can_resume() {
    assert!(sample_snapshot(DiscoveryRunStatus::Paused, 3).can_resume());
    assert!(sample_snapshot(DiscoveryRunStatus::Interrupted, 3).can_resume());
    assert!(!sample_snapshot(DiscoveryRunStatus::Running, 3).can_resume());
    assert!(!sample_snapshot(DiscoveryRunStatus::Completed, 10).can_resume());
}
```

- [ ] **Step 2: Run the Rust tests and confirm the new cases fail**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --test discovery_task_tests
```

Expected: FAIL because `DiscoveryRunSnapshot::progress_percent()` and `can_resume()` do not exist yet.

- [ ] **Step 3: Implement the runtime state, worker loop, and commands**

Create `src-tauri/src/discovery_task.rs`:

```rust
use crate::db;
use crate::discovery::{build_discovered_game_card, next_discovery_cursor, DISCOVERY_CURSOR_CONFIG_KEY};
use crate::models::{DiscoveryRunSnapshot, DiscoveryRunStatus, DiscoveryTaskRequest};
use crate::state::AppState;
use crate::steam;
use tauri::{AppHandle, Emitter, Manager};

pub const DISCOVERY_TASK_EVENT: &str = "discovery-task-updated";

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
```

Update `src-tauri/src/state.rs`:

```rust
use crate::discovery_task::DiscoveryRuntimeState;
use reqwest::Client;
use rusqlite::Connection;
use std::sync::Mutex;

pub struct AppState {
    pub db: Mutex<Connection>,
    pub http: Client,
    pub discovery: Mutex<DiscoveryRuntimeState>,
}
```

Add the new commands in `src-tauri/src/commands.rs`:

```rust
#[tauri::command]
pub fn get_discovery_task_snapshot(
    state: State<'_, AppState>,
) -> Result<Option<crate::models::DiscoveryRunSnapshot>, String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    db::load_latest_discovery_run(&conn).map_err(to_command_error)
}

#[tauri::command]
pub fn list_discovery_task_history(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<crate::models::DiscoveryRunSnapshot>, String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    db::list_discovery_runs(&conn, limit.unwrap_or(8)).map_err(to_command_error)
}

#[tauri::command]
pub fn pause_discovery_task(
    state: State<'_, AppState>,
) -> Result<crate::models::DiscoveryRunSnapshot, String> {
    let mut runtime = state.discovery.lock().map_err(|err| err.to_string())?;
    runtime.control = crate::discovery_task::DiscoveryControl::PauseRequested;
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    db::load_latest_discovery_run(&conn)
        .map_err(to_command_error)?
        .ok_or_else(|| "当前没有可暂停的发现任务。".to_string())
}
```

Register interrupted-run recovery and the new commands in `src-tauri/src/lib.rs`:

```rust
pub mod discovery_task;
```

```rust
.setup(|app| {
    let app_data_dir = app.path().app_data_dir()?;
    let db_path = app_data_dir.join("mpgs.sqlite3");
    let db = db::open_database(&db_path)?;
    db::mark_running_discovery_runs_interrupted(&db)?;
    let http = reqwest::Client::builder()
        .user_agent("MPGS/0.1 (+https://local.app)")
        .build()?;

    app.manage(AppState {
        db: std::sync::Mutex::new(db),
        http,
        discovery: std::sync::Mutex::new(discovery_task::DiscoveryRuntimeState::default()),
    });
    Ok(())
})
.invoke_handler(tauri::generate_handler![
    commands::get_dashboard,
    commands::save_config,
    commands::sync_seed_games,
    commands::discover_steam_games,
    commands::assess_game_with_ai,
    commands::preview_steam_app_list,
    commands::set_game_user_state,
    commands::get_user_collections,
    commands::get_discovery_task_snapshot,
    commands::list_discovery_task_history,
    commands::start_discovery_task,
    commands::pause_discovery_task,
    commands::resume_discovery_task,
    commands::cancel_discovery_task,
])
```

Worker behavior to implement inside `start_discovery_task()` / `resume_discovery_task()`:

```rust
tauri::async_runtime::spawn(async move {
    let app_state = app_handle.state::<AppState>();
    let mut snapshot = {
        let conn = app_state.db.lock().expect("lock db");
        db::load_discovery_run(&conn, run_id)
            .expect("load run")
            .expect("run exists")
    };

    loop {
        let control = {
            let runtime = app_state.discovery.lock().expect("lock runtime");
            runtime.control
        };

        if control == crate::discovery_task::DiscoveryControl::CancelRequested {
            snapshot.status = DiscoveryRunStatus::Cancelled;
            break;
        }

        if snapshot.added_games >= snapshot.target_added_games as usize {
            snapshot.status = DiscoveryRunStatus::Completed;
            break;
        }

        if control == crate::discovery_task::DiscoveryControl::PauseRequested {
            snapshot.status = DiscoveryRunStatus::Paused;
            break;
        }

        let preview = match steam::fetch_app_list_preview(
            &app_state.http,
            &steam_api_key,
            snapshot.page_size,
            snapshot.last_appid,
        )
        .await
        {
            Ok(preview) => preview,
            Err(error) => {
                snapshot.status = DiscoveryRunStatus::Failed;
                snapshot.last_error = Some(error.to_string());
                break;
            }
        };

        snapshot.pages_processed += 1;
        snapshot.scanned_apps += preview.apps.len();
        snapshot.have_more_results = preview.have_more_results.unwrap_or(false);

        for app in &preview.apps {
            snapshot.current_appid = Some(app.appid);
            crate::discovery_task::emit_snapshot(&app_handle, &snapshot);

            match steam::fetch_game_snapshot(&app_state.http, app.appid, &country, &language).await {
                Ok(snapshot_data) => {
                    if let Some(card) =
                        build_discovered_game_card(app, snapshot_data, &today_iso)
                    {
                        match card.section.as_str() {
                            "new" => snapshot.added_new_games += 1,
                            _ => snapshot.added_classic_games += 1,
                        }

                        let conn = app_state.db.lock().expect("lock db");
                        db::upsert_game(&conn, &card).expect("upsert discovered game");
                        snapshot.added_games += 1;
                    } else {
                        snapshot.skipped_non_multiplayer += 1;
                    }
                }
                Err(error) => {
                    snapshot.failed_games += 1;
                    snapshot.last_error = Some(error.to_string());

                    let conn = app_state.db.lock().expect("lock db");
                    db::append_discovery_failure(
                        &conn,
                        snapshot.id,
                        snapshot.pages_processed,
                        Some(app.appid),
                        "fetch_snapshot",
                        &error.to_string(),
                    )
                    .expect("record discovery failure");
                }
            }

            let conn = app_state.db.lock().expect("lock db");
            db::update_discovery_run_progress(
                &conn,
                snapshot.id,
                db::DiscoveryProgressPatch {
                    status: Some(DiscoveryRunStatus::Running),
                    current_appid: snapshot.current_appid,
                    last_appid: snapshot.last_appid,
                    pages_processed: Some(snapshot.pages_processed),
                    scanned_apps: Some(snapshot.scanned_apps),
                    added_games: Some(snapshot.added_games),
                    added_new_games: Some(snapshot.added_new_games),
                    added_classic_games: Some(snapshot.added_classic_games),
                    skipped_existing: Some(snapshot.skipped_existing),
                    skipped_non_multiplayer: Some(snapshot.skipped_non_multiplayer),
                    failed_games: Some(snapshot.failed_games),
                    have_more_results: Some(snapshot.have_more_results),
                    last_error: snapshot.last_error.clone(),
                    finished_at: None,
                },
            )
            .expect("persist live discovery snapshot");
        }

        snapshot.last_appid = next_discovery_cursor(&preview);

        if let Some(last_appid) = snapshot.last_appid {
            let conn = app_state.db.lock().expect("lock db");
            db::set_config(&conn, DISCOVERY_CURSOR_CONFIG_KEY, &last_appid.to_string())
                .expect("save discovery cursor");
        }

        if !snapshot.have_more_results || snapshot.last_appid.is_none() {
            snapshot.status = DiscoveryRunStatus::Completed;
            break;
        }
    }

    crate::discovery_task::emit_snapshot(&app_handle, &snapshot);
});
```

- [ ] **Step 4: Run the focused Rust tests and then the full Rust suite**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --test discovery_task_tests
```

Expected: PASS for the discovery task tests.

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: PASS for all Rust tests, including the existing discovery and user-state coverage.

- [ ] **Step 5: Commit the backend runtime work**

```bash
git add src-tauri/src/discovery_task.rs src-tauri/src/state.rs src-tauri/src/commands.rs src-tauri/src/lib.rs src-tauri/tests/discovery_task_tests.rs
git commit -m "feat: add background discovery task runtime"
```

## Task 3: Add Frontend Types, Client Commands, and Live Event Hook

**Files:**

- Modify: `src/types.ts`
- Modify: `src/api/client.ts`
- Create: `src/features/discovery/useDiscoveryTask.ts`
- Test: `src/features/discovery/useDiscoveryTask.test.tsx`

- [ ] **Step 1: Write the failing hook test**

Create `src/features/discovery/useDiscoveryTask.test.tsx`:

```tsx
// @vitest-environment jsdom
import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useDiscoveryTask } from "./useDiscoveryTask";

const listenMock = vi.fn();
const getSnapshotMock = vi.fn();
const listHistoryMock = vi.fn();

vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => listenMock(...args),
}));

vi.mock("../../api/client", () => ({
  getDiscoveryTaskSnapshot: () => getSnapshotMock(),
  listDiscoveryTaskHistory: () => listHistoryMock(),
  startDiscoveryTask: vi.fn(),
  pauseDiscoveryTask: vi.fn(),
  resumeDiscoveryTask: vi.fn(),
  cancelDiscoveryTask: vi.fn(),
  isTauriRuntime: () => true,
}));

describe("useDiscoveryTask", () => {
  beforeEach(() => {
    listenMock.mockReset();
    getSnapshotMock.mockReset();
    listHistoryMock.mockReset();
  });

  it("hydrates from commands and replaces snapshot when a live event arrives", async () => {
    getSnapshotMock.mockResolvedValue({
      id: 1,
      status: "running",
      targetAddedGames: 8,
      pageSize: 25,
      pagesProcessed: 1,
      scannedApps: 25,
      addedGames: 2,
      addedNewGames: 1,
      addedClassicGames: 1,
      skippedExisting: 10,
      skippedNonMultiplayer: 12,
      failedGames: 1,
      currentAppid: 500001,
      lastAppid: 500025,
      haveMoreResults: true,
      startedAt: "2026-04-26T12:00:00Z",
      updatedAt: "2026-04-26T12:01:00Z",
      finishedAt: null,
      lastError: null,
      failures: [],
      progressPercent: 25,
    });

    listHistoryMock.mockResolvedValue([]);

    let handler: ((event: { payload: unknown }) => void) | undefined;
    listenMock.mockImplementation(async (_name: string, callback: typeof handler) => {
      handler = callback;
      return () => {};
    });

    const { result } = renderHook(() => useDiscoveryTask());

    await waitFor(() => {
      expect(result.current.snapshot?.addedGames).toBe(2);
    });

    await act(async () => {
      handler?.({
        payload: {
          id: 1,
          status: "paused",
          targetAddedGames: 8,
          pageSize: 25,
          pagesProcessed: 2,
          scannedApps: 50,
          addedGames: 4,
          addedNewGames: 1,
          addedClassicGames: 3,
          skippedExisting: 21,
          skippedNonMultiplayer: 24,
          failedGames: 1,
          currentAppid: 500050,
          lastAppid: 500050,
          haveMoreResults: true,
          startedAt: "2026-04-26T12:00:00Z",
          updatedAt: "2026-04-26T12:03:00Z",
          finishedAt: null,
          lastError: null,
          failures: [],
          progressPercent: 50,
        },
      });
    });

    expect(result.current.snapshot?.status).toBe("paused");
    expect(result.current.snapshot?.progressPercent).toBe(50);
  });
});
```

- [ ] **Step 2: Run the frontend hook test and confirm it fails**

Run:

```bash
npm run test -- src/features/discovery/useDiscoveryTask.test.tsx
```

Expected: FAIL because `useDiscoveryTask`, `getDiscoveryTaskSnapshot`, `listDiscoveryTaskHistory`, and the discovery task types do not exist yet.

- [ ] **Step 3: Implement the shared TS types, client functions, and event-driven hook**

Add the TS types in `src/types.ts`:

```ts
export type DiscoveryRunStatus =
  | "running"
  | "paused"
  | "completed"
  | "failed"
  | "cancelled"
  | "interrupted";

export interface DiscoveryTaskRequest {
  targetAddedGames: number;
  pageSize: number;
}

export interface DiscoveryFailureItem {
  pageIndex: number;
  appid?: number | null;
  stage: string;
  reason: string;
  createdAt: string;
}

export interface DiscoveryRunSnapshot {
  id: number;
  status: DiscoveryRunStatus;
  targetAddedGames: number;
  pageSize: number;
  pagesProcessed: number;
  scannedApps: number;
  addedGames: number;
  addedNewGames: number;
  addedClassicGames: number;
  skippedExisting: number;
  skippedNonMultiplayer: number;
  failedGames: number;
  currentAppid?: number | null;
  lastAppid?: number | null;
  haveMoreResults: boolean;
  startedAt: string;
  updatedAt: string;
  finishedAt?: string | null;
  lastError?: string | null;
  failures: DiscoveryFailureItem[];
  progressPercent: number;
}
```

Extend `src/api/client.ts`:

```ts
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import type {
  DiscoveryRunSnapshot,
  DiscoveryTaskRequest,
} from "../types";

export const isTauriRuntime = () => "__TAURI_INTERNALS__" in window;

export async function getDiscoveryTaskSnapshot(): Promise<DiscoveryRunSnapshot | null> {
  if (!isTauriRuntime()) return null;
  return invoke<DiscoveryRunSnapshot | null>("get_discovery_task_snapshot");
}

export async function listDiscoveryTaskHistory(limit = 8): Promise<DiscoveryRunSnapshot[]> {
  if (!isTauriRuntime()) return [];
  return invoke<DiscoveryRunSnapshot[]>("list_discovery_task_history", { limit });
}

export async function startDiscoveryTask(
  request: DiscoveryTaskRequest,
): Promise<DiscoveryRunSnapshot> {
  if (!isTauriRuntime()) {
    return {
      id: 1,
      status: "completed",
      targetAddedGames: request.targetAddedGames,
      pageSize: request.pageSize,
      pagesProcessed: 1,
      scannedApps: request.pageSize,
      addedGames: Math.min(request.targetAddedGames, 3),
      addedNewGames: 1,
      addedClassicGames: Math.max(Math.min(request.targetAddedGames, 3) - 1, 0),
      skippedExisting: request.pageSize - Math.min(request.targetAddedGames, 3),
      skippedNonMultiplayer: 0,
      failedGames: 0,
      currentAppid: null,
      lastAppid: mockDashboard.stats.lastDiscoveryAppid ?? null,
      haveMoreResults: false,
      startedAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
      finishedAt: new Date().toISOString(),
      lastError: null,
      failures: [],
      progressPercent: 100,
    };
  }
  return invoke<DiscoveryRunSnapshot>("start_discovery_task", { request });
}
```

Create `src/features/discovery/useDiscoveryTask.ts`:

```ts
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import {
  cancelDiscoveryTask,
  getDiscoveryTaskSnapshot,
  isTauriRuntime,
  listDiscoveryTaskHistory,
  pauseDiscoveryTask,
  resumeDiscoveryTask,
  startDiscoveryTask,
} from "../../api/client";
import type { DiscoveryRunSnapshot, DiscoveryTaskRequest } from "../../types";

const DISCOVERY_TASK_EVENT = "discovery-task-updated";

export function useDiscoveryTask() {
  const [snapshot, setSnapshot] = useState<DiscoveryRunSnapshot | null>(null);
  const [history, setHistory] = useState<DiscoveryRunSnapshot[]>([]);
  const [isLoading, setIsLoading] = useState(true);

  async function refresh() {
    const [nextSnapshot, nextHistory] = await Promise.all([
      getDiscoveryTaskSnapshot(),
      listDiscoveryTaskHistory(8),
    ]);
    setSnapshot(nextSnapshot);
    setHistory(nextHistory);
  }

  useEffect(() => {
    let dispose: undefined | (() => void);

    void (async () => {
      try {
        await refresh();
        if (isTauriRuntime()) {
          dispose = await listen<DiscoveryRunSnapshot>(DISCOVERY_TASK_EVENT, (event) => {
            setSnapshot(event.payload);
          });
        }
      } finally {
        setIsLoading(false);
      }
    })();

    return () => {
      dispose?.();
    };
  }, []);

  return {
    snapshot,
    history,
    isLoading,
    refresh,
    start: (request: DiscoveryTaskRequest) => startDiscoveryTask(request),
    pause: () => pauseDiscoveryTask(),
    resume: () => resumeDiscoveryTask(),
    cancel: () => cancelDiscoveryTask(),
  };
}
```

- [ ] **Step 4: Run the new frontend hook test**

Run:

```bash
npm run test -- src/features/discovery/useDiscoveryTask.test.tsx
```

Expected: PASS for the hook hydration and live-event update behavior.

- [ ] **Step 5: Commit the client and hook layer**

```bash
git add src/types.ts src/api/client.ts src/features/discovery/useDiscoveryTask.ts src/features/discovery/useDiscoveryTask.test.tsx
git commit -m "feat: add frontend discovery task client"
```

## Task 4: Replace the One-Shot Settings Scan UI with a Discovery Console

**Files:**

- Create: `src/features/discovery/DiscoveryTaskPanel.tsx`
- Create: `src/features/discovery/DiscoveryTaskPanel.test.tsx`
- Modify: `src/App.tsx`
- Modify: `src/App.css`

- [ ] **Step 1: Write the failing component test**

Create `src/features/discovery/DiscoveryTaskPanel.test.tsx`:

```tsx
// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { DiscoveryTaskPanel } from "./DiscoveryTaskPanel";

const useDiscoveryTaskMock = vi.fn();

vi.mock("./useDiscoveryTask", () => ({
  useDiscoveryTask: () => useDiscoveryTaskMock(),
}));

describe("DiscoveryTaskPanel", () => {
  it("shows resume controls for paused runs and renders failure rows", () => {
    useDiscoveryTaskMock.mockReturnValue({
      snapshot: {
        id: 9,
        status: "paused",
        targetAddedGames: 8,
        pageSize: 25,
        pagesProcessed: 2,
        scannedApps: 50,
        addedGames: 4,
        addedNewGames: 1,
        addedClassicGames: 3,
        skippedExisting: 20,
        skippedNonMultiplayer: 24,
        failedGames: 2,
        currentAppid: 700050,
        lastAppid: 700050,
        haveMoreResults: true,
        startedAt: "2026-04-26T12:00:00Z",
        updatedAt: "2026-04-26T12:03:00Z",
        finishedAt: null,
        lastError: null,
        failures: [
          {
            pageIndex: 2,
            appid: 700044,
            stage: "fetch_snapshot",
            reason: "steam timeout",
            createdAt: "2026-04-26T12:02:30Z",
          },
        ],
        progressPercent: 50,
      },
      history: [],
      isLoading: false,
      refresh: vi.fn(),
      start: vi.fn(),
      pause: vi.fn(),
      resume: vi.fn(),
      cancel: vi.fn(),
    });

    render(
      <DiscoveryTaskPanel
        stats={{
          lastSyncAt: null,
          seedCount: 47,
          totalGames: 47,
          newGamesCount: 4,
          classicGamesCount: 43,
          lastDiscoveryAppid: 700050,
          dataSource: "test",
        }}
        onStatus={vi.fn()}
        onRefreshDashboard={vi.fn()}
      />,
    );

    expect(screen.getByText("已暂停")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "继续扫描" })).toBeEnabled();
    expect(screen.getByText(/steam timeout/i)).toBeInTheDocument();
  });

  it("starts a new run with the entered target and page size", async () => {
    const start = vi.fn().mockResolvedValue(undefined);

    useDiscoveryTaskMock.mockReturnValue({
      snapshot: null,
      history: [],
      isLoading: false,
      refresh: vi.fn(),
      start,
      pause: vi.fn(),
      resume: vi.fn(),
      cancel: vi.fn(),
    });

    render(
      <DiscoveryTaskPanel
        stats={{
          lastSyncAt: null,
          seedCount: 47,
          totalGames: 47,
          newGamesCount: 4,
          classicGamesCount: 43,
          lastDiscoveryAppid: 700050,
          dataSource: "test",
        }}
        onStatus={vi.fn()}
        onRefreshDashboard={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByLabelText("目标新增"), { target: { value: "6" } });
    fireEvent.change(screen.getByLabelText("每页数量"), { target: { value: "30" } });
    fireEvent.click(screen.getByRole("button", { name: "开始扫描" }));

    expect(start).toHaveBeenCalledWith({ targetAddedGames: 6, pageSize: 30 });
  });
});
```

- [ ] **Step 2: Run the component test and confirm it fails**

Run:

```bash
npm run test -- src/features/discovery/DiscoveryTaskPanel.test.tsx
```

Expected: FAIL because `DiscoveryTaskPanel` does not exist yet and `App.tsx` still uses the one-shot scan form.

- [ ] **Step 3: Implement the new panel and wire it into `App.tsx`**

Create `src/features/discovery/DiscoveryTaskPanel.tsx`:

```tsx
import { useState } from "react";
import type { DashboardStats } from "../../types";
import { useDiscoveryTask } from "./useDiscoveryTask";

const statusLabel: Record<string, string> = {
  running: "扫描中",
  paused: "已暂停",
  completed: "已完成",
  failed: "失败",
  cancelled: "已取消",
  interrupted: "已中断",
};

export function DiscoveryTaskPanel({
  stats,
  onStatus,
  onRefreshDashboard,
}: {
  stats: DashboardStats;
  onStatus: (message: string) => void;
  onRefreshDashboard: () => Promise<void>;
}) {
  const [targetAddedGames, setTargetAddedGames] = useState(6);
  const [pageSize, setPageSize] = useState(25);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const { snapshot, history, isLoading, start, pause, resume, cancel } = useDiscoveryTask();

  const isRunning = snapshot?.status === "running";
  const canResume =
    snapshot?.status === "paused" || snapshot?.status === "interrupted";

  async function handleStart() {
    setPreviewError(null);
    await start({ targetAddedGames, pageSize });
    onStatus("发现任务已启动，进度将实时更新。");
  }

  async function handleResume() {
    setPreviewError(null);
    await resume();
    onStatus("发现任务已继续。");
  }

  async function handlePause() {
    setPreviewError(null);
    await pause();
    onStatus("发现任务会在当前条目处理完成后暂停。");
  }

  async function handleCancel() {
    setPreviewError(null);
    await cancel();
    await onRefreshDashboard();
    onStatus("发现任务已取消。");
  }

  return (
    <section className="steam-preview discovery-task-panel">
      <div className="discovery-task-header">
        <strong>发现任务控制台</strong>
        <span>{snapshot ? statusLabel[snapshot.status] : "尚未开始"}</span>
      </div>

      <div className="settings-grid">
        <label>
          目标新增
          <input
            aria-label="目标新增"
            min={1}
            max={50}
            type="number"
            value={targetAddedGames}
            disabled={isRunning}
            onChange={(event) =>
              setTargetAddedGames(
                Math.min(50, Math.max(1, Number(event.currentTarget.value) || 1)),
              )
            }
          />
        </label>
        <label>
          每页数量
          <input
            aria-label="每页数量"
            min={10}
            max={50}
            type="number"
            value={pageSize}
            disabled={isRunning}
            onChange={(event) =>
              setPageSize(
                Math.min(50, Math.max(10, Number(event.currentTarget.value) || 10)),
              )
            }
          />
        </label>
      </div>

      <p className="settings-hint">
        当前库：{stats.totalGames} 个游戏；上次游标：{stats.lastDiscoveryAppid ?? "无"}；本次任务将在达到目标新增后自动停止。
      </p>

      <div className="settings-actions">
        <button className="gold-button" type="button" onClick={handleStart} disabled={isRunning}>
          开始扫描
        </button>
        <button className="muted-button" type="button" onClick={handlePause} disabled={!isRunning}>
          暂停扫描
        </button>
        <button className="muted-button" type="button" onClick={handleResume} disabled={!canResume}>
          继续扫描
        </button>
        <button className="muted-button" type="button" onClick={handleCancel} disabled={!snapshot}>
          取消任务
        </button>
      </div>

      {previewError && <p className="settings-error">{previewError}</p>}
      {isLoading && <p className="settings-hint">正在读取发现任务状态…</p>}

      {snapshot && (
        <>
          <div className="discovery-progress">
            <div style={{ width: `${snapshot.progressPercent}%` }} />
          </div>
          <div className="discovery-stats-grid">
            <em>进度 {snapshot.progressPercent}%</em>
            <em>页面 {snapshot.pagesProcessed}</em>
            <em>当前 appid {snapshot.currentAppid ?? "无"}</em>
            <em>新增 {snapshot.addedGames}</em>
            <em>已存在 {snapshot.skippedExisting}</em>
            <em>非多人 {snapshot.skippedNonMultiplayer}</em>
            <em>失败 {snapshot.failedGames}</em>
          </div>

          {snapshot.failures.length > 0 && (
            <div className="discovery-failures">
              <strong>失败项</strong>
              <div>
                {snapshot.failures.slice(0, 10).map((failure) => (
                  <em key={`${failure.pageIndex}-${failure.appid ?? "none"}-${failure.createdAt}`}>
                    第 {failure.pageIndex} 页 · {failure.appid ?? "未知 appid"} · {failure.stage} · {failure.reason}
                  </em>
                ))}
              </div>
            </div>
          )}
        </>
      )}

      {history.length > 0 && (
        <div className="discovery-history">
          <strong>扫描历史</strong>
          <div>
            {history.map((run) => (
              <em key={run.id}>
                #{run.id} · {statusLabel[run.status]} · 新增 {run.addedGames} · 页数 {run.pagesProcessed} · 游标 {run.lastAppid ?? "无"}
              </em>
            ))}
          </div>
        </div>
      )}
    </section>
  );
}
```

Replace the discovery section inside `src/App.tsx`:

```tsx
import { DiscoveryTaskPanel } from "./features/discovery/DiscoveryTaskPanel";
```

```tsx
<DiscoveryTaskPanel
  stats={stats}
  onStatus={setStatus}
  onRefreshDashboard={loadDashboard}
/>
```

Add the styles in `src/App.css`:

```css
.discovery-task-panel {
  gap: 16px;
}

.discovery-task-header,
.discovery-stats-grid,
.discovery-failures div,
.discovery-history div {
  display: flex;
  flex-wrap: wrap;
  gap: 10px 12px;
}

.discovery-progress {
  width: 100%;
  height: 10px;
  border-radius: 999px;
  background: rgba(104, 67, 28, 0.12);
  overflow: hidden;
}

.discovery-progress > div {
  height: 100%;
  background: linear-gradient(90deg, #ffb14a 0%, #ff7f50 100%);
  transition: width 180ms ease;
}

.discovery-failures,
.discovery-history {
  display: grid;
  gap: 10px;
}
```

- [ ] **Step 4: Run the UI tests and the production build**

Run:

```bash
npm run test -- src/features/discovery/DiscoveryTaskPanel.test.tsx
```

Expected: PASS for paused rendering and start-action behavior.

Run:

```bash
npm run build
```

Expected: PASS with the new discovery console compiled into the app bundle.

- [ ] **Step 5: Commit the UI integration**

```bash
git add src/features/discovery/DiscoveryTaskPanel.tsx src/features/discovery/DiscoveryTaskPanel.test.tsx src/App.tsx src/App.css
git commit -m "feat: add discovery task console"
```

## Verification Checklist

- [ ] Run all frontend tests:

```bash
npm run test
```

Expected: PASS for `src/domain/recommendation.test.ts`, `src/features/discovery/useDiscoveryTask.test.tsx`, and `src/features/discovery/DiscoveryTaskPanel.test.tsx`.

- [ ] Run all Rust tests:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: PASS for `discovery_tests`, `discovery_task_tests`, `recommendation_tests`, and `user_state_tests`.

- [ ] Run the desktop app and verify the live discovery flow:

```bash
npm run tauri dev
```

Manual check:

1. Open `设置`.
2. Set `目标新增` to `5` and `每页数量` to `25`.
3. Click `开始扫描` and confirm the progress bar, page count, current `appid`, and counters update without freezing the UI.
4. Click `暂停扫描` while a run is active and confirm the state changes to `已暂停` after the current item finishes.
5. Click `继续扫描` and confirm the task resumes from the saved cursor instead of restarting from page zero.
6. Confirm `失败项` rows appear when a fetch error occurs.
7. Confirm `扫描历史` shows the finished run.
8. Close and reopen the app during a running task, then confirm the most recent run becomes `已中断` and can be resumed.

## Self-Review

**1. Spec coverage**

- Scan progress bar: covered by Task 4.
- Current page / current appid / imported / skipped / failed counters: covered by Task 2 snapshot fields plus Task 4 rendering.
- Failure item records: covered by Task 1 DB persistence plus Task 4 failure list.
- Pause / resume / cancel: covered by Task 2 commands and Task 4 controls.
- Scan history: covered by Task 1 persistence plus Task 4 history rendering.
- Auto-continue until target imported count: covered by Task 2 worker loop stop condition.
- Interruption / recovery after restart: covered by Task 1 stale-run DB transition plus Task 2 startup hook.

**2. Placeholder scan**

- No `TODO`, `TBD`, or “similar to previous task” shortcuts remain.
- Each code-edit step includes concrete file paths, code snippets, commands, and expected outcomes.

**3. Type consistency**

- Rust and TypeScript both use `DiscoveryTaskRequest`, `DiscoveryRunSnapshot`, `DiscoveryRunStatus`, and `DiscoveryFailureItem`.
- Frontend command names match Tauri command names: `get_discovery_task_snapshot`, `list_discovery_task_history`, `start_discovery_task`, `pause_discovery_task`, `resume_discovery_task`, `cancel_discovery_task`.
- Progress is always defined against `target_added_games` / `targetAddedGames`, not pages scanned.
