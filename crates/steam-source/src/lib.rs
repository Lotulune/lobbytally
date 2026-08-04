//! Steam source adapters for MPGS M1 data feasibility.
//!
//! Pipeline stages for every source:
//! `request -> raw response validation -> source DTO -> normalized proposal`
//!
//! Default unit tests use recorded fixtures only. Live Steam calls are never
//! required for `cargo test`.

#![forbid(unsafe_code)]

pub mod app_list;
pub mod ccu;
pub mod cursor;
pub mod error;
pub mod golden;
pub mod hash;
pub mod proposal;
pub mod rate_limit;
pub mod raw;
pub mod reviews;
pub mod store;
pub mod store_search;

pub use app_list::{
    ADAPTER_VERSION as APP_LIST_ADAPTER_VERSION, AppListPage, AppListRequest,
    DEFAULT_MAX_RESULTS as APP_LIST_MAX_RESULTS, SOURCE_NAME as APP_LIST_SOURCE_NAME,
    apply_page_to_cursor, collect_pages, parse_app_list_page,
};
pub use ccu::{
    ADAPTER_VERSION as CCU_ADAPTER_VERSION, CcuRequest, CcuSampleTier,
    SOURCE_NAME as CCU_SOURCE_NAME, http_not_found_proposal, parse_ccu,
};
pub use cursor::AppListCursor;
pub use error::SourceError;
pub use golden::{
    DominantModeLabel, GOLDEN_SET_VERSION, GoldenGame, GoldenMultiplayerLabels, GoldenSet,
    ReleaseStateLabel,
};
pub use hash::{content_hash, parameter_hash};
pub use proposal::{
    AppCatalogProposal, AppRelationProposal, AppTypeProposal, CcuProposal, PopularReviewProposal,
    PopularReviewsProposal, RelationTypeProposal, ReleaseStateProposal, ReviewSummaryProposal,
    SourceStability, StoreDetailsProposal, StoreMovieProposal, StorePriceProposal,
    StoreScreenshotProposal,
};
pub use rate_limit::{DailyBudget, TokenBucket};
pub use raw::RawResponse;
pub use reviews::{
    ADAPTER_VERSION as REVIEWS_ADAPTER_VERSION, ReviewSummaryRequest,
    SOURCE_NAME as REVIEWS_SOURCE_NAME, parse_popular_reviews, parse_review_summary,
};
pub use store::{
    ADAPTER_VERSION as STORE_ADAPTER_VERSION, DEFAULT_STORE_COUNTRY, DEFAULT_STORE_LANGUAGE,
    SOURCE_NAME as STORE_SOURCE_NAME, STORE_APPDETAILS_FEASIBILITY, StoreDetailsParseResult,
    StoreDetailsRequest, parse_store_details,
};
pub use store_search::{
    ADAPTER_VERSION as STORE_SEARCH_ADAPTER_VERSION, SOURCE_NAME as STORE_SEARCH_SOURCE_NAME,
    StoreSearchCandidate, StoreSearchPage, StoreSearchRequest, StoreSearchSort,
    parse_store_search_page,
};

/// Recommended default User-Agent for MPGS server-side fetches.
pub const DEFAULT_USER_AGENT: &str =
    "MPGS-Server/0.1 (+https://github.com/Lotulune/mpgs; research)";

/// Official Web API host (requires key for GetAppList).
pub const STEAM_WEB_API_HOST: &str = "https://api.steampowered.com";

/// Store host for reviews and appdetails.
pub const STEAM_STORE_HOST: &str = "https://store.steampowered.com";
