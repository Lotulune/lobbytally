import { useEffect, useRef, useState } from "react";
import type {
  DashboardStats,
  DiscoveryRunSnapshot,
  DiscoveryRunStatus,
  SyncMode,
} from "../../types";
import { useDiscoveryTask } from "./useDiscoveryTask";

const DEFAULT_TARGET_ADDED_GAMES = 6;
const DEFAULT_PAGE_SIZE = 100;
const DEFAULT_SYNC_MODE: SyncMode = "full";
const MAX_TARGET_ADDED_GAMES = 200;
const MAX_PAGE_SIZE = 100;

const statusMeta: Record<DiscoveryRunStatus, { label: string; tone: string }> = {
  running: { label: "进行中", tone: "running" },
  paused: { label: "已暂停", tone: "paused" },
  completed: { label: "已完成", tone: "completed" },
  failed: { label: "已失败", tone: "failed" },
  cancelled: { label: "已取消", tone: "cancelled" },
  interrupted: { label: "已中断", tone: "failed" },
};

const resumableStatuses: DiscoveryRunStatus[] = ["paused", "interrupted"];
const terminalStatuses: DiscoveryRunStatus[] = ["completed", "failed", "cancelled"];

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function formatDateTime(value: string | null | undefined) {
  if (!value) {
    return "无";
  }

  const timestamp = Date.parse(value);
  if (Number.isNaN(timestamp)) {
    return value;
  }

  return new Intl.DateTimeFormat("zh-CN", {
    dateStyle: "short",
    timeStyle: "short",
  }).format(timestamp);
}

function formatNumber(value: number | null | undefined) {
  if (value == null) {
    return "0";
  }

  return new Intl.NumberFormat("zh-CN").format(value);
}

function summarizeSnapshot(snapshot: DiscoveryRunSnapshot) {
  const meta = statusMeta[snapshot.status];
  if (snapshot.completionReason === "page_budget_reached" && snapshot.addedGames === 0) {
    return `发现任务${meta.label}：本轮已检查 ${snapshot.scannedApps} 个最近发售多人候选，但没有命中新游戏。`;
  }

  return `发现任务${meta.label}：新增 ${snapshot.addedGames}/${snapshot.targetAddedGames}，已检查 ${snapshot.scannedApps} 个最近发售多人候选。`;
}

function isResumableStatus(status: DiscoveryRunStatus | null | undefined) {
  return status != null && resumableStatuses.includes(status);
}

function isTerminalStatus(status: DiscoveryRunStatus | null | undefined) {
  return status != null && terminalStatuses.includes(status);
}

function describeCompletionReason(snapshot: DiscoveryRunSnapshot | null) {
  if (!snapshot) {
    return "等待任务启动。";
  }

  if (snapshot.status === "interrupted") {
    return "任务被中断，可继续恢复。";
  }

  if (snapshot.status === "running") {
    return snapshot.currentAppid
      ? `正在处理 AppID ${snapshot.currentAppid}。`
      : "正在请求下一批最近发售候选。";
  }

  if (snapshot.completionReason === "paused") {
    return "任务已暂停，可继续从上次位置恢复。";
  }

  if (snapshot.completionReason === "cancelled") {
    return "任务已取消。";
  }

  if (snapshot.completionReason === "failed") {
    return snapshot.lastError ?? "任务失败，请查看失败记录。";
  }

  if (snapshot.completionReason === "target_reached") {
    return `本轮已达到目标，新增 ${snapshot.addedGames} 个新游戏。`;
  }

  if (snapshot.completionReason === "no_more_results") {
    return `Steam 最近发售候选已扫到末尾，本轮共检查 ${snapshot.scannedApps} 个候选。`;
  }

  if (snapshot.completionReason === "page_budget_reached") {
    return snapshot.addedGames > 0
      ? `本轮已扫满当前页数预算，共检查 ${snapshot.scannedApps} 个候选，新增 ${snapshot.addedGames} 个新游戏。`
      : `本轮已扫满当前页数预算，共检查 ${snapshot.scannedApps} 个最近发售多人候选，但没有命中新游戏。`;
  }

  if (snapshot.addedGames > 0) {
    return `本轮已新增 ${snapshot.addedGames} 个新游戏。`;
  }

  if (snapshot.scannedApps > 0) {
    return `本轮已扫完当前配置范围，共检查 ${snapshot.scannedApps} 个最近发售多人候选，但没有命中新游戏。`;
  }

  return "本轮没有可展示的发现结果。";
}

function displayProgressPercent(snapshot: DiscoveryRunSnapshot | null) {
  if (!snapshot) {
    return 0;
  }

  if (
    snapshot.completionReason === "target_reached" ||
    snapshot.completionReason === "page_budget_reached" ||
    snapshot.completionReason === "no_more_results"
  ) {
    return 100;
  }

  return snapshot.progressPercent;
}

