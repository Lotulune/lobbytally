import type { DemoStatus } from "./domain/recommendation";

export type StoreReleaseState =
  | "upcoming"
  | "released"
  | "tba"
  | "unknown";

export interface PublicConfig {
  steamApiKeyConfigured: boolean;
  llmApiKeyConfigured: boolean;
  llmBaseUrl: string;
  llmModel: string;
  country: string;
  language: string;
  aiBatchRefreshConcurrency: number;
}

export interface SaveConfigRequest {
  steamApiKey?: string;
  llmApiKey?: string;
  llmBaseUrl?: string;
  llmModel?: string;
  country?: string;
  language?: string;
  aiBatchRefreshConcurrency?: number;
}

export interface DashboardPayload {
  newGames: GameCard[];
  classics: GameCard[];
  hiddenGames: GameCard[];
  upcoming: GameCard[];
  recentDiscoveries: GameCard[];
  collections: UserCollections;
  aiAnalysisQueueFailures: AiAnalysisQueueFailureItem[];
  stats: DashboardStats;
  config: PublicConfig;
}

export interface DashboardStats {
  lastSyncAt?: string | null;
  seedCount: number;
  totalGames: number;
  newGamesCount: number;
  classicGamesCount: number;
  lastDiscoveryAppid?: number | null;
  classicDiscoveryRunning: boolean;
  classicDiscoveryStatus?: DiscoveryRunStatus | null;
  classicDiscoveryCurrentAppid?: number | null;
  classicDiscoveryLastAppid?: number | null;
  classicDiscoveryScannedApps: number;
  classicDiscoveryAddedGames: number;
  classicDiscoveryRejectedGames: number;
  classicDiscoveryFailedGames: number;
  classicDiscoverySkippedExisting: number;
  classicDiscoverySkippedRejectedCache: number;
  classicDiscoveryLastCompletedAt?: string | null;
  syncRunning: boolean;
  syncMode?: SyncMode | null;
  syncPendingCount: number;
  syncCurrentAppid?: number | null;
  syncTotalCount: number;
  syncProcessedCount: number;
  syncUpdatedCount: number;
  syncFailedCount: number;
  syncLastError?: string | null;
  syncLastErrorAppid?: number | null;
  backfillPendingCount: number;
  backfillRunning: boolean;
  backfillCurrentAppid?: number | null;
  backfillCurrentAttempt?: number | null;
  backfillTotalCount: number;
  backfillProcessedCount: number;
  backfillFailedCount: number;
  backfillMaxAttempts: number;
  backfillLastError?: string | null;
  backfillLastErrorAppid?: number | null;
  aiBatchRefreshRunning: boolean;
  aiBatchRefreshConcurrency: number;
  aiBatchRefreshPendingCount: number;
  aiBatchRefreshActiveCount: number;
  aiBatchRefreshTotalCount: number;
  aiBatchRefreshProcessedCount: number;
  aiBatchRefreshUpdatedCount: number;
  aiBatchRefreshFailedCount: number;
  aiBatchRefreshFailedPendingReviewCount: number;
  aiBatchRefreshLastError?: string | null;
  aiBatchRefreshLastErrorAppid?: number | null;
  dataSource: string;
}

export interface GameCard {
  appid: number;
  name: string;
  shortDescription?: string | null;
  section: "new" | "classic" | "classic_hidden" | string;
  releaseDate?: string | null;
  releaseDateText: string;
  releaseState: StoreReleaseState;
  demoStatus: DemoStatus;
  supportedLanguages: string[];
  isAdultContent: boolean;
  isFree: boolean;
  priceText?: string | null;
  discountPercent?: number | null;
  positiveReviewPct?: number | null;
  totalReviews?: number | null;
  currentPlayers?: number | null;
  recommendationScore: number;
  aiScore?: number | null;
  aiSummary: string;
  capsuleUrl: string;
  storeScreenshotUrls?: string[];
  tags: string[];
  multiplayerModes: string[];
  reviewSnippets: ReviewSnippet[];
  userState: UserGameState;
}

export interface UserGameState {
  favorite: boolean;
  wishlist: boolean;
  followed: boolean;
  viewed: boolean;
  updatedAt?: string | null;
}

export interface UserGameStatePatch {
  favorite?: boolean;
  wishlist?: boolean;
  followed?: boolean;
  viewed?: boolean;
}

export interface UserCollections {
  favorites: GameCard[];
  wishlist: GameCard[];
  followed: GameCard[];
  history: GameCard[];
}

export interface ReviewSnippet {
  votedUp: boolean;
  review: string;
  playtimeHours?: number | null;
}

export type SyncMode = "quick" | "full";

export interface SyncRequest {
  mode: SyncMode;
}

export interface SyncReport {
  updatedGames: number;
  failedGames: number;
  message: string;
}

export interface AiBatchRefreshReport {
  totalGames: number;
  updatedGames: number;
  failedGames: number;
  message: string;
}

