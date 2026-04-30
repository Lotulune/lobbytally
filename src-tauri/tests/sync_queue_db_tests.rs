use rusqlite::Connection;
use tauri_app_lib::db;
use tauri_app_lib::models::SyncMode;

fn seeded_memory_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    db::migrate(&conn).expect("migrate");
    db::seed_default_games(&conn).expect("seed");
    conn
}

#[test]
fn sync_queue_round_trips_jobs_and_summary() {
    let conn = seeded_memory_db();

    assert_eq!(
        db::replace_sync_jobs(&conn, [730, 570, 730], SyncMode::Full).expect("replace queue"),
        2
    );

    let jobs = db::list_sync_jobs(&conn).expect("list sync jobs");
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].appid, 730);
    assert_eq!(jobs[0].attempt, 1);
    assert_eq!(jobs[0].mode, SyncMode::Full);
    assert_eq!(jobs[1].appid, 570);

    let summary = db::sync_queue_summary(&conn)
        .expect("load sync queue summary")
        .expect("summary should exist");
    assert_eq!(summary.pending_count, 2);
    assert_eq!(summary.mode, SyncMode::Full);
    assert_eq!(summary.last_error_appid, None);
    assert_eq!(summary.last_error, None);
}

#[test]
fn sync_queue_updates_error_and_deletes_completed_jobs() {
    let conn = seeded_memory_db();
    db::replace_sync_jobs(&conn, [730, 570], SyncMode::Quick).expect("seed sync queue");

    db::update_sync_job(
        &conn,
        570,
        SyncMode::Quick,
        2,
        Some("Steam 请求过于频繁（429），该游戏已保留到待续同步队列。"),
    )
    .expect("update sync job");

    let summary = db::sync_queue_summary(&conn)
        .expect("load summary after update")
        .expect("summary should still exist");
    assert_eq!(summary.pending_count, 2);
    assert_eq!(summary.mode, SyncMode::Quick);
    assert_eq!(summary.last_error_appid, Some(570));
    assert_eq!(
        summary.last_error.as_deref(),
        Some("Steam 请求过于频繁（429），该游戏已保留到待续同步队列。")
    );

    db::delete_sync_job(&conn, 730).expect("delete completed job");
    let jobs = db::list_sync_jobs(&conn).expect("list queue after delete");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].appid, 570);
}

#[test]
fn sync_queue_mode_can_be_upgraded_for_resume() {
    let conn = seeded_memory_db();
    db::replace_sync_jobs(&conn, [730, 570], SyncMode::Quick).expect("seed quick queue");

    db::update_all_sync_job_modes(&conn, SyncMode::Full).expect("upgrade queue mode");

    let jobs = db::list_sync_jobs(&conn).expect("list upgraded queue");
    assert!(jobs.iter().all(|job| job.mode == SyncMode::Full));
    let summary = db::sync_queue_summary(&conn)
        .expect("load upgraded summary")
        .expect("summary should exist");
    assert_eq!(summary.mode, SyncMode::Full);
}
