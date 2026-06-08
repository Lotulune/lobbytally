import type { DashboardStats, GameCard, SyncMode } from "../../types";
import type { DashboardSection } from "../../features/library/gameFilters";
import { getDisplayedGameScore } from "../../features/library/gameScoreDisplay";
import type {
  DemoFilter,
  LibraryFilters,
  LibrarySortMode,
  ReleaseWindow,
  ViewId,
} from "../types";

const sortOptions: Array<{ value: LibrarySortMode; label: string }> = [
  { value: "recommended", label: "综合排序" },
  { value: "reviews", label: "好评度" },
  { value: "players", label: "游玩人数" },
  { value: "release", label: "发售时间" },
];

const demoFilterOptions: Array<{ label: string; value: DemoFilter }> = [
  { value: "all", label: "全部" },
  { value: "demo_only", label: "仅 Demo" },
  { value: "released_with_demo", label: "Demo & 已发售" },
  { value: "released", label: "已发售" },
];

const releaseWindowOptions: Array<{ label: string; value: ReleaseWindow }> = [
  { value: "all", label: "不限" },
  { value: "week", label: "近一周" },
  { value: "month", label: "近一月" },
  { value: "quarter", label: "近三月" },
  { value: "year", label: "近一年" },
];

const SECTION_PAGE_SIZES: Record<DashboardSection["id"], number> = {
  new: 12,
  classic: 12,
  recent: 8,
};

export type DashboardSectionPageState = Record<DashboardSection["id"], number>;

export function DashboardPage({
  activeView,
  filters,
  isBusy,
  quickTags,
  sections,
  sectionPages,
  selectedAppid,
  showAiAssistant = true,
  sortMode,
  stats,
  status,
  onAi,
  onChangeView,
  onChangeSectionPage,
  onOpenFilters,
  onOpenGame,
  onResetFilters,
  onSetDemoFilter,
  onSetSortMode,
  onSetMinPlayers,
  onSetMinReviewPct,
  onSetReleaseWindow,
  onToggleHideAdultContent,
  onToggleQuickTag,
  onSync,
}: {
  activeView: ViewId;
  filters: LibraryFilters;
  isBusy: boolean;
  quickTags: string[];
  sections: DashboardSection[];
  sectionPages: DashboardSectionPageState;
  selectedAppid?: number;
  showAiAssistant?: boolean;
  sortMode: LibrarySortMode;
  stats: DashboardStats;
  status: string;
  onAi: () => void;
  onChangeView: (view: ViewId) => void;
  onChangeSectionPage: (sectionId: DashboardSection["id"], page: number) => void;
  onOpenFilters: () => void;
  onOpenGame: (game: GameCard) => void;
  onResetFilters: () => void;
  onSetDemoFilter: (mode: DemoFilter) => void;
  onSetSortMode: (mode: LibrarySortMode) => void;
  onSetMinPlayers: (value: number) => void;
  onSetMinReviewPct: (value: number) => void;
  onSetReleaseWindow: (value: ReleaseWindow) => void;
  onToggleHideAdultContent: () => void;
  onToggleQuickTag: (tag: string) => void;
  onSync: (mode: SyncMode) => void;
}) {
  const isHome = activeView === "home";

  return (
    <div className="dashboard-layout">
      <section className="dashboard-main">
        <Tabs activeView={activeView} onChange={onChangeView} />
        <Toolbar
          demoFilter={filters.demoFilter}
          onSetDemoFilter={onSetDemoFilter}
          sortMode={sortMode}
          onSetSortMode={onSetSortMode}
        />

        {sections.map((section) => (
          <GameSection
            currentPage={sectionPages[section.id] ?? 1}
            games={section.games}
            isHome={isHome}
            key={section.id}
            onChangePage={(page) => onChangeSectionPage(section.id, page)}
            onSelect={onOpenGame}
            onViewAll={
              isHome
                ? () =>
                    onChangeView(
                      section.id === "recent" ? "browse" : section.id,
                    )
                : undefined
            }
            sectionId={section.id}
            selectedAppid={selectedAppid}
            subtitle={section.subtitle}
            title={section.title}
          />
        ))}

        {sections.length === 0 && (
          <div className="empty-results">
            <h2>没有匹配的游戏</h2>
            <p>试试放宽 Demo 状态、标签或好评度筛选条件。</p>
            <button className="muted-button" type="button" onClick={onResetFilters}>
              清空筛选
            </button>
          </div>
        )}

        <p className="hint-line">
          💡 已生成详细评估的游戏会优先显示综合推荐；未评估时仍显示基础推荐值。
        </p>
      </section>

      <RightRail
        filters={filters}
        isBusy={isBusy}
        onAi={onAi}
        onOpenFilters={onOpenFilters}
        onResetFilters={onResetFilters}
        onSetMinPlayers={onSetMinPlayers}
        onSetMinReviewPct={onSetMinReviewPct}
        onSetReleaseWindow={onSetReleaseWindow}
        onToggleHideAdultContent={onToggleHideAdultContent}
        onToggleQuickTag={onToggleQuickTag}
        onSync={onSync}
        quickTags={quickTags}
        showAiAssistant={showAiAssistant}
        stats={stats}
        status={status}
      />
    </div>
  );
}

