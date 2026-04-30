use crate::models::{ReviewSnippet, StoreReleaseState};
use crate::recommendation::DemoStatus;
use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use reqwest::StatusCode;
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};
use time::{format_description::FormatItem, macros::format_description, Date};

const SHORT_STEAM_DATE: &[FormatItem<'_>] =
    format_description!("[month repr:short] [day padding:none], [year]");
const LONG_STEAM_DATE: &[FormatItem<'_>] =
    format_description!("[month repr:long] [day padding:none], [year]");
const ISO_DATE: &[FormatItem<'_>] = format_description!("[year]-[month]-[day]");
const PRIMARY_REVIEW_SNIPPET_COUNT: u32 = 8;
const ENGLISH_REVIEW_FALLBACK_COUNT: u32 = 2;
const APP_DETAILS_MAX_ATTEMPTS: u8 = 3;
const APP_DETAILS_RETRY_BASE_DELAY_MS: u64 = 350;

#[derive(Debug, Clone, Copy)]
struct SteamAppListEndpoint {
    label: &'static str,
    url: &'static str,
}

const STEAM_APP_LIST_ENDPOINTS: [SteamAppListEndpoint; 2] = [
    SteamAppListEndpoint {
        label: "Steam Partner API",
        url: "https://partner.steam-api.com/IStoreService/GetAppList/v1/",
    },
    SteamAppListEndpoint {
        label: "Steam 公共 Web API 备用入口",
        url: "https://api.steampowered.com/IStoreService/GetAppList/v1/",
    },
];
const STEAM_APP_LIST_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const STEAM_STORE_SEARCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const APP_DETAILS_FILTERS: &str =
    "basic,price_overview,release_date,categories,genres,demos,content_descriptors,screenshots";

fn steam_app_list_endpoints() -> &'static [SteamAppListEndpoint] {
    &STEAM_APP_LIST_ENDPOINTS
}

#[derive(Debug, Clone)]
pub struct SteamGameSnapshot {
    pub name: Option<String>,
    pub short_description: Option<String>,
    pub release_date: Option<String>,
    pub release_date_text: Option<String>,
    pub release_state: Option<StoreReleaseState>,
    pub demo_status: DemoStatus,
    pub supported_languages: Option<Vec<String>>,
    pub is_adult_content: Option<bool>,
    pub price_text: Option<String>,
    pub discount_percent: Option<u32>,
    pub positive_review_pct: Option<f64>,
    pub total_reviews: Option<u32>,
    pub current_players: Option<u32>,
    pub capsule_url: Option<String>,
    pub store_screenshot_urls: Vec<String>,
    pub tags: Vec<String>,
    pub multiplayer_modes: Vec<String>,
    pub review_snippets: Vec<ReviewSnippet>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamGameSnapshotEnrichment {
    Discovery,
    Sync,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamAppListPreview {
    pub apps: Vec<SteamAppListItem>,
    #[serde(alias = "last_appid")]
    pub last_appid: Option<u32>,
    #[serde(alias = "have_more_results")]
    pub have_more_results: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteamAppListItem {
    pub appid: u32,
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct SteamAppListResponse {
    response: SteamAppListPreview,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamStoreSearchPage {
    pub apps: Vec<SteamAppListItem>,
    pub start: u32,
    pub total_count: u32,
    pub have_more_results: bool,
}

#[derive(Debug, Deserialize)]
struct SteamStoreSearchResponse {
    success: Option<serde_json::Value>,
    results_html: String,
    total_count: u32,
    start: Option<u32>,
}

impl SteamStoreSearchResponse {
    fn is_success(&self) -> bool {
        match self.success.as_ref() {
            None => true,
            Some(serde_json::Value::Bool(value)) => *value,
            Some(serde_json::Value::Number(value)) => value.as_u64().unwrap_or_default() == 1,
            Some(serde_json::Value::String(value)) => matches!(value.as_str(), "1" | "true"),
            Some(_) => false,
        }
    }

    fn into_page(self, requested_count: u32) -> SteamStoreSearchPage {
        let start = self.start.unwrap_or(0);
        let apps = parse_store_search_apps_from_html(&self.results_html);
        let have_more_results =
            !apps.is_empty() && start.saturating_add(requested_count) < self.total_count;

        SteamStoreSearchPage {
            apps,
            start,
            total_count: self.total_count,
            have_more_results,
        }
    }
}

pub async fn fetch_game_snapshot(
    client: &Client,
    appid: u32,
    country: &str,
    language: &str,
    enrichment: SteamGameSnapshotEnrichment,
) -> Result<SteamGameSnapshot> {
    let details = fetch_app_details(client, appid, country, language)
        .await
        .with_context(|| format!("fetch appdetails for appid {appid}"))?;
    let enrichment_plan = snapshot_enrichment_plan(enrichment, language);
    let (review_summary, mut snippets, current_players) =
        if should_fetch_review_and_player_enrichment(details.as_ref()) {
            let summary_future = async {
                if !enrichment_plan.fetch_review_summary {
                    return None;
                }

                fetch_reviews(client, appid, review_summary_query())
                    .await
                    .ok()
            };
            let snippets_future = async {
                if !enrichment_plan.fetch_review_snippets {
                    return Vec::new();
                }

                let mut snippets = fetch_reviews(
                    client,
                    appid,
                    review_snippet_query(language, PRIMARY_REVIEW_SNIPPET_COUNT),
                )
                .await
                .map(|reviews| reviews.snippets)
                .unwrap_or_default();
                if prefers_chinese_reviews(language)
                    && snippets.len() < PRIMARY_REVIEW_SNIPPET_COUNT as usize
                {
                    let english_fallback = fetch_reviews(
                        client,
                        appid,
                        review_snippet_query("english", ENGLISH_REVIEW_FALLBACK_COUNT),
                    )
                    .await
                    .map(|reviews| reviews.snippets)
                    .unwrap_or_default();
                    snippets = merge_review_snippets(
                        snippets,
                        english_fallback,
                        PRIMARY_REVIEW_SNIPPET_COUNT as usize,
                    );
                }
                if snippets.is_empty() && enrichment_plan.fetch_review_snippets_fallback_all {
                    snippets = fetch_reviews(
                        client,
                        appid,
                        review_snippet_query("all", PRIMARY_REVIEW_SNIPPET_COUNT),
                    )
                    .await
                    .map(|reviews| reviews.snippets)
                    .unwrap_or_default();
                }

                snippets
            };
            let players_future = async {
                if !enrichment_plan.fetch_current_players {
                    return None;
                }

                fetch_current_players(client, appid).await.ok()
            };
            tokio::join!(summary_future, snippets_future, players_future)
        } else {
            (None, Vec::new(), None)
        };
    let (positive_review_pct, total_reviews) = review_metrics_from_summary(review_summary.as_ref());

    let detail_tags = details
        .as_ref()
        .map(|details| details.tags())
        .unwrap_or_default();
    let release_date = details
        .as_ref()
        .and_then(|details| details.release_date.as_ref())
        .and_then(|release| parse_steam_date(release.date.as_deref()));
    let release_date_text = details
        .as_ref()
        .and_then(|details| details.release_date.as_ref())
        .and_then(|release| release.date.clone());
    let multiplayer_modes = details
        .as_ref()
        .map(|details| details.multiplayer_modes())
        .unwrap_or_default();
    let store_screenshot_urls = details
        .as_ref()
        .map(|details| details.store_screenshot_urls())
        .unwrap_or_default();
    let has_demo = details
        .as_ref()
        .map(|details| details.has_demo())
        .unwrap_or(false);
    let is_demo = details
        .as_ref()
        .and_then(|details| details.type_field.as_deref())
        .is_some_and(|kind| kind.eq_ignore_ascii_case("demo"));

    Ok(SteamGameSnapshot {
        name: details.as_ref().and_then(|details| details.name.clone()),
        short_description: details
            .as_ref()
            .and_then(|details| details.short_description.clone())
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty()),
        release_date: release_date.clone(),
        release_date_text: release_date_text.clone(),
        release_state: details.as_ref().map(|details| {
            infer_store_release_state(
                details.release_date.as_ref(),
                release_date.as_deref(),
                release_date_text.as_deref(),
            )
        }),
        demo_status: DemoStatus::from_parts(is_demo, has_demo),
        supported_languages: details
            .as_ref()
            .and_then(|details| details.supported_languages.as_deref())
            .map(parse_supported_languages)
            .filter(|languages| !languages.is_empty()),
        is_adult_content: details.as_ref().map(|details| details.is_adult_content()),
        price_text: details
            .as_ref()
            .and_then(|details| details.price_text())
            .filter(|text| !text.trim().is_empty()),
        discount_percent: details
            .as_ref()
            .and_then(|details| details.price_overview.as_ref())
            .and_then(|price| price.discount_percent),
        positive_review_pct,
        total_reviews,
        current_players,
        capsule_url: details
            .as_ref()
            .and_then(|details| details.header_image.clone()),
        store_screenshot_urls,
        tags: detail_tags,
        multiplayer_modes,
        review_snippets: std::mem::take(&mut snippets),
    })
}

pub async fn fetch_app_list_preview(
    client: &Client,
    steam_api_key: &str,
    max_results: u32,
    last_appid: Option<u32>,
) -> Result<SteamAppListPreview> {
    let mut query = vec![
        ("key", steam_api_key.to_string()),
        ("include_games", "true".to_string()),
        ("include_dlc", "false".to_string()),
        ("include_software", "false".to_string()),
        ("include_videos", "false".to_string()),
        ("include_hardware", "false".to_string()),
        ("max_results", max_results.to_string()),
    ];
    if let Some(last_appid) = last_appid {
        query.push(("last_appid", last_appid.to_string()));
    }

    let mut failures = Vec::new();
    for endpoint in steam_app_list_endpoints() {
        match fetch_app_list_preview_from_endpoint(client, *endpoint, &query).await {
            Ok(preview) => return Ok(preview),
            Err(error) => failures.push(error.to_string()),
        }
    }

    Err(anyhow!(
        "无法读取 Steam AppList。{}。请检查 Steam Web API Key 是否有效，以及当前网络代理、DNS、TLS/证书或防火墙设置。",
        failures.join("；")
    ))
}

pub async fn fetch_store_search_candidates(
    client: &Client,
    start: u32,
    count: u32,
    language: &str,
) -> Result<SteamStoreSearchPage> {
    let query = store_search_candidate_query(start, count, language);
    let count = count.clamp(1, 100);

    let response = client
        .get("https://store.steampowered.com/search/results/")
        .timeout(STEAM_STORE_SEARCH_REQUEST_TIMEOUT)
        .query(&query)
        .send()
        .await
        .map_err(|error| {
            anyhow!(
                "Steam Store Search：发送请求失败（{}）",
                describe_reqwest_error(&error)
            )
        })?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("Steam Store Search：Steam 返回 HTTP {status}。"));
    }

    let response = response
        .json::<SteamStoreSearchResponse>()
        .await
        .map_err(|error| {
            anyhow!(
                "Steam Store Search：解析响应失败（{}）",
                describe_reqwest_error(&error)
            )
        })?;
    if !response.is_success() {
        return Err(anyhow!("Steam Store Search：Steam 返回失败状态。"));
    }

    let page = response.into_page(count);
    if page.start != start {
        return Err(anyhow!(
            "Steam Store Search：分页响应异常，请求 start={start}，Steam 返回 start={}.",
            page.start
        ));
    }

    Ok(page)
}

fn store_search_candidate_query(start: u32, count: u32, language: &str) -> Vec<(String, String)> {
    let mut query = vec![
        ("query".to_string(), String::new()),
        ("start".to_string(), start.to_string()),
        ("count".to_string(), count.clamp(1, 100).to_string()),
        ("dynamic_data".to_string(), String::new()),
        ("sort_by".to_string(), "Released_DESC".to_string()),
        ("category1".to_string(), "998".to_string()),
        ("category3".to_string(), "1".to_string()),
        ("infinite".to_string(), "1".to_string()),
        ("force_infinite".to_string(), "1".to_string()),
        ("json".to_string(), "1".to_string()),
        ("ndl".to_string(), "1".to_string()),
    ];
    let language = language.trim();
    if !language.is_empty() && !language.eq_ignore_ascii_case("all") {
        query.push(("supportedlang".to_string(), language.to_string()));
    }

    query
}

async fn fetch_app_list_preview_from_endpoint(
    client: &Client,
    endpoint: SteamAppListEndpoint,
    query: &[(&str, String)],
) -> Result<SteamAppListPreview> {
    let response = client
        .get(endpoint.url)
        .timeout(STEAM_APP_LIST_REQUEST_TIMEOUT)
        .query(&query)
        .send()
        .await
        .map_err(|error| {
            anyhow!(
                "{}：发送请求失败（{}）",
                endpoint.label,
                describe_reqwest_error(&error)
            )
        })?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!(
            "{}：Steam 返回 HTTP {}。如果是 401/403，请重新确认 Steam Web API Key 或接口权限。",
            endpoint.label,
            status
        ));
    }

    let response = response
        .json::<SteamAppListResponse>()
        .await
        .map_err(|error| {
            anyhow!(
                "{}：解析响应失败（{}）",
                endpoint.label,
                describe_reqwest_error(&error)
            )
        })?;

    Ok(response.response)
}

fn parse_store_search_apps_from_html(html: &str) -> Vec<SteamAppListItem> {
    let mut apps = Vec::new();
    let mut seen = HashSet::new();

    for chunk in html.split("<a ") {
        let Some(appid) = extract_store_search_appid(chunk) else {
            continue;
        };
        if !seen.insert(appid) {
            continue;
        }

        let name = extract_store_search_title(chunk)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| format!("Steam App {appid}"));
        apps.push(SteamAppListItem { appid, name });
    }

    apps
}

fn extract_store_search_appid(chunk: &str) -> Option<u32> {
    parse_digits_after(chunk, "data-ds-appid=\"").or_else(|| parse_digits_after(chunk, "/app/"))
}

fn extract_store_search_title(chunk: &str) -> Option<String> {
    let marker = "<span class=\"title\">";
    let start = chunk.find(marker)? + marker.len();
    let rest = &chunk[start..];
    let end = rest.find("</span>")?;
    Some(decode_basic_html_entities(
        strip_html_tags(&rest[..end]).trim(),
    ))
}

fn parse_digits_after(haystack: &str, marker: &str) -> Option<u32> {
    let start = haystack.find(marker)? + marker.len();
    let digits: String = haystack[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u32>().ok()
}

fn decode_basic_html_entities(raw: &str) -> String {
    raw.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn describe_reqwest_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return "请求超时".to_string();
    }
    if error.is_connect() {
        return "网络连接失败，可能是代理、DNS、TLS/证书或防火墙问题".to_string();
    }
    if error.is_decode() {
        return "响应 JSON 解析失败".to_string();
    }

    let message = redact_steam_api_keys(&error.to_string());
    if message.trim().is_empty() {
        "未知请求错误".to_string()
    } else {
        message
    }
}

fn redact_steam_api_keys(message: &str) -> String {
    let mut output = String::with_capacity(message.len());
    let mut rest = message;

    while let Some(index) = rest.find("key=") {
        output.push_str(&rest[..index]);
        output.push_str("key=<redacted>");

        let after_key = &rest[index + "key=".len()..];
        let next_boundary = after_key
            .find(|ch: char| matches!(ch, '&' | ')' | ' ' | '\n' | '\r' | '\t'))
            .unwrap_or(after_key.len());
        rest = &after_key[next_boundary..];
    }

    output.push_str(rest);
    output
}

async fn fetch_app_details(
    client: &Client,
    appid: u32,
    country: &str,
    language: &str,
) -> Result<Option<AppDetails>> {
    let query = [
        ("appids", appid.to_string()),
        ("cc", country.to_string()),
        ("l", language.to_string()),
        ("filters", APP_DETAILS_FILTERS.to_string()),
    ];

    for attempt in 1..=APP_DETAILS_MAX_ATTEMPTS {
        let response = client
            .get("https://store.steampowered.com/api/appdetails")
            .query(&query)
            .send()
            .await;

        match response {
            Ok(response) => {
                let status = response.status();
                if !status.is_success() {
                    if attempt < APP_DETAILS_MAX_ATTEMPTS && is_retryable_steam_status(status) {
                        tokio::time::sleep(app_details_retry_delay(attempt)).await;
                        continue;
                    }
                    return Err(anyhow!(
                        "Steam appdetails for appid {appid}: Steam returned HTTP {status}"
                    ));
                }

                let response = response
                    .json::<HashMap<String, AppDetailsEnvelope>>()
                    .await
                    .map_err(|error| {
                        anyhow!(
                            "Steam appdetails for appid {appid}: parse response failed ({})",
                            describe_reqwest_error(&error)
                        )
                    })?;

                return Ok(response.get(&appid.to_string()).and_then(|entry| {
                    if entry.success {
                        entry.data.clone()
                    } else {
                        None
                    }
                }));
            }
            Err(error) => {
                if attempt < APP_DETAILS_MAX_ATTEMPTS && is_retryable_steam_transport_error(&error)
                {
                    tokio::time::sleep(app_details_retry_delay(attempt)).await;
                    continue;
                }
                return Err(anyhow!(
                    "Steam appdetails for appid {appid}: request failed ({})",
                    describe_reqwest_error(&error)
                ));
            }
        }
    }

    Err(anyhow!(
        "Steam appdetails for appid {appid}: exhausted retries unexpectedly"
    ))
}

fn is_retryable_steam_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn is_retryable_steam_transport_error(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect() || error.is_request()
}

fn app_details_retry_delay(attempt: u8) -> Duration {
    let multiplier = 1u64 << u32::from(attempt.saturating_sub(1));
    Duration::from_millis(APP_DETAILS_RETRY_BASE_DELAY_MS.saturating_mul(multiplier))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReviewQuery<'a> {
    filter: &'a str,
    language: &'a str,
    review_type: &'a str,
    purchase_type: &'a str,
    num_per_page: u32,
}

fn review_summary_query() -> ReviewQuery<'static> {
    ReviewQuery {
        filter: "all",
        language: "all",
        review_type: "all",
        purchase_type: "all",
        num_per_page: 1,
    }
}

