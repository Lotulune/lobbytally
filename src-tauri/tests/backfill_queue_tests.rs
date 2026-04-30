use tauri_app_lib::backfill_task::{BackfillJob, BackfillRuntimeState, BACKFILL_MAX_ATTEMPTS};

#[test]
fn backfill_queue_deduplicates_pending_and_active_appids() {
    let mut state = BackfillRuntimeState::default();

    assert_eq!(state.enqueue([730, 570, 730]), 2);
    assert_eq!(state.pending_len(), 2);

    let first = state.take_next_job().expect("first job");
    assert_eq!(first.appid, 730);

    assert_eq!(state.enqueue([730, 440, 570]), 1);
    assert_eq!(state.pending_len(), 2);
}

#[test]
fn backfill_queue_requeues_failed_jobs_until_retry_budget_is_exhausted() {
    let mut state = BackfillRuntimeState::default();
    state.enqueue([730]);

    let first = state.take_next_job().expect("first attempt");
    assert_eq!(first.attempt, 1);
    assert!(state.finish_job(first, false));
    assert_eq!(state.pending_len(), 1);

    let second = state.take_next_job().expect("second attempt");
    assert_eq!(second.attempt, 2);
    assert!(!state.finish_job(second, false));
    assert_eq!(second.attempt, BACKFILL_MAX_ATTEMPTS);
    assert_eq!(state.pending_len(), 0);
}

#[test]
fn backfill_queue_removes_completed_jobs() {
    let mut state = BackfillRuntimeState::default();
    state.enqueue([730, 570]);

    let first = state.take_next_job().expect("first job");
    assert!(state.finish_job(first, true));
    assert_eq!(state.pending_len(), 1);

    let second = state.take_next_job().expect("second job");
    assert_eq!(
        second,
        BackfillJob {
            appid: 570,
            attempt: 1
        }
    );
    assert!(state.finish_job(second, true));
    assert_eq!(state.pending_len(), 0);
}

#[test]
fn backfill_queue_restores_attempt_counts_in_fifo_order() {
    let mut state = BackfillRuntimeState::default();
    state.restore([
        BackfillJob {
            appid: 440,
            attempt: 2,
        },
        BackfillJob {
            appid: 570,
            attempt: 1,
        },
    ]);

    assert_eq!(
        state.take_next_job(),
        Some(BackfillJob {
            appid: 440,
            attempt: 2,
        })
    );
    assert_eq!(
        state.take_next_job(),
        Some(BackfillJob {
            appid: 570,
            attempt: 1,
        })
    );
}

#[test]
fn backfill_queue_tracks_progress_snapshot_for_ui() {
    let mut state = BackfillRuntimeState::default();
    state.active = true;

    assert_eq!(state.enqueue([730, 570, 440]), 3);

    let initial = state.snapshot();
    assert!(initial.running);
    assert_eq!(initial.pending_count, 3);
    assert_eq!(initial.total_count, 3);
    assert_eq!(initial.processed_count, 0);
    assert_eq!(initial.failed_count, 0);
    assert_eq!(initial.current_appid, None);

    let first = state.take_next_job().expect("first job");
    let in_progress = state.snapshot();
    assert_eq!(in_progress.current_appid, Some(730));
    assert_eq!(in_progress.current_attempt, Some(1));
    assert_eq!(in_progress.pending_count, 3);

    assert!(state.finish_job(first, true));
    let after_success = state.snapshot();
    assert_eq!(after_success.pending_count, 2);
    assert_eq!(after_success.processed_count, 1);
    assert_eq!(after_success.failed_count, 0);

    let second = state.take_next_job().expect("second job");
    assert!(state.finish_job(second, false));
    let after_retry = state.snapshot();
    assert_eq!(after_retry.pending_count, 2);
    assert_eq!(after_retry.processed_count, 1);
    assert_eq!(after_retry.failed_count, 0);
}
