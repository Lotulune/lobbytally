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
}

export interface SaveConfigRequest {
  steamApiKey?: string;
  llmApiKey?: string;
  llmBaseUrl?: string;
  llmModel?: string;
  country?: string;
  language?: string;
}

export interface DashboardPayload {
  newGames: GameCard[];
  classics: GameCard[];
  upcoming: GameCard[];
  recentDiscoveries: GameCard[];
  collections: UserCollections;
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
  dataSource: string;
}

export interface GameCard {
  appid: number;
  name: string;
  shortDescription?: string | null;
  section: "new" | "classic" | string;
  releaseDate?: string | null;
  releaseDateText: string;
  releaseState: StoreReleaseState;
  demoStatus: DemoStatus;
  supportedLanguages: string[];
  isAdultContent: boolean;
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
