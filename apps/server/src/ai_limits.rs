//! Per-account AI admission control.
//!
//! Daily usage and concurrency leases are persisted by storage so admission
//! remains shared across Vercel instances. Leases expire after a crashed request.

use std::{env, io};

#[cfg(test)]
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use mpgs_storage::{Repository, StorageError, StorageResult};
use sha2::{Digest, Sha256};

const DEFAULT_DAILY_BUDGET: u32 = 50;
const DEFAULT_MAX_CONCURRENT: usize = 2;
const DEFAULT_GLOBAL_MAX_CONCURRENT: usize = 20;
// Every request has a 60-second deadline. This exceeds the whole request, not
// just the model timeout, while still recovering promptly from crashed instances.
const LEASE_TTL_SECONDS: i64 = 120;

#[derive(Clone)]
pub struct AccountAiLimiter {
    daily_budget: u32,
    max_concurrent: usize,
    global_max_concurrent: usize,
    #[cfg(test)]
    active: Arc<Mutex<HashMap<String, usize>>>,
}

impl Default for AccountAiLimiter {
    fn default() -> Self {
        Self {
            daily_budget: DEFAULT_DAILY_BUDGET,
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            global_max_concurrent: DEFAULT_GLOBAL_MAX_CONCURRENT,
            #[cfg(test)]
            active: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl AccountAiLimiter {
    pub fn from_env() -> Result<Self, io::Error> {
        Ok(Self {
            daily_budget: positive_u32("MPGS_AI_ACCOUNT_DAILY_BUDGET", DEFAULT_DAILY_BUDGET)?,
            max_concurrent: positive_usize(
                "MPGS_AI_ACCOUNT_MAX_CONCURRENT",
                DEFAULT_MAX_CONCURRENT,
            )?,
            global_max_concurrent: positive_usize(
                "MPGS_AI_GLOBAL_MAX_CONCURRENT",
                DEFAULT_GLOBAL_MAX_CONCURRENT,
            )?,
            #[cfg(test)]
            active: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub const fn daily_budget(&self) -> u32 {
        self.daily_budget
    }

    pub async fn try_acquire_shared(
        &self,
        repo: Option<&Repository>,
        user_id: &str,
    ) -> StorageResult<Option<AccountAiPermit>> {
        let Some(repo) = repo else {
            #[cfg(test)]
            {
                return Ok(self.try_acquire(user_id));
            }
            #[cfg(not(test))]
            {
                return Err(StorageError::Io(io::Error::other(
                    "shared AI admission requires a database",
                )));
            }
        };
        let repo = repo.clone();
        let scope = format!("ai:{:x}", Sha256::digest(user_id.as_bytes()));
        let scope_limit = self.max_concurrent;
        let global_limit = self.global_max_concurrent;
        tokio::task::spawn_blocking(move || {
            let lease_id =
                repo.acquire_ai_lease(&scope, scope_limit, global_limit, LEASE_TTL_SECONDS)?;
            // Construct the permit inside the blocking task so cancellation
            // during acquisition still releases a newly acquired lease.
            Ok(lease_id.map(|lease_id| AccountAiPermit {
                lease: Some(PermitLease::Shared { repo, lease_id }),
            }))
        })
        .await
        .map_err(|_| StorageError::Io(io::Error::other("AI admission task failed")))?
    }

    #[cfg(test)]
    fn try_acquire(&self, user_id: &str) -> Option<AccountAiPermit> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if active.values().sum::<usize>() >= self.global_max_concurrent {
            return None;
        }
        let current = active.entry(user_id.to_owned()).or_insert(0);
        if *current >= self.max_concurrent {
            return None;
        }
        *current += 1;
        Some(AccountAiPermit {
            lease: Some(PermitLease::Local {
                user_id: user_id.to_owned(),
                active: Arc::clone(&self.active),
            }),
        })
    }
}

pub struct AccountAiPermit {
    lease: Option<PermitLease>,
}

enum PermitLease {
    Shared {
        repo: Repository,
        lease_id: String,
    },
    #[cfg(test)]
    Local {
        user_id: String,
        active: Arc<Mutex<HashMap<String, usize>>>,
    },
}

impl Drop for AccountAiPermit {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        match lease {
            PermitLease::Shared { repo, lease_id } => {
                // Synchronous Postgres owns a runtime and cannot be called on a
                // Tokio worker. Join the release thread before returning: a
                // detached release could be frozen after a serverless response.
                // Storage bounds both pool checkout and the release statement.
                let released = std::thread::Builder::new()
                    .name("ai-lease-release".to_owned())
                    .spawn(move || repo.release_ai_lease(&lease_id))
                    .ok()
                    .and_then(|thread| thread.join().ok())
                    .is_some_and(|result| result.is_ok());
                if !released {
                    tracing::warn!("AI concurrency lease release failed; lease will expire");
                }
            }
            #[cfg(test)]
            PermitLease::Local { user_id, active } => {
                let mut active = active.lock().unwrap_or_else(|poison| poison.into_inner());
                let Some(current) = active.get_mut(&user_id) else {
                    return;
                };
                *current = current.saturating_sub(1);
                if *current == 0 {
                    active.remove(&user_id);
                }
            }
        }
    }
}

fn positive_u32(name: &str, default: u32) -> Result<u32, io::Error> {
    let Ok(value) = env::var(name) else {
        return Ok(default);
    };
    value
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} must be a positive integer"),
            )
        })
}

fn positive_usize(name: &str, default: usize) -> Result<usize, io::Error> {
    let Ok(value) = env::var(name) else {
        return Ok(default);
    };
    value
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} must be a positive integer"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_concurrent_requests_per_account_and_releases_on_drop() {
        let limiter = AccountAiLimiter {
            daily_budget: 1,
            max_concurrent: 1,
            global_max_concurrent: 2,
            active: Arc::new(Mutex::new(HashMap::new())),
        };
        let permit = limiter.try_acquire("u_1").expect("first request allowed");
        assert!(limiter.try_acquire("u_1").is_none());
        assert!(limiter.try_acquire("u_2").is_some());
        drop(permit);
        assert!(limiter.try_acquire("u_1").is_some());
    }

    #[test]
    fn global_concurrency_applies_across_accounts_and_releases_on_drop() {
        let limiter = AccountAiLimiter {
            global_max_concurrent: 1,
            ..AccountAiLimiter::default()
        };
        let permit = limiter.try_acquire("u_1").expect("first request allowed");
        assert!(limiter.try_acquire("u_2").is_none());
        drop(permit);
        assert!(limiter.try_acquire("u_2").is_some());
    }
}