fn review_snippet_query(language: &str, num_per_page: u32) -> ReviewQuery<'_> {
    ReviewQuery {
        filter: "toprated",
        language,
        review_type: "all",
        purchase_type: "all",
        num_per_page,
    }
}

fn review_metrics_from_summary(summary: Option<&ReviewFetch>) -> (Option<f64>, Option<u32>) {
    let Some(summary) = summary else {
        return (None, None);
    };

    let derived_total = match (summary.total_positive, summary.total_negative) {
        (Some(positive), Some(negative)) => positive.checked_add(negative),
        _ => None,
    };
    let total_reviews = summary.total_reviews.or(derived_total);
    let positive_review_pct = match (summary.total_positive, total_reviews) {
        (Some(positive), Some(total)) if total > 0 => {
            Some((positive as f64 / total as f64) * 100.0)
        }
        _ => None,
    };

    (positive_review_pct, total_reviews)
}

fn prefers_chinese_reviews(language: &str) -> bool {
    matches!(
        language.trim().to_ascii_lowercase().as_str(),
        "schinese" | "tchinese" | "simplified chinese" | "traditional chinese"
    )
}

fn merge_review_snippets(
    primary: Vec<ReviewSnippet>,
    fallback: Vec<ReviewSnippet>,
    max_items: usize,
) -> Vec<ReviewSnippet> {
    let mut merged = Vec::with_capacity(primary.len().min(max_items));
    let mut seen = HashSet::new();

    for snippet in primary.into_iter().chain(fallback) {
        let dedupe_key = snippet.review.trim().to_string();
        if dedupe_key.is_empty() || !seen.insert(dedupe_key) {
            continue;
        }
        merged.push(snippet);
        if merged.len() >= max_items {
            break;
        }
    }

    merged
}

