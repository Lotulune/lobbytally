//! Community play-intent votes: one toggleable vote per (app, user), aggregated
//! into counts that feed the recommender's play-intent ranking signal.

use std::collections::{HashMap, HashSet};

use crate::pg::{Connection, OptionalExtension, params};

use crate::catalog;
use crate::error::{StorageError, StorageResult};

/// Authoritative state of one game's play-intent after a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayIntentState {
    pub app_id: u32,
    pub count: u32,
    pub voted: bool,
}

/// Monotonic cache-busting token for feeds and pagination cursors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayIntentEpoch {
    pub revision: u64,
}

/// Vote state used by a feed, read from one PostgreSQL snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayIntentFeedSnapshot {
    pub epoch: PlayIntentEpoch,
    pub counts: HashMap<u32, u32>,
    pub user_votes: HashSet<u32>,
}

/// Vote state used by game detail, read by one PostgreSQL statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayIntentGameSnapshot {
    pub epoch: PlayIntentEpoch,
    pub count: u32,
    pub voted: bool,
}

/// Toggle a user's vote for a game. Idempotent for a given `intent` value.
pub fn set_play_intent(
    conn: &Connection,
    user_id: &str,
    app_id: u32,
    intent: bool,
    now_ms: i64,
) -> StorageResult<PlayIntentState> {
    let tx = conn.unchecked_transaction()?;
    let account_active = tx
        .query_row(
            "SELECT user_id FROM user_accounts WHERE user_id = $1 AND status = 'active' FOR UPDATE",
            params![user_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if account_active.is_none() {
        return Err(StorageError::not_found("active account"));
    }
    if catalog::get_app(&tx, app_id)?.is_none() {
        return Err(StorageError::not_found(format!("game {app_id}")));
    }
    if intent {
        tx.execute(
            "INSERT INTO play_intent_votes (app_id, user_id, created_at_ms)
             VALUES ($1, $2, $3)
             ON CONFLICT (app_id, user_id) DO NOTHING",
            params![app_id, user_id, now_ms],
        )?;
    } else {
        tx.execute(
            "DELETE FROM play_intent_votes WHERE app_id = $1 AND user_id = $2",
            params![app_id, user_id],
        )?;
    }
    let state = PlayIntentState {
        app_id,
        count: count_for(&tx, app_id)?,
        voted: has_voted(&tx, user_id, app_id)?,
    };
    tx.commit()?;
    Ok(state)
}

pub fn count_for(conn: &Connection, app_id: u32) -> StorageResult<u32> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*)
         FROM play_intent_votes AS vote
         JOIN user_accounts AS account ON account.user_id = vote.user_id
         WHERE vote.app_id = $1 AND account.status = 'active'",
        params![app_id],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as u32)
}

pub fn has_voted(conn: &Connection, user_id: &str, app_id: u32) -> StorageResult<bool> {
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0
         FROM play_intent_votes AS vote
         JOIN user_accounts AS account ON account.user_id = vote.user_id
         WHERE vote.app_id = $1 AND vote.user_id = $2 AND account.status = 'active'",
        params![app_id, user_id],
        |row| row.get(0),
    )?;
    Ok(exists)
}

/// All non-zero vote counts keyed by app id (for feed ranking + display).
pub fn all_counts(conn: &Connection) -> StorageResult<HashMap<u32, u32>> {
    let mut stmt = conn.prepare(
        "SELECT vote.app_id, COUNT(*)
         FROM play_intent_votes AS vote
         JOIN user_accounts AS account ON account.user_id = vote.user_id
         WHERE account.status = 'active'
         GROUP BY vote.app_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)? as u32, row.get::<_, i64>(1)? as u32))
    })?;
    let mut map = HashMap::new();
    for row in rows {
        let (app_id, count) = row?;
        map.insert(app_id, count);
    }
    Ok(map)
}

/// The set of apps a user has voted for (for the `voted` flag in feeds).
pub fn user_votes(conn: &Connection, user_id: &str) -> StorageResult<HashSet<u32>> {
    let mut stmt = conn.prepare(
        "SELECT vote.app_id
         FROM play_intent_votes AS vote
         JOIN user_accounts AS account ON account.user_id = vote.user_id
         WHERE vote.user_id = $1 AND account.status = 'active'",
    )?;
    let rows = stmt.query_map(params![user_id], |row| Ok(row.get::<_, i64>(0)? as u32))?;
    let mut set = HashSet::new();
    for row in rows {
        set.insert(row?);
    }
    Ok(set)
}

