import { useEffect, useState } from "react";
import { ApiError } from "../api/client";
import type { AiSettings } from "../api/types";
import { apiClient } from "../app/runtime";
import {
  buildEasyRoutePlan,
  taskLabel,
  type CustomRoutingPreset,
  type CustomTaskRoute,
  type EasyRoutePlan,
} from "../app/customAiRoutes";
import {
  loadLocalCustomAiSettings,
  removeLocalCustomAiSettings,
  saveLocalCustomAiSettings,
  type LocalCustomAiSettings,
} from "../app/localAiSettings";
import { useToast } from "../app/ToastProvider";

type Mode = AiSettings["mode"];

function aiErrorMessage(error: unknown, action: "load" | "test" | "save" | "delete" | "discover"): string {
  if (error instanceof ApiError) {
    if (error.code === "temporarily_unavailable") {
      return "服务端尚未启用自定义 AI 凭据加密，请联系管理员配置后重试。";
    }
    if (error.code === "ai_connection_failed") {
      return "连接测试失败，请检查 HTTPS 地址、模型名称和 API Key。";
    }
    if (error.code === "unauthenticated") return "登录状态已失效，请重新登录。";
    if (error.code === "invalid_argument") return `配置不正确：${error.message}`;
    if (error.offline) return "当前无法连接服务器，请检查网络。";
    return `${error.message}（${error.code}）`;
  }
  if (action === "load") return "无法加载 AI 设置。";
  if (action === "test") return "无法连接到该服务。";
  if (action === "discover") return "无法探测上游模型列表。";
  if (action === "delete") return "无法删除自定义密钥。";
  return "AI 设置保存失败。";
}

function RouteTable({
  routes,
  title,
  note,
}: {
  routes: Array<{
    task: string;
    primary_model: string;
    fallback_models: string[];
    primary_available?: boolean;
  }>;
  title: string;
  note: string;
}) {
  return (
    <div className="ai-routes" aria-label={title}>
      <p className="cal-note">{note}</p>
      <p className="cal-note">
        <strong>箭头含义：</strong>左侧是该任务<strong>优先使用的主模型</strong>；
        <code> → </code>后面是<strong>回退模型</strong>（主模型失败/限流/不可用时依次尝试，全部失败则用确定性结果，不会假装 AI 成功）。
      </p>
      <ul className="ai-route-list">
        {routes.map((route) => (
          <li key={route.task} className="ai-route-row">
            <strong title={route.task}>{taskLabel(route.task)}</strong>
            <span
              className="chip accent"
              title={
                route.primary_available === false
                  ? "上游未发现此模型，运行时会跳过"
                  : "主模型（优先）"
              }
            >
              主 {route.primary_model}
            </span>
            {route.fallback_models.map((fb) => (
              <span key={fb} className="chip" title="回退模型">
                → 回退 {fb}
              </span>
            ))}
          </li>
        ))}
      </ul>
    </div>
  );
}

