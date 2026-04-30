import { describe, expect, it } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import type { DashboardPayload, GameCard } from "../../types";
import {
  buildDashboardSections,
  filterGames,
  type LibraryFilterCriteria,
  type LibrarySortMode,
} from "./gameFilters";

const filters: LibraryFilterCriteria = {
  demoFilter: "all",
  hideAdultContent: true,
  minPlayers: 2,
  minReviewPct: 60,
  releaseWindow: "all",
  selectedTags: [],
  selectedLanguage: "all",
};

function createGames(
  baseGames: GameCard[],
  count: number,
  prefix: string,
  offset: number,
  section?: string,
) {
  return Array.from({ length: count }, (_, index) => {
    const template = baseGames[index % baseGames.length];

    return {
      ...template,
      appid: template.appid + offset + index,
      name: `${prefix} ${index + 1}`,
      section: section ?? template.section,
      userState: { ...template.userState },
    };
  });
}

const expandedDashboard: DashboardPayload = {
  ...mockDashboard,
  newGames: createGames(mockDashboard.newGames, 14, "新游", 10_000, "new"),
  classics: createGames(mockDashboard.classics, 14, "经典", 20_000, "classic"),
  recentDiscoveries: createGames(
    [...mockDashboard.upcoming, ...mockDashboard.newGames, ...mockDashboard.classics],
    10,
    "最近",
    30_000,
    "recent",
  ),
};

describe("gameFilters", () => {
  it("keeps home summaries separate from full browse collections", () => {
    const homeSections = buildDashboardSections({
      activeView: "home",
      dashboard: expandedDashboard,
      filters,
      query: "",
      sortMode: "recommended",
    });
    const browseSections = buildDashboardSections({
      activeView: "browse",
      dashboard: expandedDashboard,
      filters,
      query: "",
      sortMode: "recommended",
    });

    expect(homeSections.map((section) => section.id)).toEqual([
      "new",
      "classic",
      "recent",
    ]);
    expect(browseSections.map((section) => section.id)).toEqual([
      "new",
      "classic",
      "recent",
    ]);
    const homeRecent = homeSections.find((section) => section.id === "recent");
    const browseRecent = browseSections.find((section) => section.id === "recent");
    const homeNew = homeSections.find((section) => section.id === "new");
    const browseNew = browseSections.find((section) => section.id === "new");
    const homeClassic = homeSections.find((section) => section.id === "classic");
    const browseClassic = browseSections.find((section) => section.id === "classic");

    expect(homeNew?.games).toHaveLength(6);
    expect(homeClassic?.games).toHaveLength(6);
    expect(homeRecent?.games).toHaveLength(3);
    expect(browseNew?.games).toHaveLength(14);
    expect(browseClassic?.games).toHaveLength(14);
    expect(browseRecent?.games).toHaveLength(10);
  });

  it("filters games by tag and review floor", () => {
    const result = filterGames(
      [...mockDashboard.newGames, ...mockDashboard.classics],
      "",
      {
        ...filters,
        minReviewPct: 90,
        selectedTags: ["射击"],
      },
      "reviews" satisfies LibrarySortMode,
    );

    expect(result.map((game) => game.name)).toEqual([
      "Deep Rock Galactic",
      "Left 4 Dead 2",
    ]);
    expect(result.every((game) => game.tags.includes("射击"))).toBe(true);
    expect(result.every((game) => (game.positiveReviewPct ?? 0) >= 90)).toBe(
      true,
    );
  });

  it("filters adult content and language-specific results using metadata", () => {
    const result = filterGames(
      [...mockDashboard.newGames, ...mockDashboard.classics, ...mockDashboard.upcoming].map(
        (game, index) => ({
          ...game,
          isAdultContent: index === 0,
          supportedLanguages:
            game.appid === mockDashboard.upcoming[0]?.appid
              ? ["english", "schinese"]
              : ["english"],
        }),
      ),
      "",
      {
        ...filters,
        selectedLanguage: "schinese",
      },
      "recommended" satisfies LibrarySortMode,
    );

    expect(result.some((game) => game.isAdultContent)).toBe(false);
    expect(result.every((game) => game.supportedLanguages.includes("schinese"))).toBe(
      true,
    );
  });
});
