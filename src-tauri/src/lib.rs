pub mod ai_batch_refresh_task;
pub mod auto_scheduler;
pub mod backfill_task;
pub mod classic_discovery_task;
pub mod commands;
pub mod db;
pub mod discovery;
pub mod discovery_task;
pub mod game_analysis;
pub mod llm;
pub mod models;
pub mod recommendation;
pub mod scoring;
pub mod state;
pub mod steam;
pub mod sync_task;

use state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            let db_path = app_data_dir.join("mpgs.sqlite3");
            let db = db::open_database(&db_path)?;
            db::mark_running_discovery_runs_interrupted(&db)?;
            db::mark_running_classic_discovery_runs_interrupted(&db)?;
            let http = reqwest::Client::builder()
                .user_agent("MPGS/0.1 (+https://local.app)")
                .build()?;

            app.manage(AppState {
                db: std::sync::Mutex::new(db),
                http,
                discovery: std::sync::Mutex::new(discovery_task::DiscoveryRuntimeState::default()),
                classic_discovery: std::sync::Mutex::new(
                    classic_discovery_task::ClassicDiscoveryRuntimeState::default(),
                ),
                backfill: std::sync::Mutex::new(backfill_task::BackfillRuntimeState::default()),
                sync: std::sync::Mutex::new(sync_task::SyncRuntimeState::default()),
                ai_batch_refresh: std::sync::Mutex::new(
                    ai_batch_refresh_task::AiBatchRefreshRuntimeState::default(),
                ),
                auto_scheduler: std::sync::Mutex::new(state::AutoSchedulerRuntimeState::default()),
            });
            auto_scheduler::kick(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_dashboard,
            commands::save_config,
            commands::sync_seed_games,
            commands::discover_steam_games,
            commands::assess_game_with_ai,
            commands::get_game_analysis,
            commands::generate_game_analysis,
            commands::refresh_all_game_analyses,
            commands::preview_steam_app_list,
            commands::set_game_user_state,
            commands::get_user_collections,
            commands::get_discovery_task_snapshot,
            commands::list_discovery_task_history,
            commands::start_discovery_task,
            commands::pause_discovery_task,
            commands::resume_discovery_task,
            commands::cancel_discovery_task,
            commands::get_classic_discovery_task_snapshot,
            commands::list_classic_discovery_task_history,
            commands::start_classic_discovery_task,
            commands::retry_ai_analysis_job,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
