//! Steam Store `appdetails` volatile adapter spike (calendar / Demo relations).
//!
//! This endpoint is **not** a documented stable Web API contract. It is isolated
//! here so M2 can swap or disable it without touching official adapters.

use serde::Deserialize;
use serde_json::Value;

use crate::error::SourceError;
use crate::proposal::{
    AppRelationProposal, AppTypeProposal, RelationTypeProposal, ReleaseStateProposal,
    SourceStability, StoreDetailsProposal, StoreMovieProposal, StorePriceProposal,
    StoreScreenshotProposal,
};
use crate::raw::RawResponse;

pub const ADAPTER_VERSION: &str = "store-appdetails-0.3.0";
pub const SOURCE_NAME: &str = "steam_store_appdetails";
/// Default storefront region for price snapshots (ISO country, lower-case query).
pub const DEFAULT_STORE_COUNTRY: &str = "cn";
pub const DEFAULT_STORE_LANGUAGE: &str = "schinese";

/// Max screenshots retained from a single appdetails response (Steam order).
pub const MAX_SCREENSHOTS: usize = 20;
/// Max movies/trailers retained from a single appdetails response (Steam order).
pub const MAX_MOVIES: usize = 5;
/// Max Unicode scalar values kept for movie titles after trim.
const MAX_MOVIE_TITLE_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreDetailsRequest {
    pub app_id: u32,
    /// Two-letter Steam store country code controlling price_overview currency.
    pub country_code: String,
    /// Steam locale used for localized store text.
    pub language: String,
}

impl StoreDetailsRequest {
    pub fn new(app_id: u32) -> Self {
        Self {
            app_id,
            country_code: DEFAULT_STORE_COUNTRY.into(),
            language: DEFAULT_STORE_LANGUAGE.into(),
        }
    }

    pub fn with_locale(
        app_id: u32,
        country_code: impl Into<String>,
        language: impl Into<String>,
    ) -> Result<Self, SourceError> {
        let country_code = country_code.into().trim().to_ascii_lowercase();
        let language = language.into().trim().to_ascii_lowercase();
        if country_code.len() != 2 || !country_code.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err(SourceError::Config {
                message: "store country must be a two-letter ISO code".into(),
            });
        }
        if language.is_empty()
            || !language
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(SourceError::Config {
                message: "store language must be a non-empty Steam locale".into(),
            });
        }
        Ok(Self {
            app_id,
            country_code,
            language,
        })
    }

    pub fn path_and_query(&self) -> String {
        format!(
            "/api/appdetails?appids={}&l={}&cc={}",
            self.app_id, self.language, self.country_code
        )
    }
}

#[derive(Debug, Deserialize)]
struct AppDetailsNode {
    success: bool,
    #[serde(default)]
    data: Option<AppDetailsData>,
}

#[derive(Debug, Deserialize)]
struct AppDetailsData {
    #[serde(default)]
    #[serde(rename = "type")]
    app_type: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    steam_appid: Option<u32>,
    #[serde(default)]
    is_free: Option<bool>,
    #[serde(default)]
    platforms: Option<PlatformsDto>,
    #[serde(default)]
    supported_languages: Option<String>,
    #[serde(default)]
    short_description: Option<String>,
    #[serde(default)]
    header_image: Option<String>,
    #[serde(default)]
    developers: Option<Vec<String>>,
    #[serde(default)]
    publishers: Option<Vec<String>>,
    #[serde(default)]
    categories: Option<Vec<CategoryDto>>,
    #[serde(default)]
    genres: Option<Vec<GenreDto>>,
    #[serde(default)]
    release_date: Option<ReleaseDateDto>,
    #[serde(default)]
    demos: Option<Vec<DemoDto>>,
    #[serde(default)]
    fullgame: Option<FullGameDto>,
    #[serde(default)]
    price_overview: Option<PriceOverviewDto>,
    #[serde(default)]
    packages: Option<Vec<i64>>,
    /// Missing → None (preserve prior on ingest). Explicit `[]` → Some(empty).
    #[serde(default)]
    screenshots: Option<Vec<ScreenshotDto>>,
    /// Missing → None (preserve prior on ingest). Explicit `[]` → Some(empty).
    #[serde(default)]
    movies: Option<Vec<MovieDto>>,
}

#[derive(Debug, Deserialize)]
struct ScreenshotDto {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    path_thumbnail: Option<String>,
    #[serde(default)]
    path_full: Option<String>,
}

/// Nested MP4 map from older Steam movie payloads (`480` / `max`).
#[derive(Debug, Deserialize)]
struct MovieMp4Dto {
    #[serde(default)]
    #[serde(rename = "480")]
    p480: Option<String>,
    #[serde(default)]
    max: Option<String>,
}

/// Nested WebM map from older Steam movie payloads (accepted but not persisted).
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct MovieWebmDto {
    #[serde(default)]
    #[serde(rename = "480")]
    p480: Option<String>,
    #[serde(default)]
    max: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MovieDto {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    highlight: Option<bool>,
    #[serde(default)]
    hls_h264: Option<String>,
    #[serde(default)]
    dash_h264: Option<String>,
    #[serde(default)]
    dash_av1: Option<String>,
    #[serde(default)]
    mp4: Option<MovieMp4Dto>,
    #[serde(default)]
    webm: Option<MovieWebmDto>,
}

/// Aggregate counters for media structure drift / rejected URLs (no raw URL dumps).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MediaParseStats {
    pub screenshots_rejected: u32,
    pub movies_rejected: u32,
    pub urls_rejected: u32,
    pub screenshots_deduped: u32,
    pub movies_deduped: u32,
    pub screenshots_truncated: u32,
    pub movies_truncated: u32,
}

