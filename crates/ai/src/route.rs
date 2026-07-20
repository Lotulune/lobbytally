//! Task-level model routing configuration.
//!
//! Model *names* in defaults are suggestions only. Production selection is
//! filtered by the live `/v1/models` registry and canary results (see
//! [`crate::model_registry`]). Routes can be overridden via environment without
//! hardcoding secrets.

use std::collections::HashMap;
use std::env;
use std::time::Duration;

use crate::error::AiError;
use crate::types::{AiTaskType, ApiProtocol, TaskRouteConfig};

/// Shared route table version. Bump when default policy changes.
pub const DEFAULT_ROUTE_VERSION: &str = "m8-route-v1";

/// Build the default multi-model route table from PRD suggestions.
///
/// Names are not permanent guarantees — callers must intersect with the
/// discovered model registry before issuing requests.
pub fn default_task_routes() -> HashMap<AiTaskType, TaskRouteConfig> {
    let version = DEFAULT_ROUTE_VERSION.to_owned();
    let mut routes = HashMap::new();

    routes.insert(
        AiTaskType::IntentParse,
        TaskRouteConfig {
            task: AiTaskType::IntentParse,
            primary_model: "grok-chat-fast".into(),
            fallback_models: vec!["grok-4.20-0309-non-reasoning".into()],
            protocol_preference: vec![ApiProtocol::ChatCompletions, ApiProtocol::Responses],
            timeout: Duration::from_secs(8),
            max_output_tokens: 512,
            enabled: true,
            route_version: version.clone(),
        },
    );

    routes.insert(
        AiTaskType::RankExplain,
        TaskRouteConfig {
            task: AiTaskType::RankExplain,
            primary_model: "grok-4.3".into(),
            fallback_models: vec!["grok-4.20-0309-non-reasoning".into()],
            // Prefer Responses when a model (e.g. grok-4.5) only works reliably there.
            protocol_preference: vec![ApiProtocol::Responses, ApiProtocol::ChatCompletions],
            timeout: Duration::from_secs(20),
            max_output_tokens: 1_800,
            enabled: true,
            route_version: version.clone(),
        },
    );

    routes.insert(
        AiTaskType::GameSummary,
        TaskRouteConfig {
            task: AiTaskType::GameSummary,
            primary_model: "grok-4.20-0309-non-reasoning".into(),
            fallback_models: vec!["grok-4.3".into()],
            protocol_preference: vec![ApiProtocol::ChatCompletions, ApiProtocol::Responses],
            timeout: Duration::from_secs(30),
            max_output_tokens: 2_000,
            enabled: true,
            route_version: version.clone(),
        },
    );

    routes.insert(
        AiTaskType::CompareGames,
        TaskRouteConfig {
            task: AiTaskType::CompareGames,
            primary_model: "grok-4.20-0309-reasoning".into(),
            fallback_models: vec!["grok-4.3".into()],
            protocol_preference: vec![ApiProtocol::ChatCompletions, ApiProtocol::Responses],
            timeout: Duration::from_secs(25),
            max_output_tokens: 2_400,
            enabled: true,
            route_version: version.clone(),
        },
    );

    routes.insert(
        AiTaskType::GroupAdvice,
        TaskRouteConfig {
            task: AiTaskType::GroupAdvice,
            primary_model: "grok-4.20-0309-reasoning".into(),
            fallback_models: vec!["grok-4.3".into()],
            protocol_preference: vec![ApiProtocol::ChatCompletions, ApiProtocol::Responses],
            timeout: Duration::from_secs(25),
            max_output_tokens: 2_000,
            enabled: true,
            route_version: version.clone(),
        },
    );

    routes.insert(
        AiTaskType::DataQuality,
        TaskRouteConfig {
            task: AiTaskType::DataQuality,
            primary_model: "grok-4.20-0309-non-reasoning".into(),
            fallback_models: vec![],
            protocol_preference: vec![ApiProtocol::ChatCompletions, ApiProtocol::Responses],
            timeout: Duration::from_secs(20),
            max_output_tokens: 1_200,
            enabled: true,
            route_version: version,
        },
    );

    routes
}