async fn fetch_reviews(client: &Client, appid: u32, query: ReviewQuery<'_>) -> Result<ReviewFetch> {
    let url = format!("https://store.steampowered.com/appreviews/{appid}");
    let response = client
        .get(url)
        .query(&[
            ("json", "1".to_string()),
            ("filter", query.filter.to_string()),
            ("language", query.language.to_string()),
            ("review_type", query.review_type.to_string()),
            ("purchase_type", query.purchase_type.to_string()),
            ("num_per_page", query.num_per_page.to_string()),
            ("cursor", "*".to_string()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<AppReviewsResponse>()
        .await?;

    let snippets = response
        .reviews
        .unwrap_or_default()
        .into_iter()
        .filter_map(|review| {
            let text = review.review?;
            if text.trim().is_empty() {
                return None;
            }
            Some(ReviewSnippet {
                voted_up: review.voted_up.unwrap_or(false),
                review: normalize_review_text(&text),
                playtime_hours: review
                    .author
                    .and_then(|author| author.playtime_forever)
                    .map(|minutes| (minutes as f64 / 60.0 * 10.0).round() / 10.0),
            })
        })
        .collect();

    Ok(ReviewFetch {
        total_reviews: response.query_summary.total_reviews,
        total_positive: response.query_summary.total_positive,
        total_negative: response.query_summary.total_negative,
        snippets,
    })
}

async fn fetch_current_players(client: &Client, appid: u32) -> Result<u32> {
    #[derive(Debug, Deserialize)]
    struct Response {
        response: PlayerCountBody,
    }

    #[derive(Debug, Deserialize)]
    struct PlayerCountBody {
        player_count: u32,
    }

    let response = client
        .get("https://api.steampowered.com/ISteamUserStats/GetNumberOfCurrentPlayers/v1/")
        .query(&[("appid", appid.to_string())])
        .send()
        .await?
        .error_for_status()?
        .json::<Response>()
        .await
        .with_context(|| format!("decode current players for appid {appid}"))?;

    Ok(response.response.player_count)
}

fn parse_steam_date(date: Option<&str>) -> Option<String> {
    let date = date?.trim();
    let lower = date.to_ascii_lowercase();
    if date.is_empty()
        || lower.contains("coming soon")
        || lower.contains("to be announced")
        || lower == "tba"
    {
        return None;
    }

    Date::parse(date, SHORT_STEAM_DATE)
        .or_else(|_| Date::parse(date, LONG_STEAM_DATE))
        .ok()
        .and_then(|date| date.format(ISO_DATE).ok())
}

fn normalize_review_text(text: &str) -> String {
    let mut normalized = text.replace(['\r', '\n', '\t'], " ");
    while normalized.contains("  ") {
        normalized = normalized.replace("  ", " ");
    }
    normalized.chars().take(420).collect()
}

fn infer_store_release_state(
    release_date: Option<&ReleaseDate>,
    parsed_release_date: Option<&str>,
    release_date_text: Option<&str>,
) -> StoreReleaseState {
    let text = release_date_text.unwrap_or_default().trim();
    let lower = text.to_ascii_lowercase();
    let coming_soon = release_date
        .and_then(|release| release.coming_soon)
        .unwrap_or(false);

    if parsed_release_date.is_some() {
        return if coming_soon {
            StoreReleaseState::Upcoming
        } else {
            StoreReleaseState::Released
        };
    }
    if lower.contains("to be announced") || lower == "tba" {
        return StoreReleaseState::Tba;
    }
    if coming_soon || lower.contains("coming soon") {
        return StoreReleaseState::Tba;
    }

    StoreReleaseState::Released
}

fn parse_supported_languages(raw: &str) -> Vec<String> {
    let cleaned = strip_html_tags(raw)
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace('*', " ");
    let lower = cleaned.to_ascii_lowercase();
    let relevant = if let Some(index) = lower.find("languages with full audio support") {
        &cleaned[..index]
    } else {
        cleaned.as_str()
    };

    let mut languages = Vec::new();
    for language in relevant
        .split([',', '\n', ';'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let Some(normalized) = normalize_supported_language(language) else {
            continue;
        };
        if !languages.contains(&normalized) {
            languages.push(normalized);
        }
    }

    languages
}

fn normalize_supported_language(raw: &str) -> Option<String> {
    let normalized = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(['-', '•'])
        .trim()
        .to_ascii_lowercase();

    if normalized.is_empty() {
        return None;
    }

    let canonical = match normalized.as_str() {
        "english" => "english",
        "simplified chinese" | "schinese" => "schinese",
        "traditional chinese" | "tchinese" => "tchinese",
        "japanese" => "japanese",
        "korean" => "korean",
        _ => normalized.as_str(),
    };

    Some(canonical.to_string())
}

fn strip_html_tags(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut inside_tag = false;

    for ch in raw.chars() {
        match ch {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => output.push(ch),
            _ => {}
        }
    }

    output
}

fn deserialize_optional_string<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value)),
        Some(serde_json::Value::Number(value)) => Ok(Some(value.to_string())),
        Some(serde_json::Value::Bool(value)) => Ok(Some(value.to_string())),
        Some(value) => Err(serde::de::Error::custom(format!(
            "expected string-compatible scalar, got {value}"
        ))),
    }
}

