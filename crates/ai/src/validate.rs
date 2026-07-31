use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::error::AiError;
use crate::types::{AiRankItem, AiRankResult};

const MAX_REASONS: usize = 6;
const MAX_CAUTIONS: usize = 4;
const MAX_REASON_CHARS: usize = 240;
const MAX_SUMMARY_CHARS: usize = 500;
const MAX_EVIDENCE_ID_CHARS: usize = 120;

/// Candidate facts provided to the model for a ranking call.
#[derive(Debug, Clone)]
pub struct CandidateEvidence {
    pub app_id: u32,
    pub evidence_ids: HashSet<String>,
}

/// Expand the evidence ids a model may cite for one feed candidate.
///
/// Models frequently emit normalized ids like `feature:online_coop:{app}` or
/// bare app ids. Only add an alias when the exact corresponding fact is present
/// in the prompt payload; a generic profile object must never authorize facts
/// that were not sent.
pub fn expand_candidate_evidence_ids(
    app_id: u32,
    prompt_item: &Value,
    base: HashSet<String>,
) -> HashSet<String> {
    let mut ids = base;
    ids.insert(format!("app:{app_id}:identity"));
    ids.insert(format!("app:{app_id}"));
    ids.insert(app_id.to_string());

    if prompt_item
        .get("name")
        .is_some_and(evidence_value_is_present)
    {
        ids.insert(format!("app:{app_id}:name"));
    }

    let party = prompt_item.get("party").and_then(Value::as_object);
    let party_min_present = party.is_some_and(|party| {
        ["recommended_min", "min"]
            .iter()
            .any(|key| party.get(*key).is_some_and(evidence_value_is_present))
    });
    let party_max_present = party.is_some_and(|party| {
        ["recommended_max", "max"]
            .iter()
            .any(|key| party.get(*key).is_some_and(evidence_value_is_present))
    });
    if party_min_present || party_max_present {
        ids.insert(format!("app:{app_id}:party"));
        ids.insert(format!("feature:party:{app_id}"));
    }
    if party_min_present {
        ids.insert(format!("feature:recommended_min_players:{app_id}"));
    }
    if party_max_present {
        ids.insert(format!("feature:recommended_max_players:{app_id}"));
    }

    let multiplayer = prompt_item.get("multiplayer").and_then(Value::as_object);
    let multiplayer_aliases = [
        ("dominant_mode", "dominant_mode"),
        ("online_coop", "online_coop"),
        ("private_session", "private_session"),
        ("self_hosted_server", "self_hosted_server"),
        ("crossplay", "crossplay"),
        ("drop_in_out", "drop_in_out"),
    ];
    let mut multiplayer_present = false;
    for (field, alias) in multiplayer_aliases {
        if multiplayer
            .and_then(|values| values.get(field))
            .is_some_and(evidence_value_is_present)
        {
            multiplayer_present = true;
            ids.insert(format!("feature:{alias}:{app_id}"));
        }
    }
    if multiplayer_present {
        ids.insert(format!("app:{app_id}:multiplayer"));
    }

    if prompt_item
        .get("release_date")
        .is_some_and(evidence_value_is_present)
    {
        ids.insert(format!("app:{app_id}:release"));
        ids.insert(format!("feature:release_date:{app_id}"));
    }
    if prompt_item
        .get("section")
        .is_some_and(evidence_value_is_present)
    {
        ids.insert(format!("app:{app_id}:section"));
    }
    ids
}

