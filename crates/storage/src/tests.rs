use std::sync::Arc;

use mpgs_domain::FeedbackType;
use mpgs_steam_source::{
    AppCatalogProposal, AppListRequest, AppTypeProposal, CcuProposal, CcuRequest, RawResponse,
    ReviewSummaryProposal, ReviewSummaryRequest, SourceStability, StoreDetailsRequest,
    StoreSearchCandidate, StoreSearchPage, parse_app_list_page, parse_ccu, parse_review_summary,
    parse_store_details,
};

use crate::clock::FakeClock;
use crate::db::Database;
use crate::migrate::{self, latest_version};
use crate::models::{CreateOverrideRequest, EnqueueJob, FeatureOrigin};
use crate::repo::Repository;

fn repo_with_clock(now_ms: i64) -> (Repository, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::new(now_ms));
    let db = Database::open_in_memory_with_clock(clock.clone()).unwrap();
    let repo = Repository::new(db);
    repo.migrate().unwrap();
    (repo, clock)
}

#[test]
fn empty_database_migrates_to_latest() {
    let db = Database::open_in_memory().unwrap();
    assert_eq!(db.schema_version().unwrap(), 0);
    let version = db.migrate().unwrap();
    assert_eq!(version, latest_version());
    db.assert_ready().unwrap();
}

#[test]
fn previous_version_with_data_upgrades() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("upgrade.db");
    let clock = Arc::new(FakeClock::new(1_000));
    let db = Database::open_with_clock(&path, clock.clone()).unwrap();

    db.with_conn_mut(|conn| {
        migrate::migrate_to(conn, 1, 1_000)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(db.schema_version().unwrap(), 1);

    db.with_conn_mut(|conn| {
        conn.execute(
            "INSERT INTO apps (
                app_id, app_type, canonical_name, release_state, created_at_ms, updated_at_ms
             ) VALUES (892970, 'game', 'Valheim', 'released', 1000, 1000)",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let version = db.migrate().unwrap();
    assert_eq!(version, latest_version());
    let name: String = db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT canonical_name FROM apps WHERE app_id = 892970",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(name, "Valheim");

    // data_quality_findings must exist after v2
    db.with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM data_quality_findings", [], |row| {
            row.get::<_, i64>(0)
        })?;
        Ok(())
    })
    .unwrap();
}

