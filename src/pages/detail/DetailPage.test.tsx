// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import { DetailPage } from "./DetailPage";

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

afterEach(() => {
  cleanup();
});

describe("DetailPage", () => {
  it("renders the current game's own store gallery thumbnails", () => {
    const game = buildGame();
    const relatedGames = buildRelatedGames();
    const { container } = render(
      <DetailPage
        game={game}
        relatedGames={relatedGames}
        isBusy={false}
        onAiAssess={vi.fn()}
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
        onAiAssess={vi.fn()}
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
        onAiAssess={vi.fn()}
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
        onAiAssess={vi.fn()}
        onBack={vi.fn()}
        onToggleState={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("tab", { name: /玩家评价/i }));

    expect(screen.getByText("✅ 推荐")).toBeInTheDocument();
  });

  it("renders the localized short description when available", () => {
    const game = buildGame();
    render(
      <DetailPage
        game={game}
        relatedGames={buildRelatedGames()}
        isBusy={false}
        onAiAssess={vi.fn()}
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
        onAiAssess={vi.fn()}
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
        onAiAssess={vi.fn()}
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