#[derive(Debug, Deserialize)]
struct PriceOverviewDto {
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    initial: Option<i64>,
    #[serde(default)]
    #[serde(rename = "final")]
    final_price: Option<i64>,
    #[serde(default)]
    discount_percent: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct PlatformsDto {
    #[serde(default)]
    windows: bool,
    #[serde(default)]
    mac: bool,
    #[serde(default)]
    linux: bool,
}

impl PlatformsDto {
    fn normalized(self) -> Vec<String> {
        [
            ("windows", self.windows),
            ("mac", self.mac),
            ("linux", self.linux),
        ]
        .into_iter()
        .filter(|(_, supported)| *supported)
        .map(|(name, _)| name.to_owned())
        .collect()
    }
}

#[derive(Debug, Deserialize)]
struct CategoryDto {
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GenreDto {
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReleaseDateDto {
    #[serde(default)]
    coming_soon: Option<bool>,
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DemoDto {
    #[serde(default)]
    appid: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct FullGameDto {
    #[serde(default)]
    appid: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoreDetailsParseResult {
    pub details: StoreDetailsProposal,
    pub relations: Vec<AppRelationProposal>,
    /// Aggregate media rejections observed while parsing this response.
    ///
    /// Callers can surface these counters without logging raw third-party URLs.
    pub media_stats: MediaParseStats,
}

pub fn parse_store_details(
    request: &StoreDetailsRequest,
    raw: &RawResponse,
) -> Result<StoreDetailsParseResult, SourceError> {
    let root: Value = raw.parse_json()?;
    let key = request.app_id.to_string();
    let node_value = root
        .get(&key)
        .ok_or_else(|| SourceError::invalid_structure(format!("missing top-level key {key}")))?;

    let node: AppDetailsNode =
        serde_json::from_value(node_value.clone()).map_err(SourceError::json_parse)?;

    if !node.success {
        return Err(SourceError::NotFound { entity_key: key });
    }

    let data = node
        .data
        .ok_or_else(|| SourceError::invalid_structure("success=true but data object is missing"))?;

    if data
        .steam_appid
        .is_some_and(|response_id| response_id != request.app_id)
    {
        return Err(SourceError::invalid_structure(format!(
            "steam_appid does not match requested app {}",
            request.app_id
        )));
    }
    let app_id = request.app_id;
    let categories: Vec<String> = data
        .categories
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| c.description)
        .collect();
    let genres: Vec<String> = data
        .genres
        .unwrap_or_default()
        .into_iter()
        .filter_map(|g| g.description)
        .collect();

    let multiplayer_category_hints = categories
        .iter()
        .filter(|c| is_multiplayer_hint(c))
        .cloned()
        .collect();

    let release_date_observed = data.release_date.is_some();
    let coming_soon = data.release_date.as_ref().and_then(|r| r.coming_soon);
    let release_date_raw = data
        .release_date
        .as_ref()
        .and_then(|r| r.date.clone())
        .filter(|d| !d.trim().is_empty());
    let (release_date, release_date_precision) =
        normalize_release_date(release_date_raw.as_deref());

    let release_state = match coming_soon {
        Some(true) => ReleaseStateProposal::ComingSoon,
        Some(false) => ReleaseStateProposal::Released,
        None => ReleaseStateProposal::Unknown,
    };

    let demo_app_ids: Vec<u32> = data
        .demos
        .unwrap_or_default()
        .into_iter()
        .filter_map(|d| d.appid)
        .filter(|id| *id != 0)
        .collect();

    let fullgame_app_id = data
        .fullgame
        .and_then(|f| f.appid)
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|id| *id != 0);

    let app_type = data
        .app_type
        .as_deref()
        .map(AppTypeProposal::from_steam_type)
        .unwrap_or(AppTypeProposal::Unknown);
    let platforms = data.platforms.map(PlatformsDto::normalized);
    let supported_languages = data
        .supported_languages
        .as_deref()
        .map(normalize_supported_languages);
    let package_id = data
        .packages
        .as_ref()
        .and_then(|packages| packages.first())
        .map(|id| id.to_string());
    let price = normalize_price(
        data.is_free,
        data.price_overview,
        &request.country_code,
        package_id,
    );

    let mut media_stats = MediaParseStats::default();
    let screenshots = data.screenshots.and_then(|items| {
        let explicitly_empty = items.is_empty();
        let normalized = normalize_screenshots(items, &mut media_stats);
        // Only an explicit upstream [] is authoritative empty. If Steam sent
        // entries but every one was rejected, treat the field as unusable so
        // ingest preserves the last known-good snapshot.
        (explicitly_empty || !normalized.is_empty()).then_some(normalized)
    });
    let movies = data.movies.and_then(|items| {
        let explicitly_empty = items.is_empty();
        let normalized = normalize_movies(items, &mut media_stats);
        (explicitly_empty || !normalized.is_empty()).then_some(normalized)
    });

    let details = StoreDetailsProposal {
        app_id,
        country_code: request.country_code.trim().to_ascii_uppercase(),
        language: request.language.clone(),
        name: data.name,
        app_type,
        release_state,
        release_date_raw,
        release_date,
        release_date_precision,
        release_date_observed,
        is_free: data.is_free,
        platforms,
        supported_languages,
        price,
        coming_soon,
        categories,
        genres,
        developers: data.developers.unwrap_or_default(),
        publishers: data.publishers.unwrap_or_default(),
        short_description: data.short_description,
        header_image_url: normalize_image_url(data.header_image.as_deref()),
        screenshots,
        movies,
        demo_app_ids: demo_app_ids.clone(),
        fullgame_app_id,
        multiplayer_category_hints,
        content_hash: raw.content_hash.clone(),
        source: SOURCE_NAME,
        stability: SourceStability::ApprovedVolatile,
        adapter_version: ADAPTER_VERSION,
    };

    let mut relations = Vec::new();
    for demo_id in demo_app_ids {
        relations.push(AppRelationProposal {
            source_app_id: demo_id,
            target_app_id: app_id,
            relation_type: RelationTypeProposal::DemoOf,
            confidence: 0.7,
            stability: SourceStability::ApprovedVolatile,
            adapter_version: ADAPTER_VERSION,
        });
    }
    if let Some(full_id) = fullgame_app_id {
        let relation_type = match details.app_type {
            AppTypeProposal::Playtest => RelationTypeProposal::PlaytestOf,
            _ => RelationTypeProposal::DemoOf,
        };
        relations.push(AppRelationProposal {
            source_app_id: app_id,
            target_app_id: full_id,
            relation_type,
            confidence: 0.75,
            stability: SourceStability::ApprovedVolatile,
            adapter_version: ADAPTER_VERSION,
        });
    }

    Ok(StoreDetailsParseResult {
        details,
        relations,
        media_stats,
    })
}

fn normalize_price(
    is_free: Option<bool>,
    overview: Option<PriceOverviewDto>,
    country_code: &str,
    package_id: Option<String>,
) -> Option<StorePriceProposal> {
    let country = country_code.trim().to_ascii_uppercase();
    if country.is_empty() {
        return None;
    }
    if is_free == Some(true) {
        let currency = currency_for_country(&country)?;
        return Some(StorePriceProposal {
            country_code: country,
            currency: currency.into(),
            initial_price_minor: Some(0),
            final_price_minor: Some(0),
            discount_percent: Some(0),
            is_purchasable: Some(true),
            package_id,
        });
    }
    if let Some(overview) = overview {
        let currency = overview
            .currency
            .map(|value| value.trim().to_ascii_uppercase())
            .filter(|value| !value.is_empty())?;
        return Some(StorePriceProposal {
            country_code: country,
            currency,
            initial_price_minor: overview.initial,
            final_price_minor: overview.final_price,
            discount_percent: overview.discount_percent,
            is_purchasable: Some(true),
            package_id,
        });
    }
    // No currency was returned. Persisting a guessed USD marker here would make
    // regional budget filters silently consume invalid data.
    None
}

/// Kind of Steam media URL for host allowlisting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SteamMediaUrlKind {
    /// Header images, screenshots, movie posters.
    Image,
    /// Progressive / HLS / DASH playable streams.
    VideoPlay,
}

/// Shared Steam media URL validator used for covers, screenshots, posters, and streams.
///
/// Rules:
/// - absolute `https://` only
/// - no userinfo, explicit port, empty host, empty path, backslash, or control chars
/// - image hosts: `steamstatic.com` and subdomains
/// - video play hosts: `video.akamai.steamstatic.com` (plus other `*.steamstatic.com` video CDNs if needed)
fn normalize_steam_media_url(raw: Option<&str>, kind: SteamMediaUrlKind) -> Option<String> {
    let value = raw?.trim();
    if value.is_empty() {
        return None;
    }
    if value.chars().any(|ch| ch.is_control() || ch == '\\') {
        return None;
    }
    let authority_and_path = value.strip_prefix("https://")?;
    if authority_and_path.contains('@') {
        return None;
    }
    let (authority, path) = authority_and_path.split_once('/')?;
    if authority.is_empty() || path.is_empty() {
        return None;
    }
    // Reject explicit ports and malformed authorities.
    if authority.contains(':') {
        return None;
    }
    let host_ok = match kind {
        SteamMediaUrlKind::Image => {
            authority == "steamstatic.com" || authority.ends_with(".steamstatic.com")
        }
        SteamMediaUrlKind::VideoPlay => is_steam_video_play_host(authority),
    };
    if !host_ok {
        return None;
    }
    Some(value.to_owned())
}

fn is_steam_video_play_host(authority: &str) -> bool {
    // Primary verified host from live appdetails (2026-07-24).
    if authority == "video.akamai.steamstatic.com" {
        return true;
    }
    // Other steamstatic video CDNs only when the subdomain signals video delivery.
    authority.ends_with(".steamstatic.com")
        && (authority.starts_with("video.") || authority.contains(".video."))
}

fn normalize_image_url(raw: Option<&str>) -> Option<String> {
    normalize_steam_media_url(raw, SteamMediaUrlKind::Image)
}

fn normalize_video_play_url(raw: Option<&str>) -> Option<String> {
    normalize_steam_media_url(raw, SteamMediaUrlKind::VideoPlay)
}

fn normalize_movie_title(raw: Option<String>) -> Option<String> {
    let trimmed = raw?.trim().to_owned();
    if trimmed.is_empty() {
        return None;
    }
    let truncated: String = trimmed.chars().take(MAX_MOVIE_TITLE_CHARS).collect();
    Some(truncated)
}

fn normalize_screenshots(
    items: Vec<ScreenshotDto>,
    stats: &mut MediaParseStats,
) -> Vec<StoreScreenshotProposal> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if out.len() >= MAX_SCREENSHOTS {
            stats.screenshots_truncated += 1;
            continue;
        }
        let Some(id) = item.id else {
            stats.screenshots_rejected += 1;
            continue;
        };
        let source_id = id.to_string();
        let thumb = normalize_image_url(item.path_thumbnail.as_deref());
        let full = normalize_image_url(item.path_full.as_deref());
        let (Some(thumbnail_url), Some(full_url)) = (thumb, full) else {
            stats.screenshots_rejected += 1;
            stats.urls_rejected += 1;
            continue;
        };
        // Do not let an invalid first occurrence suppress a later usable row
        // carrying the same Steam source id.
        if !seen.insert(source_id.clone()) {
            stats.screenshots_deduped += 1;
            continue;
        }
        let sort_order = out.len() as u16;
        out.push(StoreScreenshotProposal {
            source_id,
            sort_order,
            thumbnail_url,
            full_url,
        });
    }
    out
}

fn normalize_movies(items: Vec<MovieDto>, stats: &mut MediaParseStats) -> Vec<StoreMovieProposal> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if out.len() >= MAX_MOVIES {
            stats.movies_truncated += 1;
            continue;
        }
        let Some(id) = item.id else {
            stats.movies_rejected += 1;
            continue;
        };
        let source_id = id.to_string();
        let Some(poster_url) = normalize_image_url(item.thumbnail.as_deref()) else {
            stats.movies_rejected += 1;
            stats.urls_rejected += 1;
            continue;
        };

