import { useEffect, useMemo, useState } from "react";
import {
  getDefaultLlmBaseUrl,
  getDefaultLlmModel,
  previewSteamAppList,
  validateLlmConfig,
  validateSteamConfig,
} from "../../api/client";
import { DiscoveryTaskPanel } from "../../features/discovery/DiscoveryTaskPanel";
import type {
  AiAnalysisQueueFailureItem,
  ConnectionValidationResult,
  DashboardPayload,
  LlmProvider,
  SaveConfigRequest,
  SyncMode,
  SteamAppListPreview,
} from "../../types";

export type SettingsSectionKey =
  | "serviceConnection"
  | "onboarding"
  | "apiKeys"
  | "llmConfig"
  | "sync"
  | "aiBatch"
  | "discovery";
export type SettingsExpandedState = Record<SettingsSectionKey, boolean>;
const DEFAULT_CLASSIC_DISCOVERY_MAX_PAGES = 3;

export const defaultSettingsExpandedState: SettingsExpandedState = {
  serviceConnection: false,
  onboarding: false,
  apiKeys: false,
  llmConfig: false,
  sync: false,
  aiBatch: false,
  discovery: false,
};

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
  onOpenOnboarding,
  onRefreshAllAnalyses,
  onRetryAiAnalysisJob,
  onStartClassicDiscovery,
  status,
  stats,
  aiAnalysisQueueFailures,
  onRefreshDashboard,
  onStatus,
  onImportServiceConnectionFile,
  onSave,
  onSync,
  expandedSections,
  onExpandedSectionsChange,
}: {
  config: DashboardPayload["config"];
  isBusy: boolean;
  onOpenOnboarding: () => void;
  onRefreshAllAnalyses: (concurrency: number) => Promise<void>;
  onRetryAiAnalysisJob: (appid: number) => Promise<void>;
  onStartClassicDiscovery: (maxPages: number) => Promise<void>;
  status: string;
  stats: DashboardPayload["stats"];
  aiAnalysisQueueFailures: AiAnalysisQueueFailureItem[];
  onRefreshDashboard: () => Promise<unknown>;
  onStatus: (message: string) => void;
  onImportServiceConnectionFile: (fileText: string) => Promise<void>;
  onSave: (request: SaveConfigRequest) => Promise<void>;
  onSync: (mode: SyncMode) => void;
  expandedSections?: SettingsExpandedState;
  onExpandedSectionsChange?: (expanded: SettingsExpandedState) => void;
}) {
  const [form, setForm] = useState<SaveConfigRequest>(() => buildFormFromConfig(config));
  const [preview, setPreview] = useState<SteamAppListPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [isPreviewing, setIsPreviewing] = useState(false);
  const [isTestingSteam, setIsTestingSteam] = useState(false);
  const [isTestingLlm, setIsTestingLlm] = useState(false);
  const [steamValidation, setSteamValidation] = useState<ConnectionValidationResult | null>(
    config.steamApiKeyValidated
      ? {
          success: true,
          message: "当前已保存的 Steam Key 最近一次测试成功。",
        }
      : null,
  );
  const [llmValidation, setLlmValidation] = useState<ConnectionValidationResult | null>(
    config.llmConfigValidated
      ? {
          success: true,
          message: "当前已保存的 AI 配置最近一次测试成功。",
          provider: config.llmProvider,
          baseUrl: config.llmBaseUrl,
          model: config.llmModel,
        }
      : null,
  );
  const [classicDiscoveryMaxPages, setClassicDiscoveryMaxPages] = useState(
    DEFAULT_CLASSIC_DISCOVERY_MAX_PAGES,
  );
  const [localExpanded, setLocalExpanded] = useState<SettingsExpandedState>(
    defaultSettingsExpandedState,
  );
  const expanded = expandedSections ?? localExpanded;
  const isPublicServiceMode = stats.sourceKind === "public_service";

  useEffect(() => {
    setForm(buildFormFromConfig(config));
    setSteamValidation(
      config.steamApiKeyValidated
        ? {
            success: true,
            message: "当前已保存的 Steam Key 最近一次测试成功。",
          }
        : null,
    );
    setLlmValidation(
      config.llmConfigValidated
        ? {
            success: true,
            message: "当前已保存的 AI 配置最近一次测试成功。",
            provider: config.llmProvider,
            baseUrl: config.llmBaseUrl,
            model: config.llmModel,
          }
        : null,
    );
  }, [config]);

  const toggle = (key: SettingsSectionKey) => {
    const nextExpanded = { ...expanded, [key]: !expanded[key] };
    if (onExpandedSectionsChange) {
      onExpandedSectionsChange(nextExpanded);
      return;
    }

    setLocalExpanded(nextExpanded);
  };

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
  const steamStatus = describeCredentialStatus(
    config.steamApiKeyConfigured,
    config.steamApiKeyValidated,
  );
  const llmStatus = describeCredentialStatus(
    config.llmApiKeyConfigured,
    config.llmConfigValidated,
  );
  const llmProvider = form.llmProvider ?? config.llmProvider;
  const showAdvanced = llmProvider === "custom";
  const steamDraftKey = form.steamApiKey?.trim() ?? "";
  const canTestSteam = Boolean(steamDraftKey || config.steamApiKeyConfigured);
  const llmDraftKey = form.llmApiKey?.trim() ?? "";
  const llmProviderChanged = llmProvider !== config.llmProvider;
  const canUseSavedLlmKey = config.llmApiKeyConfigured && !llmProviderChanged && !form.clearLlmApiKey;
  const canTestLlm = Boolean(llmDraftKey || canUseSavedLlmKey);
  const serviceConnectionStatus = isPublicServiceMode ? "已连接" : "未连接";

  const onboardingSummary = useMemo(() => {
    const readyCount = Number(config.steamApiKeyValidated) + Number(config.llmConfigValidated);
    return config.onboardingCompleted ? "已完成" : `${readyCount}/2 已验证`;
  }, [config]);

  if (isPublicServiceMode) {
    return (
      <section className="settings-page">
        <h2>设置</h2>
        <div className="settings-section">
          <div className="settings-section-body">
            <div className="backfill-status-block compact">
              <div className="backfill-status-head">
                <strong>公共发现服务</strong>
                <span>只读连接</span>
              </div>
              <div className="backfill-status-grid">
                <div>
                  <span>数据源</span>
                  <strong>{stats.dataSource}</strong>
                </div>
                <div>
                  <span>公共库数量</span>
                  <strong>{formatNumber(stats.totalGames)}</strong>
                </div>
                <div>
                  <span>个人状态</span>
                  <strong>本地保存</strong>
                </div>
              </div>
            </div>
          </div>
        </div>
        <p className="mini-status">{status}</p>
      </section>
    );
  }

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

  async function handleValidateSteamDraft() {
    const draft = form.steamApiKey?.trim();
    if (!draft && !config.steamApiKeyConfigured) {
      onStatus("请先输入当前要测试的 Steam Web API Key。");
      return;
    }

    setIsTestingSteam(true);
    try {
      const result = await validateSteamConfig({ steamApiKey: draft || undefined });
      setSteamValidation(result);
      onStatus(result.message);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setSteamValidation({
        success: false,
        message,
        diagnostic: "可以先保存，再回来继续测试。",
      });
      onStatus(message);
    } finally {
      setIsTestingSteam(false);
    }
  }

  async function handleValidateLlmDraft() {
    const draftKey = form.llmApiKey?.trim();
    const draftBaseUrl = (form.llmBaseUrl ?? config.llmBaseUrl).trim();
    const draftModel = (form.llmModel ?? config.llmModel).trim();
    if (!draftKey && !canUseSavedLlmKey) {
      onStatus("请先输入当前要测试的 AI API Key。");
      return;
    }

    setIsTestingLlm(true);
    try {
      const result = await validateLlmConfig({
        provider: llmProvider,
        apiKey: draftKey || undefined,
        baseUrl: draftBaseUrl,
        model: draftModel,
      });
      setLlmValidation(result);
      onStatus(result.message);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setLlmValidation({
        success: false,
        message,
        diagnostic: "可以先保存，再回来继续测试。",
        provider: llmProvider,
        baseUrl: draftBaseUrl,
        model: draftModel,
      });
      onStatus(message);
    } finally {
      setIsTestingLlm(false);
    }
  }

  async function handleSaveAll() {
    const nextRequest: SaveConfigRequest = {
      ...form,
      clearLlmApiKey:
        form.clearLlmApiKey && !form.llmApiKey?.trim() ? true : undefined,
      steamApiKeyValidated:
        form.steamApiKey?.trim() || steamValidation?.success === false
          ? Boolean(steamValidation?.success)
          : undefined,
      llmConfigValidated:
        form.llmApiKey?.trim() ||
        llmProvider !== config.llmProvider ||
        (form.llmBaseUrl ?? config.llmBaseUrl) !== config.llmBaseUrl ||
        (form.llmModel ?? config.llmModel) !== config.llmModel
          ? Boolean(llmValidation?.success)
          : undefined,
    };

    await onSave(nextRequest);
    setForm(buildFormFromConfig({ ...config, ...nextRequest } as DashboardPayload["config"]));
  }

  async function handleImportServiceConnectionFile(file: File | null) {
    if (!file) {
      return;
    }

    try {
      await onImportServiceConnectionFile(await file.text());
    } catch (error) {
      onStatus(error instanceof Error ? error.message : String(error));
    }
  }

  function handleProviderChange(provider: LlmProvider) {
    setForm((current) => ({
      ...current,
      llmProvider: provider,
      llmApiKey: "",
      llmBaseUrl: getDefaultLlmBaseUrl(provider),
      llmModel: getDefaultLlmModel(provider),
      clearLlmApiKey: config.llmApiKeyConfigured,
    }));
    setLlmValidation(null);
  }

  return (
    <section className="settings-page">
      <h2>设置</h2>

      <SettingsSection
        title="公共发现服务连接"
        status={serviceConnectionStatus}
        expanded={expanded.serviceConnection}
        onToggle={() => toggle("serviceConnection")}
      >
        <p className="settings-card-desc">
          导入维护者提供的无密钥连接文件后，客户端仍会重新读取服务身份信息并验证匿名公共读取能力。
        </p>
        <div className="settings-form-stack">
          <label>
            导入服务连接文件
            <input
              accept="application/json,.json"
              type="file"
              onChange={(event) => {
                const file = event.currentTarget.files?.[0] ?? null;
                event.currentTarget.value = "";
                void handleImportServiceConnectionFile(file);
              }}
            />
          </label>
        </div>
        <p className="settings-hint">
          连接文件不能包含引导令牌、管理员令牌或第三方 API Key；导入不会跳过服务身份验证。
        </p>
      </SettingsSection>

      <SettingsSection
        title="初始化向导"
        status={onboardingSummary}
        expanded={expanded.onboarding}
        onToggle={() => toggle("onboarding")}
      >
        <p className="settings-card-desc">
          首次引导会带你完成 Steam 与 AI 配置。你可以随时从这里继续初始化或重新打开整个向导。
        </p>
        <div className="settings-copy-block settings-copy-block-muted">
          <div className="backfill-status-grid">
            <div>
              <span>Steam</span>
              <strong>{steamStatus}</strong>
            </div>
            <div>
              <span>AI</span>
              <strong>{llmStatus}</strong>
            </div>
            <div>
              <span>下次进入步骤</span>
              <strong>{config.onboardingCurrentStep}</strong>
            </div>
            <div>
              <span>默认提供方</span>
              <strong>{providerLabel(config.onboardingLlmProviderDraft)}</strong>
            </div>
          </div>
        </div>
        <div className="settings-card-actions">
          <button className="gold-button" type="button" onClick={onOpenOnboarding}>
            {config.onboardingCompleted ? "重新打开向导" : "继续初始化"}
          </button>
          <button
            className="ghost-button"
            type="button"
            disabled={!canTestSteam || isTestingSteam}
            onClick={() => void handleValidateSteamDraft()}
          >
            {isTestingSteam ? "Steam 测试中…" : "测试 Steam 连接"}
          </button>
          <button
            className="ghost-button"
            type="button"
            disabled={!canTestLlm || isTestingLlm}
            onClick={() => void handleValidateLlmDraft()}
          >
            {isTestingLlm ? "AI 测试中…" : "测试 AI 连接"}
          </button>
        </div>
      </SettingsSection>

      <SettingsSection
        title="API 密钥"
        status={`${steamStatus} / ${llmStatus}`}
        expanded={expanded.apiKeys}
        onToggle={() => toggle("apiKeys")}
      >
        <p className="settings-card-desc">
          Steam Key 用于同步应用列表与数据；AI Key 用于推荐、摘要和分析增强。保存不等于验证成功，验证状态会单独记录。
        </p>
        <div className="settings-form-stack">
          <label>
            Steam Web API Key
            <input
              onChange={(event) => {
                setForm({ ...form, steamApiKey: event.currentTarget.value });
                setSteamValidation(null);
              }}
              placeholder={
                config.steamApiKeyConfigured ? "已配置，输入新值可覆盖" : "输入 Steam Web API Key"
              }
              type="password"
            />
          </label>
          <label>
            {providerLabel(llmProvider)} API Key
            <input
              value={form.llmApiKey ?? ""}
              onChange={(event) => {
                setForm({
                  ...form,
                  llmApiKey: event.currentTarget.value,
                  clearLlmApiKey: undefined,
                });
                setLlmValidation(null);
              }}
              placeholder={
                config.llmApiKeyConfigured ? "已配置，输入新值可覆盖" : "输入 API Key"
              }
              type="password"
            />
          </label>
        </div>
        <p className="settings-hint">
          Steam Key 与 AI Key 仅保存在本机 SQLite，不会上传至任何服务器。
        </p>
        <ValidationBlock title="Steam 验证结果" result={steamValidation} />
        <ValidationBlock title="AI 验证结果" result={llmValidation} />
      </SettingsSection>

      <SettingsSection
        title="LLM 配置"
        status={`${providerLabel(llmProvider)} / ${llmStatus}`}
        expanded={expanded.llmConfig}
        onToggle={() => toggle("llmConfig")}
      >
        <p className="settings-card-desc">
          默认提供方是 DeepSeek。切换提供方时会同步覆盖 Base URL / 模型，并清空旧 API Key，等待你重新输入和验证。
        </p>
        <div className="settings-grid">
          <label>
            AI 提供方
            <select
              value={llmProvider}
              onChange={(event) => handleProviderChange(event.currentTarget.value as LlmProvider)}
            >
              <option value="deepseek">DeepSeek</option>
              <option value="openai">OpenAI</option>
              <option value="anthropic">Anthropic</option>
              <option value="custom">Custom</option>
            </select>
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
        <div className="settings-form-stack">
          <label>
            Base URL
            <input
              value={form.llmBaseUrl ?? ""}
              onChange={(event) => {
                setForm({ ...form, llmBaseUrl: event.currentTarget.value });
                setLlmValidation(null);
              }}
            />
          </label>
          <label>
            模型
            <input
              value={form.llmModel ?? ""}
              onChange={(event) => {
                setForm({ ...form, llmModel: event.currentTarget.value });
                setLlmValidation(null);
              }}
            />
          </label>
        </div>
        {!showAdvanced ? (
          <p className="settings-hint">
            当前是标准提供方，通常只需要填写 API Key。如果你要改高级参数，也可以直接在这里覆盖。
          </p>
        ) : null}
        <div className="settings-copy-block settings-copy-block-muted">
          <p className="settings-hint">常见默认值：</p>
          <ul className="settings-provider-list">
            <li>
              <span>DeepSeek</span>
              <code>https://api.deepseek.com</code>
              <code>deepseek-v4-flash</code>
            </li>
            <li>
              <span>OpenAI</span>
              <code>https://api.openai.com/v1</code>
              <code>gpt-4.1</code>
            </li>
            <li>
              <span>Anthropic</span>
              <code>https://api.anthropic.com</code>
              <code>claude-sonnet-4-20250514</code>
            </li>
          </ul>
        </div>
        <div className="settings-card-actions">
          <button className="gold-button" disabled={isBusy} type="button" onClick={() => void handleSaveAll()}>
            保存设置
          </button>
          <button
            className="ghost-button"
            disabled={!canTestLlm || isTestingLlm}
            type="button"
            onClick={() => void handleValidateLlmDraft()}
          >
            {isTestingLlm ? "AI 测试中…" : "测试当前草稿"}
          </button>
        </div>
      </SettingsSection>

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
            onClick={() => void handlePreviewSteamApps()}
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

function ValidationBlock({
  title,
  result,
}: {
  title: string;
  result: ConnectionValidationResult | null;
}) {
  if (!result) {
    return null;
  }

  return (
    <div className="settings-copy-block settings-copy-block-muted">
      <strong>{title}</strong>
      <p className={result.success ? "mini-status" : "settings-error"}>{result.message}</p>
      {result.diagnostic ? <p className="mini-status">{result.diagnostic}</p> : null}
      <div className="settings-provider-list">
        {typeof result.latencyMs === "number" ? <span>延迟 {result.latencyMs}ms</span> : null}
        {typeof result.appCount === "number" ? <span>预览 {result.appCount} 条</span> : null}
        {result.model ? <span>模型 {result.model}</span> : null}
      </div>
    </div>
  );
}

function buildFormFromConfig(config: DashboardPayload["config"]): SaveConfigRequest {
  return {
    llmProvider: config.llmProvider,
    llmBaseUrl: config.llmBaseUrl,
    llmModel: config.llmModel,
    country: config.country,
    language: config.language,
    aiBatchRefreshConcurrency: config.aiBatchRefreshConcurrency,
  };
}

function providerLabel(provider: LlmProvider) {
  switch (provider) {
    case "openai":
      return "OpenAI";
    case "anthropic":
      return "Anthropic";
    case "custom":
      return "Custom";
    case "deepseek":
    default:
      return "DeepSeek";
  }
}

function describeCredentialStatus(configured: boolean, validated: boolean) {
  if (validated) return "已保存 / 已验证";
  if (configured) return "已保存 / 未验证";
  return "未配置";
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
