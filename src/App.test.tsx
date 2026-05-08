// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { mockDashboard } from "./data/mockDashboard";
import type { AiRecommendationResponse, GameAnalysisReport } from "./types";

const assessGameWithAiMock = vi.fn();
const getDashboardMock = vi.fn();
const getGameAnalysisMock = vi.fn();
const generateGameAnalysisMock = vi.fn();
const recommendGamesWithAiMock = vi.fn();
const retryAiAnalysisJobMock = vi.fn();
const startClassicDiscoveryTaskMock = vi.fn();
const syncSeedGamesMock = vi.fn();
const isTauriRuntimeMock = vi.fn(() => false);
const listenMock = vi.fn();

vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => listenMock(...args),
}));

vi.mock("./api/client", async () => {
  const actual = await vi.importActual<typeof import("./api/client")>("./api/client");

  return {
    ...actual,
    assessGameWithAi: (...args: unknown[]) => assessGameWithAiMock(...args),
    getDashboard: () => getDashboardMock(),
    getGameAnalysis: (...args: unknown[]) => getGameAnalysisMock(...args),
    generateGameAnalysis: (...args: unknown[]) => generateGameAnalysisMock(...args),
    isTauriRuntime: () => isTauriRuntimeMock(),
    previewSteamAppList: vi.fn(),
    recommendGamesWithAi: (...args: unknown[]) => recommendGamesWithAiMock(...args),
    retryAiAnalysisJob: (...args: unknown[]) => retryAiAnalysisJobMock(...args),
    saveConfig: vi.fn(),
    setGameUserState: vi.fn(),
    startClassicDiscoveryTask: (...args: unknown[]) => startClassicDiscoveryTaskMock(...args),
    syncSeedGames: (...args: unknown[]) => syncSeedGamesMock(...args),
  };
});

function buildDashboard() {
  return structuredClone(mockDashboard);
}

function createGames(count: number, prefix: string) {
  return Array.from({ length: count }, (_, index) => {
    const template = mockDashboard.newGames[index % mockDashboard.newGames.length];

    return {
      ...template,
      appid: template.appid + 70_000 + index,
      name: `${prefix} ${index + 1}`,
      userState: { ...template.userState },
    };
  });
}

function buildPagedDashboard() {
  const dashboard = buildDashboard();
  dashboard.newGames = createGames(13, "分页新游");
  dashboard.classics = [];
  dashboard.recentDiscoveries = [];
  dashboard.stats = {
    ...dashboard.stats,
    totalGames: dashboard.upcoming.length + dashboard.newGames.length,
    newGamesCount: dashboard.newGames.length,
    classicGamesCount: 0,
  };
  return dashboard;
}

function buildLowActivityDiscoveryDashboard() {
  const dashboard = structuredClone(mockDashboard);
    const lowActivityGame = {
      ...dashboard.newGames[0],
      appid: 4999001,
      name: "Quiet Co-op Debut",
      isFree: false,
      positiveReviewPct: 0,
    totalReviews: 0,
    currentPlayers: 0,
    recommendationScore: 12,
    aiScore: 12,
    userState: {
      favorite: false,
      wishlist: false,
      followed: false,
      viewed: false,
      updatedAt: null,
    },
  };

  dashboard.newGames = [lowActivityGame];
  dashboard.classics = [];
  dashboard.upcoming = [];
  dashboard.recentDiscoveries = [lowActivityGame];
  dashboard.collections = {
    favorites: [],
    wishlist: [],
    followed: [],
    history: [],
  };
  dashboard.hiddenGames = [];
  dashboard.stats = {
    ...dashboard.stats,
    seedCount: 1,
    totalGames: 1,
    newGamesCount: 1,
    classicGamesCount: 0,
  };

  return dashboard;
}

function buildBackfillDashboard() {
  const dashboard = structuredClone(mockDashboard);
  dashboard.stats = {
    ...dashboard.stats,
    backfillPendingCount: 3,
    backfillRunning: true,
    backfillCurrentAppid: 730123,
    backfillCurrentAttempt: 1,
  };

  return dashboard;
}

function buildAiBatchRefreshDashboard() {
  const dashboard = structuredClone(mockDashboard);
  dashboard.stats = {
    ...dashboard.stats,
    aiBatchRefreshRunning: true,
    aiBatchRefreshConcurrency: 5,
    aiBatchRefreshPendingCount: 12,
    aiBatchRefreshActiveCount: 5,
    aiBatchRefreshTotalCount: 20,
    aiBatchRefreshProcessedCount: 8,
    aiBatchRefreshUpdatedCount: 7,
    aiBatchRefreshFailedCount: 1,
    aiBatchRefreshLastError: null,
    aiBatchRefreshLastErrorAppid: null,
  };

  return dashboard;
}

function buildClassicDiscoveryDashboard() {
  const dashboard = structuredClone(mockDashboard);
  dashboard.stats = {
    ...dashboard.stats,
    classicDiscoveryRunning: true,
    classicDiscoveryStatus: "running",
    classicDiscoveryCurrentAppid: 730456,
    classicDiscoveryLastAppid: 730450,
    classicDiscoveryScannedApps: 12,
    classicDiscoveryAddedGames: 1,
    classicDiscoveryRejectedGames: 10,
    classicDiscoveryFailedGames: 0,
  };

  return dashboard;
}

function buildDashboardWithClassicHidden() {
  const dashboard = structuredClone(mockDashboard);
  const hiddenGame = {
    ...dashboard.classics[0],
    appid: 8_888_001,
    name: "Hidden Signal Ops",
    section: "classic_hidden",
    isFree: true,
    aiSummary: "质量一般但仍值得搜索观察。",
    userState: {
      favorite: true,
      wishlist: false,
      followed: true,
      viewed: true,
      updatedAt: "2026-05-05T10:10:00.000Z",
    },
  };

  dashboard.classics = dashboard.classics.filter((game) => game.appid !== hiddenGame.appid);
  dashboard.collections = {
    favorites: [hiddenGame],
    wishlist: [],
    followed: [hiddenGame],
    history: [hiddenGame],
  };
  dashboard.hiddenGames = [hiddenGame];

  return { dashboard, hiddenGame };
}

