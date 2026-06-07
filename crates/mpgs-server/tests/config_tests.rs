use std::collections::HashMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use mpgs_server::{admin::hash_token, ConfigError, ServerConfig, StartupConfig};

fn env(values: &[(&str, &str)]) -> HashMap<String, String> {
    values
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[test]
fn server_config_loads_active_toml_files_from_config_dir() {
    let temp_dir = tempfile::tempdir().unwrap();
    let active_dir = temp_dir.path().join("active");
    fs::create_dir(&active_dir).unwrap();
    fs::write(
        active_dir.join("service.toml"),
        r#"
bind_addr = "0.0.0.0:4310"

[service_identity]
instance_id = "018fb770-8998-7699-a6e4-b7b59f2f9c01"
name = "MPGS TOML Service"
version = "2.0.0"
"#,
    )
    .unwrap();
    fs::write(
        active_dir.join("secrets.toml"),
        r#"
[database]
url = "postgres://mpgs:secret@postgres:5432/mpgs"

[admin]
token_hash = "sha256:test-hash"
session_secret = "test-session-secret"
"#,
    )
    .unwrap();

    let config = ServerConfig::from_env_vars(env(&[(
        "MPGS_CONFIG_DIR",
        temp_dir.path().to_str().unwrap(),
    )]))
    .unwrap();

    assert_eq!(
        config.bind_addr,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 4310)
    );
    assert_eq!(
        config.database_url,
        "postgres://mpgs:secret@postgres:5432/mpgs"
    );
    assert_eq!(config.service_info.service_name, "MPGS TOML Service");
    assert_eq!(config.service_info.service_version, "2.0.0");
    assert!(config.admin_auth.is_some());
}

#[test]
fn server_config_promotes_valid_pending_service_config_before_startup() {
    let temp_dir = tempfile::tempdir().unwrap();
    let active_dir = temp_dir.path().join("active");
    let pending_dir = temp_dir.path().join("pending");
    fs::create_dir(&active_dir).unwrap();
    fs::create_dir(&pending_dir).unwrap();
    fs::write(
        active_dir.join("service.toml"),
        r#"
bind_addr = "0.0.0.0:4310"

[service_identity]
instance_id = "018fb770-8998-7699-a6e4-b7b59f2f9c01"
name = "MPGS Active Service"
version = "0.1.0"
"#,
    )
    .unwrap();
    fs::write(
        active_dir.join("secrets.toml"),
        r#"
[database]
url = "postgres://mpgs:secret@postgres:5432/mpgs"

[admin]
token_hash = "sha256:test-hash"
session_secret = "test-session-secret"
"#,
    )
    .unwrap();
    fs::write(
        pending_dir.join("service.toml"),
        r#"
bind_addr = "0.0.0.0:4310"

[service_identity]
instance_id = "018fb770-8998-7699-a6e4-b7b59f2f9c01"
name = "MPGS Pending Service"
version = "0.1.0"
"#,
    )
    .unwrap();

    let config = ServerConfig::from_config_dir(temp_dir.path()).unwrap();

    assert_eq!(config.service_info.service_name, "MPGS Pending Service");
    assert!(fs::read_to_string(active_dir.join("service.toml"))
        .unwrap()
        .contains("MPGS Pending Service"));
    assert!(!pending_dir.join("service.toml").exists());
}

#[test]
fn server_config_rejects_invalid_pending_service_config_without_overwriting_active() {
    let temp_dir = tempfile::tempdir().unwrap();
    let active_dir = temp_dir.path().join("active");
    let pending_dir = temp_dir.path().join("pending");
    fs::create_dir(&active_dir).unwrap();
    fs::create_dir(&pending_dir).unwrap();
    fs::write(
        active_dir.join("service.toml"),
        r#"
bind_addr = "0.0.0.0:4310"

[service_identity]
instance_id = "018fb770-8998-7699-a6e4-b7b59f2f9c01"
name = "MPGS Active Service"
version = "0.1.0"
"#,
    )
    .unwrap();
    fs::write(
        active_dir.join("secrets.toml"),
        r#"
[database]
url = "postgres://mpgs:secret@postgres:5432/mpgs"

[admin]
token_hash = "sha256:test-hash"
session_secret = "test-session-secret"
"#,
    )
    .unwrap();
    fs::write(pending_dir.join("service.toml"), "not = valid = toml").unwrap();

    let error = ServerConfig::from_config_dir(temp_dir.path())
        .expect_err("invalid pending service config should be rejected before startup");

    assert!(matches!(error, ConfigError::InvalidActiveConfigToml { .. }));
    assert!(fs::read_to_string(active_dir.join("service.toml"))
        .unwrap()
        .contains("MPGS Active Service"));
    assert!(pending_dir.join("service.toml").exists());
}

