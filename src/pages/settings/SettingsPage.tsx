import { useState } from "react";
import { previewSteamAppList } from "../../api/client";
import { DiscoveryTaskPanel } from "../../features/discovery/DiscoveryTaskPanel";
import type {
  DashboardPayload,
  SaveConfigRequest,
  SyncMode,
  SteamAppListPreview,
} from "../../types";

export function SettingsPage({
  config,
  isBusy,
  status,
  stats,
  onRefreshDashboard,
  onStatus,
  onSave,
  onSync,
}: {
  config: DashboardPayload["config"];
  isBusy: boolean;
  status: string;
  stats: DashboardPayload["stats"];
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
  });
  const [preview, setPreview] = useState<SteamAppListPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [isPreviewing, setIsPreviewing] = useState(false);
  const hasSyncResume = !stats.syncRunning && stats.syncPendingCount > 0;
  const hasSyncActivity =
    stats.syncRunning || stats.syncPendingCount > 0 || stats.syncTotalCount > 0;
  const syncProgressPercent =
    stats.syncTotalCount > 0
      ? Math.round((stats.syncProcessedCount / stats.syncTotalCount) * 100)
      : 0;
  const syncStatusLabel = describeSyncStatus(stats);
  const { fullLabel, quickLabel } = syncActionLabels(stats);

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

  return (
    <section className="settings-page">
      <h2>设置</h2>
      <p>
        Steam Key 用于同步应用列表与数据；LLM Key 用于 AI 分析文案增强。默认使用
        DeepSeek，也兼容 OpenAI `chat/completions` 与 Anthropic `messages` 格式。
      </p>
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
      <p className="settings-hint">
        常见示例：DeepSeek OpenAI 兼容可填 `https://api.deepseek.com` 或
        `https://api.deepseek.com/v1`；DeepSeek Anthropic 兼容可填
        `https://api.deepseek.com/anthropic`；官方 Anthropic 可填
        `https://api.anthropic.com`。
      </p>
      <p className="settings-hint">
        当前库：{formatNumber(stats.totalGames)} 个游戏；最近同步：
        {formatDateTime(stats.lastSyncAt)}；Steam Key 与 LLM Key 仅保存在本机 SQLite。
      </p>
      <div className="settings-actions">
        <button className="gold-button" disabled={isBusy} type="button" onClick={() => onSave(form)}>
          保存设置
        </button>
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
      <p className="mini-status">{status}</p>
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
      <DiscoveryTaskPanel
        stats={stats}
        onRefreshDashboard={onRefreshDashboard}
        onStatus={onStatus}
      />
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
