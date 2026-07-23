#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use rusqlite::{params, Connection, OptionalExtension};
use tauri::{Manager, State};

const SESSION_KEY: &str = "mpgs.session.v1";
const SESSION_CREDENTIAL_SERVICE: &str = "dev.mpgs.desktop";
const SESSION_CREDENTIAL_ACCOUNT: &str = "session.v1";
const AI_CREDENTIAL_ACCOUNT: &str = "custom-ai.v1";

struct ClientStore(Mutex<Connection>);

impl ClientStore {
    fn open(app: &tauri::App) -> Result<Self, String> {
        let data_dir = client_data_dir(app)?;
        fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
        Self::open_at(&data_dir.join("client-state.sqlite3"))
    }

    fn open_at(path: &Path) -> Result<Self, String> {
        let connection = Connection::open(path).map_err(|error| error.to_string())?;
        connection
            .busy_timeout(Duration::from_secs(3))
            .map_err(|error| error.to_string())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = FULL;
                 CREATE TABLE IF NOT EXISTS client_kv (
                     key TEXT PRIMARY KEY NOT NULL,
                     value TEXT NOT NULL,
                     updated_at_ms INTEGER NOT NULL
                 );",
            )
            .map_err(|error| error.to_string())?;
        Ok(Self(Mutex::new(connection)))
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.0
            .lock()
            .map_err(|_| "client storage lock is poisoned".to_owned())
    }

    fn load(&self) -> Result<HashMap<String, String>, String> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT key, value FROM client_kv WHERE key <> ?1 ORDER BY key")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([SESSION_KEY], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<HashMap<_, _>, _>>()
            .map_err(|error| error.to_string())
    }

    fn take_legacy_session(&self) -> Result<Option<String>, String> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(|error| error.to_string())?;
        let value = transaction
            .query_row(
                "SELECT value FROM client_kv WHERE key = ?1",
                [SESSION_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        transaction
            .execute("DELETE FROM client_kv WHERE key = ?1", [SESSION_KEY])
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(value)
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        validate_client_key(key)?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis() as i64;
        self.lock()?
            .execute(
                "INSERT INTO client_kv (key, value, updated_at_ms) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at_ms = excluded.updated_at_ms",
                params![key, value, now_ms],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn remove(&self, key: &str) -> Result<(), String> {
        validate_client_key(key)?;
        self.lock()?
            .execute("DELETE FROM client_kv WHERE key = ?1", [key])
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

fn client_data_dir(app: &tauri::App) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("MPGS_CLIENT_DATA_DIR") {
        if path.is_empty() {
            return Err("MPGS_CLIENT_DATA_DIR must not be empty".to_owned());
        }
        return Ok(PathBuf::from(path));
    }
    app.path().app_data_dir().map_err(|error| error.to_string())
}

/// Session tokens — including per-service scoped keys — must use the OS keyring.
fn is_session_storage_key(key: &str) -> bool {
    key == SESSION_KEY || key.ends_with(".mpgs.session.v1") || key.ends_with(SESSION_KEY)
}

fn validate_client_key(key: &str) -> Result<(), String> {
    if is_session_storage_key(key) {
        Err("session tokens must use secure credential storage".to_owned())
    } else if key.starts_with("mpgs.") {
        Ok(())
    } else {
        Err("client storage key must use the mpgs namespace".to_owned())
    }
}

#[tauri::command]
fn client_store_load(store: State<'_, ClientStore>) -> Result<HashMap<String, String>, String> {
    store.load()
}

#[tauri::command]
fn client_store_take_legacy_session(store: State<'_, ClientStore>) -> Result<Option<String>, String> {
    store.take_legacy_session()
}

#[tauri::command]
fn client_store_set(
    key: String,
    value: String,
    store: State<'_, ClientStore>,
) -> Result<(), String> {
    store.set(&key, &value)
}

#[tauri::command]
fn client_store_remove(key: String, store: State<'_, ClientStore>) -> Result<(), String> {
    store.remove(&key)
}

fn session_credential() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SESSION_CREDENTIAL_SERVICE, SESSION_CREDENTIAL_ACCOUNT)
        .map_err(|error| format!("secure credential storage is unavailable: {error}"))
}

fn ai_credential() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SESSION_CREDENTIAL_SERVICE, AI_CREDENTIAL_ACCOUNT)
        .map_err(|error| format!("secure credential storage is unavailable: {error}"))
}

#[tauri::command]
fn auth_session_load() -> Result<Option<String>, String> {
    match session_credential()?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!("could not read secure session: {error}")),
    }
}

#[tauri::command]
fn auth_session_save(value: String) -> Result<(), String> {
    session_credential()?
        .set_password(&value)
        .map_err(|error| format!("could not save secure session: {error}"))
}

#[tauri::command]
fn auth_session_remove() -> Result<(), String> {
    match session_credential()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("could not remove secure session: {error}")),
    }
}

#[tauri::command]
fn ai_credential_load() -> Result<Option<String>, String> {
    match ai_credential()?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!("could not read secure AI credential: {error}")),
    }
}

#[tauri::command]
fn ai_credential_save(value: String) -> Result<(), String> {
    ai_credential()?
        .set_password(&value)
        .map_err(|error| format!("could not save secure AI credential: {error}"))
}

#[tauri::command]
fn ai_credential_remove() -> Result<(), String> {
    match ai_credential()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("could not remove secure AI credential: {error}")),
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            app.manage(ClientStore::open(app)?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            client_store_load,
            client_store_take_legacy_session,
            client_store_set,
            client_store_remove,
            auth_session_load,
            auth_session_save,
            auth_session_remove,
            ai_credential_load,
            ai_credential_save,
            ai_credential_remove
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_sqlite_persists_across_reopen_and_enforces_namespace() {
        let unique = format!(
            "mpgs-client-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let directory = std::env::temp_dir().join(unique);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("client-state.sqlite3");

        {
            let store = ClientStore::open_at(&path).unwrap();
            assert!(store.set("other.key", "rejected").is_err());
            assert!(store.set(SESSION_KEY, "rejected").is_err());
            store.set("mpgs.cache.feed", "snapshot").unwrap();
            assert_eq!(
                store.load().unwrap().get("mpgs.cache.feed").map(String::as_str),
                Some("snapshot")
            );
        }
        {
            let reopened = ClientStore::open_at(&path).unwrap();
            assert_eq!(
                reopened
                    .load()
                    .unwrap()
                    .get("mpgs.cache.feed")
                    .map(String::as_str),
                Some("snapshot")
            );
            reopened.remove("mpgs.cache.feed").unwrap();
            assert!(reopened.load().unwrap().is_empty());
        }

        fs::remove_dir_all(directory).unwrap();
    }
}
