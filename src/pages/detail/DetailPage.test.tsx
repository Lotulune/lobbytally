// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { StrictMode, type ComponentProps } from "react";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import type { GameAnalysisReport } from "../../types";
import { DetailPage } from "./DetailPage";

const clientMocks = vi.hoisted(() => ({
  generateGameAnalysis: vi.fn(),
  getGameAnalysis: vi.fn(),
}));

vi.mock("../../api/client", () => ({
  generateGameAnalysis: clientMocks.generateGameAnalysis,
  getGameAnalysis: clientMocks.getGameAnalysis,
}));

function buildGame() {
  const game = structuredClone(mockDashboard.newGames[0]);
  game.shortDescription = "双人到四人合作解谜，强调实时沟通与分工推进。";
  game.storeScreenshotUrls = [
    "https://example.com/current-thumb-1.jpg",
    "https://example.com/current-thumb-2.jpg",
    "https://example.com/current-thumb-3.jpg",
    "https://example.com/current-thumb-4.jpg",
  ];
  game.reviewSnippets = [
    {
      votedUp: true,
      review: "联机沟通压力刚刚好，和朋友开黑时节奏非常顺。",
      playtimeHours: 12,
    },
  ];
  return game;
}

function buildRelatedGames() {
  return structuredClone(mockDashboard.classics.slice(0, 3));
}

