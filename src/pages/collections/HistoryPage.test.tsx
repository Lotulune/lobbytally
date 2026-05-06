// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { GameCard } from "../../types";
import { HistoryPage } from "./HistoryPage";

function buildGame(appid: number, name: string, updatedAt: string | null): GameCard {
  return {
    appid,
    name,
    section: "classic",
    releaseDate: "2025-01-01",
    releaseDateText: "2025.01",
    releaseState: "released",
    demoStatus: "released",
    supportedLanguages: ["English"],
    isAdultContent: false,
    isFree: false,
    priceText: "",
    discountPercent: null,
    positiveReviewPct: 93,
    totalReviews: 3000,
    currentPlayers: 850,
    recommendationScore: 88,
    aiScore: 88,
    aiSummary: `${name} summary`,
    capsuleUrl: `https://example.com/${appid}.jpg`,
    tags: ["合作"],
    multiplayerModes: ["LAN Co-op"],
    reviewSnippets: [],
    userState: {
      favorite: false,
      wishlist: false,
      followed: false,
      viewed: true,
      updatedAt,
    },
  };
}

afterEach(() => {
  cleanup();
});

describe("HistoryPage", () => {
  it("sorts games by userState.updatedAt descending", () => {
    render(
      <HistoryPage
        games={[
          buildGame(201, "Older Visit", "2026-04-18T08:00:00.000Z"),
          buildGame(202, "Newest Visit", "2026-04-24T19:30:00.000Z"),
          buildGame(203, "Missing Timestamp", null),
        ]}
        onOpen={vi.fn()}
      />,
    );

    const titles = screen.getAllByRole("heading", { level: 3 }).map((node) => node.textContent);
    expect(titles).toEqual(["Newest Visit", "Older Visit", "Missing Timestamp"]);
  });

  it("shows a dedicated empty state when there is no history yet", () => {
    render(<HistoryPage games={[]} onOpen={vi.fn()} />);

    expect(screen.getByRole("heading", { name: "最近还没有浏览记录" })).toBeInTheDocument();
    expect(screen.getByText(/打开过详情页的游戏会按最近浏览时间排列/)).toBeInTheDocument();
  });
});
