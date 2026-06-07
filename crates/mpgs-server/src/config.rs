use std::collections::HashMap;
use std::fs;
use std::net::{AddrParseError, SocketAddr};
use std::path::{Path, PathBuf};

use crate::ServiceInfoConfig;
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_addr: SocketAddr,
    pub database_url: String,
    pub service_info: ServiceInfoConfig,
    pub config_health: ConfigHealth,
}

#[derive(Debug, Clone)]
pub enum StartupConfig {
    Ready(ServerConfig),
    SafeMode {
        bind_addr: SocketAddr,
        service_info: ServiceInfoConfig,
    },
}

#[derive(Debug, Clone)]
pub enum ConfigHealth {
    #[doc(hidden)]
    HealthyForTest,
    ActiveFiles {
        service_path: PathBuf,
        secrets_path: PathBuf,
    },
}

impl ConfigHealth {
    pub fn active_files(service_path: PathBuf, secrets_path: PathBuf) -> Self {
        Self::ActiveFiles {
            service_path,
            secrets_path,
        }
    }

    pub fn is_healthy(&self) -> bool {
        match self {
            Self::HealthyForTest => true,
            Self::ActiveFiles {
                service_path,
                secrets_path,
            } => {
                fs::read_to_string(service_path).is_ok() && fs::read_to_string(secrets_path).is_ok()
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Missing required environment variable {0}")]
    MissingRequiredEnv(&'static str),
    #[error("MPGS_SERVER_BIND must be a valid socket address: {0}")]
    InvalidBindAddr(AddrParseError),
    #[error("Active config file is not readable at {path:?}: {source}")]
    UnreadableActiveConfig {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Active config TOML is invalid at {path:?}: {source}")]
    InvalidActiveConfigToml {
        path: PathBuf,
        source: toml::de::Error,
    },
}

impl ServerConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_env_vars(std::env::vars())
    }

    pub fn from_env_vars(
        vars: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ConfigError> {
        let vars: HashMap<String, String> = vars.into_iter().collect();
        if let Some(config_dir) = vars.get("MPGS_CONFIG_DIR") {
            return Self::from_config_dir(config_dir);
        }

        let bind_addr = vars
            .get("MPGS_SERVER_BIND")
            .map(String::as_str)
            .unwrap_or("127.0.0.1:4310")
            .parse()
            .map_err(ConfigError::InvalidBindAddr)?;

        let database_url = vars
            .get("MPGS_DATABASE_URL")
            .cloned()
            .ok_or(ConfigError::MissingRequiredEnv("MPGS_DATABASE_URL"))?;

        let service_info = ServiceInfoConfig {
            service_instance_id: vars
                .get("MPGS_SERVICE_INSTANCE_ID")
                .cloned()
                .unwrap_or_else(|| "018fb770-8998-7699-a6e4-b7b59f2f9c01".to_string()),
            service_name: vars
                .get("MPGS_SERVICE_NAME")
                .cloned()
                .unwrap_or_else(|| "MPGS Public Discovery Service".to_string()),
            service_version: vars
                .get("MPGS_SERVICE_VERSION")
                .cloned()
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
        };

        Ok(Self {
            bind_addr,
            database_url,
            service_info,
            config_health: ConfigHealth::HealthyForTest,
        })
    }

    pub fn from_config_dir(config_dir: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let active_dir = config_dir.as_ref().join("active");
        let service_path = active_dir.join("service.toml");
        let secrets_path = active_dir.join("secrets.toml");

        let service_config: ActiveServiceConfig = read_active_toml(&service_path)?;
        let secrets_config: ActiveSecretsConfig = read_active_toml(&secrets_path)?;

        let bind_addr = service_config
            .bind_addr
            .as_deref()
            .unwrap_or("127.0.0.1:4310")
            .parse()
            .map_err(ConfigError::InvalidBindAddr)?;

        let service_info = ServiceInfoConfig {
            service_instance_id: service_config.service_identity.instance_id,
            service_name: service_config.service_identity.name,
            service_version: service_config
                .service_identity
                .version
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
        };

        Ok(Self {
            bind_addr,
            database_url: secrets_config.database.url,
            service_info,
            config_health: ConfigHealth::active_files(service_path, secrets_path),
        })
    }
}

impl StartupConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_env_vars(std::env::vars())
    }

    pub fn from_env_vars(
        vars: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ConfigError> {
        let vars: HashMap<String, String> = vars.into_iter().collect();
        if vars.contains_key("MPGS_CONFIG_DIR") {
            return match ServerConfig::from_env_vars(vars.clone()) {
                Ok(config) => Ok(Self::Ready(config)),
                Err(ConfigError::UnreadableActiveConfig { .. })
                | Err(ConfigError::InvalidActiveConfigToml { .. }) => {
                    Ok(Self::safe_mode_from_env_vars(&vars)?)
                }
                Err(error) => Err(error),
            };
        }

        ServerConfig::from_env_vars(vars).map(Self::Ready)
    }

    fn safe_mode_from_env_vars(vars: &HashMap<String, String>) -> Result<Self, ConfigError> {
        let bind_addr = vars
            .get("MPGS_SERVER_BIND")
            .map(String::as_str)
            .unwrap_or("127.0.0.1:4310")
            .parse()
            .map_err(ConfigError::InvalidBindAddr)?;

        let service_info = ServiceInfoConfig {
            service_instance_id: vars
                .get("MPGS_SERVICE_INSTANCE_ID")
                .cloned()
                .unwrap_or_else(|| "018fb770-8998-7699-a6e4-b7b59f2f9c01".to_string()),
            service_name: vars
                .get("MPGS_SERVICE_NAME")
                .cloned()
                .unwrap_or_else(|| "MPGS Public Discovery Service".to_string()),
            service_version: vars
                .get("MPGS_SERVICE_VERSION")
                .cloned()
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
        };

        Ok(Self::SafeMode {
            bind_addr,
            service_info,
        })
    }
}

fn read_active_toml<T>(path: &Path) -> Result<T, ConfigError>
where
    T: for<'de> Deserialize<'de>,
{
    let contents =
        fs::read_to_string(path).map_err(|source| ConfigError::UnreadableActiveConfig {
            path: path.to_path_buf(),
            source,
        })?;

    toml::from_str(&contents).map_err(|source| ConfigError::InvalidActiveConfigToml {
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Debug, Deserialize)]
struct ActiveServiceConfig {
    bind_addr: Option<String>,
    service_identity: ActiveServiceIdentityConfig,
}

#[derive(Debug, Deserialize)]
struct ActiveServiceIdentityConfig {
    instance_id: String,
    name: String,
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ActiveSecretsConfig {
    database: ActiveDatabaseConfig,
}

#[derive(Debug, Deserialize)]
struct ActiveDatabaseConfig {
    url: String,
}
