// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DashboardPayload, GameCard } from "../../types";
import { CollectionsHubPage } from "./CollectionsHubPage";

function buildGame(appid: number, name: string): GameCard {
  return {
    appid,
    name,
    section: "classic",
    releaseDate: "2026-04-01",
    releaseDateText: "2026.04",
    releaseState: "released",
    demoStatus: "released",
    supportedLanguages: ["English"],
    isAdultContent: false,
    isFree: false,
    priceText: "",
    discountPercent: null,
    positiveReviewPct: 95,
    totalReviews: 1200,
    currentPlayers: 420,
    recommendationScore: 91,
    aiScore: 91,
    aiSummary: `${name} summary`,
    capsuleUrl: `https://example.com/${appid}.jpg`,
    tags: ["合作"],
    multiplayerModes: ["Online Co-op"],
    reviewSnippets: [],
    userState: {
      favorite: false,
      wishlist: false,
      followed: false,
      viewed: false,
      updatedAt: null,
    },
  };
}

function buildCollections(): DashboardPayload["collections"] {
  return {
    favorites: [
      {
        ...buildGame(11, "Deep Cave Rescue"),
        userState: {
          favorite: true,
          wishlist: false,
          followed: false,
          viewed: false,
          updatedAt: "2026-04-20T08:00:00.000Z",
        },
      },
    ],
    wishlist: [
      {
        ...buildGame(22, "Moonbase Picnic"),
        userState: {
          favorite: false,
          wishlist: true,
          followed: false,
          viewed: false,
          updatedAt: "2026-04-21T09:30:00.000Z",
        },
      },
    ],
    followed: [],
    history: [],
  };
}

afterEach(() => {
  cleanup();
});

describe("CollectionsHubPage", () => {
  it("switches to wishlist-specific content when changing tabs", () => {
    render(
      <CollectionsHubPage
        collections={buildCollections()}
        onOpen={vi.fn()}
        onToggle={vi.fn()}
      />,
    );

    expect(screen.getByText("Deep Cave Rescue")).toBeInTheDocument();
    expect(screen.queryByText("Moonbase Picnic")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "愿望单" }));

    expect(screen.getByText("Moonbase Picnic")).toBeInTheDocument();
    expect(screen.queryByText("Deep Cave Rescue")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "愿望单 · 1 款" })).toBeInTheDocument();
  });

  it("renders the tab-specific empty copy for history", () => {
    render(
      <CollectionsHubPage
        collections={buildCollections()}
        onOpen={vi.fn()}
        onToggle={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "浏览记录" }));

    expect(screen.getByRole("heading", { name: "浏览记录 还是空的" })).toBeInTheDocument();
    expect(
      screen.getByText(/去详情页点击“收藏 \/ 加入愿望单 \/ 关注”/),
    ).toBeInTheDocument();
  });
});
