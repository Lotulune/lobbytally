import { useState } from "react";
import { previewSteamAppList } from "../../api/client";
import { DiscoveryTaskPanel } from "../../features/discovery/DiscoveryTaskPanel";
import type {
  AiAnalysisQueueFailureItem,
  DashboardPayload,
  SaveConfigRequest,
  SyncMode,
  SteamAppListPreview,
} from "../../types";

type SectionKey = "apiKeys" | "llmConfig" | "sync" | "aiBatch" | "discovery";
const DEFAULT_CLASSIC_DISCOVERY_MAX_PAGES = 3;

function SettingsSection({
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
    <div className="settings-section">
      <button
        type="button"
        className="settings-section-header"
        aria-expanded={expanded}
        onClick={onToggle}
      >
        <div className="settings-section-title">
          <h3>{title}</h3>
          {status && <span className="settings-section-status">{status}</span>}
        </div>
        <span className="settings-section-chevron">{expanded ? "▼" : "▶"}</span>
      </button>
      {expanded && <div className="settings-section-body">{children}</div>}
    </div>
  );
}

export function SettingsPage({
  config,
  isBusy,
  onRefreshAllAnalyses,
  onRetryAiAnalysisJob,
  onStartClassicDiscovery,
  status,
  stats,
  aiAnalysisQueueFailures,
  onRefreshDashboard,
  onStatus,
  onSave,
  onSync,
}: {
  config: DashboardPayload["config"];
  isBusy: boolean;
  onRefreshAllAnalyses: (concurrency: number) => Promise<void>;
  onRetryAiAnalysisJob: (appid: number) => Promise<void>;
  onStartClassicDiscovery: (maxPages: number) => Promise<void>;
  status: string;
  stats: DashboardPayload["stats"];
  aiAnalysisQueueFailures: AiAnalysisQueueFailureItem[];
  onRefreshDashboard: () => Promise<unknown>;
  onStatus: (message: string) => void;
  onSave: (request: SaveConfigRequest) => Promise<void>;
  onSync: (mode: SyncMode) => void;
}) {
  const [form, setForm] = useState<SaveConfigRequest>({
    llmBaseUrl: config.llmBaseUrl,
    llmModel: config.llmModel,
    country: config.country,
    language: config.language,
    aiBatchRefreshConcurrency: config.aiBatchRefreshConcurrency,
  });
  const [preview, setPreview] = useState<SteamAppListPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [isPreviewing, setIsPreviewing] = useState(false);
  const [classicDiscoveryMaxPages, setClassicDiscoveryMaxPages] = useState(
    DEFAULT_CLASSIC_DISCOVERY_MAX_PAGES,
  );
  const [expanded, setExpanded] = useState<Record<SectionKey, boolean>>({
    apiKeys: false,
    llmConfig: true,
    sync: false,
    aiBatch: false,
    discovery: false,
  });

  const toggle = (key: SectionKey) =>
    setExpanded((prev) => ({ ...prev, [key]: !prev[key] }));

  const hasSyncResume = !stats.syncRunning && stats.syncPendingCount > 0;
  const hasSyncActivity =
    stats.syncRunning || stats.syncPendingCount > 0 || stats.syncTotalCount > 0;
  const syncProgressPercent =
    stats.syncTotalCount > 0
      ? Math.round((stats.syncProcessedCount / stats.syncTotalCount) * 100)
      : 0;
  const hasAiBatchRefreshActivity =
    stats.aiBatchRefreshRunning ||
    stats.aiBatchRefreshTotalCount > 0 ||
    stats.aiBatchRefreshFailedPendingReviewCount > 0;
  const aiBatchRefreshProgressPercent =
    stats.aiBatchRefreshTotalCount > 0
      ? Math.round(
          (stats.aiBatchRefreshProcessedCount / stats.aiBatchRefreshTotalCount) * 100,
        )
      : 0;
  const syncStatusLabel = describeSyncStatus(stats);
  const aiBatchRefreshStatusLabel = describeAiBatchRefreshStatus(stats);
  const classicDiscoveryStatusLabel = describeClassicDiscoveryStatus(stats);
  const { fullLabel, quickLabel } = syncActionLabels(stats);
  const batchRefreshConcurrency = clampBatchRefreshConcurrency(
    form.aiBatchRefreshConcurrency ?? config.aiBatchRefreshConcurrency,
  );
  const classicDiscoveryActionMaxPages = clampClassicDiscoveryMaxPages(
    classicDiscoveryMaxPages,
  );

  async function handlePreviewSteamApps() {
    setIsPreviewing(true);
    setPreviewError(null);
    try {
      setPreview(await previewSteamAppList(12));
    } catch (error) {
      setPreviewError(error instanceof Error ? error.message : String(error));
    } finally {
      setIsPreviewing(false);
    }
  }

  const apiKeyStatus = [
    config.steamApiKeyConfigured ? "Steam" : "",
    config.llmApiKeyConfigured ? "LLM" : "",
  ]
    .filter(Boolean)
    .join(" + ");

  return (
    <section className="settings-page">
      <h2>设置</h2>

      {/* API 密钥 */}
      <SettingsSection
        title="API 密钥"
        status={apiKeyStatus || "未配置"}
        expanded={expanded.apiKeys}
        onToggle={() => toggle("apiKeys")}
      >
        <p className="settings-card-desc">
          Steam Key 用于同步应用列表与数据；LLM Key 用于 AI 分析文案增强。
        </p>
        <div className="settings-form-stack">
          <label>
            Steam Web API Key
            <input
              onChange={(event) => setForm({ ...form, steamApiKey: event.currentTarget.value })}
              placeholder={
                config.steamApiKeyConfigured ? "已配置，输入新值可覆盖" : "输入 Steam Web API Key"
              }
              type="password"
            />
          </label>
          <label>
            LLM API Key
            <input
              onChange={(event) => setForm({ ...form, llmApiKey: event.currentTarget.value })}
              placeholder={
                config.llmApiKeyConfigured
                  ? "已配置，输入新值可覆盖"
                  : "输入 DeepSeek / OpenAI / Anthropic API Key"
              }
              type="password"
            />
          </label>
        </div>
        <p className="settings-hint">
          Steam Key 与 LLM Key 仅保存在本机 SQLite，不会上传至任何服务器。
        </p>
      </SettingsSection>

      {/* LLM 配置 */}
      <SettingsSection
        title="LLM 配置"
        status={form.llmModel || config.llmModel || "未设置"}
        expanded={expanded.llmConfig}
        onToggle={() => toggle("llmConfig")}
      >
        <p className="settings-card-desc">
          默认提供方是 DeepSeek，同时兼容 OpenAI 的 <code>chat/completions</code> 和 Anthropic 的 <code>messages</code> 格式。
        </p>
        <div className="settings-grid">
          <label>
            LLM Base URL
            <input
              value={form.llmBaseUrl ?? ""}
              onChange={(event) =>
                setForm({ ...form, llmBaseUrl: event.currentTarget.value })
              }
            />
          </label>
          <label>
            模型
            <input
              value={form.llmModel ?? ""}
              onChange={(event) =>
                setForm({ ...form, llmModel: event.currentTarget.value })
              }
            />
          </label>
          <label>
            地区
            <input
              value={form.country ?? ""}
              onChange={(event) =>
                setForm({ ...form, country: event.currentTarget.value })
              }
            />
          </label>
          <label>
            语言
            <input
              value={form.language ?? ""}
              onChange={(event) =>
                setForm({ ...form, language: event.currentTarget.value })
              }
            />
          </label>
        </div>
        <div className="settings-copy-block settings-copy-block-muted">
          <p className="settings-hint">常见 Base URL 示例：</p>
          <ul className="settings-provider-list">
            <li>
              <span>DeepSeek OpenAI 兼容</span>
              <code>https://api.deepseek.com</code>
              <code>https://api.deepseek.com/v1</code>
            </li>
            <li>
              <span>DeepSeek Anthropic 兼容</span>
              <code>https://api.deepseek.com/anthropic</code>
            </li>
            <li>
              <span>官方 Anthropic</span>
              <code>https://api.anthropic.com</code>
            </li>
          </ul>
        </div>
        <div className="settings-card-actions">
          <button className="gold-button" disabled={isBusy} type="button" onClick={() => onSave(form)}>
            保存设置
          </button>
        </div>
      </SettingsSection>

      {/* 数据同步 */}
      <SettingsSection
        title="数据同步"
        status={syncStatusLabel}
        expanded={expanded.sync}
        onToggle={() => toggle("sync")}
      >
        <p className="settings-card-desc">
          当前库：{formatNumber(stats.totalGames)} 个游戏；最近同步：{formatDateTime(stats.lastSyncAt)}
        </p>
        <div className="settings-card-actions">
          <button
            className="gold-button"
            disabled={isBusy || stats.syncRunning}
            type="button"
            onClick={() => onSync("full")}
          >
            {fullLabel}
          </button>
          <button
            className="ghost-button"
            disabled={isBusy || stats.syncRunning}
            type="button"
            onClick={() => onSync("quick")}
          >
            {quickLabel}
          </button>
          <button
            className="muted-button"
            type="button"
            onClick={handlePreviewSteamApps}
            disabled={isPreviewing || isBusy}
          >
            {isPreviewing ? "读取中…" : "预览 Steam AppList"}
          </button>
        </div>
        <div className="backfill-status-block compact">
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
        {previewError && <p className="settings-error">{previewError}</p>}
        {preview && (
          <div className="steam-preview">
            <strong>Steam AppList 预览</strong>
            <span>
              last_appid: {preview.lastAppid ?? "无"} · more:
              {preview.haveMoreResults ? "是" : "否"}
            </span>
            <div>
              {preview.apps.slice(0, 12).map((app) => (
                <em key={app.appid}>
                  {app.name} · {app.appid}
                </em>
              ))}
            </div>
          </div>
        )}
      </SettingsSection>

      {/* AI 批量重算 */}
      <SettingsSection
        title="AI 批量重算"
        status={aiBatchRefreshStatusLabel}
        expanded={expanded.aiBatch}
        onToggle={() => toggle("aiBatch")}
      >
        <p className="settings-card-desc">
          批量重算会重新生成库内所有游戏的 AI 评分和摘要，适合算法改版后的全量回刷。
        </p>
        <label>
          AI 批量重算并发数
          <input
            aria-label="AI 批量重算并发数"
            inputMode="numeric"
            max={10}
            min={1}
            type="number"
            value={batchRefreshConcurrency}
            onChange={(event) =>
              setForm({
                ...form,
                aiBatchRefreshConcurrency: clampBatchRefreshConcurrency(
                  event.currentTarget.valueAsNumber,
                ),
              })
            }
          />
        </label>
        <p className="settings-hint">并发数支持 1-10，默认 5；常规建议 5，压测可尝试 10。</p>
        <div className="settings-card-actions">
          <button
            className="ghost-button"
            disabled={isBusy || stats.aiBatchRefreshRunning}
            type="button"
            onClick={() => void onRefreshAllAnalyses(batchRefreshConcurrency)}
          >
            {stats.aiBatchRefreshRunning ? "AI 批量重算中…" : "批量重算 AI 评分"}
          </button>
        </div>
        <div className="backfill-status-block compact">
          <div className="backfill-status-head">
            <strong>AI 批量重算</strong>
            <span>{aiBatchRefreshStatusLabel}</span>
          </div>
          {hasAiBatchRefreshActivity ? (
            <>
              <div className="discovery-progress-meta">
                <strong>进度 {aiBatchRefreshProgressPercent}%</strong>
                <span>
                  已处理 {stats.aiBatchRefreshProcessedCount}/{stats.aiBatchRefreshTotalCount}
                </span>
              </div>
              <div className="discovery-progress-track" aria-hidden="true">
                <div
                  className="discovery-progress-fill"
                  style={{ width: `${aiBatchRefreshProgressPercent}%` }}
                />
              </div>
              <div className="backfill-status-grid">
                <div>
                  <span>并发</span>
                  <strong>{formatNumber(stats.aiBatchRefreshConcurrency)}</strong>
                </div>
                <div>
                  <span>已处理</span>
                  <strong>{`${formatNumber(stats.aiBatchRefreshProcessedCount)}/${formatNumber(stats.aiBatchRefreshTotalCount)}`}</strong>
                </div>
                <div>
                  <span>剩余</span>
                  <strong>{formatNumber(stats.aiBatchRefreshPendingCount)}</strong>
                </div>
                <div>
                  <span>处理中</span>
                  <strong>{formatNumber(stats.aiBatchRefreshActiveCount)}</strong>
                </div>
                <div>
                  <span>已更新</span>
                  <strong>{formatNumber(stats.aiBatchRefreshUpdatedCount)}</strong>
                </div>
                <div>
                  <span>失败</span>
                  <strong>{formatNumber(stats.aiBatchRefreshFailedCount)}</strong>
                </div>
              </div>
              <p className="mini-status">
                {stats.aiBatchRefreshRunning
                  ? `正在按 ${stats.aiBatchRefreshConcurrency} 路并发批量重算 AI 评分。`
                  : stats.aiBatchRefreshFailedCount > 0
                    ? "本轮批量重算已完成，但有失败项。"
                    : "本轮批量重算已完成。"}
              </p>
              {stats.aiBatchRefreshLastError ? (
                <p className="settings-error">{stats.aiBatchRefreshLastError}</p>
              ) : null}
              {stats.aiBatchRefreshFailedPendingReviewCount > 0 ? (
                <div className="settings-copy-block settings-copy-block-muted">
                  <p className="settings-hint">
                    待人工处理失败项：{formatNumber(stats.aiBatchRefreshFailedPendingReviewCount)}
                  </p>
                  <div className="settings-provider-list">
                    {aiAnalysisQueueFailures.map((item) => (
                      <div key={item.appid} className="backfill-status-head">
                        <span>
                          AppID {item.appid} · 已失败 {item.attempt} 次 · {formatDateTime(item.updatedAt)}
                        </span>
                        <button
                          className="muted-button"
                          type="button"
                          disabled={isBusy}
                          onClick={() => void onRetryAiAnalysisJob(item.appid)}
                        >
                          重试
                        </button>
                      </div>
                    ))}
                  </div>
                  {aiAnalysisQueueFailures[0] ? (
                    <p className="mini-status">{aiAnalysisQueueFailures[0].lastError}</p>
                  ) : null}
                </div>
              ) : null}
            </>
          ) : (
            <p className="mini-status">点击上方按钮开始批量重算所有游戏的 AI 评分。</p>
          )}
        </div>
      </SettingsSection>

      {/* 发现任务 */}
      <SettingsSection
        title="发现任务"
        status={stats.backfillRunning ? "新游补全中" : classicDiscoveryStatusLabel}
        expanded={expanded.discovery}
        onToggle={() => toggle("discovery")}
      >
        <div className="backfill-status-block compact">
          <div className="backfill-status-head">
            <strong>精品老游补库</strong>
            <span>{classicDiscoveryStatusLabel}</span>
          </div>
          <label>
            老游补库页数
            <input
              aria-label="老游补库页数"
              inputMode="numeric"
              max={DEFAULT_CLASSIC_DISCOVERY_MAX_PAGES}
              min={1}
              type="number"
              value={classicDiscoveryActionMaxPages}
              onChange={(event) =>
                setClassicDiscoveryMaxPages(
                  clampClassicDiscoveryMaxPages(event.currentTarget.valueAsNumber),
                )
              }
            />
          </label>
          <p className="settings-hint">
            手动补库支持 1-3 页，默认 3；每页固定 100 个评论榜候选，连续 2 页无新增会提前停止。
          </p>
          <div className="settings-card-actions">
            <button
              className="ghost-button"
              disabled={isBusy || stats.classicDiscoveryRunning}
              type="button"
              onClick={() => void onStartClassicDiscovery(classicDiscoveryActionMaxPages)}
            >
              {stats.classicDiscoveryRunning ? "老游补库中…" : "启动老游补库"}
            </button>
          </div>
          <div className="backfill-status-grid">
            <div>
              <span>已扫描</span>
              <strong>{formatNumber(stats.classicDiscoveryScannedApps)}</strong>
            </div>
            <div>
              <span>已新增</span>
              <strong>{formatNumber(stats.classicDiscoveryAddedGames)}</strong>
            </div>
            <div>
              <span>已拒绝</span>
              <strong>{formatNumber(stats.classicDiscoveryRejectedGames)}</strong>
            </div>
            <div>
              <span>跳过已存在</span>
              <strong>{formatNumber(stats.classicDiscoverySkippedExisting)}</strong>
            </div>
            <div>
              <span>跳过拒绝缓存</span>
              <strong>{formatNumber(stats.classicDiscoverySkippedRejectedCache)}</strong>
            </div>
            <div>
              <span>最近完成</span>
              <strong>{formatDateTime(stats.classicDiscoveryLastCompletedAt)}</strong>
            </div>
          </div>
          <p className="mini-status">
            {stats.classicDiscoveryRunning
              ? `正在扫描 AppID ${stats.classicDiscoveryCurrentAppid ?? "未知"}；当前评论榜页进度 ${stats.classicDiscoveryScannedApps} 个候选。`
              : "老游补库会在新游发现结束且新游补全清空后启动；不必等待新游 AI 清空，但老游 AI 仍会排在新游 AI 后面。"}
          </p>
        </div>
        <DiscoveryTaskPanel
          stats={stats}
          onRefreshDashboard={onRefreshDashboard}
          onStatus={onStatus}
        />
      </SettingsSection>

      <p className="mini-status">{status}</p>
    </section>
  );
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

function describeSyncStatus(stats: DashboardPayload["stats"]) {
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

function describeAiBatchRefreshStatus(stats: DashboardPayload["stats"]) {
  if (stats.aiBatchRefreshRunning) {
    return "进行中";
  }

  if (stats.aiBatchRefreshTotalCount > 0) {
    return stats.aiBatchRefreshFailedCount > 0 ? "已完成（含失败）" : "已完成";
  }

  return "空闲";
}

function describeClassicDiscoveryStatus(stats: DashboardPayload["stats"]) {
  if (stats.classicDiscoveryRunning) {
    return "进行中";
  }
  if (stats.classicDiscoveryStatus === "interrupted") {
    return "待续跑";
  }
  if (stats.classicDiscoveryStatus === "failed") {
    return "已失败";
  }
  if (stats.classicDiscoveryStatus === "completed") {
    return "已完成";
  }
  return "空闲";
}

function syncModeLabel(mode?: SyncMode | null) {
  return mode === "quick" ? "快速同步" : "完整同步";
}

function syncActionLabels(stats: DashboardPayload["stats"]) {
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

function clampBatchRefreshConcurrency(value: number | undefined) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return 5;
  }

  return Math.min(10, Math.max(1, Math.round(value)));
}

function clampClassicDiscoveryMaxPages(value: number | undefined) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return DEFAULT_CLASSIC_DISCOVERY_MAX_PAGES;
  }

  return Math.min(DEFAULT_CLASSIC_DISCOVERY_MAX_PAGES, Math.max(1, Math.round(value)));
}