function hasPendingBackfill(stats: DashboardStats) {
  return stats.backfillRunning || stats.backfillPendingCount > 0;
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

function describePipelineHint(stats: DashboardStats) {
  if (stats.backfillRunning || stats.backfillPendingCount > 0) {
    return "新游发现结束后，老游补库会在新游补全清空后启动；新游 AI 仍会继续先跑。";
  }

  if (stats.aiBatchRefreshRunning || stats.aiBatchRefreshPendingCount > 0) {
    return "新游补全已清空；老游补库可以启动。老游 AI 会继续排在新游 AI 队列之后。";
  }

  return "当前链路：新游入库与补全并行；老游补库只等新游补全清空，老游 AI 继续排在新游 AI 后面。";
}

function backfillProgressPercent(stats: DashboardStats) {
  if (stats.backfillTotalCount === 0) {
    return 0;
  }

  return Math.round((stats.backfillProcessedCount / stats.backfillTotalCount) * 100);
}

function discoverySyncModeLabel(mode: SyncMode) {
  return mode === "quick" ? "部分拉取" : "完整拉取";
}

function SubPanel({
  title,
  status,
  expanded,
  onToggle,
  children,
}: {
  title: string;
  status?: React.ReactNode;
  expanded: boolean;
  onToggle: () => void;
  children: React.ReactNode;
}) {
  return (
    <section className="discovery-subpanel">
      <button
        type="button"
        className="discovery-subpanel-header"
        aria-expanded={expanded}
        onClick={onToggle}
      >
        <h4>{title}</h4>
        {status && <span>{status}</span>}
        <em className="discovery-subpanel-chevron">{expanded ? "▼" : "▶"}</em>
      </button>
      {expanded && <div className="discovery-subpanel-body">{children}</div>}
    </section>
  );
}

export function DiscoveryTaskPanel({
  stats,
  onStatus,
  onRefreshDashboard,
}: {
  stats: DashboardStats;
  onStatus: (message: string) => void;
  onRefreshDashboard: () => Promise<unknown>;
}) {
  const {
    snapshot,
    history,
    isLoading,
    start,
    pause,
    resume,
    cancel,
  } = useDiscoveryTask();
  const [targetAddedGames, setTargetAddedGames] = useState(DEFAULT_TARGET_ADDED_GAMES);
  const [pageSize, setPageSize] = useState(DEFAULT_PAGE_SIZE);
  const [syncMode, setSyncMode] = useState<SyncMode>(DEFAULT_SYNC_MODE);
  const [actionError, setActionError] = useState<string | null>(null);
  const [isDirty, setIsDirty] = useState(false);
  const [expandedPanels, setExpandedPanels] = useState({
    backfill: false,
    failures: false,
    history: true,
  });
  const togglePanel = (key: keyof typeof expandedPanels) =>
    setExpandedPanels((prev) => ({ ...prev, [key]: !prev[key] }));
  const lastRefreshedTerminalKeyRef = useRef<string | null>(null);
  const backfillRefreshIssuedRef = useRef(false);

  useEffect(() => {
    if (!snapshot || isDirty) {
      return;
    }

    setTargetAddedGames(snapshot.targetAddedGames);
    setPageSize(snapshot.pageSize);
    setSyncMode(snapshot.syncMode);
  }, [isDirty, snapshot]);

  useEffect(() => {
    if (!snapshot) {
      return;
    }

    onStatus(summarizeSnapshot(snapshot));
  }, [onStatus, snapshot]);

  useEffect(() => {
    if (!snapshot || !isTerminalStatus(snapshot.status)) {
      return;
    }

    const refreshKey = `${snapshot.id}:${snapshot.status}`;
    if (lastRefreshedTerminalKeyRef.current === refreshKey) {
      return;
    }

    lastRefreshedTerminalKeyRef.current = refreshKey;
    void onRefreshDashboard().catch((error) => {
      const message = error instanceof Error ? error.message : String(error);
      setActionError(message);
      onStatus(message);
    });
  }, [onRefreshDashboard, onStatus, snapshot]);

  useEffect(() => {
    if (!hasPendingBackfill(stats)) {
      backfillRefreshIssuedRef.current = false;
      return;
    }

    if (backfillRefreshIssuedRef.current) {
      return;
    }

    backfillRefreshIssuedRef.current = true;
    void onRefreshDashboard().catch((error) => {
      const message = error instanceof Error ? error.message : String(error);
      setActionError(message);
      onStatus(message);
    });
  }, [onRefreshDashboard, onStatus, stats]);

  const currentStatus = snapshot ? statusMeta[snapshot.status] : null;
  const progressPercent = displayProgressPercent(snapshot);
  const metadataProgressPercent = backfillProgressPercent(stats);
  const effectiveSyncMode = snapshot?.syncMode ?? syncMode;
  const isRunning = snapshot?.status === "running";
  const isResumable = isResumableStatus(snapshot?.status);
  const blocksStart = isRunning;
  const canCancel = !!snapshot && !isTerminalStatus(snapshot.status) && snapshot.status !== "interrupted"
    ? true
    : snapshot?.status === "interrupted";

  async function runAction(action: () => Promise<DiscoveryRunSnapshot>) {
    setActionError(null);
    try {
      const nextSnapshot = await action();
      if (isTerminalStatus(nextSnapshot.status)) {
        await onRefreshDashboard();
      }
      onStatus(summarizeSnapshot(nextSnapshot));
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setActionError(message);
      onStatus(message);
    }
  }

  return (
    <section className="discovery-task-panel">
      <div className="discovery-task-head">
        <div className="discovery-mode-field">
          <h3>发现任务控制台</h3>
          <p>
            当前库：{formatNumber(stats.totalGames)} 个游戏；最近同步：
            {formatDateTime(stats.lastSyncAt)}；最近处理 AppID：{stats.lastDiscoveryAppid ?? "无"}
          </p>
        </div>
        <div className="discovery-task-status-wrap">
          <span
            className={`discovery-status-pill ${currentStatus?.tone ?? "idle"}`}
          >
            {currentStatus?.label ?? "未开始"}
          </span>
          <small>
            {snapshot
              ? `任务 #${snapshot.id} · 更新于 ${formatDateTime(snapshot.updatedAt)}`
              : "暂无发现任务记录"}
          </small>
        </div>
      </div>

      <div className="discovery-task-controls">
        <label>
          目标新增游戏数
          <input
            aria-label="目标新增游戏数"
            max={MAX_TARGET_ADDED_GAMES}
            min={1}
            type="number"
            value={targetAddedGames}
            onChange={(event) => {
              setIsDirty(true);
              setTargetAddedGames(
                clamp(Number(event.currentTarget.value) || 1, 1, MAX_TARGET_ADDED_GAMES),
              );
            }}
          />
        </label>
        <label>
          每页候选数
          <input
            aria-label="每页候选数"
            max={MAX_PAGE_SIZE}
            min={1}
            type="number"
            value={pageSize}
            onChange={(event) => {
              setIsDirty(true);
              setPageSize(
                clamp(Number(event.currentTarget.value) || 1, 1, MAX_PAGE_SIZE),
              );
            }}
          />
        </label>
        <div>
          <span>新游戏拉取方式</span>
          <div className="status-tabs" role="group" aria-label="新游戏拉取方式">
            <button
              className={syncMode === "full" ? "active" : ""}
              type="button"
              onClick={() => {
                setIsDirty(true);
                setSyncMode("full");
              }}
            >
              完整拉取
            </button>
            <button
              className={syncMode === "quick" ? "active" : ""}
              type="button"
              onClick={() => {
                setIsDirty(true);
                setSyncMode("quick");
              }}
            >
              部分拉取
            </button>
          </div>
        </div>
      </div>

      <div className="settings-actions discovery-task-actions">
        <button
          className="gold-button"
          disabled={isLoading || blocksStart}
          type="button"
          onClick={() =>
            void runAction(async () => {
              setIsDirty(false);
              return start({ syncMode, targetAddedGames, pageSize });
            })
          }
        >
          {isLoading && !snapshot ? "启动中…" : "开始新任务"}
        </button>
        <button
          className="muted-button"
          disabled={isLoading || !isRunning}
          type="button"
          onClick={() => void runAction(pause)}
        >
          暂停任务
        </button>
        <button
          className="muted-button"
          disabled={isLoading || !isResumable}
          type="button"
          onClick={() => void runAction(resume)}
        >
          继续任务
        </button>
        <button
          className="muted-button"
          disabled={isLoading || !canCancel}
          type="button"
          onClick={() => void runAction(cancel)}
        >
          取消任务
        </button>
      </div>

      {isResumable && (
        <p className="discovery-task-hint">
          开始新任务会放弃当前可恢复的旧任务，并从最近发售候选的第一页重新扫描。
        </p>
      )}

      {actionError && <p className="settings-error">{actionError}</p>}

      <div className="discovery-progress-card">
        <div className="discovery-progress-meta">
          <strong>进度 {progressPercent}%</strong>
          <span>
            新增 {snapshot?.addedGames ?? 0}/{snapshot?.targetAddedGames ?? targetAddedGames}
          </span>
        </div>
        <div className="discovery-progress-track" aria-hidden="true">
          <div
            className="discovery-progress-fill"
            style={{ width: `${progressPercent}%` }}
          />
        </div>
        <div className="discovery-counter-grid">
          <div>
            <span>已检查</span>
            <strong>{formatNumber(snapshot?.scannedApps)}</strong>
          </div>
          <div>
            <span>新游区</span>
            <strong>{formatNumber(snapshot?.addedNewGames)}</strong>
          </div>
          <div>
            <span>老游区</span>
            <strong>{formatNumber(snapshot?.addedClassicGames)}</strong>
          </div>
          <div>
            <span>失败</span>
            <strong>{formatNumber(snapshot?.failedGames)}</strong>
          </div>
          <div>
            <span>当前 AppID</span>
            <strong>{snapshot?.currentAppid ?? "无"}</strong>
          </div>
          <div>
            <span>最近处理 AppID</span>
            <strong>{snapshot?.lastAppid ?? "无"}</strong>
          </div>
        </div>
        <p className="mini-status">{describeCompletionReason(snapshot)}</p>
        <p className="mini-status">{describePipelineHint(stats)}</p>
      </div>

      <div className="discovery-grid">
        <SubPanel
          title="元数据补全"
          status={`${formatNumber(stats.backfillProcessedCount)}/${formatNumber(stats.backfillTotalCount)}`}
          expanded={expandedPanels.backfill}
          onToggle={() => togglePanel("backfill")}
        >
          <div className="discovery-table">
            <div className="discovery-table-row">
              <strong>{describeBackfillStatus(stats)}</strong>
              <span>进度 {metadataProgressPercent}%</span>
              <div className="discovery-progress-track" aria-hidden="true">
                <div
                  className="discovery-progress-fill"
                  style={{ width: `${metadataProgressPercent}%` }}
                />
              </div>
              <span>剩余队列</span>
              <strong>{formatNumber(stats.backfillPendingCount)}</strong>
              <span>已处理</span>
              <strong>{`${formatNumber(stats.backfillProcessedCount)}/${formatNumber(stats.backfillTotalCount)}`}</strong>
              <span>失败</span>
              <strong>{formatNumber(stats.backfillFailedCount)}</strong>
              <span>当前 AppID</span>
              <strong>{stats.backfillCurrentAppid ?? "无"}</strong>
              <span>尝试</span>
              <strong>{`${stats.backfillCurrentAttempt ?? 0}/${stats.backfillMaxAttempts}`}</strong>
              <p>
                {stats.backfillLastError
                  ? `AppID ${stats.backfillLastErrorAppid ?? "无"} · ${stats.backfillLastError}`
                  : effectiveSyncMode === "quick"
                    ? "当前选择部分拉取；新游戏不会自动补录评论和在线人数。"
                    : "新游补全会并发进行；补全清空后即可启动老游补库。"}
              </p>
            </div>
          </div>
        </SubPanel>

        <SubPanel
          title="失败记录"
          status={`${snapshot?.failures.length ?? 0} 条`}
          expanded={expandedPanels.failures}
          onToggle={() => togglePanel("failures")}
        >
          {snapshot?.failures.length ? (
            <div className="discovery-table">
              {snapshot.failures.map((failure) => (
                <div className="discovery-table-row" key={`${failure.createdAt}-${failure.pageIndex}-${failure.appid ?? "none"}`}>
                  <strong>{failure.stage}</strong>
                  <span>第 {failure.pageIndex} 页 · AppID {failure.appid ?? "无"}</span>
                  <p>{failure.reason}</p>
                </div>
              ))}
            </div>
          ) : (
            <p className="settings-hint">当前任务没有失败记录。</p>
          )}
        </SubPanel>

        <SubPanel
          title="历史任务"
          status={`${history.length} 条`}
          expanded={expandedPanels.history}
          onToggle={() => togglePanel("history")}
        >
          {history.length ? (
            <div className="discovery-table">
              {history.map((item) => (
                <div className="discovery-table-row" key={item.id}>
                  <div className="discovery-history-head">
                    <strong>任务 #{item.id}</strong>
                    <span className={`discovery-status-pill ${statusMeta[item.status].tone}`}>
                      {statusMeta[item.status].label}
                    </span>
                  </div>
                  <span>
                    新增 {item.addedGames}/{item.targetAddedGames} · 已检查 {item.scannedApps} · 失败 {item.failedGames}
                  </span>
                  <span>{`拉取方式：${discoverySyncModeLabel(item.syncMode)}`}</span>
                  <span>{describeCompletionReason(item)}</span>
                  <p>{formatDateTime(item.finishedAt ?? item.updatedAt)}</p>
                </div>
              ))}
            </div>
          ) : (
            <p className="settings-hint">暂无历史任务。</p>
          )}
        </SubPanel>
      </div>
    </section>
  );
}
