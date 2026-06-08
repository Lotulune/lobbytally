/* eslint-disable */
// This file is generated. Do not edit by hand.
// Generated from docs/openapi/mpgs-server.openapi.json.

export interface AdminAuditEventsResponse {
  events: AdminAuditEventSummary[];
}

export interface AdminAuditEventSummary {
  actor: string;
  eventType: string;
  outcome: string;
}

export interface AdminCreateTaskRequest {
  appid?: number | null;
  taskType: AdminTaskKind;
}

export interface AdminCreateTaskResponse {
  task: AdminTaskSummary;
}

export interface AdminDiagnosticsResponse {
  activeConfig: string;
  httpsStatus: string;
  llm: string;
  postgres: string;
  publicBaseUrl?: string | null;
  publicBaseUrlStatus: string;
  publicCors: string;
  r2: string;
  restartPolicy: string;
  safeMode: boolean;
  steam: string;
}

export interface AdminOverviewResponse {
  connectionShareConfigured: boolean;
  failureSummary: AdminTaskFailureSummary;
  latestAuditEvent?: null | AdminAuditEventSummary;
  latestTask?: null | AdminTaskSummary;
  pendingReviewCount: number;
  publicCatalogStatus: PublicCatalogStatus;
  publicGameCount: number;
  restartRequired: boolean;
  serviceName: string;
}

export type AdminReviewAction = "accept_public" | "accept_hidden" | "reject" | "archive";

export interface AdminReviewActionRequest {
  action: AdminReviewAction;
  note?: string | null;
}

export interface AdminReviewActionResponse {
  game: AdminReviewCandidate;
}

export interface AdminReviewCandidate {
  appid: number;
  name: string;
  recommendationScore?: number | null;
  reviewNote?: string | null;
  reviewStatus: string;
  updatedAt: string;
  visibility: string;
}

export interface AdminReviewQueueResponse {
  items: AdminReviewCandidate[];
}

export interface AdminSessionRequest {
  token: string;
}

export interface AdminSessionResponse {
  authenticated: boolean;
}

export interface AdminTaskFailureItem {
  attempt: number;
  createdAt: string;
  provider?: string | null;
  reason: string;
  retryable: boolean;
  stage: string;
  target?: string | null;
  taskId?: number | null;
}

export interface AdminTaskFailureSummary {
  latestFailure?: null | AdminTaskFailureItem;
  openFailureCount: number;
  retryableFailureCount: number;
}

export type AdminTaskKind = "manual_appid_discovery";

export interface AdminTasksResponse {
  failures: AdminTaskFailureItem[];
  failureSummary: AdminTaskFailureSummary;
  recentTasks: AdminTaskSummary[];
}

export interface AdminTaskSummary {
  createdAt: string;
  id: number;
  status: string;
  target?: string | null;
  targetAppid?: number | null;
  taskType: string;
  updatedAt: string;
}

export interface ConfigStateResponse {
  activeConfigVersion: string;
  lastStartupStatus: string;
  pendingConfigVersion?: string | null;
  restartRequired: boolean;
}

export interface DiscoveryHomeResponse {
  sections: DiscoveryHomeSections;
  status: PublicCatalogStatus;
  totalGames: number;
}

export interface DiscoveryHomeSections {
  highConfidence: PublicGameListItem[];
  newlyPublished: PublicGameListItem[];
  recentlyAdded: PublicGameListItem[];
}

export interface HealthResponse {
  status: HealthStatus;
}

export type HealthStatus = "ok" | "unavailable";

export interface PageMeta {
  limit: number;
  offset: number;
  total: number;
}

export interface PendingConfigResponse {
  pendingConfigVersion: string;
  restartRequired: boolean;
}

export interface PendingServiceIdentityRequest {
  serviceName: string;
}

export type PublicCatalogStatus = "empty" | "ready" | "updating" | "unavailable";

export interface PublicGameAnalysis {
  appid: number;
  generatedAt: string;
  report: unknown;
}

export interface PublicGameDetail {
  game: PublicGameListItem;
}

export interface PublicGameListItem {
  appid: number;
  name: string;
  recommendationScore?: number | null;
  updatedAt: string;
}

export interface PublicGamesPage {
  items: PublicGameListItem[];
  page: PageMeta;
}

export interface RestartRequest {
  confirm: boolean;
}

export interface RestartResponse {
  mode: string;
  restartScheduled: boolean;
}

export type ServiceCapability = "public_catalog_read";

export interface ServiceConnectionFileResponse {
  apiVersion: string;
  baseUrl: string;
  capabilities: ServiceCapability[];
  serviceInfoUrl: string;
  serviceInstanceId: string;
  serviceName: string;
}

export interface ServiceErrorBody {
  code: string;
  details: Record<string, string>;
  message: string;
  requestId: string;
}

export interface ServiceErrorEnvelope {
  error: ServiceErrorBody;
}

export interface ServiceInfo {
  apiVersion: string;
  capabilities: ServiceCapability[];
  publicCatalogStatus: PublicCatalogStatus;
  serviceInstanceId: string;
  serviceName: string;
  serviceVersion: string;
}

export interface SetupCompleteRequest {
  adminToken: string;
  databaseUrl: string;
  publicBaseUrl: string;
  serviceName: string;
  setupToken: string;
  steamApiKey: string;
}

export interface SetupCompleteResponse {
  configured: boolean;
  restartRequired: boolean;
}

export interface SetupStatusResponse {
  configured: boolean;
}
