// First-run onboarding: pick a theme (live, per-card skinned), then set core
// preferences. Preferences are pushed via PUT /v1/preferences; failures keep
// the user local-only and are surfaced without blocking browsing.
// Styles: styles/screens/settings.css（.onboarding 作用域）+ base.css 共享类。

import { useMemo, useRef, useState } from "react";
import { ApiError } from "../api/client";
import { apiClient, markOnboarded } from "../app/runtime";
import {
  flushPendingPreferencePatch,
  queuePreferencePatch,
  SESSION_OPTIONS,
  type PendingPreferencesPatch,
} from "../app/preferences";
import { useTheme } from "../app/ThemeProvider";
import { useToast } from "../app/ToastProvider";
import { THEME_ORDER, THEMES } from "../theme/registry";
import type { ThemeId } from "../theme/types";
import { Button } from "../components/Button";
import { Panel } from "../components/Panel";

const PARTY_CHOICES = [2, 3, 4, 5, 6, 8];

const BUDGET_CHOICES: { label: string; minor: number | null }[] = [
  { label: "¥50 以内", minor: 5_000 },
  { label: "¥100 以内", minor: 10_000 },
  { label: "¥150 以内", minor: 15_000 },
  { label: "¥300 以内", minor: 30_000 },
  { label: "不限", minor: null },
];

const STEP_LABELS = ["界面风格", "联机偏好"] as const;

function StepIndicator({ step }: { step: 0 | 1 }) {
  return (
    <ol className="onboarding-steps" aria-label="引导进度">
      {STEP_LABELS.map((label, idx) => (
        <li
          key={label}
          className={idx === step ? "current" : idx < step ? "done" : ""}
          aria-current={idx === step ? "step" : undefined}
        >
          <span className="step-index" aria-hidden="true">
            {idx + 1}
          </span>
          {label}
        </li>
      ))}
    </ol>
  );
}

function ThemePickerCard({
  id,
  selected,
  onPick,
}: {
  id: ThemeId;
  selected: boolean;
  onPick: (id: ThemeId) => void;
}) {
  const theme = THEMES[id];
  return (
    <button
      type="button"
      data-theme={id}
      className={`theme-card${selected ? " selected" : ""}`}
      onClick={() => onPick(id)}
      aria-pressed={selected}
    >
      <span className="preview" aria-hidden="true">
        <span className="swatch" style={{ background: "var(--accent)" }} />
        <span className="swatch" style={{ background: "var(--accent-2)" }} />
        <span className="swatch" style={{ background: "var(--surface-2)" }} />
      </span>
      <span className="label">
        <strong>{theme.label}</strong>
        <span>{theme.tagline}</span>
      </span>
    </button>
  );
}

