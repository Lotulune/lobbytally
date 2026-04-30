import { useEffect, useMemo, useState } from "react";
import {
  assessGameWithAi,
  getDashboard,
  saveConfig,
  setGameUserState,
  syncSeedGames,
} from "./api/client";
import { buildDashboardSections, filterGames } from "./features/library/gameFilters";
import { AiAssistantPage } from "./pages/ai/AiAssistantPage";
import { AboutPage } from "./pages/about/AboutPage";
import { CollectionsHubPage } from "./pages/collections/CollectionsHubPage";
import { HistoryPage } from "./pages/collections/HistoryPage";
import { WishlistTrackerPage } from "./pages/collections/WishlistTrackerPage";
import { DashboardPage } from "./pages/dashboard/DashboardPage";
import { DetailPage } from "./pages/detail/DetailPage";
import { FilterPage } from "./pages/filter/FilterPage";
import { SettingsPage } from "./pages/settings/SettingsPage";
import { UpcomingPage } from "./pages/upcoming/UpcomingPage";
import type { LibraryFilters, ViewId, LibrarySortMode } from "./pages/types";
import type {
  AiAssessment,
  DashboardPayload,
  GameCard,
  SaveConfigRequest,
  SyncMode,
  UserGameStatePatch,
} from "./types";
import "./App.css";

const defaultTagOptions = ["合作", "独立", "像素风格", "解谜", "轻松"];
const DEFAULT_MIN_PLAYERS = 0;
const DEFAULT_MIN_REVIEW_PCT = 0;
const BACKFILL_REFRESH_INTERVAL_MS = 2_000;

const navPrimary: Array<{ id: ViewId; label: string; icon: string; badge?: string }> = [
  { id: "home", label: "首页", icon: "⌂" },
  { id: "new", label: "新游区", icon: "⊕", badge: "NEW" },
  { id: "classic", label: "精品老游区", icon: "☆" },
  { id: "upcoming", label: "即将上线", icon: "◷" },
  { id: "wishlist", label: "愿望单追踪", icon: "♡" },
  { id: "browse", label: "浏览全部", icon: "⌘" },
];

const navUtility: Array<{ id: ViewId; label: string; icon: string }> = [
  { id: "filter", label: "筛选器", icon: "▽" },
  { id: "saved", label: "收藏夹", icon: "▱" },
  { id: "history", label: "游玩记录", icon: "↺" },
  { id: "settings", label: "设置", icon: "⚙" },
  { id: "about", label: "关于", icon: "ⓘ" },
];

const DEFAULT_FILTERS: LibraryFilters = {
  demoFilter: "all",
  hideAdultContent: true,
  minPlayers: DEFAULT_MIN_PLAYERS,
  minReviewPct: DEFAULT_MIN_REVIEW_PCT,
  releaseWindow: "all",
  selectedTags: [],
  selectedLanguage: "all",
};

