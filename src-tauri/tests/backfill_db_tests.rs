use rusqlite::Connection;
use tauri_app_lib::db;

fn seeded_memory_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    db::migrate(&conn).expect("migrate");
    conn
}

#[test]
fn metadata_backfill_queue_round_trips_and_deduplicates() {
    let conn = seeded_memory_db();

    assert_eq!(
        db::enqueue_metadata_backfill(&conn, [730, 570, 730]).expect("enqueue"),
        2
    );
    assert_eq!(
        db::enqueue_metadata_backfill(&conn, [570, 440]).expect("enqueue"),
        1
    );

    let jobs = db::list_metadata_backfill_jobs(&conn).expect("list jobs");

    assert_eq!(jobs.len(), 3);
    assert_eq!(jobs[0].appid, 730);
    assert_eq!(jobs[0].attempt, 1);
    assert_eq!(jobs[1].appid, 570);
    assert_eq!(jobs[2].appid, 440);
}

#[test]
fn metadata_backfill_queue_updates_attempt_and_deletes_completed_jobs() {
    let conn = seeded_memory_db();
    db::enqueue_metadata_backfill(&conn, [730]).expect("enqueue");

    db::update_metadata_backfill_attempt(&conn, 730, 2, Some("temporary upstream error"))
        .expect("update attempt");
    let jobs = db::list_metadata_backfill_jobs(&conn).expect("list jobs after update");

    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].appid, 730);
    assert_eq!(jobs[0].attempt, 2);

    db::delete_metadata_backfill_job(&conn, 730).expect("delete job");
    assert!(db::list_metadata_backfill_jobs(&conn)
        .expect("list jobs after delete")
        .is_empty());
}

#[test]
fn dashboard_stats_include_backfill_queue_summary() {
    let conn = seeded_memory_db();
    db::enqueue_metadata_backfill(&conn, [730, 570]).expect("enqueue");
    db::update_metadata_backfill_attempt(&conn, 570, 2, Some("temporary upstream error"))
        .expect("update attempt");

    let dashboard = db::load_dashboard(&conn).expect("load dashboard");

    assert_eq!(dashboard.stats.backfill_pending_count, 2);
    assert_eq!(dashboard.stats.backfill_last_error_appid, Some(570));
    assert_eq!(
        dashboard.stats.backfill_last_error.as_deref(),
        Some("temporary upstream error")
    );
}
