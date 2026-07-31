//! Steam `IStoreService/GetAppList` pagination and incremental spike.

use serde::Deserialize;

use crate::cursor::AppListCursor;
use crate::error::SourceError;
use crate::proposal::{AppCatalogProposal, AppTypeProposal, SourceStability};
use crate::raw::RawResponse;

pub const ADAPTER_VERSION: &str = "app-list-0.1.0";
pub const SOURCE_NAME: &str = "steam_istore_getapplist";
pub const DEFAULT_MAX_RESULTS: u32 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppListRequest {
    pub last_appid: u32,
    pub if_modified_since: u32,
    pub max_results: u32,
    pub include_games: bool,
    pub include_dlc: bool,
    pub include_software: bool,
    pub include_videos: bool,
    pub include_hardware: bool,
}

impl AppListRequest {
    pub fn from_cursor(cursor: &AppListCursor, max_results: u32) -> Self {
        Self {
            last_appid: cursor.last_appid,
            if_modified_since: cursor.if_modified_since,
            max_results,
            include_games: true,
            include_dlc: false,
            include_software: false,
            include_videos: false,
            include_hardware: false,
        }
    }

    /// Build query pairs for Web API (key excluded for hashing / logging).
    pub fn query_pairs_without_key(&self) -> Vec<(&'static str, String)> {
        vec![
            ("include_games", bool_flag(self.include_games)),
            ("include_dlc", bool_flag(self.include_dlc)),
            ("include_software", bool_flag(self.include_software)),
            ("include_videos", bool_flag(self.include_videos)),
            ("include_hardware", bool_flag(self.include_hardware)),
            ("last_appid", self.last_appid.to_string()),
            ("max_results", self.max_results.to_string()),
            ("if_modified_since", self.if_modified_since.to_string()),
        ]
    }

    pub fn path_and_query_without_key(&self) -> String {
        let pairs = self.query_pairs_without_key();
        let query = pairs
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        format!("/IStoreService/GetAppList/v1/?{query}")
    }
}

fn bool_flag(value: bool) -> String {
    if value { "true" } else { "false" }.to_owned()
}

#[derive(Debug, Deserialize)]
struct AppListEnvelope {
    response: AppListResponseBody,
}