function App() {
  const [dashboard, setDashboard] = useState<DashboardPayload | null>(null);
  const [activeView, setActiveView] = useState<ViewId>("home");
  const [selectedGame, setSelectedGame] = useState<GameCard | null>(null);
  const [query, setQuery] = useState("");
  const [sortMode, setSortMode] = useState<LibrarySortMode>("recommended");
  const [filters, setFilters] = useState<LibraryFilters>(DEFAULT_FILTERS);
  const [status, setStatus] = useState("正在打开 Co-Play 多人游戏雷达……");
  const [isBusy, setIsBusy] = useState(false);
  const [assessment, setAssessment] = useState<AiAssessment | null>(null);

  useEffect(() => {
    void loadDashboard();
  }, []);

  useEffect(() => {
    if (!dashboard) {
      return;
    }

    if (
      !dashboard.stats.syncRunning &&
      !dashboard.stats.backfillRunning &&
      dashboard.stats.backfillPendingCount === 0
    ) {
      return;
    }

    let isDisposed = false;
    let timer: number | null = null;

    const poll = async () => {
      try {
        await refreshDashboard();
      } catch (error) {
        if (!isDisposed) {
          setStatus(error instanceof Error ? error.message : String(error));
        }
      } finally {
        if (!isDisposed) {
          timer = window.setTimeout(poll, BACKFILL_REFRESH_INTERVAL_MS);
        }
      }
    };

    timer = window.setTimeout(poll, BACKFILL_REFRESH_INTERVAL_MS);

    return () => {
      isDisposed = true;
      if (timer != null) {
        window.clearTimeout(timer);
      }
    };
  }, [dashboard, refreshDashboard]);

  async function refreshDashboard() {
    const payload = await getDashboard();
    setDashboard(payload);
    setSelectedGame(
      (current) =>
        current ??
        payload.upcoming[0] ??
        payload.newGames[0] ??
        payload.classics[0] ??
        null,
    );
    return payload;
  }

  async function loadDashboard() {
    setIsBusy(true);
    try {
      const payload = await refreshDashboard();
      setStatus(payload.stats.dataSource);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setIsBusy(false);
    }
  }

  async function handleSync(mode: SyncMode) {
    setIsBusy(true);
    setStatus(
      mode === "full"
        ? "正在完整同步 Steam 评论、在线人数、商店图与评测样本……"
        : "正在快速同步 Steam 商店图、价格与发售信息……",
    );
    try {
      const report = await syncSeedGames(mode);
      setStatus(report.message);
      await refreshDashboard();
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setIsBusy(false);
    }
  }

  async function handleAiAssess(game: GameCard) {
    setIsBusy(true);
    setStatus(`正在让 AI 评估《${game.name}》……`);
    try {
      const nextAssessment = await assessGameWithAi(game.appid);
      setAssessment(nextAssessment);
      setStatus(`AI：${nextAssessment.summary}`);
      await loadDashboard();
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setIsBusy(false);
    }
  }

  async function handleUserState(
    appid: number,
    patch: UserGameStatePatch,
    message: string,
  ) {
    setIsBusy(true);
    try {
      await setGameUserState(appid, patch);
      await loadDashboard();
      setSelectedGame((current) =>
        current?.appid === appid
          ? {
              ...current,
              userState: {
                ...current.userState,
                ...patch,
                updatedAt: new Date().toISOString(),
              },
            }
          : current,
      );
      setStatus(message);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setIsBusy(false);
    }
  }

  async function handleSaveConfig(request: SaveConfigRequest) {
    setIsBusy(true);
    try {
      const config = await saveConfig(request);
      setDashboard((current) => (current ? { ...current, config } : current));
      setStatus("设置已保存。Steam Key 和 LLM Key 仅保存在本地 SQLite。");
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setIsBusy(false);
    }
  }

  function openDetail(game: GameCard) {
    setSelectedGame(game);
    setActiveView("detail");
    void handleUserState(game.appid, { viewed: true }, `已打开《${game.name}》详情。`);
  }

  function resetFilters() {
    setFilters({ ...DEFAULT_FILTERS });
  }

  function setDemoFilter(demoFilter: LibraryFilters["demoFilter"]) {
    setFilters((current) => ({ ...current, demoFilter }));
  }

  function setMinPlayers(minPlayers: number) {
    setFilters((current) => ({ ...current, minPlayers }));
  }

  function setMinReviewPct(minReviewPct: number) {
    setFilters((current) => ({ ...current, minReviewPct }));
  }

  function setReleaseWindow(releaseWindow: LibraryFilters["releaseWindow"]) {
    setFilters((current) => ({ ...current, releaseWindow }));
  }

  function toggleHideAdultContent() {
    setFilters((current) => ({
      ...current,
      hideAdultContent: !current.hideAdultContent,
    }));
  }

  function toggleQuickTag(tag: string) {
    setFilters((current) => ({
      ...current,
      selectedTags: current.selectedTags.includes(tag)
        ? current.selectedTags.filter((item) => item !== tag)
        : [...current.selectedTags, tag],
    }));
  }

  const allGames = useMemo(
    () => [
      ...(dashboard?.upcoming ?? []),
      ...(dashboard?.newGames ?? []),
      ...(dashboard?.classics ?? []),
    ],
    [dashboard],
  );

  const availableTags = useMemo(() => {
    const seen = new Set<string>();
    const ordered: string[] = [];

    for (const game of allGames) {
      for (const tag of game.tags) {
        if (!seen.has(tag)) {
          seen.add(tag);
          ordered.push(tag);
        }
      }
    }

    return ordered;
  }, [allGames]);

  const quickTagOptions = useMemo(() => {
    const seedTags = defaultTagOptions.filter((tag) => availableTags.includes(tag));
    const extras = availableTags.filter((tag) => !seedTags.includes(tag));
    return [...seedTags, ...extras].slice(0, 5);
  }, [availableTags]);

  const sections = useMemo(
    () =>
      dashboard
        ? buildDashboardSections({
            activeView,
            dashboard,
            filters,
            query,
            sortMode,
          })
        : [],
    [activeView, dashboard, filters, query, sortMode],
  );

  const visibleNewGames = useMemo(
    () => filterGames(dashboard?.newGames ?? [], query, filters, sortMode),
    [dashboard?.newGames, filters, query, sortMode],
  );

  const visibleClassics = useMemo(
    () => filterGames(dashboard?.classics ?? [], query, filters, sortMode),
    [dashboard?.classics, filters, query, sortMode],
  );

  const visibleUpcomingGames = useMemo(
    () => filterGames(dashboard?.upcoming ?? [], query, filters, sortMode),
    [dashboard?.upcoming, filters, query, sortMode],
  );

  if (!dashboard) {
    return (
      <main className="loading-shell">
        <div className="loading-card">
          <LogoMark />
          <h1>正在整理多人游戏目录</h1>
          <p>{status}</p>
        </div>
      </main>
    );
  }

  const showDashboardRail = ["home", "new", "classic", "browse"].includes(activeView);

  return (
    <main className="coplay-shell">
      <Sidebar activeView={activeView} onNavigate={setActiveView} />

      <section className="page-surface">
        <TopBar
          activeView={activeView}
          query={query}
          selectedGame={selectedGame}
          setQuery={setQuery}
          onDetail={() => setActiveView("detail")}
        />

        {activeView === "filter" && (
          <FilterPage
            availableTags={availableTags}
            defaultFilters={DEFAULT_FILTERS}
            defaultTagOptions={defaultTagOptions}
            filters={filters}
            onApply={(nextFilters) => {
              setFilters(nextFilters);
              setActiveView("home");
            }}
            onCancel={() => setActiveView("home")}
          />
        )}

        {activeView === "detail" && selectedGame && (
          <DetailPage
            game={selectedGame}
            isBusy={isBusy}
            onAiAssess={() => handleAiAssess(selectedGame)}
            onBack={() => setActiveView("home")}
            onToggleState={(patch, message) =>
              handleUserState(selectedGame.appid, patch, message)
            }
            relatedGames={allGames.filter((game) => game.appid !== selectedGame.appid)}
          />
        )}

        {activeView === "ai" && (
          <AiAssistantPage
            assessment={assessment}
            games={[...visibleNewGames, ...visibleClassics].slice(0, 4)}
            isBusy={isBusy}
            onAssess={(game) => {
              setSelectedGame(game);
              void handleAiAssess(game);
            }}
            selectedGame={selectedGame}
          />
        )}

        {activeView === "saved" && (
          <CollectionsHubPage
            collections={dashboard.collections}
            onOpen={openDetail}
            onToggle={(game, patch, message) =>
              handleUserState(game.appid, patch, message)
            }
          />
        )}

        {activeView === "wishlist" && (
          <WishlistTrackerPage
            games={dashboard.collections.wishlist}
            onOpen={openDetail}
            onToggle={(game, patch, message) =>
              handleUserState(game.appid, patch, message)
            }
          />
        )}

        {activeView === "upcoming" && (
          <UpcomingPage
            games={visibleUpcomingGames}
            onOpen={openDetail}
            onToggleFollow={(game) =>
              void handleUserState(
                game.appid,
                { followed: !game.userState.followed },
                game.userState.followed
                  ? `已取消关注《${game.name}》。`
                  : `已关注《${game.name}》的上线动态。`,
              )
            }
          />
        )}

        {activeView === "history" && (
          <HistoryPage
            games={dashboard.collections.history}
            onOpen={openDetail}
          />
        )}

        {activeView === "settings" && (
          <SettingsPage
            config={dashboard.config}
            isBusy={isBusy}
            onRefreshDashboard={refreshDashboard}
            onSave={handleSaveConfig}
            onStatus={setStatus}
            onSync={handleSync}
            status={status}
            stats={dashboard.stats}
          />
        )}

        {activeView === "about" && (
          <AboutPage
            config={dashboard.config}
            stats={dashboard.stats}
          />
        )}

        {showDashboardRail && (
          <DashboardPage
            activeView={activeView}
            filters={filters}
            isBusy={isBusy}
            onAi={() => setActiveView("ai")}
            onChangeView={setActiveView}
            onOpenFilters={() => setActiveView("filter")}
            onOpenGame={openDetail}
            onResetFilters={resetFilters}
            onSetDemoFilter={setDemoFilter}
            onSetMinPlayers={setMinPlayers}
            onSetMinReviewPct={setMinReviewPct}
            onSetReleaseWindow={setReleaseWindow}
            onSetSortMode={setSortMode}
            onToggleHideAdultContent={toggleHideAdultContent}
            onToggleQuickTag={toggleQuickTag}
            onSync={handleSync}
            quickTags={quickTagOptions}
            sections={sections}
            selectedAppid={selectedGame?.appid}
            sortMode={sortMode}
            stats={dashboard.stats}
            status={status}
          />
        )}
      </section>
    </main>
  );
}

