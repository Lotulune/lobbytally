import type {
  AiAssessment,
  DashboardPayload,
  GameCard,
  SaveConfigRequest,
} from "../types";

export type ViewId =
  | "home"
  | "new"
  | "classic"
  | "upcoming"
  | "wishlist"
  | "browse"
  | "filter"
  | "saved"
  | "history"
  | "settings"
  | "about"
  | "ai"
  | "detail";

export type LibrarySortMode = "recommended" | "reviews" | "players" | "release";
export type DemoFilter = "all" | "demo_only" | "released_with_demo" | "released";
export type ReleaseWindow = "all" | "week" | "month" | "quarter" | "year";
export type LanguageFilter = "all" | "schinese" | "english";

export interface LibraryFilters {
  demoFilter: DemoFilter;
  hideAdultContent: boolean;
  minPlayers: number;
  minReviewPct: number;
  releaseWindow: ReleaseWindow;
  selectedTags: string[];
  selectedLanguage: LanguageFilter;
}

export interface PageSharedProps {
  dashboard: DashboardPayload;
  allGames: GameCard[];
  selectedGame: GameCard | null;
  isBusy: boolean;
  assessment: AiAssessment | null;
  status: string;
  onOpenGame: (game: GameCard) => void;
  onSaveConfig: (request: SaveConfigRequest) => Promise<void>;
}