export interface SteamDiscoveryReport {
  scannedApps: number;
  skippedExisting: number;
  skippedNonMultiplayer: number;
  addedGames: number;
  addedNewGames: number;
  addedClassicGames: number;
  failedGames: number;
  lastAppid?: number | null;
  haveMoreResults: boolean;
  message: string;
}

export interface AiAssessment {
  appid: number;
  score: number;
  summary: string;
  bestFor: string[];
  risks: string[];
}

export type AnalysisSource = "hybrid" | "rule";

export type AnalysisConfidence = "high" | "medium" | "low";

export type AnalysisEvidenceKind =
  | "positive_review_pct"
  | "total_reviews"
  | "current_players"
  | "tags"
  | "multiplayer_modes"
  | "short_description"
  | "review_snippet";

export type AnalysisReviewStance = "strength" | "risk";

export type AnalysisDimensionKey =
  | "review_quality"
  | "multiplayer_fit"
  | "activity_health"
  | "content_depth"
  | "accessibility"
  | "discovery_value"
  | "approachability"
  | "multiplayer_fun"
  | "reputation_stability";

export type RecommendationPool =
  | "new_release"
  | "evergreen"
  | "hidden_gem"
  | "friends_party"
  | "demo_potential";

export interface AnalysisDimensionScore {
  key: AnalysisDimensionKey;
  label: string;
  score: number;
  reason: string;
}

export interface AnalysisPoint {
  title: string;
  reason: string;
}

export interface AnalysisEvidenceItem {
  kind: AnalysisEvidenceKind;
  label: string;
  value: string;
  interpretation: string;
}

export interface AnalysisReviewEvidenceItem {
  stance: AnalysisReviewStance;
  quote: string;
  playtimeText: string;
  interpretation: string;
}

export interface AnalysisRiskFlag {
  key: string;
  label: string;
  severity: number;
  reason: string;
}

export interface GameAnalysisReport {
  appid: number;
  generatedAt: string;
  source: AnalysisSource;
  confidence: AnalysisConfidence;
  scoreVersion?: string;
  qualityScore?: number;
  recommendationScore?: number;
  confidenceScore?: number;
  poolType?: RecommendationPool;
  riskFlags?: AnalysisRiskFlag[];
  overallScore: number;
  overview: string;
  dimensionScores: AnalysisDimensionScore[];
  strengths: AnalysisPoint[];
  risks: AnalysisPoint[];
  evidence: AnalysisEvidenceItem[];
  reviewEvidence: AnalysisReviewEvidenceItem[];
}

export interface SteamAppListPreview {
  apps: SteamAppListItem[];
  lastAppid?: number | null;
  haveMoreResults?: boolean | null;
}

export interface SteamAppListItem {
  appid: number;
  name: string;
}

export type DiscoveryRunStatus =
  | "running"
  | "paused"
  | "completed"
  | "failed"
  | "cancelled"
  | "interrupted";

export type DiscoveryCompletionReason =
  | "target_reached"
  | "page_budget_reached"
  | "no_more_results"
  | "paused"
  | "cancelled"
  | "failed"
  | "interrupted";

export interface DiscoveryTaskRequest {
  syncMode: SyncMode;
  targetAddedGames: number;
  pageSize: number;
}

export interface DiscoveryFailureItem {
  pageIndex: number;
  appid?: number | null;
  stage: string;
  reason: string;
  createdAt: string;
}

export interface DiscoveryRunSnapshot {
  id: number;
  status: DiscoveryRunStatus;
  completionReason?: DiscoveryCompletionReason | null;
  syncMode: SyncMode;
  targetAddedGames: number;
  pageSize: number;
  pagesProcessed: number;
  scannedApps: number;
  addedGames: number;
  addedNewGames: number;
  addedClassicGames: number;
  skippedExisting: number;
  skippedNonMultiplayer: number;
  failedGames: number;
  currentAppid?: number | null;
  lastAppid?: number | null;
  haveMoreResults: boolean;
  startedAt: string;
  updatedAt: string;
  finishedAt?: string | null;
  lastError?: string | null;
  failures: DiscoveryFailureItem[];
  progressPercent: number;
}

export interface ClassicDiscoveryTaskRequest {
  maxPages?: number;
}

export interface ClassicDiscoveryRunSnapshot {
  id: number;
  status: DiscoveryRunStatus;
  maxPages: number;
  pageSize: number;
  pagesProcessed: number;
  scannedApps: number;
  consideredApps: number;
  addedGames: number;
  rejectedGames: number;
  skippedExisting: number;
  skippedRejectedCache: number;
  failedGames: number;
  currentAppid?: number | null;
  lastAppid?: number | null;
  consecutiveEmptyPages: number;
  ruleVersion: string;
  startedAt: string;
  updatedAt: string;
  finishedAt?: string | null;
  lastError?: string | null;
}

export interface AiAnalysisQueueFailureItem {
  appid: number;
  attempt: number;
  lastError: string;
  updatedAt: string;
}
