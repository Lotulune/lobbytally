// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DashboardStats, DiscoveryRunSnapshot } from "../../types";
import { DiscoveryTaskPanel } from "./DiscoveryTaskPanel";

const useDiscoveryTaskMock = vi.fn();
let hookState: ReturnType<typeof createHookState>;

vi.mock("./useDiscoveryTask", () => ({
  useDiscoveryTask: () => useDiscoveryTaskMock(),
}));

function openDiscoverySubPanel(title: string) {
  fireEvent.click(screen.getByRole("button", { name: new RegExp(title) }));
}

function createSnapshot(
  overrides: Partial<DiscoveryRunSnapshot> = {},
): DiscoveryRunSnapshot {
  return {
    id: 42,
    status: "paused",
    syncMode: "full",
    targetAddedGames: 6,
    pageSize: 20,
    pagesProcessed: 2,
    scannedApps: 40,
    addedGames: 3,
    addedNewGames: 2,
    addedClassicGames: 1,
    skippedExisting: 18,
    skippedNonMultiplayer: 17,
    failedGames: 1,
    currentAppid: 730123,
    lastAppid: 730140,
    haveMoreResults: true,
    startedAt: "2026-04-27T09:00:00.000Z",
    updatedAt: "2026-04-27T09:10:00.000Z",
    finishedAt: null,
    lastError: "temporary fetch failure",
    failures: [
      {
        pageIndex: 2,
        appid: 730123,
        stage: "fetch_snapshot",
        reason: "temporary fetch failure",
        createdAt: "2026-04-27T09:09:00.000Z",
      },
    ],
    progressPercent: 50,
    ...overrides,
  };
}

const stats: DashboardStats = {
  lastSyncAt: "2026-04-27T08:00:00.000Z",
  seedCount: 120,
  totalGames: 88,
  newGamesCount: 32,
  classicGamesCount: 56,
  lastDiscoveryAppid: 730000,
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
  backfillMaxAttempts: 2,
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
  aiBatchRefreshLastError: null,
  aiBatchRefreshLastErrorAppid: null,
  dataSource: "SQLite",
};

function createHookState(
  overrides: Partial<ReturnType<typeof createHookStateBase>> = {},
) {
  return {
    ...createHookStateBase(),
    ...overrides,
  };
}

function createHookStateBase() {
  return {
    snapshot: null as DiscoveryRunSnapshot | null,
    history: [] as DiscoveryRunSnapshot[],
    isLoading: false,
    start: vi.fn(),
    pause: vi.fn(),
    resume: vi.fn(),
    cancel: vi.fn(),
    refresh: vi.fn(),
  };
}

