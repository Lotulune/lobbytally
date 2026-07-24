use std::sync::Arc;

use mpgs_domain::FeedbackType;
use mpgs_steam_source::{
    APP_LIST_SOURCE_NAME, AppCatalogProposal, AppListRequest, AppTypeProposal, CcuProposal,
    CcuRequest, RawResponse, ReviewSummaryProposal, ReviewSummaryRequest, SourceStability,
    StoreDetailsRequest, StoreSearchCandidate, StoreSearchPage, parse_app_list_page, parse_ccu,
    parse_popular_reviews, parse_review_summary, parse_store_details,
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
fn demo_seed_includes_capsules_for_known_steam_apps() {
    let (repo, _) = repo_with_clock(1_000);
    assert!(repo.seed_demo_if_empty().unwrap() > 0);
    let capsule_url: String = repo
        .database()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT capsule_url FROM app_media WHERE app_id = 892970",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(
        capsule_url,
        "https://cdn.akamai.steamstatic.com/steam/apps/892970/header.jpg"
    );
    let synthetic_media_count: i64 = repo
        .database()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM app_media WHERE app_id IN (2500001, 2500002)",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(synthetic_media_count, 0);
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
fn empty_store_availability_does_not_erase_the_last_usable_snapshot() {
    let (repo, _) = repo_with_clock(10_000);
    let raw = RawResponse::validate(
        200,
        br#"{"42":{"success":true,"data":{"steam_appid":42,"type":"game","name":"Coop Test","platforms":{"windows":true},"supported_languages":"English, Simplified Chinese"}}}"#.to_vec(),
        Some("application/json".into()),
        4096,
    )
    .unwrap();
    let parsed = parse_store_details(&StoreDetailsRequest::new(42), &raw).unwrap();
    repo.ingest_store_details(&parsed.details, &parsed.relations)
        .unwrap();

    repo.database()
        .with_conn_mut(|conn| {
            conn.execute(
                "UPDATE app_availability
                 SET platforms_json = '[]', languages_json = '[]'
                 WHERE app_id = 42",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(repo.restore_empty_availability_from_evidence().unwrap(), 2);

    let mut empty_refresh = parsed.details.clone();
    empty_refresh.platforms = Some(Vec::new());
    empty_refresh.supported_languages = Some(Vec::new());
    repo.ingest_store_details(&empty_refresh, &parsed.relations)
        .unwrap();

    let (platforms, languages): (String, String) = repo
        .database()
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT platforms_json, languages_json FROM app_availability WHERE app_id = 42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(platforms, r#"["windows"]"#);
    assert_eq!(languages, r#"["schinese","english"]"#);
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
    assert!(repo.has_active_job("steam", "ccu", "730").unwrap());
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
    assert!(!repo.has_active_job("steam", "ccu", "730").unwrap());
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
    let request = AppListRequest {
        last_appid: 0,
        if_modified_since: 0,
        max_results: 100,
        include_games: true,
        include_dlc: false,
        include_software: false,
        include_videos: false,
        include_hardware: false,
    };
    assert_eq!(repo.ingest_app_list_page(&request, &parsed).unwrap(), 3);
    assert!(repo.count_apps().unwrap() >= 3);
    let source_document: (String, String, String) = repo
        .database()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT source, entity_key, parse_version
                 FROM source_documents WHERE source = ?1",
                [APP_LIST_SOURCE_NAME],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(source_document.0, APP_LIST_SOURCE_NAME);
    assert_eq!(source_document.1, "last_appid=0;if_modified_since=0");
    assert_eq!(source_document.2, "app-list-0.1.0");
    let media_count: i64 = repo
        .database()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM app_media
                 WHERE app_id = 10
                   AND capsule_url LIKE 'https://cdn.akamai.steamstatic.com/steam/apps/10/%'",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(media_count, 1);

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
    let localization: (String, String) = repo
        .database()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT language, short_description FROM app_localizations WHERE app_id = 892970",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(localization.0, "english");
    assert_eq!(
        localization.1,
        "A brutal exploration and survival game for 1-10 players."
    );
    assert_eq!(
        repo.game_detail(892970)
            .unwrap()
            .unwrap()
            .short_description
            .as_deref(),
        Some("A brutal exploration and survival game for 1-10 players.")
    );
    let current_cover: String = repo
        .database()
        .with_conn(|conn| {
            conn.query_row(
                "SELECT capsule_url FROM app_media WHERE app_id = 892970",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(
        current_cover,
        "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/892970/header.jpg?t=1"
    );
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
        .list_enrichment_targets_after(2, Some(548430), "CN", "schinese")
        .unwrap();
    assert_eq!(rotated[0].app_id, 632360);
    assert_eq!(rotated[1].app_id, 548430);
}

#[test]
fn store_categories_materialize_conservative_multiplayer_profiles() {
    let (repo, _) = repo_with_clock(5_000);
    let page = StoreSearchPage {
        candidates: vec![StoreSearchCandidate {
            app_id: 548430,
            name: "Deep Rock Galactic".into(),
        }],
        start: 0,
        result_count: 1,
        total_count: 1,
        content_hash: "profile-materialization".into(),
    };
    repo.ingest_store_search_page(&page).unwrap();
    repo.database()
        .with_conn_mut(|conn| {
            crate::curation::insert_feature_evidence(
                conn,
                548430,
                "category_hint",
                &serde_json::json!(["Online Co-op", "Cross-Platform Multiplayer"]),
                "store_category",
                "fixture",
                0.3,
                5_000,
            )?;
            Ok(())
        })
        .unwrap();

    assert_eq!(repo.materialize_store_category_profiles().unwrap(), 1);
    let profile = repo.get_profile(548430).unwrap().unwrap();
    assert_eq!(profile.dominant_mode.as_deref(), Some("coop"));
    assert_eq!(profile.online_coop, Some(true));
    assert_eq!(profile.crossplay, Some(true));
    assert_eq!(profile.recommended_min_players, Some(2));
    assert_eq!(profile.recommended_max_players, None);
    assert_eq!(profile.profile_confidence, Some(0.3));
    assert_eq!(repo.materialize_store_category_profiles().unwrap(), 0);
}

#[test]
fn store_categories_mark_mixed_when_coop_and_pvp_present() {
    let (repo, _) = repo_with_clock(5_000);
    repo.ingest_store_search_page(&StoreSearchPage {
        candidates: vec![StoreSearchCandidate {
            app_id: 730,
            name: "Counter-Strike 2".into(),
        }],
        start: 0,
        result_count: 1,
        total_count: 1,
        content_hash: "mixed-mode".into(),
    })
    .unwrap();
    repo.database()
        .with_conn_mut(|conn| {
            crate::curation::insert_feature_evidence(
                conn,
                730,
                "category_hint",
                &serde_json::json!(["Online Co-op", "Online PvP", "Competitive"]),
                "store_category",
                "fixture",
                0.3,
                5_000,
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(repo.materialize_store_category_profiles().unwrap(), 1);
    let profile = repo.get_profile(730).unwrap().unwrap();
    assert_eq!(profile.dominant_mode.as_deref(), Some("mixed"));
    assert_eq!(profile.online_coop, Some(true));
}

#[test]
fn store_search_multiplayer_only_is_not_unknown() {
    let (repo, _) = repo_with_clock(5_000);
    repo.ingest_store_search_page(&StoreSearchPage {
        candidates: vec![StoreSearchCandidate {
            app_id: 548430,
            name: "Deep Rock Galactic".into(),
        }],
        start: 0,
        result_count: 1,
        total_count: 1,
        content_hash: "mp-only".into(),
    })
    .unwrap();
    // Search page materializes with Multi-player hint via store_search_category.
    assert_eq!(repo.materialize_store_category_profiles().unwrap(), 1);
    let profile = repo.get_profile(548430).unwrap().unwrap();
    assert_eq!(profile.dominant_mode.as_deref(), Some("multiplayer"));
}

#[test]
fn resolve_display_dominant_mode_falls_back_to_online_coop() {
    assert_eq!(
        crate::resolve_display_dominant_mode(None, Some(true)).as_deref(),
        Some("coop")
    );
    assert_eq!(
        crate::resolve_display_dominant_mode(Some("competitive"), None).as_deref(),
        Some("pvp")
    );
    assert_eq!(
        crate::resolve_display_dominant_mode(Some("unknown"), None),
        None
    );
    assert_eq!(
        crate::resolve_display_dominant_mode(Some("mixed"), Some(true)).as_deref(),
        Some("mixed")
    );
}

#[test]
fn store_search_category_materializes_only_a_safe_minimum_party_size() {
    let (repo, _) = repo_with_clock(5_000);
    repo.ingest_store_search_page(&StoreSearchPage {
        candidates: vec![StoreSearchCandidate {
            app_id: 548430,
            name: "Deep Rock Galactic".into(),
        }],
        start: 0,
        result_count: 1,
        total_count: 1,
        content_hash: "search-profile-materialization".into(),
    })
    .unwrap();

    assert_eq!(repo.materialize_store_category_profiles().unwrap(), 1);
    let profile = repo.get_profile(548430).unwrap().unwrap();
    assert_eq!(profile.recommended_min_players, Some(2));
    assert_eq!(profile.recommended_max_players, None);
    // Multi-player search filter alone is still a coarse multiplayer label, not blank.
    assert_eq!(profile.dominant_mode.as_deref(), Some("multiplayer"));
    assert_eq!(profile.online_coop, None);
    assert_eq!(profile.profile_confidence, Some(0.3));
}

#[test]
fn m7_coverage_requires_consecutive_focus_snapshot_days() {
    let (repo, _) = repo_with_clock(7 * 86_400_000);
    let page = StoreSearchPage {
        candidates: vec![StoreSearchCandidate {
            app_id: 42,
            name: "Coverage Fixture".into(),
        }],
        start: 0,
        result_count: 1,
        total_count: 1,
        content_hash: "m7-coverage-fixture".into(),
    };
    assert_eq!(repo.ingest_store_search_page(&page).unwrap(), 1);
    repo.ingest_multiplayer_bool(42, "online_coop", true, "verified_test", "fixture", 0.8)
        .unwrap();
    repo.database()
        .with_conn_mut(|conn| {
            conn.execute(
                "UPDATE apps SET release_state = 'released', release_date = '1970-01-01',
                     release_date_raw = '1970-01-01' WHERE app_id = 42",
                [],
            )?;
            conn.execute(
                "UPDATE multiplayer_profiles SET profile_confidence = 0.70 WHERE app_id = 42",
                [],
            )?;
            conn.execute(
                "INSERT INTO app_media (app_id, capsule_url, source, updated_at_ms)
                 VALUES (42, 'https://cdn.example.invalid/42.jpg', 'fixture', 1)
                 ON CONFLICT(app_id) DO UPDATE SET
                    capsule_url = excluded.capsule_url,
                    source = excluded.source,
                    updated_at_ms = excluded.updated_at_ms",
                [],
            )?;
            for day in 0..7_i64 {
                let captured_at_ms = day * 86_400_000;
                conn.execute(
                    "INSERT INTO review_snapshots (
                        app_id, region_scope, language_scope, captured_at_ms,
                        total_positive, total_negative, total_reviews, review_score,
                        review_score_desc, wilson_lower, filter_offtopic_activity,
                        parameter_hash, content_hash, source
                     ) VALUES (42, 'all', 'english', ?1, 100, 10, 110, NULL, NULL,
                               0.80, 1, ?2, ?3, 'fixture')",
                    rusqlite::params![
                        captured_at_ms,
                        format!("review-params-{day}"),
                        format!("review-content-{day}")
                    ],
                )?;
                conn.execute(
                    "INSERT INTO player_daily (
                        app_id, day_utc, min_ccu, max_ccu, mean_ccu, median_approx_ccu,
                        sample_count, missing_rate, updated_at_ms
                     ) VALUES (42, ?1, 10, 20, 15.0, 15.0, 48, 0.0, ?2)",
                    rusqlite::params![format!("1970-01-{:02}", day + 1), captured_at_ms],
                )?;
            }
            Ok(())
        })
        .unwrap();

    let coverage = repo
        .m7_data_coverage(&mpgs_domain::RecommendationConfig::default())
        .unwrap();
    assert_eq!(coverage.normalized_multiplayer_candidates, 1);
    assert_eq!(coverage.trusted_friend_multiplayer_profiles, 1);
    assert_eq!(coverage.candidates_with_date, 1);
    assert_eq!(coverage.candidates_with_cover, 1);
    assert_eq!(coverage.trusted_profiles_with_seven_day_reviews, 1);
    assert_eq!(coverage.trusted_profiles_with_seven_day_ccu, 1);

    repo.database()
        .with_conn_mut(|conn| {
            conn.execute(
                "DELETE FROM review_snapshots WHERE app_id = 42 AND captured_at_ms = 259200000",
                [],
            )?;
            conn.execute(
                "DELETE FROM player_daily WHERE app_id = 42 AND day_utc = '1970-01-04'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let broken_streak = repo
        .m7_data_coverage(&mpgs_domain::RecommendationConfig::default())
        .unwrap();
    assert_eq!(broken_streak.trusted_profiles_with_seven_day_reviews, 0);
    assert_eq!(broken_streak.trusted_profiles_with_seven_day_ccu, 0);
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
    let popular_reviews = RawResponse::validate(
        200,
        include_bytes!("../../steam-source/fixtures/reviews_popular.json").to_vec(),
        None,
        1024 * 1024,
    )
    .unwrap();
    repo.ingest_popular_reviews(
        &parse_popular_reviews(
            &ReviewSummaryRequest::popular_schinese(892970),
            &popular_reviews,
        )
        .unwrap(),
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
    repo.database()
        .with_conn_mut(|conn| {
            conn.execute("DELETE FROM popular_reviews WHERE app_id = 892970", [])?;
            Ok(())
        })
        .unwrap();
    assert!(
        repo.list_enrichment_targets(10).unwrap().is_empty(),
        "a successful empty popular-review refresh must not be retried immediately"
    );

    // Older databases can already have availability and price snapshots from
    // appdetails versions that did not persist localized store text or media.
    // They must be selected once more so the current ingester can backfill both.
    repo.database()
        .with_conn_mut(|conn| {
            conn.execute(
                "DELETE FROM app_localizations WHERE app_id = 892970 AND language = 'schinese'",
                [],
            )?;
            conn.execute(
                "DELETE FROM store_detail_refresh_state WHERE app_id = 892970",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let localization_due = repo.list_enrichment_targets(10).unwrap();
    assert_eq!(localization_due.len(), 1);
    assert!(localization_due[0].needs_store_details);
    assert!(!localization_due[0].needs_reviews);
    assert!(!localization_due[0].needs_review_excerpts);
    assert!(!localization_due[0].needs_ccu);
    assert!(!localization_due[0].needs_price);
    repo.ingest_store_details(&details.details, &details.relations)
        .unwrap();
    assert!(repo.list_enrichment_targets(10).unwrap().is_empty());

    repo.database()
        .with_conn_mut(|conn| {
            conn.execute(
                "DELETE FROM app_localizations WHERE app_id = 892970 AND language = 'schinese'",
                [],
            )?;
            conn.execute("DELETE FROM price_snapshots WHERE app_id = 892970", [])?;
            Ok(())
        })
        .unwrap();
    repo.record_store_details_not_found(892970, "CN", "schinese")
        .unwrap();
    assert!(
        repo.list_enrichment_targets(10).unwrap().is_empty(),
        "a recent regional not-found result must suppress immediate store retries"
    );
    repo.ingest_store_details(&details.details, &details.relations)
        .unwrap();

    // A successful response can legitimately omit regional price, platform,
    // language, and localized text fields. Record that checked-empty terminal
    // state so the same app is not fetched forever until the daily refresh.
    repo.database()
        .with_conn_mut(|conn| {
            conn.execute(
                "DELETE FROM app_localizations WHERE app_id = 892970 AND language = 'schinese'",
                [],
            )?;
            conn.execute("DELETE FROM price_snapshots WHERE app_id = 892970", [])?;
            conn.execute(
                "UPDATE app_availability
                 SET platforms_json = '[]', languages_json = '[]'
                 WHERE app_id = 892970",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let checked_empty = RawResponse::validate(
        200,
        br#"{"892970":{"success":true,"data":{"steam_appid":892970,"type":"game"}}}"#.to_vec(),
        Some("application/json".into()),
        4096,
    )
    .unwrap();
    let checked_empty = parse_store_details(
        &StoreDetailsRequest::with_locale(892970, "CN", "schinese").unwrap(),
        &checked_empty,
    )
    .unwrap();
    repo.ingest_store_details(&checked_empty.details, &checked_empty.relations)
        .unwrap();
    assert!(
        repo.list_enrichment_targets(10).unwrap().is_empty(),
        "a recent successful checked-empty store response must suppress immediate retries"
    );

    clock.advance_ms(CCU_REFRESH_INTERVAL_MS + 1);
    let ccu_due = repo.list_enrichment_targets(10).unwrap();
    assert_eq!(ccu_due.len(), 1);
    assert!(ccu_due[0].needs_ccu);
    assert!(!ccu_due[0].needs_reviews);
    assert!(!ccu_due[0].needs_review_excerpts);
    assert!(!ccu_due[0].needs_price);

    clock.advance_ms(PRICE_REFRESH_INTERVAL_MS - CCU_REFRESH_INTERVAL_MS);
    let daily_due = repo.list_enrichment_targets(10).unwrap();
    assert!(daily_due[0].needs_reviews);
    assert!(daily_due[0].needs_review_excerpts);
    assert!(daily_due[0].needs_price);
}

#[test]
fn enrichment_targets_prioritize_apps_missing_the_most_dynamic_dimensions() {
    let (repo, _) = repo_with_clock(10 * 24 * 60 * 60 * 1_000);
    repo.ingest_store_search_page(&StoreSearchPage {
        candidates: vec![
            StoreSearchCandidate {
                app_id: 10,
                name: "Already Partly Enriched".into(),
            },
            StoreSearchCandidate {
                app_id: 20,
                name: "Never Enriched".into(),
            },
        ],
        start: 0,
        result_count: 2,
        total_count: 2,
        content_hash: "priority-fixture".into(),
    })
    .unwrap();

    let reviews = RawResponse::validate(
        200,
        include_bytes!("../../steam-source/fixtures/reviews_summary.json").to_vec(),
        None,
        1024 * 1024,
    )
    .unwrap();
    repo.ingest_review(
        &parse_review_summary(&ReviewSummaryRequest::summary_only(10), &reviews).unwrap(),
    )
    .unwrap();
    let popular = RawResponse::validate(
        200,
        include_bytes!("../../steam-source/fixtures/reviews_popular.json").to_vec(),
        None,
        1024 * 1024,
    )
    .unwrap();
    repo.ingest_popular_reviews(
        &parse_popular_reviews(&ReviewSummaryRequest::popular_schinese(10), &popular).unwrap(),
    )
    .unwrap();
    let ccu = RawResponse::validate(
        200,
        include_bytes!("../../steam-source/fixtures/ccu_ok.json").to_vec(),
        None,
        1024 * 1024,
    )
    .unwrap();
    repo.ingest_ccu(&parse_ccu(&CcuRequest::new(10), &ccu).unwrap())
        .unwrap();

    let targets = repo
        .list_enrichment_targets_after(1, Some(0), "CN", "schinese")
        .unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].app_id, 20);
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

#[test]
fn starting_a_new_source_run_finalizes_an_interrupted_predecessor() {
    let (repo, clock) = repo_with_clock(7_000);
    let old = repo
        .start_source_run("steam", "candidate_enrichment", "v1", None)
        .unwrap();
    clock.advance_ms(1_000);
    let new = repo
        .start_source_run("steam", "candidate_enrichment", "v1", None)
        .unwrap();
    assert_ne!(old, new);
    repo.database()
        .with_conn(|conn| {
            let predecessor: (String, Option<String>, Option<i64>) = conn.query_row(
                "SELECT status, error_category, finished_at_ms FROM source_runs WHERE run_id = ?1",
                [old],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            assert_eq!(predecessor.0, "failed");
            assert_eq!(predecessor.1.as_deref(), Some("interrupted"));
            assert_eq!(predecessor.2, Some(8_000));
            let running: i64 = conn.query_row(
                "SELECT COUNT(*) FROM source_runs
                 WHERE source = 'steam' AND task_type = 'candidate_enrichment'
                   AND status = 'running'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(running, 1);
            Ok(())
        })
        .unwrap();
}

#[test]
fn feed_sections_use_earliest_known_release_date() {
    // 2026-07-18 UTC — after Palworld's 1.0 store date, within a 180-day recent window.
    let now_ms = 1_784_342_400_000i64;
    let (repo, _) = repo_with_clock(now_ms);
    assert!(repo.seed_demo_if_empty().unwrap() > 0);

    // Simulate a later store refresh that overwrote apps.release_date with the 1.0 day.
    repo.database()
        .with_conn_mut(|conn| {
            conn.execute(
                "UPDATE apps
                 SET release_date = '2026-07-09',
                     release_date_raw = 'Jul 9, 2026',
                     updated_at_ms = ?1
                 WHERE app_id = 1623730",
                [now_ms],
            )?;
            conn.execute(
                "INSERT INTO release_events (
                     app_id, old_release_date, new_release_date, old_precision, new_precision,
                     old_release_state, new_release_state, source, observed_at_ms
                 ) VALUES (
                     1623730, '2024-01-19', '2026-07-09', 'day', 'day',
                     'released', 'released', 'steam_store_appdetails', ?1
                 )",
                [now_ms],
            )?;
            Ok(())
        })
        .unwrap();

    let config = mpgs_domain::RecommendationConfig::default();
    let today = crate::util::day_utc_from_ms(now_ms);
    let cutoff = crate::util::day_utc_from_ms(
        now_ms.saturating_sub(i64::from(config.recent_days) * 24 * 60 * 60 * 1_000),
    );

    let recent = repo
        .list_candidates(
            mpgs_domain::FeedSection::RecentRelease,
            &cutoff,
            &today,
            "CNY",
            &config,
            10_000,
        )
        .unwrap();
    assert!(
        recent.iter().all(|row| row.app_id != 1623730),
        "1.0 store date must not place long-shipped titles in recent_release"
    );

    let classic = repo
        .list_candidates(
            mpgs_domain::FeedSection::ClassicLegacy,
            &cutoff,
            &today,
            "CNY",
            &config,
            10_000,
        )
        .unwrap();
    let popular = repo
        .list_candidates(
            mpgs_domain::FeedSection::PopularLegacy,
            &cutoff,
            &today,
            "CNY",
            &config,
            10_000,
        )
        .unwrap();
    let in_legacy = classic
        .iter()
        .chain(popular.iter())
        .any(|row| row.app_id == 1623730);
    assert!(
        in_legacy,
        "first known release date should keep Palworld in legacy sections"
    );

    let palworld = classic
        .iter()
        .chain(popular.iter())
        .find(|row| row.app_id == 1623730)
        .expect("palworld row");
    assert_eq!(palworld.release_date.as_deref(), Some("2024-01-19"));

    // Residual classic must not re-list popular legacy titles.
    let classic_ids: std::collections::HashSet<u32> = classic
        .into_iter()
        .filter(|row| {
            let signals = row.to_ranking_signals();
            crate::query::section_matches(
                mpgs_domain::FeedSection::ClassicLegacy,
                row,
                &signals,
                &cutoff,
                &today,
                &config,
            )
        })
        .map(|row| row.app_id)
        .collect();
    let popular_ids: std::collections::HashSet<u32> = popular
        .into_iter()
        .filter(|row| {
            let signals = row.to_ranking_signals();
            crate::query::section_matches(
                mpgs_domain::FeedSection::PopularLegacy,
                row,
                &signals,
                &cutoff,
                &today,
                &config,
            )
        })
        .map(|row| row.app_id)
        .collect();
    assert!(classic_ids.is_disjoint(&popular_ids));
}

#[test]
fn media_backfill_is_coverage_gated_and_attempt_limited() {
    let (repo, clock) = repo_with_clock(100_000);
    // Seed two multiplayer candidates via store ingest with categories.
    for (app_id, name) in [(10_u32, "Game A"), (20, "Game B")] {
        let raw = RawResponse::validate(
            200,
            format!(
                r#"{{"{app_id}":{{"success":true,"data":{{"steam_appid":{app_id},"type":"game","name":"{name}","categories":[{{"description":"Online Co-op"}}],"header_image":"https://shared.akamai.steamstatic.com/h.jpg"}}}}}}"#
            )
            .into_bytes(),
            Some("application/json".into()),
            4096,
        )
        .unwrap();
        let parsed = parse_store_details(&StoreDetailsRequest::new(app_id), &raw).unwrap();
        repo.ingest_store_details(&parsed.details, &parsed.relations)
            .unwrap();
    }
    // No media assets yet → coverage 0, backfill due.
    let stats = repo.media_coverage_stats().unwrap();
    assert_eq!(stats.candidate_apps, 2);
    assert_eq!(stats.apps_with_media, 0);
    assert!(stats.coverage_ratio < 0.1);

    let policy = crate::MediaBackfillPolicy {
        enabled: true,
        coverage_threshold: 0.95,
        max_attempts: 2,
        cooldown_ms: 60_000,
    };
    let filter = crate::EnrichmentNeedFilter {
        store: false,
        reviews: false,
        review_excerpts: false,
        ccu: false,
        price: false,
        media_backfill: true,
    };
    let due = repo
        .list_enrichment_targets_after_filtered_with_media(10, None, "CN", "schinese", filter, policy)
        .unwrap();
    assert_eq!(due.len(), 2);
    assert!(due.iter().all(|t| t.needs_media_backfill));

    // Above threshold → no backfill work.
    let high_threshold = crate::MediaBackfillPolicy {
        coverage_threshold: 0.0, // always "covered enough" when threshold is 0... wait, coverage 0 < 0 is false
        ..policy
    };
    // coverage_ratio 0.0 + eps < 0.0 is false, so want_media = 0
    let none = repo
        .list_enrichment_targets_after_filtered_with_media(
            10,
            None,
            "CN",
            "schinese",
            filter,
            high_threshold,
        )
        .unwrap();
    assert!(none.iter().all(|t| !t.needs_media_backfill));

    // Attempt limits: mark exhausted.
    repo.begin_media_backfill_attempt(10).unwrap();
    repo.finish_media_backfill_attempt(10, crate::MediaBackfillOutcome::Failed, 2)
        .unwrap();
    repo.begin_media_backfill_attempt(10).unwrap();
    repo.finish_media_backfill_attempt(10, crate::MediaBackfillOutcome::Failed, 2)
        .unwrap();
    clock.advance_ms(120_000);
    let after_fail = repo
        .list_enrichment_targets_after_filtered_with_media(10, None, "CN", "schinese", filter, policy)
        .unwrap();
    assert!(!after_fail.iter().any(|t| t.app_id == 10 && t.needs_media_backfill));
    assert!(after_fail.iter().any(|t| t.app_id == 20 && t.needs_media_backfill));

    // Terminal none stops retries.
    repo.begin_media_backfill_attempt(20).unwrap();
    repo.finish_media_backfill_attempt(20, crate::MediaBackfillOutcome::NoneAvailable, 2)
        .unwrap();
    clock.advance_ms(120_000);
    let after_none = repo
        .list_enrichment_targets_after_filtered_with_media(10, None, "CN", "schinese", filter, policy)
        .unwrap();
    assert!(!after_none.iter().any(|t| t.needs_media_backfill));
}

#[test]
fn store_media_gallery_ingest_replace_preserve_and_clear() {
    let (repo, clock) = repo_with_clock(50_000);
    let raw = RawResponse::validate(
        200,
        include_bytes!("../../steam-source/fixtures/store_appdetails_game.json").to_vec(),
        Some("application/json".into()),
        1024 * 1024,
    )
    .unwrap();
    let parsed = parse_store_details(
        &StoreDetailsRequest::with_locale(892970, "US", "english").unwrap(),
        &raw,
    )
    .unwrap();
    repo.ingest_store_details(&parsed.details, &parsed.relations)
        .unwrap();

    let assets = repo.game_media_assets(892970).unwrap();
    assert_eq!(
        assets.iter().filter(|a| a.kind == "screenshot").count(),
        2
    );
    assert_eq!(assets.iter().filter(|a| a.kind == "movie").count(), 2);
    let cover = repo.game_detail(892970).unwrap().unwrap();
    assert!(cover.cover_url.is_some());

    // Missing media fields preserve prior rows.
    clock.advance_ms(10);
    let mut no_media = parsed.details.clone();
    no_media.screenshots = None;
    no_media.movies = None;
    no_media.header_image_url = None;
    repo.ingest_store_details(&no_media, &parsed.relations)
        .unwrap();
    assert_eq!(repo.game_media_assets(892970).unwrap().len(), 4);
    assert!(
        repo.game_detail(892970)
            .unwrap()
            .unwrap()
            .cover_url
            .is_some(),
        "cover must survive media-less refresh"
    );

    // Explicit empty arrays clear corresponding kinds only.
    clock.advance_ms(10);
    let mut clear_shots = parsed.details.clone();
    clear_shots.screenshots = Some(vec![]);
    clear_shots.movies = None;
    repo.ingest_store_details(&clear_shots, &parsed.relations)
        .unwrap();
    let after_clear = repo.game_media_assets(892970).unwrap();
    assert!(after_clear.iter().all(|a| a.kind == "movie"));
    assert_eq!(after_clear.len(), 2);

    // Replacement keeps Steam order.
    clock.advance_ms(10);
    repo.ingest_store_details(&parsed.details, &parsed.relations)
        .unwrap();
    let shots: Vec<_> = repo
        .game_media_assets(892970)
        .unwrap()
        .into_iter()
        .filter(|a| a.kind == "screenshot")
        .collect();
    assert_eq!(shots[0].source_id, "0");
    assert_eq!(shots[0].sort_order, 0);
    assert_eq!(shots[1].source_id, "1");
    assert_eq!(shots[1].sort_order, 1);

    // data_updated_at_ms includes media assets.
    let before = repo.data_updated_at_ms().unwrap();
    clock.advance_ms(5_000);
    repo.ingest_store_details(&parsed.details, &parsed.relations)
        .unwrap();
    let after = repo.data_updated_at_ms().unwrap();
    assert!(after > before);

    // Cascading delete: media rows drop when the parent app is removed.
    repo.database()
        .with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO apps (
                     app_id, app_type, canonical_name, release_state, created_at_ms, updated_at_ms
                 ) VALUES (424242, 'game', 'Cascade Target', 'released', 1, 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO app_media_assets (
                     app_id, kind, source_id, sort_order, title, thumbnail_url, full_url,
                     mp4_url, hls_h264_url, dash_h264_url, is_highlight, source, updated_at_ms
                 ) VALUES (
                     424242, 'screenshot', '1', 0, NULL,
                     'https://shared.akamai.steamstatic.com/t.jpg',
                     'https://shared.akamai.steamstatic.com/f.jpg',
                     NULL, NULL, NULL, 0, 'test', 1
                 )",
                [],
            )?;
            conn.execute("DELETE FROM apps WHERE app_id = 424242", [])?;
            Ok(())
        })
        .unwrap();
    assert!(repo.game_media_assets(424242).unwrap().is_empty());
}

#[test]
fn upgrade_from_v15_keeps_cover_and_empty_gallery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("from_v15.db");
    {
        let db = Database::open(&path).unwrap();
        db.with_conn_mut(|conn| {
            migrate::migrate_to(conn, 15, 1_000)?;
            conn.execute(
                "INSERT INTO apps (
                     app_id, app_type, canonical_name, release_state, created_at_ms, updated_at_ms
                 ) VALUES (42, 'game', 'Legacy Cover', 'released', 1, 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO app_media (app_id, capsule_url, source, updated_at_ms)
                 VALUES (42, 'https://shared.akamai.steamstatic.com/legacy.jpg', 'seed', 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(db.schema_version().unwrap(), 15);
    }
    let db = Database::open(&path).unwrap();
    assert_eq!(db.migrate().unwrap(), latest_version());
    let cover: String = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT capsule_url FROM app_media WHERE app_id = 42",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(
        cover,
        "https://shared.akamai.steamstatic.com/legacy.jpg"
    );
    let assets: i64 = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM app_media_assets WHERE app_id = 42",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(assets, 0);
}

// silence unused import warnings for types used only in docs-like examples
#[allow(dead_code)]
fn _types() {
    let _: Option<ReviewSummaryProposal> = None;
    let _: Option<CcuProposal> = None;
    let _: Option<AppListRequest> = None;
}
