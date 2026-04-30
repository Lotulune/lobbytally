use crate::llm::{self, AnalysisNarrative, LlmRuntimeConfig};
use crate::models::{
    AiAssessment, AnalysisConfidence, AnalysisDimensionScore, AnalysisEvidenceItem,
    AnalysisEvidenceKind, AnalysisPoint, AnalysisReviewEvidenceItem, AnalysisReviewStance,
    AnalysisSource, GameAnalysisReport, GameCard,
};
use anyhow::{anyhow, Result};
use reqwest::Client;

pub fn build_rule_report(game: &GameCard, generated_at: String) -> Result<GameAnalysisReport> {
    let has_core_signal = !game.tags.is_empty()
        || !game.multiplayer_modes.is_empty()
        || game.positive_review_pct.is_some()
        || game.total_reviews.is_some()
        || !game.review_snippets.is_empty();
    if !has_core_signal {
        return Err(anyhow!("数据不足，暂时无法分析"));
    }

    let dimension_scores = vec![
        approachability_dimension(game),
        multiplayer_fun_dimension(game),
        content_depth_dimension(game),
        reputation_stability_dimension(game),
        activity_health_dimension(game),
    ];
    let overall_score = round_score(
        dimension_scores.iter().map(|item| item.score).sum::<f64>() / dimension_scores.len() as f64,
    );

    let evidence = build_evidence(game);
    let review_evidence = build_review_evidence(game);
    let confidence = derive_confidence(game, evidence.len(), review_evidence.len());
    let strengths = build_strengths(game, &dimension_scores);
    let risks = build_risks(game, &dimension_scores);

    Ok(GameAnalysisReport {
        appid: game.appid,
        generated_at,
        source: AnalysisSource::Rule,
        confidence,
        overall_score,
        overview: build_overview(game, overall_score),
        dimension_scores,
        strengths,
        risks,
        evidence,
        review_evidence,
    })
}

pub fn apply_narrative_patch(
    mut report: GameAnalysisReport,
    narrative: AnalysisNarrative,
) -> GameAnalysisReport {
    if !narrative.overview.trim().is_empty() {
        report.overview = narrative.overview;
    }
    if !narrative.strengths.is_empty() {
        report.strengths = narrative.strengths;
    }
    if !narrative.risks.is_empty() {
        report.risks = narrative.risks;
    }
    for (key, reason) in narrative.dimension_reasons {
        if reason.trim().is_empty() {
            continue;
        }
        if let Some(dimension) = report
            .dimension_scores
            .iter_mut()
            .find(|item| item.key == key)
        {
            dimension.reason = reason;
        }
    }
    report.source = AnalysisSource::Hybrid;
    report
}

pub fn summarize_report_as_assessment(report: &GameAnalysisReport) -> AiAssessment {
    let best_for = report
        .strengths
        .iter()
        .map(|item| item.title.clone())
        .filter(|item| !item.trim().is_empty())
        .take(3)
        .collect::<Vec<_>>();
    let risks = report
        .risks
        .iter()
        .map(|item| item.title.clone())
        .filter(|item| !item.trim().is_empty())
        .take(3)
        .collect::<Vec<_>>();

    AiAssessment {
        appid: report.appid,
        score: report.overall_score,
        summary: report.overview.clone(),
        best_for,
        risks,
    }
}

pub async fn generate_game_analysis(
    client: &Client,
    config: &LlmRuntimeConfig,
    game: &GameCard,
    generated_at: String,
) -> Result<GameAnalysisReport> {
    let rule_report = build_rule_report(game, generated_at)?;
    if config.api_key.is_none() {
        return Ok(rule_report);
    }

    match llm::generate_analysis_narrative(client, config, game, &rule_report).await {
        Ok(narrative) => Ok(apply_narrative_patch(rule_report, narrative)),
        Err(_) => Ok(rule_report),
    }
}

