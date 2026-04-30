use crate::backfill_task::BackfillRuntimeState;
use crate::discovery_task::DiscoveryRuntimeState;
use crate::sync_task::SyncRuntimeState;
use reqwest::Client;
use rusqlite::Connection;
use std::sync::Mutex;

pub struct AppState {
    pub db: Mutex<Connection>,
    pub http: Client,
    pub discovery: Mutex<DiscoveryRuntimeState>,
    pub backfill: Mutex<BackfillRuntimeState>,
    pub sync: Mutex<SyncRuntimeState>,
}
