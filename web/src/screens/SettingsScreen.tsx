// Settings: durable preference editing, theme + FX controls,
// cache management, and sync/offline status.
// Styles: styles/screens/settings.css（.settings-screen 作用域）+ base.css 共享类。

import { useEffect, useRef, useState } from "react";
import { ApiError } from "../api/client";
import type { UserPreferences } from "../api/types";
import { requestAccountSignIn } from "../app/auth";
import { apiClient, feedbackQueue, requiresServiceConnect } from "../app/runtime";
import { useTheme } from "../app/ThemeProvider";
import { useToast } from "../app/ToastProvider";
import {
  applyPendingPreferencePatch,
  defaultPreferences,
  editablePreferencePatch,
  EXCLUDED_MODE_OPTIONS,
  flushPendingPreferencePatch,
  hasPendingPreferencePatch,
  LANGUAGE_OPTIONS,
  PLATFORM_OPTIONS,
  preferencesChanged,
  queuePreferencePatch,
  SESSION_OPTIONS,
  toggleMember,
} from "../app/preferences";
import { THEME_ORDER, THEMES } from "../theme/registry";
import type { FxIntensity } from "../fx/types";
import { Button } from "../components/Button";
import { Chip } from "../components/Chip";
import { Panel } from "../components/Panel";
import { Skeleton } from "../components/Skeleton";
import { AiSettingsScreen } from "./AiSettingsScreen";
import { ServicePanel } from "./ServicePanel";

const PARTY_CHOICES = [2, 3, 4, 5, 6, 8];
const BUDGET_CHOICES: { label: string; minor: number | null }[] = [
  { label: "¥50", minor: 5_000 },
  { label: "¥100", minor: 10_000 },
  { label: "¥150", minor: 15_000 },
  { label: "¥300", minor: 30_000 },
  { label: "不限", minor: null },
];
const FX_CHOICES: { id: FxIntensity; label: string }[] = [
  { id: "full", label: "全" },
  { id: "low", label: "低" },
  { id: "off", label: "关" },
];

function MultiToggle({
  legend,
  options,
  selected,
  onToggle,
}: {
  legend: string;
  options: { id: string; label: string }[];
  selected: string[];
  onToggle: (id: string) => void;
}) {
  return (
    <fieldset className="pref-row set-fieldset">
      <legend>{legend}</legend>
      <div className="seg">
        {options.map((opt) => (
          <Button
            key={opt.id}
            size="small"
            aria-pressed={selected.includes(opt.id)}
            onClick={() => onToggle(opt.id)}
          >
            {opt.label}
          </Button>
        ))}
      </div>
    </fieldset>
  );
}

