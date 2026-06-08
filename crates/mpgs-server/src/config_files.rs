use crate::admin::hash_token;
use mpgs_core::models::ServiceCapability;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use utoipa::ToSchema;

#[derive(Debug, Clone)]
pub struct ConfigFileManager {
    config_dir: PathBuf,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PendingServiceIdentityRequest {
    pub service_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfigStateResponse {
    pub active_config_version: String,
    pub pending_config_version: Option<String>,
    pub restart_required: bool,
    pub last_startup_status: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PendingConfigResponse {
    pub pending_config_version: String,
    pub restart_required: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConnectionFileResponse {
    pub service_name: String,
    pub service_instance_id: String,
    pub api_version: String,
    pub base_url: String,
    pub service_info_url: String,
    pub capabilities: Vec<ServiceCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDeploymentDiagnostics {
    pub public_base_url: Option<String>,
    pub public_base_url_status: String,
    pub https_status: String,
    pub public_cors: String,
    pub restart_policy: String,
    pub steam: String,
    pub llm: String,
    pub r2: String,
}

impl Default for ConfigDeploymentDiagnostics {
    fn default() -> Self {
        Self {
            public_base_url: None,
            public_base_url_status: "missing".to_string(),
            https_status: "unknown".to_string(),
            public_cors: "disabled".to_string(),
            restart_policy: "external_required".to_string(),
            steam: "unknown".to_string(),
            llm: "unknown".to_string(),
            r2: "unknown".to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ActiveServiceConfig {
    bind_addr: Option<String>,
    service_identity: ActiveServiceIdentityConfig,
    service_connection: Option<ActiveServiceConnectionConfig>,
    public_cors: Option<ActivePublicCorsConfig>,
    deployment: Option<ActiveDeploymentConfig>,
}

#[derive(Debug, Deserialize)]
struct ActiveServiceIdentityConfig {
    instance_id: String,
    name: String,
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ActiveServiceConnectionConfig {
    public_base_url: String,
}

#[derive(Debug, Deserialize)]
struct ActivePublicCorsConfig {
    allow_any_origin: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ActiveDeploymentConfig {
    restart_policy: Option<String>,
}

impl ConfigFileManager {
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: config_dir.into(),
        }
    }

    pub fn state(&self) -> io::Result<ConfigStateResponse> {
        Ok(ConfigStateResponse {
            active_config_version: self.active_config_version()?,
            pending_config_version: self.pending_config_version()?,
            restart_required: pending_service_path(&self.config_dir).is_file(),
            last_startup_status: "ok".to_string(),
        })
    }

    pub fn write_pending_service_identity(
        &self,
        request: &PendingServiceIdentityRequest,
    ) -> io::Result<PendingConfigResponse> {
        let active_service = read_service_config(&active_service_path(&self.config_dir))?;
        let pending_dir = self.config_dir.join("pending");
        fs::create_dir_all(&pending_dir)?;

        let service_toml = format!(
            r#"bind_addr = "{bind_addr}"

[service_identity]
instance_id = "{instance_id}"
name = "{service_name}"
version = "{version}"
"#,
            bind_addr = escape_toml_string(
                active_service
                    .bind_addr
                    .as_deref()
                    .unwrap_or("0.0.0.0:4310")
            ),
            instance_id = escape_toml_string(&active_service.service_identity.instance_id),
            service_name = escape_toml_string(&request.service_name),
            version = escape_toml_string(
                active_service
                    .service_identity
                    .version
                    .as_deref()
                    .unwrap_or(env!("CARGO_PKG_VERSION"))
            )
        );
        let service_toml =
            if let Some(service_connection) = active_service.service_connection.as_ref() {
                format!(
                    r#"{service_toml}
[service_connection]
public_base_url = "{public_base_url}"
"#,
                    public_base_url = escape_toml_string(normalize_public_base_url(
                        &service_connection.public_base_url
                    ))
                )
            } else {
                service_toml
            };
        let service_toml = if let Some(public_cors) = active_service.public_cors.as_ref() {
            format!(
                r#"{service_toml}
[public_cors]
allow_any_origin = {allow_any_origin}
"#,
                allow_any_origin = public_cors.allow_any_origin.unwrap_or(false)
            )
        } else {
            service_toml
        };
        let service_toml = if let Some(deployment) = active_service.deployment.as_ref() {
            if let Some(restart_policy) = deployment.restart_policy.as_deref() {
                format!(
                    r#"{service_toml}
[deployment]
restart_policy = "{restart_policy}"
"#,
                    restart_policy = escape_toml_string(restart_policy)
                )
            } else {
                service_toml
            }
        } else {
            service_toml
        };
        atomic_write(&pending_service_path(&self.config_dir), &service_toml)?;

        Ok(PendingConfigResponse {
            pending_config_version: hash_token(&service_toml),
            restart_required: true,
        })
    }

    pub fn service_connection_file(&self) -> io::Result<ServiceConnectionFileResponse> {
        let active_service = read_service_config(&active_service_path(&self.config_dir))?;
        let Some(service_connection) = active_service.service_connection.as_ref() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "service_connection.public_base_url is not configured",
            ));
        };
        let base_url = normalize_public_base_url(&service_connection.public_base_url).to_string();

        Ok(ServiceConnectionFileResponse {
            service_name: active_service.service_identity.name,
            service_instance_id: active_service.service_identity.instance_id,
            api_version: "v1".to_string(),
            service_info_url: format!("{base_url}/api/v1/service-info"),
            base_url,
            capabilities: vec![ServiceCapability::PublicCatalogRead],
        })
    }

    pub fn deployment_diagnostics(&self) -> ConfigDeploymentDiagnostics {
        let Ok(active_service) = read_service_config(&active_service_path(&self.config_dir)) else {
            return ConfigDeploymentDiagnostics::default();
        };

        let public_base_url = active_service
            .service_connection
            .as_ref()
            .map(|connection| normalize_public_base_url(&connection.public_base_url).to_string());
        let secrets = read_secrets_toml(&active_secrets_path(&self.config_dir)).ok();

        ConfigDeploymentDiagnostics {
            public_base_url_status: public_base_url_status(public_base_url.as_deref()),
            https_status: https_status(public_base_url.as_deref()),
            public_cors: if active_service
                .public_cors
                .as_ref()
                .and_then(|cors| cors.allow_any_origin)
                .unwrap_or(false)
            {
                "allow_any_origin".to_string()
            } else {
                "disabled".to_string()
            },
            restart_policy: active_service
                .deployment
                .and_then(|deployment| deployment.restart_policy)
                .filter(|restart_policy| !restart_policy.trim().is_empty())
                .unwrap_or_else(|| "external_required".to_string()),
            steam: provider_status(secrets.as_ref(), "steam", &["api_key"]),
            llm: provider_status(secrets.as_ref(), "llm", &["api_key"]),
            r2: provider_status(
                secrets.as_ref(),
                "r2",
                &["access_key_id", "secret_access_key", "bucket"],
            ),
            public_base_url,
        }
    }

    pub fn validate_pending_service_config(&self) -> io::Result<bool> {
        let path = pending_service_path(&self.config_dir);
        if !path.is_file() {
            return Ok(false);
        }

        read_service_config(&path).map(|_| true)
    }

    pub fn active_config_version(&self) -> io::Result<String> {
        let service_toml = fs::read_to_string(active_service_path(&self.config_dir))?;
        let secrets_toml = fs::read_to_string(active_secrets_path(&self.config_dir))?;
        Ok(hash_token(&format!("{service_toml}\n{secrets_toml}")))
    }

    fn pending_config_version(&self) -> io::Result<Option<String>> {
        let path = pending_service_path(&self.config_dir);
        if !path.is_file() {
            return Ok(None);
        }

        let service_toml = fs::read_to_string(path)?;
        Ok(Some(hash_token(&service_toml)))
    }
}

fn read_service_config(path: &Path) -> io::Result<ActiveServiceConfig> {
    let contents = fs::read_to_string(path)?;
    toml::from_str(&contents).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn read_secrets_toml(path: &Path) -> io::Result<toml::Value> {
    let contents = fs::read_to_string(path)?;
    toml::from_str(&contents).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn active_service_path(config_dir: &Path) -> PathBuf {
    config_dir.join("active").join("service.toml")
}

fn active_secrets_path(config_dir: &Path) -> PathBuf {
    config_dir.join("active").join("secrets.toml")
}

fn pending_service_path(config_dir: &Path) -> PathBuf {
    config_dir.join("pending").join("service.toml")
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

fn public_base_url_status(base_url: Option<&str>) -> String {
    match base_url {
        Some(value) if !value.trim().is_empty() => "configured".to_string(),
        _ => "missing".to_string(),
    }
}

fn https_status(base_url: Option<&str>) -> String {
    let Some(base_url) = base_url else {
        return "unknown".to_string();
    };
    let value = base_url.trim().to_ascii_lowercase();
    if value.starts_with("https://") {
        "ok".to_string()
    } else if value.starts_with("http://localhost")
        || value.starts_with("http://127.0.0.1")
        || value.starts_with("http://[::1]")
    {
        "local_http_allowed".to_string()
    } else if value.starts_with("http://") {
        "public_http_rejected_by_clients".to_string()
    } else {
        "invalid_url".to_string()
    }
}

fn provider_status(secrets: Option<&toml::Value>, section: &str, keys: &[&str]) -> String {
    let Some(table) = secrets
        .and_then(|value| value.get(section))
        .and_then(toml::Value::as_table)
    else {
        return "missing".to_string();
    };

    if keys.iter().any(|key| {
        table
            .get(*key)
            .and_then(toml::Value::as_str)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    }) {
        "configured".to_string()
    } else {
        "missing".to_string()
    }
}
