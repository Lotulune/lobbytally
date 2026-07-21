import { useEffect, useState } from "react";
import { ApiError } from "../api/client";
import type { AiSettings } from "../api/types";
import { apiClient } from "../app/runtime";
import {
  loadLocalCustomAiSettings,
  removeLocalCustomAiSettings,
  saveLocalCustomAiSettings,
  type LocalCustomAiSettings,
} from "../app/localAiSettings";
import { useToast } from "../app/ToastProvider";

type Mode = AiSettings["mode"];

function aiErrorMessage(error: unknown, action: "load" | "test" | "save" | "delete"): string {
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
  if (action === "delete") return "无法删除自定义密钥。";
  return "AI 设置保存失败。";
}

export function AiSettingsScreen({ embedded = false }: { embedded?: boolean }) {
  const toast = useToast();
  const [settings, setSettings] = useState<AiSettings | null>(null);
  const [mode, setMode] = useState<Mode>("builtin");
  const [baseUrl, setBaseUrl] = useState("");
  const [model, setModel] = useState("");
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
      if (mode === "custom" && !model.trim()) {
        throw new ApiError({
          code: "invalid_argument",
          status: 400,
          message: "请填写模型名称",
        });
      }
      const next = await apiClient.putAiSettings(
        mode === "custom"
          ? {
              mode,
              provider: "openai_compat",
              baseUrl,
              model: model.trim(),
            }
          : { mode },
      );
      if (mode === "custom" && userId) {
        // Single model only for custom until vendor-neutral multi-model UX exists.
        // Do not invent per-task models from name heuristics.
        const local: LocalCustomAiSettings = {
          userId,
          baseUrl,
          model: model.trim(),
          apiKey: effectiveKey,
          routingPreset: "single",
          fallbackModel: null,
          routes: undefined,
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
        model: model.trim() || "probe-model",
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

  const removeKey = async () => {
    try {
      await removeLocalCustomAiSettings();
      const next = await apiClient.deleteCustomAiKey();
      setApiKey("");
      setLocalCustom(null);
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
            {settings.builtin.daily_remaining !== null && (
              <span className="chip">今日剩余 {settings.builtin.daily_remaining} 次</span>
            )}
          </div>
          <p className="cal-note">使用服务端内置 AI。模型与回退在后台自动完成，无需配置。</p>
        </>
      )}

      {mode === "custom" && (
        <div className="stack-form ai-custom-form">
          <p className="cal-note">
            填写 OpenAI 兼容接口的地址、模型名和 Key。当前为<strong>单模型</strong>模式（跨厂商名称无法可靠推断能力，故不做自动分模型）。
          </p>
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
            模型
            <input
              value={model}
              placeholder="例如 gpt-4o-mini"
              onChange={(event) => setModel(event.target.value)}
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