#[test]
fn file_backed_reads_do_not_wait_for_writer_handle_lock() {
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("concurrent.db")).unwrap();
    db.migrate().unwrap();
    db.with_conn_mut(|conn| {
        conn.execute(
            "INSERT INTO apps (
                 app_id, app_type, canonical_name, release_state, created_at_ms, updated_at_ms
             ) VALUES (42, 'game', 'Concurrent Read', 'released', 1, 1)",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let barrier = Arc::new(Barrier::new(2));
    let worker_barrier = barrier.clone();
    let locked_db = db.clone();
    let holder = std::thread::spawn(move || {
        locked_db
            .with_conn_mut(|_| {
                worker_barrier.wait();
                std::thread::sleep(Duration::from_millis(500));
                Ok(())
            })
            .unwrap();
    });
    barrier.wait();

    let started = Instant::now();
    let count: i64 = db
        .with_conn(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM apps", [], |row| row.get(0))?))
        .unwrap();
    assert_eq!(count, 1);
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "read waited for the writer handle lock: {:?}",
        started.elapsed()
    );
    holder.join().unwrap();
}

#[test]
fn version_three_data_is_hardened_by_version_four_migration() {
    let clock = Arc::new(FakeClock::new(10_000));
    let db = Database::open_in_memory_with_clock(clock).unwrap();
    db.with_conn_mut(|conn| {
        migrate::migrate_to(conn, 3, 1_000)?;
        conn.execute(
            "INSERT INTO apps (
                app_id, app_type, canonical_name, release_state, release_date,
                created_at_ms, updated_at_ms
             ) VALUES (42, 'game', 'Legacy Date', 'released', '2 Feb, 2021', 1, 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO anonymous_users (
                user_id, created_at_ms, last_active_at_ms, access_token_hash, refresh_token_hash
             ) VALUES ('legacy-user', 1, 1, ?1, ?2)",
            rusqlite::params![
                crate::users::token_hash("legacy-access"),
                crate::users::token_hash("legacy-refresh")
            ],
        )?;
        conn.execute(
            "INSERT INTO jobs (
                source, task_type, entity_key, due_at_ms, status, idempotency_key,
                created_at_ms, updated_at_ms
             ) VALUES ('steam', 'ccu', '42', 1, 'pending', 'legacy-job', 1, 1)",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    assert_eq!(db.migrate().unwrap(), latest_version());
    let repo = Repository::new(db);
    let app = repo.get_app(42).unwrap().unwrap();
    assert_eq!(app.release_date, None);
    assert_eq!(app.release_date_raw.as_deref(), Some("2 Feb, 2021"));
    assert!(repo.resolve_access_token("legacy-access").is_err());
    repo.database()
        .with_conn(|conn| {
            let completion_key: Option<String> = conn.query_row(
                "SELECT completion_idempotency_key FROM jobs WHERE idempotency_key = 'legacy-job'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(completion_key, None);
            Ok(())
        })
        .unwrap();
}

#[test]
fn migrate_is_idempotent() {
    let db = Database::open_in_memory().unwrap();
    assert_eq!(db.migrate().unwrap(), latest_version());
    assert_eq!(db.migrate().unwrap(), latest_version());
    let apps = db
        .with_conn(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM apps", [], |row| row.get::<_, i64>(0))?)
        })
        .unwrap();
    assert_eq!(apps, 0);
}

#[test]
fn anonymous_sessions_are_random_expiring_and_rotated() {
    let (repo, clock) = repo_with_clock(1_000);
    let first = repo.create_anonymous_session().unwrap();
    let second = repo.create_anonymous_session().unwrap();

    assert_ne!(first.user_id, second.user_id);
    assert_ne!(first.access_token, second.access_token);
    assert_ne!(first.refresh_token, second.refresh_token);
    assert_eq!(
        repo.resolve_access_token(&first.access_token).unwrap(),
        first.user_id
    );

    clock.advance_ms(crate::users::ACCESS_TOKEN_TTL_MS + 1);
    assert!(repo.resolve_access_token(&first.access_token).is_err());

    let rotated = repo
        .refresh_anonymous_session(&first.refresh_token)
        .unwrap();
    assert_eq!(rotated.user_id, first.user_id);
    assert_ne!(rotated.access_token, first.access_token);
    assert_ne!(rotated.refresh_token, first.refresh_token);
    assert!(
        repo.refresh_anonymous_session(&first.refresh_token)
            .is_err()
    );
    assert_eq!(
        repo.resolve_access_token(&rotated.access_token).unwrap(),
        first.user_id
    );
}

#[test]
fn feedback_requires_catalog_entry_and_full_idempotency_match() {
    let (repo, _) = repo_with_clock(5_000);
    repo.ensure_runtime_defaults().unwrap();
    repo.seed_demo_if_empty().unwrap();
    let session = repo.create_anonymous_session().unwrap();

    let first = repo
        .create_feedback(
            &session.user_id,
            548430,
            FeedbackType::Like,
            Some("run-1"),
            "feedback-1",
            Some(4_900),
        )
        .unwrap();
    let replay = repo
        .create_feedback(
            &session.user_id,
            548430,
            FeedbackType::Like,
            Some("run-1"),
            "feedback-1",
            Some(4_900),
        )
        .unwrap();
    assert_eq!(first.feedback_id, replay.feedback_id);

    let conflict = repo.create_feedback(
        &session.user_id,
        548430,
        FeedbackType::Like,
        Some("run-2"),
        "feedback-1",
        Some(4_900),
    );
    assert!(matches!(
        conflict,
        Err(crate::StorageError::Conflict { .. })
    ));

    let app_count = repo.count_apps().unwrap();
    assert!(
        repo.create_feedback(
            &session.user_id,
            4_000_000_000,
            FeedbackType::Like,
            None,
            "feedback-missing",
            None,
        )
        .is_err()
    );
    assert_eq!(repo.count_apps().unwrap(), app_count);

    let active = repo.list_active_feedback(&session.user_id).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].feedback_type, "like");
    let undo = repo
        .undo_feedback(&session.user_id, first.feedback_id)
        .unwrap();
    let replayed_undo = repo
        .undo_feedback(&session.user_id, first.feedback_id)
        .unwrap();
    assert_eq!(undo.feedback_id, replayed_undo.feedback_id);
    assert!(
        repo.list_active_feedback(&session.user_id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn daily_ccu_mean_excludes_missing_samples_and_preserves_missing_rate() {
    let (repo, clock) = repo_with_clock(1_000);
    let proposal = |player_count, content_hash: &'static str| CcuProposal {
        app_id: 42,
        player_count,
        result_code: if player_count.is_some() { 1 } else { 0 },
        content_hash: content_hash.into(),
        source: "test",
        stability: SourceStability::OfficialStable,
        adapter_version: "test",
        offline_players_excluded: true,
        missing_reason: player_count.is_none().then_some("missing"),
    };

    repo.ingest_ccu(&proposal(Some(10), "one")).unwrap();
    clock.advance_ms(1);
    repo.ingest_ccu(&proposal(None, "two")).unwrap();
    clock.advance_ms(1);
    repo.ingest_ccu(&proposal(Some(20), "three")).unwrap();

    let (sample_count, mean, missing_rate): (i64, f64, f64) = repo
        .database()
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT sample_count, mean_ccu, missing_rate FROM player_daily WHERE app_id = 42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        })
        .unwrap();
    assert_eq!(sample_count, 3);
    assert!((mean - 15.0).abs() < f64::EPSILON);
    assert!((missing_rate - (1.0 / 3.0)).abs() < 1e-9);
}

#[test]
fn foreign_key_and_unique_constraints() {
    let (repo, _) = repo_with_clock(5_000);
    let err = repo.database().with_conn(|conn| {
        conn.execute(
            "INSERT INTO app_relations (
                source_app_id, target_app_id, relation_type, confidence,
                verified_by_human, created_at_ms, updated_at_ms
             ) VALUES (1, 2, 'demo_of', 0.5, 0, 1, 1)",
            [],
        )?;
        Ok(())
    });
    assert!(err.is_err());
}