function buildAnalysisReport(
  appid: number,
  overrides: Partial<GameAnalysisReport> = {},
): GameAnalysisReport {
  return {
    appid,
    generatedAt: "2026-04-30T12:45:00.000Z",
    source: "hybrid",
    confidence: "high",
    overallScore: 92,
    overview: "打开详情页后应直接显示缓存分析。",
    dimensionScores: [
      {
        key: "approachability",
        label: "易上手度",
        score: 88,
        reason: "回归测试夹具。",
      },
      {
        key: "multiplayer_fun",
        label: "联机乐趣",
        score: 94,
        reason: "回归测试夹具。",
      },
      {
        key: "content_depth",
        label: "内容深度",
        score: 86,
        reason: "回归测试夹具。",
      },
      {
        key: "reputation_stability",
        label: "口碑稳定性",
        score: 95,
        reason: "回归测试夹具。",
      },
      {
        key: "activity_health",
        label: "活跃健康度",
        score: 90,
        reason: "回归测试夹具。",
      },
    ],
    strengths: [{ title: "缓存可见", reason: "详情页直接显示摘要。" }],
    risks: [{ title: "无", reason: "纯回归夹具。" }],
    evidence: [
      {
        kind: "positive_review_pct",
        label: "好评率",
        value: "97%",
        interpretation: "回归测试夹具。",
      },
    ],
    reviewEvidence: [],
    ...overrides,
  };
}

function getGameTitles(sectionHeading: string) {
  const heading = screen.getByRole("heading", { name: sectionHeading });
  const section = heading.closest(".game-section");

  if (!(section instanceof HTMLElement)) {
    throw new Error(`Missing game section for ${sectionHeading}`);
  }

  return within(section)
    .getAllByRole("heading", { level: 3 })
    .map((node) => node.textContent);
}

function createDeferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });

  return { promise, resolve, reject };
}

