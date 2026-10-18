//! Disposable local PostgreSQL fixtures for native desktop acceptance tests.

use std::env;

use mpgs_storage::{Database, Repository};

pub fn run_command(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let action = args.next().ok_or("e2e-fixture requires create or drop")?;
    let name = args
        .next()
        .ok_or("e2e-fixture requires a generated database name")?;
    if args.next().is_some() || !matches!(action.as_str(), "create" | "drop") {
        return Err("usage: e2e-fixture <create|drop> <mpgs_e2e_32-hex-digits>".into());
    }
    validate_name(&name)?;
    let admin_url = env::var("MPGS_TEST_DATABASE_URL")
        .map_err(|_| "MPGS_TEST_DATABASE_URL is required for E2E fixtures".to_owned())?;
    let mut fixture_url = validate_local_url(&admin_url)?;
    fixture_url.set_path(&format!("/{name}"));
    let admin = Database::open(&admin_url).map_err(display)?;
    if action == "drop" {
        drop_fixture(&admin, &name)?;
        println!("fixture_dropped={name}");
        return Ok(());
    }
    admin
        .with_conn(|conn| {
            conn.execute_batch(&format!("CREATE DATABASE \"{name}\""))?;
            Ok(())
        })
        .map_err(display)?;
    let seeded = (|| {
        let repo = Repository::new(Database::open(fixture_url.as_str()).map_err(display)?);
        repo.migrate().map_err(display)?;
        repo.ensure_runtime_defaults().map_err(display)?;
        repo.seed_demo_if_empty().map_err(display)?;
        repo.readiness_check().map_err(display)
    })();
    if let Err(error) = seeded {
        if let Err(cleanup) = drop_fixture(&admin, &name) {
            return Err(format!(
                "fixture setup failed: {error}; cleanup failed: {cleanup}"
            ));
        }
        return Err(error);
    }
    println!("fixture_created={name}");
    Ok(())
}

fn validate_name(name: &str) -> Result<(), String> {
    let suffix = name.strip_prefix("mpgs_e2e_").unwrap_or("");
    if suffix.len() != 32 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(
            "fixture database name must be mpgs_e2e_ followed by 32 hexadecimal digits".into(),
        );
    }
    Ok(())
}

fn validate_local_url(value: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| "MPGS_TEST_DATABASE_URL must be a local PostgreSQL URL".to_owned())?;
    if !matches!(url.scheme(), "postgres" | "postgresql")
        || !matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
        )
    {
        return Err("E2E fixture creation and deletion require loopback PostgreSQL".into());
    }
    Ok(url)
}

fn drop_fixture(admin: &Database, name: &str) -> Result<(), String> {
    validate_name(name)?;
    admin
        .with_conn(|conn| {
            conn.execute_batch(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))?;
            Ok(())
        })
        .map_err(display)
}

fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_destruction_rejects_existing_names_and_sql_tokens() {
        for name in [
            "postgres",
            "mpgs",
            "mpgs_e2e_short",
            "mpgs_e2e_\"; DROP DATABASE postgres; --",
        ] {
            assert!(validate_name(name).is_err());
        }
        assert!(validate_name("mpgs_e2e_0123456789abcdef0123456789abcdef").is_ok());
    }

    #[test]
    fn fixture_commands_reject_remote_databases_without_exposing_credentials() {
        let error =
            validate_local_url("postgresql://postgres:secret@db.example.com/postgres").unwrap_err();
        assert!(!error.contains("secret"));
        assert!(validate_local_url("postgresql://postgres:secret@127.0.0.1/postgres").is_ok());
    }
}
