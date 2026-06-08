import { invoke } from "@tauri-apps/api/core";
import { getCurrentServiceConnection } from "../domain/serviceConnectionStorage";
import { setStoredUserGameState } from "../domain/userGameStateStorage";
import {
  fetchPublicDashboard,
  fetchPublicGameAnalysis,
} from "./publicServiceClient";
import { mockDashboard } from "../data/mockDashboard";
import type {
  AiAssessment,
  AiBatchRefreshReport,
  AiRecommendationRequest,
  AiRecommendationResponse,
  AiRecommendedGame,
  AnalysisConfidence,
  AnalysisDimensionScore,
  AnalysisEvidenceItem,
  AnalysisPoint,
  AnalysisReviewEvidenceItem,
  AnalysisReviewStance,
  AnalysisSource,
  ConnectionValidationResult,
  DashboardPayload,
  DiscoveryRunSnapshot,
  DiscoveryTaskRequest,
  GameAnalysisReport,
  GameCard,
  ClassicDiscoveryRunSnapshot,
  ClassicDiscoveryTaskRequest,
  LlmProvider,
  PublicConfig,
  SaveConfigRequest,
  SteamDiscoveryReport,
  SteamAppListPreview,
  SyncMode,
  SyncReport,
  SyncRequest,
  UserCollections,
  UserGameState,
  UserGameStatePatch,
  ValidateLlmConfigRequest,
  ValidateSteamConfigRequest,
} from "../types";

export const isTauriRuntime = () =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const mockDiscoveryTaskHistory: DiscoveryRunSnapshot[] = [];
const mockGameAnalysisCache = new Map<number, GameAnalysisReport>();
let mockGameAnalysisVersion = 0;

export function __resetMockGameAnalysisCacheForTests() {
  mockGameAnalysisCache.clear();
  mockGameAnalysisVersion = 0;
}

function allMockGames() {
  return [
    ...mockDashboard.upcoming,
    ...mockDashboard.newGames,
    ...mockDashboard.classics,
    ...mockDashboard.hiddenGames,
  ];
}

function cloneAnalysisPoint(point: AnalysisPoint): AnalysisPoint {
  return { ...point };
}

function cloneDimensionScore(score: AnalysisDimensionScore): AnalysisDimensionScore {
  return { ...score };
}

function cloneEvidenceItem(item: AnalysisEvidenceItem): AnalysisEvidenceItem {
  return { ...item };
}

function cloneReviewEvidenceItem(
  item: AnalysisReviewEvidenceItem,
): AnalysisReviewEvidenceItem {
  return { ...item };
}

function cloneGameAnalysisReport(report: GameAnalysisReport): GameAnalysisReport {
  return {
    ...report,
    dimensionScores: report.dimensionScores.map(cloneDimensionScore),
    strengths: report.strengths.map(cloneAnalysisPoint),
    risks: report.risks.map(cloneAnalysisPoint),
    evidence: report.evidence.map(cloneEvidenceItem),
    reviewEvidence: report.reviewEvidence.map(cloneReviewEvidenceItem),
  };
}

function nextMockGeneratedAt() {
  mockGameAnalysisVersion += 1;
  return new Date(Date.UTC(2026, 3, 30, 12, 0, mockGameAnalysisVersion)).toISOString();
}