        let mut url_rejections = 0u32;
        let mp4_raw = item
            .mp4
            .as_ref()
            .and_then(|m| m.max.as_deref().or(m.p480.as_deref()));
        let mp4_url = match mp4_raw {
            Some(raw) => match normalize_video_play_url(Some(raw)) {
                Some(url) => Some(url),
                None => {
                    url_rejections += 1;
                    None
                }
            },
            None => None,
        };
        let hls_h264_url = match item.hls_h264.as_deref() {
            Some(raw) => match normalize_video_play_url(Some(raw)) {
                Some(url) => Some(url),
                None => {
                    url_rejections += 1;
                    None
                }
            },
            None => None,
        };
        let dash_h264_url = match item.dash_h264.as_deref() {
            Some(raw) => match normalize_video_play_url(Some(raw)) {
                Some(url) => Some(url),
                None => {
                    url_rejections += 1;
                    None
                }
            },
            None => None,
        };
        // dash_av1 / webm are observed but not persisted in the current contract.
        let _ = (item.dash_av1, item.webm);

        stats.urls_rejected += url_rejections;
        if mp4_url.is_none() && hls_h264_url.is_none() && dash_h264_url.is_none() {
            stats.movies_rejected += 1;
            continue;
        }
        // As with screenshots, only a usable row claims the source id.
        if !seen.insert(source_id.clone()) {
            stats.movies_deduped += 1;
            continue;
        }