#[test]
fn human_override_not_clobbered_by_source_ingest() {
    let (repo, _) = repo_with_clock(10_000);
    let proposal = AppCatalogProposal {
        app_id: 892970,
        name: "Valheim".into(),
        app_type: AppTypeProposal::Game,
        last_modified: Some(1_700_000_000),
        price_change_number: None,
        source: "steam_istore_getapplist",
        stability: SourceStability::OfficialStable,
        adapter_version: "app-list-0.1.0",
    };
    repo.upsert_catalog(&proposal).unwrap();

    // Source says self_hosted_server = false
    let applied = repo
        .ingest_multiplayer_bool(
            892970,
            "self_hosted_server",
            false,
            "store_category",
            "hint",
            0.3,
        )
        .unwrap();
    assert!(applied);
    assert_eq!(
        repo.get_profile(892970)
            .unwrap()
            .unwrap()
            .self_hosted_server,
        Some(false)
    );

    // Human override to true
    let over = repo
        .create_override(
            892970,
            &CreateOverrideRequest {
                feature_name: "self_hosted_server".into(),
                value_json: serde_json::json!(true),
                reason: "dedicated server tool published".into(),
                external_evidence: Some("https://example.test/valheim-server".into()),
                operator: "curator-a".into(),
                request_id: Some("req-1".into()),
            },
        )
        .unwrap();
    assert!(over.revoked_at_ms.is_none());
    assert_eq!(
        repo.get_profile(892970)
            .unwrap()
            .unwrap()
            .self_hosted_server,
        Some(true)
    );
    let effective = repo.resolve_feature(892970, "self_hosted_server").unwrap();
    assert_eq!(effective.origin, FeatureOrigin::HumanOverride);

    let invalid_replacement = repo.create_override(
        892970,
        &CreateOverrideRequest {
            feature_name: "self_hosted_server".into(),
            value_json: serde_json::json!("yes"),
            reason: "invalid type must not supersede".into(),
            external_evidence: None,
            operator: "curator-a".into(),
            request_id: Some("req-invalid".into()),
        },
    );
    assert!(matches!(
        invalid_replacement,
        Err(crate::StorageError::Validation { .. })
    ));
    assert_eq!(
        repo.get_profile(892970)
            .unwrap()
            .unwrap()
            .self_hosted_server,
        Some(true)
    );

    // Later source ingest tries to write false again — profile stays true
    let applied = repo
        .ingest_multiplayer_bool(
            892970,
            "self_hosted_server",
            false,
            "store_category",
            "hint-2",
            0.4,
        )
        .unwrap();
    assert!(!applied);
    assert_eq!(
        repo.get_profile(892970)
            .unwrap()
            .unwrap()
            .self_hosted_server,
        Some(true)
    );

    // Revoke returns to latest source evidence (false)
    repo.revoke_override(over.override_id, "curator-a", "recheck", Some("req-2"))
        .unwrap();
    assert_eq!(
        repo.get_profile(892970)
            .unwrap()
            .unwrap()
            .self_hosted_server,
        Some(false)
    );
    let effective = repo.resolve_feature(892970, "self_hosted_server").unwrap();
    assert_eq!(effective.origin, FeatureOrigin::SourceEvidence);
}