function Tabs({
  activeView,
  onChange,
}: {
  activeView: ViewId;
  onChange: (view: ViewId) => void;
}) {
  return (
    <div className="section-tabs">
      <button
        className={activeView !== "classic" ? "active" : ""}
        onClick={() => onChange("new")}
        type="button"
      >
        新游区
      </button>
      <button
        className={activeView === "classic" ? "active" : ""}
        onClick={() => onChange("classic")}
        type="button"
      >
        精品老游区
      </button>
    </div>
  );
}

function Toolbar({
  demoFilter,
  onSetDemoFilter,
  sortMode,
  onSetSortMode,
}: {
  demoFilter: DemoFilter;
  onSetDemoFilter: (mode: DemoFilter) => void;
  sortMode: LibrarySortMode;
  onSetSortMode: (mode: LibrarySortMode) => void;
}) {
  return (
    <div className="toolbar">
      <div aria-label="Demo 状态" className="status-tabs" role="group">
        {demoFilterOptions.map((option) => (
          <button
            aria-pressed={demoFilter === option.value}
            className={demoFilter === option.value ? "active" : ""}
            key={option.value}
            onClick={() => onSetDemoFilter(option.value)}
            type="button"
          >
            {option.label}
          </button>
        ))}
      </div>
      <div aria-label="排序方式" className="sort-row" role="group">
        {sortOptions.map((option) => (
          <button
            aria-pressed={sortMode === option.value}
            className={sortMode === option.value ? "active" : ""}
            key={option.value}
            onClick={() => onSetSortMode(option.value)}
            type="button"
          >
            {option.label}
          </button>
        ))}
      </div>
    </div>
  );
}