        let sort_order = out.len() as u16;
        out.push(StoreMovieProposal {
            source_id,
            sort_order,
            title: normalize_movie_title(item.name),
            poster_url,
            highlight: item.highlight.unwrap_or(false),
            mp4_url,
            hls_h264_url,
            dash_h264_url,
        });
    }
    out
}

fn currency_for_country(country: &str) -> Option<&'static str> {
    match country {
        "CN" => Some("CNY"),
        "US" => Some("USD"),
        "GB" => Some("GBP"),
        "JP" => Some("JPY"),
        "KR" => Some("KRW"),
        "CA" => Some("CAD"),
        "AU" => Some("AUD"),
        "DE" | "ES" | "FR" | "IT" | "NL" | "PT" => Some("EUR"),
        _ => None,
    }
}

fn normalize_supported_languages(raw: &str) -> Vec<String> {
    const LANGUAGES: &[(&str, &str)] = &[
        ("Simplified Chinese", "schinese"),
        ("简体中文", "schinese"),
        ("Traditional Chinese", "tchinese"),
        ("繁体中文", "tchinese"),
        ("Portuguese - Brazil", "brazilian"),
        ("葡萄牙语 - 巴西", "brazilian"),
        ("Brazilian Portuguese", "brazilian"),
        ("巴西葡萄牙语", "brazilian"),
        ("Spanish - Latin America", "latam"),
        ("西班牙语 - 拉丁美洲", "latam"),
        ("Arabic", "arabic"),
        ("阿拉伯语", "arabic"),
        ("Bulgarian", "bulgarian"),
        ("保加利亚语", "bulgarian"),
        ("Czech", "czech"),
        ("捷克语", "czech"),
        ("Danish", "danish"),
        ("丹麦语", "danish"),
        ("Dutch", "dutch"),
        ("荷兰语", "dutch"),
        ("English", "english"),
        ("英语", "english"),
        ("Finnish", "finnish"),
        ("芬兰语", "finnish"),
        ("French", "french"),
        ("法语", "french"),
        ("German", "german"),
        ("德语", "german"),
        ("Greek", "greek"),
        ("希腊语", "greek"),
        ("Hungarian", "hungarian"),
        ("匈牙利语", "hungarian"),
        ("Indonesian", "indonesian"),
        ("印度尼西亚语", "indonesian"),
        ("Italian", "italian"),
        ("意大利语", "italian"),
        ("Japanese", "japanese"),
        ("日语", "japanese"),
        ("Korean", "koreana"),
        ("韩语", "koreana"),
        ("Norwegian", "norwegian"),
        ("挪威语", "norwegian"),
        ("Polish", "polish"),
        ("波兰语", "polish"),
        ("Portuguese", "portuguese"),
        ("葡萄牙语", "portuguese"),
        ("Romanian", "romanian"),
        ("罗马尼亚语", "romanian"),
        ("Russian", "russian"),
        ("俄语", "russian"),
        ("Spanish - Spain", "spanish"),
        ("西班牙语 - 西班牙", "spanish"),
        ("Spanish", "spanish"),
        ("西班牙语", "spanish"),
        ("Swedish", "swedish"),
        ("瑞典语", "swedish"),
        ("Thai", "thai"),
        ("泰语", "thai"),
        ("Turkish", "turkish"),
        ("土耳其语", "turkish"),
        ("Ukrainian", "ukrainian"),
        ("乌克兰语", "ukrainian"),
        ("Vietnamese", "vietnamese"),
        ("越南语", "vietnamese"),
    ];
    let tokens: Vec<String> = raw
        .split(',')
        .map(normalize_language_token)
        .filter(|token| !token.is_empty())
        .collect();
    let mut found = Vec::new();
    for (label, code) in LANGUAGES {
        let label = label.to_ascii_lowercase();
        if tokens.iter().any(|token| token == &label) && !found.iter().any(|item| item == code) {
            found.push((*code).to_owned());
        }
    }
    found
}

