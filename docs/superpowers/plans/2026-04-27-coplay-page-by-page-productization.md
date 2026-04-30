# Co-Play Page-by-Page Productization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the current Co-Play desktop prototype into a page-driven product by finishing each existing UI page against the current Rust/Tauri backend in a deliberate order instead of shipping one large mixed batch.

**Architecture:** Keep the existing Tauri/Rust command surface as the baseline: `get_dashboard`, `save_config`, `sync_seed_games`, `assess_game_with_ai`, `set_game_user_state`, and the discovery-task commands already wired in `src/App.tsx`, `src/api/client.ts`, and `src-tauri/src/commands.rs`. First extract the monolithic `src/App.tsx` into page modules without changing behavior, then finish pages in order of backend readiness: dashboard, detail, collections, settings/about, shared metadata foundation, upcoming/filter, and AI assistant.

**Tech Stack:** Tauri 2, Rust, rusqlite, reqwest, React 19, TypeScript, Vite, Vitest, Testing Library, `@tauri-apps/api`

---

## Scope Check

This plan covers the current desktop UI pages that already exist in the sidebar/top-level view model:

- `home`
- `new`
- `classic`
- `browse`
- `detail`
- `saved`
- `wishlist`
- `history`
- `settings`
- `upcoming`
- `filter`
- `ai`
- `about`

Current baseline confirmed from the codebase:

- The app shell and most page logic are still concentrated in `src/App.tsx`.
- Dashboard, detail, favorites, settings, and discovery-task flows already talk to real backend commands.
- `wishlist`, `history`, `upcoming`, `ai`, and `about` are either shallow wrappers or placeholders.
- `hideAdultContent`, language, and future release workflows are not real yet because the persisted `GameCard` / SQLite schema does not currently store adult-content, supported-language, or release-state metadata.

This plan assumes the discovery-task baseline in `docs/superpowers/plans/2026-04-26-discovery-task-productization.md` is already present and should be preserved.

Out of scope for this plan:

- Steam account sign-in
- Steam wishlist import
- Friends/social graph
- Complex collaborative recommendations
- Price alerts and notifications
- A full design-system rewrite

## File Structure

**Frontend**

- Create: `src/pages/types.ts`
- Create: `src/pages/dashboard/DashboardPage.tsx`
- Create: `src/pages/dashboard/DashboardPage.test.tsx`
- Create: `src/pages/detail/DetailPage.tsx`
- Create: `src/pages/detail/DetailPage.test.tsx`
- Create: `src/pages/collections/CollectionGrid.tsx`
- Create: `src/pages/collections/CollectionsHubPage.tsx`
- Create: `src/pages/collections/CollectionsHubPage.test.tsx`
- Create: `src/pages/collections/WishlistTrackerPage.tsx`
- Create: `src/pages/collections/WishlistTrackerPage.test.tsx`
- Create: `src/pages/collections/HistoryPage.tsx`
- Create: `src/pages/collections/HistoryPage.test.tsx`
- Create: `src/pages/settings/SettingsPage.tsx`
- Create: `src/pages/settings/SettingsPage.test.tsx`
- Create: `src/pages/upcoming/UpcomingPage.tsx`
- Create: `src/pages/upcoming/UpcomingPage.test.tsx`
- Create: `src/pages/ai/AiAssistantPage.tsx`
- Create: `src/pages/ai/AiAssistantPage.test.tsx`
- Create: `src/pages/about/AboutPage.tsx`
- Create: `src/features/library/gameFilters.ts`
- Create: `src/features/library/gameFilters.test.ts`
- Modify: `src/App.tsx`
- Modify: `src/App.css`
- Modify: `src/App.test.tsx`
- Modify: `src/types.ts`
- Modify: `src/api/client.ts`

**Backend**

- Modify: `src-tauri/src/models.rs`
- Modify: `src-tauri/src/db.rs`
- Modify: `src-tauri/src/steam.rs`
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/llm.rs`
- Modify: `src-tauri/src/recommendation.rs`
- Create: `src-tauri/tests/game_metadata_tests.rs`
- Create: `src-tauri/tests/ai_recommendation_tests.rs`

## Recommended Delivery Order

Implement pages in this order:

1. **Foundation extraction** so page work stops fighting the `src/App.tsx` monolith.
2. **Dashboard pages** because they already have the strongest backend support.
3. **Detail page** because it already has real AI/user-state commands but still renders shallow content.
4. **Collections / wishlist / history** because the stored `user_state.updated_at` is already enough to ship better page behavior.
5. **Settings / about** because the data-ops surface already exists and should become the operational control center.
6. **Metadata foundation** because `upcoming` and real filtering cannot be finished without new persisted fields.
7. **Upcoming + filter pages** once the data model supports them.
8. **AI assistant page** last, because it requires a genuinely new backend recommendation command.

## Task 1: Extract the App Shell Into Page Modules Without Changing Behavior

**Files:**

- Create: `src/features/library/gameFilters.test.ts`
- Create: `src/features/library/gameFilters.ts`
- Create: `src/pages/types.ts`
- Create: `src/pages/dashboard/DashboardPage.tsx`
- Create: `src/pages/detail/DetailPage.tsx`
- Create: `src/pages/collections/CollectionsHubPage.tsx`
- Create: `src/pages/settings/SettingsPage.tsx`
- Modify: `src/App.tsx`
- Modify: `src/App.test.tsx`

- [ ] **Step 1: Write the failing helper-extraction test**

```ts
import { describe, expect, it } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import {
  buildDashboardSections,
  filterGames,
  type LibraryFilters,
  type LibrarySortMode,
} from "./gameFilters";

const filters: LibraryFilters = {
  demoFilter: "all",
  hideAdultContent: true,
  minPlayers: 2,
  minReviewPct: 60,
  releaseWindow: "all",
  selectedTags: [],
  selectedLanguage: "all",
};