function Sidebar({
  activeView,
  onNavigate,
}: {
  activeView: ViewId;
  onNavigate: (view: ViewId) => void;
}) {
  return (
    <aside className="coplay-sidebar">
      <div className="brand-row">
        <LogoMark />
        <div>
          <strong>Co-Play</strong>
          <span>发现好玩的多人游戏</span>
        </div>
      </div>

      <nav className="sidebar-nav" aria-label="主导航">
        {navPrimary.map((item) => (
          <NavButton
            active={activeView === item.id}
            item={item}
            key={item.id}
            onClick={() => onNavigate(item.id)}
          />
        ))}
      </nav>

      <div className="nav-divider" />

      <nav className="sidebar-nav" aria-label="工具导航">
        {navUtility.map((item) => (
          <NavButton
            active={activeView === item.id}
            item={item}
            key={item.id}
            onClick={() => onNavigate(item.id)}
          />
        ))}
      </nav>

      <div className="invite-card">
        <p>和朋友一起发现好游戏！</p>
        <div className="avatar-stack">
          <span>🧑</span>
          <span>👩</span>
          <span>👧</span>
          <button type="button">＋</button>
        </div>
        <button className="gold-button" type="button">
          邀请好友
        </button>
      </div>
    </aside>
  );
}

function NavButton({
  active,
  item,
  onClick,
}: {
  active: boolean;
  item: { label: string; icon: string; badge?: string };
  onClick: () => void;
}) {
  return (
    <button className={active ? "side-link active" : "side-link"} onClick={onClick} type="button">
      <span aria-hidden="true" className="side-icon">
        {item.icon}
      </span>
      <span>{item.label}</span>
      {item.badge && <em aria-hidden="true">{item.badge}</em>}
    </button>
  );
}

