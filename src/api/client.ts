import { invoke } from "@tauri-apps/api/core";
import { mockDashboard } from "../data/mockDashboard";
import type {
  AiAssessment,
  DashboardPayload,
  DiscoveryRunSnapshot,
  DiscoveryTaskRequest,
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
} from "../types";

export const isTauriRuntime = () => "__TAURI_INTERNALS__" in window;

const mockDiscoveryTaskHistory: DiscoveryRunSnapshot[] = [];

function allMockGames() {
  return [...mockDashboard.upcoming, ...mockDashboard.newGames, ...mockDashboard.classics];
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
  if (!isTauriRuntime()) return mockDashboard;
  return invoke<DashboardPayload>("get_dashboard");
}

export async function saveConfig(
  request: SaveConfigRequest,
): Promise<PublicConfig> {
  if (!isTauriRuntime()) {
    return {
      ...mockDashboard.config,
      steamApiKeyConfigured:
        Boolean(request.steamApiKey?.trim()) ||
        mockDashboard.config.steamApiKeyConfigured,
      llmApiKeyConfigured:
        Boolean(request.llmApiKey?.trim()) ||
        mockDashboard.config.llmApiKeyConfigured,
      llmBaseUrl: request.llmBaseUrl || mockDashboard.config.llmBaseUrl,
      llmModel: request.llmModel || mockDashboard.config.llmModel,
      country: request.country || mockDashboard.config.country,
      language: request.language || mockDashboard.config.language,
    };
  }
  return invoke<PublicConfig>("save_config", { request });
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

export async function setGameUserState(
  appid: number,
  patch: UserGameStatePatch,
): Promise<UserGameState> {
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
      currentAppid: null,
      updatedAt: now,
      finishedAt: now,
    };
    upsertMockDiscoverySnapshot(snapshot);
    return cloneDiscoverySnapshot(snapshot);
  }

  return invoke<DiscoveryRunSnapshot>("cancel_discovery_task");
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
