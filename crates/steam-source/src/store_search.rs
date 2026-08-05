//! Steam Store search adapter used to discover multiplayer candidates.
//!
//! The endpoint and its HTML fragment are not a documented stable contract.
//! Keep this adapter isolated, versioned, rate-limited by callers, and backed
//! by recorded parser tests.

use std::collections::HashSet;

use scraper::{Html, Selector};
use serde::Deserialize;

use crate::error::SourceError;
use crate::proposal::{AppCatalogProposal, AppTypeProposal, SourceStability};
use crate::raw::RawResponse;

pub const ADAPTER_VERSION: &str = "store-search-0.3.0";
pub const SOURCE_NAME: &str = "steam_store_search";
pub const MULTIPLAYER_CATEGORY_HINT: &str = "Multi-player";
pub const MAX_PAGE_SIZE: u32 = 100;

/// Steam store search ranking. New multiplayer titles rarely have reviews, so
/// discovery defaults to release date rather than popularity-by-reviews.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StoreSearchSort {
    #[default]
    ReleasedDesc,
    /// Upcoming multiplayer releases, paired with Steam's `comingsoon` filter.
    ReleasedAsc,
    ReviewsDesc,
}

impl StoreSearchSort {
    pub fn as_query_value(self) -> &'static str {
        match self {
            Self::ReleasedDesc => "Released_DESC",
            Self::ReleasedAsc => "Released_ASC",
            Self::ReviewsDesc => "Reviews_DESC",
        }
    }

    pub fn filter(self) -> Option<&'static str> {
        match self {
            Self::ReleasedAsc => Some("comingsoon"),
            Self::ReleasedDesc | Self::ReviewsDesc => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreSearchRequest {
    pub start: u32,
    pub count: u32,
    pub sort: StoreSearchSort,
}

impl StoreSearchRequest {
    pub fn new(start: u32, count: u32) -> Result<Self, SourceError> {
        Self::with_sort(start, count, StoreSearchSort::default())
    }

    pub fn with_sort(start: u32, count: u32, sort: StoreSearchSort) -> Result<Self, SourceError> {
        if !(1..=MAX_PAGE_SIZE).contains(&count) {
            return Err(SourceError::Config {
                message: format!("store search count must be between 1 and {MAX_PAGE_SIZE}"),
            });
        }
        Ok(Self { start, count, sort })
    }

    pub fn path_and_query(&self) -> String {
        let filter = self
            .sort
            .filter()
            .map_or(String::new(), |value| format!("&filter={value}"));
        format!(
            "/search/results/?query&start={}&count={}&sort_by={}{}&category1=998&category2=1&infinite=1&cc=CN&l=schinese&json=1",
            self.start,
            self.count,
            self.sort.as_query_value(),
            filter,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSearchCandidate {
    pub app_id: u32,
    pub name: String,
}

impl StoreSearchCandidate {
    pub fn catalog_proposal(&self) -> AppCatalogProposal {
        AppCatalogProposal {
            app_id: self.app_id,
            name: self.name.clone(),
            app_type: AppTypeProposal::Game,
            last_modified: None,
            price_change_number: None,
            source: SOURCE_NAME,
            stability: SourceStability::ApprovedVolatile,
            adapter_version: ADAPTER_VERSION,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSearchPage {
    pub candidates: Vec<StoreSearchCandidate>,
    pub start: u32,
    pub result_count: u32,
    pub total_count: u32,
    pub content_hash: String,
    pub sort: StoreSearchSort,
}

impl StoreSearchPage {
    pub fn next_start(&self) -> u32 {
        self.start.saturating_add(self.result_count)
    }

    pub fn is_complete(&self) -> bool {
        self.result_count == 0 || self.next_start() >= self.total_count
    }
}

#[derive(Debug, Deserialize)]
struct StoreSearchEnvelope {
    success: u8,
    start: u32,
    total_count: u32,
    results_html: String,
}

pub fn parse_store_search_page(
    request: &StoreSearchRequest,
    raw: &RawResponse,
) -> Result<StoreSearchPage, SourceError> {
    let envelope: StoreSearchEnvelope = raw.parse_json()?;
    if envelope.success != 1 {
        return Err(SourceError::invalid_structure(
            "store search success flag is not 1",
        ));
    }
    if envelope.start != request.start {
        return Err(SourceError::invalid_structure(format!(
            "store search start mismatch: requested {}, received {}",
            request.start, envelope.start
        )));
    }

    let fragment = Html::parse_fragment(&envelope.results_html);
    let row_selector = Selector::parse("a.search_result_row[data-ds-appid]")
        .map_err(|error| SourceError::invalid_structure(error.to_string()))?;
    let title_selector = Selector::parse("span.title")
        .map_err(|error| SourceError::invalid_structure(error.to_string()))?;

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    let mut result_count = 0_u32;
    for row in fragment.select(&row_selector) {
        result_count = result_count.saturating_add(1);
        let raw_app_id = row
            .value()
            .attr("data-ds-appid")
            .ok_or_else(|| SourceError::invalid_structure("search row is missing data-ds-appid"))?;
        let app_id = raw_app_id
            .split(',')
            .next()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|value| *value != 0)
            .ok_or_else(|| {
                SourceError::invalid_structure(format!(
                    "invalid search row data-ds-appid: {raw_app_id}"
                ))
            })?;
        let name = row
            .select(&title_selector)
            .next()
            .map(|title| title.text().collect::<String>())
            .map(|title| title.trim().to_owned())
            .filter(|title| !title.is_empty())
            .ok_or_else(|| {
                SourceError::invalid_structure(format!(
                    "search row for app {app_id} is missing a title"
                ))
            })?;

        if seen.insert(app_id) {
            candidates.push(StoreSearchCandidate { app_id, name });
        }
    }

    if result_count == 0 && request.start < envelope.total_count {
        return Err(SourceError::invalid_structure(
            "store search returned no app rows before total_count",
        ));
    }

    Ok(StoreSearchPage {
        candidates,
        start: envelope.start,
        result_count,
        total_count: envelope.total_count,
        content_hash: raw.content_hash.clone(),
        sort: request.sort,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(body: &str) -> RawResponse {
        RawResponse::validate(
            200,
            body.as_bytes().to_vec(),
            Some("application/json".into()),
            64 * 1024,
        )
        .unwrap()
    }

    #[test]
    fn parses_search_rows_with_html_entities_and_deduplicates() {
        let raw = page(
            r#"{
                "success": 1,
                "start": 0,
                "total_count": 2,
                "results_html": "<a class=\"search_result_row ds_collapse_flag\" data-ds-appid=\"548430\"><span class=\"title\">Deep Rock Galactic</span></a><a class=\"search_result_row\" data-ds-appid=\"632360,999\"><span class=\"title\">Risk &amp; Rain</span></a><a class=\"search_result_row\" data-ds-appid=\"548430\"><span class=\"title\">Duplicate</span></a>"
            }"#,
        );
        let request = StoreSearchRequest::new(0, 100).unwrap();
        let parsed = parse_store_search_page(&request, &raw).unwrap();

        assert_eq!(parsed.result_count, 3);
        assert_eq!(parsed.candidates.len(), 2);
        assert_eq!(parsed.candidates[0].app_id, 548430);
        assert_eq!(parsed.candidates[1].name, "Risk & Rain");
        assert!(parsed.is_complete());
    }

    #[test]
    fn rejects_start_mismatch_and_empty_nonterminal_page() {
        let request = StoreSearchRequest::new(100, 100).unwrap();
        let mismatch = page(r#"{"success":1,"start":0,"total_count":200,"results_html":""}"#);
        assert!(matches!(
            parse_store_search_page(&request, &mismatch),
            Err(SourceError::InvalidStructure { .. })
        ));

        let empty = page(r#"{"success":1,"start":100,"total_count":200,"results_html":""}"#);
        assert!(matches!(
            parse_store_search_page(&request, &empty),
            Err(SourceError::InvalidStructure { .. })
        ));
    }

    #[test]
    fn request_is_bounded_and_uses_the_multiplayer_filter() {
        assert!(StoreSearchRequest::new(0, 0).is_err());
        assert!(StoreSearchRequest::new(0, 101).is_err());
        let path = StoreSearchRequest::new(200, 100).unwrap().path_and_query();
        assert!(path.contains("start=200"));
        assert!(path.contains("category2=1"));
        // New multiplayer titles lack reviews; discovery ranks by release date.
        assert!(path.contains("sort_by=Released_DESC"));
        let reviews = StoreSearchRequest::with_sort(0, 50, StoreSearchSort::ReviewsDesc)
            .unwrap()
            .path_and_query();
        assert!(reviews.contains("sort_by=Reviews_DESC"));
        assert!(reviews.contains("cc=CN&l=schinese"));

        let upcoming = StoreSearchRequest::with_sort(0, 50, StoreSearchSort::ReleasedAsc)
            .unwrap()
            .path_and_query();
        assert!(upcoming.contains("sort_by=Released_ASC"));
        assert!(upcoming.contains("filter=comingsoon"));
    }
}