fn build_overview(game: &GameCard, overall_score: f64) -> String {
    let review_phrase = match game.positive_review_pct.unwrap_or(0.0) {
        pct if pct >= 90.0 => "口碑基础扎实",
        pct if pct >= 80.0 => "口碑总体健康",
        pct if pct > 0.0 => "口碑存在分歧",
        _ => "口碑样本有限",
    };
    let multiplayer_phrase = if game.multiplayer_modes.is_empty() {
        "多人玩法信息偏少"
    } else {
        "多人协作定位明确"
    };
    format!(
        "{}，{}，规则评分 {:.1} 分。",
        review_phrase, multiplayer_phrase, overall_score
    )
}

fn build_evidence(game: &GameCard) -> Vec<AnalysisEvidenceItem> {
    let mut items = Vec::new();
    if let Some(pct) = game.positive_review_pct {
        items.push(AnalysisEvidenceItem {
            kind: AnalysisEvidenceKind::PositiveReviewPct,
            label: "好评率".to_string(),
            value: format!("{pct:.1}%"),
            interpretation: if pct >= 90.0 {
                "正向口碑明显，玩家满意度基础较稳。".to_string()
            } else if pct >= 80.0 {
                "整体评价偏正面，但仍需结合差评主题判断。".to_string()
            } else {
                "评价分歧较明显，需要重点核对风险点。".to_string()
            },
        });
    }
    if let Some(total_reviews) = game.total_reviews {
        items.push(AnalysisEvidenceItem {
            kind: AnalysisEvidenceKind::TotalReviews,
            label: "评测总量".to_string(),
            value: total_reviews.to_string(),
            interpretation: if total_reviews >= 1000 {
                "样本量充足，口碑波动更有参考价值。".to_string()
            } else {
                "样本量一般，结论需要保留一定弹性。".to_string()
            },
        });
    }
    if let Some(current_players) = game.current_players {
        items.push(AnalysisEvidenceItem {
            kind: AnalysisEvidenceKind::CurrentPlayers,
            label: "当前在线".to_string(),
            value: current_players.to_string(),
            interpretation: if current_players >= 1000 {
                "活跃度不错，组队成功率通常更稳定。".to_string()
            } else if current_players >= 100 {
                "有一定活跃样本，适合固定好友局。".to_string()
            } else {
                "在线样本偏小，临时匹配体验需要谨慎看待。".to_string()
            },
        });
    }
    if !game.tags.is_empty() {
        items.push(AnalysisEvidenceItem {
            kind: AnalysisEvidenceKind::Tags,
            label: "标签".to_string(),
            value: game
                .tags
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join(" / "),
            interpretation: "标签能帮助判断题材、节奏与目标人群。".to_string(),
        });
    }
    if !game.multiplayer_modes.is_empty() {
        items.push(AnalysisEvidenceItem {
            kind: AnalysisEvidenceKind::MultiplayerModes,
            label: "联机模式".to_string(),
            value: game.multiplayer_modes.join(" / "),
            interpretation: "联机模式直接影响开黑方式与组队门槛。".to_string(),
        });
    }
    if let Some(short_description) = game
        .short_description
        .as_ref()
        .filter(|text| !text.is_empty())
    {
        items.push(AnalysisEvidenceItem {
            kind: AnalysisEvidenceKind::ShortDescription,
            label: "简介".to_string(),
            value: short_description.clone(),
            interpretation: "商店简介用于补足玩法目标与节奏信息。".to_string(),
        });
    }
    items
}

fn build_review_evidence(game: &GameCard) -> Vec<AnalysisReviewEvidenceItem> {
    game.review_snippets
        .iter()
        .take(4)
        .map(|snippet| AnalysisReviewEvidenceItem {
            stance: if snippet.voted_up {
                AnalysisReviewStance::Strength
            } else {
                AnalysisReviewStance::Risk
            },
            quote: snippet.review.clone(),
            playtime_text: snippet
                .playtime_hours
                .map(|hours| format!("{hours:.1}h"))
                .unwrap_or_else(|| "未知时长".to_string()),
            interpretation: if snippet.voted_up {
                "正向评测通常反映了合作体验、反馈手感或上手门槛的优势。".to_string()
            } else {
                "负向评测可用于识别内容深度、平衡或留存方面的风险。".to_string()
            },
        })
        .collect()
}