fn deserialize_optional_u32<'de, D>(deserializer: D) -> std::result::Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(value)) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid u32 value: {value}"))),
        Some(serde_json::Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            trimmed
                .parse::<u32>()
                .map(Some)
                .map_err(|_| serde::de::Error::custom(format!("invalid u32 string: {trimmed}")))
        }
        Some(value) => Err(serde::de::Error::custom(format!(
            "expected u32-compatible scalar, got {value}"
        ))),
    }
}

#[derive(Debug, Clone)]
struct ReviewFetch {
    total_reviews: Option<u32>,
    total_positive: Option<u32>,
    total_negative: Option<u32>,
    snippets: Vec<ReviewSnippet>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SnapshotEnrichmentPlan {
    fetch_review_summary: bool,
    fetch_review_snippets: bool,
    fetch_review_snippets_fallback_all: bool,
    fetch_current_players: bool,
}

fn snapshot_enrichment_plan(
    enrichment: SteamGameSnapshotEnrichment,
    language: &str,
) -> SnapshotEnrichmentPlan {
    match enrichment {
        SteamGameSnapshotEnrichment::Discovery => SnapshotEnrichmentPlan {
            fetch_review_summary: true,
            fetch_review_snippets: false,
            fetch_review_snippets_fallback_all: false,
            fetch_current_players: false,
        },
        SteamGameSnapshotEnrichment::Sync => SnapshotEnrichmentPlan {
            fetch_review_summary: false,
            fetch_review_snippets: false,
            fetch_review_snippets_fallback_all: false,
            fetch_current_players: false,
        },
        SteamGameSnapshotEnrichment::Full => SnapshotEnrichmentPlan {
            fetch_review_summary: true,
            fetch_review_snippets: true,
            fetch_review_snippets_fallback_all: !language.eq_ignore_ascii_case("all"),
            fetch_current_players: true,
        },
    }
}

#[derive(Debug, Clone, Deserialize)]
struct AppDetailsEnvelope {
    success: bool,
    data: Option<AppDetails>,
}

#[derive(Debug, Clone, Deserialize)]
struct AppDetails {
    #[serde(rename = "type")]
    type_field: Option<String>,
    name: Option<String>,
    short_description: Option<String>,
    header_image: Option<String>,
    screenshots: Option<Vec<StoreScreenshot>>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    required_age: Option<String>,
    is_free: Option<bool>,
    supported_languages: Option<String>,
    price_overview: Option<PriceOverview>,
    release_date: Option<ReleaseDate>,
    categories: Option<Vec<StoreDescriptor>>,
    genres: Option<Vec<StoreDescriptor>>,
    demos: Option<Vec<DemoEntry>>,
    content_descriptors: Option<ContentDescriptors>,
}

impl AppDetails {
    fn has_demo(&self) -> bool {
        self.demos.as_ref().is_some_and(|demos| !demos.is_empty())
    }

