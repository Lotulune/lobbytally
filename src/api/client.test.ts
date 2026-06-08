import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  __resetMockGameAnalysisCacheForTests,
  assessGameWithAi,
  discoverSteamGames,
  generateGameAnalysis,
  getDashboard,
  getGameAnalysis,
  getUserCollections,
  isTauriRuntime,
  recommendGamesWithAi,
  refreshAllGameAnalyses,
  retryAiAnalysisJob,
  setGameUserState,
  startClassicDiscoveryTask,
  startDiscoveryTask,
  syncSeedGames,
} from "./client";
import { mockDashboard } from "../data/mockDashboard";
import {
  clearCurrentServiceConnection,
  saveCurrentServiceConnection,
} from "../domain/serviceConnectionStorage";
import type { GameAnalysisReport, ServiceInfo } from "../types";

const { invokeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

const compatibleInfo: ServiceInfo = {
  serviceInstanceId: "018fb770-8998-7699-a6e4-b7b59f2f9c01",
  serviceName: "MPGS Test Service",
  serviceVersion: "0.1.0",
  apiVersion: "v1",
  publicCatalogStatus: "ready",
  capabilities: ["public_catalog_read"],
};

function configureServiceConnection() {
  saveCurrentServiceConnection({
    baseUrl: "https://mpgs.example.test",
    info: compatibleInfo,
    validatedAt: "2026-06-08T00:00:00.000Z",
  });
}

const publicHomePayload = {
  status: "ready",
  totalGames: 2,
  sections: {
    newlyPublished: [
      {
        appid: 3744430,
        name: "Together Moon Escape",
        recommendationScore: 92,
        updatedAt: "2026-06-08T00:00:00Z",
      },
    ],
    highConfidence: [
      {
        appid: 548430,
        name: "Deep Rock Galactic",
        recommendationScore: 95,
        updatedAt: "2026-06-08T00:00:00Z",
      },
    ],
    recentlyAdded: [],
  },
};

const publicGamesPayload = {
  items: [
    {
      appid: 3744430,
      name: "Together Moon Escape",
      recommendationScore: 92,
      updatedAt: "2026-06-08T00:00:00Z",
    },
    {
      appid: 548430,
      name: "Deep Rock Galactic",
      recommendationScore: 95,
      updatedAt: "2026-06-08T00:00:00Z",
    },
  ],
  page: { limit: 100, offset: 0, total: 2 },
};

function fetchJson(
  value: unknown,
  status = 200,
  headers: Record<string, string> = {},
) {
  const normalizedHeaders = new Map(
    Object.entries(headers).map(([key, headerValue]) => [
      key.toLowerCase(),
      headerValue,
    ]),
  );

  return Promise.resolve({
    ok: status >= 200 && status < 300,
    status,
    headers: {
      get: (name: string) => normalizedHeaders.get(name.toLowerCase()) ?? null,
    },
    json: async () => value,
  } as Response);
}

function installPublicServiceFetch(report?: GameAnalysisReport) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url = String(input);
    if (url === "https://mpgs.example.test/api/v1/discovery-home") {
      return fetchJson(publicHomePayload);
    }

    if (url === "https://mpgs.example.test/api/v1/games?limit=100&offset=0") {
      return fetchJson(publicGamesPayload);
    }

    if (url === "https://mpgs.example.test/api/v1/games/548430/analysis" && report) {
      return fetchJson({
        appid: 548430,
        generatedAt: report.generatedAt,
        report,
      });
    }

    if (url === "https://mpgs.example.test/api/v1/games/548430/analysis") {
      return fetchJson({ error: { code: "public_game_not_found" } }, 404);
    }

    throw new Error(`Unexpected URL: ${url}`);
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function setTauriRuntime(enabled: boolean) {
  if (enabled) {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    return;
  }

  delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
}

describe("game analysis client", () => {
  beforeEach(() => {
    __resetMockGameAnalysisCacheForTests();
    window.localStorage.clear();
    clearCurrentServiceConnection();
    invokeMock.mockReset();
    setTauriRuntime(false);
    vi.unstubAllGlobals();
  });

  it("returns null before a browser-mode report has been generated", async () => {
    expect(isTauriRuntime()).toBe(false);

    await expect(getGameAnalysis(3744430)).resolves.toBeNull();
  });

  it("caches a browser-mode report after generation", async () => {
    const generated = await generateGameAnalysis(3087930, false);
    const cached = await getGameAnalysis(3087930);

    expect(generated.appid).toBe(3087930);
    expect(generated.overview.length).toBeGreaterThan(0);
    expect(cached).toEqual(generated);
  });

  it("overwrites the cached browser-mode report when force refresh is enabled", async () => {
    const first = await generateGameAnalysis(548430, false);
    const second = await generateGameAnalysis(548430, true);
    const cached = await getGameAnalysis(548430);

    expect(second.appid).toBe(548430);
    expect(second.generatedAt).not.toBe(first.generatedAt);
    expect(second.overview).not.toBe(first.overview);
    expect(cached).toEqual(second);
  });

  it("fetches public dashboard data from the configured service instead of Tauri commands", async () => {
    configureServiceConnection();
    const fetchMock = installPublicServiceFetch();

    const dashboard = await getDashboard();

    expect(invokeMock).not.toHaveBeenCalled();
    expect(fetchMock).toHaveBeenCalledWith(
      "https://mpgs.example.test/api/v1/discovery-home",
      expect.objectContaining({ method: "GET" }),
    );
    expect(fetchMock).toHaveBeenCalledWith(
      "https://mpgs.example.test/api/v1/games?limit=100&offset=0",
      expect.objectContaining({ method: "GET" }),
    );
    expect(dashboard.newGames[0]).toMatchObject({
      appid: 3744430,
      name: "Together Moon Escape",
      recommendationScore: 92,
      userState: {
        favorite: false,
        wishlist: false,
        followed: false,
        viewed: false,
      },
    });
    expect(dashboard.classics[0]).toMatchObject({
      appid: 548430,
      name: "Deep Rock Galactic",
    });
    expect(dashboard.stats).toMatchObject({
      totalGames: 2,
      newGamesCount: 1,
      classicGamesCount: 1,
      dataSource: "公共发现服务：MPGS Test Service",
    });
    expect(dashboard.config.onboardingCompleted).toBe(true);
  });

  it("uses ETags and cached public dashboard bodies for 304 responses", async () => {
    configureServiceConnection();
    let homeRequests = 0;
    let gamesRequests = 0;
    const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === "https://mpgs.example.test/api/v1/discovery-home") {
        homeRequests += 1;
        if (homeRequests === 1) {
          return fetchJson(publicHomePayload, 200, { ETag: '"home-rev-1"' });
        }

        expect(init?.headers).toMatchObject({
          "If-None-Match": '"home-rev-1"',
        });
        return fetchJson(null, 304);
      }

      if (url === "https://mpgs.example.test/api/v1/games?limit=100&offset=0") {
        gamesRequests += 1;
        if (gamesRequests === 1) {
          return fetchJson(publicGamesPayload, 200, { ETag: '"games-rev-1"' });
        }

        expect(init?.headers).toMatchObject({
          "If-None-Match": '"games-rev-1"',
        });
        return fetchJson(null, 304);
      }

      throw new Error(`Unexpected URL: ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);

    const firstDashboard = await getDashboard();
    const secondDashboard = await getDashboard();

    expect(firstDashboard.newGames[0].name).toBe("Together Moon Escape");
    expect(secondDashboard.newGames[0].name).toBe("Together Moon Escape");
    expect(secondDashboard.classics[0].name).toBe("Deep Rock Galactic");
    expect(fetchMock).toHaveBeenCalledTimes(4);
  });

  it("falls back to the cached public dashboard snapshot when the service is unreachable", async () => {
    configureServiceConnection();
    const fetchMock = vi
      .fn()
      .mockImplementationOnce(() =>
        fetchJson(publicHomePayload, 200, { ETag: '"home-rev-1"' }),
      )
      .mockImplementationOnce(() =>
        fetchJson(publicGamesPayload, 200, { ETag: '"games-rev-1"' }),
      )
      .mockRejectedValue(new Error("offline"));
    vi.stubGlobal("fetch", fetchMock);

    await expect(getDashboard()).resolves.toMatchObject({
      stats: { totalGames: 2 },
    });

    await expect(getDashboard()).resolves.toMatchObject({
      stats: {
        totalGames: 2,
        dataSource: "公共发现服务：MPGS Test Service",
      },
      classics: [{ appid: 548430, name: "Deep Rock Galactic" }],
    });
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("keeps the old Tauri dashboard command as a fallback when no service is configured", async () => {
    setTauriRuntime(true);
    invokeMock.mockResolvedValueOnce(mockDashboard);
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    await expect(getDashboard()).resolves.toBe(mockDashboard);

    expect(invokeMock).toHaveBeenCalledWith("get_dashboard");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("reads public analysis from the configured service with GET requests only", async () => {
    configureServiceConnection();
    const report: GameAnalysisReport = {
      appid: 548430,
      generatedAt: "2026-06-08T00:00:00Z",
      source: "rule",
      confidence: "high",
      overallScore: 95,
      overview: "Public rule analysis.",
      dimensionScores: [],
      strengths: [],
      risks: [],
      evidence: [],
      reviewEvidence: [],
    };
    const fetchMock = installPublicServiceFetch(report);

    await expect(getGameAnalysis(548430)).resolves.toEqual(report);
    await expect(generateGameAnalysis(548430, true)).resolves.toEqual(report);

    expect(invokeMock).not.toHaveBeenCalled();
    expect(fetchMock).toHaveBeenCalledWith(
      "https://mpgs.example.test/api/v1/games/548430/analysis",
      expect.objectContaining({ method: "GET" }),
    );
    expect(fetchMock).not.toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("keeps personal game state local and merges it into public REST dashboards", async () => {
    configureServiceConnection();
    const fetchMock = installPublicServiceFetch();

    await expect(setGameUserState(548430, { favorite: true })).resolves.toMatchObject({
      favorite: true,
    });
    const dashboard = await getDashboard();

    expect(fetchMock).not.toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({ method: "POST" }),
    );
    expect(dashboard.classics[0].userState).toMatchObject({
      favorite: true,
      wishlist: false,
      followed: false,
      viewed: false,
    });
    expect(dashboard.collections.favorites).toHaveLength(1);
    expect(dashboard.collections.favorites[0]).toMatchObject({
      appid: 548430,
      name: "Deep Rock Galactic",
    });
  });

  it("reads service-mode collections from public REST data instead of local Tauri SQLite", async () => {
    configureServiceConnection();
    setTauriRuntime(true);
    const fetchMock = installPublicServiceFetch();

    await setGameUserState(548430, { wishlist: true });
    const collections = await getUserCollections();

    expect(invokeMock).not.toHaveBeenCalled();
    expect(fetchMock).toHaveBeenCalledWith(
      "https://mpgs.example.test/api/v1/discovery-home",
      expect.objectContaining({ method: "GET" }),
    );
    expect(fetchMock).toHaveBeenCalledWith(
      "https://mpgs.example.test/api/v1/games?limit=100&offset=0",
      expect.objectContaining({ method: "GET" }),
    );
    expect(collections.wishlist).toHaveLength(1);
    expect(collections.wishlist[0]).toMatchObject({
      appid: 548430,
      name: "Deep Rock Galactic",
    });
  });

  it("freezes legacy client-side discovery, sync, and AI command paths when a public service is configured", async () => {
    configureServiceConnection();
    setTauriRuntime(true);

    const blockedActions = [
      () => syncSeedGames("full"),
      () => discoverSteamGames(1, 10),
      () => assessGameWithAi(548430),
      () =>
        recommendGamesWithAi({
          prompt: "recommend co-op games",
          contextMessages: [],
          limit: 3,
        }),
      () => refreshAllGameAnalyses(2),
      () => retryAiAnalysisJob(548430),
      () =>
        startDiscoveryTask({
          syncMode: "full",
          targetAddedGames: 5,
          pageSize: 25,
        }),
      () => startClassicDiscoveryTask(2),
    ];

    for (const runAction of blockedActions) {
      await expect(runAction()).rejects.toThrow(
        "公共发现服务模式下，客户端不会执行本地同步、发现或 AI 任务。",
      );
    }

    expect(invokeMock).not.toHaveBeenCalled();
  });
});