function GameSection({
  currentPage,
  sectionId,
  title,
  subtitle,
  games,
  isHome,
  selectedAppid,
  onChangePage,
  onSelect,
  onViewAll,
}: {
  currentPage: number;
  sectionId: DashboardSection["id"];
  title: string;
  subtitle: string;
  games: GameCard[];
  isHome: boolean;
  selectedAppid?: number;
  onChangePage: (page: number) => void;
  onSelect: (game: GameCard) => void;
  onViewAll?: () => void;
}) {
  const pageSize = SECTION_PAGE_SIZES[sectionId];
  const totalPages = Math.max(1, Math.ceil(games.length / pageSize));
  const safePage = Math.min(currentPage, totalPages);
  const visibleGames = isHome
    ? games
    : games.slice((safePage - 1) * pageSize, safePage * pageSize);

  return (
    <section className="game-section">
      <div className="game-section-head">
        <div>
          <h2>{title}</h2>
          <span>{subtitle}</span>
        </div>
        <div className="game-section-tools">
          {onViewAll && (
            <button onClick={onViewAll} type="button">
              查看全部 〉
            </button>
          )}
          {!isHome && (
            <div
              aria-label={`${title} 分页`}
              className="game-section-page"
              role="group"
            >
              <span>{`共 ${games.length} 款`}</span>
              <button
                disabled={safePage <= 1}
                onClick={() => onChangePage(Math.max(1, safePage - 1))}
                type="button"
              >
                上一页
              </button>
              <strong>{`第 ${safePage} / ${totalPages} 页`}</strong>
              <button
                disabled={safePage >= totalPages}
                onClick={() => onChangePage(Math.min(totalPages, safePage + 1))}
                type="button"
              >
                下一页
              </button>
            </div>
          )}
        </div>
      </div>

      <div className="game-grid">
        {visibleGames.map((game) => {
          const scoreDisplay = getDisplayedGameScore(game);

          return (
            <button
              className={selectedAppid === game.appid ? "game-card selected" : "game-card"}
              key={game.appid}
              onClick={() => onSelect(game)}
              type="button"
            >
              <div className="cover-wrap">
                <img src={game.capsuleUrl} alt="" loading="lazy" />
                <div className="card-pill-stack">
                  {game.isFree ? <span className="demo-pill free">Free</span> : null}
                  <span className={`demo-pill ${game.demoStatus}`}>
                    {demoLabel(game.demoStatus)}
                  </span>
                </div>
              </div>
              <div className="game-body">
                <h3>{game.name}</h3>
                <p>{(Array.isArray(game.tags) ? game.tags : []).slice(0, 3).join(" · ")}</p>
                <div className="review-line">
                  <span>♟ {formatPct(game.positiveReviewPct)} 好评</span>
                  <em>({formatNumber(game.totalReviews)})</em>
                </div>
                <div className="player-line">♟ {formatNumber(game.currentPlayers)} 当前在线</div>
                <div className="card-bottom">
                  <span>{game.releaseDateText} 发行</span>
                  <strong>
                    {Math.round(scoreDisplay.value)}
                    <small>{scoreDisplay.label}</small>
                  </strong>
                </div>
              </div>
            </button>
          );
        })}
      </div>
    </section>
  );
}

