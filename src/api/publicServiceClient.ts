import type {
  DiscoveryHomeResponse,
  PublicGameAnalysis,
  PublicGameListItem,
  PublicGamesPage,
} from "./generated/mpgsServerApi";
import type {
  DashboardPayload,
  GameAnalysisReport,
  GameCard,
  PublicConfig,
  ServiceInfo,
  UserCollections,
} from "../types";
import type { CurrentServiceConnection } from "../domain/serviceConnectionStorage";
import { normalizeServiceBaseUrl } from "../domain/serviceConnection";
import { getStoredUserGameState } from "../domain/userGameStateStorage";

export interface PublicServiceFetchOptions {
  fetcher?: typeof fetch;
}

const PUBLIC_GAMES_LIMIT = 100;

export async function fetchPublicDashboard(
  connection: CurrentServiceConnection,
  options: PublicServiceFetchOptions = {},
): Promise<DashboardPayload> {
  const baseUrl = normalizeServiceBaseUrl(connection.baseUrl);
  const [home, gamesPage] = await Promise.all([
    fetchJson<DiscoveryHomeResponse>(
      `${baseUrl}/api/v1/discovery-home`,
      options.fetcher,
    ),
    fetchJson<PublicGamesPage>(
      `${baseUrl}/api/v1/games?limit=${PUBLIC_GAMES_LIMIT}&offset=0`,
      options.fetcher,
    ),
  ]);

  const newlyPublished = mapPublicGames(
    connection,
    home.sections.newlyPublished,
    "new",
  );
  const highConfidence = mapPublicGames(
    connection,
    home.sections.highConfidence,
    "classic",
  );
  const recentDiscoveries = mapPublicGames(
    connection,
    home.sections.recentlyAdded,
    "new",
  );
  const listGames = mapPublicGames(connection, gamesPage.items, "classic");
  const classics = highConfidence.length > 0 ? highConfidence : listGames;
  const publicGames = dedupeGames([
    ...newlyPublished,
    ...classics,
    ...recentDiscoveries,
    ...listGames,
  ]);

  return {
    newGames: newlyPublished,
    classics,
    hiddenGames: [],
    upcoming: [],
    recentDiscoveries:
      recentDiscoveries.length > 0
        ? recentDiscoveries
        : [...newlyPublished, ...highConfidence].slice(0, 8),
    collections: buildCollections(publicGames),
    aiAnalysisQueueFailures: [],
    stats: buildPublicDashboardStats(home, gamesPage, connection.info),
    config: buildPublicClientConfig(),
  };
}

export async function fetchPublicGameAnalysis(
  connection: CurrentServiceConnection,
  appid: number,
  options: PublicServiceFetchOptions = {},
): Promise<GameAnalysisReport | null> {
  const baseUrl = normalizeServiceBaseUrl(connection.baseUrl);
  const response = await (options.fetcher ?? fetch)(
    `${baseUrl}/api/v1/games/${appid}/analysis`,
    { method: "GET" },
  );

  if (response.status === 404) {
    return null;
  }
  if (!response.ok) {
    throw new Error(`公共游戏分析读取失败：HTTP ${response.status}。`);
  }

  const payload = (await response.json()) as PublicGameAnalysis;
  return isGameAnalysisReport(payload.report) ? payload.report : null;
}

function mapPublicGames(
  connection: CurrentServiceConnection,
  items: PublicGameListItem[],
  section: GameCard["section"],
): GameCard[] {
  return items.map((item) => mapPublicGame(connection, item, section));
}

function mapPublicGame(
  connection: CurrentServiceConnection,
  item: PublicGameListItem,
  section: GameCard["section"],
): GameCard {
  const score = item.recommendationScore ?? 0;

  return {
    appid: item.appid,
    name: item.name,
    section,
    releaseDate: null,
    releaseDateText: "公开库",
    releaseState: "unknown",
    demoStatus: "unknown",
    supportedLanguages: [],
    isAdultContent: false,
    isFree: false,
    priceText: null,
    discountPercent: null,
    positiveReviewPct: null,
    totalReviews: null,
    currentPlayers: null,
    recommendationScore: score,
    aiScore: score,
    aiSummary: "公共发现服务暂未提供客户端详情摘要。",
    capsuleUrl: steamHeaderUrl(item.appid),
    storeScreenshotUrls: [],
    tags: [],
    multiplayerModes: [],
    reviewSnippets: [],
    userState: getStoredUserGameState(connection.info.serviceInstanceId, item.appid),
  };
}

function dedupeGames(games: GameCard[]): GameCard[] {
  const seen = new Set<number>();
  const deduped: GameCard[] = [];

  for (const game of games) {
    if (seen.has(game.appid)) {
      continue;
    }
    seen.add(game.appid);
    deduped.push(game);
  }

  return deduped;
}