export function AiSettingsScreen({ embedded = false }: { embedded?: boolean }) {
  const toast = useToast();
  const [settings, setSettings] = useState<AiSettings | null>(null);
  const [mode, setMode] = useState<Mode>("builtin");
  const [baseUrl, setBaseUrl] = useState("");
  const [model, setModel] = useState("");
  const [fallbackModel, setFallbackModel] = useState("");
  const [routingPreset, setRoutingPreset] = useState<CustomRoutingPreset>("easy");
  const [routes, setRoutes] = useState<CustomTaskRoute[]>([]);
  const [planNotes, setPlanNotes] = useState<string[]>([]);
  const [apiKey, setApiKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [localCustom, setLocalCustom] = useState<LocalCustomAiSettings | null>(null);

  const applySettings = (next: AiSettings) => {
    setSettings(next);
    setMode(next.mode);
    setBaseUrl(next.base_url ?? "");
    setModel(next.model ?? "");
  };

  useEffect(() => {
    let cancelled = false;
    const userId = apiClient.sessionUserId();
    void Promise.all([
      apiClient.getAiSettings(),
      userId ? loadLocalCustomAiSettings(userId) : Promise.resolve(null),
    ])
      .then(([next, local]) => {
        if (!cancelled) {
          setLocalCustom(local);
          applySettings(
            local
              ? {
                  ...next,
                  mode: "custom",
                  provider: "openai_compat",
                  base_url: local.baseUrl,
                  model: local.model,
                  configured: true,
                  key_mask: "本机安全存储",
                }
              : next,
          );
          if (local) {
            setBaseUrl(local.baseUrl);
            setModel(local.model);
            setFallbackModel(local.fallbackModel ?? "");
            setRoutingPreset(local.routingPreset);
            setRoutes(local.routes ?? []);
          }
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          const message = aiErrorMessage(error, "load");
          setErrorMessage(message);
          toast.show(message);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [toast]);

  const applyPlan = (plan: EasyRoutePlan) => {
    setRoutingPreset(plan.preset);
    setModel(plan.model);
    setFallbackModel(plan.fallback_model ?? "");
    setRoutes(plan.routes);
    setPlanNotes(plan.notes);
  };

  const save = async () => {
    setErrorMessage(null);
    setBusy(true);
    try {
      const userId = apiClient.sessionUserId();
      const effectiveKey = apiKey || localCustom?.apiKey || "";
      if (mode === "custom" && (!userId || !effectiveKey)) {
        throw new ApiError({
          code: "invalid_argument",
          status: 400,
          message: "API Key 不能为空",
        });
      }
      const next = await apiClient.putAiSettings(
        mode === "custom"
          ? {
              mode,
              provider: "openai_compat",
              baseUrl,
              model,
            }
          : { mode },
      );
      if (mode === "custom" && userId) {
        const local: LocalCustomAiSettings = {
          userId,
          baseUrl,
          model,
          apiKey: effectiveKey,
          routingPreset,
          fallbackModel: fallbackModel || null,
          routes: routingPreset === "easy" ? routes : routes.length ? routes : undefined,
        };
        await saveLocalCustomAiSettings(local);
        setLocalCustom(local);
      } else {
        await removeLocalCustomAiSettings();
        setLocalCustom(null);
      }
      setApiKey("");
      applySettings(
        mode === "custom" ? { ...next, configured: true, key_mask: "本机安全存储" } : next,
      );
      toast.show("AI 设置已保存");
    } catch (error: unknown) {
      const message = aiErrorMessage(error, "save");
      setErrorMessage(message);
      toast.show(message);
    } finally {
      setBusy(false);
    }
  };

  const test = async () => {
    setErrorMessage(null);
    setBusy(true);
    try {
      const effectiveKey = apiKey || localCustom?.apiKey || "";
      await apiClient.testAiSettings({
        provider: "openai_compat",
        baseUrl,
        model: model || "probe-model",
        apiKey: effectiveKey,
      });
      toast.show("连接正常");
    } catch (error: unknown) {
      const message = aiErrorMessage(error, "test");
      setErrorMessage(message);
      toast.show(message);
    } finally {
      setBusy(false);
    }
  };

  const easySetup = async () => {
    setErrorMessage(null);
    setBusy(true);
    try {
      const effectiveKey = apiKey || localCustom?.apiKey || "";
      if (!baseUrl.trim() || !effectiveKey) {
        throw new ApiError({
          code: "invalid_argument",
          status: 400,
          message: "请先填写 HTTPS 地址和 API Key",
        });
      }
      const { models } = await apiClient.discoverCustomModels({
        baseUrl,
        apiKey: effectiveKey,
      });
      const plan = buildEasyRoutePlan(models);
      applyPlan(plan);
      toast.show(
        plan.preset === "easy"
          ? `已自动配置多模型（发现 ${models.length} 个）`
          : `已配置单模型（发现 ${models.length} 个）`,
      );
    } catch (error: unknown) {
      const message = aiErrorMessage(error, "discover");
      setErrorMessage(message);
      toast.show(message);
    } finally {
      setBusy(false);
    }
  };

  const removeKey = async () => {
    try {
      await removeLocalCustomAiSettings();
      const next = await apiClient.deleteCustomAiKey();
      setApiKey("");
      setLocalCustom(null);
      setRoutes([]);
      setPlanNotes([]);
      setFallbackModel("");
      applySettings(next);
      toast.show("自定义密钥已删除");
    } catch (error: unknown) {
      const message = aiErrorMessage(error, "delete");
      setErrorMessage(message);
      toast.show(message);
    }
  };

  if (!settings) {
    const loading = (
      <div className="panel">
        <h4>AI 设置</h4>
        <div className="skeleton" style={{ height: 180 }} />
      </div>
    );
    return embedded ? (
      loading
    ) : (
      <section className="settings">
        <h2 className="settings-title">AI 设置</h2>
        {loading}
      </section>
    );
  }

  const panel = (
    <div className="panel" aria-label="AI 设置">
      <h4>AI 设置</h4>
      <p className="cal-note">
        用于顶部「描述推荐」。桌面端密钥保存在系统凭据库；浏览器预览仅保留在当前标签页会话，服务端不会写入数据库。
      </p>
      <div className="seg" role="tablist" aria-label="AI 模式">
        {(["builtin", "custom", "off"] as const).map((choice) => (
          <button
            key={choice}
            type="button"
            className="btn small"
            aria-pressed={mode === choice}
            onClick={() => setMode(choice)}
          >
            {choice === "builtin" ? "内置 AI" : choice === "custom" ? "自定义 API" : "关闭 AI"}
          </button>
        ))}
      </div>

      {mode === "builtin" && (
        <>
          <div className="statusline ai-statusline">
            <span className={settings.builtin.available ? "chip ok" : "chip danger"}>
              {settings.builtin.available ? "可用" : "不可用"}
            </span>
            <span className="chip">{settings.builtin.multi_model ? "多模型路由" : "单模型"}</span>
            {settings.builtin.provider && <span className="chip">{settings.builtin.provider}</span>}
            {settings.builtin.daily_remaining !== null && (
              <span className="chip">剩余 {settings.builtin.daily_remaining}</span>
            )}
          </div>
          {settings.builtin.routes && settings.builtin.routes.length > 0 && (
            <RouteTable
              title="内置任务模型路由"
              note="服务端按任务自动选模型。你无需配置。"
              routes={settings.builtin.routes
                .filter((r) => r.enabled)
                .map((r) => ({
                  task: r.task,
                  primary_model: r.primary_model,
                  fallback_models: r.fallback_models,
                  primary_available: r.primary_available,
                }))}
            />
          )}
          {settings.builtin.discovered_models && settings.builtin.discovered_models.length > 0 && (
            <p className="cal-note">
              上游已发现：{settings.builtin.discovered_models.slice(0, 12).join(", ")}
              {settings.builtin.discovered_models.length > 12 ? "…" : ""}
            </p>
          )}
        </>
      )}

      {mode === "custom" && (
        <div className="stack-form ai-custom-form">
          <label>
            HTTPS 地址
            <input
              type="url"
              value={baseUrl}
              placeholder="https://api.example.com/v1"
              onChange={(event) => setBaseUrl(event.target.value)}
            />
          </label>
          <label>
            API Key
            <input
              type="password"
              value={apiKey}
              autoComplete="off"
              onChange={(event) => setApiKey(event.target.value)}
              placeholder={settings.configured || localCustom ? "输入新密钥以替换" : ""}
            />
          </label>
          {(settings.configured || localCustom) && (
            <span className="chip ok">密钥 {settings.key_mask ?? "本机安全存储"}</span>
          )}

          <div className="easy-setup-box">
            <button
              type="button"
              className="btn accent"
              disabled={busy || (!apiKey && !localCustom) || !baseUrl.trim()}
              onClick={() => void easySetup()}
            >
              {busy ? "配置中…" : "一键省心配置"}
            </button>
            <p className="cal-note">
              填好地址和 Key 后点一次即可：自动探测 <code>/v1/models</code>，为「理解你的话 /
              推荐理由 / 比较」等任务分配主模型与回退链。密钥仍只在本机。
            </p>
          </div>

          <div className="seg" role="tablist" aria-label="自定义路由方式">
            <button
              type="button"
              className="btn small"
              aria-pressed={routingPreset === "easy"}
              onClick={() => setRoutingPreset("easy")}
            >
              多模型（推荐）
            </button>
            <button
              type="button"
              className="btn small"
              aria-pressed={routingPreset === "single"}
              onClick={() => {
                setRoutingPreset("single");
                setRoutes([]);
              }}
            >
              单模型
            </button>
          </div>

          {routingPreset === "single" ? (
            <label>
              模型
              <input
                value={model}
                placeholder="model-name"
                onChange={(event) => setModel(event.target.value)}
              />
            </label>
          ) : (
            <>
              <label>
                默认主模型（推荐任务）
                <input
                  value={model}
                  placeholder="探测后自动填入"
                  onChange={(event) => setModel(event.target.value)}
                />
              </label>
              <label>
                默认回退模型（可选）
                <input
                  value={fallbackModel}
                  placeholder="主模型失败时使用"
                  onChange={(event) => setFallbackModel(event.target.value)}
                />
              </label>
            </>
          )}

          {routes.length > 0 && routingPreset === "easy" && (
            <RouteTable
              title="自定义任务模型路由"
              note="以下为省心配置结果，可保存后用于描述推荐。"
              routes={routes.map((r) => ({
                task: r.task,
                primary_model: r.primary_model,
                fallback_models: r.fallback_models,
              }))}
            />
          )}
          {planNotes.length > 0 && (
            <ul className="cal-note">
              {planNotes.map((n) => (
                <li key={n}>{n}</li>
              ))}
            </ul>
          )}

          <div className="seg">
            <button
              type="button"
              className="btn small"
              disabled={busy || (!apiKey && !localCustom)}
              onClick={() => void test()}
            >
              {apiKey ? "测试当前输入" : "测试已保存配置"}
            </button>
            {(settings.configured || localCustom) && (
              <button type="button" className="btn small ghost" onClick={() => void removeKey()}>
                删除密钥
              </button>
            )}
          </div>
        </div>
      )}

      {errorMessage && (
        <p className="cal-note ai-error" role="alert">
          {errorMessage}
        </p>
      )}
      <div className="onboarding-actions" style={{ justifyContent: "flex-start", marginTop: 16 }}>
        <button
          type="button"
          className="btn primary"
          disabled={busy || (mode === "custom" && !apiKey && !localCustom)}
          onClick={() => void save()}
        >
          {busy ? "处理中" : "保存"}
        </button>
      </div>
    </div>
  );

  if (embedded) return panel;
  return (
    <section className="settings ai-settings" aria-label="AI 设置">
      <h2 className="settings-title">AI 设置</h2>
      {panel}
    </section>
  );
}
