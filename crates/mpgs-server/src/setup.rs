use crate::admin::{hash_token, verify_token_hash};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SetupAccess {
    config_dir: PathBuf,
    setup_token_hash: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupCompleteRequest {
    pub setup_token: String,
    pub service_name: String,
    pub public_base_url: String,
    pub database_url: String,
    pub admin_token: String,
    pub steam_api_key: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupCompleteResponse {
    pub configured: bool,
    pub restart_required: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatusResponse {
    pub configured: bool,
}

impl SetupAccess {
    pub fn new(config_dir: impl Into<PathBuf>, setup_token_hash: String) -> Self {
        Self {
            config_dir: config_dir.into(),
            setup_token_hash,
        }
    }

    #[doc(hidden)]
    pub fn for_test_token(config_dir: impl Into<PathBuf>, setup_token: &str) -> Self {
        Self::new(config_dir, hash_token(setup_token))
    }

    pub fn verify_token(&self, token: &str) -> bool {
        verify_token_hash(&self.setup_token_hash, token)
    }

    pub fn is_configured(&self) -> bool {
        active_service_path(&self.config_dir).is_file()
            && active_secrets_path(&self.config_dir).is_file()
    }

    pub fn complete(&self, request: &SetupCompleteRequest) -> io::Result<()> {
        let active_dir = self.config_dir.join("active");
        fs::create_dir_all(&active_dir)?;

        let instance_id = Uuid::now_v7();
        let session_secret = Uuid::now_v7().to_string();
        let service_toml = format!(
            r#"bind_addr = "0.0.0.0:4310"

[service_identity]
instance_id = "{instance_id}"
name = "{service_name}"
version = "{version}"

[service_connection]
public_base_url = "{public_base_url}"

[public_cors]
allow_any_origin = true

[deployment]
restart_policy = "compose:unless-stopped"
"#,
            service_name = escape_toml_string(&request.service_name),
            public_base_url =
                escape_toml_string(normalize_public_base_url(&request.public_base_url)),
            version = env!("CARGO_PKG_VERSION")
        );
        let secrets_toml = format!(
            r#"[database]
url = "{database_url}"

[admin]
token_hash = "{admin_token_hash}"
session_secret = "{session_secret}"

[steam]
api_key = "{steam_api_key}"
"#,
            database_url = escape_toml_string(&request.database_url),
            admin_token_hash = hash_token(&request.admin_token),
            session_secret = session_secret,
            steam_api_key = escape_toml_string(&request.steam_api_key)
        );

        atomic_write(&active_service_path(&self.config_dir), &service_toml)?;
        atomic_write(&active_secrets_path(&self.config_dir), &secrets_toml)
    }
}

fn active_service_path(config_dir: &Path) -> PathBuf {
    config_dir.join("active").join("service.toml")
}

fn active_secrets_path(config_dir: &Path) -> PathBuf {
    config_dir.join("active").join("secrets.toml")
}

fn atomic_write(path: &Path, contents: &str) -> io::Result<()> {
    let temp_path = path.with_extension("tmp");
    fs::write(&temp_path, contents)?;
    fs::rename(temp_path, path)
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn normalize_public_base_url(value: &str) -> &str {
    value.trim().trim_end_matches('/')
}
