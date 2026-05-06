use crate::ai_batch_refresh_task::AiBatchRefreshRuntimeState;
use crate::backfill_task::BackfillRuntimeState;
use crate::classic_discovery_task::ClassicDiscoveryRuntimeState;
use crate::discovery_task::DiscoveryRuntimeState;
use crate::sync_task::SyncRuntimeState;
use reqwest::Client;
use rusqlite::Connection;
use std::sync::Mutex;

#[derive(Debug, Default)]
pub struct AutoSchedulerRuntimeState {
    pub evaluating: bool,
    pub startup_new_discovery_bootstrap_completed: bool,
}

pub struct AppState {
    pub db: Mutex<Connection>,
    pub http: Client,
    pub discovery: Mutex<DiscoveryRuntimeState>,
    pub classic_discovery: Mutex<ClassicDiscoveryRuntimeState>,
    pub backfill: Mutex<BackfillRuntimeState>,
    pub sync: Mutex<SyncRuntimeState>,
    pub ai_batch_refresh: Mutex<AiBatchRefreshRuntimeState>,
    pub auto_scheduler: Mutex<AutoSchedulerRuntimeState>,
}
