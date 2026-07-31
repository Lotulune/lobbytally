//! Multi-game comparison explanation validation (PRD AI-009).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::AiError;

pub const COMPARE_PROMPT_VERSION: &str = "compare-v1";

/// Allowed comparison dimensions — models may not invent arbitrary columns.
pub const COMPARE_COLUMNS: &[&str] = &[
    "party_size",
    "platforms",
    "price",
    "multiplayer_mode",
    "service_dependency",
    "content_pacing",
    "review_quality",
    "data_updated_at",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CompareExplanation {
    pub summary: String,
    pub differences: Vec<CompareDifference>,
    pub risks: Vec<String>,
    pub preferred_app_id: Option<u32>,
    pub preferred_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareDifference {
    pub column: String,
    pub text: String,
    pub evidence_ids: Vec<String>,
}

fn evidence_id_allowed(id: &str, allowed_evidence: &std::collections::HashSet<String>) -> bool {
    allowed_evidence.contains(id)
}

fn evidence_belongs_to_app(id: &str, app_id: u32) -> bool {
    id == app_id.to_string()
        || id == format!("app:{app_id}")
        || id.starts_with(&format!("app:{app_id}:"))
}

fn looks_like_html_or_url(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains('<')
        || lower.contains('>')
        || lower.contains("javascript:")
        || lower.contains("http://")
        || lower.contains("https://")
}

pub fn parse_compare_explanation(
    value: &Value,
    allowed_app_ids: &[u32],
    allowed_evidence: &std::collections::HashSet<String>,
) -> Result<CompareExplanation, AiError> {
    let mut explanation: CompareExplanation = serde_json::from_value(value.clone())
        .map_err(|error| AiError::InvalidOutput(format!("compare schema: {error}")))?;

    if explanation.summary.chars().count() > 800 {
        return Err(AiError::InvalidOutput(
            "compare.summary exceeds 800 chars".into(),
        ));
    }
    if looks_like_html_or_url(&explanation.summary) {
        return Err(AiError::InvalidOutput(
            "compare.summary contains disallowed content".into(),
        ));
    }
    if explanation.differences.len() > 12 {
        return Err(AiError::InvalidOutput(
            "compare.differences exceeds 12 entries".into(),
        ));
    }
    if explanation.risks.len() > 8 {
        return Err(AiError::InvalidOutput(
            "compare.risks exceeds 8 entries".into(),
        ));
    }
    for risk in &explanation.risks {
        if risk.chars().count() > 400 || looks_like_html_or_url(risk) {
            return Err(AiError::InvalidOutput(
                "compare risk contains disallowed content".into(),
            ));
        }
    }

    let mut cited_evidence = std::collections::HashSet::new();
    for diff in &explanation.differences {
        if !COMPARE_COLUMNS.contains(&diff.column.as_str()) {
            return Err(AiError::InvalidOutput(format!(
                "compare column '{}' is not allowed",
                diff.column
            )));
        }
        if diff.text.chars().count() > 400 {
            return Err(AiError::InvalidOutput(
                "compare difference text exceeds 400 chars".into(),
            ));
        }
        if looks_like_html_or_url(&diff.text) {
            return Err(AiError::InvalidOutput(
                "compare difference contains disallowed content".into(),
            ));
        }
        if !diff.text.trim().is_empty() && diff.evidence_ids.is_empty() {
            return Err(AiError::InvalidOutput(
                "compare differences require evidence_ids".into(),
            ));
        }
        if diff.evidence_ids.len() > 12 {
            return Err(AiError::InvalidOutput(
                "compare difference evidence_ids exceeds 12 entries".into(),
            ));
        }
        for id in &diff.evidence_ids {
            if evidence_id_allowed(id, allowed_evidence) {
                cited_evidence.insert(id.as_str());
                continue;
            }
            return Err(AiError::InvalidOutput(format!(
                "compare evidence_id '{id}' is not allowed"
            )));
        }
    }

    let has_general_claim = !explanation.summary.trim().is_empty()
        || explanation.risks.iter().any(|risk| !risk.trim().is_empty());
    if has_general_claim && cited_evidence.is_empty() {
        return Err(AiError::InvalidOutput(
            "compare summary/risks require evidence-backed differences".into(),
        ));
    }

    if let Some(app_id) = explanation.preferred_app_id
        && !allowed_app_ids.contains(&app_id)
    {
        return Err(AiError::InvalidOutput(format!(
            "preferred_app_id {app_id} is outside the compare set"
        )));
    }

    // Drop preferred_reason without a preferred app.
    if explanation.preferred_app_id.is_none() {
        explanation.preferred_reason = None;
    } else if let (Some(app_id), Some(reason)) = (
        explanation.preferred_app_id,
        explanation.preferred_reason.as_deref(),
    ) && !reason.trim().is_empty()
    {
        if reason.chars().count() > 400 || looks_like_html_or_url(reason) {
            return Err(AiError::InvalidOutput(
                "preferred_reason contains disallowed content".into(),
            ));
        }
        if !cited_evidence
            .iter()
            .any(|id| evidence_belongs_to_app(id, app_id))
        {
            return Err(AiError::InvalidOutput(
                "preferred_reason requires evidence for preferred_app_id".into(),
            ));
        }
    }

    Ok(explanation)
}

pub fn compare_system_prompt() -> &'static str {
    "You are MPGS comparison assistant. Explain only differences present in the \
     server fact matrix. Use only allowed column names. Never invent AppIDs, prices, \
     platforms, or evidence. Output JSON only."
}

pub fn compare_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["summary", "differences", "risks"],
        "properties": {
            "summary": { "type": "string" },
            "differences": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["column", "text", "evidence_ids"],
                    "properties": {
                        "column": { "type": "string" },
                        "text": { "type": "string" },
                        "evidence_ids": { "type": "array", "items": { "type": "string" } }
                    }
                }
            },
            "risks": { "type": "array", "items": { "type": "string" } },
            "preferred_app_id": { "type": ["integer", "null"], "minimum": 1 },
            "preferred_reason": { "type": ["string", "null"] }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    #[test]
    fn rejects_arbitrary_column_names() {
        let value = json!({
            "summary": "x",
            "differences": [{
                "column": "sql_expression",
                "text": "bad",
                "evidence_ids": ["e1"]
            }],
            "risks": []
        });
        let mut evidence = HashSet::new();
        evidence.insert("e1".into());
        assert!(parse_compare_explanation(&value, &[1, 2], &evidence).is_err());
    }

    #[test]
    fn rejects_preferred_app_outside_set() {
        let value = json!({
            "summary": "ok",
            "differences": [],
            "risks": [],
            "preferred_app_id": 999
        });
        assert!(parse_compare_explanation(&value, &[1, 2], &HashSet::new()).is_err());
    }

    #[test]
    fn accepts_allowed_columns_with_evidence() {
        let value = json!({
            "summary": "A has larger party size",
            "differences": [{
                "column": "party_size",
                "text": "A supports more players",
                "evidence_ids": ["app:1:party_size"]
            }],
            "risks": ["data may be stale"],
            "preferred_app_id": 1,
            "preferred_reason": "party size"
        });
        let mut evidence = HashSet::new();
        evidence.insert("app:1:party_size".into());
        let parsed = parse_compare_explanation(&value, &[1, 2], &evidence).unwrap();
        assert_eq!(parsed.preferred_app_id, Some(1));
    }

    #[test]
    fn rejects_app_prefixed_evidence_that_was_not_explicitly_allowed() {
        let value = json!({
            "summary": "A has a lower price",
            "differences": [{
                "column": "price",
                "text": "A is cheaper",
                "evidence_ids": ["app:1:invented-price"]
            }],
            "risks": []
        });
        assert!(parse_compare_explanation(&value, &[1, 2], &HashSet::new()).is_err());
    }

    #[test]
    fn rejects_visible_summary_without_evidence_backed_differences() {
        let value = json!({
            "summary": "A is the safer choice",
            "differences": [],
            "risks": [],
            "preferred_app_id": null,
            "preferred_reason": null
        });
        assert!(parse_compare_explanation(&value, &[1, 2], &HashSet::new()).is_err());
    }
}