fn derive_confidence(
    game: &GameCard,
    evidence_count: usize,
    review_evidence_count: usize,
) -> AnalysisConfidence {
    let metadata_score = usize::from(game.positive_review_pct.is_some())
        + usize::from(game.total_reviews.is_some())
        + usize::from(game.current_players.is_some())
        + usize::from(!game.tags.is_empty())
        + usize::from(!game.multiplayer_modes.is_empty());

    if metadata_score >= 4 && evidence_count >= 4 && review_evidence_count >= 2 {
        AnalysisConfidence::High
    } else if metadata_score >= 2 && evidence_count >= 2 {
        AnalysisConfidence::Medium
    } else {
        AnalysisConfidence::Low
    }
}

fn build_strengths(game: &GameCard, dimensions: &[AnalysisDimensionScore]) -> Vec<AnalysisPoint> {
    let mut items = Vec::new();
    if let Some(dimension) = dimensions
        .iter()
        .find(|item| item.key == "approachability" && item.score >= 75.0)
    {
        items.push(AnalysisPoint {
            title: "上手负担较低".to_string(),
            reason: dimension.reason.clone(),
        });
    }
    if let Some(dimension) = dimensions
        .iter()
        .find(|item| item.key == "multiplayer_fun" && item.score >= 78.0)
    {
        items.push(AnalysisPoint {
            title: "适合朋友开黑".to_string(),
            reason: dimension.reason.clone(),
        });
    }
    if let Some(pct) = game.positive_review_pct.filter(|pct| *pct >= 90.0) {
        items.push(AnalysisPoint {
            title: "口碑表现稳定".to_string(),
            reason: format!("好评率达到 {pct:.1}%，玩家满意度基础较强。"),
        });
    }
    if items.is_empty() {
        items.push(AnalysisPoint {
            title: "具备基础尝试价值".to_string(),
            reason: "现有元数据仍显示出一定的多人尝试空间。".to_string(),
        });
    }
    items
}

fn build_risks(game: &GameCard, dimensions: &[AnalysisDimensionScore]) -> Vec<AnalysisPoint> {
    let mut items = Vec::new();
    if let Some(dimension) = dimensions
        .iter()
        .find(|item| item.key == "content_depth" && item.score <= 72.0)
    {
        items.push(AnalysisPoint {
            title: "长期内容待观察".to_string(),
            reason: dimension.reason.clone(),
        });
    }
    if game
        .review_snippets
        .iter()
        .any(|snippet| !snippet.voted_up && !snippet.review.trim().is_empty())
    {
        items.push(AnalysisPoint {
            title: "差评风险点需核对".to_string(),
            reason: "已有负向评测提到体验短板，建议确认是否踩中你的雷点。".to_string(),
        });
    }
    if game.current_players.unwrap_or(0) < 100 {
        items.push(AnalysisPoint {
            title: "在线样本偏小".to_string(),
            reason: "活跃人数不足时，匹配与留存结论会更不稳定。".to_string(),
        });
    }
    if items.is_empty() {
        items.push(AnalysisPoint {
            title: "后续更新仍要继续观察".to_string(),
            reason: "即使基础面不错，版本节奏和内容扩展仍可能影响长期体验。".to_string(),
        });
    }
    items
}

fn approachability_dimension(game: &GameCard) -> AnalysisDimensionScore {
    let mut score: f64 = 52.0;
    if game.demo_status != crate::recommendation::DemoStatus::Unknown {
        score += 10.0;
    }
    if game
        .tags
        .iter()
        .any(|tag| contains_any(tag, &["casual", "co-op", "simulation", "party"]))
    {
        score += 12.0;
    }
    if game
        .supported_languages
        .iter()
        .any(|lang| lang.contains("Chinese"))
    {
        score += 8.0;
    }
    AnalysisDimensionScore {
        key: "approachability".to_string(),
        label: "易上手度".to_string(),
        score: round_score(score.clamp(0.0, 100.0)),
        reason: if game.demo_status == crate::recommendation::DemoStatus::ReleasedWithDemo {
            "有试玩或明确的合作标签，通常更利于朋友局快速判断是否合拍。".to_string()
        } else {
            "标签和语言支持说明它并非高门槛取向，但仍需看实际教程设计。".to_string()
        },
    }
}