pub fn epoch(conn: &Connection) -> StorageResult<PlayIntentEpoch> {
    conn.query_row(
        "SELECT revision FROM play_intent_state WHERE singleton = 1",
        [],
        |row| {
            Ok(PlayIntentEpoch {
                revision: row.get(0)?,
            })
        },
    )
    .map_err(StorageError::from)
}

pub fn feed_snapshot(
    conn: &Connection,
    user_id: Option<&str>,
) -> StorageResult<PlayIntentFeedSnapshot> {
    let mut statement = conn.prepare(
        "WITH votes AS (
             SELECT vote.app_id, COUNT(*) AS vote_count,
                    BOOL_OR(vote.user_id = $1::TEXT) AS voted
             FROM play_intent_votes AS vote
             JOIN user_accounts AS account ON account.user_id = vote.user_id
             WHERE account.status = 'active'
             GROUP BY vote.app_id
         )
         SELECT state.revision, votes.app_id, votes.vote_count,
                COALESCE(votes.voted, FALSE)
         FROM play_intent_state AS state
         LEFT JOIN votes ON TRUE
         WHERE state.singleton = 1",
    )?;
    let rows = statement.query_map(params![user_id], |row| {
        Ok((
            row.get::<_, u64>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, bool>(3)?,
        ))
    })?;
    let mut revision = None;
    let mut counts = HashMap::new();
    let mut user_votes = HashSet::new();
    for row in rows {
        let (current_revision, app_id, count, voted) = row?;
        revision = Some(current_revision);
        if let Some(app_id) = app_id {
            let app_id = app_id as u32;
            counts.insert(app_id, count.unwrap_or(0).max(0) as u32);
            if voted {
                user_votes.insert(app_id);
            }
        }
    }
    let epoch = PlayIntentEpoch {
        revision: revision
            .ok_or_else(|| StorageError::migration("play-intent state is missing"))?,
    };
    Ok(PlayIntentFeedSnapshot {
        epoch,
        counts,
        user_votes,
    })
}

