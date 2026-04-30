// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import {
  buildDashboardSections,
  type DashboardSection,
} from "../../features/library/gameFilters";
import type { GameCard, SyncMode } from "../../types";
import type { LibraryFilters, ViewId } from "../types";
import { DashboardPage } from "./DashboardPage";

type ToggleQuickTag = (tag: string) => void;

const filters: LibraryFilters = {
  demoFilter: "all",
  hideAdultContent: true,
  minPlayers: 2,
  minReviewPct: 60,
  releaseWindow: "all",
  selectedTags: [],
  selectedLanguage: "all",
};

function createGames(count: number, prefix: string): GameCard[] {
  return Array.from({ length: count }, (_, index) => {
    const template = mockDashboard.newGames[index % mockDashboard.newGames.length];

    return {
      ...template,
      appid: template.appid + 50_000 + index,
      name: `${prefix} ${index + 1}`,
      userState: { ...template.userState },
    };
  });
}

function renderDashboardPage({
  activeView = "browse",
  currentFilters = filters,
  onToggleQuickTag = vi.fn<ToggleQuickTag>(),
  onSync = vi.fn(),
  sectionsOverride,
  statsOverride,
}: {
  activeView?: ViewId;
  currentFilters?: LibraryFilters;
  onToggleQuickTag?: ToggleQuickTag;
  onSync?: (mode: SyncMode) => void;
  sectionsOverride?: DashboardSection[];
  statsOverride?: typeof mockDashboard.stats;
} = {}) {
  const sections =
    sectionsOverride ??
    buildDashboardSections({
      activeView,
      dashboard: mockDashboard,
      filters: currentFilters,
      query: "",
      sortMode: "recommended",
    });

  render(
    <DashboardPage
      activeView={activeView}
      filters={currentFilters}
      isBusy={false}
      onAi={vi.fn()}
      onChangeView={vi.fn()}
      onOpenFilters={vi.fn()}
      onOpenGame={vi.fn()}
      onResetFilters={vi.fn()}
      onSetDemoFilter={vi.fn()}
      onSetMinPlayers={vi.fn()}
      onSetMinReviewPct={vi.fn()}
      onSetReleaseWindow={vi.fn()}
      onSetSortMode={vi.fn()}
      onSync={onSync}
      onToggleHideAdultContent={vi.fn()}
      onToggleQuickTag={onToggleQuickTag}
      quickTags={["解谜", "合作"]}
      sections={sections}
      selectedAppid={undefined}
      sortMode="recommended"
      stats={statsOverride ?? mockDashboard.stats}
      status="ok"
    />,
  );

  return { sections };
}

describe("DashboardPage", () => {
  afterEach(() => {
    cleanup();
  });

  it("renders all three dashboard sections in browse mode", () => {
    renderDashboardPage({ activeView: "browse" });

    expect(screen.getByRole("heading", { name: "新游区" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "精品老游区" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "最近发现" })).toBeInTheDocument();
  });

  it("routes quick-tag clicks through the page callback", () => {
    const onToggleQuickTag = vi.fn<ToggleQuickTag>();
    renderDashboardPage({ activeView: "home", onToggleQuickTag });

    const quickTagPanel = screen
      .getAllByRole("button", { name: "更多标签 〉" })[0]
      .closest(".tag-panel");

    if (!(quickTagPanel instanceof HTMLElement)) {
      throw new Error("Missing quick-tag panel");
    }

    fireEvent.click(within(quickTagPanel).getByRole("button", { name: "解谜" }));

    expect(onToggleQuickTag).toHaveBeenCalledTimes(1);
    expect(onToggleQuickTag).toHaveBeenCalledWith("解谜");
  });

  it("paginates non-home sections instead of truncating them", () => {
    renderDashboardPage({
      activeView: "new",
      sectionsOverride: [
        {
          id: "new",
          title: "新游区",
          subtitle: "近一个月发布的多人游戏",
          games: createGames(13, "测试新游"),
        },
      ],
    });

    expect(screen.getByText("共 13 款")).toBeInTheDocument();
    expect(screen.getByText("第 1 / 2 页")).toBeInTheDocument();
    expect(screen.getByText("测试新游 12")).toBeInTheDocument();
    expect(screen.queryByText("测试新游 13")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "下一页" }));

    expect(screen.getByText("第 2 / 2 页")).toBeInTheDocument();
    expect(screen.getByText("测试新游 13")).toBeInTheDocument();
    expect(screen.queryByText("测试新游 1")).not.toBeInTheDocument();
  });

  it("shows live backfill progress in the right rail", () => {
    renderDashboardPage({
      activeView: "home",
      statsOverride: {
        ...mockDashboard.stats,
        syncRunning: true,
        syncMode: "full",
        syncCurrentAppid: 440123,
        syncTotalCount: 6,
        syncProcessedCount: 3,
        syncUpdatedCount: 3,
        syncFailedCount: 0,
        backfillRunning: true,
        backfillPendingCount: 3,
        backfillCurrentAppid: 730123,
        backfillCurrentAttempt: 1,
        backfillTotalCount: 5,
        backfillProcessedCount: 2,
        backfillFailedCount: 0,
      },
    });

    expect(screen.getByText("Steam 同步")).toBeInTheDocument();
    expect(screen.getByText("完整同步中")).toBeInTheDocument();
    expect(screen.getByText("3/6")).toBeInTheDocument();
    expect(screen.getByText("440123")).toBeInTheDocument();
    expect(screen.getByText("元数据补录")).toBeInTheDocument();
    expect(screen.getByText("补录中")).toBeInTheDocument();
    expect(screen.getByText("2/5")).toBeInTheDocument();
    expect(screen.getByText("730123")).toBeInTheDocument();
  });

  it("offers both full and quick sync actions in the right rail", () => {
    const onSync = vi.fn();

    renderDashboardPage({
      activeView: "home",
      onSync,
    });

    fireEvent.click(screen.getByRole("button", { name: "完整同步" }));
    fireEvent.click(screen.getByRole("button", { name: "快速同步" }));

    expect(onSync).toHaveBeenNthCalledWith(1, "full");
    expect(onSync).toHaveBeenNthCalledWith(2, "quick");
  });
});
