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
    let review_quality = facts.positive_review_pct.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0 * 36.0;
    let review_confidence = log_weight(facts.total_reviews.unwrap_or(0) as f64, 10_000.0) * 8.0;
    let player_activity = log_weight(facts.current_players.unwrap_or(0) as f64, 10_000.0) * 14.0;
    let multiplayer_fit = multiplayer_fit_score(&facts.multiplayer_modes);
    let demo_bonus = match facts.demo_status {
        DemoStatus::DemoOnly => 4.0,
        DemoStatus::ReleasedWithDemo => 4.0,
        DemoStatus::Released => 1.5,
        DemoStatus::Unknown => 0.0,
    };
    let freshness = freshness_score(facts.release_date.as_deref(), today_iso);
    let ai_score = facts.ai_score.unwrap_or(72.0).clamp(0.0, 100.0) / 100.0 * 20.0;

    round_one(
        review_quality
            + review_confidence
            + player_activity
            + multiplayer_fit
            + demo_bonus
            + freshness
            + ai_score,
    )
    .clamp(0.0, 100.0)
}

pub fn bucket_game(facts: &GameFacts, today_iso: &str) -> ReleaseBucket {
    match days_since_release(facts.release_date.as_deref(), today_iso) {
        Some(days) if (0..=30).contains(&days) => ReleaseBucket::New,
        _ => ReleaseBucket::Classic,
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

fn multiplayer_fit_score(modes: &[String]) -> f64 {
    if modes.is_empty() {
        return 0.0;
    }

    let normalized = modes
        .iter()
        .map(|mode| mode.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ");

    let mut score: f64 = 8.0;
    if normalized.contains("co-op") || normalized.contains("cooperative") {
        score += 4.0;
    }
    if normalized.contains("online")
        || normalized.contains("lan")
        || normalized.contains("multi-player")
    {
        score += 2.0;
    }

    score.clamp(0.0, 14.0)
}

fn freshness_score(release_date: Option<&str>, today_iso: &str) -> f64 {
    match days_since_release(release_date, today_iso) {
        Some(days) if (0..=7).contains(&days) => 5.0,
        Some(days) if (8..=30).contains(&days) => 4.0,
        Some(days) if (31..=180).contains(&days) => 1.5,
        _ => 0.0,
    }
}

fn days_since_release(release_date: Option<&str>, today_iso: &str) -> Option<i64> {
    let release = Date::parse(release_date?, ISO_DATE).ok()?;
    let today = Date::parse(today_iso, ISO_DATE).ok()?;
    Some((today - release).whole_days())
}

fn log_weight(value: f64, max_reference: f64) -> f64 {
    if value <= 0.0 {
        return 0.0;
    }

    ((value + 1.0).log10() / (max_reference + 1.0).log10()).clamp(0.0, 1.0)
}

fn round_one(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}