describe("DiscoveryTaskPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    hookState = createHookState();
    useDiscoveryTaskMock.mockImplementation(() => hookState);
  });

  afterEach(() => {
    vi.useRealTimers();
    cleanup();
  });

  it("renders resume controls and failure rows for a paused run", async () => {
    const resume = vi.fn().mockResolvedValue(createSnapshot({ status: "running" }));
    hookState = createHookState({
      snapshot: createSnapshot(),
      history: [createSnapshot({ id: 41, status: "completed", progressPercent: 100 })],
      resume,
    });

    render(
      <DiscoveryTaskPanel
        stats={stats}
        onRefreshDashboard={vi.fn().mockResolvedValue(undefined)}
        onStatus={vi.fn()}
      />,
    );

    expect(screen.getByText("已暂停")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "继续任务" })).toBeInTheDocument();
    openDiscoverySubPanel("失败记录");
    expect(screen.getByText("temporary fetch failure")).toBeInTheDocument();
    expect(screen.getByText("fetch_snapshot")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "继续任务" }));

    await waitFor(() => expect(resume).toHaveBeenCalledTimes(1));
  });

  it("starts a new run with the entered target and page size", async () => {
    const start = vi.fn().mockResolvedValue(createSnapshot({ status: "running" }));
    hookState = createHookState({ start });

    render(
      <DiscoveryTaskPanel
        stats={stats}
        onRefreshDashboard={vi.fn().mockResolvedValue(undefined)}
        onStatus={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByLabelText("目标新增游戏数"), {
      target: { value: "12" },
    });
    fireEvent.change(screen.getByLabelText("每页候选数"), {
      target: { value: "30" },
    });
    fireEvent.click(screen.getByRole("button", { name: "开始新任务" }));

    await waitFor(() =>
      expect(start).toHaveBeenCalledWith({
        syncMode: "full",
        targetAddedGames: 12,
        pageSize: 30,
      }),
    );
  });

  it("clamps page size to the store-search-safe backend maximum", async () => {
    const start = vi.fn().mockResolvedValue(createSnapshot({ status: "running" }));
    hookState = createHookState({ start });

    render(
      <DiscoveryTaskPanel
        stats={stats}
        onRefreshDashboard={vi.fn().mockResolvedValue(undefined)}
        onStatus={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByLabelText("每页候选数"), {
      target: { value: "999" },
    });
    fireEvent.click(screen.getByRole("button", { name: "开始新任务" }));

    await waitFor(() =>
      expect(start).toHaveBeenCalledWith({
        syncMode: "full",
        targetAddedGames: 6,
        pageSize: 100,
      }),
    );
  });

  it("clamps target added games to the larger backend maximum", async () => {
    const start = vi.fn().mockResolvedValue(createSnapshot({ status: "running" }));
    hookState = createHookState({ start });

    render(
      <DiscoveryTaskPanel
        stats={stats}
        onRefreshDashboard={vi.fn().mockResolvedValue(undefined)}
        onStatus={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByLabelText("目标新增游戏数"), {
      target: { value: "999" },
    });
    fireEvent.click(screen.getByRole("button", { name: "开始新任务" }));

    await waitFor(() =>
      expect(start).toHaveBeenCalledWith({
        syncMode: "full",
        targetAddedGames: 200,
        pageSize: 100,
      }),
    );
  });

  it("starts a new run with partial fetch mode when selected", async () => {
    const start = vi.fn().mockResolvedValue(
      createSnapshot({ status: "running", syncMode: "quick" }),
    );
    hookState = createHookState({ start });

    render(
      <DiscoveryTaskPanel
        stats={stats}
        onRefreshDashboard={vi.fn().mockResolvedValue(undefined)}
        onStatus={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "部分拉取" }));
    fireEvent.click(screen.getByRole("button", { name: "开始新任务" }));

    await waitFor(() =>
      expect(start).toHaveBeenCalledWith({
        syncMode: "quick",
        targetAddedGames: 6,
        pageSize: 100,
      }),
    );
  });

  it("keeps start available for interrupted runs so the user can restart from page one", () => {
    hookState = createHookState({
      snapshot: createSnapshot({ status: "interrupted" }),
    });

    render(
      <DiscoveryTaskPanel
        stats={stats}
        onRefreshDashboard={vi.fn().mockResolvedValue(undefined)}
        onStatus={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "继续任务" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "开始新任务" })).toBeEnabled();
    expect(
      screen.getByText("开始新任务会放弃当前可恢复的旧任务，并从最近发售候选的第一页重新扫描。"),
    ).toBeInTheDocument();
  });

  it("refreshes dashboard when a running task later becomes cancelled", async () => {
    const onRefreshDashboard = vi.fn().mockResolvedValue(undefined);
    hookState = createHookState({
      snapshot: createSnapshot({ status: "running", finishedAt: null }),
    });

    const { rerender } = render(
      <DiscoveryTaskPanel
        stats={stats}
        onRefreshDashboard={onRefreshDashboard}
        onStatus={vi.fn()}
      />,
    );

    expect(onRefreshDashboard).not.toHaveBeenCalled();

    hookState = createHookState({
      snapshot: createSnapshot({
        status: "cancelled",
        finishedAt: "2026-04-27T09:11:00.000Z",
      }),
    });
    rerender(
      <DiscoveryTaskPanel
        stats={stats}
        onRefreshDashboard={onRefreshDashboard}
        onStatus={vi.fn()}
      />,
    );

    await waitFor(() => expect(onRefreshDashboard).toHaveBeenCalledTimes(1));
  });

  it("renders backend page index without adding one", () => {
    hookState = createHookState({
      snapshot: createSnapshot({
        failures: [
          {
            pageIndex: 2,
            appid: 730123,
            stage: "fetch_snapshot",
            reason: "temporary fetch failure",
            createdAt: "2026-04-27T09:09:00.000Z",
          },
        ],
      }),
    });

    render(
      <DiscoveryTaskPanel
        stats={stats}
        onRefreshDashboard={vi.fn().mockResolvedValue(undefined)}
        onStatus={vi.fn()}
      />,
    );

    openDiscoverySubPanel("失败记录");
    expect(screen.getByText("第 2 页 · AppID 730123")).toBeInTheDocument();
    expect(screen.queryByText("第 3 页 · AppID 730123")).not.toBeInTheDocument();
  });

  it("renders backfill status and requests refresh while metadata completion is pending", async () => {
    const onRefreshDashboard = vi.fn().mockResolvedValue(undefined);

    render(
      <DiscoveryTaskPanel
        stats={{
          ...stats,
          backfillPendingCount: 3,
          backfillRunning: true,
          backfillCurrentAppid: 730123,
          backfillCurrentAttempt: 1,
          backfillTotalCount: 5,
          backfillProcessedCount: 2,
          backfillFailedCount: 1,
          backfillLastError: "temporary upstream error",
          backfillLastErrorAppid: 570,
        }}
        onRefreshDashboard={onRefreshDashboard}
        onStatus={vi.fn()}
      />,
    );

    openDiscoverySubPanel("元数据补全");
    expect(screen.getByText("元数据补全")).toBeInTheDocument();
    expect(screen.getByText("补录中")).toBeInTheDocument();
    expect(screen.getAllByText("2/5").length).toBeGreaterThan(0);
    expect(screen.getAllByText("3").length).toBeGreaterThan(0);
    expect(screen.getByText("730123")).toBeInTheDocument();
    expect(screen.getByText("1/2")).toBeInTheDocument();
    expect(screen.getByText("AppID 570 · temporary upstream error")).toBeInTheDocument();

    await waitFor(() => expect(onRefreshDashboard).toHaveBeenCalledTimes(1));
  });
});