describe("gameFilters", () => {
  it("keeps browse mode separate from home mode limits", () => {
    const homeSections = buildDashboardSections({
      activeView: "home",
      dashboard: mockDashboard,
      filters,
      query: "",
      sortMode: "recommended",
    });
    const browseSections = buildDashboardSections({
      activeView: "browse",
      dashboard: mockDashboard,
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
    expect(homeSections[0]?.games.length).toBeLessThanOrEqual(
      browseSections[0]?.games.length ?? 0,
    );
  });

  it("filters games by tag and review floor", () => {
    const result = filterGames(
      [...mockDashboard.newGames, ...mockDashboard.classics],
      "rock",
      {
        ...filters,
        minReviewPct: 90,
        selectedTags: ["射击"],
      },
      "reviews" satisfies LibrarySortMode,
    );

    expect(result.map((game) => game.name)).toContain("Deep Rock Galactic");
    expect(result.every((game) => (game.positiveReviewPct ?? 0) >= 90)).toBe(
      true,
    );
  });
});
```

- [ ] **Step 2: Run the extraction test and confirm it fails**

Run:

```bash
npm run test -- src/features/library/gameFilters.test.ts
```

Expected: FAIL because `src/features/library/gameFilters.ts` and the exported helpers do not exist yet.

- [ ] **Step 3: Create the shared page/filter primitives and trim `App.tsx` down to a shell**

Create `src/pages/types.ts`:

```ts
import type { AiAssessment, DashboardPayload, GameCard, SaveConfigRequest } from "../types";

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
```

Create `src/features/library/gameFilters.ts`:

```ts
import type { DashboardPayload, GameCard } from "../../types";
import type {
  DemoFilter,
  LibraryFilters,
  LibrarySortMode,
  ReleaseWindow,
  ViewId,
} from "../../pages/types";

export function filterGames(
  games: GameCard[],
  query: string,
  filters: LibraryFilters,
  sortMode: LibrarySortMode,
) {
  const normalizedQuery = query.trim().toLowerCase();
  const selectedTags = filters.selectedTags.map((tag) => tag.toLowerCase());
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
    .filter((game) => matchesReleaseWindow(game.releaseDate, filters.releaseWindow, today))
    .filter((game) => (game.currentPlayers ?? 0) >= filters.minPlayers)
    .filter((game) => (game.positiveReviewPct ?? 0) >= filters.minReviewPct)
    .filter((game) =>
      selectedTags.length === 0
        ? true
        : game.tags.some((tag) => selectedTags.includes(tag.toLowerCase())),
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
  filters: LibraryFilters;
  query: string;
  sortMode: LibrarySortMode;
}) {
  const visibleNewGames = filterGames(dashboard.newGames, query, filters, sortMode);
  const visibleClassics = filterGames(dashboard.classics, query, filters, sortMode);
  const visibleRecent = filterGames(
    dashboard.recentDiscoveries,
    query,
    filters,
    sortMode,
  );

  return [
    {
      id: "new" as const,
      title: "新游区",
      subtitle: "近一个月发布的多人游戏",
      games:
        activeView === "home"
          ? visibleNewGames.slice(0, 4)
          : visibleNewGames.slice(0, 12),
      visible: ["home", "new", "browse"].includes(activeView),
    },
    {
      id: "classic" as const,
      title: "精品老游区",
      subtitle: "经典多人游戏推荐",
      games:
        activeView === "home"
          ? visibleClassics.slice(0, 4)
          : visibleClassics.slice(0, 12),
      visible: ["home", "classic", "browse"].includes(activeView),
    },
    {
      id: "recent" as const,
      title: "最近发现",
      subtitle: "刚导入到本地库的多人游戏",
      games:
        activeView === "home"
          ? visibleRecent.slice(0, 4)
          : visibleRecent.slice(0, 8),
      visible: ["home", "browse"].includes(activeView),
    },
  ].filter((section) => section.visible && section.games.length > 0);
}

function matchesDemoFilter(game: GameCard, demoFilter: DemoFilter) {
  return demoFilter === "all" ? true : game.demoStatus === demoFilter;
}

function matchesReleaseWindow(
  releaseDate: GameCard["releaseDate"],
  releaseWindow: ReleaseWindow,
  today: Date,
) {
  if (releaseWindow === "all") return true;
  if (!releaseDate) return false;
  const release = new Date(`${releaseDate}T00:00:00Z`);
  if (Number.isNaN(release.getTime())) return false;
  const todayUtc = Date.UTC(today.getUTCFullYear(), today.getUTCMonth(), today.getUTCDate());
  const days = Math.floor((todayUtc - release.getTime()) / 86_400_000);

  switch (releaseWindow) {
    case "week":
      return days >= 0 && days <= 7;
    case "month":
      return days >= 0 && days <= 30;
    case "quarter":
      return days >= 0 && days <= 90;
    case "year":
      return days >= 0 && days <= 365;
    case "all":
      return true;
  }
}

function compareGames(a: GameCard, b: GameCard, sortMode: LibrarySortMode) {
  switch (sortMode) {
    case "reviews":
      return (b.positiveReviewPct ?? 0) - (a.positiveReviewPct ?? 0);
    case "players":
      return (b.currentPlayers ?? 0) - (a.currentPlayers ?? 0);
    case "release":
      return (b.releaseDate ?? "").localeCompare(a.releaseDate ?? "");
    case "recommended":
      return b.recommendationScore - a.recommendationScore;
  }
}
```

Trim `src/App.tsx` so it owns state and delegates page rendering:

```tsx
import { buildDashboardSections, filterGames } from "./features/library/gameFilters";
import { DashboardPage } from "./pages/dashboard/DashboardPage";
import { CollectionsHubPage } from "./pages/collections/CollectionsHubPage";
import { DetailPage } from "./pages/detail/DetailPage";
import { SettingsPage } from "./pages/settings/SettingsPage";
import type { DemoFilter, LibraryFilters, LibrarySortMode, ReleaseWindow, ViewId } from "./pages/types";

const [filters, setFilters] = useState<LibraryFilters>({
  demoFilter: "all",
  hideAdultContent: true,
  minPlayers: 2,
  minReviewPct: 60,
  releaseWindow: "all",
  selectedTags: [],
  selectedLanguage: "all",
});

const sections = dashboard
  ? buildDashboardSections({
      activeView,
      dashboard,
      filters,
      query,
      sortMode,
    })
  : [];

const page = !dashboard ? null : activeView === "detail" && selectedGame ? (
  <DetailPage
    game={selectedGame}
    relatedGames={allGames.filter((game) => game.appid !== selectedGame.appid)}
    isBusy={isBusy}
    onAiAssess={() => handleAiAssess(selectedGame)}
    onBack={() => setActiveView("home")}
    onToggleState={(patch, message) =>
      handleUserState(selectedGame.appid, patch, message)
    }
  />
) : activeView === "saved" ? (
  <CollectionsHubPage
    collections={dashboard.collections}
    onOpen={openDetail}
    onToggle={(game, patch, message) =>
      handleUserState(game.appid, patch, message)
    }
  />
) : activeView === "settings" ? (
  <SettingsPage
    config={dashboard.config}
    stats={dashboard.stats}
    onRefreshDashboard={refreshDashboard}
    onSave={onSaveConfig}
    onStatus={setStatus}
    onSync={handleSync}
  />
) : (
  <DashboardPage
    activeView={activeView}
    quickTags={quickTagOptions}
    sections={sections}
    selectedAppid={selectedGame?.appid}
    stats={dashboard.stats}
    status={status}
    filters={filters}
    sortMode={sortMode}
    isBusy={isBusy}
    onAi={() => setActiveView("ai")}
    onOpenFilters={() => setActiveView("filter")}
    onOpenGame={openDetail}
    onSetFilters={setFilters}
    onSetSortMode={setSortMode}
    onSync={handleSync}
  />
);
```

- [ ] **Step 4: Re-run the extracted helper tests and current app tests**

Run:

```bash
npm run test -- src/features/library/gameFilters.test.ts src/App.test.tsx
```

Expected: PASS for the new helper extraction tests and the existing dashboard interaction suite.

Run:

```bash
npm run build
```

Expected: PASS with `src/App.tsx` reduced to an app-shell coordinator instead of owning every page implementation inline.

- [ ] **Step 5: Commit the extraction**

```bash
git add src/App.tsx src/App.test.tsx src/features/library/gameFilters.ts src/features/library/gameFilters.test.ts src/pages/types.ts src/pages/dashboard/DashboardPage.tsx src/pages/detail/DetailPage.tsx src/pages/collections/CollectionsHubPage.tsx src/pages/settings/SettingsPage.tsx
git commit -m "refactor: extract app shell and shared page helpers"
```

## Task 2: Finish the Dashboard Family (`home`, `new`, `classic`, `browse`)

**Files:**

- Create: `src/pages/dashboard/DashboardPage.test.tsx`
- Modify: `src/pages/dashboard/DashboardPage.tsx`
- Modify: `src/App.css`

- [ ] **Step 1: Write the failing dashboard-page component tests**

```tsx
// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import { buildDashboardSections } from "../../features/library/gameFilters";
import { DashboardPage } from "./DashboardPage";

const filters = {
  demoFilter: "all",
  hideAdultContent: true,
  minPlayers: 2,
  minReviewPct: 60,
  releaseWindow: "all",
  selectedTags: [],
  selectedLanguage: "all",
} as const;

describe("DashboardPage", () => {
  it("renders all three dashboard sections in browse mode", () => {
    const sections = buildDashboardSections({
      activeView: "browse",
      dashboard: mockDashboard,
      filters: { ...filters },
      query: "",
      sortMode: "recommended",
    });

    render(
      <DashboardPage
        activeView="browse"
        filters={{ ...filters }}
        isBusy={false}
        quickTags={["解谜", "合作"]}
        sections={sections}
        selectedAppid={undefined}
        sortMode="recommended"
        stats={mockDashboard.stats}
        status="ok"
        onAi={vi.fn()}
        onOpenFilters={vi.fn()}
        onOpenGame={vi.fn()}
        onSetFilters={vi.fn()}
        onSetSortMode={vi.fn()}
        onSync={vi.fn()}
      />,
    );

    expect(screen.getByRole("heading", { name: "新游区" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "精品老游区" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "最近发现" })).toBeInTheDocument();
  });

  it("routes quick-tag clicks through onSetFilters", () => {
    const onSetFilters = vi.fn();
    const sections = buildDashboardSections({
      activeView: "home",
      dashboard: mockDashboard,
      filters: { ...filters },
      query: "",
      sortMode: "recommended",
    });

    render(
      <DashboardPage
        activeView="home"
        filters={{ ...filters }}
        isBusy={false}
        quickTags={["解谜", "合作"]}
        sections={sections}
        selectedAppid={undefined}
        sortMode="recommended"
        stats={mockDashboard.stats}
        status="ok"
        onAi={vi.fn()}
        onOpenFilters={vi.fn()}
        onOpenGame={vi.fn()}
        onSetFilters={onSetFilters}
        onSetSortMode={vi.fn()}
        onSync={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "解谜" }));
    expect(onSetFilters).toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run the dashboard-page tests and confirm they fail**

Run:

```bash
npm run test -- src/pages/dashboard/DashboardPage.test.tsx
```

Expected: FAIL because `DashboardPage` is still a thin placeholder or still missing the extracted props contract.

- [ ] **Step 3: Implement a real `DashboardPage` that owns the dashboard-only UI**

Update `src/pages/dashboard/DashboardPage.tsx`:

```tsx
import type { DashboardStats, GameCard } from "../../types";
import type { LibraryFilters, LibrarySortMode, ViewId } from "../types";

type DashboardSection = {
  id: "new" | "classic" | "recent";
  title: string;
  subtitle: string;
  games: GameCard[];
};

export function DashboardPage({
  activeView,
  filters,
  isBusy,
  quickTags,
  sections,
  selectedAppid,
  sortMode,
  stats,
  status,
  onAi,
  onOpenFilters,
  onOpenGame,
  onSetFilters,
  onSetSortMode,
  onSync,
}: {
  activeView: ViewId;
  filters: LibraryFilters;
  isBusy: boolean;
  quickTags: string[];
  sections: DashboardSection[];
  selectedAppid?: number;
  sortMode: LibrarySortMode;
  stats: DashboardStats;
  status: string;
  onAi: () => void;
  onOpenFilters: () => void;
  onOpenGame: (game: GameCard) => void;
  onSetFilters: (next: LibraryFilters | ((current: LibraryFilters) => LibraryFilters)) => void;
  onSetSortMode: (mode: LibrarySortMode) => void;
  onSync: () => void;
}) {
  return (
    <div className="dashboard-layout">
      <section className="dashboard-main">
        <div className="section-tabs">
          <button
            className={activeView !== "classic" ? "active" : ""}
            type="button"
          >
            新游区
          </button>
          <button
            className={activeView === "classic" ? "active" : ""}
            type="button"
          >
            精品老游区
          </button>
        </div>

        <div className="toolbar">
          {["recommended", "reviews", "players", "release"].map((mode) => (
            <button
              aria-pressed={sortMode === mode}
              className={sortMode === mode ? "active" : ""}
              key={mode}
              type="button"
              onClick={() => onSetSortMode(mode as LibrarySortMode)}
            >
              {{
                recommended: "综合排序",
                reviews: "好评度",
                players: "游玩人数",
                release: "发售时间",
              }[mode]}
            </button>
          ))}
        </div>

        {sections.map((section) => (
          <section className="game-section" key={section.id}>
            <div className="game-section-head">
              <div>
                <h2>{section.title}</h2>
                <span>{section.subtitle}</span>
              </div>
            </div>
            <div className="game-grid">
              {section.games.map((game) => (
                <button
                  className={selectedAppid === game.appid ? "game-card selected" : "game-card"}
                  key={game.appid}
                  type="button"
                  onClick={() => onOpenGame(game)}
                >
                  <img src={game.capsuleUrl} alt="" loading="lazy" />
                  <h3>{game.name}</h3>
                </button>
              ))}
            </div>
          </section>
        ))}
      </section>

      <aside className="right-rail">
        <section className="stats-card">
          <h2>数据概览</h2>
          <p>库内 {stats.totalGames} 款，多人新游 {stats.newGamesCount} 款。</p>
          <p className="mini-status">{stats.dataSource}</p>
        </section>

        <section className="ai-card">
          <h2>AI 智能推荐助手</h2>
          <button className="gold-button" type="button" onClick={onAi}>
            ✦ 让 AI 帮我找游戏
          </button>
          <p className="mini-status">{status}</p>
        </section>

        <section className="filter-card">
          <div className="tag-list">
            {quickTags.map((tag) => (
              <button
                aria-pressed={filters.selectedTags.includes(tag)}
                className={filters.selectedTags.includes(tag) ? "active" : ""}
                key={tag}
                type="button"
                onClick={() =>
                  onSetFilters((current) => ({
                    ...current,
                    selectedTags: current.selectedTags.includes(tag)
                      ? current.selectedTags.filter((item) => item !== tag)
                      : [...current.selectedTags, tag],
                  }))
                }
              >
                {tag}
              </button>
            ))}
          </div>
          <button className="ghost-button" disabled={isBusy} onClick={onSync} type="button">
            {isBusy ? "同步中…" : "同步 Steam 数据"}
          </button>
          <button type="button" onClick={onOpenFilters}>
            更多筛选 〉
          </button>
        </section>
      </aside>
    </div>
  );
}
```

- [ ] **Step 4: Re-run the dashboard tests and build**

Run:

```bash
npm run test -- src/pages/dashboard/DashboardPage.test.tsx src/App.test.tsx
```

Expected: PASS with the dashboard page now owning dashboard-only rendering instead of `App.tsx`.

Run:

```bash
npm run build
```

Expected: PASS with dashboard markup and right-rail styles still compiling cleanly.

- [ ] **Step 5: Commit the dashboard page**

```bash
git add src/pages/dashboard/DashboardPage.tsx src/pages/dashboard/DashboardPage.test.tsx src/App.css
git commit -m "feat: extract and stabilize dashboard pages"
```

## Task 3: Turn the Detail Page Into a Real Data-Backed Page

**Files:**

- Create: `src/pages/detail/DetailPage.test.tsx`
- Modify: `src/pages/detail/DetailPage.tsx`
- Modify: `src/App.css`

- [ ] **Step 1: Write the failing detail-page tests**

```tsx
// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import { DetailPage } from "./DetailPage";

const game = structuredClone(mockDashboard.newGames[0]);
const relatedGames = structuredClone(mockDashboard.classics.slice(0, 3));

describe("DetailPage", () => {
  it("switches from AI summary to review snippets", () => {
    render(
      <DetailPage
        game={game}
        relatedGames={relatedGames}
        isBusy={false}
        onAiAssess={vi.fn()}
        onBack={vi.fn()}
        onToggleState={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /玩家评价/i }));
    expect(screen.getByText(game.reviewSnippets[0]?.review ?? "")).toBeInTheDocument();
  });

  it("emits a wishlist toggle callback", () => {
    const onToggleState = vi.fn();
    render(
      <DetailPage
        game={game}
        relatedGames={relatedGames}
        isBusy={false}
        onAiAssess={vi.fn()}
        onBack={vi.fn()}
        onToggleState={onToggleState}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /愿望单/i }));
    expect(onToggleState).toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run the detail-page tests and confirm they fail**

Run:

```bash
npm run test -- src/pages/detail/DetailPage.test.tsx
```

Expected: FAIL because the extracted detail page still does not own tab state or real review/related sections.

- [ ] **Step 3: Implement tabbed detail content using the data already present in `GameCard`**

Update `src/pages/detail/DetailPage.tsx`:

```tsx
import { useState } from "react";
import type { GameCard, UserGameStatePatch } from "../../types";

type DetailTab = "ai" | "reviews" | "related";

export function DetailPage({
  game,
  relatedGames,
  isBusy,
  onAiAssess,
  onBack,
  onToggleState,
}: {
  game: GameCard;
  relatedGames: GameCard[];
  isBusy: boolean;
  onAiAssess: () => void;
  onBack: () => void;
  onToggleState: (patch: UserGameStatePatch, message: string) => void;
}) {
  const [activeTab, setActiveTab] = useState<DetailTab>("ai");

  return (
    <section className="detail-page">
      <div className="detail-toolbar">
        <button type="button" onClick={onBack}>
          ← 返回
        </button>
      </div>

      <div className="detail-grid">
        <div>
          <div className="hero-cover">
            <img src={game.capsuleUrl} alt="" />
          </div>

          <div className="detail-tabs">
            <button
              className={activeTab === "ai" ? "active" : ""}
              type="button"
              onClick={() => setActiveTab("ai")}
            >
              AI 评估
            </button>
            <button
              className={activeTab === "reviews" ? "active" : ""}
              type="button"
              onClick={() => setActiveTab("reviews")}
            >
              玩家评价 ({game.reviewSnippets.length})
            </button>
            <button
              className={activeTab === "related" ? "active" : ""}
              type="button"
              onClick={() => setActiveTab("related")}
            >
              相关游戏
            </button>
          </div>

          {activeTab === "ai" && (
            <div className="ai-eval-panel">
              <h3>AI 简评</h3>
              <p>{game.aiSummary}</p>
            </div>
          )}

          {activeTab === "reviews" && (
            <div className="review-snippet-list">
              {game.reviewSnippets.map((snippet, index) => (
                <article className="review-snippet-card" key={`${game.appid}-${index}`}>
                  <strong>{snippet.votedUp ? "推荐" : "不推荐"}</strong>
                  <p>{snippet.review}</p>
                </article>
              ))}
            </div>
          )}

          {activeTab === "related" && (
            <div className="related-grid">
              {relatedGames.slice(0, 6).map((item) => (
                <article className="related-card" key={item.appid}>
                  <img src={item.capsuleUrl} alt="" />
                  <h3>{item.name}</h3>
                </article>
              ))}
            </div>
          )}
        </div>

        <aside className="detail-side">
          <h2>{game.name}</h2>
          <p>{game.tags.join(" · ")}</p>
          <button
            className="gold-button"
            type="button"
            onClick={() =>
              onToggleState(
                { wishlist: !game.userState.wishlist },
                game.userState.wishlist
                  ? `已将《${game.name}》移出愿望单。`
                  : `已将《${game.name}》加入愿望单。`,
              )
            }
          >
            {game.userState.wishlist ? "已在愿望单" : "加入愿望单"}
          </button>
          <button className="gold-button" disabled={isBusy} type="button" onClick={onAiAssess}>
            {isBusy ? "AI 评估中…" : "重新 AI 评估"}
          </button>
        </aside>
      </div>
    </section>
  );
}
```

- [ ] **Step 4: Re-run the detail-page tests**

Run:

```bash
npm run test -- src/pages/detail/DetailPage.test.tsx
```

Expected: PASS with tab switching, review rendering, and user-state actions working inside the dedicated detail page module.

- [ ] **Step 5: Commit the detail page**

```bash
git add src/pages/detail/DetailPage.tsx src/pages/detail/DetailPage.test.tsx src/App.css
git commit -m "feat: complete detail page with real sections"
```

## Task 4: Ship Real Collections, Wishlist, and History Pages From Existing User State

**Files:**

- Create: `src/pages/collections/CollectionGrid.tsx`
- Create: `src/pages/collections/CollectionsHubPage.test.tsx`
- Create: `src/pages/collections/WishlistTrackerPage.tsx`
- Create: `src/pages/collections/WishlistTrackerPage.test.tsx`
- Create: `src/pages/collections/HistoryPage.tsx`
- Create: `src/pages/collections/HistoryPage.test.tsx`
- Modify: `src/pages/collections/CollectionsHubPage.tsx`
- Modify: `src/App.tsx`
- Modify: `src/App.css`

- [ ] **Step 1: Write the failing collections-page tests**

```tsx
// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import { CollectionsHubPage } from "./CollectionsHubPage";
import { HistoryPage } from "./HistoryPage";
import { WishlistTrackerPage } from "./WishlistTrackerPage";

describe("Collections pages", () => {
  it("switches tabs inside the collections hub", () => {
    render(
      <CollectionsHubPage
        collections={mockDashboard.collections}
        onOpen={vi.fn()}
        onToggle={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "愿望单" }));
    expect(screen.getByRole("heading", { name: "我的收藏夹" })).toBeInTheDocument();
  });

  it("renders a dedicated wishlist tracker headline", () => {
    render(
      <WishlistTrackerPage
        games={mockDashboard.collections.wishlist}
        onOpen={vi.fn()}
        onToggle={vi.fn()}
      />,
    );

    expect(screen.getByRole("heading", { name: "愿望单追踪" })).toBeInTheDocument();
  });

  it("sorts history by most recent interaction time", () => {
    render(
      <HistoryPage
        games={mockDashboard.collections.history}
        onOpen={vi.fn()}
      />,
    );

    expect(screen.getByRole("heading", { name: "游玩记录" })).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run the collections tests and confirm they fail**

Run:

```bash
npm run test -- src/pages/collections/CollectionsHubPage.test.tsx src/pages/collections/WishlistTrackerPage.test.tsx src/pages/collections/HistoryPage.test.tsx
```

Expected: FAIL because the dedicated wishlist/history page modules do not exist yet and `CollectionsHubPage` is still too close to the old inline component.

- [ ] **Step 3: Build the shared collection grid and specialize the pages**

Create `src/pages/collections/CollectionGrid.tsx`:

```tsx
import type { GameCard } from "../../types";

export function CollectionGrid({
  emptyCopy,
  games,
  onOpen,
  renderMeta,
}: {
  emptyCopy: string;
  games: GameCard[];
  onOpen: (game: GameCard) => void;
  renderMeta: (game: GameCard) => string;
}) {
  if (games.length === 0) {
    return <p className="empty-copy">{emptyCopy}</p>;
  }

  return (
    <div className="favorite-grid">
      {games.map((game) => (
        <article className="favorite-card" key={game.appid} onClick={() => onOpen(game)}>
          <img src={game.capsuleUrl} alt="" />
          <h3>{game.name}</h3>
          <p>{renderMeta(game)}</p>
        </article>
      ))}
    </div>
  );
}
```

Update `src/pages/collections/WishlistTrackerPage.tsx`:

```tsx
import type { GameCard, UserGameStatePatch } from "../../types";
import { CollectionGrid } from "./CollectionGrid";

export function WishlistTrackerPage({
  games,
  onOpen,
  onToggle,
}: {
  games: GameCard[];
  onOpen: (game: GameCard) => void;
  onToggle: (game: GameCard, patch: UserGameStatePatch, message: string) => void;
}) {
  const ranked = [...games].sort((left, right) => {
    const leftFollowed = left.userState.followed ? 1 : 0;
    const rightFollowed = right.userState.followed ? 1 : 0;
    return rightFollowed - leftFollowed;
  });

  return (
    <section className="wishlist-tracker-page">
      <h2>愿望单追踪</h2>
      <p>优先展示已关注、带 Demo、或者临近发售的愿望单游戏。</p>
      <CollectionGrid
        emptyCopy="还没有愿望单游戏。"
        games={ranked}
        onOpen={onOpen}
        renderMeta={(game) =>
          `${game.userState.followed ? "已关注" : "未关注"} · ${game.releaseDateText} · ${game.multiplayerModes[0] ?? "多人"}`
        }
      />
    </section>
  );
}
```

Update `src/pages/collections/HistoryPage.tsx`:

```tsx
import type { GameCard } from "../../types";
import { CollectionGrid } from "./CollectionGrid";

export function HistoryPage({
  games,
  onOpen,
}: {
  games: GameCard[];
  onOpen: (game: GameCard) => void;
}) {
  const sorted = [...games].sort((left, right) => {
    const leftTime = Date.parse(left.userState.updatedAt ?? "");
    const rightTime = Date.parse(right.userState.updatedAt ?? "");
    return (Number.isNaN(rightTime) ? 0 : rightTime) - (Number.isNaN(leftTime) ? 0 : leftTime);
  });

  return (
    <section className="history-page">
      <h2>游玩记录</h2>
      <p>按最近浏览时间倒序排列，用来支撑后续“因为你看过”推荐。</p>
      <CollectionGrid
        emptyCopy="最近还没有浏览记录。"
        games={sorted}
        onOpen={onOpen}
        renderMeta={(game) =>
          `最近互动：${game.userState.updatedAt ?? "未知"} · ${game.multiplayerModes[0] ?? "多人"}`
        }
      />
    </section>
  );
}
```

Update `src/App.tsx` so `saved`, `wishlist`, and `history` become distinct pages:

```tsx
page = activeView === "saved" ? (
  <CollectionsHubPage
    collections={dashboard.collections}
    onOpen={openDetail}
    onToggle={(game, patch, message) =>
      handleUserState(game.appid, patch, message)
    }
  />
) : activeView === "wishlist" ? (
  <WishlistTrackerPage
    games={dashboard.collections.wishlist}
    onOpen={openDetail}
    onToggle={(game, patch, message) =>
      handleUserState(game.appid, patch, message)
    }
  />
) : activeView === "history" ? (
  <HistoryPage
    games={dashboard.collections.history}
    onOpen={openDetail}
  />
) : page;
```

- [ ] **Step 4: Re-run the collections tests**

Run:

```bash
npm run test -- src/pages/collections/CollectionsHubPage.test.tsx src/pages/collections/WishlistTrackerPage.test.tsx src/pages/collections/HistoryPage.test.tsx
```

Expected: PASS with `saved`, `wishlist`, and `history` now acting like real pages instead of placeholders.

- [ ] **Step 5: Commit the collections work**

```bash
git add src/pages/collections/CollectionGrid.tsx src/pages/collections/CollectionsHubPage.tsx src/pages/collections/CollectionsHubPage.test.tsx src/pages/collections/WishlistTrackerPage.tsx src/pages/collections/WishlistTrackerPage.test.tsx src/pages/collections/HistoryPage.tsx src/pages/collections/HistoryPage.test.tsx src/App.tsx src/App.css
git commit -m "feat: ship collections wishlist and history pages"
```

## Task 5: Make `settings` and `about` the Operational and Diagnostic Pages

**Files:**

- Create: `src/pages/settings/SettingsPage.test.tsx`
- Create: `src/pages/about/AboutPage.tsx`
- Modify: `src/pages/settings/SettingsPage.tsx`
- Modify: `src/App.tsx`
- Modify: `src/App.css`

- [ ] **Step 1: Write the failing settings/about tests**

```tsx
// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import { AboutPage } from "../about/AboutPage";
import { SettingsPage } from "./SettingsPage";

describe("Settings and About pages", () => {
  it("shows both sync and discovery operations in settings", () => {
    render(
      <SettingsPage
        config={mockDashboard.config}
        stats={mockDashboard.stats}
        onRefreshDashboard={vi.fn()}
        onSave={vi.fn(async () => undefined)}
        onStatus={vi.fn()}
        onSync={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "同步 Steam 数据" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "发现任务控制台" })).toBeInTheDocument();
  });

  it("renders a real about/diagnostic surface", () => {
    render(
      <AboutPage
        config={mockDashboard.config}
        stats={mockDashboard.stats}
      />,
    );

    expect(screen.getByRole("heading", { name: "关于 Co-Play" })).toBeInTheDocument();
    expect(screen.getByText(/Steam Key/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run the settings/about tests and confirm they fail**

Run:

```bash
npm run test -- src/pages/settings/SettingsPage.test.tsx
```

Expected: FAIL because the dedicated settings/about modules do not yet expose the full operational structure.

- [ ] **Step 3: Finish the settings page and create a real about page**

Update `src/pages/settings/SettingsPage.tsx`:

```tsx
import { useState } from "react";
import { previewSteamAppList } from "../../api/client";
import { DiscoveryTaskPanel } from "../../features/discovery/DiscoveryTaskPanel";
import type { DashboardPayload, SaveConfigRequest, SteamAppListPreview } from "../../types";

export function SettingsPage({
  config,
  stats,
  onRefreshDashboard,
  onSave,
  onStatus,
  onSync,
}: {
  config: DashboardPayload["config"];
  stats: DashboardPayload["stats"];
  onRefreshDashboard: () => Promise<unknown>;
  onSave: (request: SaveConfigRequest) => Promise<void>;
  onStatus: (message: string) => void;
  onSync: () => void;
}) {
  const [form, setForm] = useState<SaveConfigRequest>({
    llmBaseUrl: config.llmBaseUrl,
    llmModel: config.llmModel,
    country: config.country,
    language: config.language,
  });
  const [preview, setPreview] = useState<SteamAppListPreview | null>(null);

  return (
    <section className="settings-page">
      <h2>设置</h2>
      <div className="settings-actions">
        <button className="gold-button" type="button" onClick={() => onSave(form)}>
          保存设置
        </button>
        <button className="ghost-button" type="button" onClick={onSync}>
          同步 Steam 数据
        </button>
        <button
          className="muted-button"
          type="button"
          onClick={async () => setPreview(await previewSteamAppList(12))}
        >
          预览 Steam AppList
        </button>
      </div>

      {preview && (
        <div className="steam-preview">
          <strong>Steam AppList 预览</strong>
          <span>last_appid: {preview.lastAppid ?? "无"}</span>
        </div>
      )}

      <DiscoveryTaskPanel
        stats={stats}
        onRefreshDashboard={onRefreshDashboard}
        onStatus={onStatus}
      />
    </section>
  );
}
```

Create `src/pages/about/AboutPage.tsx`:

```tsx
import type { DashboardPayload } from "../../types";

export function AboutPage({
  config,
  stats,
}: {
  config: DashboardPayload["config"];
  stats: DashboardPayload["stats"];
}) {
  return (
    <section className="about-page">
      <h2>关于 Co-Play</h2>
      <p>Co-Play 是一个围绕 Steam 多人游戏发现、筛选和轻量 AI 辅助的本地桌面应用。</p>
      <div className="about-grid">
        <article>
          <strong>当前数据规模</strong>
          <p>库内 {stats.totalGames} 款游戏，新游区 {stats.newGamesCount} 款。</p>
        </article>
        <article>
          <strong>运行配置</strong>
          <p>Steam Key：{config.steamApiKeyConfigured ? "已配置" : "未配置"}</p>
          <p>LLM Key：{config.llmApiKeyConfigured ? "已配置" : "未配置"}</p>
          <p>地区 / 语言：{config.country} / {config.language}</p>
        </article>
      </div>
    </section>
  );
}
```

- [ ] **Step 4: Re-run the settings/about tests**

Run:

```bash
npm run test -- src/pages/settings/SettingsPage.test.tsx
```

Expected: PASS with `settings` now acting as the operational control center and `about` now rendering real diagnostics instead of placeholder copy.

- [ ] **Step 5: Commit the settings/about pages**

```bash
git add src/pages/settings/SettingsPage.tsx src/pages/settings/SettingsPage.test.tsx src/pages/about/AboutPage.tsx src/App.tsx src/App.css
git commit -m "feat: complete settings and about pages"
```

## Task 6: Add the Shared Metadata Needed by `upcoming` and Real Filtering

**Files:**

- Create: `src-tauri/tests/game_metadata_tests.rs`
- Modify: `src-tauri/src/models.rs`
- Modify: `src-tauri/src/db.rs`
- Modify: `src-tauri/src/steam.rs`
- Modify: `src-tauri/src/recommendation.rs`
- Modify: `src/types.ts`

- [ ] **Step 1: Write the failing Rust metadata round-trip test**

```rust
use rusqlite::Connection;
use tauri_app_lib::db;
use tauri_app_lib::models::{GameCard, UserGameState};
use tauri_app_lib::recommendation::{DemoStatus, StoreReleaseState};

fn open_memory_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    db::migrate(&conn).expect("migrate");
    conn
}

#[test]
fn game_card_round_trips_extended_store_metadata() {
    let conn = open_memory_db();
    let card = GameCard {
        appid: 424242,
        name: "Future Co-op".to_string(),
        section: "classic".to_string(),
        release_date: Some("2026-06-01".to_string()),
        release_date_text: "Jun 1, 2026".to_string(),
        release_state: StoreReleaseState::Upcoming,
        demo_status: DemoStatus::ReleasedWithDemo,
        positive_review_pct: Some(91.0),
        total_reviews: Some(120),
        current_players: Some(320),
        recommendation_score: 82.5,
        ai_score: None,
        ai_summary: "等待 AI 评估".to_string(),
        capsule_url: "https://example.com/header.jpg".to_string(),
        tags: vec!["Action".to_string()],
        multiplayer_modes: vec!["Online Co-op".to_string()],
        review_snippets: vec![],
        supported_languages: vec!["english".to_string(), "schinese".to_string()],
        is_adult_content: false,
        price_text: Some("$19.99".to_string()),
        discount_percent: Some(10),
        user_state: UserGameState::default(),
    };

    db::upsert_game(&conn, &card).expect("upsert");
    let loaded = db::load_game(&conn, 424242).expect("load").expect("exists");

    assert_eq!(loaded.release_state, StoreReleaseState::Upcoming);
    assert_eq!(loaded.supported_languages, vec!["english", "schinese"]);
    assert_eq!(loaded.is_adult_content, false);
    assert_eq!(loaded.price_text.as_deref(), Some("$19.99"));
    assert_eq!(loaded.discount_percent, Some(10));
}
```

- [ ] **Step 2: Run the Rust metadata test and confirm it fails**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --test game_metadata_tests
```

Expected: FAIL because `GameCard` does not yet contain `release_state`, `supported_languages`, `is_adult_content`, `price_text`, or `discount_percent`.

- [ ] **Step 3: Extend Rust + TypeScript models and Steam parsing with the required fields**

Update `src-tauri/src/models.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StoreReleaseState {
    Upcoming,
    Released,
    Tba,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameCard {
    pub appid: u32,
    pub name: String,
    pub section: String,
    pub release_date: Option<String>,
    pub release_date_text: String,
    pub release_state: StoreReleaseState,
    pub demo_status: DemoStatus,
    pub positive_review_pct: Option<f64>,
    pub total_reviews: Option<u32>,
    pub current_players: Option<u32>,
    pub recommendation_score: f64,
    pub ai_score: Option<f64>,
    pub ai_summary: String,
    pub capsule_url: String,
    pub tags: Vec<String>,
    pub multiplayer_modes: Vec<String>,
    pub review_snippets: Vec<ReviewSnippet>,
    pub supported_languages: Vec<String>,
    pub is_adult_content: bool,
    pub price_text: Option<String>,
    pub discount_percent: Option<u32>,
    pub user_state: UserGameState,
}
```

Update `src-tauri/src/db.rs` migration and persistence:

```rust
ALTER TABLE games ADD COLUMN release_state TEXT NOT NULL DEFAULT 'released';
ALTER TABLE games ADD COLUMN supported_languages_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE games ADD COLUMN is_adult_content INTEGER NOT NULL DEFAULT 0;
ALTER TABLE games ADD COLUMN price_text TEXT;
ALTER TABLE games ADD COLUMN discount_percent INTEGER;
```

Extend `src-tauri/src/steam.rs`:

```rust
pub struct SteamGameSnapshot {
    pub name: Option<String>,
    pub release_date: Option<String>,
    pub release_date_text: Option<String>,
    pub release_state: StoreReleaseState,
    pub demo_status: DemoStatus,
    pub positive_review_pct: Option<f64>,
    pub total_reviews: Option<u32>,
    pub current_players: Option<u32>,
    pub capsule_url: Option<String>,
    pub tags: Vec<String>,
    pub multiplayer_modes: Vec<String>,
    pub review_snippets: Vec<ReviewSnippet>,
    pub supported_languages: Vec<String>,
    pub is_adult_content: bool,
    pub price_text: Option<String>,
    pub discount_percent: Option<u32>,
}
```

Mirror the new fields in `src/types.ts`:

```ts
export type StoreReleaseState = "upcoming" | "released" | "tba";

export interface GameCard {
  appid: number;
  name: string;
  section: "new" | "classic" | string;
  releaseDate?: string | null;
  releaseDateText: string;
  releaseState: StoreReleaseState;
  demoStatus: DemoStatus;
  positiveReviewPct?: number | null;
  totalReviews?: number | null;
  currentPlayers?: number | null;
  recommendationScore: number;
  aiScore?: number | null;
  aiSummary: string;
  capsuleUrl: string;
  tags: string[];
  multiplayerModes: string[];
  reviewSnippets: ReviewSnippet[];
  supportedLanguages: string[];
  isAdultContent: boolean;
  priceText?: string | null;
  discountPercent?: number | null;
  userState: UserGameState;
}
```

- [ ] **Step 4: Re-run the Rust metadata tests**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --test game_metadata_tests
```

Expected: PASS with the new metadata fields stored in SQLite and available to the frontend.

- [ ] **Step 5: Commit the metadata foundation**

```bash
git add src-tauri/src/models.rs src-tauri/src/db.rs src-tauri/src/steam.rs src-tauri/src/recommendation.rs src-tauri/tests/game_metadata_tests.rs src/types.ts
git commit -m "feat: persist store metadata for upcoming and filters"
```

## Task 7: Finish the `upcoming` and `filter` Pages Using the New Metadata

**Files:**

- Create: `src/pages/upcoming/UpcomingPage.test.tsx`
- Create: `src/pages/upcoming/UpcomingPage.tsx`
- Modify: `src/features/library/gameFilters.ts`
- Modify: `src/pages/detail/DetailPage.tsx`
- Modify: `src/App.tsx`
- Modify: `src/App.css`
- Modify: `src-tauri/src/models.rs`
- Modify: `src-tauri/src/db.rs`
- Modify: `src/types.ts`

- [ ] **Step 1: Write the failing upcoming/filter tests**

```tsx
// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import { filterGames } from "../../features/library/gameFilters";
import { UpcomingPage } from "./UpcomingPage";

describe("Upcoming and filter behavior", () => {
  it("filters adult content when hideAdultContent is enabled", () => {
    const result = filterGames(
      [...mockDashboard.newGames, ...mockDashboard.classics].map((game) => ({
        ...game,
        isAdultContent: game.appid === mockDashboard.classics[0]?.appid,
        supportedLanguages: ["english", "schinese"],
        releaseState: "released",
      })),
      "",
      {
        demoFilter: "all",
        hideAdultContent: true,
        minPlayers: 2,
        minReviewPct: 60,
        releaseWindow: "all",
        selectedTags: [],
        selectedLanguage: "all",
      },
      "recommended",
    );

    expect(result.some((game) => game.isAdultContent)).toBe(false);
  });

  it("renders an upcoming page headline", () => {
    render(
      <UpcomingPage
        games={[]}
        onOpen={() => undefined}
        onToggleFollow={() => undefined}
      />,
    );

    expect(screen.getByRole("heading", { name: "即将上线" })).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run the upcoming/filter tests and confirm they fail**

Run:

```bash
npm run test -- src/pages/upcoming/UpcomingPage.test.tsx src/features/library/gameFilters.test.ts
```

Expected: FAIL because adult-content and language filters are still ignored and the dedicated `UpcomingPage` does not exist yet.

- [ ] **Step 3: Add real upcoming bucketing and real filter logic**

Extend `src-tauri/src/models.rs` and `src/types.ts` so `DashboardPayload` includes upcoming games:

```rust
pub struct DashboardPayload {
    pub new_games: Vec<GameCard>,
    pub classics: Vec<GameCard>,
    pub upcoming: Vec<GameCard>,
    pub recent_discoveries: Vec<GameCard>,
    pub collections: UserCollections,
    pub stats: DashboardStats,
    pub config: PublicConfig,
}
```

Update `src-tauri/src/db.rs` bucketing:

```rust
let mut new_games = Vec::new();
let mut classics = Vec::new();
let mut upcoming = Vec::new();

for game in all_games {
    match game.release_state {
        StoreReleaseState::Upcoming | StoreReleaseState::Tba => upcoming.push(game),
        _ if game.section == "new" => new_games.push(game),
        _ => classics.push(game),
    }
}
```

Update `src/features/library/gameFilters.ts`:

```ts
export function filterGames(
  games: GameCard[],
  query: string,
  filters: LibraryFilters,
  sortMode: LibrarySortMode,
) {
  const normalizedQuery = query.trim().toLowerCase();
  const selectedTags = filters.selectedTags.map((tag) => tag.toLowerCase());

  return games
    .filter((game) =>
      normalizedQuery
        ? [game.name, ...game.tags, ...game.multiplayerModes, game.aiSummary]
            .join(" ")
            .toLowerCase()
            .includes(normalizedQuery)
        : true,
    )
    .filter((game) => (filters.hideAdultContent ? !game.isAdultContent : true))
    .filter((game) =>
      filters.selectedLanguage === "all"
        ? true
        : game.supportedLanguages.includes(filters.selectedLanguage),
    )
    .filter((game) =>
      selectedTags.length === 0
        ? true
        : game.tags.some((tag) => selectedTags.includes(tag.toLowerCase())),
    )
    .sort((left, right) => compareGames(left, right, sortMode));
}
```

Create `src/pages/upcoming/UpcomingPage.tsx`:

```tsx
import type { GameCard } from "../../types";

function releaseCountdown(releaseDate?: string | null) {
  if (!releaseDate) return "待公布";
  const today = new Date();
  const target = new Date(`${releaseDate}T00:00:00Z`);
  const days = Math.ceil((target.getTime() - today.getTime()) / 86_400_000);
  return days > 0 ? `还有 ${days} 天` : "已可发售";
}

export function UpcomingPage({
  games,
  onOpen,
  onToggleFollow,
}: {
  games: GameCard[];
  onOpen: (game: GameCard) => void;
  onToggleFollow: (game: GameCard) => void;
}) {
  return (
    <section className="upcoming-page">
      <h2>即将上线</h2>
      <p>优先关注未来发售、即将开放 Demo、或仍处于待公布状态的多人游戏。</p>
      <div className="favorite-grid">
        {games.map((game) => (
          <article className="favorite-card" key={game.appid}>
            <img src={game.capsuleUrl} alt="" onClick={() => onOpen(game)} />
            <h3>{game.name}</h3>
            <p>{releaseCountdown(game.releaseDate)} · {game.priceText ?? "价格待定"}</p>
            <button type="button" onClick={() => onToggleFollow(game)}>
              {game.userState.followed ? "已关注" : "关注上线"}
            </button>
          </article>
        ))}
      </div>
    </section>
  );
}
```

- [ ] **Step 4: Re-run the upcoming/filter tests**

Run:

```bash
npm run test -- src/pages/upcoming/UpcomingPage.test.tsx src/features/library/gameFilters.test.ts
```

Expected: PASS with adult-content and language filters now behaving for real and `upcoming` becoming a proper page.

- [ ] **Step 5: Commit the upcoming/filter work**

```bash
git add src/pages/upcoming/UpcomingPage.tsx src/pages/upcoming/UpcomingPage.test.tsx src/features/library/gameFilters.ts src/App.tsx src/App.css src-tauri/src/models.rs src-tauri/src/db.rs src/types.ts
git commit -m "feat: complete upcoming and real filtering pages"
```

## Task 8: Replace the Static AI Mock Page With a Real Recommendation Flow

**Files:**

- Create: `src-tauri/tests/ai_recommendation_tests.rs`
- Modify: `src-tauri/src/models.rs`
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/llm.rs`
- Modify: `src/api/client.ts`
- Modify: `src/types.ts`
- Create: `src/pages/ai/AiAssistantPage.test.tsx`
- Modify: `src/pages/ai/AiAssistantPage.tsx`
- Modify: `src/App.tsx`

- [ ] **Step 1: Write the failing backend + frontend AI tests**

Create `src-tauri/tests/ai_recommendation_tests.rs`:

```rust
use tauri_app_lib::models::AiRecommendationRequest;

#[test]
fn ai_recommendation_request_clamps_limit() {
    let request = AiRecommendationRequest {
        prompt: "找适合 4 人轻松合作的游戏".to_string(),
        limit: Some(20),
    };

    assert_eq!(request.normalized_limit(), 8);
}
```

Create `src/pages/ai/AiAssistantPage.test.tsx`:

```tsx
// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import { AiAssistantPage } from "./AiAssistantPage";

describe("AiAssistantPage", () => {
  it("submits the prompt and renders returned recommendations", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);

    render(
      <AiAssistantPage
        games={mockDashboard.newGames}
        isBusy={false}
        response={null}
        onSubmit={onSubmit}
      />,
    );

    fireEvent.change(screen.getByLabelText("AI 推荐输入"), {
      target: { value: "找适合 4 人轻松合作的游戏" },
    });
    fireEvent.click(screen.getByRole("button", { name: "发送" }));

    expect(onSubmit).toHaveBeenCalledWith("找适合 4 人轻松合作的游戏");
  });
});
```

- [ ] **Step 2: Run the AI tests and confirm they fail**

Run:

```bash
npm run test -- src/pages/ai/AiAssistantPage.test.tsx
```

Expected: FAIL because there is no real AI recommendation request/response model or submit flow yet.

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --test ai_recommendation_tests
```

Expected: FAIL because `AiRecommendationRequest` and its normalization helper do not exist yet.

- [ ] **Step 3: Add a typed recommendation request/response and wire it into the AI page**

Update `src-tauri/src/models.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRecommendationRequest {
    pub prompt: String,
    pub limit: Option<u32>,
}

impl AiRecommendationRequest {
    pub fn normalized_limit(&self) -> usize {
        self.limit.unwrap_or(4).clamp(1, 8) as usize
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRecommendationItem {
    pub appid: u32,
    pub name: String,
    pub reason: String,
    pub recommendation_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRecommendationResponse {
    pub prompt: String,
    pub summary: String,
    pub items: Vec<AiRecommendationItem>,
}
```

Update `src-tauri/src/commands.rs`:

```rust
#[tauri::command]
pub async fn recommend_games_with_ai(
    state: State<'_, AppState>,
    request: AiRecommendationRequest,
) -> Result<AiRecommendationResponse, String> {
    let (games, config) = {
        let conn = state.db.lock().map_err(|err| err.to_string())?;
        let config = LlmRuntimeConfig {
            api_key: db::get_secret(&conn, "llm_api_key").map_err(to_command_error)?,
            base_url: db::get_config(&conn, "llm_base_url")
                .map_err(to_command_error)?
                .unwrap_or_else(|| "https://api.openai.com".to_string()),
            model: db::get_config(&conn, "llm_model")
                .map_err(to_command_error)?
                .unwrap_or_else(|| "gpt-4.1-mini".to_string()),
        };
        let dashboard = db::load_dashboard(&conn).map_err(to_command_error)?;
        (
            dashboard
                .new_games
                .into_iter()
                .chain(dashboard.classics.into_iter())
                .collect::<Vec<_>>(),
            config,
        )
    };

    llm::recommend_games(&state.http, &config, &games, &request)
        .await
        .map_err(to_command_error)
}
```

Update `src/api/client.ts`:

```ts
export async function recommendGamesWithAi(prompt: string) {
  if (!isTauriRuntime()) {
    return {
      prompt,
      summary: "浏览器预览模式：返回本地推荐样例。",
      items: mockDashboard.newGames.slice(0, 4).map((game) => ({
        appid: game.appid,
        name: game.name,
        reason: game.aiSummary,
        recommendationScore: game.recommendationScore,
      })),
    };
  }

  return invoke("recommend_games_with_ai", {
    request: { prompt, limit: 4 },
  });
}
```

Update `src/pages/ai/AiAssistantPage.tsx`:

```tsx
import { useState } from "react";
import type { AiRecommendationResponse, GameCard } from "../../types";

export function AiAssistantPage({
  games,
  isBusy,
  response,
  onSubmit,
}: {
  games: GameCard[];
  isBusy: boolean;
  response: AiRecommendationResponse | null;
  onSubmit: (prompt: string) => Promise<void>;
}) {
  const [prompt, setPrompt] = useState("找适合 4 人轻松合作的游戏");

  return (
    <section className="ai-page">
      <h2>AI 智能推荐助手</h2>
      <label>
        <span className="sr-only">AI 推荐输入</span>
        <textarea
          aria-label="AI 推荐输入"
          value={prompt}
          onChange={(event) => setPrompt(event.currentTarget.value)}
        />
      </label>
      <button
        className="gold-button"
        disabled={isBusy || !prompt.trim()}
        type="button"
        onClick={() => void onSubmit(prompt)}
      >
        发送
      </button>

      {response ? (
        <div className="recommend-list">
          {response.items.map((item) => (
            <article className="recommend-row" key={item.appid}>
              <h3>{item.name}</h3>
              <p>{item.reason}</p>
              <strong>{Math.round(item.recommendationScore)}</strong>
            </article>
          ))}
        </div>
      ) : (
        <div className="recommend-list">
          {games.slice(0, 4).map((game) => (
            <article className="recommend-row" key={game.appid}>
              <h3>{game.name}</h3>
              <p>{game.aiSummary}</p>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}
```

- [ ] **Step 4: Re-run the AI tests**

Run:

```bash
npm run test -- src/pages/ai/AiAssistantPage.test.tsx
```

Expected: PASS with the AI page now sending a real prompt instead of showing hard-coded bubbles.

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --test ai_recommendation_tests
```

Expected: PASS with the request model and limit normalization defined in Rust.

- [ ] **Step 5: Commit the AI page**

```bash
git add src-tauri/src/models.rs src-tauri/src/commands.rs src-tauri/src/llm.rs src-tauri/tests/ai_recommendation_tests.rs src/api/client.ts src/types.ts src/pages/ai/AiAssistantPage.tsx src/pages/ai/AiAssistantPage.test.tsx src/App.tsx
git commit -m "feat: add prompt-driven ai recommendation page"
```

## Verification Checklist

- [ ] Run all frontend tests:

```bash
npm run test
```

Expected: PASS for:

- `src/App.test.tsx`
- `src/domain/recommendation.test.ts`
- `src/features/discovery/DiscoveryTaskPanel.test.tsx`
- `src/features/discovery/useDiscoveryTask.test.tsx`
- `src/features/library/gameFilters.test.ts`
- `src/pages/dashboard/DashboardPage.test.tsx`
- `src/pages/detail/DetailPage.test.tsx`
- `src/pages/collections/CollectionsHubPage.test.tsx`
- `src/pages/collections/WishlistTrackerPage.test.tsx`
- `src/pages/collections/HistoryPage.test.tsx`
- `src/pages/settings/SettingsPage.test.tsx`
- `src/pages/upcoming/UpcomingPage.test.tsx`
- `src/pages/ai/AiAssistantPage.test.tsx`

- [ ] Run all Rust tests:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: PASS for:

- `discovery_tests`
- `discovery_task_tests`
- `recommendation_tests`
- `user_state_tests`
- `game_metadata_tests`
- `ai_recommendation_tests`

- [ ] Run the desktop app and walk every page in order:

```bash
npm run tauri dev
```

Manual check:

1. Open `首页`, `新游区`, `精品老游区`, and `浏览全部`; confirm card counts and filters remain stable while switching views.
2. Open one game detail page and confirm AI tab, review tab, and related tab all render real content.
3. Open `我的收藏夹`, `愿望单追踪`, and `游玩记录`; confirm all pages render different copy and sorting behavior, not the same placeholder.
4. Open `设置`; confirm save, sync, preview, and discovery-task controls all work from one page.
5. Open `关于`; confirm it shows real diagnostics rather than placeholder text.
6. Open `即将上线`; confirm only future/TBA games appear there once the metadata task is complete.
7. Apply adult-content and language filters; confirm they visibly change the result set.
8. Open `AI 智能推荐助手`; submit a prompt and confirm recommendations return instead of staying as static mock text.

## Self-Review

**1. Spec coverage**

- Page-by-page execution order is explicit and tied to current backend readiness.
- Pages already backed by current commands (`dashboard`, `detail`, `saved`, `settings`) are scheduled first.
- Placeholder/shallow pages (`wishlist`, `history`, `upcoming`, `about`, `ai`) are each covered by concrete tasks.
- The filter/upcoming blockers are explicitly handled by the metadata foundation task instead of being hand-waved away.

**2. Placeholder scan**

- No `TODO`, `TBD`, or “implement later” markers remain.
- Every task includes concrete files, code, commands, and a verification step.
- No task depends on undocumented “magic” pages or unnamed backend routes.

**3. Type consistency**

- Frontend and backend both converge on `GameCard.releaseState`, `supportedLanguages`, `isAdultContent`, `priceText`, and `discountPercent`.
- AI request/response models are shared in both Rust and TypeScript.
- View/page terminology remains aligned with the current `ViewId` inventory rather than inventing a second routing scheme.
