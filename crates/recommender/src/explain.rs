use mpgs_domain::RankingSignals;
use serde::{Deserialize, Serialize};

use crate::ScoreBreakdown;
use crate::unit;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    pub reasons: Vec<String>,
    pub cautions: Vec<String>,
    pub evidence_ids: Vec<String>,
}

pub fn explain(
    app_id: u32,
    signals: &RankingSignals,
    score: &ScoreBreakdown,
    dominant_mode: Option<&str>,
) -> Explanation {
    let mut reason_candidates: Vec<(f64, String, Option<String>)> = Vec::new();
    let mut caution_candidates: Vec<(f64, String, Option<String>)> = Vec::new();
    let mp = &signals.multiplayer;
    let personal = &signals.personal_components;

    if unit(mp.private_session) >= 0.6 {
        reason_candidates.push((
            0.22 * unit(mp.private_session),
            "支持私人房间联机".into(),
            Some(format!("feature:private_session:{app_id}")),
        ));
    }
    if unit(mp.self_host_or_dedicated) >= 0.6 {
        reason_candidates.push((
            0.20 * unit(mp.self_host_or_dedicated),
            "可自建服或专用服务器".into(),
            Some(format!("feature:self_hosted_server:{app_id}")),
        ));
    }
    if unit(mp.online_coop) >= 0.6 {
        reason_candidates.push((
            0.18 * unit(mp.online_coop),
            "具备在线合作体验".into(),
            Some(format!("feature:online_coop:{app_id}")),
        ));
    }
    if unit(personal.group_fit) >= 0.68 {
        reason_candidates.push((
            0.25 * (unit(personal.group_fit) - 0.5),
            "人数匹配当前小组".into(),
            None,
        ));
    }
    if unit(personal.mode_fit) >= 0.68 {
        reason_candidates.push((
            0.20 * (unit(personal.mode_fit) - 0.5),
            "合作或竞技取向符合本次偏好".into(),
            None,
        ));
    }
    if unit(personal.access_fit) >= 0.68 {
        reason_candidates.push((
            0.20 * (unit(personal.access_fit) - 0.5),
            "平台与联机可达性较匹配".into(),
            None,
        ));
    }
    if unit(personal.hosting_fit) >= 0.68 {
        reason_candidates.push((
            0.15 * (unit(personal.hosting_fit) - 0.5),
            "开房或开服方式符合本次偏好".into(),
            None,
        ));
    }
    if unit(personal.session_fit) >= 0.68 {
        reason_candidates.push((
            0.10 * (unit(personal.session_fit) - 0.5),
            "单局时长适合当前安排".into(),
            None,
        ));
    }
    if unit(signals.quality) >= 0.70 {
        reason_candidates.push((
            0.30 * (unit(signals.quality) - 0.5),
            "口碑质量在候选中更有竞争力".into(),
            Some(format!("review:{app_id}:summary")),
        ));
    }

    if unit(mp.matchmaking_core) >= 0.6 {
        caution_candidates.push((
            0.60 * unit(mp.matchmaking_core),
            "核心体验偏公共匹配".into(),
            Some(format!("feature:matchmaking_core:{app_id}")),
        ));
    }
    if unit(mp.public_world_dependency) >= 0.6 {
        caution_candidates.push((
            0.70 * unit(mp.public_world_dependency),
            "依赖公共世界玩家生态".into(),
            Some(format!("feature:public_world_dependency:{app_id}")),
        ));
    }
    if unit(mp.service_shutdown_risk) >= 0.5 {
        caution_candidates.push((
            unit(mp.service_shutdown_risk),
            "服务停运风险需关注".into(),
            Some(format!("feature:service_shutdown_risk:{app_id}")),
        ));
    }
    if unit(mp.group_size_mismatch) >= 0.4 {
        caution_candidates.push((
            0.90 * unit(mp.group_size_mismatch),
            "推荐人数与当前小组可能不匹配".into(),
            None,
        ));
    }
    if unit(signals.data_confidence) < 0.45 {
        caution_candidates.push((0.85, "资料较少，部分联机条件仍待核实".into(), None));
    } else if unit(signals.data_confidence) < 0.70 {
        caution_candidates.push((0.35, "仍有部分联机细节待核实".into(), None));
    }
    if let Some(mode) = dominant_mode
        && mode.eq_ignore_ascii_case("mmo")
    {
        caution_candidates.push((0.65, "MMO/公共世界主导".into(), None));
    }

    reason_candidates.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    let selected_reasons: Vec<_> = reason_candidates.into_iter().take(3).collect();
    let mut reasons: Vec<String> = selected_reasons
        .iter()
        .map(|(_, reason, _)| reason.clone())
        .collect();
    if reasons.is_empty() && score.friend_fit >= 0.5 {
        reasons.push("熟人联机适配度尚可".into());
    }
    if reasons.is_empty() {
        reasons.push("进入候选池".into());
    }

    caution_candidates.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    let cautions: Vec<String> = caution_candidates
        .first()
        .map(|(_, caution, _)| vec![caution.clone()])
        .unwrap_or_default();
    let mut evidence_ids: Vec<String> = selected_reasons
        .into_iter()
        .filter_map(|(_, _, evidence_id)| evidence_id)
        .collect();
    if let Some((_, _, Some(evidence_id))) = caution_candidates.first() {
        evidence_ids.push(evidence_id.clone());
    }
    evidence_ids.sort_unstable();
    evidence_ids.dedup();

    Explanation {
        reasons,
        cautions,
        evidence_ids,
    }
}

#[cfg(test)]
mod tests {
    use mpgs_domain::{MultiplayerSignals, PersonalFitSignals, RankingSignals};

    use super::*;

    #[test]
    fn explanation_selects_three_distinctive_reasons_and_one_uncertainty() {
        let signals = RankingSignals {
            multiplayer: MultiplayerSignals {
                private_session: 1.0,
                self_host_or_dedicated: 1.0,
                online_coop: 1.0,
                ..Default::default()
            },
            personal_components: PersonalFitSignals {
                group_fit: 0.9,
                mode_fit: 0.9,
                access_fit: 0.9,
                ..Default::default()
            },
            quality: 0.9,
            data_confidence: 0.4,
            ..Default::default()
        };
        let score = crate::score(mpgs_domain::FeedSection::RecentRelease, &signals, None);
        let result = explain(42, &signals, &score, Some("coop"));

        assert_eq!(result.reasons.len(), 3);
        assert_eq!(result.cautions.len(), 1);
        assert!(result.cautions[0].contains("资料较少"));
        assert!(!result.reasons.iter().any(|reason| reason == "早期数据"));
        assert!(result.evidence_ids.len() <= result.reasons.len());
    }
}