export function SettingsScreen() {
  const { themeId, setTheme, intensity, setIntensity, fireAction } = useTheme();
  const toast = useToast();
  const [base, setBase] = useState<UserPreferences | null>(null);
  const [draft, setDraft] = useState<UserPreferences>(() =>
    applyPendingPreferencePatch(defaultPreferences()),
  );
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [pendingPreferences, setPendingPreferences] = useState(hasPendingPreferencePatch);
  const [syncingPreferences, setSyncingPreferences] = useState(false);
  const [accountAuthenticated, setAccountAuthenticated] = useState(() =>
    apiClient.isAccountAuthenticated(),
  );
  const [pendingFeedback, setPendingFeedback] = useState(() => feedbackQueue.pendingCount());
  const saveBtnRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    let cancelled = false;
    apiClient
      .getPreferences()
      .then((prefs) => {
        if (cancelled) return;
        setBase(prefs);
        setDraft(applyPendingPreferencePatch(prefs));
        setLoading(false);
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        // Offline or not-yet-created: fall back to editable defaults.
        setLoading(false);
        if (error instanceof ApiError && error.offline) {
          setLoadError("离线：保存时会先将偏好保留在本机。");
        } else {
          setLoadError("无法加载服务端偏好，正在使用本地默认值。");
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(
    () => apiClient.subscribeAuth(() => setAccountAuthenticated(apiClient.isAccountAuthenticated())),
    [],
  );

  // Keep the "pending feedback" chip live as the queue syncs.
  useEffect(
    () => feedbackQueue.subscribe(() => setPendingFeedback(feedbackQueue.pendingCount())),
    [],
  );

  // When the server prefs never loaded (offline), allow saving anyway so the
  // attempt can create/update once connectivity returns.
  const dirty = base === null ? true : preferencesChanged(base, draft);

  const patch = (fields: Partial<UserPreferences>) => setDraft((d) => ({ ...d, ...fields }));

  const save = async () => {
    if (!apiClient.isAccountAuthenticated()) {
      requestAccountSignIn();
      return;
    }
    setSaving(true);
    if (!queuePreferencePatch(editablePreferencePatch(draft))) {
      toast.show("无法在本机保存偏好，请检查存储权限后重试。");
      setSaving(false);
      return;
    }
    setPendingPreferences(true);
    try {
      const saved = await flushPendingPreferencePatch(apiClient);
      if (!saved) return;
      setBase(saved);
      setDraft(saved);
      setPendingPreferences(false);
      fireAction("confirm", saveBtnRef.current);
      toast.show("偏好已保存");
    } catch (error) {
      fireAction("error", saveBtnRef.current);
      toast.show(
        error instanceof ApiError && error.offline
          ? "偏好已保存在本机，将在联机后同步。"
          : "偏好已保存在本机，服务恢复后会重试同步。",
      );
    } finally {
      setSaving(false);
    }
  };

  const clearCache = () => {
    const removed = apiClient.clearCachedResponses();
    toast.show(`已清除 ${removed} 项缓存快照（未同步反馈已保留）`);
  };

  const syncPendingPreferences = async () => {
    setSyncingPreferences(true);
    try {
      const saved = await flushPendingPreferencePatch(apiClient);
      setPendingPreferences(hasPendingPreferencePatch());
      if (saved) {
        setBase(saved);
        setDraft(saved);
        setLoadError(null);
        toast.show("本地偏好已同步");
      }
    } catch (error) {
      toast.show(
        error instanceof ApiError && error.offline
          ? "仍处于离线状态，本地偏好已保留。"
          : "偏好同步失败，本地副本已保留。",
      );
    } finally {
      setSyncingPreferences(false);
    }
  };

  const coopLabel =
    draft.coop_competitive <= 0.25 ? "偏合作" : draft.coop_competitive >= 0.75 ? "偏竞技" : "均衡";

  const sessionIdx = SESSION_OPTIONS.findIndex(
    (choice) =>
      choice.min === draft.session_minutes_min && choice.max === draft.session_minutes_max,
  );

  return (
    <section aria-label="设置" className="settings settings-screen">
      <h2 className="settings-title">设置</h2>

      <Panel title="外观">
        <div className="pref-row">
          <label>主题</label>
          <div className="seg">
            {THEME_ORDER.map((id) => (
              <Button
                key={id}
                size="small"
                aria-pressed={themeId === id}
                onClick={() => setTheme(id)}
              >
                {THEMES[id].label}
              </Button>
            ))}
          </div>
        </div>
        <div className="pref-row">
          <label>动态特效强度</label>
          <div className="seg">
            {FX_CHOICES.map((choice) => (
              <Button
                key={choice.id}
                size="small"
                aria-pressed={intensity === choice.id}
                onClick={() => setIntensity(choice.id)}
              >
                {choice.label}
              </Button>
            ))}
          </div>
        </div>
      </Panel>

      <Panel title="推荐偏好">
        {loading ? (
          <Skeleton height={220} />
        ) : (
          <div className="prefs-form">
            {loadError && <p className="cal-note settings-note">{loadError}</p>}

            <h5 className="pref-group-title">联机方式</h5>

            <div className="pref-row">
              <label>常用人数</label>
              <div className="seg">
                {PARTY_CHOICES.map((n) => (
                  <Button
                    key={n}
                    size="small"
                    aria-pressed={draft.party_size === n}
                    onClick={() => patch({ party_size: n })}
                  >
                    {n} 人
                  </Button>
                ))}
              </div>
            </div>

            <div className="pref-row">
              <label htmlFor="set-coop">
                合作 ↔ 竞技 <output>{coopLabel}</output>
              </label>
              <input
                id="set-coop"
                type="range"
                min={0}
                max={1}
                step={0.05}
                value={draft.coop_competitive}
                onChange={(e) => patch({ coop_competitive: Number(e.target.value) })}
              />
            </div>

            <div className="pref-row">
              <label>单次游玩时长</label>
              <div className="seg">
                {SESSION_OPTIONS.map((choice, idx) => (
                  <Button
                    key={choice.label}
                    size="small"
                    aria-pressed={sessionIdx === idx}
                    onClick={() =>
                      patch({
                        session_minutes_min: choice.min,
                        session_minutes_max: choice.max,
                      })
                    }
                  >
                    {choice.label}
                  </Button>
                ))}
                {sessionIdx === -1 && (
                  <Button size="small" aria-pressed="true" disabled>
                    {draft.session_minutes_min}–{draft.session_minutes_max} 分钟
                  </Button>
                )}
              </div>
            </div>

            <div className="pref-row">
              <label>每人预算</label>
              <div className="seg">
                {BUDGET_CHOICES.map((choice) => (
                  <Button
                    key={choice.label}
                    size="small"
                    aria-pressed={draft.budget_max_each_minor === choice.minor}
                    onClick={() => patch({ budget_max_each_minor: choice.minor })}
                  >
                    {choice.label}
                  </Button>
                ))}
              </div>
            </div>

            <div className="pref-row">
              <label htmlFor="set-host">
                自建服务器意愿{" "}
                <output>
                  {draft.self_hosting_willingness >= 0.7
                    ? "愿意折腾"
                    : draft.self_hosting_willingness >= 0.4
                      ? "看情况"
                      : "最好免配置"}
                </output>
              </label>
              <input
                id="set-host"
                type="range"
                min={0}
                max={1}
                step={0.05}
                value={draft.self_hosting_willingness}
                onChange={(e) => patch({ self_hosting_willingness: Number(e.target.value) })}
              />
            </div>

            <h5 className="pref-group-title">平台与内容</h5>

            <MultiToggle
              legend="平台"
              options={PLATFORM_OPTIONS}
              selected={draft.platforms}
              onToggle={(id) => patch({ platforms: toggleMember(draft.platforms, id) })}
            />
            <MultiToggle
              legend="语言"
              options={LANGUAGE_OPTIONS}
              selected={draft.languages}
              onToggle={(id) => patch({ languages: toggleMember(draft.languages, id) })}
            />
            <MultiToggle
              legend="排除模式"
              options={EXCLUDED_MODE_OPTIONS}
              selected={draft.excluded_modes}
              onToggle={(id) => patch({ excluded_modes: toggleMember(draft.excluded_modes, id) })}
            />

            <div className="settings-actions">
              <Button
                ref={saveBtnRef}
                variant="primary"
                disabled={saving || !dirty}
                onClick={() => void save()}
              >
                {saving ? (
                  <>
                    <span className="spin" /> 保存中
                  </>
                ) : dirty ? (
                  "保存偏好"
                ) : (
                  "已保存"
                )}
              </Button>
              {!accountAuthenticated && (
                <p className="cal-note settings-note">保存到云端需要登录，点击保存将先打开登录。</p>
              )}
            </div>
          </div>
        )}
      </Panel>

      {requiresServiceConnect && <ServicePanel />}

      <Panel title="数据与缓存">
        <div className="statusline settings-statusline">
          <Chip tone={navigator.onLine ? "ok" : "danger"}>
            {navigator.onLine ? "在线" : "离线"}
          </Chip>
          {pendingFeedback > 0 && (
            <Chip tone="warn">{pendingFeedback} 条反馈待同步</Chip>
          )}
          {pendingPreferences && <Chip tone="warn">偏好待同步</Chip>}
        </div>
        <div className="seg">
          <Button size="small" onClick={clearCache}>
            清除缓存快照
          </Button>
          <Button size="small" onClick={() => void feedbackQueue.flush()}>
            立即同步反馈
          </Button>
          {pendingPreferences && (
            <Button
              size="small"
              disabled={syncingPreferences}
              onClick={() => void syncPendingPreferences()}
            >
              {syncingPreferences ? "同步中" : "立即同步偏好"}
            </Button>
          )}
        </div>
        <p className="cal-note settings-note">清除缓存不会删除尚未同步的反馈。</p>
      </Panel>

      {accountAuthenticated ? (
        <AiSettingsScreen embedded />
      ) : (
        <Panel title="AI 设置" aria-label="AI 设置">
          <p className="cal-note settings-note">
            登录后可选择内置 AI、自定义 OpenAI 兼容 API，或关闭 AI。
          </p>
          <Button variant="primary" onClick={requestAccountSignIn}>
            登录后配置
          </Button>
        </Panel>
      )}
    </section>
  );
}