fn normalize_language_token(token: &str) -> String {
    let mut stripped = String::with_capacity(token.len());
    let mut in_tag = false;
    for ch in token.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => stripped.push(ch),
            _ => {}
        }
    }
    stripped
        .trim()
        .trim_end_matches('*')
        .trim()
        .to_ascii_lowercase()
}

fn normalize_release_date(raw: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return (None, None);
    };

    if let Some((year, month, day)) = parse_iso_day(raw) {
        return (
            Some(format!("{year:04}-{month:02}-{day:02}")),
            Some("day".into()),
        );
    }

    if let Some((year, month, day)) = parse_chinese_day(raw) {
        return (
            Some(format!("{year:04}-{month:02}-{day:02}")),
            Some("day".into()),
        );
    }

    if parse_chinese_month(raw).is_some() {
        return (None, Some("month".into()));
    }

    if parse_chinese_year(raw).is_some() {
        return (None, Some("year".into()));
    }

    let cleaned = raw.replace(',', " ");
    let parts: Vec<_> = cleaned.split_whitespace().collect();
    if parts.len() == 3 {
        let parsed = if let (Ok(day), Some(month), Ok(year)) = (
            parts[0].parse::<u32>(),
            month_number(parts[1]),
            parts[2].parse::<i32>(),
        ) {
            Some((year, month, day))
        } else if let (Some(month), Ok(day), Ok(year)) = (
            month_number(parts[0]),
            parts[1].parse::<u32>(),
            parts[2].parse::<i32>(),
        ) {
            Some((year, month, day))
        } else {
            None
        };
        if let Some((year, month, day)) = parsed.filter(|&(y, m, d)| valid_day(y, m, d)) {
            return (
                Some(format!("{year:04}-{month:02}-{day:02}")),
                Some("day".into()),
            );
        }
    }

    if parts.len() == 2 {
        let quarter = parts[0]
            .strip_prefix('Q')
            .or_else(|| parts[0].strip_prefix('q'))
            .and_then(|value| value.parse::<u8>().ok());
        if quarter.is_some_and(|value| (1..=4).contains(&value)) && parts[1].parse::<i32>().is_ok()
        {
            return (None, Some("quarter".into()));
        }
        if month_number(parts[0]).is_some() && parts[1].parse::<i32>().is_ok() {
            return (None, Some("month".into()));
        }
    }

    if raw.parse::<i32>().is_ok() {
        return (None, Some("year".into()));
    }
    (None, Some("tba".into()))
}

fn parse_iso_day(value: &str) -> Option<(i32, u32, u32)> {
    if value.len() != 10
        || !value.is_ascii()
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
    {
        return None;
    }
    let year = value[0..4].parse().ok()?;
    let month = value[5..7].parse().ok()?;
    let day = value[8..10].parse().ok()?;
    valid_day(year, month, day).then_some((year, month, day))
}

fn compact_chinese_date(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
        .trim_start_matches("预计")
        .to_owned()
}

