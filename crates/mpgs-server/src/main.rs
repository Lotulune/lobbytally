use anyhow::Result;
use mpgs_server::{build_router_with_state, db, AppState, DatabaseHealth, StartupConfig};

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().any(|arg| arg == "--export-openapi") {
        println!(
            "{}",
            serde_json::to_string_pretty(&mpgs_server::build_openapi())?
        );
        return Ok(());
    }

    let startup_config = StartupConfig::from_env()?;
    let (bind_addr, app) = match startup_config {
        StartupConfig::Ready(config) => {
            let pool = db::connect_and_migrate(&config.database_url).await?;
            let public_catalog_status = db::public_catalog_status(&pool).await?;
            (
                config.bind_addr,
                build_router_with_state(AppState::new_with_config_health(
                    config
                        .service_info
                        .service_info_with_catalog_status(public_catalog_status),
                    DatabaseHealth::Pool(pool),
                    config.config_health,
                )),
            )
        }
        StartupConfig::SafeMode {
            bind_addr,
            service_info,
        } => (
            bind_addr,
            build_router_with_state(AppState::safe_mode(service_info)),
        ),
    };
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;

    axum::serve(listener, app).await?;
    Ok(())
}