fn evidence_value_is_present(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

/// Drop forged evidence ids and orphaned claims so ranking scores can still apply.
/// Returns the sanitized result and whether anything was stripped.
pub fn sanitize_rank_result(
    value: &Value,
    candidates: &[CandidateEvidence],
    max_items: usize,
) -> Result<(AiRankResult, bool), AiError> {
    let by_id: HashMap<u32, &CandidateEvidence> =
        candidates.iter().map(|c| (c.app_id, c)).collect();
    let all_evidence: HashSet<&str> = candidates
        .iter()
        .flat_map(|candidate| candidate.evidence_ids.iter().map(String::as_str))
        .collect();

    let obj = value
        .as_object()
        .ok_or_else(|| AiError::InvalidOutput("root must be an object".into()))?;
    let recommendations = obj
        .get("recommendations")
        .and_then(Value::as_array)
        .ok_or_else(|| AiError::InvalidOutput("recommendations must be an array".into()))?;

    let mut cleaned_items = Vec::new();
    let mut stripped = false;
    for item in recommendations {
        let Some(item_obj) = item.as_object() else {
            stripped = true;
            continue;
        };
        let Some(raw_app_id) = item_obj.get("app_id").and_then(Value::as_u64) else {
            stripped = true;
            continue;
        };
        let Ok(app_id) = u32::try_from(raw_app_id) else {
            stripped = true;
            continue;
        };
        let Some(candidate) = by_id.get(&app_id) else {
            stripped = true;
            continue;
        };

        let mut evidence = string_list(
            item_obj,
            "reason_evidence_ids",
            0,
            12,
            MAX_EVIDENCE_ID_CHARS,
        )
        .unwrap_or_default();
        let before = evidence.len();
        evidence.retain(|id| candidate.evidence_ids.contains(id));
        if evidence.len() != before {
            stripped = true;
        }
        let mut reasons =
            string_list(item_obj, "reasons", 0, MAX_REASONS, MAX_REASON_CHARS).unwrap_or_default();
        let mut cautions = string_list(item_obj, "cautions", 0, MAX_CAUTIONS, MAX_REASON_CHARS)
            .unwrap_or_default();

        // Never launder an unsupported claim by attaching unrelated prompt evidence.
        if (!reasons.is_empty() || !cautions.is_empty()) && evidence.is_empty() {
            reasons.clear();
            cautions.clear();
            stripped = true;
        }

        let fit_score = match unit_field(item_obj, "fit_score", 0) {
            Ok(v) => v,
            Err(_) => {
                stripped = true;
                continue;
            }
        };
        let confidence = match unit_field(item_obj, "confidence", 0) {
            Ok(v) => v,
            Err(_) => {
                stripped = true;
                continue;
            }
        };

        cleaned_items.push(json!({
            "app_id": app_id,
            "fit_score": fit_score,
            "confidence": confidence,
            "reason_evidence_ids": evidence,
            "reasons": reasons,
            "cautions": cautions,
        }));
    }

    let summary = obj
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let mut summary_evidence_ids =
        root_string_list(obj, "summary_evidence_ids", 20, MAX_EVIDENCE_ID_CHARS)
            .unwrap_or_default();
    let before_summary = summary_evidence_ids.len();
    summary_evidence_ids.retain(|id| all_evidence.contains(id.as_str()));
    if summary_evidence_ids.len() != before_summary {
        stripped = true;
    }
    let mut summary_out = summary;
    if looks_like_html_or_url(&summary_out) {
        summary_out.clear();
        summary_evidence_ids.clear();
        stripped = true;
    } else if !summary_out.is_empty() && summary_evidence_ids.is_empty() {
        summary_out.clear();
        stripped = true;
    }
    if summary_out.chars().count() > MAX_SUMMARY_CHARS {
        summary_out = summary_out.chars().take(MAX_SUMMARY_CHARS).collect();
        stripped = true;
    }

    if cleaned_items.is_empty() && !recommendations.is_empty() {
        return Err(AiError::InvalidOutput(
            "no recommendations remained after sanitization".into(),
        ));
    }

    let sanitized = json!({
        "recommendations": cleaned_items,
        "summary": summary_out,
        "summary_evidence_ids": summary_evidence_ids,
    });
    let result = validate_rank_result(&sanitized, candidates, max_items)?;
    Ok((result, stripped))
}

/// Validate AI ranking JSON: candidate membership, score ranges, evidence refs.
pub fn validate_rank_result(
    value: &Value,
    candidates: &[CandidateEvidence],
    max_items: usize,
) -> Result<AiRankResult, AiError> {
    let obj = value
        .as_object()
        .ok_or_else(|| AiError::InvalidOutput("root must be an object".into()))?;
    let recommendations = obj
        .get("recommendations")
        .and_then(Value::as_array)
        .ok_or_else(|| AiError::InvalidOutput("recommendations must be an array".into()))?;
    if recommendations.len() > max_items {
        return Err(AiError::InvalidOutput(format!(
            "recommendations length {} exceeds max {max_items}",
            recommendations.len()
        )));
    }

    let by_id: HashMap<u32, &CandidateEvidence> =
        candidates.iter().map(|c| (c.app_id, c)).collect();
    let mut seen = HashSet::new();
    let mut items = Vec::with_capacity(recommendations.len());

    for (idx, item) in recommendations.iter().enumerate() {
        let item_obj = item.as_object().ok_or_else(|| {
            AiError::InvalidOutput(format!("recommendations[{idx}] must be an object"))
        })?;
        let raw_app_id = item_obj
            .get("app_id")
            .and_then(Value::as_u64)
            .ok_or_else(|| AiError::InvalidOutput(format!("recommendations[{idx}].app_id")))?;
        let app_id = u32::try_from(raw_app_id).map_err(|_| {
            AiError::InvalidOutput(format!(
                "recommendations[{idx}].app_id exceeds the supported range"
            ))
        })?;
        if !by_id.contains_key(&app_id) {
            return Err(AiError::InvalidOutput(format!(
                "app_id {app_id} is outside the candidate set"
            )));
        }
        if !seen.insert(app_id) {
            return Err(AiError::InvalidOutput(format!("duplicate app_id {app_id}")));
        }
        let fit_score = unit_field(item_obj, "fit_score", idx)?;
        let confidence = unit_field(item_obj, "confidence", idx)?;
        let reasons = string_list(item_obj, "reasons", idx, MAX_REASONS, MAX_REASON_CHARS)?;
        let cautions = string_list(item_obj, "cautions", idx, MAX_CAUTIONS, MAX_REASON_CHARS)?;
        let evidence = string_list(
            item_obj,
            "reason_evidence_ids",
            idx,
            12,
            MAX_EVIDENCE_ID_CHARS,
        )?;
        let allowed = &by_id[&app_id].evidence_ids;
        for evidence_id in &evidence {
            if !allowed.contains(evidence_id) {
                return Err(AiError::InvalidOutput(format!(
                    "evidence_id '{evidence_id}' is not allowed for app_id {app_id}"
                )));
            }
        }
        // Every user-visible candidate claim requires evidence.
        if (!reasons.is_empty() || !cautions.is_empty()) && evidence.is_empty() {
            return Err(AiError::InvalidOutput(format!(
                "recommendations[{idx}] reasons/cautions require reason_evidence_ids"
            )));
        }
        items.push(AiRankItem {
            app_id,
            fit_score,
            confidence,
            reason_evidence_ids: evidence,
            reasons,
            cautions,
        });
    }

    let summary = obj
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if summary.chars().count() > MAX_SUMMARY_CHARS {
        return Err(AiError::InvalidOutput("summary is too long".into()));
    }
    if looks_like_html_or_url(summary) {
        return Err(AiError::InvalidOutput(
            "summary contains disallowed content".into(),
        ));
    }
    let summary_evidence_ids =
        root_string_list(obj, "summary_evidence_ids", 20, MAX_EVIDENCE_ID_CHARS)?;
    let all_evidence: HashSet<&str> = candidates
        .iter()
        .flat_map(|candidate| candidate.evidence_ids.iter().map(String::as_str))
        .collect();
    for evidence_id in &summary_evidence_ids {
        if !all_evidence.contains(evidence_id.as_str()) {
            return Err(AiError::InvalidOutput(format!(
                "summary evidence_id '{evidence_id}' is not allowed"
            )));
        }
    }
    if !summary.is_empty() && summary_evidence_ids.is_empty() {
        return Err(AiError::InvalidOutput(
            "summary requires summary_evidence_ids".into(),
        ));
    }

    Ok(AiRankResult {
        recommendations: items,
        summary: summary.to_owned(),
        summary_evidence_ids,
    })
}

fn root_string_list(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    max_items: usize,
    max_chars: usize,
) -> Result<Vec<String>, AiError> {
    let Some(raw) = obj.get(key) else {
        return Err(AiError::InvalidOutput(format!("{key} is required")));
    };
    let arr = raw
        .as_array()
        .ok_or_else(|| AiError::InvalidOutput(format!("{key} must be an array")))?;
    if arr.len() > max_items {
        return Err(AiError::InvalidOutput(format!(
            "{key} exceeds {max_items} items"
        )));
    }
    let mut out = Vec::with_capacity(arr.len());
    for (idx, value) in arr.iter().enumerate() {
        let text = value
            .as_str()
            .ok_or_else(|| AiError::InvalidOutput(format!("{key}[{idx}] must be string")))?
            .trim();
        if text.is_empty() {
            continue;
        }
        if text.chars().count() > max_chars || looks_like_html_or_url(text) {
            return Err(AiError::InvalidOutput(format!("{key}[{idx}] is invalid")));
        }
        out.push(text.to_owned());
    }
    Ok(out)
}

fn unit_field(obj: &serde_json::Map<String, Value>, key: &str, idx: usize) -> Result<f64, AiError> {
    let value = obj
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| AiError::InvalidOutput(format!("recommendations[{idx}].{key}")))?;
    if !(0.0..=1.0).contains(&value) || value.is_nan() {
        return Err(AiError::InvalidOutput(format!(
            "recommendations[{idx}].{key} must be in [0,1]"
        )));
    }
    Ok(value)
}

