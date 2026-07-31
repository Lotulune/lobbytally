use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags, backup::Backup};

use crate::db::Database;
use crate::error::{StorageError, StorageResult};
use crate::migrate;

/// Online backup of the active database into `dest_path`.
pub fn backup_to_path(db: &Database, dest_path: impl AsRef<Path>) -> StorageResult<()> {
    let dest_path = dest_path.as_ref();
    prepare_destination(dest_path, "backup")?;
    let mut temporary = TemporaryDatabase::create_for(dest_path)?;

    db.with_conn(|src| {
        let mut dst = Connection::open(temporary.path())?;
        {
            let backup = Backup::new(src, &mut dst)?;
            backup
                .run_to_completion(100, std::time::Duration::from_millis(5), None)
                .map_err(|e| StorageError::migration(format!("backup failed: {e}")))?;
        }
        Ok(())
    })?;
    verify_backup_source(temporary.path())?;
    temporary.publish_noclobber(dest_path, "backup")
}

/// Restore a backup file into a new destination path and verify integrity/migrations.
pub fn restore_from_backup(
    backup_path: impl AsRef<Path>,
    dest_path: impl AsRef<Path>,
    now_ms: i64,
) -> StorageResult<Database> {
    let backup_path = backup_path.as_ref();
    let dest_path = dest_path.as_ref();
    if !backup_path.exists() {
        return Err(StorageError::not_found(format!(
            "backup {}",
            backup_path.display()
        )));
    }
    verify_backup_source(backup_path)?;
    prepare_destination(dest_path, "restore")?;
    let mut temporary = TemporaryDatabase::create_for(dest_path)?;
    std::fs::copy(backup_path, temporary.path())?;

    let db = Database::open(temporary.path())?;
    // Ensure restored DB can still accept forward migrations, then invalidate
    // every credential whose revocation may have occurred after the snapshot.
    db.with_conn_mut(|conn| {
        migrate::migrate_to_latest(conn, now_ms)?;
        invalidate_restored_sessions(conn, now_ms)?;
        Ok(())
    })?;
    db.assert_ready()?;
    // Fold all WAL content into the main file before publishing a single-file
    // artifact. Database::open re-enables WAL on the final path.
    db.with_conn_mut(|conn| {
        let mode: String = conn.query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("delete") {
            return Err(StorageError::migration(format!(
                "unable to finalize restored database journal: {mode}"
            )));
        }
        Ok(())
    })?;
    drop(db);

    temporary.publish_noclobber(dest_path, "restore")?;
    let restored = Database::open(dest_path)?;
    restored.assert_ready()?;
    Ok(restored)
}

/// Open a read-only connection against a file for verification helpers.
pub fn open_readonly(path: impl AsRef<Path>) -> StorageResult<Connection> {
    let conn = Connection::open_with_flags(
        path.as_ref(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    Ok(conn)
}

fn verify_backup_source(path: &Path) -> StorageResult<()> {
    let conn = open_readonly(path)?;
    let mut stmt = conn.prepare("PRAGMA integrity_check")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let checks = rows.collect::<Result<Vec<_>, _>>()?;
    if checks != ["ok".to_owned()] {
        return Err(StorageError::migration(format!(
            "backup integrity_check failed: {checks:?}"
        )));
    }
    Ok(())
}

fn invalidate_restored_sessions(conn: &mut Connection, now_ms: i64) -> StorageResult<()> {
    let transaction = conn.transaction()?;
    transaction.execute(
        "UPDATE account_sessions
         SET revoked_at_ms = COALESCE(revoked_at_ms, ?1)",
        [now_ms],
    )?;
    transaction.execute(
        "UPDATE anonymous_users
         SET access_expires_at_ms = 0, refresh_expires_at_ms = 0",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

fn prepare_destination(path: &Path, operation: &str) -> StorageResult<()> {
    let Some(file_name) = path.file_name() else {
        return Err(StorageError::validation(format!(
            "{operation} destination must name a file"
        )));
    };
    if file_name.is_empty() {
        return Err(StorageError::validation(format!(
            "{operation} destination must name a file"
        )));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    // This is only an early diagnostic. publish_noclobber performs the actual
    // atomic no-overwrite check and also rejects dangling symlinks.
    if path.exists() {
        return Err(StorageError::conflict(format!(
            "{operation} destination already exists: {}",
            path.display()
        )));
    }
    Ok(())
}

struct TemporaryDatabase {
    path: PathBuf,
    reservation: Option<File>,
}

impl TemporaryDatabase {
    fn create_for(destination: &Path) -> StorageResult<Self> {
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let file_name = destination
            .file_name()
            .ok_or_else(|| StorageError::validation("database destination must name a file"))?;
        for _ in 0..32 {
            let mut random = [0_u8; 16];
            getrandom::fill(&mut random)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let mut name = OsString::from(".");
            name.push(file_name);
            name.push(format!(".tmp-{suffix}"));
            let path = parent.join(name);
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        reservation: Some(file),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(StorageError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "unable to reserve a unique temporary database path",
        )))
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn publish_noclobber(&mut self, destination: &Path, operation: &str) -> StorageResult<()> {
        match std::fs::hard_link(&self.path, destination) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(StorageError::conflict(format!(
                    "{operation} destination already exists: {}",
                    destination.display()
                )));
            }
            Err(error) => return Err(error.into()),
        }
        // The hard link is the atomic publication point. Closing and removing
        // the private name cannot affect the published inode.
        self.reservation.take();
        let _ = std::fs::remove_file(&self.path);
        remove_sqlite_sidecars(&self.path);
        Ok(())
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        self.reservation.take();
        let _ = std::fs::remove_file(&self.path);
        remove_sqlite_sidecars(&self.path);
    }
}

fn remove_sqlite_sidecars(path: &Path) {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(sidecar));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        accounts::{RegisterAccount, register_account},
        users,
    };

    #[test]
    fn restored_database_invalidates_account_and_anonymous_tokens() {
        let directory = tempfile::tempdir().unwrap();
        let live_path = directory.path().join("live.db");
        let backup_path = directory.path().join("backup.db");
        let restored_path = directory.path().join("restored.db");
        let live = Database::open(&live_path).unwrap();
        live.migrate().unwrap();
        let anonymous = live
            .with_conn_mut(|conn| users::create_anonymous_session(conn, 1))
            .unwrap();
        let account = live
            .with_conn_mut(|conn| {
                register_account(
                    conn,
                    &RegisterAccount {
                        username: "restore_user".into(),
                        display_name: "Restore User".into(),
                        password: "restore-password-long".into(),
                        device_label: "test".into(),
                    },
                    Some(&anonymous.user_id),
                    2,
                )
            })
            .unwrap();
        backup_to_path(&live, &backup_path).unwrap();

        let restored = restore_from_backup(&backup_path, &restored_path, 3).unwrap();
        assert!(
            restored
                .with_conn(|conn| {
                    crate::accounts::resolve_account_user_id(conn, &account.access_token, 3)
                })
                .is_err()
        );
        assert!(
            restored
                .with_conn_mut(|conn| {
                    crate::accounts::refresh_account_session(conn, &account.refresh_token, 3)
                })
                .is_err()
        );
        assert!(
            restored
                .with_conn(|conn| users::resolve_user_id(conn, &anonymous.access_token, 3))
                .is_err()
        );
        assert!(
            restored
                .with_conn_mut(|conn| {
                    users::refresh_anonymous_session(conn, &anonymous.refresh_token, 3)
                })
                .is_err()
        );
    }
}