#[test]
fn server_config_loads_required_database_and_public_identity_env() {
    let config = ServerConfig::from_env_vars(env(&[
        ("MPGS_SERVER_BIND", "0.0.0.0:4310"),
        (
            "MPGS_DATABASE_URL",
            "postgres://mpgs:secret@localhost:5432/mpgs",
        ),
        (
            "MPGS_SERVICE_INSTANCE_ID",
            "018fb770-8998-7699-a6e4-b7b59f2f9c01",
        ),
        ("MPGS_SERVICE_NAME", "MPGS Test Service"),
        ("MPGS_SERVICE_VERSION", "9.8.7"),
    ]))
    .unwrap();

    assert_eq!(
        config.bind_addr,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 4310)
    );
    assert_eq!(
        config.database_url,
        "postgres://mpgs:secret@localhost:5432/mpgs"
    );
    assert_eq!(
        config.service_info.service_instance_id,
        "018fb770-8998-7699-a6e4-b7b59f2f9c01"
    );
    assert_eq!(config.service_info.service_name, "MPGS Test Service");
    assert_eq!(config.service_info.service_version, "9.8.7");
}

#[test]
fn server_config_requires_database_url_with_clear_error() {
    let error = ServerConfig::from_env_vars(env(&[("MPGS_SERVER_BIND", "127.0.0.1:4310")]))
        .expect_err("database URL should be required for server startup");

    assert!(matches!(
        error,
        ConfigError::MissingRequiredEnv("MPGS_DATABASE_URL")
    ));
    assert!(error.to_string().contains("MPGS_DATABASE_URL"));
}

#[test]
fn server_config_rejects_invalid_bind_address() {
    let error = ServerConfig::from_env_vars(env(&[
        ("MPGS_SERVER_BIND", "not-a-socket"),
        (
            "MPGS_DATABASE_URL",
            "postgres://mpgs:secret@localhost:5432/mpgs",
        ),
    ]))
    .expect_err("invalid bind address should be rejected during config load");

    assert!(matches!(error, ConfigError::InvalidBindAddr(_)));
    assert!(error.to_string().contains("MPGS_SERVER_BIND"));
}

#[test]
fn startup_config_enters_safe_mode_when_active_config_is_missing() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = StartupConfig::from_env_vars(env(&[(
        "MPGS_CONFIG_DIR",
        temp_dir.path().to_str().unwrap(),
    )]))
    .unwrap();

    let StartupConfig::SafeMode {
        bind_addr,
        service_info,
        ..
    } = config
    else {
        panic!("missing active config should enter safe mode");
    };

    assert_eq!(bind_addr, SocketAddr::from(([127, 0, 0, 1], 4310)));
    assert_eq!(service_info.service_name, "MPGS Public Discovery Service");
}

#[test]
fn startup_config_keeps_setup_access_available_in_safe_mode() {
    let temp_dir = tempfile::tempdir().unwrap();
    fs::write(
        temp_dir.path().join("setup.toml"),
        format!(
            r#"
[setup]
token_hash = "{}"
"#,
            hash_token("repair-setup-token")
        ),
    )
    .unwrap();

    let config = StartupConfig::from_env_vars(env(&[(
        "MPGS_CONFIG_DIR",
        temp_dir.path().to_str().unwrap(),
    )]))
    .unwrap();

    let StartupConfig::SafeMode { setup_access, .. } = config else {
        panic!("missing active config should enter safe mode");
    };

    assert!(setup_access
        .as_ref()
        .unwrap()
        .verify_token("repair-setup-token"));
}