fn multiplayer_fun_dimension(game: &GameCard) -> AnalysisDimensionScore {
    let joined = game
        .multiplayer_modes
        .iter()
        .map(|mode| mode.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    let mut score: f64 = if joined.is_empty() { 45.0 } else { 68.0 };
    if joined.contains("co-op") {
        score += 16.0;
    }
    if joined.contains("online") || joined.contains("lan") {
        score += 8.0;
    }
    AnalysisDimensionScore {
        key: "multiplayer_fun".to_string(),
        label: "联机乐趣".to_string(),
        score: round_score(score.clamp(0.0, 100.0)),
        reason: if joined.contains("co-op") {
            "联机模式直接围绕合作展开，更容易形成明确分工和朋友局乐趣。".to_string()
        } else {
            "存在联机信息，但合作密度与重复游玩价值还需要更多样本。".to_string()
        },
    }
}

fn content_depth_dimension(game: &GameCard) -> AnalysisDimensionScore {
    let mut score: f64 = 56.0;
    let tag_count = game.tags.len() as f64;
    score += (tag_count * 4.0).min(12.0);
    if game.review_snippets.iter().any(|snippet| {
        !snippet.voted_up
            && contains_any(
                &snippet.review,
                &["thin", "variety", "repeat", "late-game", "late game"],
            )
    }) {
        score -= 8.0;
    }
    AnalysisDimensionScore {
        key: "content_depth".to_string(),
        label: "内容深度".to_string(),
        score: round_score(score.clamp(0.0, 100.0)),
        reason: if score >= 72.0 {
            "标签覆盖面和评测反馈显示它不只靠一次性新鲜感支撑。".to_string()
        } else {
            "现有标签能支撑短中期体验，但差评提示后段变化和内容厚度仍需观察。".to_string()
        },
    }
}

fn reputation_stability_dimension(game: &GameCard) -> AnalysisDimensionScore {
    let review_pct = game.positive_review_pct.unwrap_or(0.0);
    let total_reviews = game.total_reviews.unwrap_or(0) as f64;
    let review_volume_bonus = (total_reviews.log10() * 8.0).clamp(0.0, 24.0);
    let score = review_pct * 0.72 + review_volume_bonus;
    AnalysisDimensionScore {
        key: "reputation_stability".to_string(),
        label: "口碑稳定性".to_string(),
        score: round_score(score.clamp(0.0, 100.0)),
        reason: if total_reviews >= 1000.0 {
            "好评率配合较充足的样本量，能减少偶然波动对判断的干扰。".to_string()
        } else {
            "已有口碑方向，但样本量仍不足以完全排除波动影响。".to_string()
        },
    }
}

fn activity_health_dimension(game: &GameCard) -> AnalysisDimensionScore {
    let players = game.current_players.unwrap_or(0) as f64;
    let player_score = if players <= 0.0 {
        35.0
    } else {
        35.0 + (players.log10() * 18.0).clamp(0.0, 50.0)
    };
    AnalysisDimensionScore {
        key: "activity_health".to_string(),
        label: "活跃健康度".to_string(),
        score: round_score(player_score.clamp(0.0, 100.0)),
        reason: if players >= 1000.0 {
            "当前在线样本不错，说明它至少具备一定的持续开黑需求。".to_string()
        } else if players >= 100.0 {
            "仍有一定活跃基础，更适合拉固定好友一起玩。".to_string()
        } else {
            "活跃样本较小，临时匹配或长期留存的稳定性需要保守看待。".to_string()
        },
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    let text = text.to_ascii_lowercase();
    needles.iter().any(|needle| text.contains(needle))
}

fn round_score(score: f64) -> f64 {
    (score * 10.0).round() / 10.0
}
