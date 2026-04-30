import type { DashboardPayload, GameCard } from "../../types";
import type {
  DemoFilter,
  LibraryFilters,
  LibrarySortMode,
  ReleaseWindow,
  ViewId,
} from "../../pages/types";

export type LibraryFilterCriteria = Pick<
  LibraryFilters,
  | "demoFilter"
  | "hideAdultContent"
  | "minPlayers"
  | "minReviewPct"
  | "releaseWindow"
  | "selectedLanguage"
  | "selectedTags"
>;

export type { LibrarySortMode } from "../../pages/types";

export interface DashboardSection {
  id: "new" | "classic" | "recent";
  title: string;
  subtitle: string;
  games: GameCard[];
}

const HOME_SECTION_LIMITS: Record<DashboardSection["id"], number> = {
  new: 6,
  classic: 6,
  recent: 3,
};

export function filterGames(
  games: GameCard[],
  query: string,
  filters: LibraryFilterCriteria,
  sortMode: LibrarySortMode,
) {
  const normalizedQuery = query.trim().toLowerCase();
  const normalizedSelectedTags = filters.selectedTags.map((tag) => tag.toLowerCase());
  const today = new Date();

  return games
    .filter((game) => {
      const haystack = [
        game.name,
        ...game.tags,
        ...game.multiplayerModes,
        game.aiSummary,
      ]
        .join(" ")
        .toLowerCase();
      return normalizedQuery ? haystack.includes(normalizedQuery) : true;
    })
    .filter((game) => matchesDemoFilter(game, filters.demoFilter))
    .filter((game) => (filters.hideAdultContent ? !game.isAdultContent : true))
    .filter((game) => matchesReleaseWindow(game, filters.releaseWindow, today))
    .filter((game) => (game.currentPlayers ?? 0) >= filters.minPlayers)
    .filter((game) => (game.positiveReviewPct ?? 0) >= filters.minReviewPct)
    .filter((game) =>
      filters.selectedLanguage === "all"
        ? true
        : game.supportedLanguages.some(
            (language) => language.toLowerCase() === filters.selectedLanguage,
          ),
    )
    .filter((game) =>
      normalizedSelectedTags.length === 0
        ? true
        : game.tags.some((tag) => normalizedSelectedTags.includes(tag.toLowerCase())),
    )
    .sort((left, right) => compareGames(left, right, sortMode));
}

export function buildDashboardSections({
  activeView,
  dashboard,
  filters,
  query,
  sortMode,
}: {
  activeView: ViewId;
  dashboard: DashboardPayload;
  filters: LibraryFilterCriteria;
  query: string;
  sortMode: LibrarySortMode;
}): DashboardSection[] {
  const visibleNewGames = filterGames(dashboard.newGames, query, filters, sortMode);
  const visibleClassics = filterGames(dashboard.classics, query, filters, sortMode);
  const visibleRecentDiscoveries = filterGames(
    dashboard.recentDiscoveries,
    query,
    filters,
    sortMode,
  );

  const sections: Array<DashboardSection & { visible: boolean }> = [
    {
      id: "new",
      title: "新游区",
      subtitle: "近一个月发布的多人游戏",
      games:
        activeView === "home"
          ? visibleNewGames.slice(0, HOME_SECTION_LIMITS.new)
          : visibleNewGames,
      visible: ["home", "new", "browse"].includes(activeView),
    },
    {
      id: "classic",
      title: "精品老游区",
      subtitle: "经典多人游戏推荐",
      games:
        activeView === "home"
          ? visibleClassics.slice(0, HOME_SECTION_LIMITS.classic)
          : visibleClassics,
      visible: ["home", "classic", "browse"].includes(activeView),
    },
    {
      id: "recent",
      title: "最近发现",
      subtitle: "刚导入到本地库的多人游戏",
      games:
        activeView === "home"
          ? visibleRecentDiscoveries.slice(0, HOME_SECTION_LIMITS.recent)
          : visibleRecentDiscoveries,
      visible: ["home", "browse"].includes(activeView),
    },
  ];

  return sections
    .filter((section) => section.visible && section.games.length > 0)
    .map(({ visible, ...section }) => section);
}

export function matchesDemoFilter(game: GameCard, demoFilter: DemoFilter) {
  return demoFilter === "all" ? true : game.demoStatus === demoFilter;
}

export function matchesReleaseWindow(
  game: GameCard,
  releaseWindow: ReleaseWindow,
  today: Date,
) {
  if (releaseWindow === "all") return true;
  if (game.releaseState === "tba") return false;

  const days = daysSinceRelease(game.releaseDate, today);
  if (days === null) return false;

  const distance =
    game.releaseState === "upcoming"
      ? days < 0
        ? Math.abs(days)
        : null
      : days >= 0
        ? days
        : null;
  if (distance === null) return false;

  switch (releaseWindow) {
    case "week":
      return distance <= 7;
    case "month":
      return distance <= 30;
    case "quarter":
      return distance <= 90;
    case "year":
      return distance <= 365;
  }
}

export function compareGames(
  left: GameCard,
  right: GameCard,
  sortMode: LibrarySortMode,
) {
  switch (sortMode) {
    case "reviews":
      return (right.positiveReviewPct ?? 0) - (left.positiveReviewPct ?? 0);
    case "players":
      return (right.currentPlayers ?? 0) - (left.currentPlayers ?? 0);
    case "release":
      return (right.releaseDate ?? "").localeCompare(left.releaseDate ?? "");
    case "recommended":
      return right.recommendationScore - left.recommendationScore;
  }
}

function daysSinceRelease(
  releaseDate: string | null | undefined,
  today: Date,
): number | null {
  if (!releaseDate) return null;
  const release = new Date(`${releaseDate}T00:00:00Z`);
  if (Number.isNaN(release.getTime())) return null;
  const todayUtc = Date.UTC(
    today.getUTCFullYear(),
    today.getUTCMonth(),
    today.getUTCDate(),
  );
  return Math.floor((todayUtc - release.getTime()) / 86_400_000);
}