function buildMockAnalysisReport(game: GameCard): GameAnalysisReport {
  const generatedAt = nextMockGeneratedAt();
  const versionLabel = `模拟分析 ${mockGameAnalysisVersion}`;
  const positiveReviewPct = game.positiveReviewPct ?? 0;
  const totalReviews = game.totalReviews ?? 0;
  const currentPlayers = game.currentPlayers ?? 0;
  const source: AnalysisSource = "rule";
  const confidence: AnalysisConfidence =
    positiveReviewPct >= 85 && totalReviews >= 100 ? "high" : "medium";
  const dimensionScores: AnalysisDimensionScore[] = [
    {
      key: "approachability",
      label: "易上手度",
      score: Math.min(95, Math.max(60, (game.aiScore ?? game.recommendationScore) - 4)),
      reason: `${versionLabel}：${game.demoStatus.includes("demo") ? "有 Demo 或试玩路径" : "直接发售"}，上手门槛相对清晰。`,
    },
    {
      key: "multiplayer_fun",
      label: "联机乐趣",
      score: Math.min(96, Math.max(62, game.recommendationScore)),
      reason: `${versionLabel}：联机模式覆盖 ${game.multiplayerModes.join(" / ") || "多人协作"}。`,
    },
    {
      key: "content_depth",
      label: "内容深度",
      score: Math.min(92, Math.max(58, game.recommendationScore - 3)),
      reason: `${versionLabel}：标签聚焦 ${game.tags.slice(0, 3).join(" / ") || "多人体验"}，适合先看中期留存。`,
    },
    {
      key: "reputation_stability",
      label: "口碑稳定性",
      score: Math.min(98, Math.max(50, positiveReviewPct)),
      reason: `${versionLabel}：好评率 ${positiveReviewPct || "未知"}%，评测量 ${totalReviews || "未知"}。`,
    },
    {
      key: "activity_health",
      label: "活跃健康度",
      score: Math.min(95, currentPlayers > 0 ? 60 + Math.log10(currentPlayers + 1) * 10 : 55),
      reason: `${versionLabel}：当前在线 ${currentPlayers || "未知"}，可作为组队活跃参考。`,
    },
  ];

  return {
    appid: game.appid,
    generatedAt,
    source,
    confidence,
    scoreVersion: "v2",
    qualityScore: Math.max(55, (game.aiScore ?? game.recommendationScore) - 6),
    recommendationScore: game.aiScore ?? game.recommendationScore,
    confidenceScore: confidence === "high" ? 0.82 : 0.58,
    poolType: game.demoStatus === "demo_only" ? "demo_potential" : "evergreen",
    riskFlags: [],
    overallScore: game.aiScore ?? game.recommendationScore,
    overview: `${versionLabel}：${game.aiSummary || "适合多人尝鲜，但需要结合评测再判断。"}（生成于 ${generatedAt}）`,
    dimensionScores,
    strengths: [
      {
        title: "适合朋友开黑",
        reason: `${versionLabel}：多人模式信息明确，适合固定队伍快速开局。`,
      },
      {
        title: "口碑基础清晰",
        reason: `${versionLabel}：好评率与现有推荐分都指向较稳的第一印象。`,
      },
    ],
    risks: [
      {
        title: "浏览器模式为本地模拟",
        reason: `${versionLabel}：当前结果未走真实数据库或在线叙事补丁。`,
      },
    ],
    evidence: [
      {
        kind: "positive_review_pct",
        label: "好评率",
        value: positiveReviewPct ? `${positiveReviewPct}%` : "未知",
        interpretation: `${versionLabel}：浏览器模式直接复用 mockDashboard 元数据。`,
      },
      {
        kind: "multiplayer_modes",
        label: "联机模式",
        value: game.multiplayerModes.join(" / ") || "未知",
        interpretation: `${versionLabel}：联机模式决定了朋友局的协作方式。`,
      },
    ],
    reviewEvidence: [
      {
        stance: "strength" satisfies AnalysisReviewStance,
        quote: game.aiSummary || "浏览器模式未收集真实评测。",
        playtimeText: "mock",
        interpretation: `${versionLabel}：浏览器模式仅提供本地预览证据。`,
      },
    ],
  };
}

function cloneDiscoverySnapshot(
  snapshot: DiscoveryRunSnapshot,
): DiscoveryRunSnapshot {
  return {
    ...snapshot,
    failures: snapshot.failures.map((failure) => ({ ...failure })),
  };
}

function upsertMockDiscoverySnapshot(snapshot: DiscoveryRunSnapshot) {
  const nextSnapshot = cloneDiscoverySnapshot(snapshot);
  const existingIndex = mockDiscoveryTaskHistory.findIndex(
    (item) => item.id === nextSnapshot.id,
  );

  if (existingIndex >= 0) {
    mockDiscoveryTaskHistory.splice(existingIndex, 1);
  }

  mockDiscoveryTaskHistory.unshift(nextSnapshot);
}