/// Load routes from defaults, then apply environment overrides.
///
/// Override format (optional):
/// - `MPGS_AI_ROUTE_<TASK>_MODEL` primary model
/// - `MPGS_AI_ROUTE_<TASK>_FALLBACKS` comma-separated fallbacks
/// - `MPGS_AI_ROUTE_<TASK>_TIMEOUT_SECS`
/// - `MPGS_AI_MODEL` sets a single-model override for every online task when
///   task-specific overrides are absent (compat with M5 single-model deploy).
pub fn task_routes_from_env() -> Result<HashMap<AiTaskType, TaskRouteConfig>, AiError> {
    let mut routes = default_task_routes();
    let global_model = env::var("MPGS_AI_MODEL")
        .ok()
        .filter(|s| !s.trim().is_empty());

    for (task, route) in routes.iter_mut() {
        let key = task_env_key(*task);
        if let Ok(primary) = env::var(format!("MPGS_AI_ROUTE_{key}_MODEL")) {
            let primary = primary.trim();
            if primary.is_empty() {
                return Err(AiError::Config(format!(
                    "MPGS_AI_ROUTE_{key}_MODEL must not be empty"
                )));
            }
            route.primary_model = primary.to_owned();
        } else if let Some(model) = &global_model {
            // Single-model deployments keep working without task-level config.
            if matches!(
                task,
                AiTaskType::IntentParse
                    | AiTaskType::RankExplain
                    | AiTaskType::CompareGames
                    | AiTaskType::GroupAdvice
            ) {
                route.primary_model = model.clone();
                route.fallback_models.clear();
            }
        }

        if let Ok(raw) = env::var(format!("MPGS_AI_ROUTE_{key}_FALLBACKS")) {
            route.fallback_models = raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
        }

        if let Ok(raw) = env::var(format!("MPGS_AI_ROUTE_{key}_TIMEOUT_SECS")) {
            let secs: u64 = raw.parse().map_err(|_| {
                AiError::Config(format!(
                    "MPGS_AI_ROUTE_{key}_TIMEOUT_SECS must be an integer"
                ))
            })?;
            route.timeout = Duration::from_secs(secs.clamp(1, 120));
        }

        if let Ok(raw) = env::var(format!("MPGS_AI_ROUTE_{key}_ENABLED")) {
            route.enabled = matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            );
        }
    }

    Ok(routes)
}

fn task_env_key(task: AiTaskType) -> &'static str {
    match task {
        AiTaskType::IntentParse => "INTENT_PARSE",
        AiTaskType::RankExplain => "RANK_EXPLAIN",
        AiTaskType::FeatureExtract => "FEATURE_EXTRACT",
        AiTaskType::Embed => "EMBED",
        AiTaskType::GameSummary => "GAME_SUMMARY",
        AiTaskType::CompareGames => "COMPARE_GAMES",
        AiTaskType::GroupAdvice => "GROUP_ADVICE",
        AiTaskType::DataQuality => "DATA_QUALITY",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_routes_cover_prd_online_tasks() {
        let routes = default_task_routes();
        assert!(routes.contains_key(&AiTaskType::IntentParse));
        assert!(routes.contains_key(&AiTaskType::RankExplain));
        assert!(routes.contains_key(&AiTaskType::GameSummary));
        assert!(routes.contains_key(&AiTaskType::CompareGames));
        assert!(routes.contains_key(&AiTaskType::GroupAdvice));
        assert!(routes.contains_key(&AiTaskType::DataQuality));

        let rank = routes.get(&AiTaskType::RankExplain).unwrap();
        assert_eq!(rank.primary_model, "grok-4.3");
        assert!(
            rank.fallback_models
                .contains(&"grok-4.20-0309-non-reasoning".to_owned())
        );
        assert_eq!(rank.protocol_preference[0], ApiProtocol::Responses);
    }

    #[test]
    fn intent_parse_uses_fast_primary() {
        let routes = default_task_routes();
        let intent = routes.get(&AiTaskType::IntentParse).unwrap();
        assert_eq!(intent.primary_model, "grok-chat-fast");
        assert!(intent.timeout <= Duration::from_secs(10));
        assert!(intent.max_output_tokens <= 1_024);
    }

    #[test]
    fn route_version_is_shared_across_defaults() {
        let routes = default_task_routes();
        let versions: Vec<_> = routes.values().map(|r| r.route_version.as_str()).collect();
        assert!(versions.iter().all(|v| *v == DEFAULT_ROUTE_VERSION));
    }
}