function LogoMark() {
  return (
    <span className="logo-mark" aria-hidden="true">
      <i />
      <i />
      <b />
    </span>
  );
}

function TopBar({
  activeView,
  query,
  setQuery,
  selectedGame,
  onDetail,
}: {
  activeView: ViewId;
  query: string;
  setQuery: (value: string) => void;
  selectedGame: GameCard | null;
  onDetail: () => void;
}) {
  const title =
    activeView === "filter"
      ? "筛选器"
      : activeView === "detail"
        ? selectedGame?.name ?? "游戏详情"
      : activeView === "ai"
          ? "AI 智能推荐助手"
          : activeView === "upcoming"
            ? "即将上线"
          : activeView === "saved"
            ? "我的收藏夹"
            : activeView === "wishlist"
              ? "愿望单追踪"
              : activeView === "history"
                ? "游玩记录"
                : activeView === "settings"
                  ? "设置"
                  : activeView === "about"
                    ? "关于 Co-Play"
                    : "为你发现值得一玩的多人游戏";

  return (
    <header className="coplay-topbar">
      <div className="title-cluster">
        <span className="people-icon">♟</span>
        <h1>{title}</h1>
      </div>

      <div className="top-actions">
        <label className="search-pill">
          <input
            value={query}
            onChange={(event) => setQuery(event.currentTarget.value)}
            placeholder="搜索游戏名称、类型、标签..."
          />
          <span>⌕</span>
        </label>
        <button className="icon-button" type="button" aria-label="通知">
          ♧
        </button>
        <button className="profile-button" type="button" onClick={onDetail}>
          <span>👩🏻</span>
          <b>⌄</b>
        </button>
      </div>
    </header>
  );
}

export default App;