export async function getDashboard(): Promise<DashboardPayload> {
  const serviceConnection = getCurrentServiceConnection();
  if (serviceConnection) {
    return fetchPublicDashboard(serviceConnection);
  }

  if (!isTauriRuntime()) return mockDashboard;
  return invoke<DashboardPayload>("get_dashboard");
}

export async function saveConfig(
  request: SaveConfigRequest,
): Promise<PublicConfig> {
  if (!isTauriRuntime()) {
    const nextProvider = request.llmProvider ?? mockDashboard.config.llmProvider;
    const steamConfigured =
      request.clearSteamApiKey
        ? false
        : request.steamApiKey !== undefined
          ? Boolean(request.steamApiKey.trim())
          : mockDashboard.config.steamApiKeyConfigured;
    const llmConfigured =
      request.clearLlmApiKey
        ? false
        : request.llmApiKey !== undefined
          ? Boolean(request.llmApiKey.trim())
          : mockDashboard.config.llmApiKeyConfigured;
    const steamValidated =
      request.steamApiKeyValidated ??
      (request.steamApiKey !== undefined || request.clearSteamApiKey
        ? false
        : mockDashboard.config.steamApiKeyValidated);
    const llmValidated =
      request.llmConfigValidated ??
      (request.llmApiKey !== undefined ||
      request.clearLlmApiKey ||
      request.llmProvider !== undefined ||
      request.llmBaseUrl !== undefined ||
      request.llmModel !== undefined
        ? false
        : mockDashboard.config.llmConfigValidated);

    return {
      ...mockDashboard.config,
      steamApiKeyConfigured: steamConfigured,
      steamApiKeyValidated: steamValidated,
      llmApiKeyConfigured: llmConfigured,
      llmConfigValidated: llmValidated,
      llmProvider: nextProvider,
      llmBaseUrl: request.llmBaseUrl || mockDashboard.config.llmBaseUrl,
      llmModel: request.llmModel || mockDashboard.config.llmModel,
      country: request.country || mockDashboard.config.country,
      language: request.language || mockDashboard.config.language,
      aiBatchRefreshConcurrency:
        clampBatchRefreshConcurrency(request.aiBatchRefreshConcurrency) ??
        mockDashboard.config.aiBatchRefreshConcurrency,
      onboardingCompleted:
        request.onboardingCompleted ?? mockDashboard.config.onboardingCompleted,
      onboardingCurrentStep:
        request.onboardingCurrentStep ?? mockDashboard.config.onboardingCurrentStep,
      onboardingLlmProviderDraft:
        request.onboardingLlmProviderDraft ?? mockDashboard.config.onboardingLlmProviderDraft,
    };
  }
  return invoke<PublicConfig>("save_config", { request });
}

export async function validateSteamConfig(
  request: ValidateSteamConfigRequest,
): Promise<ConnectionValidationResult> {
  if (!isTauriRuntime()) {
    if (request.steamApiKey !== undefined && !request.steamApiKey.trim()) {
      throw new Error("请先输入 Steam Web API Key。");
    }

    return {
      success: true,
      message: "浏览器预览模式：已模拟 Steam 连接成功。",
      diagnostic: "预览模式不会真正请求 Steam。",
      latencyMs: 120,
      appCount: 5,
    };
  }

  return invoke<ConnectionValidationResult>("validate_steam_config", { request });
}

export async function validateLlmConfig(
  request: ValidateLlmConfigRequest,
): Promise<ConnectionValidationResult> {
  if (!isTauriRuntime()) {
    if (request.apiKey !== undefined && !request.apiKey.trim()) {
      throw new Error("请先输入 API Key。");
    }

    return {
      success: true,
      message: "浏览器预览模式：已模拟 AI 连接成功。",
      diagnostic: "预览模式不会真正调用模型。",
      latencyMs: 180,
      provider: request.provider,
      baseUrl: request.baseUrl,
      model: request.model,
    };
  }

  return invoke<ConnectionValidationResult>("validate_llm_config", { request });
}

export async function syncSeedGames(mode: SyncMode = "full"): Promise<SyncReport> {
  if (!isTauriRuntime()) {
    return {
      updatedGames: mockDashboard.stats.seedCount,
      failedGames: 0,
      message:
        mode === "full"
          ? "浏览器预览模式：已模拟完整同步。"
          : "浏览器预览模式：已模拟快速同步。",
    };
  }
  return invoke<SyncReport>("sync_seed_games", {
    request: { mode } satisfies SyncRequest,
  });
}