    fn tags(&self) -> Vec<String> {
        self.genres
            .clone()
            .unwrap_or_default()
            .into_iter()
            .chain(self.categories.clone().unwrap_or_default())
            .filter_map(|descriptor| descriptor.description)
            .take(12)
            .collect()
    }

    fn multiplayer_modes(&self) -> Vec<String> {
        self.categories
            .clone()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|descriptor| descriptor.multiplayer_label())
            .collect()
    }

    fn is_adult_content(&self) -> bool {
        self.required_age
            .as_deref()
            .and_then(|value| value.parse::<u32>().ok())
            .is_some_and(|age| age >= 18)
            || self
                .content_descriptors
                .as_ref()
                .is_some_and(ContentDescriptors::contains_adult_marker)
    }

    fn price_text(&self) -> Option<String> {
        if self.is_free.unwrap_or(false) {
            return Some("Free To Play".to_string());
        }

        self.price_overview
            .as_ref()
            .and_then(|price| price.final_formatted.clone())
    }

    fn store_screenshot_urls(&self) -> Vec<String> {
        self.screenshots
            .clone()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|screenshot| {
                let thumbnail = screenshot
                    .path_thumbnail
                    .filter(|url| !url.trim().is_empty());
                let full = screenshot.path_full.filter(|url| !url.trim().is_empty());
                thumbnail.or(full)
            })
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty())
            .collect()
    }
}

fn should_fetch_review_and_player_enrichment(details: Option<&AppDetails>) -> bool {
    details
        .map(AppDetails::multiplayer_modes)
        .is_some_and(|modes| !modes.is_empty())
}