pub fn game_snapshot(
    conn: &Connection,
    user_id: Option<&str>,
    app_id: u32,
) -> StorageResult<PlayIntentGameSnapshot> {
    conn.query_row(
        "SELECT state.revision,
                (SELECT COUNT(*)
                 FROM play_intent_votes AS vote
                 JOIN user_accounts AS account ON account.user_id = vote.user_id
                 WHERE vote.app_id = $1 AND account.status = 'active'),
                CASE WHEN $2::TEXT IS NULL THEN FALSE ELSE EXISTS (
                    SELECT 1
                    FROM play_intent_votes AS vote
                    JOIN user_accounts AS account ON account.user_id = vote.user_id
                    WHERE vote.app_id = $1 AND vote.user_id = $2 AND account.status = 'active'
                ) END
         FROM play_intent_state AS state WHERE state.singleton = 1",
        params![app_id, user_id],
        |row| {
            Ok(PlayIntentGameSnapshot {
                epoch: PlayIntentEpoch {
                    revision: row.get(0)?,
                },
                count: row.get::<_, i64>(1)?.max(0) as u32,
                voted: row.get(2)?,
            })
        },
    )
    .map_err(StorageError::from)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;
    use crate::accounts::{RegisterAccount, register_account};
    use crate::db::Database;

    fn setup() -> (Database, String) {
        let db = Database::open_in_memory().unwrap();
        db.with_conn_mut(|conn| {
            crate::migrate::migrate_to_latest(conn, 0)?;
            conn.execute(
                "INSERT INTO apps (app_id, app_type, canonical_name, release_state, created_at_ms, updated_at_ms)
                 VALUES (10, 'game', 'Ten', 'released', 0, 0), (20, 'game', 'Twenty', 'released', 0, 0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let account = db
            .with_conn_mut(|conn| {
                register_account(
                    conn,
                    &RegisterAccount {
                        username: "primary".into(),
                        display_name: "Primary".into(),
                        password: "primary-password-long".into(),
                        device_label: "test".into(),
                    },
                    None,
                    0,
                )
            })
            .unwrap();
        (db, account.user_id)
    }

    #[test]
    fn toggle_is_idempotent_and_counts() {
        let (db, user) = setup();
        db.with_conn_mut(|conn| {
            let a = set_play_intent(conn, &user, 10, true, 1)?;
            assert_eq!(a.count, 1);
            assert!(a.voted);
            // voting again keeps the count at 1
            let b = set_play_intent(conn, &user, 10, true, 2)?;
            assert_eq!(b.count, 1);
            // un-voting removes it
            let c = set_play_intent(conn, &user, 10, false, 3)?;
            assert_eq!(c.count, 0);
            assert!(!c.voted);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn counts_and_user_votes_aggregate() {
        let (db, user) = setup();
        let other = db
            .with_conn_mut(|conn| {
                register_account(
                    conn,
                    &RegisterAccount {
                        username: "other".into(),
                        display_name: "Other".into(),
                        password: "other-password-long".into(),
                        device_label: "test".into(),
                    },
                    None,
                    5,
                )
            })
            .unwrap()
            .user_id;
        db.with_conn_mut(|conn| {
            set_play_intent(conn, &user, 10, true, 1)?;
            set_play_intent(conn, &other, 10, true, 2)?;
            set_play_intent(conn, &user, 20, true, 3)?;
            let counts = all_counts(conn)?;
            assert_eq!(counts.get(&10), Some(&2));
            assert_eq!(counts.get(&20), Some(&1));
            let votes = user_votes(conn, &user)?;
            assert!(votes.contains(&10) && votes.contains(&20));
            assert!(has_voted(conn, &user, 10)?);
            assert!(!has_voted(conn, &other, 20)?);
            let e = epoch(conn)?;
            assert_eq!(e.revision, 3);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn revision_is_monotonic_and_snapshots_are_consistent() {
        let (db, user) = setup();
        db.with_conn_mut(|conn| {
            assert_eq!(epoch(conn)?.revision, 0);
            set_play_intent(conn, &user, 10, true, 7)?;
            assert_eq!(epoch(conn)?.revision, 1);
            // Idempotent writes do not change the logical revision.
            set_play_intent(conn, &user, 10, true, 7)?;
            assert_eq!(epoch(conn)?.revision, 1);

            let feed = feed_snapshot(conn, Some(&user))?;
            assert_eq!(feed.epoch.revision, 1);
            assert_eq!(feed.counts.get(&10), Some(&1));
            assert!(feed.user_votes.contains(&10));

            set_play_intent(conn, &user, 10, false, 7)?;
            assert_eq!(epoch(conn)?.revision, 2);
            let game = game_snapshot(conn, Some(&user), 10)?;
            assert_eq!(game.epoch.revision, 2);
            assert_eq!(game.count, 0);
            assert!(!game.voted);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn vote_for_missing_game_is_not_found() {
        let (db, user) = setup();
        db.with_conn_mut(|conn| {
            let result = set_play_intent(conn, &user, 999, true, 1);
            assert!(matches!(result, Err(StorageError::NotFound { .. })));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn concurrent_duplicate_votes_change_count_and_revision_once() {
        let (db, user) = setup();
        let handles: Vec<_> = (0..4).map(|_| db.reconnect().unwrap()).collect();
        let barrier = Arc::new(Barrier::new(handles.len()));
        let threads: Vec<_> = handles
            .into_iter()
            .map(|handle| {
                let user = user.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    handle
                        .with_conn(|conn| set_play_intent(conn, &user, 10, true, 1))
                        .unwrap()
                })
            })
            .collect();
        for thread in threads {
            let state = thread.join().unwrap();
            assert_eq!(state.count, 1);
            assert!(state.voted);
        }
        let snapshot = db
            .with_conn(|conn| feed_snapshot(conn, Some(&user)))
            .unwrap();
        assert_eq!(snapshot.epoch.revision, 1);
        assert_eq!(snapshot.counts.get(&10), Some(&1));
        assert!(snapshot.user_votes.contains(&10));
    }
}