export async function discoverSteamGames(
  maxPages = 2,
  pageSize = 25,
  startAppid?: number,
): Promise<SteamDiscoveryReport> {
  if (!isTauriRuntime()) {
    const scannedApps = Math.min(pageSize * maxPages, mockDashboard.stats.seedCount);
    return {
      scannedApps,
      skippedExisting: scannedApps,
      skippedNonMultiplayer: 0,
      addedGames: 0,
      addedNewGames: 0,
      addedClassicGames: 0,
      failedGames: 0,
      lastAppid:
        mockDashboard.classics[mockDashboard.classics.length - 1]?.appid ?? null,
      haveMoreResults: false,
      message: "浏览器预览模式：已模拟扫描 Steam 最近发售多人候选，未写入新游戏。",
    };
  }
  return invoke<SteamDiscoveryReport>("discover_steam_games", {
    maxPages,
    pageSize,
    startAppid,
  });
}

export async function assessGameWithAi(appid: number): Promise<AiAssessment> {
  if (!isTauriRuntime()) {
    const game = allMockGames().find((item) => item.appid === appid);
    return {
      appid,
      score: game?.aiScore ?? game?.recommendationScore ?? 80,
      summary: game?.aiSummary ?? "适合多人尝鲜，但需要结合评测再判断。",
      bestFor: ["朋友开黑", "多人筛选", "小众发现"],
      risks: ["浏览器预览未调用真实大模型"],
    };
  }
  return invoke<AiAssessment>("assess_game_with_ai", { appid });
}

export async function recommendGamesWithAi(
  request: AiRecommendationRequest,
): Promise<AiRecommendationResponse> {
  if (!isTauriRuntime()) {
    return buildMockRecommendationResponse(request);
  }

  return invoke<AiRecommendationResponse>("recommend_games_with_ai", { request });
}

export async function refreshAllGameAnalyses(
  concurrency?: number,
): Promise<AiBatchRefreshReport> {
  const normalizedConcurrency =
    clampBatchRefreshConcurrency(concurrency) ?? mockDashboard.config.aiBatchRefreshConcurrency;
  if (!isTauriRuntime()) {
    const games = allMockGames();
    for (const game of games) {
      const report = buildMockAnalysisReport(game);
      mockGameAnalysisCache.set(game.appid, report);
    }

    return {
      totalGames: games.length,
      updatedGames: games.length,
      failedGames: 0,
      message: `浏览器预览模式：已模拟按 ${normalizedConcurrency} 路并发重算 ${games.length} 款游戏的 AI 评分。`,
    };
  }

  return invoke<AiBatchRefreshReport>("refresh_all_game_analyses", {
    concurrency: normalizedConcurrency,
  });
}

export async function retryAiAnalysisJob(
  appid: number,
): Promise<AiBatchRefreshReport> {
  if (!isTauriRuntime()) {
    mockDashboard.aiAnalysisQueueFailures = mockDashboard.aiAnalysisQueueFailures.filter(
      (item) => item.appid !== appid,
    );
    mockDashboard.stats.aiBatchRefreshFailedPendingReviewCount =
      mockDashboard.aiAnalysisQueueFailures.length;
    return {
      totalGames: 1,
      updatedGames: 0,
      failedGames: 0,
      message: `浏览器预览模式：已模拟重试 AppID ${appid} 的 AI 分析任务。`,
    };
  }

  return invoke<AiBatchRefreshReport>("retry_ai_analysis_job", { appid });
}

function clampBatchRefreshConcurrency(value?: number): number | undefined {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return undefined;
  }

  return Math.min(10, Math.max(1, Math.round(value)));
}

export function getDefaultLlmBaseUrl(provider: LlmProvider): string {
  switch (provider) {
    case "openai":
      return "https://api.openai.com/v1";
    case "anthropic":
      return "https://api.anthropic.com";
    case "custom":
      return "https://api.deepseek.com";
    case "deepseek":
    default:
      return "https://api.deepseek.com";
  }
}