#[derive(Debug, Deserialize)]
struct AppListResponseBody {
    #[serde(default)]
    apps: Vec<AppListAppDto>,
    #[serde(default)]
    have_more_results: bool,
    #[serde(default)]
    last_appid: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct AppListAppDto {
    appid: u32,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    last_modified: Option<u32>,
    #[serde(default)]
    price_change_number: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppListPage {
    pub proposals: Vec<AppCatalogProposal>,
    pub have_more_results: bool,
    pub page_last_appid: u32,
    pub page_max_last_modified: u32,
    pub content_hash: String,
}

impl AppListPage {
    /// Validate that applying this page would move the request cursor forward.
    ///
    /// The response-level parser cannot know the request cursor, so collectors
    /// must call this before ingesting or persisting the next checkpoint.
    pub fn validate_for_request(&self, request: &AppListRequest) -> Result<(), SourceError> {
        if self.proposals.is_empty() {
            return Ok(());
        }
        if self.page_last_appid <= request.last_appid {
            return Err(SourceError::invalid_structure(format!(
                "app list cursor did not advance: requested last_appid {}, received {}",
                request.last_appid, self.page_last_appid
            )));
        }
        Ok(())
    }
}

/// Parse a validated GetAppList response into normalized proposals.
pub fn parse_app_list_page(raw: &RawResponse) -> Result<AppListPage, SourceError> {
    let envelope: AppListEnvelope = raw.parse_json()?;
    let body = envelope.response;

    if body.apps.is_empty() && body.have_more_results {
        return Err(SourceError::invalid_structure(
            "have_more_results is true but apps array is empty",
        ));
    }

    let mut proposals = Vec::with_capacity(body.apps.len());
    let mut page_max_last_modified = 0_u32;
    let mut max_appid = 0_u32;

    for app in body.apps {
        if app.appid == 0 {
            return Err(SourceError::invalid_structure("appid must be non-zero"));
        }
        max_appid = max_appid.max(app.appid);
        if let Some(modified) = app.last_modified {
            page_max_last_modified = page_max_last_modified.max(modified);
        }

        let name = app.name.filter(|n| !n.trim().is_empty()).ok_or_else(|| {
            SourceError::invalid_structure(format!("app {} missing name", app.appid))
        })?;

        proposals.push(AppCatalogProposal {
            app_id: app.appid,
            name,
            // GetAppList does not always include type; default to game when only games requested.
            app_type: AppTypeProposal::Game,
            last_modified: app.last_modified,
            price_change_number: app.price_change_number,
            source: SOURCE_NAME,
            stability: SourceStability::OfficialStable,
            adapter_version: ADAPTER_VERSION,
        });
    }

    let page_last_appid = body.last_appid.unwrap_or(max_appid);
    if body.have_more_results && page_last_appid == 0 {
        return Err(SourceError::invalid_structure(
            "have_more_results requires last_appid",
        ));
    }
    if !proposals.is_empty() && page_last_appid != max_appid {
        return Err(SourceError::invalid_structure(format!(
            "response last_appid {page_last_appid} does not match page maximum appid {max_appid}"
        )));
    }

    Ok(AppListPage {
        proposals,
        have_more_results: body.have_more_results,
        page_last_appid,
        page_max_last_modified,
        content_hash: raw.content_hash.clone(),
    })
}

/// Apply a parsed page onto a durable cursor (in-memory model of resume).
pub fn apply_page_to_cursor(cursor: &mut AppListCursor, page: &AppListPage) {
    cursor.advance_page(
        page.page_last_appid,
        page.page_max_last_modified,
        page.have_more_results,
    );
    if !page.have_more_results {
        cursor.complete_pass();
    }
}

/// Collect multi-page fixture responses as a single catalog sample.
pub fn collect_pages(
    raw_pages: &[&RawResponse],
) -> Result<(Vec<AppCatalogProposal>, AppListCursor), SourceError> {
    let mut cursor = AppListCursor::new_pass(0, ADAPTER_VERSION);
    let mut all = Vec::new();

    for raw in raw_pages {
        let request = AppListRequest::from_cursor(&cursor, DEFAULT_MAX_RESULTS);
        let page = parse_app_list_page(raw)?;
        page.validate_for_request(&request)?;
        all.extend(page.proposals.iter().cloned());
        apply_page_to_cursor(&mut cursor, &page);
    }

    Ok((all, cursor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::RawResponse;

    fn fixture(name: &str) -> RawResponse {
        let body = match name {
            "page1" => include_bytes!("../fixtures/app_list_page1.json").to_vec(),
            "page2" => include_bytes!("../fixtures/app_list_page2.json").to_vec(),
            "incremental" => include_bytes!("../fixtures/app_list_incremental.json").to_vec(),
            "empty_more" => include_bytes!("../fixtures/app_list_invalid_empty_more.json").to_vec(),
            other => panic!("unknown fixture {other}"),
        };
        RawResponse::validate(200, body, Some("application/json".into()), 1024 * 1024).unwrap()
    }

    #[test]
    fn paginates_and_resumes_with_cursor() {
        let page1 = parse_app_list_page(&fixture("page1")).unwrap();
        assert_eq!(page1.proposals.len(), 3);
        assert!(page1.have_more_results);
        assert_eq!(page1.page_last_appid, 730);

        let mut cursor = AppListCursor::new_pass(0, ADAPTER_VERSION);
        apply_page_to_cursor(&mut cursor, &page1);
        assert_eq!(cursor.last_appid, 730);
        assert!(cursor.have_more_results);

        let request = AppListRequest::from_cursor(&cursor, 50);
        assert_eq!(request.last_appid, 730);
        assert!(
            request
                .path_and_query_without_key()
                .contains("last_appid=730")
        );

        let page2 = parse_app_list_page(&fixture("page2")).unwrap();
        assert_eq!(page2.proposals.len(), 2);
        assert!(!page2.have_more_results);
        apply_page_to_cursor(&mut cursor, &page2);

        assert_eq!(cursor.last_appid, 0);
        assert!(!cursor.have_more_results);
        assert_eq!(cursor.if_modified_since, 1_700_000_500);
    }

    #[test]
    fn incremental_filter_uses_if_modified_since() {
        let mut cursor = AppListCursor::new_pass(1_700_000_000, ADAPTER_VERSION);
        let page = parse_app_list_page(&fixture("incremental")).unwrap();
        assert_eq!(page.proposals.len(), 2);
        for proposal in &page.proposals {
            assert!(proposal.last_modified.unwrap() >= 1_700_000_000);
            assert_eq!(proposal.stability, SourceStability::OfficialStable);
        }
        apply_page_to_cursor(&mut cursor, &page);
        assert_eq!(cursor.if_modified_since, 1_700_000_420);
    }

    #[test]
    fn collect_pages_merges_catalog() {
        let p1 = fixture("page1");
        let p2 = fixture("page2");
        let (apps, cursor) = collect_pages(&[&p1, &p2]).unwrap();
        assert_eq!(apps.len(), 5);
        assert_eq!(apps[0].app_id, 10);
        assert_eq!(apps[4].app_id, 892970);
        assert!(!cursor.have_more_results);
    }

    #[test]
    fn empty_page_with_more_results_is_structure_error() {
        let err = parse_app_list_page(&fixture("empty_more")).unwrap_err();
        assert!(matches!(err, SourceError::InvalidStructure { .. }));
    }

    #[test]
    fn parse_failure_is_not_silent_empty_success() {
        let raw = RawResponse::validate(
            200,
            br#"{"response":{"apps":"not-an-array"}}"#.to_vec(),
            None,
            1024,
        )
        .unwrap();
        let err = parse_app_list_page(&raw).unwrap_err();
        assert!(matches!(err, SourceError::JsonParse { .. }));
    }

    #[test]
    fn rejects_response_cursor_that_does_not_match_page_maximum() {
        let raw = RawResponse::validate(
            200,
            br#"{"response":{"apps":[{"appid":10,"name":"A"},{"appid":20,"name":"B"}],"have_more_results":true,"last_appid":10}}"#.to_vec(),
            None,
            1024,
        )
        .unwrap();
        let err = parse_app_list_page(&raw).unwrap_err();
        assert!(matches!(err, SourceError::InvalidStructure { .. }));
        assert!(err.to_string().contains("page maximum appid 20"));
    }

    #[test]
    fn rejects_page_that_does_not_advance_request_cursor() {
        let page = parse_app_list_page(&fixture("page1")).unwrap();
        let request = AppListRequest {
            last_appid: page.page_last_appid,
            if_modified_since: 0,
            max_results: 100,
            include_games: true,
            include_dlc: false,
            include_software: false,
            include_videos: false,
            include_hardware: false,
        };
        let err = page.validate_for_request(&request).unwrap_err();
        assert!(matches!(err, SourceError::InvalidStructure { .. }));
        assert!(err.to_string().contains("did not advance"));
    }
}