function RightRail({
  filters,
  isBusy,
  onAi,
  onOpenFilters,
  onResetFilters,
  onSetMinPlayers,
  onSetMinReviewPct,
  onSetReleaseWindow,
  onToggleHideAdultContent,
  onToggleQuickTag,
  onSync,
  quickTags,
  showAiAssistant,
  stats,
  status,
}: {
  filters: LibraryFilters;
  isBusy: boolean;
  onAi: () => void;
  onOpenFilters: () => void;
  onResetFilters: () => void;
  onSetMinPlayers: (value: number) => void;
  onSetMinReviewPct: (value: number) => void;
  onSetReleaseWindow: (value: ReleaseWindow) => void;
  onToggleHideAdultContent: () => void;
  onToggleQuickTag: (tag: string) => void;
  onSync: (mode: SyncMode) => void;
  quickTags: string[];
  showAiAssistant: boolean;
  stats: DashboardStats;
  status: string;
}) {
  const activeReleaseWindow =
    releaseWindowOptions.find(
      (option) => option.value === filters.releaseWindow,
    ) ?? releaseWindowOptions[0];
  const isPublicServiceMode = stats.sourceKind === "public_service";
  const hasBackfillActivity =
    stats.backfillRunning ||
    stats.backfillPendingCount > 0 ||
    stats.backfillTotalCount > 0;
  const hasSyncResume = !stats.syncRunning && stats.syncPendingCount > 0;
  const hasSyncActivity =
    stats.syncRunning || stats.syncPendingCount > 0 || stats.syncTotalCount > 0;
  const syncProgressPercent =
    stats.syncTotalCount > 0
      ? Math.round((stats.syncProcessedCount / stats.syncTotalCount) * 100)
      : 0;
  const syncStatusLabel = describeSyncStatus(stats);
  const { fullLabel, quickLabel } = syncActionLabels(stats);
  const backfillProgressPercent =
    stats.backfillTotalCount > 0
      ? Math.round((stats.backfillProcessedCount / stats.backfillTotalCount) * 100)
      : 0;
  const backfillStatusLabel = describeBackfillStatus(stats);

  function cycleReleaseWindow() {
    const currentIndex = releaseWindowOptions.findIndex(
      (option) => option.value === filters.releaseWindow,
    );
    const nextOption =
      releaseWindowOptions[(currentIndex + 1) % releaseWindowOptions.length];
    onSetReleaseWindow(nextOption.value);
  }

  function handleQuickTagClick(tag: string) {
    onToggleQuickTag(tag);
  }

  return (
    <aside className="right-rail">
      <section className="stats-card">
        <div className="rail-title">
          <span>▦</span>
          <h2>数据概览</h2>
        </div>
        <div className="stats-grid">
          <div className="stats-item">
            <strong>{formatNumber(stats.totalGames)}</strong>
            <span>库内游戏</span>
          </div>
          <div className="stats-item">
            <strong>{formatNumber(stats.newGamesCount)}</strong>
            <span>新游区</span>
          </div>
          <div className="stats-item">
            <strong>{formatNumber(stats.classicGamesCount)}</strong>
            <span>老游区</span>
          </div>
          {isPublicServiceMode ? (
            <>
              <div className="stats-item">
                <strong>匿名只读</strong>
                <span>公共库状态</span>
              </div>
              <div className="stats-item">
                <strong>本地保存</strong>
                <span>个人状态</span>
              </div>
            </>
          ) : (
            <div className="stats-item">
              <strong>{stats.lastDiscoveryAppid ?? "无"}</strong>
              <span>扫描游标</span>
            </div>
          )}
        </div>
        {!isPublicServiceMode ? (
          <p className="stats-meta">最近同步：{formatDateTime(stats.lastSyncAt)}</p>
        ) : null}
        {!isPublicServiceMode ? (
          <>
            <div className="backfill-status-block">
              <div className="backfill-status-head">
                <strong>Steam 同步</strong>
                <span>{syncStatusLabel}</span>
              </div>
              {hasSyncActivity ? (
                <>
                  <div className="discovery-progress-track" aria-hidden="true">
                    <div
                      className="discovery-progress-fill"
                      style={{ width: `${syncProgressPercent}%` }}
                    />
                  </div>
                  <div className="backfill-status-grid">
                    <div>
                      <span>模式</span>
                      <strong>{syncModeLabel(stats.syncMode)}</strong>
                    </div>
                    <div>
                      <span>已处理</span>
                      <strong>{`${formatNumber(stats.syncProcessedCount)}/${formatNumber(stats.syncTotalCount)}`}</strong>
                    </div>
                    <div>
                      <span>剩余</span>
                      <strong>{formatNumber(stats.syncPendingCount)}</strong>
                    </div>
                    <div>
                      <span>已更新</span>
                      <strong>{formatNumber(stats.syncUpdatedCount)}</strong>
                    </div>
                    <div>
                      <span>失败</span>
                      <strong>{formatNumber(stats.syncFailedCount)}</strong>
                    </div>
                    <div>
                      <span>当前 AppID</span>
                      <strong>{stats.syncCurrentAppid ?? "无"}</strong>
                    </div>
                  </div>
                  <p className="mini-status">
                    {stats.syncCurrentAppid
                      ? `当前正在同步 AppID ${stats.syncCurrentAppid}。`
                      : hasSyncResume
                        ? `队列中仍有 ${formatNumber(stats.syncPendingCount)} 个游戏待续同步。`
                        : stats.syncFailedCount > 0
                        ? `同步已结束，但最近一次失败发生在 AppID ${stats.syncLastErrorAppid ?? "无"}。`
                        : "本轮同步已完成。"}
                  </p>
                  {stats.syncLastError ? <p className="settings-error">{stats.syncLastError}</p> : null}
                </>
              ) : (
                <p className="mini-status">
                  完整同步会刷新商店图、评论、在线人数和评价样本；快速同步只刷新商店侧元数据。
                </p>
              )}
            </div>
            <div className="backfill-status-block">
              <div className="backfill-status-head">
                <strong>元数据补录</strong>
                <span>{backfillStatusLabel}</span>
              </div>
              {hasBackfillActivity ? (
                <>
                  <div className="discovery-progress-track" aria-hidden="true">
                    <div
                      className="discovery-progress-fill"
                      style={{ width: `${backfillProgressPercent}%` }}
                    />
                  </div>
                  <div className="backfill-status-grid">
                    <div>
                      <span>已处理</span>
                      <strong>{`${formatNumber(stats.backfillProcessedCount)}/${formatNumber(stats.backfillTotalCount)}`}</strong>
                    </div>
                    <div>
                      <span>剩余</span>
                      <strong>{formatNumber(stats.backfillPendingCount)}</strong>
                    </div>
                    <div>
                      <span>失败</span>
                      <strong>{formatNumber(stats.backfillFailedCount)}</strong>
                    </div>
                    <div>
                      <span>当前 AppID</span>
                      <strong>{stats.backfillCurrentAppid ?? "无"}</strong>
                    </div>
                  </div>
                  <p className="mini-status">
                    {stats.backfillCurrentAppid
                      ? `当前正在补录 AppID ${stats.backfillCurrentAppid}（第 ${stats.backfillCurrentAttempt ?? 1}/${stats.backfillMaxAttempts} 次尝试）。`
                      : stats.backfillPendingCount > 0
                        ? "新游补全清空后即可启动老游补库；老游 AI 仍会排在新游 AI 后面。"
                        : stats.backfillFailedCount > 0
                          ? "本轮补录已结束，但有部分游戏补录失败。"
                          : "本轮新游补全已完成；若有老游待入库，现在可以继续启动老游补库。"}
                  </p>
                </>
              ) : (
                <p className="mini-status">新游发现可选完整拉取或部分拉取；只有完整拉取才会补录在线人数和评论片段，老游补库只等新游补全清空。</p>
              )}
            </div>
          </>
        ) : null}
        <p className="mini-status">{stats.dataSource}</p>
      </section>

      {showAiAssistant ? (
        <section className="ai-card">
          <div className="rail-title">
            <span>✨</span>
            <h2>AI 智能推荐助手</h2>
            <em>Beta</em>
          </div>
          <p>让 AI 帮你找到最适合的多人游戏</p>
          <div className="prompt-box">
            例如：
            <br />
            想找适合和朋友休闲联机，
            <br />
            不太复杂但很有趣的游戏
          </div>
          <button className="gold-button" type="button" onClick={onAi}>
            ✦ 让 AI 帮我找游戏
          </button>
          <p className="mini-status">{status}</p>
        </section>
      ) : null}

      <section className="filter-card">
        <div className="filter-head">
          <h2>筛选条件</h2>
          <button type="button" onClick={onResetFilters}>
            ↻ 重置
          </button>
        </div>
        <label className="range-field">
          <span>
            在线人数下限
            <b className="range-current">{filters.minPlayers}</b>
          </span>
          <div className="range-control-row">
            <input
              max="1000"
              min="0"
              onChange={(event) => {
                const value = Number(event.currentTarget.value);
                onSetMinPlayers(value);
              }}
              type="range"
              value={filters.minPlayers}
            />
            <input
              className="range-number-input"
              max="1000"
              min="0"
              onChange={(event) => {
                const value = Number(event.currentTarget.value);
                onSetMinPlayers(
                  Number.isFinite(value) ? Math.max(0, Math.min(1000, value)) : 0,
                );
              }}
              type="number"
              value={filters.minPlayers}
            />
          </div>
          <small>
            <b>0</b>
            <b>1000+</b>
          </small>
        </label>
        <label className="range-field">
          <span>
            Steam 好评度
            <b className="range-current">{filters.minReviewPct}%</b>
          </span>
          <div className="range-control-row">
            <input
              max="100"
              min="0"
              onChange={(event) => {
                const value = Number(event.currentTarget.value);
                onSetMinReviewPct(value);
              }}
              type="range"
              value={filters.minReviewPct}
            />
            <input
              className="range-number-input"
              max="100"
              min="0"
              onChange={(event) => {
                const value = Number(event.currentTarget.value);
                onSetMinReviewPct(
                  Number.isFinite(value) ? Math.max(0, Math.min(100, value)) : 0,
                );
              }}
              type="number"
              value={filters.minReviewPct}
            />
          </div>
          <small>
            <b>0%</b>
            <b>100%</b>
          </small>
        </label>
        <SelectRow
          label="发售时间"
          onClick={cycleReleaseWindow}
          value={activeReleaseWindow.label}
        />
        <div className="select-row static">
          <span>支持语言</span>
          <small>跟随当前抓取语言</small>
        </div>
        <div className="tag-panel">
          <div>
            <span>游戏标签</span>
            <button type="button" onClick={onOpenFilters}>
              更多标签 〉
            </button>
          </div>
          <div className="tag-list">
            {quickTags.map((tag) => {
              const isSelected =
                Array.isArray(filters.selectedTags) &&
                filters.selectedTags.includes(tag);
              return (
                <button
                  aria-pressed={isSelected}
                  className={isSelected ? "active" : ""}
                  key={tag}
                  onClick={() => handleQuickTagClick(tag)}
                  type="button"
                >
                  {tag}
                </button>
              );
            })}
          </div>
        </div>
        {!isPublicServiceMode ? (
          <div className="stacked-actions">
            <button
              className="gold-button"
              disabled={isBusy || stats.syncRunning}
              onClick={() => onSync("full")}
              type="button"
            >
              {fullLabel}
            </button>
            <button
              className="ghost-button"
              disabled={isBusy || stats.syncRunning}
              onClick={() => onSync("quick")}
              type="button"
            >
              {quickLabel}
            </button>
          </div>
        ) : null}
        <button
          aria-pressed={filters.hideAdultContent}
          className={filters.hideAdultContent ? "toggle-row active" : "toggle-row"}
          onClick={onToggleHideAdultContent}
          type="button"
        >
          <span>隐藏成人内容</span>
          <i />
          <small>{filters.hideAdultContent ? "已隐藏" : "未隐藏"}</small>
        </button>
      </section>
    </aside>
  );
}

