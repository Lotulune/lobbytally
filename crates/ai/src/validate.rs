use std::collections::{HashMap, HashSet};

use serde_json::Value;

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
        let app_id = item_obj
            .get("app_id")
            .and_then(Value::as_u64)
            .ok_or_else(|| AiError::InvalidOutput(format!("recommendations[{idx}].app_id")))?
            as u32;
        if !by_id.contains_key(&app_id) {
            return Err(AiError::InvalidOutput(format!(
                "app_id {app_id} is outside the candidate set"
            )));
        }
        if !seen.insert(app_id) {
            return Err(AiError::InvalidOutput(format!(
                "duplicate app_id {app_id}"
            )));
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
        // Concrete facts require evidence; empty evidence is only allowed with empty reasons.
        if !reasons.is_empty() && evidence.is_empty() {
            return Err(AiError::InvalidOutput(format!(
                "recommendations[{idx}] reasons require reason_evidence_ids"
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
        .trim()
        .chars()
        .take(MAX_SUMMARY_CHARS)
        .collect::<String>();

    Ok(AiRankResult {
        recommendations: items,
        summary,
    })
}

fn unit_field(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    idx: usize,
) -> Result<f64, AiError> {
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
    lower.contains("<script")
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
            "summary": "ok"
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
            "summary": "inject"
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
            "summary": "x"
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
            "summary": "x"
        });
        assert!(validate_rank_result(&value, &candidates(), 20).is_err());
    }
}