export function getDefaultLlmModel(provider: LlmProvider): string {
  switch (provider) {
    case "openai":
      return "gpt-4.1";
    case "anthropic":
      return "claude-sonnet-4-20250514";
    case "custom":
      return "deepseek-v4-flash";
    case "deepseek":
    default:
      return "deepseek-v4-flash";
  }
}

export async function getGameAnalysis(
  appid: number,
): Promise<GameAnalysisReport | null> {
  const serviceConnection = getCurrentServiceConnection();
  if (serviceConnection) {
    return fetchPublicGameAnalysis(serviceConnection, appid);
  }

  if (!isTauriRuntime()) {
    const cached = mockGameAnalysisCache.get(appid);
    return cached ? cloneGameAnalysisReport(cached) : null;
  }

  return invoke<GameAnalysisReport | null>("get_game_analysis", { appid });
}

export async function generateGameAnalysis(
  appid: number,
  forceRefresh = false,
): Promise<GameAnalysisReport> {
  const serviceConnection = getCurrentServiceConnection();
  if (serviceConnection) {
    const report = await fetchPublicGameAnalysis(serviceConnection, appid);
    if (!report) {
      throw new Error("公共发现服务暂未提供该游戏分析。");
    }
    return report;
  }

  if (!isTauriRuntime()) {
    if (!forceRefresh) {
      const cached = mockGameAnalysisCache.get(appid);
      if (cached) {
        return cloneGameAnalysisReport(cached);
      }
    }

    const game = allMockGames().find((item) => item.appid === appid);
    if (!game) {
      throw new Error(`未找到 Steam App ${appid}`);
    }

    const report = buildMockAnalysisReport(game);
    mockGameAnalysisCache.set(appid, report);
    return cloneGameAnalysisReport(report);
  }

  return invoke<GameAnalysisReport>("generate_game_analysis", {
    appid,
    forceRefresh,
  });
}

export async function setGameUserState(
  appid: number,
  patch: UserGameStatePatch,
): Promise<UserGameState> {
  const serviceConnection = getCurrentServiceConnection();
  if (serviceConnection) {
    return setStoredUserGameState(
      serviceConnection.info.serviceInstanceId,
      appid,
      patch,
    );
  }

  if (!isTauriRuntime()) {
    const game = allMockGames().find((item) => item.appid === appid);
    if (!game) throw new Error(`未找到 Steam App ${appid}`);
    game.userState = {
      ...game.userState,
      ...patch,
      updatedAt: new Date().toISOString(),
    };
    refreshMockCollections();
    return game.userState;
  }
  return invoke<UserGameState>("set_game_user_state", { appid, patch });
}

export async function getUserCollections(): Promise<UserCollections> {
  if (!isTauriRuntime()) {
    refreshMockCollections();
    return mockDashboard.collections;
  }
  return invoke<UserCollections>("get_user_collections");
}

export async function previewSteamAppList(
  maxResults = 20,
  lastAppid?: number,
): Promise<SteamAppListPreview> {
  if (!isTauriRuntime()) {
    return {
      apps: allMockGames()
        .slice(0, maxResults)
        .map((game) => ({ appid: game.appid, name: game.name })),
      lastAppid:
        mockDashboard.classics[mockDashboard.classics.length - 1]?.appid ?? null,
      haveMoreResults: false,
    };
  }
  return invoke<SteamAppListPreview>("preview_steam_app_list", {
    maxResults,
    lastAppid,
  });
}

export async function getDiscoveryTaskSnapshot(): Promise<DiscoveryRunSnapshot | null> {
  if (!isTauriRuntime()) {
    return mockDiscoveryTaskHistory[0]
      ? cloneDiscoverySnapshot(mockDiscoveryTaskHistory[0])
      : null;
  }

  return invoke<DiscoveryRunSnapshot | null>("get_discovery_task_snapshot");
}

export async function listDiscoveryTaskHistory(
  limit = 8,
): Promise<DiscoveryRunSnapshot[]> {
  if (!isTauriRuntime()) {
    return mockDiscoveryTaskHistory
      .slice(0, limit)
      .map((snapshot) => cloneDiscoverySnapshot(snapshot));
  }

  return invoke<DiscoveryRunSnapshot[]>("list_discovery_task_history", {
    limit,
  });
}