function buildCollections(games: GameCard[]): UserCollections {
  return {
    favorites: games.filter((game) => game.userState.favorite),
    wishlist: games.filter((game) => game.userState.wishlist),
    followed: games.filter((game) => game.userState.followed),
    history: games.filter((game) => game.userState.viewed),
  };
}

async function fetchJson<T>(
  url: string,
  fetcher: typeof fetch = fetch,
): Promise<T> {
  const response = await fetcher(url, { method: "GET" });
  if (!response.ok) {
    throw new Error(`公共发现服务读取失败：HTTP ${response.status}。`);
  }

  return (await response.json()) as T;
}

function buildPublicDashboardStats(
  home: DiscoveryHomeResponse,
  gamesPage: PublicGamesPage,
  serviceInfo: ServiceInfo,
): DashboardPayload["stats"] {
  const newGamesCount = home.sections.newlyPublished.length;
  const classicGamesCount =
    home.sections.highConfidence.length > 0
      ? home.sections.highConfidence.length
      : gamesPage.items.length;

  return {
    lastSyncAt: null,
    seedCount: gamesPage.page.total,
    totalGames: home.totalGames,
    newGamesCount,
    classicGamesCount,
    lastDiscoveryAppid: null,
    classicDiscoveryRunning: false,
    classicDiscoveryStatus: null,
    classicDiscoveryCurrentAppid: null,
    classicDiscoveryLastAppid: null,
    classicDiscoveryScannedApps: 0,
    classicDiscoveryAddedGames: 0,
    classicDiscoveryRejectedGames: 0,
    classicDiscoveryFailedGames: 0,
    classicDiscoverySkippedExisting: 0,
    classicDiscoverySkippedRejectedCache: 0,
    classicDiscoveryLastCompletedAt: null,
    syncRunning: false,
    syncMode: null,
    syncPendingCount: 0,
    syncCurrentAppid: null,
    syncTotalCount: 0,
    syncProcessedCount: 0,
    syncUpdatedCount: 0,
    syncFailedCount: 0,
    syncLastError: null,
    syncLastErrorAppid: null,
    backfillPendingCount: 0,
    backfillRunning: false,
    backfillCurrentAppid: null,
    backfillCurrentAttempt: null,
    backfillTotalCount: 0,
    backfillProcessedCount: 0,
    backfillFailedCount: 0,
    backfillMaxAttempts: 0,
    backfillLastError: null,
    backfillLastErrorAppid: null,
    aiBatchRefreshRunning: false,
    aiBatchRefreshConcurrency: 0,
    aiBatchRefreshPendingCount: 0,
    aiBatchRefreshActiveCount: 0,
    aiBatchRefreshTotalCount: 0,
    aiBatchRefreshProcessedCount: 0,
    aiBatchRefreshUpdatedCount: 0,
    aiBatchRefreshFailedCount: 0,
    aiBatchRefreshFailedPendingReviewCount: 0,
    aiBatchRefreshLastError: null,
    aiBatchRefreshLastErrorAppid: null,
    dataSource: `公共发现服务：${serviceInfo.serviceName}`,
  };
}

function buildPublicClientConfig(): PublicConfig {
  return {
    steamApiKeyConfigured: false,
    steamApiKeyValidated: false,
    llmApiKeyConfigured: false,
    llmConfigValidated: false,
    llmProvider: "deepseek",
    llmBaseUrl: "https://api.deepseek.com",
    llmModel: "deepseek-v4-flash",
    country: "US",
    language: "schinese",
    aiBatchRefreshConcurrency: 0,
    onboardingCompleted: true,
    onboardingCurrentStep: 1,
    onboardingLlmProviderDraft: "deepseek",
  };
}

function steamHeaderUrl(appid: number) {
  return `https://cdn.cloudflare.steamstatic.com/steam/apps/${appid}/header.jpg`;
}

function isGameAnalysisReport(value: unknown): value is GameAnalysisReport {
  if (!value || typeof value !== "object") {
    return false;
  }

  const candidate = value as Partial<GameAnalysisReport>;
  return (
    typeof candidate.appid === "number" &&
    typeof candidate.generatedAt === "string" &&
    (candidate.source === "hybrid" || candidate.source === "rule") &&
    (candidate.confidence === "high" ||
      candidate.confidence === "medium" ||
      candidate.confidence === "low") &&
    typeof candidate.overallScore === "number" &&
    typeof candidate.overview === "string" &&
    Array.isArray(candidate.dimensionScores) &&
    Array.isArray(candidate.strengths) &&
    Array.isArray(candidate.risks) &&
    Array.isArray(candidate.evidence) &&
    Array.isArray(candidate.reviewEvidence)
  );
}
