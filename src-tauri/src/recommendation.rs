use serde::{Deserialize, Serialize};
use time::{format_description::FormatItem, macros::format_description, Date};

const ISO_DATE: &[FormatItem<'_>] = format_description!("[year]-[month]-[day]");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DemoStatus {
    DemoOnly,
    ReleasedWithDemo,
    Released,
    Unknown,
}

impl DemoStatus {
    pub fn from_parts(is_demo_app: bool, has_demo: bool) -> Self {
        match (is_demo_app, has_demo) {
            (true, _) => Self::DemoOnly,
            (false, true) => Self::ReleasedWithDemo,
            (false, false) => Self::Released,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseBucket {
    New,
    Classic,
    ClassicHidden,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameFacts {
    pub appid: u32,
    pub name: String,
    pub release_date: Option<String>,
    pub positive_review_pct: Option<f64>,
    pub total_reviews: Option<u32>,
    pub current_players: Option<u32>,
    pub multiplayer_modes: Vec<String>,
    pub demo_status: DemoStatus,
    pub ai_score: Option<f64>,
}

pub fn compute_recommendation_score(facts: &GameFacts, today_iso: &str) -> f64 {
    if let Some(ai_score) = facts.ai_score {
        return round_one(ai_score.clamp(0.0, 100.0));
    }

    let review_quality = lightweight_review_quality(facts);
    let multiplayer_fit = lightweight_multiplayer_fit(&facts.multiplayer_modes);
    let freshness = freshness_score(facts.release_date.as_deref(), today_iso);
    let discovery_value = lightweight_discovery_value(facts, review_quality, freshness);
    let confidence = lightweight_confidence(facts);
    let uncertainty_penalty = (1.0 - confidence) * 10.0;
    let preanalysis_penalty = 4.0;

    let lightweight_quality_proxy =
        0.45 * review_quality + 0.30 * multiplayer_fit + 0.15 * freshness + 0.10 * discovery_value;

    round_one(
        (0.55 * lightweight_quality_proxy
            + 0.20 * multiplayer_fit
            + 0.15 * discovery_value
            + 0.10 * freshness
            - uncertainty_penalty
            - preanalysis_penalty)
            .clamp(0.0, 100.0),
    )
}

pub fn bucket_game(facts: &GameFacts, today_iso: &str) -> ReleaseBucket {
    match days_since_release(facts.release_date.as_deref(), today_iso) {
        Some(days) if (0..=30).contains(&days) => ReleaseBucket::New,
        Some(days) if days > 30 => {
            let review_pct = facts.positive_review_pct.unwrap_or_default();
            let total_reviews = facts.total_reviews.unwrap_or_default();
            if review_pct >= 80.0 && total_reviews >= 1_000 {
                ReleaseBucket::Classic
            } else if review_pct >= 60.0 && total_reviews >= 300 {
                ReleaseBucket::ClassicHidden
            } else {
                ReleaseBucket::ClassicHidden
            }
        }
        _ => ReleaseBucket::ClassicHidden,
    }
}

pub fn today_iso_utc() -> String {
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = duration / 86_400;
    let date = Date::from_julian_day(2_440_588 + days as i32).unwrap_or(Date::MIN);
    date.format(ISO_DATE)
        .unwrap_or_else(|_| "2026-04-26".to_string())
}

fn lightweight_review_quality(facts: &GameFacts) -> f64 {
    let raw_positive_rate = facts.positive_review_pct.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0;
    let total_reviews = facts.total_reviews.unwrap_or(0) as f64;
    let positive = total_reviews * raw_positive_rate;
    let bayes_positive_rate = (positive + 35.0) / (total_reviews + 35.0 + 15.0);
    let confidence = 1.0 - (-(total_reviews) / 120.0).exp();

    (100.0 * (bayes_positive_rate * 0.85 + raw_positive_rate * 0.15) * confidence
        + 55.0 * (1.0 - confidence))
        .clamp(0.0, 100.0)
}

fn lightweight_multiplayer_fit(modes: &[String]) -> f64 {
    if modes.is_empty() {
        return 25.0;
    }

    let normalized = modes
        .iter()
        .map(|mode| mode.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ");

    let mut score: f64 = 48.0;
    if normalized.contains("co-op") || normalized.contains("cooperative") {
        score += 18.0;
    }
    if normalized.contains("online") || normalized.contains("lan") {
        score += 12.0;
    }
    if normalized.contains("local") || normalized.contains("split screen") {
        score += 12.0;
    }
    if normalized.contains("pvp") {
        score += 8.0;
    }
    if modes.len() >= 2 {
        score += 5.0;
    }

    score.clamp(0.0, 100.0)
}

fn freshness_score(release_date: Option<&str>, today_iso: &str) -> f64 {
    match days_since_release(release_date, today_iso) {
        Some(days) if (0..=30).contains(&days) => 100.0,
        Some(days) if (31..=90).contains(&days) => 75.0,
        Some(days) if (91..=365).contains(&days) => 45.0,
        Some(_) => 25.0,
        None => 35.0,
    }
}

fn lightweight_discovery_value(facts: &GameFacts, review_quality: f64, freshness: f64) -> f64 {
    let positive_review_pct = facts.positive_review_pct.unwrap_or(0.0);
    let total_reviews = facts.total_reviews.unwrap_or(0);
    let current_players = facts.current_players.unwrap_or(0);
    let sleeper_score = if review_quality >= 78.0 && total_reviews < 500 {
        25.0
    } else {
        0.0
    } + if current_players < 300 && positive_review_pct >= 85.0 {
        20.0
    } else {
        0.0
    } + if matches!(
        facts.demo_status,
        DemoStatus::DemoOnly | DemoStatus::ReleasedWithDemo
    ) {
        15.0
    } else {
        0.0
    } + if freshness >= 75.0 { 10.0 } else { 0.0 };
    let demo_potential = match facts.demo_status {
        DemoStatus::DemoOnly => 60.0,
        DemoStatus::ReleasedWithDemo => 45.0,
        DemoStatus::Released => 15.0,
        DemoStatus::Unknown => 10.0,
    };

    (0.45 * freshness + 0.40 * sleeper_score + 0.15 * demo_potential).clamp(0.0, 100.0)
}

fn lightweight_confidence(facts: &GameFacts) -> f64 {
    let review_confidence = 1.0 - (-(facts.total_reviews.unwrap_or(0) as f64) / 120.0).exp();
    let mode_confidence = (facts.multiplayer_modes.len() as f64 / 3.0).min(1.0);
    let activity_confidence = if facts.current_players.is_some() {
        1.0
    } else {
        0.35
    };

    (0.60 * review_confidence + 0.25 * activity_confidence + 0.15 * mode_confidence).clamp(0.0, 1.0)
}

fn days_since_release(release_date: Option<&str>, today_iso: &str) -> Option<i64> {
    let release = Date::parse(release_date?, ISO_DATE).ok()?;
    let today = Date::parse(today_iso, ISO_DATE).ok()?;
    Some((today - release).whole_days())
}

fn round_one(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}
