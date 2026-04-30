// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { mockDashboard } from "./data/mockDashboard";

const getDashboardMock = vi.fn();
const syncSeedGamesMock = vi.fn();

vi.mock("./api/client", async () => {
  const actual = await vi.importActual<typeof import("./api/client")>("./api/client");

  return {
    ...actual,
    assessGameWithAi: vi.fn(),
    getDashboard: () => getDashboardMock(),
    previewSteamAppList: vi.fn(),
    saveConfig: vi.fn(),
    setGameUserState: vi.fn(),
    syncSeedGames: (...args: unknown[]) => syncSeedGamesMock(...args),
  };
});

function buildDashboard() {
  return structuredClone(mockDashboard);
}

function buildLowActivityDiscoveryDashboard() {
  const dashboard = structuredClone(mockDashboard);
  const lowActivityGame = {
    ...dashboard.newGames[0],
    appid: 4999001,
    name: "Quiet Co-op Debut",
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

describe("App dashboard interactions", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getDashboardMock.mockResolvedValue(buildDashboard());
    syncSeedGamesMock.mockResolvedValue({
      updatedGames: 0,
      failedGames: 0,
      message: "已启动 Steam 同步任务。",
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
});