#[derive(Debug, Clone, Deserialize)]
struct ReleaseDate {
    date: Option<String>,
    coming_soon: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
struct StoreDescriptor {
    #[serde(default, deserialize_with = "deserialize_optional_u32")]
    id: Option<u32>,
    description: Option<String>,
}

impl StoreDescriptor {
    fn multiplayer_label(&self) -> Option<String> {
        let description = self
            .description
            .as_deref()
            .map(str::trim)
            .filter(|description| !description.is_empty());

        if self.id.is_some_and(is_multiplayer_category_id) {
            return Some(
                description
                    .map(ToOwned::to_owned)
                    .or_else(|| {
                        self.id
                            .and_then(multiplayer_category_fallback_label)
                            .map(str::to_string)
                    })
                    .expect("known multiplayer category ids should always have fallback labels"),
            );
        }

        description
            .filter(|description| is_multiplayer_category_description(description))
            .map(ToOwned::to_owned)
    }
}

fn is_multiplayer_category_id(category_id: u32) -> bool {
    matches!(
        category_id,
        1 | 9 | 24 | 27 | 36 | 37 | 38 | 39 | 47 | 48 | 49
    )
}

fn multiplayer_category_fallback_label(category_id: u32) -> Option<&'static str> {
    match category_id {
        1 => Some("Multi-player"),
        9 => Some("Co-op"),
        24 => Some("Shared/Split Screen"),
        27 => Some("Cross-Platform Multiplayer"),
        36 => Some("Online PvP"),
        37 => Some("Shared/Split Screen PvP"),
        38 => Some("Online Co-op"),
        39 => Some("Shared/Split Screen Co-op"),
        47 => Some("LAN PvP"),
        48 => Some("LAN Co-op"),
        49 => Some("PvP"),
        _ => None,
    }
}

fn is_multiplayer_category_description(description: &str) -> bool {
    let lower = description.to_ascii_lowercase();
    lower.contains("multi-player")
        || lower.contains("multiplayer")
        || lower.contains("co-op")
        || lower.contains("lan")
        || lower.contains("shared/split")
        || lower.contains("pvp")
        || description.contains("多人")
        || description.contains("合作")
        || description.contains("局域网")
        || description.contains("分屏")
        || description.contains("对战")
}