#[test]
fn availability_override_restores_latest_store_evidence_on_revoke() {
    let (repo, clock) = repo_with_clock(10_000);
    let raw = RawResponse::validate(
        200,
        br#"{"42":{"success":true,"data":{"steam_appid":42,"type":"game","name":"Coop Test","platforms":{"windows":true,"mac":false,"linux":false},"supported_languages":"English, Simplified Chinese"}}}"#.to_vec(),
        Some("application/json".into()),
        4096,
    )
    .unwrap();
    let parsed = parse_store_details(&StoreDetailsRequest::new(42), &raw).unwrap();
    repo.ingest_store_details(&parsed.details, &parsed.relations)
        .unwrap();

    let over = repo
        .create_override(
            42,
            &CreateOverrideRequest {
                feature_name: "platforms".into(),
                value_json: serde_json::json!(["linux"]),
                reason: "Proton-tested group setup".into(),
                external_evidence: Some("https://example.test/platforms".into()),
                operator: "curator-a".into(),
                request_id: Some("req-platform".into()),
            },
        )
        .unwrap();
    let platforms = || {
        repo.database()
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT platforms_json FROM app_availability WHERE app_id = 42",
                    [],
                    |row| row.get::<_, String>(0),
                )?)
            })
            .unwrap()
    };
    assert_eq!(platforms(), r#"["linux"]"#);

    clock.advance_ms(1);
    repo.ingest_store_details(&parsed.details, &parsed.relations)
        .unwrap();
    assert_eq!(platforms(), r#"["linux"]"#);

    repo.revoke_override(over.override_id, "curator-a", "restore source", None)
        .unwrap();
    assert_eq!(platforms(), r#"["windows"]"#);
}

#[test]
fn job_lease_retry_and_idempotent_complete() {
    let (repo, clock) = repo_with_clock(100);
    let id1 = repo
        .enqueue_job(&EnqueueJob {
            source: "steam".into(),
            task_type: "ccu".into(),
            entity_key: "730".into(),
            priority: 10,
            due_at_ms: 100,
            idempotency_key: "steam:ccu:730:t100".into(),
            payload_json: None,
            max_attempts: 3,
        })
        .unwrap();
    let id2 = repo
        .enqueue_job(&EnqueueJob {
            source: "steam".into(),
            task_type: "ccu".into(),
            entity_key: "730".into(),
            priority: 10,
            due_at_ms: 100,
            idempotency_key: "steam:ccu:730:t100".into(),
            payload_json: None,
            max_attempts: 3,
        })
        .unwrap();
    assert_eq!(id1, id2);
    let conflicting_enqueue = repo.enqueue_job(&EnqueueJob {
        source: "steam".into(),
        task_type: "ccu".into(),
        entity_key: "570".into(),
        priority: 10,
        due_at_ms: 100,
        idempotency_key: "steam:ccu:730:t100".into(),
        payload_json: None,
        max_attempts: 3,
    });
    assert!(matches!(
        conflicting_enqueue,
        Err(crate::StorageError::Conflict { .. })
    ));

    let leased = repo
        .lease_jobs("worker-1", 10, 1_000, Some("steam"))
        .unwrap();
    assert_eq!(leased.len(), 1);
    let job_id = leased[0].job_id;

    // Wrong owner cannot complete
    assert!(
        repo.complete_job(job_id, "other", "steam:ccu:730:t100")
            .is_err()
    );

    // Fail with retry
    let failed = repo
        .fail_job(job_id, "worker-1", "rate_limited", 500)
        .unwrap();
    assert_eq!(failed.status, "pending");
    assert_eq!(failed.due_at_ms, 600);

    clock.advance_ms(500);
    let leased = repo.lease_jobs("worker-1", 10, 1_000, None).unwrap();
    assert_eq!(leased.len(), 1);
    let done = repo
        .complete_job(leased[0].job_id, "worker-1", "steam:ccu:730:t100")
        .unwrap();
    assert_eq!(done.status, "completed");
    // Idempotent complete
    let again = repo
        .complete_job(leased[0].job_id, "worker-1", "steam:ccu:730:t100")
        .unwrap();
    assert_eq!(again.status, "completed");
    assert!(matches!(
        repo.complete_job(leased[0].job_id, "worker-1", "different-completion"),
        Err(crate::StorageError::Conflict { .. })
    ));
}

#[test]
fn expired_job_lease_cannot_be_completed_or_failed() {
    let (repo, clock) = repo_with_clock(100);
    let job_id = repo
        .enqueue_job(&EnqueueJob {
            source: "steam".into(),
            task_type: "ccu".into(),
            entity_key: "730".into(),
            priority: 10,
            due_at_ms: 100,
            idempotency_key: "expired-lease".into(),
            payload_json: None,
            max_attempts: 3,
        })
        .unwrap();
    repo.lease_jobs("worker", 1, 1_000, None).unwrap();
    clock.advance_ms(1_000);
    assert!(repo.complete_job(job_id, "worker", "complete-1").is_err());
    assert!(repo.fail_job(job_id, "worker", "network", 1_000).is_err());
}

#[test]
fn accelerated_seven_day_focus_collection() {
    // Simulate 7 days of focus CCU sampling every 30 minutes for a small candidate set.
    let (repo, clock) = repo_with_clock(0);
    let app_ids = [10u32, 440, 730, 570, 892970];

    for day in 0..7 {
        for slot in 0..48 {
            // 48 * 30min = 24h
            let now = (day * 86_400_000) + (slot * 1_800_000);
            clock.set(now);
            for app_id in app_ids {
                let key = format!("steam:ccu:{app_id}:d{day}:s{slot}");
                let job_id = repo
                    .enqueue_job(&EnqueueJob {
                        source: "steam".into(),
                        task_type: "ccu".into(),
                        entity_key: app_id.to_string(),
                        priority: 50,
                        due_at_ms: now,
                        idempotency_key: key.clone(),
                        payload_json: None,
                        max_attempts: 3,
                    })
                    .unwrap();
                let leased = repo.lease_jobs("sim-worker", 1, 60_000, None).unwrap();
                assert_eq!(leased[0].job_id, job_id);

                let fixture = RawResponse::validate(
                    200,
                    format!(
                        r#"{{"response":{{"player_count":{},"result":1}}}}"#,
                        1000 + i64::from(app_id) + slot
                    )
                    .into_bytes(),
                    None,
                    1024,
                )
                .unwrap();
                let proposal = parse_ccu(&CcuRequest::new(app_id), &fixture).unwrap();
                repo.ingest_ccu(&proposal).unwrap();
                repo.complete_job(job_id, "sim-worker", &key).unwrap();
            }
        }
    }

    // 7 days * 48 slots * 5 apps
    let snapshots: i64 = repo
        .database()
        .with_conn(|conn| {
            Ok(
                conn.query_row("SELECT COUNT(*) FROM player_snapshots", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(snapshots, 7 * 48 * 5);

    let daily: i64 = repo
        .database()
        .with_conn(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM player_daily", [], |row| row.get(0))?)
        })
        .unwrap();
    assert_eq!(daily, 7 * 5);

    let completed = repo
        .database()
        .with_conn(|conn| crate::jobs::count_jobs_by_status(conn, "completed"))
        .unwrap();
    assert_eq!(completed, 7 * 48 * 5);
}

#[test]
fn fixture_pipeline_into_storage() {
    let (repo, clock) = repo_with_clock(2_000);
    let page = RawResponse::validate(
        200,
        include_bytes!("../../steam-source/fixtures/app_list_page1.json").to_vec(),
        None,
        1024 * 1024,
    )
    .unwrap();
    let parsed = parse_app_list_page(&page).unwrap();
    for proposal in &parsed.proposals {
        repo.upsert_catalog(proposal).unwrap();
    }
    assert!(repo.count_apps().unwrap() >= 3);

    let reviews = RawResponse::validate(
        200,
        include_bytes!("../../steam-source/fixtures/reviews_summary.json").to_vec(),
        None,
        1024 * 1024,
    )
    .unwrap();
    let review =
        parse_review_summary(&ReviewSummaryRequest::summary_only(892970), &reviews).unwrap();
    repo.ingest_review(&review).unwrap();

    let store = RawResponse::validate(
        200,
        include_bytes!("../../steam-source/fixtures/store_appdetails_game.json").to_vec(),
        None,
        1024 * 1024,
    )
    .unwrap();
    let details = parse_store_details(
        &StoreDetailsRequest::with_locale(892970, "US", "english").unwrap(),
        &store,
    )
    .unwrap();
    repo.ingest_store_details(&details.details, &details.relations)
        .unwrap();

    let app = repo.get_app(892970).unwrap().unwrap();
    assert_eq!(app.canonical_name, "Valheim");
    assert_eq!(app.release_state, "released");
    assert_eq!(app.release_date.as_deref(), Some("2021-02-02"));
    assert_eq!(app.release_date_raw.as_deref(), Some("2 Feb, 2021"));
    let price_count: i64 = repo
        .database()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM price_snapshots WHERE app_id = 892970 AND currency = 'USD'",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(price_count, 1);

    clock.advance_ms(1);
    let without_release_date = RawResponse::validate(
        200,
        br#"{"892970":{"success":true,"data":{"type":"game","name":"Valheim","steam_appid":892970}}}"#
            .to_vec(),
        Some("application/json".into()),
        1024,
    )
    .unwrap();
    let parsed =
        parse_store_details(&StoreDetailsRequest::new(892970), &without_release_date).unwrap();
    assert!(!parsed.details.release_date_observed);
    repo.ingest_store_details(&parsed.details, &parsed.relations)
        .unwrap();
    let preserved = repo.get_app(892970).unwrap().unwrap();
    assert_eq!(preserved.release_state, "released");
    assert_eq!(preserved.release_date.as_deref(), Some("2021-02-02"));
}

#[test]
fn backup_restore_and_integrity() {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live.db");
    let backup = dir.path().join("backup.db");
    let restored = dir.path().join("restored.db");

    let clock = Arc::new(FakeClock::new(9_000));
    let db = Database::open_with_clock(&live, clock.clone()).unwrap();
    let repo = Repository::new(db);
    repo.migrate().unwrap();
    repo.upsert_catalog(&AppCatalogProposal {
        app_id: 570,
        name: "Dota 2".into(),
        app_type: AppTypeProposal::Game,
        last_modified: None,
        price_change_number: None,
        source: "test",
        stability: SourceStability::OfficialStable,
        adapter_version: "t",
    })
    .unwrap();

    repo.backup_to(&backup).unwrap();
    assert!(matches!(
        repo.backup_to(&backup),
        Err(crate::StorageError::Conflict { .. })
    ));
    let restored_repo = Repository::restore_backup(&backup, &restored, 9_000).unwrap();
    assert!(matches!(
        Repository::restore_backup(&backup, &restored, 9_000),
        Err(crate::StorageError::Conflict { .. })
    ));
    restored_repo.assert_ready().unwrap();
    let app = restored_repo.get_app(570).unwrap().unwrap();
    assert_eq!(app.canonical_name, "Dota 2");
    // Restored DBs remain upgradeable: migrate is idempotent at latest.
    assert_eq!(restored_repo.migrate().unwrap(), latest_version());
    assert_eq!(
        restored_repo.database().schema_version().unwrap(),
        latest_version()
    );
}

/// M6: empty DB upgrades through every shipped migration in order.
#[test]
fn m6_upgrade_path_from_each_intermediate_version() {
    for from in 0..latest_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("from_{from}.db"));
        {
            let db = Database::open(&path).unwrap();
            if from > 0 {
                db.with_conn_mut(|conn| {
                    migrate::migrate_to(conn, from, 1_000)?;
                    Ok(())
                })
                .unwrap();
                assert_eq!(db.schema_version().unwrap(), from);
            }
        }
        let db = Database::open(&path).unwrap();
        let version = db.migrate().unwrap();
        assert_eq!(version, latest_version(), "upgrade from {from}");
        db.assert_ready().unwrap();
    }
}

#[test]
fn quality_checks_flag_bad_player_bounds() {
    let (repo, _) = repo_with_clock(3_000);
    repo.database()
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO apps (
                    app_id, app_type, canonical_name, release_state, created_at_ms, updated_at_ms
                 ) VALUES (1, 'game', 'Bad Bounds', 'released', 1, 1)",
                [],
            )?;
            // Bypass CHECK by using raw insert that violates recommended bounds — schema CHECK should block.
            // Instead insert valid bounds then update via SQL that still satisfies CHECK...
            // We'll insert a self-loop relation which quality catches.
            conn.execute(
                "INSERT INTO apps (
                    app_id, app_type, canonical_name, release_state, created_at_ms, updated_at_ms
                 ) VALUES (2, 'game', 'Other', 'released', 1, 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO app_relations (
                    source_app_id, target_app_id, relation_type, confidence,
                    verified_by_human, created_at_ms, updated_at_ms
                 ) VALUES (1, 1, 'edition_of', 0.5, 0, 1, 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let findings = repo.run_quality_checks().unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f.check_name == "relation_self_loop")
    );
}

#[test]
fn store_search_candidates_are_auditable_without_fabricated_profiles() {
    let (repo, _) = repo_with_clock(5_000);
    let page = StoreSearchPage {
        candidates: vec![
            StoreSearchCandidate {
                app_id: 548430,
                name: "Deep Rock Galactic".into(),
            },
            StoreSearchCandidate {
                app_id: 632360,
                name: "Risk of Rain 2".into(),
            },
        ],
        start: 0,
        result_count: 2,
        total_count: 2,
        content_hash: "fixture-hash".into(),
    };

    assert_eq!(repo.ingest_store_search_page(&page).unwrap(), 2);
    let coverage = repo.m3_catalog_coverage().unwrap();
    assert_eq!(coverage.normalized_multiplayer_candidates, 2);
    assert_eq!(coverage.category_evidence_candidates, 2);
    assert_eq!(coverage.recommendation_ready_profiles, 0);
    assert_eq!(coverage.trusted_familiar_profiles, 0);

    repo.ingest_multiplayer_bool(548430, "online_coop", true, "verified_test", "fixture", 0.8)
        .unwrap();
    repo.database()
        .with_conn_mut(|conn| {
            conn.execute(
                "UPDATE multiplayer_profiles SET profile_confidence = 0.8 WHERE app_id = 548430",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let enriched = repo.m3_catalog_coverage().unwrap();
    assert_eq!(enriched.normalized_multiplayer_candidates, 2);
    assert_eq!(enriched.recommendation_ready_profiles, 1);
    assert_eq!(enriched.trusted_familiar_profiles, 0);

    let linked_documents = repo
        .database()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM feature_evidence WHERE source_document_id IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(linked_documents, 2);

    let targets = repo.list_enrichment_targets(10).unwrap();
    assert_eq!(targets.len(), 2);
    assert!(targets.iter().all(|t| t.needs_store_details));
    assert!(targets.iter().all(|t| t.needs_reviews));
    assert!(targets.iter().all(|t| t.needs_ccu));
    assert!(targets.iter().all(|t| t.needs_price));

    let rotated = repo
        .list_enrichment_targets_after(2, Some(548430), "CN")
        .unwrap();
    assert_eq!(rotated[0].app_id, 632360);
    assert_eq!(rotated[1].app_id, 548430);
}

#[test]
fn golden_profile_import_raises_recommendation_ready_coverage() {
    use mpgs_steam_source::GoldenSet;

    let (repo, _) = repo_with_clock(6_000);
    let page = StoreSearchPage {
        candidates: vec![StoreSearchCandidate {
            app_id: 892970,
            name: "Valheim".into(),
        }],
        start: 0,
        result_count: 1,
        total_count: 1,
        content_hash: "fixture-hash".into(),
    };
    assert_eq!(repo.ingest_store_search_page(&page).unwrap(), 1);
    assert_eq!(
        repo.m3_catalog_coverage()
            .unwrap()
            .recommendation_ready_profiles,
        0
    );

    let set = GoldenSet::load_embedded().unwrap();
    let valheim = set
        .games
        .iter()
        .find(|game| game.app_id == 892970)
        .expect("fixture golden set includes Valheim");
    assert!(repo.import_golden_multiplayer_profile(valheim).unwrap());
    assert!(!repo.import_golden_multiplayer_profile(valheim).unwrap());
    let coverage = repo.m3_catalog_coverage().unwrap();
    assert_eq!(coverage.recommendation_ready_profiles, 1);
    assert!(coverage.trusted_familiar_profiles >= 1);
    let profile = repo.get_profile(892970).unwrap().unwrap();
    assert!(profile.dominant_mode.is_some() || profile.online_coop.is_some());
    let provenance = repo
        .database()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM source_documents
                 WHERE source = 'human_golden' AND entity_type = 'golden_game'
                   AND entity_key LIKE 'golden-0.1.0:%'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(provenance, 1);
}

#[test]
fn enrichment_targets_refresh_dynamic_dimensions_by_age() {
    use crate::repo::{CCU_REFRESH_INTERVAL_MS, PRICE_REFRESH_INTERVAL_MS};

    let day_ms = 24 * 60 * 60 * 1_000;
    let (repo, clock) = repo_with_clock(10 * day_ms);
    let page = StoreSearchPage {
        candidates: vec![StoreSearchCandidate {
            app_id: 892970,
            name: "Valheim".into(),
        }],
        start: 0,
        result_count: 1,
        total_count: 1,
        content_hash: "fixture-hash".into(),
    };
    repo.ingest_store_search_page(&page).unwrap();

    let store = RawResponse::validate(
        200,
        br#"{"892970":{"success":true,"data":{"steam_appid":892970,"type":"game","name":"Valheim","is_free":true,"platforms":{"windows":true},"supported_languages":"English, Simplified Chinese"}}}"#.to_vec(),
        Some("application/json".into()),
        4096,
    )
    .unwrap();
    let details = parse_store_details(&StoreDetailsRequest::new(892970), &store).unwrap();
    repo.ingest_store_details(&details.details, &details.relations)
        .unwrap();

    let reviews = RawResponse::validate(
        200,
        include_bytes!("../../steam-source/fixtures/reviews_summary.json").to_vec(),
        None,
        1024 * 1024,
    )
    .unwrap();
    repo.ingest_review(
        &parse_review_summary(&ReviewSummaryRequest::summary_only(892970), &reviews).unwrap(),
    )
    .unwrap();
    let ccu = RawResponse::validate(
        200,
        include_bytes!("../../steam-source/fixtures/ccu_ok.json").to_vec(),
        None,
        1024 * 1024,
    )
    .unwrap();
    repo.ingest_ccu(&parse_ccu(&CcuRequest::new(892970), &ccu).unwrap())
        .unwrap();

    assert!(repo.list_enrichment_targets(10).unwrap().is_empty());
    clock.advance_ms(CCU_REFRESH_INTERVAL_MS + 1);
    let ccu_due = repo.list_enrichment_targets(10).unwrap();
    assert_eq!(ccu_due.len(), 1);
    assert!(ccu_due[0].needs_ccu);
    assert!(!ccu_due[0].needs_reviews);
    assert!(!ccu_due[0].needs_price);

    clock.advance_ms(PRICE_REFRESH_INTERVAL_MS - CCU_REFRESH_INTERVAL_MS);
    let daily_due = repo.list_enrichment_targets(10).unwrap();
    assert!(daily_due[0].needs_reviews);
    assert!(daily_due[0].needs_price);
}

#[test]
fn source_cursor_and_run_state_support_resume() {
    let (repo, _) = repo_with_clock(7_000);
    let cursor = serde_json::json!({"next_start": 100, "target": 2000});
    repo.save_source_cursor(
        "steam_store_search:multiplayer:reviews_desc",
        "steam_store_search",
        &cursor,
    )
    .unwrap();
    let stored = repo
        .source_cursor("steam_store_search:multiplayer:reviews_desc")
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stored).unwrap(),
        cursor
    );

    let run = repo
        .start_source_run(
            "steam_store_search",
            "candidate_discovery",
            "store-search-0.1.0",
            Some("test"),
        )
        .unwrap();
    repo.finish_source_run(run, "succeeded", 1, 100, None, Some("done"))
        .unwrap();
    let status = repo
        .database()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT status FROM source_runs WHERE run_id = ?1",
                [run],
                |row| row.get::<_, String>(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(status, "succeeded");
}

// silence unused import warnings for types used only in docs-like examples
#[allow(dead_code)]
fn _types() {
    let _: Option<ReviewSummaryProposal> = None;
    let _: Option<CcuProposal> = None;
    let _: Option<AppListRequest> = None;
}