export function OnboardingScreen({ onDone }: { onDone: () => void }) {
  const { themeId, setTheme, fireAction } = useTheme();
  const toast = useToast();
  const [step, setStep] = useState<0 | 1>(0);
  const [party, setParty] = useState(4);
  const [coopCompetitive, setCoopCompetitive] = useState(0.15);
  const [sessionIdx, setSessionIdx] = useState(1);
  const [budgetIdx, setBudgetIdx] = useState(2);
  const [selfHosting, setSelfHosting] = useState(0.7);
  const [saving, setSaving] = useState(false);
  const doneBtnRef = useRef<HTMLButtonElement>(null);

  const coopLabel = useMemo(() => {
    if (coopCompetitive <= 0.25) return "偏合作";
    if (coopCompetitive >= 0.75) return "偏竞技";
    return "均衡";
  }, [coopCompetitive]);

  const finish = async () => {
    setSaving(true);
    const session = SESSION_OPTIONS[sessionIdx] ?? SESSION_OPTIONS[1]!;
    const budget = BUDGET_CHOICES[budgetIdx] ?? BUDGET_CHOICES[2]!;
    const patch: PendingPreferencesPatch = {
      preference_confidence: 1,
      party_size: party,
      coop_competitive: coopCompetitive,
      session_minutes_min: session.min,
      session_minutes_max: session.max,
      budget_max_each_minor: budget.minor,
      self_hosting_willingness: selfHosting,
    };
    if (!queuePreferencePatch(patch)) {
      toast.show("无法在本机保存偏好，请检查存储权限后重试。");
      setSaving(false);
      return;
    }
    try {
      const saved = await flushPendingPreferencePatch(apiClient);
      if (!saved) {
        toast.show("偏好已保存在本机，登录后会自动同步。");
      }
      fireAction("confirm", doneBtnRef.current);
    } catch (error) {
      const offline = error instanceof ApiError && error.offline;
      toast.show(
        offline
          ? "偏好已保存在本机，将在联机后同步。"
          : "偏好已保存在本机，服务恢复后会重试同步。",
      );
    } finally {
      markOnboarded();
      setSaving(false);
      onDone();
    }
  };

  const skipPreferences = () => {
    markOnboarded();
    onDone();
  };

  if (step === 0) {
    return (
      <div className="onboarding">
        <StepIndicator step={0} />
        <header className="onboarding-head">
          <h1>选择你的界面风格</h1>
          <p className="sub">每种主题都有自己的动态特效与点击反馈，随时可在顶栏切换。</p>
        </header>
        <div className="theme-grid">
          {THEME_ORDER.map((id) => (
            <ThemePickerCard
              key={id}
              id={id}
              selected={themeId === id}
              onPick={(picked) => setTheme(picked)}
            />
          ))}
        </div>
        <div className="onboarding-actions">
          <Button variant="primary" onClick={() => setStep(1)}>
            继续 →
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="onboarding">
      <StepIndicator step={1} />
      <header className="onboarding-head">
        <h1>你们通常怎么玩？</h1>
        <p className="sub">这些偏好驱动四个分区的排序，之后可以随时调整。</p>
      </header>
      <Panel className="onboarding-prefs-panel">
        <div className="prefs-form">
          <div className="pref-row">
            <label htmlFor="party-seg">常用人数</label>
            <div className="seg" id="party-seg" role="group" aria-label="常用人数">
              {PARTY_CHOICES.map((n) => (
                <Button
                  key={n}
                  aria-pressed={party === n}
                  onClick={() => setParty(n)}
                >
                  {n} 人
                </Button>
              ))}
            </div>
          </div>

          <div className="pref-row">
            <label htmlFor="coop-range">
              合作 ↔ 竞技
              <output>{coopLabel}</output>
            </label>
            <input
              id="coop-range"
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={coopCompetitive}
              onChange={(event) => setCoopCompetitive(Number(event.target.value))}
            />
          </div>

          <div className="pref-row">
            <label htmlFor="session-seg">单次游玩时长</label>
            <div className="seg" id="session-seg" role="group" aria-label="单次游玩时长">
              {SESSION_OPTIONS.map((choice, idx) => (
                <Button
                  key={choice.label}
                  aria-pressed={sessionIdx === idx}
                  onClick={() => setSessionIdx(idx)}
                >
                  {choice.label}
                </Button>
              ))}
            </div>
          </div>

          <div className="pref-row">
            <label htmlFor="budget-seg">每人预算</label>
            <div className="seg" id="budget-seg" role="group" aria-label="每人预算">
              {BUDGET_CHOICES.map((choice, idx) => (
                <Button
                  key={choice.label}
                  aria-pressed={budgetIdx === idx}
                  onClick={() => setBudgetIdx(idx)}
                >
                  {choice.label}
                </Button>
              ))}
            </div>
          </div>

          <div className="pref-row">
            <label htmlFor="host-range">
              自建服务器意愿
              <output>
                {selfHosting >= 0.7 ? "愿意折腾" : selfHosting >= 0.4 ? "看情况" : "最好免配置"}
              </output>
            </label>
            <input
              id="host-range"
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={selfHosting}
              onChange={(event) => setSelfHosting(Number(event.target.value))}
            />
          </div>
        </div>
      </Panel>

      <div className="onboarding-actions">
        <Button variant="ghost" onClick={() => setStep(0)}>
          ← 换个主题
        </Button>
        <Button variant="ghost" disabled={saving} onClick={skipPreferences}>
          稍后设置/不确定
        </Button>
        <Button
          ref={doneBtnRef}
          variant="primary"
          disabled={saving}
          onClick={() => void finish()}
        >
          {saving ? (
            <>
              <span className="spin" /> 保存中
            </>
          ) : (
            "开始探索"
          )}
        </Button>
      </div>
    </div>
  );
}
