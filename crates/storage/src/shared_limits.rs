//! Cross-instance request budgets and expiring AI admission leases.

use std::collections::BTreeSet;

use crate::{
    StorageError, StorageResult,
    pg::{Connection, TransactionBehavior, params},
};

pub fn consume_http_limit(
    conn: &Connection,
    identity_keys: &[String],
    identity_limit: u64,
    global_limit: u64,
    window_seconds: i64,
    now_ms: i64,
) -> StorageResult<bool> {
    let identity_limit = i64::try_from(identity_limit)
        .map_err(|_| StorageError::validation("identity request limit is too large"))?;
    let global_limit = i64::try_from(global_limit)
        .map_err(|_| StorageError::validation("global request limit is too large"))?;
    if identity_keys.is_empty()
        || identity_keys.len() > 8
        || identity_limit <= 0
        || global_limit <= 0
        || !(1..=3_600).contains(&window_seconds)
    {
        return Err(StorageError::validation("invalid shared request budget"));
    }
    let namespace = identity_keys[0]
        .split_once(':')
        .map(|(namespace, _)| namespace)
        .filter(|namespace| {
            !namespace.is_empty()
                && namespace
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        })
        .ok_or_else(|| {
            StorageError::validation("request budget keys require a namespace prefix")
        })?;
    let mut keys: BTreeSet<&str> = BTreeSet::new();
    for key in identity_keys {
        if key.len() > 256
            || key.split_once(':').map(|(prefix, _)| prefix) != Some(namespace)
            || key.ends_with(":global")
        {
            return Err(StorageError::validation(
                "request budget keys must share a namespace",
            ));
        }
        keys.insert(key);
    }
    let global_key = format!("{namespace}:global");
    let window_ms = window_seconds * 1_000;
    let window_start = now_ms.div_euclid(window_ms) * window_ms;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
    tx.query_row(
        "SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':rate:' || $1, 0))",
        [namespace],
        |_| Ok(()),
    )?;
    // Windows have a maximum of one hour. Old identities are no longer useful.
    tx.execute(
        "DELETE FROM http_rate_limits WHERE window_started_at_ms < $1",
        [now_ms.saturating_sub(2 * 60 * 60 * 1_000)],
    )?;
    let mut budgets: Vec<(&str, i64)> = keys.into_iter().map(|key| (key, identity_limit)).collect();
    budgets.push((&global_key, global_limit));
    for (key, limit) in &budgets {
        let count: i64 = tx.query_row("SELECT COALESCE((SELECT request_count FROM http_rate_limits WHERE scope = $1 AND window_started_at_ms = $2), 0)", params![key, window_start], |row| row.get(0))?;
        if count >= *limit {
            tx.commit()?;
            return Ok(false);
        }
    }
    for (key, _) in budgets {
        tx.execute("INSERT INTO http_rate_limits (scope, window_started_at_ms, request_count) VALUES ($1, $2, 1)
            ON CONFLICT (scope) DO UPDATE SET window_started_at_ms = EXCLUDED.window_started_at_ms,
            request_count = CASE WHEN http_rate_limits.window_started_at_ms = EXCLUDED.window_started_at_ms
                THEN http_rate_limits.request_count + 1 ELSE 1 END", params![key, window_start])?;
    }
    tx.commit()?;
    Ok(true)
}

pub fn acquire_ai_lease(
    conn: &Connection,
    scope: &str,
    scope_limit: usize,
    global_limit: usize,
    ttl_seconds: i64,
    now_ms: i64,
) -> StorageResult<Option<String>> {
    if scope.is_empty()
        || scope.len() > 256
        || scope_limit == 0
        || global_limit == 0
        || !(1..=600).contains(&ttl_seconds)
    {
        return Err(StorageError::validation("invalid AI concurrency budget"));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
    tx.query_row(
        "SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':ai-admission', 0))",
        [],
        |_| Ok(()),
    )?;
    tx.execute(
        "DELETE FROM ai_concurrency_leases WHERE expires_at_ms <= $1",
        [now_ms],
    )?;
    let (global_count, scope_count): (u64, u64) = tx.query_row(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE scope = $1) FROM ai_concurrency_leases",
        [scope],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if global_count >= global_limit as u64 || scope_count >= scope_limit as u64 {
        tx.commit()?;
        return Ok(None);
    }
    let mut random = [0u8; 24];
    getrandom::fill(&mut random)
        .map_err(|error| StorageError::migration(format!("AI lease randomness failed: {error}")))?;
    let lease_id = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    tx.execute(
        "INSERT INTO ai_concurrency_leases (lease_id, scope, expires_at_ms) VALUES ($1, $2, $3)",
        params![lease_id, scope, now_ms.saturating_add(ttl_seconds * 1_000)],
    )?;
    tx.commit()?;
    Ok(Some(lease_id))
}

pub fn release_ai_lease(conn: &Connection, lease_id: &str) -> StorageResult<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
    tx.execute_batch("SET LOCAL statement_timeout = '1500ms'")?;
    tx.execute(
        "DELETE FROM ai_concurrency_leases WHERE lease_id = $1",
        [lease_id],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{Database, FakeClock, Repository};
    use std::sync::Arc;

    #[test]
    fn shared_request_budgets_are_atomic_and_rejection_does_not_spend_other_keys() {
        let clock = Arc::new(FakeClock::new(1_000));
        let db = Database::open_in_memory_with_clock(clock.clone()).unwrap();
        db.migrate().unwrap();
        let first = Repository::new(db.clone());
        let second = Repository::new(db.reconnect().unwrap());
        let keys = vec!["http:account:a".to_owned(), "http:ip:one".to_owned()];
        assert!(first.consume_http_limit(&keys, 1, 3, 60).unwrap());
        assert!(
            !second
                .consume_http_limit(
                    &["http:account:a".to_owned(), "http:ip:two".to_owned()],
                    1,
                    3,
                    60
                )
                .unwrap()
        );
        assert!(
            second
                .consume_http_limit(
                    &["http:account:b".to_owned(), "http:ip:two".to_owned()],
                    1,
                    3,
                    60
                )
                .unwrap()
        );
        assert!(
            first
                .consume_http_limit(&["http:account:c".to_owned()], 1, 3, 60)
                .unwrap()
        );
        assert!(
            !second
                .consume_http_limit(&["http:account:d".to_owned()], 1, 3, 60)
                .unwrap()
        );
        assert!(
            second
                .consume_http_limit(&["ai:account:a".to_owned()], 1, 3, 60)
                .unwrap()
        );
        clock.advance_ms(60_000);
        assert!(second.consume_http_limit(&keys, 1, 3, 60).unwrap());
    }

    #[test]
    fn ai_leases_enforce_shared_limits_release_and_expiry() {
        let clock = Arc::new(FakeClock::new(1_000));
        let db = Database::open_in_memory_with_clock(clock.clone()).unwrap();
        db.migrate().unwrap();
        let first = Repository::new(db.clone());
        let second = Repository::new(db.reconnect().unwrap());
        let lease = first.acquire_ai_lease("ai:a", 1, 2, 60).unwrap().unwrap();
        assert!(second.acquire_ai_lease("ai:a", 1, 2, 60).unwrap().is_none());
        assert!(second.acquire_ai_lease("ai:b", 1, 2, 60).unwrap().is_some());
        assert!(first.acquire_ai_lease("ai:c", 1, 2, 60).unwrap().is_none());
        second.release_ai_lease(&lease).unwrap();
        assert!(first.acquire_ai_lease("ai:c", 1, 2, 60).unwrap().is_some());
        clock.advance_ms(60_000);
        assert!(second.acquire_ai_lease("ai:a", 1, 2, 60).unwrap().is_some());
    }

    #[test]
    fn simultaneous_instances_cannot_overspend_the_global_budget() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let first = Repository::new(db.clone());
        let second = Repository::new(db.reconnect().unwrap());
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let other_barrier = barrier.clone();
        let worker = std::thread::spawn(move || {
            other_barrier.wait();
            first
                .consume_http_limit(&["http:a".to_owned()], 1, 1, 60)
                .unwrap()
        });
        barrier.wait();
        let admitted = second
            .consume_http_limit(&["http:b".to_owned()], 1, 1, 60)
            .unwrap();
        assert_eq!(
            usize::from(worker.join().unwrap()) + usize::from(admitted),
            1
        );
    }
}