function buildReport(
  appid: number,
  overrides: Partial<GameAnalysisReport> = {},
): GameAnalysisReport {
  return {
    appid,
    generatedAt: "2026-04-30T12:00:00.000Z",
    source: "hybrid",
    confidence: "high",
    overallScore: 91,
    overview: "这是一份缓存中的 AI 详细评估。",
    dimensionScores: [
      {
        key: "approachability",
        label: "易上手度",
        score: 88,
        reason: "新手和固定队都能快速进入状态。",
      },
      {
        key: "multiplayer_fun",
        label: "联机乐趣",
        score: 93,
        reason: "强调沟通、分工和反复开黑。",
      },
    ],
    strengths: [
      {
        title: "很适合朋友局",
        reason: "多人机制与合作循环都比较明确。",
      },
    ],
    risks: [
      {
        title: "后期内容待确认",
        reason: "需要继续观察中后期留存反馈。",
      },
    ],
    evidence: [
      {
        kind: "positive_review_pct",
        label: "好评率",
        value: "92%",
        interpretation: "整体口碑表现稳定。",
      },
    ],
    reviewEvidence: [
      {
        stance: "strength",
        quote: "和朋友开黑时节奏非常顺。",
        playtimeText: "12 小时游玩",
        interpretation: "评论证据与多人节奏判断一致。",
      },
    ],
    ...overrides,
  };
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

function renderDetailPage(
  game = buildGame(),
  props: Partial<ComponentProps<typeof DetailPage>> = {},
) {
  return render(
    <DetailPage
      game={game}
      relatedGames={buildRelatedGames()}
      isBusy={false}
      onBack={vi.fn()}
      onAnalysisUpdated={vi.fn()}
      onToggleState={vi.fn()}
      {...props}
    />,
  );
}

afterEach(() => {
  cleanup();
});

beforeEach(() => {
  clientMocks.getGameAnalysis.mockReset();
  clientMocks.generateGameAnalysis.mockReset();
  clientMocks.getGameAnalysis.mockImplementation(async (appid: number) => buildReport(appid));
  clientMocks.generateGameAnalysis.mockImplementation(async (appid: number, forceRefresh?: boolean) =>
    buildReport(appid, {
      generatedAt: forceRefresh ? "2026-04-30T12:30:00.000Z" : "2026-04-30T12:05:00.000Z",
      overview: forceRefresh ? "这是一份强制刷新的 AI 详细评估。" : "这是一份新生成的 AI 详细评估。",
    }),
  );
});

describe("DetailPage", () => {
  it("renders cached report without auto-generating", async () => {
    const game = buildGame();
    const cachedReport = buildReport(game.appid, {
      overview: "缓存命中的 AI 详细评估。",
    });
    clientMocks.getGameAnalysis.mockResolvedValueOnce(cachedReport);

    renderDetailPage(game);

    expect(await screen.findByText(cachedReport.overview)).toBeInTheDocument();
    expect(clientMocks.getGameAnalysis).toHaveBeenCalledWith(game.appid);
    expect(clientMocks.generateGameAnalysis).not.toHaveBeenCalled();

    const toggleButton = screen.getByRole("button", { name: "查看完整报告" });
    expect(toggleButton).toHaveAttribute("aria-expanded", "false");
    expect(toggleButton).toHaveAttribute("aria-controls");

    const detailsId = toggleButton.getAttribute("aria-controls");
    fireEvent.click(toggleButton);

    expect(screen.getByRole("button", { name: "收起完整报告" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(screen.getByRole("region", { name: "完整分析报告" })).toHaveAttribute(
      "id",
      detailsId,
    );
  });

  it("shows separate recommendation and quality scores for a V2 report", async () => {
    const game = buildGame();
    const v2Report = {
      ...buildReport(game.appid, {
        overallScore: 87,
      }),
      scoreVersion: "v2",
      recommendationScore: 87,
      qualityScore: 74,
      confidenceScore: 0.82,
      poolType: "hidden_gem",
      riskFlags: [],
    } as unknown as GameAnalysisReport;
    clientMocks.getGameAnalysis.mockResolvedValueOnce(v2Report);

    renderDetailPage(game);

    expect(await screen.findByText(v2Report.overview)).toBeInTheDocument();
    expect(screen.getByText("综合推荐")).toBeInTheDocument();
    expect(screen.getByText("游戏质量")).toBeInTheDocument();
    expect(screen.getByText("74")).toBeInTheDocument();
  });

  it("sanitizes bbcode-style review evidence before rendering it", async () => {
    const game = buildGame();
    const cachedReport = buildReport(game.appid, {
      reviewEvidence: [
        {
          stance: "strength",
          quote:
            "[url=https://store.steampowered.com/]这游戏在 Steam Deck 上也很好玩[/url]，朋友局节奏很顺。",
          playtimeText: "5.6 小时",
          interpretation: "正向评论通常反映合作体验和上手门槛。",
        },
      ],
    });
    clientMocks.getGameAnalysis.mockResolvedValueOnce(cachedReport);

    renderDetailPage(game);

    fireEvent.click(await screen.findByRole("button", { name: "查看完整报告" }));

    expect(await screen.findByText(/这游戏在 Steam Deck 上也很好玩/)).toBeInTheDocument();
    expect(screen.queryByText(/\[url=/)).not.toBeInTheDocument();
    expect(screen.queryByText(/\[\/url\]/)).not.toBeInTheDocument();
  });

  it("auto-generates a report when cache is missing", async () => {
    const game = buildGame();
    const generatedReport = buildReport(game.appid, {
      overview: "这是一份自动生成的 AI 详细评估。",
    });
    clientMocks.getGameAnalysis.mockResolvedValueOnce(null);
    clientMocks.generateGameAnalysis.mockResolvedValueOnce(generatedReport);

    renderDetailPage(game);

    await waitFor(() =>
      expect(clientMocks.generateGameAnalysis).toHaveBeenCalledWith(game.appid, false),
    );
    expect(await screen.findByText(generatedReport.overview)).toBeInTheDocument();
  });

  it("still loads analysis correctly inside StrictMode", async () => {
    const game = buildGame();
    const cachedReport = buildReport(game.appid, {
      overview: "StrictMode 下依然能展示的缓存分析。",
    });
    clientMocks.getGameAnalysis.mockResolvedValue(cachedReport);

    render(
      <StrictMode>
        <DetailPage
          game={game}
          relatedGames={buildRelatedGames()}
          isBusy={false}
          onBack={vi.fn()}
          onToggleState={vi.fn()}
        />
      </StrictMode>,
    );

    expect(await screen.findByText(cachedReport.overview)).toBeInTheDocument();
    expect(clientMocks.generateGameAnalysis).not.toHaveBeenCalled();
  });

  it("forces generateGameAnalysis(appid, true) when clicking 重新 AI 评估", async () => {
    const game = buildGame();
    const cachedReport = buildReport(game.appid, {
      overview: "缓存版分析。",
    });
    const refreshedReport = buildReport(game.appid, {
      generatedAt: "2026-04-30T12:45:00.000Z",
      overview: "刷新后的 AI 详细评估。",
    });
    clientMocks.getGameAnalysis.mockResolvedValueOnce(cachedReport);
    clientMocks.generateGameAnalysis.mockResolvedValueOnce(refreshedReport);

    renderDetailPage(game);

    expect(await screen.findByText(cachedReport.overview)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "重新 AI 评估" }));

    await waitFor(() =>
      expect(clientMocks.generateGameAnalysis).toHaveBeenCalledWith(game.appid, true),
    );
    expect(await screen.findByText(refreshedReport.overview)).toBeInTheDocument();
  });

  it("notifies the parent to refresh card state after analysis refresh succeeds", async () => {
    const game = buildGame();
    const onAnalysisUpdated = vi.fn();
    clientMocks.getGameAnalysis.mockResolvedValueOnce(buildReport(game.appid));

    renderDetailPage(game, { onAnalysisUpdated });

    expect(await screen.findByText("这是一份缓存中的 AI 详细评估。")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "重新 AI 评估" }));

    await waitFor(() =>
      expect(clientMocks.generateGameAnalysis).toHaveBeenCalledWith(game.appid, true),
    );
    await waitFor(() => expect(onAnalysisUpdated).toHaveBeenCalledTimes(1));
  });

  it("queues a single back action until refresh and parent sync both finish", async () => {
    const game = buildGame();
    const onBack = vi.fn();
    const onAnalysisUpdated = vi.fn();
    const refreshRequest = createDeferred<GameAnalysisReport>();
    const parentSyncRequest = createDeferred<void>();
    const refreshedReport = buildReport(game.appid, {
      generatedAt: "2026-04-30T12:45:00.000Z",
      overview: "刷新完成后的 AI 详细评估。",
    });

    clientMocks.getGameAnalysis.mockResolvedValueOnce(buildReport(game.appid));
    clientMocks.generateGameAnalysis.mockImplementationOnce(() => refreshRequest.promise);
    onAnalysisUpdated.mockImplementationOnce(() => parentSyncRequest.promise);

    renderDetailPage(game, { onAnalysisUpdated, onBack });

    expect(await screen.findByText("这是一份缓存中的 AI 详细评估。")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "刷新分析" }));
    fireEvent.click(screen.getByRole("button", { name: "← 返回" }));

    expect(
      screen.getByRole("button", { name: "AI 评估完成后返回…" }),
    ).toBeInTheDocument();

    refreshRequest.resolve(refreshedReport);

    await waitFor(() => expect(onAnalysisUpdated).toHaveBeenCalledTimes(1));
    expect(onBack).not.toHaveBeenCalled();

    parentSyncRequest.resolve();

    await waitFor(() => expect(onBack).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("button", { name: "← 返回" })).toBeInTheDocument();
  });

  it("renders the current game's own store gallery thumbnails", () => {
    const game = buildGame();
    const relatedGames = buildRelatedGames();
    const { container } = render(
      <DetailPage
        game={game}
        relatedGames={relatedGames}
        isBusy={false}
        onBack={vi.fn()}
        onToggleState={vi.fn()}
      />,
    );

    const thumbSources = Array.from(container.querySelectorAll(".thumb-row img")).map((node) =>
      node.getAttribute("src"),
    );

    expect(thumbSources).toEqual((game.storeScreenshotUrls ?? []).slice(0, 5));
    expect(thumbSources).not.toContain(relatedGames[0]?.capsuleUrl);
    expect(thumbSources).not.toContain(game.capsuleUrl);
  });

  it("shows the first store screenshot by default and switches when another thumbnail is clicked", () => {
    const game = buildGame();
    const { container } = render(
      <DetailPage
        game={game}
        relatedGames={buildRelatedGames()}
        isBusy={false}
        onBack={vi.fn()}
        onToggleState={vi.fn()}
      />,
    );

    const heroImage = container.querySelector(".hero-cover img");
    const thumbnailButtons = screen.getAllByRole("button", { name: /查看《.*》展示图/i });

    expect(heroImage).toHaveAttribute("src", game.storeScreenshotUrls?.[0]);

    fireEvent.click(thumbnailButtons[2]);

    expect(heroImage).toHaveAttribute("src", game.storeScreenshotUrls?.[2]);
    expect(thumbnailButtons[2]).toHaveAttribute("aria-pressed", "true");
  });

  it("switches from AI summary to review snippets", () => {
    const game = buildGame();
    render(
      <DetailPage
        game={game}
        relatedGames={buildRelatedGames()}
        isBusy={false}
        onBack={vi.fn()}
        onToggleState={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("tab", { name: /玩家评价/i }));

    expect(screen.getByText(game.reviewSnippets[0].review)).toBeInTheDocument();
  });

  it("renders an emphasized positive review badge", () => {
    const game = buildGame();
    render(
      <DetailPage
        game={game}
        relatedGames={buildRelatedGames()}
        isBusy={false}
        onBack={vi.fn()}
        onToggleState={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("tab", { name: /玩家评价/i }));

    expect(screen.getByText("✅ 推荐")).toBeInTheDocument();
  });

  it("uses public-service copy for empty review and related panels in read-only mode", () => {
    const game = {
      ...buildGame(),
      reviewSnippets: [],
    };

    renderDetailPage(game, {
      analysisReadOnly: true,
      relatedGames: [],
    });

    fireEvent.click(screen.getByRole("tab", { name: /玩家评价/i }));
    expect(screen.getByText("当前公共数据暂未提供评论摘录。")).toBeInTheDocument();
    expect(screen.queryByText(/本地库/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("tab", { name: /相关游戏/i }));
    expect(screen.getByText("公共发现服务暂未提供可关联的候选项。")).toBeInTheDocument();
    expect(screen.queryByText(/同步更多游戏/)).not.toBeInTheDocument();
  });

  it("renders the localized short description when available", () => {
    const game = buildGame();
    render(
      <DetailPage
        game={game}
        relatedGames={buildRelatedGames()}
        isBusy={false}
        onBack={vi.fn()}
        onToggleState={vi.fn()}
      />,
    );

    expect(screen.getByText(game.shortDescription ?? "")).toBeInTheDocument();
  });

  it("supports keyboard navigation across detail tabs", () => {
    const game = buildGame();
    render(
      <DetailPage
        game={game}
        relatedGames={buildRelatedGames()}
        isBusy={false}
        onBack={vi.fn()}
        onToggleState={vi.fn()}
      />,
    );

    const aiTab = screen.getByRole("tab", { name: "AI 评估" });
    fireEvent.keyDown(aiTab, { key: "ArrowRight" });

    expect(screen.getByRole("tab", { name: /玩家评价/i })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByText(game.reviewSnippets[0].review)).toBeInTheDocument();
  });

  it("emits a wishlist toggle callback", () => {
    const game = buildGame();
    const onToggleState = vi.fn();

    render(
      <DetailPage
        game={game}
        relatedGames={buildRelatedGames()}
        isBusy={false}
        onBack={vi.fn()}
        onToggleState={onToggleState}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /愿望单/i }));

    expect(onToggleState).toHaveBeenCalledWith(
      { wishlist: true },
      expect.stringContaining(game.name),
    );
  });
});
