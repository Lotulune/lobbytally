use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use utoipa::ToSchema;

const ADMIN_SESSION_COOKIE: &str = "mpgs_admin_session";

#[derive(Debug, Clone)]
pub struct AdminAuthConfig {
    token_hash: String,
    session_secret: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminSessionRequest {
    pub token: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminSessionResponse {
    pub authenticated: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminOverviewResponse {
    pub service_name: String,
    pub public_catalog_status: mpgs_core::models::PublicCatalogStatus,
    pub public_game_count: i64,
    pub pending_review_count: i64,
    pub latest_task: Option<AdminTaskSummary>,
    pub failure_summary: AdminTaskFailureSummary,
    pub restart_required: bool,
    pub connection_share_configured: bool,
    pub latest_audit_event: Option<AdminAuditEventSummary>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminAuditEventSummary {
    pub event_type: String,
    pub actor: String,
    pub outcome: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminAuditEventsResponse {
    pub events: Vec<AdminAuditEventSummary>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdminTaskKind {
    ManualAppidDiscovery,
}

impl AdminTaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ManualAppidDiscovery => "manual_appid_discovery",
        }
    }

    pub fn requires_appid(self) -> bool {
        match self {
            Self::ManualAppidDiscovery => true,
        }
    }

    pub fn audit_event_type(self) -> &'static str {
        match self {
            Self::ManualAppidDiscovery => "admin.task.manual_appid_discovery.created",
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminCreateTaskRequest {
    pub task_type: AdminTaskKind,
    pub appid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdminTaskSummary {
    pub id: i64,
    pub task_type: String,
    pub status: String,
    pub target: Option<String>,
    pub target_appid: Option<u32>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdminTaskFailureItem {
    pub task_id: Option<i64>,
    pub stage: String,
    pub target: Option<String>,
    pub provider: Option<String>,
    pub retryable: bool,
    pub attempt: i32,
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdminTaskFailureSummary {
    pub open_failure_count: i64,
    pub retryable_failure_count: i64,
    pub latest_failure: Option<AdminTaskFailureItem>,
}

impl AdminTaskFailureSummary {
    pub fn empty() -> Self {
        Self {
            open_failure_count: 0,
            retryable_failure_count: 0,
            latest_failure: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdminTasksResponse {
    pub recent_tasks: Vec<AdminTaskSummary>,
    pub failure_summary: AdminTaskFailureSummary,
    pub failures: Vec<AdminTaskFailureItem>,
}

#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdminCreateTaskResponse {
    pub task: AdminTaskSummary,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminReviewActionRequest {
    pub action: AdminReviewAction,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminReviewAction {
    AcceptPublic,
    AcceptHidden,
    Reject,
    Archive,
}

impl AdminReviewAction {
    pub fn review_status(self) -> &'static str {
        match self {
            Self::AcceptPublic | Self::AcceptHidden => "accepted",
            Self::Reject => "rejected",
            Self::Archive => "archived",
        }
    }

    pub fn visibility(self) -> &'static str {
        match self {
            Self::AcceptPublic => "public",
            Self::AcceptHidden | Self::Reject | Self::Archive => "hidden",
        }
    }

    pub fn audit_event_type(self) -> &'static str {
        match self {
            Self::AcceptPublic => "admin.review.accept_public",
            Self::AcceptHidden => "admin.review.accept_hidden",
            Self::Reject => "admin.review.reject",
            Self::Archive => "admin.review.archive",
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminDiagnosticsResponse {
    pub postgres: String,
    pub active_config: String,
    pub safe_mode: bool,
    pub public_base_url: Option<String>,
    pub public_base_url_status: String,
    pub https_status: String,
    pub public_cors: String,
    pub restart_policy: String,
    pub steam: String,
    pub llm: String,
    pub r2: String,
}

impl AdminAuthConfig {
    pub fn new(token_hash: String, session_secret: String) -> Self {
        Self {
            token_hash,
            session_secret,
        }
    }

    #[doc(hidden)]
    pub fn for_test_token(token: &str) -> Self {
        Self {
            token_hash: hash_admin_token(token),
            session_secret: "test-admin-session-secret".to_string(),
        }
    }

    pub fn verify_token(&self, token: &str) -> bool {
        verify_token_hash(&self.token_hash, token)
    }

    pub fn session_cookie(&self) -> String {
        let payload = "v1";
        let signature = sign_session(payload, &self.session_secret);
        format!("{ADMIN_SESSION_COOKIE}={payload}.{signature}; Path=/; HttpOnly; SameSite=Strict")
    }

    pub fn verify_cookie_header(&self, cookie_header: Option<&str>) -> bool {
        let Some(cookie_header) = cookie_header else {
            return false;
        };

        cookie_header.split(';').map(str::trim).any(|cookie| {
            let Some(value) = cookie.strip_prefix(&format!("{ADMIN_SESSION_COOKIE}=")) else {
                return false;
            };
            let Some((payload, signature)) = value.split_once('.') else {
                return false;
            };

            constant_time_eq(signature, &sign_session(payload, &self.session_secret))
        })
    }
}

pub fn hash_admin_token(token: &str) -> String {
    hash_token(token)
}

pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    format!(
        "sha256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
    )
}

pub fn verify_token_hash(expected_hash: &str, token: &str) -> bool {
    constant_time_eq(expected_hash, &hash_token(token))
}

fn sign_session(payload: &str, secret: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any secret length");
    mac.update(payload.as_bytes());
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(mac.finalize().into_bytes())
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    left.as_bytes().ct_eq(right.as_bytes()).into()
}