#[derive(Debug, Clone, Deserialize)]
struct PriceOverview {
    final_formatted: Option<String>,
    discount_percent: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
struct StoreScreenshot {
    path_thumbnail: Option<String>,
    path_full: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ContentDescriptors {
    ids: Option<Vec<u32>>,
    notes: Option<String>,
}

impl ContentDescriptors {
    fn contains_adult_marker(&self) -> bool {
        let notes = self
            .notes
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        notes.contains("adult")
            || notes.contains("sexual")
            || notes.contains("nudity")
            || notes.contains("mature")
            || (self.ids.as_ref().is_some_and(|ids| !ids.is_empty())
                && (notes.contains("violence") || notes.contains("gore")))
    }
}

#[derive(Debug, Clone, Deserialize)]
struct DemoEntry {
    #[allow(dead_code)]
    appid: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
struct AppReviewsResponse {
    query_summary: QuerySummary,
    reviews: Option<Vec<SteamReview>>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuerySummary {
    total_reviews: Option<u32>,
    total_positive: Option<u32>,
    total_negative: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
struct SteamReview {
    review: Option<String>,
    voted_up: Option<bool>,
    author: Option<ReviewAuthor>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReviewAuthor {
    playtime_forever: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_app_list_endpoints_use_documented_partner_host_before_public_fallback() {
        let endpoints = steam_app_list_endpoints();

        assert_eq!(
            endpoints.first().map(|endpoint| endpoint.url),
            Some("https://partner.steam-api.com/IStoreService/GetAppList/v1/")
        );
        assert!(endpoints
            .iter()
            .any(|endpoint| endpoint.url
                == "https://api.steampowered.com/IStoreService/GetAppList/v1/"));
    }

    #[test]
    fn steam_app_list_query_keeps_secret_key_out_of_display_text() {
        let message = redact_steam_api_keys(
            "error sending request for url (https://api.steampowered.com/IStoreService/GetAppList/v1/?key=SECRET123&include_games=true)",
        );

        assert!(!message.contains("SECRET123"));
        assert!(message.contains("key=<redacted>"));
        assert!(message.contains("include_games=true"));
    }

    #[test]
    fn steam_app_list_response_accepts_steam_snake_case_cursor_fields() {
        let raw = r#"
        {
            "response": {
                "apps": [
                    { "appid": 3020, "name": "First App" },
                    { "appid": 3490, "name": "Last App" }
                ],
                "last_appid": 3490,
                "have_more_results": true
            }
        }
        "#;

        let response: SteamAppListResponse =
            serde_json::from_str(raw).expect("decode Steam AppList response");

        assert_eq!(response.response.last_appid, Some(3490));
        assert_eq!(response.response.have_more_results, Some(true));
        assert_eq!(response.response.apps.len(), 2);
    }

    #[test]
    fn store_search_html_parser_preserves_order_and_deduplicates_apps() {
        let html = r#"
        <a href="https://store.steampowered.com/app/3266580/Apes_Warfare/"
           data-ds-appid="3266580" class="search_result_row">
            <span class="title">Apes &amp; Warfare</span>
        </a>
        <a href="https://store.steampowered.com/app/4038550/Hamam/"
           data-ds-appid="4038550" class="search_result_row">
            <span class="title">Hamam: The Steaming Backrooms</span>
        </a>
        <a href="https://store.steampowered.com/app/3266580/Apes_Warfare/"
           data-ds-appid="3266580" class="search_result_row">
            <span class="title">Duplicate Should Be Ignored</span>
        </a>
        "#;

        let apps = parse_store_search_apps_from_html(html);

        assert_eq!(
            apps,
            vec![
                SteamAppListItem {
                    appid: 3_266_580,
                    name: "Apes & Warfare".to_string(),
                },
                SteamAppListItem {
                    appid: 4_038_550,
                    name: "Hamam: The Steaming Backrooms".to_string(),
                },
            ]
        );
    }

    #[test]
    fn store_search_page_reports_more_results_from_start_and_total() {
        let response = SteamStoreSearchResponse {
            success: Some(serde_json::json!(1)),
            results_html: String::new(),
            total_count: 105,
            start: Some(100),
        };

        let page = response.into_page(25);

        assert_eq!(page.start, 100);
        assert_eq!(page.total_count, 105);
        assert!(!page.have_more_results);
    }

    #[test]
    fn store_search_candidate_query_prefilters_to_multiplayer_games() {
        let query = store_search_candidate_query(50, 500, "english");

        assert!(query.contains(&("category1".to_string(), "998".to_string())));
        assert!(query.contains(&("category3".to_string(), "1".to_string())));
        assert!(query.contains(&("sort_by".to_string(), "Released_DESC".to_string())));
        assert!(query.contains(&("start".to_string(), "50".to_string())));
        assert!(query.contains(&("count".to_string(), "100".to_string())));
        assert!(query.contains(&("supportedlang".to_string(), "english".to_string())));
    }

    #[test]
    fn review_summary_query_uses_all_languages_and_minimal_payload() {
        let query = review_summary_query();

        assert_eq!(query.filter, "all");
        assert_eq!(query.language, "all");
        assert_eq!(query.review_type, "all");
        assert_eq!(query.purchase_type, "all");
        assert_eq!(query.num_per_page, 1);
    }

    #[test]
    fn review_snippet_query_uses_toprated_reviews_and_requested_count() {
        let query = review_snippet_query("schinese", 8);

        assert_eq!(query.filter, "toprated");
        assert_eq!(query.language, "schinese");
        assert_eq!(query.review_type, "all");
        assert_eq!(query.purchase_type, "all");
        assert_eq!(query.num_per_page, 8);
    }

    #[test]
    fn chinese_review_preference_detects_common_aliases() {
        assert!(prefers_chinese_reviews("schinese"));
        assert!(prefers_chinese_reviews("tchinese"));
        assert!(prefers_chinese_reviews("simplified chinese"));
        assert!(!prefers_chinese_reviews("english"));
        assert!(!prefers_chinese_reviews("all"));
    }

    #[test]
    fn merge_review_snippets_preserves_order_and_deduplicates_reviews() {
        let merged = merge_review_snippets(
            vec![
                ReviewSnippet {
                    voted_up: true,
                    review: "中文热评 1".to_string(),
                    playtime_hours: Some(10.0),
                },
                ReviewSnippet {
                    voted_up: false,
                    review: "中文热评 2".to_string(),
                    playtime_hours: Some(4.5),
                },
            ],
            vec![
                ReviewSnippet {
                    voted_up: true,
                    review: "中文热评 2".to_string(),
                    playtime_hours: Some(5.0),
                },
                ReviewSnippet {
                    voted_up: true,
                    review: "Fallback English".to_string(),
                    playtime_hours: Some(8.0),
                },
            ],
            3,
        );

        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].review, "中文热评 1");
        assert_eq!(merged[1].review, "中文热评 2");
        assert_eq!(merged[2].review, "Fallback English");
    }

    #[test]
    fn review_metrics_use_global_positive_and_negative_totals() {
        let summary = ReviewFetch {
            total_reviews: Some(125),
            total_positive: Some(100),
            total_negative: Some(25),
            snippets: Vec::new(),
        };

        let (positive_pct, total_reviews) = review_metrics_from_summary(Some(&summary));

        assert_eq!(positive_pct, Some(80.0));
        assert_eq!(total_reviews, Some(125));
    }

    #[test]
    fn review_metrics_fall_back_to_positive_plus_negative_total() {
        let summary = ReviewFetch {
            total_reviews: None,
            total_positive: Some(7),
            total_negative: Some(3),
            snippets: Vec::new(),
        };

        let (positive_pct, total_reviews) = review_metrics_from_summary(Some(&summary));

        assert_eq!(positive_pct, Some(70.0));
        assert_eq!(total_reviews, Some(10));
    }

    #[test]
    fn app_details_without_multiplayer_modes_skip_expensive_enrichment() {
        let details = AppDetails {
            type_field: Some("game".to_string()),
            name: Some("Quiet Solo Game".to_string()),
            short_description: None,
            header_image: None,
            screenshots: None,
            required_age: None,
            is_free: Some(false),
            supported_languages: None,
            price_overview: None,
            release_date: None,
            categories: Some(vec![StoreDescriptor {
                id: None,
                description: Some("Single-player".to_string()),
            }]),
            genres: None,
            demos: None,
            content_descriptors: None,
        };

        assert!(!should_fetch_review_and_player_enrichment(Some(&details)));
    }

    #[test]
    fn app_details_with_multiplayer_modes_fetch_expensive_enrichment() {
        let details = AppDetails {
            type_field: Some("game".to_string()),
            name: Some("Co-op Signal".to_string()),
            short_description: None,
            header_image: None,
            screenshots: None,
            required_age: None,
            is_free: Some(false),
            supported_languages: None,
            price_overview: None,
            release_date: None,
            categories: Some(vec![StoreDescriptor {
                id: Some(38),
                description: Some("Online Co-op".to_string()),
            }]),
            genres: None,
            demos: None,
            content_descriptors: None,
        };

        assert!(should_fetch_review_and_player_enrichment(Some(&details)));
    }

    #[test]
    fn app_details_collect_thumbnail_screenshots_for_store_gallery() {
        let details = AppDetails {
            type_field: Some("game".to_string()),
            name: Some("Gallery Test".to_string()),
            short_description: None,
            header_image: None,
            screenshots: Some(vec![
                StoreScreenshot {
                    path_thumbnail: Some("https://cdn.example.test/thumb-1.jpg".to_string()),
                    path_full: None,
                },
                StoreScreenshot {
                    path_thumbnail: Some("  ".to_string()),
                    path_full: Some("https://cdn.example.test/full-blank-thumb.jpg".to_string()),
                },
                StoreScreenshot {
                    path_thumbnail: None,
                    path_full: Some("https://cdn.example.test/full-2.jpg".to_string()),
                },
            ]),
            required_age: None,
            is_free: Some(false),
            supported_languages: None,
            price_overview: None,
            release_date: None,
            categories: None,
            genres: None,
            demos: None,
            content_descriptors: None,
        };

        assert_eq!(
            details.store_screenshot_urls(),
            vec![
                "https://cdn.example.test/thumb-1.jpg".to_string(),
                "https://cdn.example.test/full-blank-thumb.jpg".to_string(),
                "https://cdn.example.test/full-2.jpg".to_string(),
            ]
        );
    }

    #[test]
    fn sync_enrichment_skips_expensive_secondary_requests() {
        let plan = snapshot_enrichment_plan(SteamGameSnapshotEnrichment::Sync, "schinese");

        assert!(!plan.fetch_review_summary);
        assert!(!plan.fetch_review_snippets);
        assert!(!plan.fetch_review_snippets_fallback_all);
        assert!(!plan.fetch_current_players);
    }

    #[test]
    fn app_details_filters_include_screenshots() {
        assert!(APP_DETAILS_FILTERS
            .split(',')
            .any(|value| value == "screenshots"));
    }

    #[test]
    fn app_details_accepts_numeric_required_age_from_store_api() {
        let json = r#"
        {
            "success": true,
            "data": {
                "type": "game",
                "name": "Counter-Strike 2",
                "required_age": 0,
                "is_free": true,
                "categories": [
                    { "description": "Multi-player" }
                ]
            }
        }
        "#;

        let envelope: AppDetailsEnvelope =
            serde_json::from_str(json).expect("decode appdetails envelope");
        let details = envelope.data.expect("details should be present");

        assert_eq!(details.required_age.as_deref(), Some("0"));
        assert_eq!(details.name.as_deref(), Some("Counter-Strike 2"));
        assert_eq!(details.multiplayer_modes(), vec!["Multi-player"]);
    }

    #[test]
    fn app_details_extract_multiplayer_modes_from_chinese_localized_categories() {
        let json = r#"
        {
            "success": true,
            "data": {
                "type": "game",
                "name": "Chinese Multiplayer Test",
                "categories": [
                    { "id": 1, "description": "多人" },
                    { "id": 38, "description": "在线合作" },
                    { "id": 29, "description": "Steam 集换式卡牌" }
                ]
            }
        }
        "#;

        let envelope: AppDetailsEnvelope =
            serde_json::from_str(json).expect("decode localized appdetails envelope");
        let details = envelope.data.expect("details should be present");

        assert_eq!(details.multiplayer_modes(), vec!["多人", "在线合作"]);
    }

    #[test]
    fn app_details_with_chinese_multiplayer_modes_fetch_expensive_enrichment() {
        let json = r#"
        {
            "success": true,
            "data": {
                "type": "game",
                "name": "Chinese Enrichment Test",
                "categories": [
                    { "id": 1, "description": "多人" },
                    { "id": 48, "description": "局域网合作" }
                ]
            }
        }
        "#;

        let envelope: AppDetailsEnvelope =
            serde_json::from_str(json).expect("decode localized appdetails envelope");
        let details = envelope.data.expect("details should be present");

        assert!(should_fetch_review_and_player_enrichment(Some(&details)));
    }

    #[test]
    fn app_details_retry_backoff_grows_exponentially() {
        assert_eq!(app_details_retry_delay(1), Duration::from_millis(350));
        assert_eq!(app_details_retry_delay(2), Duration::from_millis(700));
        assert_eq!(app_details_retry_delay(3), Duration::from_millis(1_400));
    }

    #[test]
    fn app_details_retryable_statuses_match_steam_rate_limit_and_server_errors() {
        assert!(is_retryable_steam_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_steam_status(StatusCode::BAD_GATEWAY));
        assert!(!is_retryable_steam_status(StatusCode::NOT_FOUND));
    }

    #[test]
    fn app_details_accept_mixed_descriptor_id_types() {
        let json = r#"
        {
            "success": true,
            "data": {
                "type": "game",
                "name": "Descriptor Type Test",
                "categories": [
                    { "id": 1, "description": "多人" },
                    { "id": 38, "description": "在线合作" }
                ],
                "genres": [
                    { "id": "3", "description": "角色扮演" },
                    { "id": "23", "description": "动作" }
                ]
            }
        }
        "#;

        let envelope: AppDetailsEnvelope =
            serde_json::from_str(json).expect("decode mixed descriptor id envelope");
        let details = envelope.data.expect("details should be present");

        assert_eq!(details.multiplayer_modes(), vec!["多人", "在线合作"]);
        assert_eq!(
            details.tags(),
            vec![
                "角色扮演".to_string(),
                "动作".to_string(),
                "多人".to_string(),
                "在线合作".to_string(),
            ]
        );
    }

    #[test]
    fn discovery_snapshot_enrichment_avoids_live_metrics_requests() {
        let plan = snapshot_enrichment_plan(SteamGameSnapshotEnrichment::Discovery, "schinese");

        assert!(plan.fetch_review_summary);
        assert!(!plan.fetch_review_snippets);
        assert!(!plan.fetch_review_snippets_fallback_all);
        assert!(!plan.fetch_current_players);
    }

    #[test]
    fn full_snapshot_enrichment_keeps_snippets_and_player_count() {
        let plan = snapshot_enrichment_plan(SteamGameSnapshotEnrichment::Full, "schinese");

        assert!(plan.fetch_review_summary);
        assert!(plan.fetch_review_snippets);
        assert!(plan.fetch_review_snippets_fallback_all);
        assert!(plan.fetch_current_players);
    }

    #[test]
    fn full_snapshot_enrichment_skips_all_language_fallback_when_already_global() {
        let plan = snapshot_enrichment_plan(SteamGameSnapshotEnrichment::Full, "all");

        assert!(plan.fetch_review_summary);
        assert!(plan.fetch_review_snippets);
        assert!(!plan.fetch_review_snippets_fallback_all);
        assert!(plan.fetch_current_players);
    }
}