fn parse_chinese_day(value: &str) -> Option<(i32, u32, u32)> {
    let compact = compact_chinese_date(value);
    let (year, rest) = compact.split_once('年')?;
    let (month, day) = rest.split_once('月')?;
    let day = day.strip_suffix('日').or_else(|| day.strip_suffix('号'))?;
    let (year, month, day) = (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    valid_day(year, month, day).then_some((year, month, day))
}

fn parse_chinese_month(value: &str) -> Option<(i32, u32)> {
    let compact = compact_chinese_date(value);
    let (year, month) = compact.split_once('年')?;
    let month = month.strip_suffix('月')?;
    let (year, month) = (year.parse().ok()?, month.parse().ok()?);
    ((1..=12).contains(&month)).then_some((year, month))
}

fn parse_chinese_year(value: &str) -> Option<i32> {
    let compact = compact_chinese_date(value);
    compact.strip_suffix('年')?.parse().ok()
}

fn month_number(value: &str) -> Option<u32> {
    match value.to_ascii_lowercase().as_str() {
        "jan" | "january" => Some(1),
        "feb" | "february" => Some(2),
        "mar" | "march" => Some(3),
        "apr" | "april" => Some(4),
        "may" => Some(5),
        "jun" | "june" => Some(6),
        "jul" | "july" => Some(7),
        "aug" | "august" => Some(8),
        "sep" | "sept" | "september" => Some(9),
        "oct" | "october" => Some(10),
        "nov" | "november" => Some(11),
        "dec" | "december" => Some(12),
        _ => None,
    }
}

fn valid_day(year: i32, month: u32, day: u32) -> bool {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

fn is_multiplayer_hint(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    lower.contains("multi")
        || lower.contains("co-op")
        || lower.contains("coop")
        || lower.contains("pvp")
        || lower.contains("online")
        || lower.contains("mmo")
        || lower.contains("lan")
}

/// Static feasibility summary used by docs and runtime diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreAdapterFeasibility {
    pub endpoint: &'static str,
    pub stability: SourceStability,
    pub supports_release_calendar: bool,
    pub supports_demo_relation: bool,
    pub requires_web_api_key: bool,
    pub recommended_fallback: &'static str,
}

pub const STORE_APPDETAILS_FEASIBILITY: StoreAdapterFeasibility = StoreAdapterFeasibility {
    endpoint: "https://store.steampowered.com/api/appdetails",
    stability: SourceStability::ApprovedVolatile,
    supports_release_calendar: true,
    supports_demo_relation: true,
    requires_web_api_key: false,
    recommended_fallback: "human curation + release_events table when structure changes",
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::RawResponse;

    fn fixture(name: &str) -> RawResponse {
        let body = match name {
            "game" => include_bytes!("../fixtures/store_appdetails_game.json").to_vec(),
            "demo" => include_bytes!("../fixtures/store_appdetails_demo.json").to_vec(),
            "coming_soon" => {
                include_bytes!("../fixtures/store_appdetails_coming_soon.json").to_vec()
            }
            "fail" => include_bytes!("../fixtures/store_appdetails_fail.json").to_vec(),
            other => panic!("unknown fixture {other}"),
        };
        RawResponse::validate(200, body, Some("application/json".into()), 1024 * 1024).unwrap()
    }

    #[test]
    fn parses_released_game_with_demo_relation() {
        let request = StoreDetailsRequest::with_locale(892970, "US", "english").unwrap();
        let result = parse_store_details(&request, &fixture("game")).unwrap();
        assert_eq!(result.details.app_id, 892970);
        assert_eq!(result.details.language, "english");
        assert_eq!(result.details.release_state, ReleaseStateProposal::Released);
        assert_eq!(result.details.release_date.as_deref(), Some("2021-02-02"));
        assert_eq!(
            result.details.release_date_precision.as_deref(),
            Some("day")
        );
        assert_eq!(result.details.stability, SourceStability::ApprovedVolatile);
        assert_eq!(
            result.details.header_image_url.as_deref(),
            Some(
                "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/892970/header.jpg?t=1"
            )
        );
        assert!(!result.details.multiplayer_category_hints.is_empty());
        let price = result.details.price.expect("price_overview");
        assert_eq!(price.country_code, "US");
        assert_eq!(price.currency, "USD");
        assert_eq!(price.initial_price_minor, Some(1999));
        assert_eq!(price.final_price_minor, Some(999));
        assert_eq!(price.discount_percent, Some(50));
        assert_eq!(price.package_id.as_deref(), Some("123456"));
        assert!(result.relations.iter().any(|r| {
            matches!(r.relation_type, RelationTypeProposal::DemoOf) && r.target_app_id == 892970
        }));
    }

    #[test]
    fn free_game_synthesizes_zero_price_snapshot() {
        let request = StoreDetailsRequest::new(570);
        let raw = RawResponse::validate(
            200,
            br#"{"570":{"success":true,"data":{"type":"game","name":"Dota 2","steam_appid":570,"is_free":true}}}"#
                .to_vec(),
            Some("application/json".into()),
            1024,
        )
        .unwrap();
        let result = parse_store_details(&request, &raw).unwrap();
        let price = result.details.price.expect("free price");
        assert_eq!(price.final_price_minor, Some(0));
        assert_eq!(price.currency, "CNY");
        assert_eq!(price.is_purchasable, Some(true));
    }

    #[test]
    fn missing_price_overview_does_not_invent_a_currency() {
        let request = StoreDetailsRequest::new(42);
        let raw = RawResponse::validate(
            200,
            br#"{"42":{"success":true,"data":{"type":"game","name":"Soon","steam_appid":42,"is_free":false}}}"#
                .to_vec(),
            Some("application/json".into()),
            1024,
        )
        .unwrap();
        let result = parse_store_details(&request, &raw).unwrap();
        assert_eq!(result.details.price, None);
    }

    #[test]
    fn request_includes_country_for_price_region() {
        let request = StoreDetailsRequest::new(1);
        assert!(request.path_and_query().contains("cc=cn"));
    }

    #[test]
    fn parses_demo_fullgame_link() {
        let request = StoreDetailsRequest::new(1_888_930);
        let result = parse_store_details(&request, &fixture("demo")).unwrap();
        assert_eq!(result.details.app_type, AppTypeProposal::Demo);
        assert_eq!(result.details.fullgame_app_id, Some(892970));
        assert!(result.relations.iter().any(|r| {
            r.source_app_id == 1_888_930
                && r.target_app_id == 892970
                && matches!(r.relation_type, RelationTypeProposal::DemoOf)
        }));
    }

    #[test]
    fn parses_coming_soon_calendar_fields() {
        let request = StoreDetailsRequest::new(2_500_000);
        let result = parse_store_details(&request, &fixture("coming_soon")).unwrap();
        assert_eq!(
            result.details.release_state,
            ReleaseStateProposal::ComingSoon
        );
        assert_eq!(result.details.coming_soon, Some(true));
        assert!(result.details.release_date_raw.is_some());
        assert_eq!(result.details.release_date, None);
        assert_eq!(
            result.details.release_date_precision.as_deref(),
            Some("quarter")
        );
    }

    #[test]
    fn normalizes_chinese_storefront_release_dates() {
        let raw = RawResponse::validate(
            200,
            r#"{"42":{"success":true,"data":{"steam_appid":42,"type":"game","release_date":{"coming_soon":false,"date":"2021 年 2 月 2 日"}}}}"#
                .as_bytes()
                .to_vec(),
            Some("application/json".into()),
            4096,
        )
        .unwrap();
        let result = parse_store_details(&StoreDetailsRequest::new(42), &raw).unwrap();
        assert_eq!(result.details.release_date.as_deref(), Some("2021-02-02"));
        assert_eq!(
            result.details.release_date_precision.as_deref(),
            Some("day")
        );
    }

    #[test]
    fn unsuccessful_appdetails_is_not_found() {
        let request = StoreDetailsRequest::new(1);
        let err = parse_store_details(&request, &fixture("fail")).unwrap_err();
        assert!(matches!(err, SourceError::NotFound { .. }));
    }

    #[test]
    fn feasibility_marks_store_as_volatile() {
        assert_eq!(
            STORE_APPDETAILS_FEASIBILITY.stability,
            SourceStability::ApprovedVolatile
        );
        const {
            assert!(STORE_APPDETAILS_FEASIBILITY.supports_demo_relation);
            assert!(!STORE_APPDETAILS_FEASIBILITY.requires_web_api_key);
        }
    }

    #[test]
    fn request_uses_china_region_matching_default_cny_preferences() {
        assert_eq!(
            StoreDetailsRequest::new(42).path_and_query(),
            "/api/appdetails?appids=42&l=schinese&cc=cn"
        );
    }

    #[test]
    fn request_locale_is_configurable_and_validated() {
        let request = StoreDetailsRequest::with_locale(42, "US", "english").unwrap();
        assert_eq!(
            request.path_and_query(),
            "/api/appdetails?appids=42&l=english&cc=us"
        );
        assert!(StoreDetailsRequest::with_locale(42, "USA", "english").is_err());
        assert!(StoreDetailsRequest::with_locale(42, "US", "../bad").is_err());
    }

    #[test]
    fn response_app_id_must_match_request() {
        let raw = RawResponse::validate(
            200,
            br#"{"42":{"success":true,"data":{"steam_appid":43,"type":"game"}}}"#.to_vec(),
            Some("application/json".into()),
            1024,
        )
        .unwrap();
        let error = parse_store_details(&StoreDetailsRequest::new(42), &raw).unwrap_err();
        assert!(matches!(error, SourceError::InvalidStructure { .. }));
    }

    #[test]
    fn parses_structured_platforms_and_normalizes_languages() {
        let raw = RawResponse::validate(
            200,
            br#"{"42":{"success":true,"data":{"steam_appid":42,"type":"game","platforms":{"windows":true,"mac":false,"linux":true},"supported_languages":"English, Simplified Chinese<strong>*</strong>, Japanese"}}}"#.to_vec(),
            Some("application/json".into()),
            4096,
        )
        .unwrap();
        let result = parse_store_details(&StoreDetailsRequest::new(42), &raw).unwrap();
        assert_eq!(
            result.details.platforms,
            Some(vec!["windows".into(), "linux".into()])
        );
        assert_eq!(
            result.details.supported_languages,
            Some(vec!["schinese".into(), "english".into(), "japanese".into()])
        );
    }

    #[test]
    fn normalizes_macos_to_the_client_platform_identifier() {
        let raw = RawResponse::validate(
            200,
            br#"{"42":{"success":true,"data":{"steam_appid":42,"type":"game","platforms":{"windows":false,"mac":true,"linux":false}}}}"#.to_vec(),
            Some("application/json".into()),
            4096,
        )
        .unwrap();
        let result = parse_store_details(&StoreDetailsRequest::new(42), &raw).unwrap();
        assert_eq!(result.details.platforms, Some(vec!["mac".into()]));
    }

    #[test]
    fn normalizes_supported_languages_from_the_chinese_storefront() {
        let raw = RawResponse::validate(
            200,
            r#"{"42":{"success":true,"data":{"steam_appid":42,"type":"game","supported_languages":"英语, 简体中文, 葡萄牙语 - 巴西, 日语, 韩语"}}}"#
                .as_bytes()
                .to_vec(),
            Some("application/json".into()),
            4096,
        )
        .unwrap();
        let result = parse_store_details(&StoreDetailsRequest::new(42), &raw).unwrap();
        assert_eq!(
            result.details.supported_languages,
            Some(vec![
                "schinese".into(),
                "brazilian".into(),
                "english".into(),
                "japanese".into(),
                "koreana".into(),
            ])
        );
    }

    #[test]
    fn rejects_non_steamstatic_header_images() {
        let raw = RawResponse::validate(
            200,
            br#"{"42":{"success":true,"data":{"steam_appid":42,"type":"game","header_image":"https://example.com/header.jpg"}}}"#.to_vec(),
            Some("application/json".into()),
            4096,
        )
        .unwrap();
        let result = parse_store_details(&StoreDetailsRequest::new(42), &raw).unwrap();
        assert_eq!(result.details.header_image_url, None);
    }

    #[test]
    fn parses_screenshots_and_movies_with_order_dedup_and_url_rejects() {
        let request = StoreDetailsRequest::with_locale(892970, "US", "english").unwrap();
        let result = parse_store_details(&request, &fixture("game")).unwrap();

        let screenshots = result.details.screenshots.expect("screenshots present");
        assert_eq!(screenshots.len(), 2);
        assert_eq!(screenshots[0].source_id, "0");
        assert_eq!(screenshots[0].sort_order, 0);
        assert!(screenshots[0].thumbnail_url.contains("ss_thumb_0"));
        assert!(screenshots[0].full_url.contains("ss_full_0"));
        assert_eq!(screenshots[1].source_id, "1");
        assert_eq!(screenshots[1].sort_order, 1);

        let movies = result.details.movies.expect("movies present");
        assert_eq!(movies.len(), 2);
        assert_eq!(movies[0].source_id, "257363622");
        assert_eq!(
            movies[0].title.as_deref(),
            Some("1.0 Release Date Reveal Trailer")
        );
        assert!(movies[0].highlight);
        assert!(movies[0].mp4_url.is_none());
        assert_eq!(
            movies[0].hls_h264_url.as_deref(),
            Some("https://video.akamai.steamstatic.com/steam/apps/892970/movie_highlight.m3u8")
        );
        assert_eq!(
            movies[0].dash_h264_url.as_deref(),
            Some("https://video.akamai.steamstatic.com/steam/apps/892970/movie_highlight.mpd")
        );
        assert_eq!(movies[1].source_id, "1001");
        assert!(!movies[1].highlight);
        assert_eq!(
            movies[1].mp4_url.as_deref(),
            Some("https://video.akamai.steamstatic.com/steam/apps/892970/movie_max.mp4")
        );
        assert!(movies[1].hls_h264_url.is_none());
    }

    #[test]
    fn missing_media_fields_yield_none_not_empty_vecs() {
        let raw = RawResponse::validate(
            200,
            br#"{"42":{"success":true,"data":{"steam_appid":42,"type":"game","name":"No Media"}}}"#
                .to_vec(),
            Some("application/json".into()),
            1024,
        )
        .unwrap();
        let result = parse_store_details(&StoreDetailsRequest::new(42), &raw).unwrap();
        assert_eq!(result.details.screenshots, None);
        assert_eq!(result.details.movies, None);
    }

    #[test]
    fn explicit_empty_media_arrays_yield_some_empty() {
        let raw = RawResponse::validate(
            200,
            br#"{"42":{"success":true,"data":{"steam_appid":42,"type":"game","screenshots":[],"movies":[]}}}"#.to_vec(),
            Some("application/json".into()),
            1024,
        )
        .unwrap();
        let result = parse_store_details(&StoreDetailsRequest::new(42), &raw).unwrap();
        assert_eq!(result.details.screenshots.as_deref(), Some(&[][..]));
        assert_eq!(result.details.movies.as_deref(), Some(&[][..]));
    }

    #[test]
    fn enforces_screenshot_and_movie_caps_preserving_steam_order() {
        let mut screenshots = String::new();
        for i in 0..25 {
            if i > 0 {
                screenshots.push(',');
            }
            screenshots.push_str(&format!(
                r#"{{"id":{i},"path_thumbnail":"https://shared.akamai.steamstatic.com/t/{i}.jpg","path_full":"https://shared.akamai.steamstatic.com/f/{i}.jpg"}}"#
            ));
        }
        let mut movies = String::new();
        for i in 0..8 {
            if i > 0 {
                movies.push(',');
            }
            movies.push_str(&format!(
                r#"{{"id":{i},"name":"M{i}","thumbnail":"https://shared.akamai.steamstatic.com/p/{i}.jpg","hls_h264":"https://video.akamai.steamstatic.com/v/{i}.m3u8"}}"#
            ));
        }
        let body = format!(
            r#"{{"42":{{"success":true,"data":{{"steam_appid":42,"type":"game","screenshots":[{screenshots}],"movies":[{movies}]}}}}}}"#
        );
        let raw = RawResponse::validate(
            200,
            body.into_bytes(),
            Some("application/json".into()),
            64 * 1024,
        )
        .unwrap();
        let result = parse_store_details(&StoreDetailsRequest::new(42), &raw).unwrap();
        let shots = result.details.screenshots.expect("screenshots");
        let movs = result.details.movies.expect("movies");
        assert_eq!(shots.len(), MAX_SCREENSHOTS);
        assert_eq!(movs.len(), MAX_MOVIES);
        assert_eq!(shots[0].source_id, "0");
        assert_eq!(shots[19].source_id, "19");
        assert_eq!(movs[0].source_id, "0");
        assert_eq!(movs[4].source_id, "4");
    }

    #[test]
    fn rejects_video_play_urls_outside_steam_video_cdn() {
        let raw = RawResponse::validate(
            200,
            br#"{"42":{"success":true,"data":{"steam_appid":42,"type":"game","movies":[{"id":1,"thumbnail":"https://shared.akamai.steamstatic.com/p.jpg","mp4":{"max":"https://cdn.example.com/x.mp4"}}]}}}"#.to_vec(),
            Some("application/json".into()),
            4096,
        )
        .unwrap();
        let result = parse_store_details(&StoreDetailsRequest::new(42), &raw).unwrap();
        assert_eq!(result.details.movies, None);
        assert_eq!(result.media_stats.movies_rejected, 1);
        assert_eq!(result.media_stats.urls_rejected, 1);
    }

    #[test]
    fn nonempty_all_invalid_media_is_not_an_authoritative_empty_snapshot() {
        let raw = RawResponse::validate(
            200,
            br#"{"42":{"success":true,"data":{"steam_appid":42,"type":"game","screenshots":[{"id":1,"path_thumbnail":"https://example.com/t.jpg","path_full":"https://example.com/f.jpg"}],"movies":[{"id":2,"thumbnail":"https://shared.akamai.steamstatic.com/p.jpg","hls_h264":"https://example.com/m.m3u8"}]}}}"#.to_vec(),
            Some("application/json".into()),
            4096,
        )
        .unwrap();
        let result = parse_store_details(&StoreDetailsRequest::new(42), &raw).unwrap();
        assert_eq!(result.details.screenshots, None);
        assert_eq!(result.details.movies, None);
        assert_eq!(result.media_stats.screenshots_rejected, 1);
        assert_eq!(result.media_stats.movies_rejected, 1);
    }

    #[test]
    fn invalid_duplicate_does_not_hide_later_usable_media() {
        let raw = RawResponse::validate(
            200,
            br#"{"42":{"success":true,"data":{"steam_appid":42,"type":"game","screenshots":[{"id":1,"path_thumbnail":"https://example.com/t.jpg","path_full":"https://example.com/f.jpg"},{"id":1,"path_thumbnail":"https://shared.akamai.steamstatic.com/t.jpg","path_full":"https://shared.akamai.steamstatic.com/f.jpg"}],"movies":[{"id":2,"thumbnail":"https://shared.akamai.steamstatic.com/p.jpg","hls_h264":"https://example.com/m.m3u8"},{"id":2,"thumbnail":"https://shared.akamai.steamstatic.com/p.jpg","hls_h264":"https://video.akamai.steamstatic.com/m.m3u8"}]}}}"#.to_vec(),
            Some("application/json".into()),
            4096,
        )
        .unwrap();
        let result = parse_store_details(&StoreDetailsRequest::new(42), &raw).unwrap();
        assert_eq!(result.details.screenshots.as_ref().map(Vec::len), Some(1));
        assert_eq!(result.details.movies.as_ref().map(Vec::len), Some(1));
    }
}