describe("App dashboard interactions", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    isTauriRuntimeMock.mockReturnValue(false);
    listenMock.mockResolvedValue(() => undefined);
    getDashboardMock.mockResolvedValue(buildDashboard());
    assessGameWithAiMock.mockResolvedValue({
      appid: 0,
      score: 80,
      summary: "AI 评估结果",
      bestFor: [],
      risks: [],
    });
    getGameAnalysisMock.mockResolvedValue(null);
    generateGameAnalysisMock.mockImplementation(async (appid: number) =>
      buildAnalysisReport(appid),
    );
    recommendGamesWithAiMock.mockResolvedValue({
      reply: "我在已入库且已发售的游戏里找到了 1 个匹配候选。",
      followUpQuestion: null,
      exactMatchCount: 1,
      source: "rule",
      llmUsed: false,
      diagnostic: "测试默认使用规则匹配。",
      items: [],
    });
    retryAiAnalysisJobMock.mockResolvedValue({
      totalGames: 1,
      updatedGames: 0,
      failedGames: 0,
      message: "已重新加入 AI 分析队列。",
    });
    syncSeedGamesMock.mockResolvedValue({
      updatedGames: 0,
      failedGames: 0,
      message: "已启动 Steam 同步任务。",
    });
    startClassicDiscoveryTaskMock.mockResolvedValue({
      id: 99,
      status: "running",
      maxPages: 3,
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
      startedAt: "2026-05-05T10:00:00Z",
      updatedAt: "2026-05-05T10:00:00Z",
      finishedAt: null,
      lastError: null,
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    cleanup();
  });

  it("renders sort controls as direct-action buttons instead of a native combobox", async () => {
    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });

    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "综合排序" })).toHaveAttribute("aria-pressed", "true");
  });

  it("reorders the new games section when clicking the players sort button", async () => {
    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });

    expect(getGameTitles("新游区")).toEqual([
      "Together Moon Escape",
      "Pebble Knights",
      "Burglin' Gnomes",
      "Void Crew",
    ]);

    fireEvent.click(screen.getByRole("button", { name: "游玩人数" }));

    await waitFor(() =>
      expect(getGameTitles("新游区")).toEqual([
        "Void Crew",
        "Together Moon Escape",
        "Pebble Knights",
        "Burglin' Gnomes",
      ]),
    );

    expect(screen.getByRole("button", { name: "游玩人数" })).toHaveAttribute("aria-pressed", "true");
  });

  it("opens the full new-games view when clicking the first 查看全部 action", async () => {
    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });

    fireEvent.click(screen.getAllByRole("button", { name: "查看全部 〉" })[0]);

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "新游区" })).toBeInTheDocument();
      expect(screen.queryByRole("heading", { name: "精品老游区" })).not.toBeInTheDocument();
      expect(screen.queryByRole("heading", { name: "最近发现" })).not.toBeInTheDocument();
    });
  });

  it("filters dashboard cards when clicking the demo status tabs", async () => {
    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });

    fireEvent.click(screen.getByRole("button", { name: "仅 Demo" }));

    await waitFor(() => {
      expect(getGameTitles("新游区")).toEqual([
        "Together Moon Escape",
        "Pebble Knights",
      ]);
      expect(screen.queryByRole("heading", { name: "精品老游区" })).not.toBeInTheDocument();
    });
  });

  it("keeps classic_hidden out of default sections but still searchable through browse and collections", async () => {
    const { dashboard, hiddenGame } = buildDashboardWithClassicHidden();
    getDashboardMock.mockResolvedValue(dashboard);

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });
    expect(screen.queryByText(hiddenGame.name)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "浏览全部" }));
    fireEvent.change(screen.getByPlaceholderText("搜索游戏名称、类型、标签..."), {
      target: { value: "Hidden Signal Ops" },
    });

    await waitFor(() =>
      expect(screen.getAllByText(hiddenGame.name).length).toBeGreaterThan(0),
    );

    fireEvent.click(screen.getByRole("button", { name: "收藏夹" }));
    await waitFor(() =>
      expect(screen.getAllByText(hiddenGame.name).length).toBeGreaterThan(0),
    );
  });

  it("applies tag selections from the filter page", async () => {
    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });

    fireEvent.click(screen.getByRole("button", { name: "筛选器" }));
    await screen.findByRole("button", { name: "应用筛选" });

    fireEvent.click(screen.getByRole("button", { name: "射击" }));
    fireEvent.click(screen.getByRole("button", { name: "应用筛选" }));

    await waitFor(() => {
      expect(getGameTitles("精品老游区")).toEqual([
        "Deep Rock Galactic",
        "Left 4 Dead 2",
      ]);
      expect(screen.queryByRole("heading", { name: "新游区" })).not.toBeInTheDocument();
    });
  });

  it("filters immediately from the right-rail quick tags", async () => {
    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });

    fireEvent.click(screen.getByRole("button", { name: "解谜" }));

    await waitFor(() =>
      expect(getGameTitles("新游区")).toEqual(["Together Moon Escape"]),
    );
  });

  it("keeps newly discovered low-activity games visible by default", async () => {
    getDashboardMock.mockResolvedValue(buildLowActivityDiscoveryDashboard());

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });
    expect(screen.getByRole("heading", { name: "最近发现" })).toBeInTheDocument();
    expect(screen.getAllByText("Quiet Co-op Debut").length).toBeGreaterThan(0);
  });

  it("polls for dashboard refresh while metadata backfill is running", async () => {
    vi.useFakeTimers();
    getDashboardMock.mockResolvedValue(buildBackfillDashboard());

    render(<App />);

    await act(async () => {
      await Promise.resolve();
    });

    expect(screen.getByRole("heading", { name: "新游区" })).toBeInTheDocument();
    expect(getDashboardMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      vi.advanceTimersByTime(2_200);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(getDashboardMock).toHaveBeenCalledTimes(2);
  });

  it("polls for dashboard refresh while AI batch refresh is running", async () => {
    vi.useFakeTimers();
    getDashboardMock.mockResolvedValue(buildAiBatchRefreshDashboard());

    render(<App />);

    await act(async () => {
      await Promise.resolve();
    });

    expect(screen.getByRole("heading", { name: "新游区" })).toBeInTheDocument();
    expect(getDashboardMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      vi.advanceTimersByTime(2_200);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(getDashboardMock).toHaveBeenCalledTimes(2);
  });

  it("polls for dashboard refresh while classic discovery is running", async () => {
    vi.useFakeTimers();
    getDashboardMock.mockResolvedValue(buildClassicDiscoveryDashboard());

    render(<App />);

    await act(async () => {
      await Promise.resolve();
    });

    expect(screen.getByRole("heading", { name: "新游区" })).toBeInTheDocument();
    expect(getDashboardMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      vi.advanceTimersByTime(2_200);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(getDashboardMock).toHaveBeenCalledTimes(2);
  });

  it("does not reopen auto onboarding after dismissing it in the same session", async () => {
    isTauriRuntimeMock.mockReturnValue(true);
    const dashboard = buildDashboard();
    dashboard.config.onboardingCompleted = false;
    getDashboardMock.mockResolvedValue(dashboard);

    render(<App />);

    expect(await screen.findByRole("heading", { name: "准备 Steam Web API" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "稍后设置" }));

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "新游区" })).toBeInTheDocument();
    });

    fireEvent.click(screen.getByRole("button", { name: "设置" }));

    await waitFor(() => {
      expect(screen.getByRole("button", { name: /初始化向导/ })).toBeInTheDocument();
    });

    expect(screen.queryByRole("heading", { name: "准备 Steam Web API" })).not.toBeInTheDocument();
  });

  it("refreshes the dashboard when discovery task events arrive in tauri runtime", async () => {
    vi.useFakeTimers();
    isTauriRuntimeMock.mockReturnValue(true);
    let discoveryEventHandler:
      | ((event: { payload: unknown }) => void)
      | null = null;
    listenMock.mockImplementation(
      async (eventName: string, handler: (event: { payload: unknown }) => void) => {
        if (eventName === "discovery-task-updated") {
          discoveryEventHandler = handler;
        }
        return () => undefined;
      },
    );
    getDashboardMock.mockResolvedValue(buildDashboard());

    render(<App />);

    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(getDashboardMock).toHaveBeenCalledTimes(1);
    expect(discoveryEventHandler).not.toBeNull();

    await act(async () => {
      discoveryEventHandler?.({ payload: { status: "running" } });
      await Promise.resolve();
    });

    await act(async () => {
      vi.advanceTimersByTime(2_200);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(getDashboardMock).toHaveBeenCalledTimes(2);
  });

  it("shows a toast when classic discovery finishes and dismisses it on click", async () => {
    vi.useFakeTimers();
    const runningDashboard = buildClassicDiscoveryDashboard();
    const completedDashboard = buildDashboard();
    completedDashboard.stats = {
      ...completedDashboard.stats,
      classicDiscoveryRunning: false,
      classicDiscoveryStatus: "completed",
      classicDiscoveryScannedApps: 146,
      classicDiscoveryAddedGames: 8,
    };

    getDashboardMock
      .mockResolvedValueOnce(runningDashboard)
      .mockResolvedValue(completedDashboard);

    render(<App />);

    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    await act(async () => {
      vi.advanceTimersByTime(2_200);
      await Promise.resolve();
      await Promise.resolve();
    });

    const toast = screen.getByRole("button", { name: /老游补库已完成/i });
    expect(within(toast).getByText("已新增 8 个老游戏，扫描 146 个候选。")).toBeInTheDocument();

    fireEvent.click(toast);

    expect(
      screen.queryByRole("button", { name: /老游补库已完成/i }),
    ).not.toBeInTheDocument();
  });

  it("auto dismisses task toast after hovering for three seconds and leaving", async () => {
    vi.useFakeTimers();
    const runningDashboard = buildAiBatchRefreshDashboard();
    const completedDashboard = buildDashboard();
    completedDashboard.stats = {
      ...completedDashboard.stats,
      aiBatchRefreshRunning: false,
      aiBatchRefreshTotalCount: 20,
      aiBatchRefreshProcessedCount: 20,
      aiBatchRefreshUpdatedCount: 19,
      aiBatchRefreshFailedCount: 1,
    };

    getDashboardMock
      .mockResolvedValueOnce(runningDashboard)
      .mockResolvedValue(completedDashboard);

    render(<App />);

    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    await act(async () => {
      vi.advanceTimersByTime(2_200);
      await Promise.resolve();
      await Promise.resolve();
    });

    const toast = screen.getByRole("button", { name: /AI 批量重算已完成/i });
    fireEvent.mouseEnter(toast);

    await act(async () => {
      vi.advanceTimersByTime(3_000);
      await Promise.resolve();
    });

    fireEvent.mouseLeave(toast);

    expect(
      screen.queryByRole("button", { name: /AI 批量重算已完成/i }),
    ).not.toBeInTheDocument();
  });

  it("clears the busy state after ai assess even if polling refresh overtakes loadDashboard", async () => {
    vi.useFakeTimers();
    const dashboard = buildAiBatchRefreshDashboard();
    const manualRefresh = createDeferred<typeof dashboard>();
    assessGameWithAiMock.mockResolvedValueOnce({
      appid: dashboard.newGames[0].appid,
      score: 91,
      summary: "AI 评估完成",
      bestFor: [],
      risks: [],
    });
    getDashboardMock
      .mockResolvedValueOnce(dashboard)
      .mockImplementationOnce(() => manualRefresh.promise)
      .mockResolvedValue(dashboard);

    render(<App />);

    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(screen.getByRole("heading", { name: "新游区" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "✦ 让 AI 帮我找游戏" }));

    const targetRow = screen
      .getByRole("heading", { name: dashboard.newGames[0].name })
      .closest(".recommend-row");
    if (!(targetRow instanceof HTMLElement)) {
      throw new Error("Missing AI recommendation row");
    }

    fireEvent.click(within(targetRow).getByRole("button", { name: "评估" }));
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(getDashboardMock).toHaveBeenCalledTimes(2);

    await act(async () => {
      vi.advanceTimersByTime(2_200);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(getDashboardMock).toHaveBeenCalledTimes(3);

    await act(async () => {
      manualRefresh.resolve(dashboard);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(within(targetRow).getByRole("button", { name: "评估" })).toBeEnabled();
  });

  it("routes full and quick sync requests with their selected mode", async () => {
    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });

    fireEvent.click(screen.getByRole("button", { name: "完整同步" }));
    expect(syncSeedGamesMock).toHaveBeenNthCalledWith(1, "full");

    await waitFor(() =>
      expect(screen.getByRole("button", { name: "快速同步" })).toBeEnabled(),
    );
    fireEvent.click(screen.getByRole("button", { name: "快速同步" }));
    expect(syncSeedGamesMock).toHaveBeenNthCalledWith(2, "quick");
  });

  it("opens a dashboard game card into detail view and shows cached analysis", async () => {
    const dashboard = buildDashboard();
    const report = buildAnalysisReport(dashboard.newGames[0].appid);
    getDashboardMock.mockResolvedValue(dashboard);
    getGameAnalysisMock.mockResolvedValue(report);
    generateGameAnalysisMock.mockResolvedValue(report);
    const scrollToMock = vi.fn();
    Object.defineProperty(window, "scrollTo", {
      configurable: true,
      value: scrollToMock,
      writable: true,
    });
    Object.defineProperty(window, "scrollY", {
      configurable: true,
      value: 580,
      writable: true,
    });

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });

    const newGamesSection = screen
      .getByRole("heading", { name: "新游区" })
      .closest(".game-section");
    if (!(newGamesSection instanceof HTMLElement)) {
      throw new Error("Missing 新游区 section");
    }

    fireEvent.click(
      within(newGamesSection).getByRole("button", {
        name: new RegExp(dashboard.newGames[0].name, "i"),
      }),
    );

    expect(await screen.findByText(report.overview)).toBeInTheDocument();
    expect(generateGameAnalysisMock).not.toHaveBeenCalled();
    expect(scrollToMock).toHaveBeenCalledWith({ top: 0, behavior: "auto" });
  });

  it("refreshes the selected detail game from the latest dashboard payload", async () => {
    const initialDashboard = buildDashboard();
    const updatedDashboard = buildDashboard();
    const targetAppid = initialDashboard.newGames[0].appid;
    const refreshedDescription = "刷新后的详情简介，应替换掉旧的选中游戏对象。";

    initialDashboard.newGames[0] = {
      ...initialDashboard.newGames[0],
      shortDescription: "旧的详情简介。",
    };
    updatedDashboard.newGames[0] = {
      ...updatedDashboard.newGames[0],
      shortDescription: refreshedDescription,
    };

    getDashboardMock
      .mockResolvedValueOnce(initialDashboard)
      .mockResolvedValue(updatedDashboard);
    getGameAnalysisMock.mockResolvedValue(buildAnalysisReport(targetAppid));

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });

    const newGamesSection = screen
      .getByRole("heading", { name: "新游区" })
      .closest(".game-section");
    if (!(newGamesSection instanceof HTMLElement)) {
      throw new Error("Missing 新游区 section");
    }

    fireEvent.click(
      within(newGamesSection).getByRole("button", {
        name: new RegExp(initialDashboard.newGames[0].name, "i"),
      }),
    );

    expect(await screen.findByText("旧的详情简介。")).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByText(refreshedDescription)).toBeInTheDocument(),
    );
  });

  it("syncs the dashboard card as soon as detail analysis is auto-generated", async () => {
    const dashboard = buildDashboard();
    const target = {
      ...dashboard.newGames[0],
      recommendationScore: 61,
      aiScore: null,
      aiSummary: "还没有 AI 评测。",
    };
    dashboard.newGames[0] = target;
    const generatedReport = buildAnalysisReport(target.appid, {
      overallScore: 97,
      overview: "首次打开详情后自动生成的 AI 分析。",
    });

    getDashboardMock.mockResolvedValue(dashboard);
    getGameAnalysisMock.mockResolvedValueOnce(null);
    generateGameAnalysisMock.mockResolvedValueOnce(generatedReport);

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });

    const getNewGamesSection = () =>
      screen.getByRole("heading", { name: "新游区" }).closest(".game-section") as HTMLElement;
    const getHomeCard = () =>
      within(getNewGamesSection()).getByRole("button", {
        name: new RegExp(target.name, "i"),
      });

    expect(within(getHomeCard()).getByText("61")).toBeInTheDocument();

    fireEvent.click(getHomeCard());

    expect(await screen.findByText(generatedReport.overview)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "← 返回" }));

    await waitFor(() =>
      expect(within(getHomeCard()).getByText("97")).toBeInTheDocument(),
    );
    expect(within(getHomeCard()).getByText("综合推荐")).toBeInTheDocument();
    expect(within(getHomeCard()).queryByText("61")).not.toBeInTheDocument();
  });

  it("keeps the latest aiScore when older dashboard requests resolve afterwards", async () => {
    const initialDashboard = buildDashboard();
    const target = {
      ...initialDashboard.newGames[0],
      recommendationScore: 61,
      aiScore: 61,
    };
    initialDashboard.newGames[0] = target;

    const staleDashboard = structuredClone(initialDashboard);
    const refreshedDashboard = structuredClone(initialDashboard);
    refreshedDashboard.newGames[0] = {
      ...refreshedDashboard.newGames[0],
      aiScore: 97,
      aiSummary: "新的 AI 评测结果",
    };

    const cachedReport = buildAnalysisReport(target.appid, {
      overview: "详情页缓存分析。",
    });
    const refreshedReport = buildAnalysisReport(target.appid, {
      overallScore: 97,
      overview: "新的 AI 评测结果",
    });
    const staleRequest = createDeferred<typeof staleDashboard>();
    const refreshedRequest = createDeferred<typeof refreshedDashboard>();

    getDashboardMock
      .mockResolvedValueOnce(initialDashboard)
      .mockImplementationOnce(() => staleRequest.promise)
      .mockImplementationOnce(() => refreshedRequest.promise);
    getGameAnalysisMock.mockResolvedValue(cachedReport);
    generateGameAnalysisMock.mockResolvedValue(refreshedReport);

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });

    const newGamesSection = screen
      .getByRole("heading", { name: "新游区" })
      .closest(".game-section");
    if (!(newGamesSection instanceof HTMLElement)) {
      throw new Error("Missing 新游区 section");
    }

    fireEvent.click(
      within(newGamesSection).getByRole("button", {
        name: new RegExp(target.name, "i"),
      }),
    );

    expect(await screen.findByText(cachedReport.overview)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "刷新分析" }));
    await waitFor(() =>
      expect(generateGameAnalysisMock).toHaveBeenCalledWith(target.appid, true),
    );

    fireEvent.click(screen.getByRole("button", { name: "← 返回" }));

    const getUpdatedCard = () =>
      within(
        screen.getByRole("heading", { name: "新游区" }).closest(".game-section") as HTMLElement,
      ).getByRole("button", {
        name: new RegExp(target.name, "i"),
      });

    let updatedCard = getUpdatedCard();
    expect(within(updatedCard).getByText("97")).toBeInTheDocument();
    expect(within(updatedCard).getByText("综合推荐")).toBeInTheDocument();

    await act(async () => {
      staleRequest.resolve(staleDashboard);
      await Promise.resolve();
      await Promise.resolve();
    });

    updatedCard = getUpdatedCard();
    expect(within(updatedCard).getByText("97")).toBeInTheDocument();
    expect(within(updatedCard).queryByText("61")).not.toBeInTheDocument();

    await act(async () => {
      refreshedRequest.resolve(refreshedDashboard);
      await Promise.resolve();
      await Promise.resolve();
    });

    updatedCard = getUpdatedCard();
    expect(within(updatedCard).getByText("97")).toBeInTheDocument();
  });

  it("updates the home card even when leaving detail before analysis refresh finishes", async () => {
    const initialDashboard = buildDashboard();
    const target = {
      ...initialDashboard.newGames[0],
      recommendationScore: 61,
      aiScore: 61,
    };
    initialDashboard.newGames[0] = target;

    const refreshedDashboard = structuredClone(initialDashboard);
    refreshedDashboard.newGames[0] = {
      ...refreshedDashboard.newGames[0],
      aiScore: 97,
      aiSummary: "离开详情后也该同步到首页",
    };

    const cachedReport = buildAnalysisReport(target.appid, {
      overview: "详情页缓存分析。",
    });
    const refreshedReport = buildAnalysisReport(target.appid, {
      overallScore: 97,
      overview: "离开详情后也该同步到首页",
    });
    const staleDashboardRequest = createDeferred<typeof initialDashboard>();
    const refreshAnalysisRequest = createDeferred<GameAnalysisReport>();

    getDashboardMock
      .mockResolvedValueOnce(initialDashboard)
      .mockImplementationOnce(() => staleDashboardRequest.promise)
      .mockResolvedValueOnce(refreshedDashboard);
    getGameAnalysisMock.mockResolvedValue(cachedReport);
    generateGameAnalysisMock.mockImplementationOnce(() => refreshAnalysisRequest.promise);

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });

    const getNewGamesSection = () =>
      screen.getByRole("heading", { name: "新游区" }).closest(".game-section") as HTMLElement;
    const getHomeCard = () =>
      within(getNewGamesSection()).getByRole("button", {
        name: new RegExp(target.name, "i"),
      });

    fireEvent.click(getHomeCard());

    expect(await screen.findByText(cachedReport.overview)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "刷新分析" }));
    fireEvent.click(screen.getByRole("button", { name: "← 返回" }));

    await act(async () => {
      refreshAnalysisRequest.resolve(refreshedReport);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(within(getHomeCard()).getByText("97")).toBeInTheDocument();
    expect(within(getHomeCard()).getByText("综合推荐")).toBeInTheDocument();

    await act(async () => {
      staleDashboardRequest.resolve(initialDashboard);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(within(getHomeCard()).getByText("97")).toBeInTheDocument();
    expect(within(getHomeCard()).queryByText("61")).not.toBeInTheDocument();
  });

  it("keeps the AI assistant card score synced after its follow-up dashboard reload", async () => {
    const initialDashboard = buildDashboard();
    const target = {
      ...initialDashboard.newGames[0],
      recommendationScore: 61,
      aiScore: null,
      aiSummary: "评估前仍显示基础推荐值。",
    };
    initialDashboard.newGames[0] = target;

    const staleDashboard = structuredClone(initialDashboard);
    assessGameWithAiMock.mockResolvedValueOnce({
      appid: target.appid,
      score: 97,
      summary: "AI 助手刚生成的新推荐值",
      bestFor: [],
      risks: [],
    });
    getDashboardMock
      .mockResolvedValueOnce(initialDashboard)
      .mockResolvedValueOnce(staleDashboard);

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });
    fireEvent.click(screen.getByRole("button", { name: "✦ 让 AI 帮我找游戏" }));

    const getAiCard = () =>
      screen.getByRole("heading", { name: target.name }).closest(".recommend-row") as HTMLElement;

    expect(within(getAiCard()).getByText("61")).toBeInTheDocument();
    expect(within(getAiCard()).getByText("推荐值")).toBeInTheDocument();

    fireEvent.click(within(getAiCard()).getByRole("button", { name: "评估" }));

    await waitFor(() => expect(assessGameWithAiMock).toHaveBeenCalledWith(target.appid));
    await waitFor(() => expect(getDashboardMock).toHaveBeenCalledTimes(2));

    expect(within(getAiCard()).getByText("97")).toBeInTheDocument();
    expect(within(getAiCard()).getByText("综合推荐")).toBeInTheDocument();
    expect(within(getAiCard()).queryByText("61")).not.toBeInTheDocument();
  });

  it("lets the AI assistant recommend from a user prompt with reasons and gaps", async () => {
    const dashboard = buildDashboard();
    const target = dashboard.newGames[0];
    recommendGamesWithAiMock.mockResolvedValueOnce({
      reply: "没有完全匹配，我先按本地合作和轻松氛围给你近似推荐。",
      followUpQuestion: "你愿意把人数放宽到 4 人吗？",
      exactMatchCount: 0,
      source: "rule",
      llmUsed: false,
      diagnostic: "未配置 LLM Key，使用本地规则匹配。",
      items: [
        {
          game: target,
          matchScore: 82,
          reason: "支持本地合作，适合轻松开局。",
          matchedTraits: ["本地合作", "轻松休闲"],
          missingTraits: ["6 人以上"],
          caveats: ["最多 4 人"],
          exactMatch: false,
        },
      ],
    });
    getDashboardMock.mockResolvedValue(dashboard);

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });
    fireEvent.click(screen.getByRole("button", { name: "✦ 让 AI 帮我找游戏" }));

    fireEvent.change(
      screen.getByPlaceholderText("描述你想要的游戏，例如：本地合作、轻松、不要恐怖"),
      {
        target: { value: "想找 6 人以上 本地合作 轻松一点" },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "发送需求" }));

    await waitFor(() =>
      expect(recommendGamesWithAiMock).toHaveBeenCalledWith({
        prompt: "想找 6 人以上 本地合作 轻松一点",
        contextMessages: expect.any(Array),
        limit: 5,
      }),
    );
    expect(await screen.findByText("支持本地合作，适合轻松开局。")).toBeInTheDocument();
    expect(screen.getByText("82%")).toBeInTheDocument();
    expect(screen.getByText("近似匹配")).toBeInTheDocument();
    expect(screen.getByText("规则匹配")).toBeInTheDocument();
    expect(screen.getByText("未配置 LLM Key，使用本地规则匹配。")).toBeInTheDocument();
    expect(screen.getByText("缺口：6 人以上")).toBeInTheDocument();
    expect(screen.getByText("你愿意把人数放宽到 4 人吗？")).toBeInTheDocument();
    expect(
      screen.getByLabelText("AI 推荐对话").textContent?.match(/你愿意把人数放宽到 4 人吗？/g) ?? [],
    ).toHaveLength(0);
  });

  it("shows an animated thinking indicator while the AI recommendation is pending", async () => {
    const dashboard = buildDashboard();
    const target = dashboard.newGames[0];
    const pendingRecommendation = createDeferred<AiRecommendationResponse>();
    recommendGamesWithAiMock.mockImplementationOnce(() => pendingRecommendation.promise);
    getDashboardMock.mockResolvedValue(dashboard);

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });
    fireEvent.click(screen.getByRole("button", { name: "✦ 让 AI 帮我找游戏" }));

    fireEvent.change(
      screen.getByPlaceholderText("描述你想要的游戏，例如：本地合作、轻松、不要恐怖"),
      {
        target: { value: "想找一个本地合作派对游戏" },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "发送需求" }));

    expect(await screen.findByText("AI 正在思考")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "发送需求" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "新对话" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "对话历史" })).toBeDisabled();

    await act(async () => {
      pendingRecommendation.resolve({
        reply: "我按本地合作和派对氛围找到了候选。",
        followUpQuestion: null,
        exactMatchCount: 1,
        source: "hybrid",
        llmUsed: true,
        diagnostic: "LLM 已基于库内候选润色推荐理由。",
        items: [
          {
            game: target,
            matchScore: 93,
            reason: "适合本地合作派对，节奏轻松。",
            matchedTraits: ["本地合作", "派对"],
            missingTraits: [],
            caveats: [],
            exactMatch: true,
          },
        ],
      });
      await Promise.resolve();
      await Promise.resolve();
    });

    await waitFor(() =>
      expect(screen.queryByText("AI 正在思考")).not.toBeInTheDocument(),
    );
    expect(screen.getByText("适合本地合作派对，节奏轻松。")).toBeInTheDocument();
  });

  it("starts a new AI conversation and restores the previous one from in-memory history", async () => {
    const dashboard = buildDashboard();
    const firstTarget = dashboard.newGames[0];
    const secondTarget = dashboard.classics[0];
    recommendGamesWithAiMock
      .mockResolvedValueOnce({
        reply: "第一轮已找到候选。",
        followUpQuestion: null,
        exactMatchCount: 1,
        source: "hybrid",
        llmUsed: true,
        diagnostic: "LLM 已基于库内候选润色推荐理由。",
        items: [
          {
            game: firstTarget,
            matchScore: 91,
            reason: "第一轮理由：本地合作且轻松。",
            matchedTraits: ["本地合作", "轻松休闲"],
            missingTraits: [],
            caveats: [],
            exactMatch: true,
          },
        ],
      })
      .mockResolvedValueOnce({
        reply: "第二轮已找到候选。",
        followUpQuestion: null,
        exactMatchCount: 1,
        source: "rule",
        llmUsed: false,
        diagnostic: "测试默认使用规则匹配。",
        items: [
          {
            game: secondTarget,
            matchScore: 84,
            reason: "第二轮理由：适合生存开黑。",
            matchedTraits: ["生存玩法"],
            missingTraits: [],
            caveats: [],
            exactMatch: true,
          },
        ],
      });
    getDashboardMock.mockResolvedValue(dashboard);

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });
    fireEvent.click(screen.getByRole("button", { name: "✦ 让 AI 帮我找游戏" }));

    const promptInput = screen.getByPlaceholderText(
      "描述你想要的游戏，例如：本地合作、轻松、不要恐怖",
    );
    fireEvent.change(promptInput, {
      target: { value: "想找本地合作轻松一点" },
    });
    fireEvent.click(screen.getByRole("button", { name: "发送需求" }));

    expect(await screen.findByText("第一轮理由：本地合作且轻松。")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "新对话" }));

    expect(screen.getByText("告诉我人数、联机方式、氛围、想排除的类型，我会只从已入库且已发售的游戏里找。")).toBeInTheDocument();
    expect(screen.queryByText("想找本地合作轻松一点")).not.toBeInTheDocument();
    expect(screen.queryByText("第一轮理由：本地合作且轻松。")).not.toBeInTheDocument();

    fireEvent.change(promptInput, {
      target: { value: "这次想找生存开黑" },
    });
    fireEvent.click(screen.getByRole("button", { name: "发送需求" }));

    expect(await screen.findByText("第二轮理由：适合生存开黑。")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "对话历史" }));
    fireEvent.click(
      screen.getByRole("button", { name: /继续对话 想找本地合作轻松一点/ }),
    );

    expect(screen.getByText("想找本地合作轻松一点")).toBeInTheDocument();
    expect(screen.getByText("第一轮理由：本地合作且轻松。")).toBeInTheDocument();
    expect(screen.queryByText("这次想找生存开黑")).not.toBeInTheDocument();
    expect(screen.queryByText("第二轮理由：适合生存开黑。")).not.toBeInTheDocument();
  });

  it("opens a recommended game detail and returns to the preserved AI answer", async () => {
    const dashboard = buildDashboard();
    const target = dashboard.newGames[0];
    recommendGamesWithAiMock.mockResolvedValueOnce({
      reply: "我按本地合作和轻松氛围给你找到了一个候选。",
      followUpQuestion: null,
      exactMatchCount: 1,
      source: "hybrid",
      llmUsed: true,
      diagnostic: "LLM 已基于库内候选润色推荐理由。",
      items: [
        {
          game: target,
          matchScore: 91,
          reason: "支持本地合作，口碑也比较稳。",
          matchedTraits: ["本地合作", "轻松休闲"],
          missingTraits: [],
          caveats: ["仍建议看近期评测"],
          exactMatch: true,
        },
      ],
    });
    getDashboardMock.mockResolvedValue(dashboard);
    getGameAnalysisMock.mockImplementation(async (appid: number) =>
      buildAnalysisReport(appid),
    );
    generateGameAnalysisMock.mockImplementation(async (appid: number) =>
      buildAnalysisReport(appid),
    );

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });
    fireEvent.click(screen.getByRole("button", { name: "✦ 让 AI 帮我找游戏" }));
    fireEvent.change(
      screen.getByPlaceholderText("描述你想要的游戏，例如：本地合作、轻松、不要恐怖"),
      {
        target: { value: "想找本地合作轻松一点" },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "发送需求" }));

    expect(await screen.findByText("支持本地合作，口碑也比较稳。")).toBeInTheDocument();
    const aiCard = screen
      .getByRole("heading", { name: target.name })
      .closest(".recommend-row") as HTMLElement;
    fireEvent.click(within(aiCard).getByRole("button", { name: "详情" }));

    expect(await screen.findByText("打开详情页后应直接显示缓存分析。")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "← 返回" }));

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "AI 智能推荐助手 Beta" })).toBeInTheDocument();
      expect(screen.getByText("想找本地合作轻松一点")).toBeInTheDocument();
      expect(screen.getByText("支持本地合作，口碑也比较稳。")).toBeInTheDocument();
      expect(screen.getByText("LLM 已增强")).toBeInTheDocument();
    });
  });

  it("keeps a hidden recommended game selected after opening detail triggers a dashboard reload", async () => {
    const { dashboard, hiddenGame } = buildDashboardWithClassicHidden();
    recommendGamesWithAiMock.mockResolvedValueOnce({
      reply: "我在隐藏候选里找到了一个更接近的游戏。",
      followUpQuestion: null,
      exactMatchCount: 1,
      source: "hybrid",
      llmUsed: true,
      diagnostic: "LLM 已基于库内候选润色推荐理由。",
      items: [
        {
          game: hiddenGame,
          matchScore: 94,
          reason: "隐藏候选也应该能正确打开自己的详情。",
          matchedTraits: ["本地合作"],
          missingTraits: [],
          caveats: [],
          exactMatch: true,
        },
      ],
    });
    getDashboardMock.mockResolvedValue(dashboard);
    getGameAnalysisMock.mockImplementation(async (appid: number) =>
      buildAnalysisReport(appid),
    );
    generateGameAnalysisMock.mockImplementation(async (appid: number) =>
      buildAnalysisReport(appid),
    );

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });
    fireEvent.click(screen.getByRole("button", { name: "✦ 让 AI 帮我找游戏" }));
    fireEvent.change(
      screen.getByPlaceholderText("描述你想要的游戏，例如：本地合作、轻松、不要恐怖"),
      {
        target: { value: "想找一个隐藏候选" },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "发送需求" }));

    expect(await screen.findByText("隐藏候选也应该能正确打开自己的详情。")).toBeInTheDocument();

    const aiCard = screen
      .getByRole("heading", { name: hiddenGame.name })
      .closest(".recommend-row") as HTMLElement;
    fireEvent.click(within(aiCard).getByRole("button", { name: "详情" }));

    await waitFor(() =>
      expect(
        screen.getByRole("heading", { level: 1, name: hiddenGame.name }),
      ).toBeInTheDocument(),
    );
    expect(
      screen.queryByRole("heading", { level: 1, name: dashboard.newGames[0].name }),
    ).not.toBeInTheDocument();
  });

  it("returns to the previously browsed page instead of resetting pagination", async () => {
    const dashboard = buildPagedDashboard();

    getDashboardMock.mockResolvedValue(dashboard);
    getGameAnalysisMock.mockImplementation(async (appid: number) =>
      buildAnalysisReport(appid),
    );
    generateGameAnalysisMock.mockImplementation(async (appid: number) =>
      buildAnalysisReport(appid),
    );

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });

    fireEvent.click(screen.getAllByRole("button", { name: "查看全部 〉" })[0]);

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "新游区" })).toBeInTheDocument();
      expect(screen.queryByRole("heading", { name: "精品老游区" })).not.toBeInTheDocument();
      expect(screen.queryByRole("heading", { name: "最近发现" })).not.toBeInTheDocument();
      expect(screen.getByText("第 1 / 2 页")).toBeInTheDocument();
    });

    fireEvent.click(screen.getByRole("button", { name: "下一页" }));

    await waitFor(() => {
      expect(screen.getByText("第 2 / 2 页")).toBeInTheDocument();
      expect(screen.queryByText("分页新游 1")).not.toBeInTheDocument();
    });

    const newGamesSection = screen
      .getByRole("heading", { name: "新游区" })
      .closest(".game-section");
    if (!(newGamesSection instanceof HTMLElement)) {
      throw new Error("Missing 新游区 section");
    }

    const targetName = within(newGamesSection).getAllByRole("heading", { level: 3 })[0]
      ?.textContent;
    if (!targetName) {
      throw new Error("Missing page-2 target game");
    }

    fireEvent.click(
      within(newGamesSection).getByRole("button", {
        name: new RegExp(targetName, "i"),
      }),
    );

    expect(await screen.findByText("打开详情页后应直接显示缓存分析。")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "← 返回" }));

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "新游区" })).toBeInTheDocument();
      expect(screen.queryByRole("heading", { name: "精品老游区" })).not.toBeInTheDocument();
      expect(screen.queryByRole("heading", { name: "最近发现" })).not.toBeInTheDocument();
      expect(screen.getByText("第 2 / 2 页")).toBeInTheDocument();
      expect(screen.getByText(targetName)).toBeInTheDocument();
      expect(screen.queryByText("分页新游 1")).not.toBeInTheDocument();
    });
  });

  it("restores the previous scroll position when returning from detail", async () => {
    const dashboard = buildPagedDashboard();

    getDashboardMock.mockResolvedValue(dashboard);
    getGameAnalysisMock.mockImplementation(async (appid: number) =>
      buildAnalysisReport(appid),
    );
    generateGameAnalysisMock.mockImplementation(async (appid: number) =>
      buildAnalysisReport(appid),
    );

    const scrollToMock = vi.fn();
    Object.defineProperty(window, "scrollTo", {
      configurable: true,
      value: scrollToMock,
      writable: true,
    });
    Object.defineProperty(window, "scrollY", {
      configurable: true,
      value: 640,
      writable: true,
    });

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });

    fireEvent.click(screen.getAllByRole("button", { name: "查看全部 〉" })[0]);

    await waitFor(() => {
      expect(screen.getByText("第 1 / 2 页")).toBeInTheDocument();
    });

    fireEvent.click(screen.getByRole("button", { name: "下一页" }));

    await waitFor(() => {
      expect(screen.getByText("第 2 / 2 页")).toBeInTheDocument();
    });

    const newGamesSection = screen
      .getByRole("heading", { name: "新游区" })
      .closest(".game-section");
    if (!(newGamesSection instanceof HTMLElement)) {
      throw new Error("Missing 新游区 section");
    }

    const targetName = within(newGamesSection).getAllByRole("heading", { level: 3 })[0]
      ?.textContent;
    if (!targetName) {
      throw new Error("Missing page-2 target game");
    }

    fireEvent.click(
      within(newGamesSection).getByRole("button", {
        name: new RegExp(targetName, "i"),
      }),
    );

    expect(await screen.findByText("打开详情页后应直接显示缓存分析。")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "← 返回" }));

    await waitFor(() => {
      expect(screen.getByText("第 2 / 2 页")).toBeInTheDocument();
      expect(scrollToMock).toHaveBeenCalledWith({ top: 640, behavior: "auto" });
    });
  });

  it("shows the scroll dock while scrolling and switches action by scroll direction", async () => {
    const root = document.documentElement;
    const body = document.body;
    Object.defineProperty(window, "innerHeight", {
      configurable: true,
      value: 900,
      writable: true,
    });
    Object.defineProperty(root, "scrollHeight", {
      configurable: true,
      value: 3000,
    });
    Object.defineProperty(body, "scrollHeight", {
      configurable: true,
      value: 3000,
    });
    Object.defineProperty(root, "clientHeight", {
      configurable: true,
      value: 900,
    });

    let scrollYValue = 0;
    Object.defineProperty(window, "scrollY", {
      configurable: true,
      get: () => scrollYValue,
      set: (value: number) => {
        scrollYValue = value;
      },
    });

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });
    const pageSurface = document.querySelector(".page-surface");
    if (!(pageSurface instanceof HTMLElement)) {
      throw new Error("Missing page-surface container");
    }
    Object.defineProperty(pageSurface, "scrollHeight", {
      configurable: true,
      value: 3000,
    });
    Object.defineProperty(pageSurface, "clientHeight", {
      configurable: true,
      value: 900,
    });
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    await act(async () => {
      scrollYValue = 450;
      pageSurface.scrollTop = 450;
      pageSurface.dispatchEvent(new Event("scroll"));
      window.dispatchEvent(new Event("scroll"));
      await Promise.resolve();
    });

    const dock = screen.getByRole("button", { name: /置底/i });
    expect(dock).toHaveAttribute("tabindex", "0");
    expect(dock.closest(".scroll-dock")).toHaveAttribute("aria-hidden", "false");
    expect(screen.getByText("21%")).toBeInTheDocument();

    await act(async () => {
      scrollYValue = 180;
      pageSurface.scrollTop = 180;
      pageSurface.dispatchEvent(new Event("scroll"));
      window.dispatchEvent(new Event("scroll"));
      await Promise.resolve();
    });

    expect(screen.getByRole("button", { name: /置顶/i })).toHaveAttribute("tabindex", "0");
  });

  it("removes the scroll dock button from the tab order after it hides", async () => {
    const root = document.documentElement;
    const body = document.body;
    Object.defineProperty(window, "innerHeight", {
      configurable: true,
      value: 900,
      writable: true,
    });
    Object.defineProperty(root, "scrollHeight", {
      configurable: true,
      value: 3000,
    });
    Object.defineProperty(body, "scrollHeight", {
      configurable: true,
      value: 3000,
    });
    Object.defineProperty(root, "clientHeight", {
      configurable: true,
      value: 900,
    });

    let scrollYValue = 0;
    Object.defineProperty(window, "scrollY", {
      configurable: true,
      get: () => scrollYValue,
      set: (value: number) => {
        scrollYValue = value;
      },
    });

    render(<App />);

    await screen.findByRole("heading", { name: "新游区" });
    const pageSurface = document.querySelector(".page-surface");
    if (!(pageSurface instanceof HTMLElement)) {
      throw new Error("Missing page-surface container");
    }
    Object.defineProperty(pageSurface, "scrollHeight", {
      configurable: true,
      value: 3000,
    });
    Object.defineProperty(pageSurface, "clientHeight", {
      configurable: true,
      value: 900,
    });
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    vi.useFakeTimers();

    const hiddenButton = screen.getByRole("button", { name: /置底/i, hidden: true });
    expect(hiddenButton).toHaveAttribute("tabindex", "-1");
    expect(hiddenButton.closest(".scroll-dock")).toHaveAttribute("aria-hidden", "true");

    await act(async () => {
      scrollYValue = 450;
      pageSurface.scrollTop = 450;
      pageSurface.dispatchEvent(new Event("scroll"));
      window.dispatchEvent(new Event("scroll"));
      await Promise.resolve();
    });

    const visibleButton = screen.getByRole("button", { name: /置底/i });
    expect(visibleButton).toHaveAttribute("tabindex", "0");
    expect(visibleButton.closest(".scroll-dock")).toHaveAttribute("aria-hidden", "false");

    act(() => {
      vi.advanceTimersByTime(2_500);
    });

    expect(screen.queryByRole("button", { name: /置底/i })).not.toBeInTheDocument();

    const hiddenAgainButton = screen.getByRole("button", { name: /置底/i, hidden: true });
    expect(hiddenAgainButton).toHaveAttribute("tabindex", "-1");
    expect(hiddenAgainButton.closest(".scroll-dock")).toHaveAttribute("aria-hidden", "true");
  });
});