fn string_list(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    idx: usize,
    max_items: usize,
    max_chars: usize,
) -> Result<Vec<String>, AiError> {
    let Some(raw) = obj.get(key) else {
        return Ok(Vec::new());
    };
    let arr = raw.as_array().ok_or_else(|| {
        AiError::InvalidOutput(format!("recommendations[{idx}].{key} must be an array"))
    })?;
    if arr.len() > max_items {
        return Err(AiError::InvalidOutput(format!(
            "recommendations[{idx}].{key} exceeds {max_items} items"
        )));
    }
    let mut out = Vec::with_capacity(arr.len());
    for (j, value) in arr.iter().enumerate() {
        let text = value.as_str().ok_or_else(|| {
            AiError::InvalidOutput(format!("recommendations[{idx}].{key}[{j}] must be string"))
        })?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() > max_chars {
            return Err(AiError::InvalidOutput(format!(
                "recommendations[{idx}].{key}[{j}] too long"
            )));
        }
        if looks_like_html_or_url(trimmed) {
            return Err(AiError::InvalidOutput(format!(
                "recommendations[{idx}].{key}[{j}] contains disallowed content"
            )));
        }
        out.push(trimmed.to_owned());
    }
    Ok(out)
}

fn looks_like_html_or_url(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains('<')
        || lower.contains('>')
        || lower.contains("javascript:")
        || lower.contains("http://")
        || lower.contains("https://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn candidates() -> Vec<CandidateEvidence> {
        vec![
            CandidateEvidence {
                app_id: 1,
                evidence_ids: HashSet::from(["e1".into()]),
            },
            CandidateEvidence {
                app_id: 2,
                evidence_ids: HashSet::from(["e2".into()]),
            },
        ]
    }

    #[test]
    fn accepts_valid_payload() {
        let value = json!({
            "recommendations": [{
                "app_id": 1,
                "fit_score": 0.9,
                "confidence": 0.8,
                "reason_evidence_ids": ["e1"],
                "reasons": ["private coop"],
                "cautions": []
            }],
            "summary": "ok",
            "summary_evidence_ids": ["e1"]
        });
        let result = validate_rank_result(&value, &candidates(), 20).unwrap();
        assert_eq!(result.recommendations.len(), 1);
        assert_eq!(result.recommendations[0].app_id, 1);
    }

    #[test]
    fn rejects_out_of_candidate_app_id() {
        let value = json!({
            "recommendations": [{
                "app_id": 999,
                "fit_score": 0.9,
                "confidence": 0.8,
                "reason_evidence_ids": [],
                "reasons": [],
                "cautions": []
            }],
            "summary": "",
            "summary_evidence_ids": []
        });
        let err = validate_rank_result(&value, &candidates(), 20).unwrap_err();
        assert!(matches!(err, AiError::InvalidOutput(_)));
    }

    #[test]
    fn rejects_forged_evidence() {
        let value = json!({
            "recommendations": [{
                "app_id": 1,
                "fit_score": 0.5,
                "confidence": 0.5,
                "reason_evidence_ids": ["forged"],
                "reasons": ["claim"],
                "cautions": []
            }],
            "summary": "",
            "summary_evidence_ids": []
        });
        assert!(validate_rank_result(&value, &candidates(), 20).is_err());
    }

    #[test]
    fn rejects_score_out_of_range() {
        let value = json!({
            "recommendations": [{
                "app_id": 1,
                "fit_score": 1.5,
                "confidence": 0.5,
                "reason_evidence_ids": [],
                "reasons": [],
                "cautions": []
            }],
            "summary": "",
            "summary_evidence_ids": []
        });
        assert!(validate_rank_result(&value, &candidates(), 20).is_err());
    }

    #[test]
    fn rejects_app_id_that_overflows_u32() {
        let value = json!({
            "recommendations": [{
                "app_id": 4_294_967_297_u64,
                "fit_score": 0.5,
                "confidence": 0.5,
                "reason_evidence_ids": [],
                "reasons": [],
                "cautions": []
            }],
            "summary": "",
            "summary_evidence_ids": []
        });
        assert!(validate_rank_result(&value, &candidates(), 20).is_err());
    }

    #[test]
    fn user_visible_cautions_and_summary_require_evidence() {
        let caution = json!({
            "recommendations": [{
                "app_id": 1,
                "fit_score": 0.5,
                "confidence": 0.5,
                "reason_evidence_ids": [],
                "reasons": [],
                "cautions": ["requires an external service"]
            }],
            "summary": "",
            "summary_evidence_ids": []
        });
        assert!(validate_rank_result(&caution, &candidates(), 20).is_err());

        let summary = json!({
            "recommendations": [],
            "summary": "Game 1 supports private co-op",
            "summary_evidence_ids": []
        });
        assert!(validate_rank_result(&summary, &candidates(), 20).is_err());

        let html_summary = json!({
            "recommendations": [],
            "summary": "<b>Game 1</b>",
            "summary_evidence_ids": ["e1"]
        });
        assert!(validate_rank_result(&html_summary, &candidates(), 20).is_err());
    }

    #[test]
    fn sanitize_drops_claims_instead_of_attaching_unrelated_evidence() {
        let raw = json!({
            "recommendations": [{
                "app_id": 1,
                "fit_score": 0.9,
                "confidence": 0.8,
                "reason_evidence_ids": ["forged", "web:99"],
                "reasons": ["good private coop"],
                "cautions": []
            }],
            "summary": "nice picks",
            "summary_evidence_ids": ["web:1"]
        });
        let (result, stripped) = sanitize_rank_result(&raw, &candidates(), 20).unwrap();
        assert!(stripped);
        assert!(result.recommendations[0].reasons.is_empty());
        assert!(result.recommendations[0].reason_evidence_ids.is_empty());
        assert!(result.summary.is_empty());
        assert!(result.summary_evidence_ids.is_empty());
    }

    #[test]
    fn expand_candidate_evidence_ids_only_adds_aliases_for_present_facts() {
        let item = json!({
            "name": "Game",
            "multiplayer": {"dominant_mode": "coop", "online_coop": true},
            "party": {"recommended_min": 2}
        });
        let expanded = expand_candidate_evidence_ids(42, &item, HashSet::new());
        assert!(expanded.contains("feature:online_coop:42"));
        assert!(expanded.contains("app:42:multiplayer"));
        assert!(expanded.contains("42"));
        assert!(!expanded.contains("feature:self_hosted_server:42"));
        assert!(!expanded.contains("feature:crossplay:42"));
        assert!(!expanded.contains("app:42:profile"));
        assert!(!expanded.contains("review:42:summary"));
    }

    #[test]
    fn null_profile_fields_do_not_create_evidence_aliases() {
        let item = json!({
            "party": {"recommended_min": null, "recommended_max": null},
            "multiplayer": {"dominant_mode": null, "self_hosted_server": null},
            "release_date": null,
            "section": "popular"
        });
        let expanded = expand_candidate_evidence_ids(7, &item, HashSet::new());
        assert!(!expanded.contains("app:7:party"));
        assert!(!expanded.contains("app:7:multiplayer"));
        assert!(!expanded.contains("feature:self_hosted_server:7"));
        assert!(!expanded.contains("app:7:release"));
        assert!(expanded.contains("app:7:section"));
    }
}
