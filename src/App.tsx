import { useEffect, useMemo, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { ScrollDock } from "./components/ScrollDock";
import {
  TaskToastLayer,
  type TaskToastItem,
  type TaskToastTone,
} from "./components/TaskToastLayer";
import {
  applyGameAnalysisSnapshotToDashboard,
  applyGameAnalysisSnapshotToGame,
  snapshotFromReport,
  type GameAnalysisSnapshot,
} from "./features/library/gameDashboardState";
import {
  assessGameWithAi,
  getDashboard,
  getDiscoveryTaskSnapshot,
  isTauriRuntime,
  recommendGamesWithAi,
  refreshAllGameAnalyses,
  retryAiAnalysisJob,
  saveConfig,
  setGameUserState,
  startClassicDiscoveryTask,
  syncSeedGames,
} from "./api/client";
import { buildDashboardSections, filterGames } from "./features/library/gameFilters";
import {
  AiAssistantPage,
  INITIAL_AI_MESSAGES,
} from "./pages/ai/AiAssistantPage";
import { AboutPage } from "./pages/about/AboutPage";
import { CollectionsHubPage } from "./pages/collections/CollectionsHubPage";
import { HistoryPage } from "./pages/collections/HistoryPage";
import { WishlistTrackerPage } from "./pages/collections/WishlistTrackerPage";
import {
  DashboardPage,
  type DashboardSectionPageState,
} from "./pages/dashboard/DashboardPage";
import { DetailPage } from "./pages/detail/DetailPage";
import { FilterPage } from "./pages/filter/FilterPage";
import { OnboardingWizard } from "./pages/onboarding/OnboardingWizard";
import {
  defaultSettingsExpandedState,
  SettingsPage,
  type SettingsExpandedState,
} from "./pages/settings/SettingsPage";
import { UpcomingPage } from "./pages/upcoming/UpcomingPage";
import type { LibraryFilters, ViewId, LibrarySortMode } from "./pages/types";
import type {
  AiAssessment,
  AiRecommendationMessage,
  AiRecommendationRequest,
  AiRecommendationResponse,
  DashboardStats,
  DashboardPayload,
  DiscoveryRunSnapshot,
  DiscoveryRunStatus,
  GameCard,
  SaveConfigRequest,
  SyncMode,
  UserGameStatePatch,
} from "./types";
import "./App.css";
import { listen } from "@tauri-apps/api/event";

const defaultTagOptions = ["合作", "独立", "像素风格", "解谜", "轻松"];
const DEFAULT_MIN_PLAYERS = 0;
const DEFAULT_MIN_REVIEW_PCT = 0;
const DASHBOARD_POLL_INTERVAL_MS = 2_000;
const DISCOVERY_TASK_EVENT = "discovery-task-updated";
const MAX_VISIBLE_TASK_TOASTS = 4;

type AiConversation = {
  id: number;
  title: string;
  messages: AiRecommendationMessage[];
  recommendation: AiRecommendationResponse | null;
  updatedAt: number;
};

type AppMode =
  | { type: "main" }
  | { type: "onboarding"; source: "auto" | "settings" };

function createAiConversation(id: number): AiConversation {
  return {
    id,
    title: "新对话",
    messages: INITIAL_AI_MESSAGES,
    recommendation: null,
    updatedAt: Date.now(),
  };
}

function getAiConversationTitle(messages: AiRecommendationMessage[]) {
  const prompt = messages.find((message) => message.role === "user")?.content.trim();

  if (!prompt) {
    return "新对话";
  }

  return prompt.length > 18 ? `${prompt.slice(0, 18)}...` : prompt;
}

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

const DEFAULT_SECTION_PAGES: DashboardSectionPageState = {
  new: 1,
  classic: 1,
  recent: 1,
};

function hasIndependentScrollContainer(container: HTMLElement | null) {
  if (container == null) {
    return false;
  }

  const overflowY = window.getComputedStyle(container).overflowY;
  return overflowY === "auto" || overflowY === "scroll" || overflowY === "overlay";
}

function getPageScrollTop(container: HTMLElement | null) {
  if (container != null && hasIndependentScrollContainer(container)) {
    return container.scrollTop;
  }

  return window.scrollY || document.documentElement.scrollTop || document.body.scrollTop || 0;
}

function setPageScrollTop(container: HTMLElement | null, top: number) {
  const safeTop = Math.max(0, top);

  if (container != null && hasIndependentScrollContainer(container)) {
    container.scrollTop = safeTop;
    return;
  }

  window.scrollTo({ top: safeTop, behavior: "auto" });
}

function App() {
  const [dashboard, setDashboard] = useState<DashboardPayload | null>(null);
  const [appMode, setAppMode] = useState<AppMode>({ type: "main" });
  const [activeView, setActiveView] = useState<ViewId>("home");
  const [selectedGame, setSelectedGame] = useState<GameCard | null>(null);
  const [query, setQuery] = useState("");
  const [sortMode, setSortMode] = useState<LibrarySortMode>("recommended");
  const [filters, setFilters] = useState<LibraryFilters>(DEFAULT_FILTERS);
  const [status, setStatus] = useState("正在打开 Co-Play 多人游戏雷达……");
  const [isBusy, setIsBusy] = useState(false);
  const [assessment, setAssessment] = useState<AiAssessment | null>(null);
  const aiConversationIdRef = useRef(1);
  const [aiConversations, setAiConversations] = useState<AiConversation[]>(() => [
    createAiConversation(1),
  ]);
  const [activeAiConversationId, setActiveAiConversationId] = useState(1);
  const [taskToasts, setTaskToasts] = useState<TaskToastItem[]>([]);
  const [sectionPages, setSectionPages] =
    useState<DashboardSectionPageState>(DEFAULT_SECTION_PAGES);
  const [settingsExpandedSections, setSettingsExpandedSections] =
    useState<SettingsExpandedState>(defaultSettingsExpandedState);
  const [discoveryTaskRunning, setDiscoveryTaskRunning] = useState(false);
  const mountedRef = useRef(true);
  const dashboardRequestIdRef = useRef(0);
  const autoOnboardingDismissedRef = useRef(false);
  const detailReturnViewRef = useRef<ViewId>("home");
  const pageSurfaceRef = useRef<HTMLElement | null>(null);
  const viewScrollPositionsRef = useRef<Partial<Record<ViewId, number>>>({});
  const pendingScrollRestoreRef = useRef<{ view: ViewId; top: number } | null>(null);
  const pendingAnalysisSnapshotsRef = useRef(new Map<number, GameAnalysisSnapshot>());
  const latestDiscoverySnapshotRef = useRef<DiscoveryRunSnapshot | null>(null);
  const latestStatsRef = useRef<DashboardStats | null>(null);
  const taskToastIdRef = useRef(0);

  useEffect(() => {
    void loadDashboard();
  }, []);

  useEffect(() => {
    void getDiscoveryTaskSnapshot()
      .then((snapshot) => {
        latestDiscoverySnapshotRef.current = snapshot;
        setDiscoveryTaskRunning(snapshot?.status === "running");
      })
      .catch(() => {
        setDiscoveryTaskRunning(false);
      });
  }, []);

  useEffect(() => {
    mountedRef.current = true;

    return () => {
      mountedRef.current = false;
      dashboardRequestIdRef.current += 1;
    };
  }, []);

  useEffect(() => {
    const pendingRestore = pendingScrollRestoreRef.current;
    if (!pendingRestore || pendingRestore.view !== activeView) {
      return;
    }

    pendingScrollRestoreRef.current = null;
    const rafId = window.requestAnimationFrame(() => {
      setPageScrollTop(pageSurfaceRef.current, pendingRestore.top);
    });

    return () => {
      window.cancelAnimationFrame(rafId);
    };
  }, [activeView, dashboard, sectionPages]);

  useEffect(() => {
    if (!dashboard) {
      return;
    }

    if (
      !discoveryTaskRunning &&
      !dashboard.stats.classicDiscoveryRunning &&
      !dashboard.stats.syncRunning &&
      !dashboard.stats.backfillRunning &&
      dashboard.stats.backfillPendingCount === 0 &&
      !dashboard.stats.aiBatchRefreshRunning
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
          timer = window.setTimeout(poll, DASHBOARD_POLL_INTERVAL_MS);
        }
      }
    };

    timer = window.setTimeout(poll, DASHBOARD_POLL_INTERVAL_MS);

    return () => {
      isDisposed = true;
      if (timer != null) {
        window.clearTimeout(timer);
      }
    };
  }, [dashboard, discoveryTaskRunning, refreshDashboard]);

  useEffect(() => {
    if (!isTauriRuntime()) {
      return;
    }

    let isDisposed = false;
    let unlisten: (() => void) | null = null;

    void listen<DiscoveryRunSnapshot>(DISCOVERY_TASK_EVENT, ({ payload }) => {
      if (isDisposed) {
        return;
      }

      const previousSnapshot = latestDiscoverySnapshotRef.current;
      latestDiscoverySnapshotRef.current = payload;
      const running = payload.status === "running";
      setDiscoveryTaskRunning(running);
      maybeNotifyDiscoveryCompletion(previousSnapshot, payload, enqueueTaskToast);
      if (!running) {
        void refreshDashboard().catch((error) => {
          setStatus(error instanceof Error ? error.message : String(error));
        });
      }
    }).then((cleanup) => {
      if (isDisposed) {
        cleanup();
        return;
      }
      unlisten = cleanup;
    });

    return () => {
      isDisposed = true;
      unlisten?.();
    };
  }, []);

  async function refreshDashboard(requestId = beginDashboardRequest()) {
    const payload = await getDashboard();
    if (!isDashboardRequestCurrent(requestId)) {
      return null;
    }

    maybeNotifyStatsTransitions(latestStatsRef.current, payload.stats, enqueueTaskToast);
    latestStatsRef.current = payload.stats;
    const nextPayload = applyPendingGameAnalysisSnapshots(payload);
    const latestGames = [
      ...nextPayload.upcoming,
      ...nextPayload.newGames,
      ...nextPayload.classics,
      ...nextPayload.hiddenGames,
    ];
    setDashboard(nextPayload);
    setAppMode((current) => {
      if (nextPayload.config.onboardingCompleted) {
        autoOnboardingDismissedRef.current = false;
        return current;
      }
      if (current.type === "onboarding") {
        return current;
      }
      if (isTauriRuntime() && !autoOnboardingDismissedRef.current) {
        return { type: "onboarding", source: "auto" };
      }
      return current;
    });
    setSelectedGame(
      (current) =>
        latestGames.find((game) => game.appid === current?.appid) ??
        latestGames[0] ??
        null,
    );
    return payload;
  }

  async function loadDashboard(manageBusyState = true) {
    const requestId = beginDashboardRequest();
    if (manageBusyState) {
      setIsBusy(true);
    }
    try {
      const payload = await refreshDashboard(requestId);
      if (payload) {
        setStatus(payload.stats.dataSource);
      }
    } catch (error) {
      if (isDashboardRequestCurrent(requestId)) {
        setStatus(error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (manageBusyState && isDashboardRequestCurrent(requestId)) {
        setIsBusy(false);
      }
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
      commitGameAnalysisSnapshot({
        appid: game.appid,
        aiScore: nextAssessment.score,
        aiSummary: nextAssessment.summary,
      });
      setAssessment(nextAssessment);
      setStatus(`AI：${nextAssessment.summary}`);
      await loadDashboard(false);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setIsBusy(false);
    }
  }

  async function handleAiRecommend(
    request: AiRecommendationRequest,
  ): Promise<AiRecommendationResponse> {
    setIsBusy(true);
    setStatus("正在从已入库游戏里匹配你的需求……");
    try {
      const response = await recommendGamesWithAi(request);
      setStatus(response.reply);
      return response;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setStatus(message);
      throw error;
    } finally {
      setIsBusy(false);
    }
  }

  function handleAiMessagesChange(messages: AiRecommendationMessage[]) {
    setAiConversations((current) =>
      current.map((conversation) =>
        conversation.id === activeAiConversationId
          ? {
              ...conversation,
              messages,
              title: getAiConversationTitle(messages),
              updatedAt: Date.now(),
            }
          : conversation,
      ),
    );
  }

  function handleAiRecommendationChange(
    recommendation: AiRecommendationResponse | null,
  ) {
    setAiConversations((current) =>
      current.map((conversation) =>
        conversation.id === activeAiConversationId
          ? {
              ...conversation,
              recommendation,
              updatedAt: Date.now(),
            }
          : conversation,
      ),
    );
  }

  function handleNewAiConversation() {
    aiConversationIdRef.current += 1;
    const nextConversation = createAiConversation(aiConversationIdRef.current);
    setAiConversations((current) => [nextConversation, ...current]);
    setActiveAiConversationId(nextConversation.id);
    setAssessment(null);
  }

  function handleSelectAiConversation(id: number) {
    setActiveAiConversationId(id);
    setAssessment(null);
  }

  async function handleRefreshAllAnalyses(concurrency: number) {
    setIsBusy(true);
    setStatus(`正在按 ${concurrency} 路并发批量重算库内游戏的 AI 评分……`);
    try {
      const report = await refreshAllGameAnalyses(concurrency);
      await refreshDashboard();
      setStatus(report.message);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setIsBusy(false);
    }
  }

  async function handleRetryAiAnalysisJob(appid: number) {
    setIsBusy(true);
    setStatus(`正在重试 AppID ${appid} 的 AI 分析任务……`);
    try {
      const report = await retryAiAnalysisJob(appid);
      await refreshDashboard();
      setStatus(report.message);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    } finally {
      setIsBusy(false);
    }
  }

  async function handleStartClassicDiscovery(maxPages: number) {
    setIsBusy(true);
    setStatus(`正在启动精品老游补库，按评论榜最多扫描 ${maxPages} 页……`);
    try {
      const snapshot = await startClassicDiscoveryTask(maxPages);
      await refreshDashboard();
      setStatus(
        `已启动精品老游补库：任务 #${snapshot.id}，本轮最多扫描 ${snapshot.maxPages} 页，每页 ${snapshot.pageSize} 个候选。`,
      );
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
      await loadDashboard(false);
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

  async function handleExitOnboarding(target: "app" | "settings") {
    autoOnboardingDismissedRef.current = true;
    await refreshDashboard();
    if (target === "settings") {
      setActiveView("settings");
    }
    setAppMode({ type: "main" });
  }

  function handleOpenOnboardingFromSettings() {
    setAppMode({ type: "onboarding", source: "settings" });
  }

  function syncGameAnalysisLocally(snapshot: GameAnalysisSnapshot) {
    pendingAnalysisSnapshotsRef.current.set(snapshot.appid, snapshot);
    setDashboard((current) =>
      current ? applyGameAnalysisSnapshotToDashboard(current, snapshot) : current,
    );
    setSelectedGame((current) =>
      current ? applyGameAnalysisSnapshotToGame(current, snapshot) : current,
    );
  }

  function applyPendingGameAnalysisSnapshots(payload: DashboardPayload) {
    let nextPayload = payload;

    for (const snapshot of pendingAnalysisSnapshotsRef.current.values()) {
      if (dashboardContainsSnapshot(nextPayload, snapshot)) {
        pendingAnalysisSnapshotsRef.current.delete(snapshot.appid);
        continue;
      }

      nextPayload = applyGameAnalysisSnapshotToDashboard(nextPayload, snapshot);
    }

    return nextPayload;
  }

  function commitGameAnalysisSnapshot(snapshot: GameAnalysisSnapshot) {
    flushSync(() => {
      syncGameAnalysisLocally(snapshot);
    });
  }

  function beginDashboardRequest() {
    const requestId = dashboardRequestIdRef.current + 1;
    dashboardRequestIdRef.current = requestId;
    return requestId;
  }

  function enqueueTaskToast(
    tone: TaskToastTone,
    title: string,
    message: string,
  ) {
    taskToastIdRef.current += 1;
    const toast: TaskToastItem = {
      id: taskToastIdRef.current,
      tone,
      title,
      message,
    };
    setTaskToasts((current) => [toast, ...current].slice(0, MAX_VISIBLE_TASK_TOASTS));
  }

  function dismissTaskToast(id: number) {
    setTaskToasts((current) => current.filter((toast) => toast.id !== id));
  }

  function isDashboardRequestCurrent(requestId: number) {
    return mountedRef.current && dashboardRequestIdRef.current === requestId;
  }

  function dashboardContainsSnapshot(
    payload: DashboardPayload,
    snapshot: GameAnalysisSnapshot,
  ) {
    const matchingGames = [
      ...payload.newGames,
      ...payload.classics,
      ...payload.hiddenGames,
      ...payload.upcoming,
      ...payload.recentDiscoveries,
      ...payload.collections.favorites,
      ...payload.collections.wishlist,
      ...payload.collections.followed,
      ...payload.collections.history,
    ].filter((game) => game.appid === snapshot.appid);

    return (
      matchingGames.length > 0 &&
      matchingGames.every(
        (game) =>
          game.aiScore === snapshot.aiScore &&
          game.aiSummary === snapshot.aiSummary,
      )
    );
  }

  function rememberCurrentViewScroll(view: ViewId) {
    viewScrollPositionsRef.current[view] = getPageScrollTop(pageSurfaceRef.current);
  }

  function scrollCurrentViewToTop() {
    setPageScrollTop(pageSurfaceRef.current, 0);
  }

  function openDetail(game: GameCard) {
    setSelectedGame(game);
    rememberCurrentViewScroll(activeView);
    detailReturnViewRef.current = activeView;
    setActiveView("detail");
    scrollCurrentViewToTop();
    void handleUserState(game.appid, { viewed: true }, `已打开《${game.name}》详情。`);
  }

  function returnFromDetail() {
    const nextView =
      detailReturnViewRef.current === "detail" ? "home" : detailReturnViewRef.current;
    pendingScrollRestoreRef.current = {
      view: nextView,
      top: viewScrollPositionsRef.current[nextView] ?? 0,
    };
    setActiveView(nextView);
  }

  function resetFilters() {
    setFilters({ ...DEFAULT_FILTERS });
    setSectionPages(DEFAULT_SECTION_PAGES);
  }

  function setDemoFilter(demoFilter: LibraryFilters["demoFilter"]) {
    setFilters((current) => ({ ...current, demoFilter }));
    setSectionPages(DEFAULT_SECTION_PAGES);
  }

  function setMinPlayers(minPlayers: number) {
    setFilters((current) => ({ ...current, minPlayers }));
    setSectionPages(DEFAULT_SECTION_PAGES);
  }

  function setMinReviewPct(minReviewPct: number) {
    setFilters((current) => ({ ...current, minReviewPct }));
    setSectionPages(DEFAULT_SECTION_PAGES);
  }

  function setReleaseWindow(releaseWindow: LibraryFilters["releaseWindow"]) {
    setFilters((current) => ({ ...current, releaseWindow }));
    setSectionPages(DEFAULT_SECTION_PAGES);
  }

  function toggleHideAdultContent() {
    setFilters((current) => ({
      ...current,
      hideAdultContent: !current.hideAdultContent,
    }));
    setSectionPages(DEFAULT_SECTION_PAGES);
  }

  function toggleQuickTag(tag: string) {
    setFilters((current) => ({
      ...current,
      selectedTags: current.selectedTags.includes(tag)
        ? current.selectedTags.filter((item) => item !== tag)
        : [...current.selectedTags, tag],
    }));
    setSectionPages(DEFAULT_SECTION_PAGES);
  }

  function changeSectionPage(sectionId: keyof DashboardSectionPageState, page: number) {
    setSectionPages((current) =>
      current[sectionId] === page
        ? current
        : { ...current, [sectionId]: Math.max(1, page) },
    );
  }

  const allGames = useMemo(
    () => [
      ...(dashboard?.upcoming ?? []),
      ...(dashboard?.newGames ?? []),
      ...(dashboard?.classics ?? []),
      ...(dashboard?.hiddenGames ?? []),
    ],
    [dashboard],
  );

  const availableTags = useMemo(() => {
    const seen = new Set<string>();
    const ordered: string[] = [];

    for (const game of allGames) {
      if (Array.isArray(game.tags)) {
        for (const tag of game.tags) {
          if (!seen.has(tag)) {
            seen.add(tag);
            ordered.push(tag);
          }
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

  const activeAiConversation =
    aiConversations.find((conversation) => conversation.id === activeAiConversationId) ??
    aiConversations[0] ??
    createAiConversation(1);

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

  if (appMode.type === "onboarding") {
    return (
      <ErrorBoundary>
        <OnboardingWizard
          config={dashboard.config}
          source={appMode.source}
          onExit={handleExitOnboarding}
        />
      </ErrorBoundary>
    );
  }

  const showDashboardRail = ["home", "new", "classic", "browse"].includes(activeView);

  return (
    <ErrorBoundary>
      <main className="coplay-shell">
        <Sidebar activeView={activeView} onNavigate={setActiveView} />

        <section
          className="page-surface"
          ref={(node) => {
            pageSurfaceRef.current = node;
          }}
        >
        <TopBar
          activeView={activeView}
          query={query}
          selectedGame={selectedGame}
          setQuery={setQuery}
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
            onBack={returnFromDetail}
            onAnalysisUpdated={(report) => {
              commitGameAnalysisSnapshot(snapshotFromReport(report));
              void refreshDashboard().catch((error) => {
                setStatus(error instanceof Error ? error.message : String(error));
              });
            }}
            onToggleState={(patch, message) =>
              handleUserState(selectedGame.appid, patch, message)
            }
            relatedGames={allGames.filter((game) => game.appid !== selectedGame.appid)}
          />
        )}

        {activeView === "ai" && (
          <AiAssistantPage
            activeConversationId={activeAiConversation.id}
            assessment={assessment}
            conversations={aiConversations.map((conversation) => ({
              id: conversation.id,
              title: conversation.title,
              messageCount: conversation.messages.filter((message) => message.role === "user")
                .length,
              updatedAt: conversation.updatedAt,
            }))}
            games={[...visibleNewGames, ...visibleClassics].slice(0, 4)}
            isBusy={isBusy}
            messages={activeAiConversation.messages}
            onAssess={(game) => {
              setSelectedGame(game);
              void handleAiAssess(game);
            }}
            onMessagesChange={handleAiMessagesChange}
            onNewConversation={handleNewAiConversation}
            onOpen={openDetail}
            onRecommend={handleAiRecommend}
            onRecommendationChange={handleAiRecommendationChange}
            onSelectConversation={handleSelectAiConversation}
            recommendation={activeAiConversation.recommendation}
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
            onOpenOnboarding={handleOpenOnboardingFromSettings}
            onRefreshAllAnalyses={handleRefreshAllAnalyses}
            onRetryAiAnalysisJob={handleRetryAiAnalysisJob}
            onStartClassicDiscovery={handleStartClassicDiscovery}
            onRefreshDashboard={refreshDashboard}
            onSave={handleSaveConfig}
            onStatus={setStatus}
            onSync={handleSync}
            expandedSections={settingsExpandedSections}
            onExpandedSectionsChange={setSettingsExpandedSections}
            status={status}
            stats={dashboard.stats}
            aiAnalysisQueueFailures={dashboard.aiAnalysisQueueFailures}
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
            onChangeSectionPage={changeSectionPage}
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
            sectionPages={sectionPages}
            selectedAppid={selectedGame?.appid}
            sortMode={sortMode}
            stats={dashboard.stats}
            status={status}
          />
        )}
      </section>
      <ScrollDock scrollContainer={pageSurfaceRef.current} />
      <TaskToastLayer toasts={taskToasts} onDismiss={dismissTaskToast} />
    </main>
    </ErrorBoundary>
  );
}

function maybeNotifyDiscoveryCompletion(
  previousSnapshot: DiscoveryRunSnapshot | null,
  nextSnapshot: DiscoveryRunSnapshot,
  notify: (tone: TaskToastTone, title: string, message: string) => void,
) {
  if (!previousSnapshot || previousSnapshot.id !== nextSnapshot.id) {
    return;
  }

  if (previousSnapshot.status === nextSnapshot.status) {
    return;
  }

  if (!isRunningStatus(previousSnapshot.status) || !isTerminalDiscoveryStatus(nextSnapshot.status)) {
    return;
  }

  switch (nextSnapshot.status) {
    case "completed":
      notify(
        "success",
        "新游入库已完成",
        `任务 #${nextSnapshot.id} 已结束，新增 ${nextSnapshot.addedGames} 个新游戏，已检查 ${nextSnapshot.scannedApps} 个候选。`,
      );
      break;
    case "failed":
      notify(
        "danger",
        "新游入库失败",
        nextSnapshot.lastError ?? `任务 #${nextSnapshot.id} 失败，请查看发现任务记录。`,
      );
      break;
    case "cancelled":
      notify("warning", "新游入库已取消", `任务 #${nextSnapshot.id} 已取消。`);
      break;
    case "interrupted":
      notify("warning", "新游入库已中断", `任务 #${nextSnapshot.id} 已中断，可继续恢复。`);
      break;
    case "paused":
      notify("warning", "新游入库已暂停", `任务 #${nextSnapshot.id} 已暂停。`);
      break;
    default:
      break;
  }
}

function maybeNotifyStatsTransitions(
  previousStats: DashboardStats | null,
  nextStats: DashboardStats,
  notify: (tone: TaskToastTone, title: string, message: string) => void,
) {
  if (!previousStats) {
    return;
  }

  if (previousStats.backfillRunning && !nextStats.backfillRunning && nextStats.backfillPendingCount === 0) {
    notify(
      nextStats.backfillFailedCount > previousStats.backfillFailedCount ? "warning" : "success",
      nextStats.backfillFailedCount > previousStats.backfillFailedCount ? "新游补全结束（含失败）" : "新游补全已完成",
      `已处理 ${nextStats.backfillProcessedCount}/${nextStats.backfillTotalCount}，失败 ${nextStats.backfillFailedCount}。`,
    );
  }

  if (previousStats.syncRunning && !nextStats.syncRunning && nextStats.syncPendingCount === 0) {
    notify(
      nextStats.syncFailedCount > previousStats.syncFailedCount ? "warning" : "success",
      nextStats.syncFailedCount > previousStats.syncFailedCount ? "Steam 同步结束（含失败）" : "Steam 同步已完成",
      `已更新 ${nextStats.syncUpdatedCount} 个游戏，失败 ${nextStats.syncFailedCount}。`,
    );
  }

  if (previousStats.classicDiscoveryRunning && !nextStats.classicDiscoveryRunning) {
    const tone =
      nextStats.classicDiscoveryStatus === "failed"
        ? "danger"
        : nextStats.classicDiscoveryStatus === "completed"
          ? "success"
          : "warning";
    const title =
      nextStats.classicDiscoveryStatus === "failed"
        ? "老游补库失败"
        : nextStats.classicDiscoveryStatus === "completed"
          ? "老游补库已完成"
          : "老游补库已停止";
    notify(
      tone,
      title,
      `已新增 ${nextStats.classicDiscoveryAddedGames} 个老游戏，扫描 ${nextStats.classicDiscoveryScannedApps} 个候选。`,
    );
  }

  if (previousStats.aiBatchRefreshRunning && !nextStats.aiBatchRefreshRunning) {
    notify(
      nextStats.aiBatchRefreshFailedCount > previousStats.aiBatchRefreshFailedCount
        ? "warning"
        : "success",
      nextStats.aiBatchRefreshFailedCount > previousStats.aiBatchRefreshFailedCount
        ? "AI 批量重算结束（含失败）"
        : "AI 批量重算已完成",
      `已处理 ${nextStats.aiBatchRefreshProcessedCount}/${nextStats.aiBatchRefreshTotalCount}，失败 ${nextStats.aiBatchRefreshFailedCount}。`,
    );
  }
}

function isRunningStatus(status: DiscoveryRunStatus) {
  return status === "running";
}

function isTerminalDiscoveryStatus(status: DiscoveryRunStatus) {
  return (
    status === "completed" ||
    status === "failed" ||
    status === "cancelled" ||
    status === "interrupted" ||
    status === "paused"
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
}: {
  activeView: ViewId;
  query: string;
  setQuery: (value: string) => void;
  selectedGame: GameCard | null;
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
      </div>
    </header>
  );
}

export default App;