function SelectRow({
  label,
  onClick,
  value,
}: {
  label: string;
  onClick: () => void;
  value: string;
}) {
  return (
    <div className="select-row">
      <span>{label}</span>
      <button type="button" onClick={onClick}>
        {value}⌄
      </button>
    </div>
  );
}

function demoLabel(status: GameCard["demoStatus"]) {
  switch (status) {
    case "demo_only":
      return "Demo";
    case "released_with_demo":
      return "Demo & 已发售";
    case "released":
      return "已发售";
    case "unknown":
      return "未知";
  }
}

function formatPct(value?: number | null) {
  return typeof value === "number" ? `${Math.round(value)}%` : "—";
}

function formatNumber(value?: number | null) {
  return typeof value === "number" ? value.toLocaleString("zh-CN") : "—";
}

function formatDateTime(value?: string | null) {
  if (!value) return "未同步";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function describeBackfillStatus(stats: DashboardStats) {
  if (stats.backfillRunning) {
    return "新游补全中";
  }

  if (stats.backfillPendingCount > 0) {
    return "待补全";
  }

  if (stats.backfillTotalCount > 0) {
    return stats.backfillFailedCount > 0 ? "已完成（含失败）" : "已完成";
  }

  return "空闲";
}

function describeSyncStatus(stats: DashboardStats) {
  if (stats.syncRunning) {
    return `${syncModeLabel(stats.syncMode)}中`;
  }

  if (stats.syncPendingCount > 0) {
    return "待续同步";
  }

  if (stats.syncTotalCount > 0) {
    const modeLabel = syncModeLabel(stats.syncMode);
    return stats.syncFailedCount > 0 ? `${modeLabel}已完成（含失败）` : `${modeLabel}已完成`;
  }

  return "空闲";
}

function syncModeLabel(mode?: SyncMode | null) {
  return mode === "quick" ? "快速同步" : "完整同步";
}

function syncActionLabels(stats: DashboardStats) {
  if (stats.syncRunning) {
    return {
      fullLabel: stats.syncMode === "full" ? "完整同步中…" : "完整同步",
      quickLabel: stats.syncMode === "quick" ? "快速同步中…" : "快速同步",
    };
  }

  if (stats.syncPendingCount === 0) {
    return {
      fullLabel: "完整同步",
      quickLabel: "快速同步",
    };
  }

  if (stats.syncMode === "quick") {
    return {
      fullLabel: "继续并升级为完整同步",
      quickLabel: "继续快速同步",
    };
  }

  return {
    fullLabel: "继续完整同步",
    quickLabel: "继续待续同步",
  };
}
