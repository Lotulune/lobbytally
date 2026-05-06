// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { GameCard } from "../../types";
import { WishlistTrackerPage } from "./WishlistTrackerPage";

function buildGame(appid: number, name: string): GameCard {
  return {
    appid,
    name,
    section: "new",
    releaseDate: "2026-04-01",
    releaseDateText: "2026.04",
    releaseState: "released",
    demoStatus: "demo_only",
    supportedLanguages: ["English"],
    isAdultContent: false,
    isFree: false,
    priceText: "",
    discountPercent: null,
    positiveReviewPct: 89,
    totalReviews: 800,
    currentPlayers: 180,
    recommendationScore: 86,
    aiScore: 86,
    aiSummary: `${name} summary`,
    capsuleUrl: `https://example.com/${appid}.jpg`,
    tags: ["合作"],
    multiplayerModes: ["Online Co-op"],
    reviewSnippets: [],
    userState: {
      favorite: false,
      wishlist: true,
      followed: false,
      viewed: false,
      updatedAt: "2026-04-22T12:00:00.000Z",
    },
  };
}

afterEach(() => {
  cleanup();
});

describe("WishlistTrackerPage", () => {
  it("renders wishlist games from local user-state fixtures", () => {
    render(
      <WishlistTrackerPage
        games={[buildGame(101, "Orbit Outing"), buildGame(102, "River Raiders")]}
        onOpen={vi.fn()}
        onToggle={vi.fn()}
      />,
    );

    expect(screen.getByRole("heading", { name: "愿望单追踪" })).toBeInTheDocument();
    expect(screen.getByText("Orbit Outing")).toBeInTheDocument();
    expect(screen.getByText("River Raiders")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "愿望单 · 2 款" })).toBeInTheDocument();
  });

  it("removes a game from wishlist through the shared toggle callback", () => {
    const onToggle = vi.fn();

    render(
      <WishlistTrackerPage
        games={[buildGame(101, "Orbit Outing")]}
        onOpen={vi.fn()}
        onToggle={onToggle}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "移出愿望单《Orbit Outing》" }));

    expect(onToggle).toHaveBeenCalledWith(
      expect.objectContaining({ appid: 101, name: "Orbit Outing" }),
      { wishlist: false },
      "已从愿望单移除《Orbit Outing》。",
    );
  });
});