export async function startDiscoveryTask(
  request: DiscoveryTaskRequest,
): Promise<DiscoveryRunSnapshot> {
  if (!isTauriRuntime()) {
    const now = new Date().toISOString();
    const snapshot: DiscoveryRunSnapshot = {
      id: Date.now(),
      status: "completed",
      completionReason: "target_reached",
      syncMode: request.syncMode,
      targetAddedGames: Math.max(1, request.targetAddedGames),
      pageSize: Math.max(1, request.pageSize),
      pagesProcessed: 1,
      scannedApps: Math.max(1, request.pageSize),
      addedGames: Math.max(1, request.targetAddedGames),
      addedNewGames: Math.max(1, request.targetAddedGames),
      addedClassicGames: 0,
      skippedExisting: 0,
      skippedNonMultiplayer: 0,
      failedGames: 0,
      currentAppid: null,
      lastAppid: mockDashboard.recentDiscoveries[0]?.appid ?? null,
      haveMoreResults: false,
      startedAt: now,
      updatedAt: now,
      finishedAt: now,
      lastError: null,
      failures: [],
      progressPercent: 100,
    };

    upsertMockDiscoverySnapshot(snapshot);
    return cloneDiscoverySnapshot(snapshot);
  }

  return invoke<DiscoveryRunSnapshot>("start_discovery_task", { request });
}

export async function pauseDiscoveryTask(): Promise<DiscoveryRunSnapshot> {
  if (!isTauriRuntime()) {
    const current = mockDiscoveryTaskHistory[0];
    if (!current) {
      throw new Error("No discovery task is available in browser mode.");
    }

    const snapshot: DiscoveryRunSnapshot = {
      ...cloneDiscoverySnapshot(current),
      status: "paused",
      completionReason: "paused",
      currentAppid: null,
      updatedAt: new Date().toISOString(),
      finishedAt: null,
    };
    upsertMockDiscoverySnapshot(snapshot);
    return cloneDiscoverySnapshot(snapshot);
  }

  return invoke<DiscoveryRunSnapshot>("pause_discovery_task");
}

export async function resumeDiscoveryTask(): Promise<DiscoveryRunSnapshot> {
  if (!isTauriRuntime()) {
    const current = mockDiscoveryTaskHistory[0];
    if (!current) {
      throw new Error("No discovery task is available in browser mode.");
    }

    const snapshot: DiscoveryRunSnapshot = {
      ...cloneDiscoverySnapshot(current),
      status: "running",
      completionReason: null,
      finishedAt: null,
      updatedAt: new Date().toISOString(),
    };
    upsertMockDiscoverySnapshot(snapshot);
    return cloneDiscoverySnapshot(snapshot);
  }

  return invoke<DiscoveryRunSnapshot>("resume_discovery_task");
}

export async function cancelDiscoveryTask(): Promise<DiscoveryRunSnapshot> {
  if (!isTauriRuntime()) {
    const current = mockDiscoveryTaskHistory[0];
    if (!current) {
      throw new Error("No discovery task is available in browser mode.");
    }

    const now = new Date().toISOString();
    const snapshot: DiscoveryRunSnapshot = {
      ...cloneDiscoverySnapshot(current),
      status: "cancelled",
      completionReason: "cancelled",
      currentAppid: null,
      updatedAt: now,
      finishedAt: now,
    };
    upsertMockDiscoverySnapshot(snapshot);
    return cloneDiscoverySnapshot(snapshot);
  }

  return invoke<DiscoveryRunSnapshot>("cancel_discovery_task");
}

export async function startClassicDiscoveryTask(
  maxPages?: number,
): Promise<ClassicDiscoveryRunSnapshot> {
  if (!isTauriRuntime()) {
    return {
      id: Date.now(),
      status: "running",
      maxPages: clampClassicDiscoveryMaxPages(maxPages) ?? 3,
      pageSize: 100,
      pagesProcessed: 0,
      scannedApps: 0,
      consideredApps: 0,
      addedGames: 0,
      rejectedGames: 0,
      skippedExisting: 0,
      skippedRejectedCache: 0,
      failedGames: 0,
      currentAppid: null,
      lastAppid: null,
      consecutiveEmptyPages: 0,
      ruleVersion: "classic_v2",
      startedAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
      finishedAt: null,
      lastError: null,
    };
  }

  return invoke<ClassicDiscoveryRunSnapshot>("start_classic_discovery_task", {
    request: { maxPages: clampClassicDiscoveryMaxPages(maxPages) } satisfies ClassicDiscoveryTaskRequest,
  });
}

function clampClassicDiscoveryMaxPages(value?: number): number | undefined {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return undefined;
  }

  return Math.min(3, Math.max(1, Math.round(value)));
}

function refreshMockCollections() {
  const games = allMockGames();
  mockDashboard.collections = {
    favorites: games.filter((game) => game.userState.favorite),
    wishlist: games.filter((game) => game.userState.wishlist),
    followed: games.filter((game) => game.userState.followed),
    history: games.filter((game) => game.userState.viewed),
  };
}

function buildMockRecommendationResponse(
  request: AiRecommendationRequest,
): AiRecommendationResponse {
  const query = [
    ...request.contextMessages.slice(-4).map((message) => message.content),
    request.prompt,
  ]
    .join(" ")
    .toLocaleLowerCase();
  const wantsLocal = /本地|同屏|分屏|local|couch/.test(query);
  const wantsCasual = /轻松|休闲|可爱|casual|cozy|cute/.test(query);
  const wantsSurvival = /生存|survival/.test(query);
  const excludePixel = /不要像素|非像素|no pixel/.test(query);

  const items = allMockGames()
    .filter((game) => game.releaseState === "released")
    .filter((game) => !excludePixel || !game.tags.join(" ").toLocaleLowerCase().includes("pixel"))
    .map((game): AiRecommendedGame => {
      const corpus = [
        game.name,
        game.aiSummary,
        game.tags.join(" "),
        game.multiplayerModes.join(" "),
      ]
        .join(" ")
        .toLocaleLowerCase();
      const matchedTraits: string[] = [];
      const missingTraits: string[] = [];

      if (wantsLocal) {
        if (/local|split|本地|同屏|分屏/.test(corpus)) {
          matchedTraits.push("本地合作");
        } else {
          missingTraits.push("本地合作");
        }
      }
      if (wantsCasual) {
        if (/casual|cute|cozy|轻松|休闲|解谜|派对/.test(corpus)) {
          matchedTraits.push("轻松休闲");
        } else {
          missingTraits.push("轻松休闲");
        }
      }
      if (wantsSurvival) {
        if (/survival|生存/.test(corpus)) {
          matchedTraits.push("生存玩法");
        } else {
          missingTraits.push("生存玩法");
        }
      }

      const matchScore = Math.min(
        100,
        Math.max(
          0,
          (game.aiScore ?? game.recommendationScore) +
            matchedTraits.length * 8 -
            missingTraits.length * 5,
        ),
      );

      return {
        game,
        matchScore,
        reason:
          matchedTraits.length > 0
            ? `匹配${matchedTraits.join("、")}，${game.aiSummary}`
            : game.aiSummary,
        matchedTraits,
        missingTraits,
        caveats:
          missingTraits.length > 0
            ? [`缺少：${missingTraits.join("、")}`]
            : ["浏览器预览使用本地模拟推荐"],
        exactMatch: matchedTraits.length > 0 && missingTraits.length === 0,
      };
    })
    .filter((item) => item.matchedTraits.length > 0 || item.missingTraits.length === 0)
    .sort((left, right) => right.matchScore - left.matchScore)
    .slice(0, request.limit ?? 5);

  const exactMatchCount = items.filter((item) => item.exactMatch).length;

  return {
    reply:
      exactMatchCount === items.length
        ? `浏览器预览模式：找到 ${items.length} 个已入库已发售候选。`
        : "浏览器预览模式：没有完全匹配，我先按最接近的条件推荐。",
    followUpQuestion:
      exactMatchCount < items.length ? "你更愿意放宽人数、玩法还是联机方式？" : null,
    exactMatchCount,
    source: "rule",
    llmUsed: false,
    diagnostic: "浏览器预览模式使用本地规则匹配，没有调用真实 LLM。",
    items,
  };
}
