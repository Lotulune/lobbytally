import { type CSSProperties, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  getDefaultLlmBaseUrl,
  getDefaultLlmModel,
  isTauriRuntime,
  saveConfig,
  validateLlmConfig,
  validateSteamConfig,
} from "../../api/client";
import type {
  ConnectionValidationResult,
  LlmProvider,
  PublicConfig,
  SaveConfigRequest,
} from "../../types";
import "../../App.css";

type OnboardingSource = "auto" | "settings";
type ExitTarget = "app" | "settings";

const STEAM_APPLY_URL = "https://steamcommunity.com/dev/apikey";
const VALIDATION_SUCCESS_FEEDBACK_MS = 10_000;

const STEP_ITEMS = [
  { id: 1, title: "欢迎" },
  { id: 2, title: "准备 Steam Web API" },
  { id: 3, title: "填写 Steam Key" },
  { id: 4, title: "准备 AI 提供方" },
  { id: 5, title: "完成配置" },
] as const;

export function OnboardingWizard({
  config,
  source,
  onExit,
}: {
  config: PublicConfig;
  source: OnboardingSource;
  onExit: (target: ExitTarget) => Promise<void> | void;
}) {
  const [savedConfig, setSavedConfig] = useState(config);
  const [currentStep, setCurrentStep] = useState(() => resolveEntryStep(config, source));
  const [previousStep, setPreviousStep] = useState(() => resolveEntryStep(config, source));
  const [steamApiKey, setSteamApiKey] = useState("");
  const [llmProvider, setLlmProvider] = useState<LlmProvider>(
    config.onboardingLlmProviderDraft ?? config.llmProvider,
  );
  const [llmApiKey, setLlmApiKey] = useState("");
  const [llmBaseUrl, setLlmBaseUrl] = useState(() =>
    resolveInitialBaseUrl(config, config.onboardingLlmProviderDraft ?? config.llmProvider),
  );
  const [llmModel, setLlmModel] = useState(() =>
    resolveInitialModel(config, config.onboardingLlmProviderDraft ?? config.llmProvider),
  );
  const [steamValidation, setSteamValidation] = useState<ConnectionValidationResult | null>(
    () => resolveSteamValidationFromConfig(config),
  );
  const [llmValidation, setLlmValidation] = useState<ConnectionValidationResult | null>(
    () => resolveLlmValidationFromConfig(config),
  );
  const steamValidationRef = useRef(steamValidation);
  const llmValidationRef = useRef(llmValidation);
  const steamSuccessVisibleUntilRef = useRef(0);
  const llmSuccessVisibleUntilRef = useRef(0);
  const [notice, setNotice] = useState("");
  const [showAdvanced, setShowAdvanced] = useState(
    (config.onboardingLlmProviderDraft ?? config.llmProvider) === "custom",
  );
  const [busyAction, setBusyAction] = useState<
    | "step"
    | "steam-test"
    | "steam-save"
    | "llm-test"
    | "llm-save"
    | "exit"
    | null
  >(null);

  useEffect(() => {
    setSavedConfig(config);
    const nextProvider = config.onboardingLlmProviderDraft ?? config.llmProvider;
    if (source === "auto") {
      const nextStep = resolveEntryStep(config, source);
      setPreviousStep(nextStep);
      setCurrentStep(nextStep);
    }
    setLlmProvider(nextProvider);
    setLlmBaseUrl(resolveInitialBaseUrl(config, nextProvider));
    setLlmModel(resolveInitialModel(config, nextProvider));
    setSteamApiKey("");
    setLlmApiKey("");
    const steamConfigValidation = resolveSteamValidationFromConfig(config);
    const llmConfigValidation = resolveLlmValidationFromConfig(config);
    const steamFeedbackRemainingMs = getSuccessFeedbackRemainingMs(
      steamValidationRef.current,
      steamSuccessVisibleUntilRef.current,
    );
    const llmFeedbackRemainingMs = getSuccessFeedbackRemainingMs(
      llmValidationRef.current,
      llmSuccessVisibleUntilRef.current,
    );

    let steamResetTimer: number | undefined;
    let llmResetTimer: number | undefined;

    if (steamFeedbackRemainingMs > 0) {
      steamResetTimer = window.setTimeout(() => {
        steamSuccessVisibleUntilRef.current = 0;
        setSteamValidationState(steamConfigValidation);
      }, steamFeedbackRemainingMs);
    } else {
      setSteamValidationState(steamConfigValidation);
    }

    if (llmFeedbackRemainingMs > 0) {
      llmResetTimer = window.setTimeout(() => {
        llmSuccessVisibleUntilRef.current = 0;
        setLlmValidationState(llmConfigValidation);
      }, llmFeedbackRemainingMs);
    } else {
      setLlmValidationState(llmConfigValidation);
    }

    return () => {
      if (steamResetTimer !== undefined) {
        window.clearTimeout(steamResetTimer);
      }
      if (llmResetTimer !== undefined) {
        window.clearTimeout(llmResetTimer);
      }
    };
  }, [config, source]);

  useEffect(() => {
    scrollWindowToTop();
  }, []);

  useEffect(() => {
    scrollWindowToTop();
  }, [currentStep]);

  const steamDraftDirty = steamApiKey.trim().length > 0;
  const llmDraftDirty =
    llmApiKey.trim().length > 0 ||
    llmProvider !== savedConfig.llmProvider ||
    llmBaseUrl.trim() !== savedConfig.llmBaseUrl.trim() ||
    llmModel.trim() !== savedConfig.llmModel.trim();

  const steamReady =
    (steamDraftDirty && steamValidation?.success) ||
    (!steamDraftDirty && savedConfig.steamApiKeyValidated);
  const llmReady =
    (llmDraftDirty && llmValidation?.success) ||
    (!llmDraftDirty && savedConfig.llmConfigValidated);
  const canSaveSteam = steamDraftDirty || savedConfig.steamApiKeyConfigured;
  const canSaveLlm = llmDraftDirty || savedConfig.llmApiKeyConfigured;
  const currentProviderLabel = providerLabel(llmProvider);
  const currentProviderPortalHost = displayUrl(providerPortalUrl(llmProvider));
  const llmSetupSteps = useMemo(() => providerSetupSteps(llmProvider), [llmProvider]);
  const steamCanReuseSavedValidation = !steamDraftDirty && savedConfig.steamApiKeyValidated;
  const llmCanReuseSavedValidation = !llmDraftDirty && savedConfig.llmConfigValidated;

  const stepState = useMemo(
    () => ({
      1: currentStep > 1 || savedConfig.onboardingCompleted,
      2: steamReady,
      3: steamReady,
      4: llmReady,
      5: llmReady,
    }),
    [currentStep, llmReady, savedConfig.onboardingCompleted, steamReady],
  );

  async function persistConfig(request: SaveConfigRequest) {
    const next = await saveConfig(request);
    setSavedConfig(next);
    return next;
  }

  function setSteamValidationState(result: ConnectionValidationResult | null) {
    steamValidationRef.current = result;
    setSteamValidation(result);
  }

  function setLlmValidationState(result: ConnectionValidationResult | null) {
    llmValidationRef.current = result;
    setLlmValidation(result);
  }

  function showSteamValidationResult(result: ConnectionValidationResult) {
    steamSuccessVisibleUntilRef.current = result.success
      ? Date.now() + VALIDATION_SUCCESS_FEEDBACK_MS
      : 0;
    setSteamValidationState(result);
  }

  function showLlmValidationResult(result: ConnectionValidationResult) {
    llmSuccessVisibleUntilRef.current = result.success
      ? Date.now() + VALIDATION_SUCCESS_FEEDBACK_MS
      : 0;
    setLlmValidationState(result);
  }

  async function goToStep(step: number) {
    if (step === currentStep) {
      return;
    }

    setBusyAction("step");
    try {
      await persistConfig({
        onboardingCurrentStep: step,
        onboardingLlmProviderDraft: llmProvider,
      });
      setPreviousStep(currentStep);
      setCurrentStep(step);
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleExit() {
    setBusyAction("exit");
    try {
      if (source === "auto") {
        await persistConfig({
          onboardingCompleted: true,
          onboardingCurrentStep: currentStep,
          onboardingLlmProviderDraft: llmProvider,
        });
        await onExit("app");
        return;
      }

      await persistConfig({
        onboardingCurrentStep: currentStep,
        onboardingLlmProviderDraft: llmProvider,
      });
      await onExit("settings");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleValidateSteam() {
    const draftKey = steamApiKey.trim();
    if (!draftKey && !savedConfig.steamApiKeyConfigured) {
      const message = "请先输入当前要测试的 Steam Web API Key。";
      showSteamValidationResult({
        success: false,
        message,
        diagnostic: "如果已经在设置中保存过 Key，可以留空使用已保存的 Key 测试。",
      });
      setNotice(message);
      return;
    }

    setBusyAction("steam-test");
    try {
      const result = await validateSteamConfig({
        steamApiKey: draftKey || undefined,
      });
      showSteamValidationResult(result);
      setNotice(result.message);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      showSteamValidationResult({
        success: false,
        message,
        diagnostic: "你仍然可以直接保存，之后再回到设置里重试。",
      });
      setNotice(message);
    } finally {
      setBusyAction(null);
    }
  }

  async function handleSaveSteamAndContinue(validated: boolean) {
    setBusyAction("steam-save");
    try {
      if (!canSaveSteam) {
        setNotice("请先输入 Steam Key，或者直接跳过这一部分。");
        return;
      }

      await persistConfig({
        steamApiKey: steamDraftDirty ? steamApiKey : undefined,
        steamApiKeyValidated: validated,
        onboardingCurrentStep: 4,
        onboardingLlmProviderDraft: llmProvider,
      });
      setPreviousStep(currentStep);
      setCurrentStep(4);
      setSteamApiKey("");
      setNotice(validated ? "Steam Key 已保存并标记为已验证。" : "Steam Key 已保存，但还未验证。");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleSteamNext() {
    if (!canSaveSteam) {
      await goToStep(4);
      return;
    }

    await handleSaveSteamAndContinue(
      Boolean(steamValidation?.success) || steamCanReuseSavedValidation,
    );
  }

  async function handleValidateLlm() {
    const draftKey = llmApiKey.trim();
    const draftBaseUrl = llmBaseUrl.trim();
    const draftModel = llmModel.trim();
    if (!draftKey && !savedConfig.llmApiKeyConfigured) {
      const message = "请先输入当前要测试的 AI API Key。";
      showLlmValidationResult({
        success: false,
        message,
        diagnostic: "如果已经在设置中保存过 Key，可以留空使用已保存的 Key 测试。",
        provider: llmProvider,
        baseUrl: draftBaseUrl,
        model: draftModel,
      });
      setNotice(message);
      return;
    }
    if (!draftBaseUrl || !draftModel) {
      const message = "请确认 Base URL 和模型名称已填写。";
      showLlmValidationResult({
        success: false,
        message,
        diagnostic: "标准提供方会自动填入默认值，自定义提供方需要手动填写。",
        provider: llmProvider,
        baseUrl: draftBaseUrl,
        model: draftModel,
      });
      setNotice(message);
      return;
    }

    setBusyAction("llm-test");
    try {
      const result = await validateLlmConfig({
        provider: llmProvider,
        apiKey: draftKey || undefined,
        baseUrl: draftBaseUrl,
        model: draftModel,
      });
      showLlmValidationResult(result);
      setNotice(result.message);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      showLlmValidationResult({
        success: false,
        message,
        diagnostic: "你仍然可以直接保存，之后再回到设置里重试。",
        provider: llmProvider,
        baseUrl: llmBaseUrl,
        model: llmModel,
      });
      setNotice(message);
    } finally {
      setBusyAction(null);
    }
  }

  async function handleSaveLlmAndFinish(validated: boolean) {
    setBusyAction("llm-save");
    try {
      if (!canSaveLlm && !savedConfig.llmConfigValidated) {
        setNotice("请先输入 AI Key，或者使用“跳过 AI，先进入应用”。");
        return;
      }

      await persistConfig({
        llmProvider,
        llmApiKey: llmApiKey.trim() ? llmApiKey : undefined,
        clearLlmApiKey: llmProvider !== savedConfig.llmProvider && !llmApiKey.trim(),
        llmBaseUrl,
        llmModel,
        llmConfigValidated: validated,
        onboardingCompleted: true,
        onboardingCurrentStep: 5,
        onboardingLlmProviderDraft: llmProvider,
      });
      await onExit(source === "settings" ? "settings" : "app");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      setBusyAction(null);
    }
  }

  async function handleSkipAiAndFinish() {
    setBusyAction("llm-save");
    try {
      await persistConfig({
        onboardingCompleted: true,
        onboardingCurrentStep: 5,
        onboardingLlmProviderDraft: llmProvider,
      });
      await onExit(source === "settings" ? "settings" : "app");
    } catch (error) {
      setNotice(error instanceof Error ? error.message : String(error));
    } finally {
      setBusyAction(null);
    }
  }

  const headerExitLabel = source === "auto" ? "稍后设置" : "返回设置";
  const stepMotionClass = currentStep >= previousStep ? "forward" : "backward";

  return (
    <main className="onboarding-shell">
      <div className="onboarding-head">
        <div className="brand-row onboarding-brand">
          <LogoMark />
          <div>
            <strong>Co-Play</strong>
            <span>发现好玩的多人游戏</span>
          </div>
        </div>
        <button className="onboarding-top-action" type="button" onClick={() => void handleExit()}>
          {busyAction === "exit" ? "处理中…" : headerExitLabel}
        </button>
      </div>

      <section className="onboarding-frame">
        <aside
          className="onboarding-sidebar"
          style={{ "--onboarding-active-step": currentStep - 1 } as CSSProperties}
        >
          <div className="onboarding-sidebar-track" aria-hidden="true" />
          <div className="onboarding-step-glider" aria-hidden="true" />
          {STEP_ITEMS.map((step) => {
            const complete = stepState[step.id as keyof typeof stepState];
            const active = currentStep === step.id;
            const visualDone = !active && (step.id < currentStep || complete);
            return (
              <button
                key={step.id}
                type="button"
                className={[
                  "onboarding-step",
                  active ? "active" : "",
                  visualDone ? "done" : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
                onClick={() => void goToStep(step.id)}
              >
                <span className="onboarding-step-index">
                  {visualDone ? "✓" : step.id}
                </span>
                <span>{step.title}</span>
              </button>
            );
          })}
        </aside>

        <div className={`onboarding-content ${stepMotionClass}`} key={currentStep}>
          {currentStep === 1 && (
            <div className="onboarding-panel onboarding-panel-welcome">
              <div className="onboarding-copy">
                <h1>欢迎使用 Co-Play</h1>
                <p>
                  首次使用只需约 2 分钟的简单设置，我们将引导你准备并填写
                  <strong> Steam Web API Key </strong>
                  与
                  <strong> {currentProviderLabel} API Key </strong>
                  ，以便获取多人游戏数据并为你提供 AI 智能推荐。
                </p>
                <p>你可以现在添加，也可以之后在「设置」中随时修改。</p>
              </div>

              <div className="onboarding-hero-image-wrap">
                <img
                  src="/assets/onboarding-welcome-hero.png"
                  alt="Co-Play 初始设置示意"
                  className="onboarding-img onboarding-welcome-hero-img"
                />
              </div>

              <WizardActions note="你的 API Key 仅保存在本地设备，用于调用官方接口。">
                <button className="gold-button onboarding-primary onboarding-next-button" type="button" onClick={() => void goToStep(2)}>
                  下一步
                </button>
                {source === "auto" ? (
                  <button className="ghost-button onboarding-secondary" type="button" onClick={() => void handleExit()}>
                    稍后再说
                  </button>
                ) : null}
              </WizardActions>
            </div>
          )}

          {currentStep === 2 && (
            <div className="onboarding-panel onboarding-panel-steam-prep">
              <div className="onboarding-copy">
                <h1>准备 Steam Web API</h1>
                <p>
                  Co-Play 通过 Steam Web API 获取多人游戏的元数据、评测数量、发行信息以及热度等信号，从而为你推荐更合适的多人游戏。
                </p>
              </div>

              <div className="onboarding-steam-prep-layout">
                <article className="onboarding-card onboarding-steam-steps">
                  <h3>获取 Steam Web API Key</h3>
                  <div className="onboarding-procedure-list">
                    <ProcedureItem
                      index={1}
                      iconSrc="/assets/onboarding-steam-icon-account.png"
                      title="登录 Steam 账号"
                      description="使用你的 Steam 账号登录。"
                    />
                    <ProcedureItem
                      index={2}
                      iconSrc="/assets/onboarding-steam-icon-web.png"
                      title="打开 Steam Web API 申请页"
                      description="进入 Steam 官方的 API 申请页面。"
                    />
                    <ProcedureItem
                      index={3}
                      iconSrc="/assets/onboarding-steam-icon-key.png"
                      title="填写域名并生成 Key"
                      description="填写域名（可为 localhost），勾选同意并生成你的 API Key。"
                    />
                  </div>
                  <article className="onboarding-tip-card">
                    <strong>
                      <Icon name="bulb" />
                      小贴士
                    </strong>
                    <p>本地测试时，域名可以填写 localhost 或你自己的网站域名（如：example.com）。</p>
                  </article>
                  <LinkRow label="申请页面地址：" value="steamcommunity.com/dev/apikey" />
                </article>

                <article className="onboarding-card onboarding-steam-preview">
                  <h3>流程示意</h3>
                  <div className="onboarding-preview-browser">
                    <div className="onboarding-preview-browser-bar">
                      <span className="onboarding-steam-logo"><Icon name="steam" /> STEAM</span>
                      <span>商店</span>
                      <span>社区</span>
                      <span>关于</span>
                      <span>客服</span>
                    </div>
                    <div className="onboarding-preview-browser-body">
                      <strong>创建 Steam Web API Key</strong>
                      <label className="onboarding-mini-field">
                        <span>域名名称（可包含端口号）</span>
                        <input value="localhost" readOnly />
                      </label>
                      <span className="onboarding-mini-muted">例：localhost 或 yourdomain.com</span>
                      <span className="onboarding-mini-check">☑ 我同意 Steam Web API 使用条款</span>
                      <span className="onboarding-mini-cta">注册 <Icon name="chevronRight" /></span>
                    </div>
                  </div>
                  <span className="onboarding-flow-arrow">↓</span>
                  <div className="onboarding-preview-key">
                    <span>生成的 API Key</span>
                    <div className="onboarding-preview-key-row">
                      <span>A1B2C3D4E5F6G7H8I9J0K1L2M3N4O5P6</span>
                      <Icon name="copy" />
                    </div>
                  </div>
                  <span className="onboarding-flow-arrow">↓</span>
                  <div className="onboarding-preview-app">
                    <div className="brand-row onboarding-inline-brand">
                      <LogoMark />
                      <strong>Co-Play</strong>
                    </div>
                    <span>粘贴你的 Steam Web API Key</span>
                    <div className="onboarding-preview-key-row secure">
                      <span>**********************</span>
                      <Icon name="lock" />
                      <b>✓</b>
                    </div>
                  </div>
                </article>
              </div>

              <WizardActions note="我们不会上传你的 Key，仅在本地用于调用官方接口。">
                <button
                  className="gold-button onboarding-primary onboarding-external-button"
                  type="button"
                  onClick={() => void openExternalUrl(STEAM_APPLY_URL)}
                >
                  前往申请页面
                </button>
                <button className="ghost-button onboarding-secondary onboarding-next-button" type="button" onClick={() => void goToStep(3)}>
                  下一步
                </button>
                <button className="ghost-button onboarding-secondary" type="button" onClick={() => void goToStep(1)}>
                  上一步
                </button>
              </WizardActions>
            </div>
          )}

          {currentStep === 3 && (
            <div className="onboarding-panel onboarding-panel-steam-key">
              <div className="onboarding-copy">
                <h1>填写 Steam Key</h1>
                <p>将你刚刚申请到的 Steam Web API Key 粘贴到下方，我们会先在本地验证可用性。</p>
              </div>

              <div className="onboarding-steam-key-layout">
                <article className="onboarding-card onboarding-form-card onboarding-steam-form">
                  <label className="onboarding-field">
                    <span>Steam Web API Key</span>
                    <div className="onboarding-input-with-button">
                      <input
                        type="password"
                        value={steamApiKey}
                        placeholder={
                          savedConfig.steamApiKeyConfigured ? "已配置，输入新值可覆盖" : "******************************"
                        }
                        onChange={(event) => {
                          setSteamApiKey(event.currentTarget.value);
                          steamSuccessVisibleUntilRef.current = 0;
                          setSteamValidationState(null);
                        }}
                      />
                      <button type="button" aria-label="粘贴 Steam Key">
                        <Icon name="clipboard" />
                        粘贴
                      </button>
                    </div>
                  </label>

                  <label className="onboarding-checkbox-row">
                    <input type="checkbox" checked readOnly />
                    <span>启动时自动检测更新</span>
                    <Icon name="help" />
                  </label>

                  <div className="onboarding-test-row">
                    <button
                      className="ghost-button onboarding-test-button"
                      type="button"
                      disabled={busyAction === "steam-test"}
                      onClick={() => void handleValidateSteam()}
                    >
                      {busyAction === "steam-test" ? "Steam 测试中…" : "测试 Steam 连接"}
                    </button>
                    <span>先测试再继续，可以确认 Key 能读取 Steam 基础应用数据。</span>
                  </div>
                </article>

                <article className="onboarding-card onboarding-side-benefit-card">
                  <h3>为什么需要它？</h3>
                  <ul className="onboarding-icon-list">
                    <li><Icon name="appList" /> 获取游戏基础信息</li>
                    <li><Icon name="chart" /> 同步发售与好评数据</li>
                    <li><Icon name="filter" /> 辅助新游 / 老游筛选</li>
                  </ul>
                </article>
              </div>

              <SteamValidationPanel
                result={steamValidation}
                savedConfig={savedConfig}
                isTesting={busyAction === "steam-test"}
              />

              <WizardActions note="你的 API Key 仅保存在本地设备，用于调用官方接口。">
                <button
                  className="gold-button onboarding-primary onboarding-next-button"
                  type="button"
                  disabled={busyAction === "steam-save" || busyAction === "step"}
                  onClick={() => void handleSteamNext()}
                >
                  {busyAction === "steam-save" ? "保存中…" : "下一步"}
                </button>
                <button className="ghost-button onboarding-secondary" type="button" onClick={() => void goToStep(2)}>
                  上一步
                </button>
                <button className="text-button" type="button" onClick={() => void goToStep(4)}>
                  稍后在设置中修改
                </button>
              </WizardActions>
            </div>
          )}

          {currentStep === 4 && (
            <div className="onboarding-panel onboarding-panel-llm-prep">
              <div className="onboarding-copy">
                <h1>准备 {currentProviderLabel} API</h1>
                <p>
                  Co-Play 使用 <strong>{currentProviderLabel}</strong> 提供的大模型 API，对多人游戏基于玩家人数、评测分数、混合评测样本、发行日期等多维信号进行综合分析，生成简洁、可靠的推荐值。
                </p>
              </div>

              <div className="onboarding-llm-prep-top">
                <article className="onboarding-provider-steps">
                  <h3>获取 API Key 只需 3 步</h3>
                  <div className="onboarding-step-cards">
                    {llmSetupSteps.map((step) => (
                      <div key={step.id} className="onboarding-step-card">
                        <span className="onboarding-step-card-index">{step.id}</span>
                        <span className="onboarding-step-icon">
                          <img
                            src={`/assets/onboarding-ai-icon-${
                              step.id === 1 ? "account" : step.id === 2 ? "key" : "copy"
                            }.png`}
                            alt=""
                          />
                        </span>
                        <strong>{step.title}</strong>
                        <p>{step.description}</p>
                      </div>
                    ))}
                  </div>
                  <LinkRow label="平台地址：" value={currentProviderPortalHost} />
                </article>

                <article className="onboarding-card onboarding-ai-explainer">
                  <h3>LLM 在本应用中的作用</h3>
                  <ul className="onboarding-bullet-list feature">
                    <li>总结优缺点</li>
                    <li>生成推荐值</li>
                    <li>避免冷门精品被忽略</li>
                    <li>辅助标签筛选</li>
                  </ul>
                </article>
              </div>

              <article className="onboarding-card onboarding-ai-pipeline-card">
                <div className="onboarding-pipeline-head">
                  <h3>AI 如何生成推荐值</h3>
                  <p>先读取 Steam 多维信号，再交给 {currentProviderLabel} 分析优缺点，最后输出可扫读的推荐结论。</p>
                </div>

                <div className="onboarding-pipeline-grid">
                  <section className="onboarding-pipeline-panel input">
                    <span className="onboarding-pipeline-step">01</span>
                    <h4>多维信号输入</h4>
                    <ul className="onboarding-metric-list">
                      <li><span>玩家人数</span><b>3,102 当前在线</b></li>
                      <li><span>评测分数</span><b>90% 好评</b></li>
                      <li><span>评测样本</span><b>10,230 条评测</b></li>
                      <li><span>发行日期</span><b>2024-05-10</b></li>
                      <li><span>其他信号</span><b>标签、语言、时长</b></li>
                    </ul>
                  </section>

                  <section className="onboarding-pipeline-panel analysis">
                    <span className="onboarding-pipeline-step">02</span>
                    <h4>{currentProviderLabel} 分析</h4>
                    <div className="onboarding-analysis-card good">
                      <strong>积极评价</strong>
                      <p>合作乐趣十足，角色技能设计有深度，匹配体验温和。</p>
                    </div>
                    <div className="onboarding-analysis-card soft">
                      <strong>消极评价</strong>
                      <p>后期内容重复度较高，部分职业平衡性有待优化。</p>
                    </div>
                  </section>

                  <section className="onboarding-pipeline-panel sentiment">
                    <span className="onboarding-pipeline-step">03</span>
                    <h4>情感倾向</h4>
                    <div className="onboarding-sentiment-ring"><strong>8 : 2</strong></div>
                    <span>正面 : 负面</span>
                  </section>

                  <section className="onboarding-pipeline-panel score">
                    <span className="onboarding-pipeline-step">04</span>
                    <h4>生成推荐值</h4>
                    <div className="onboarding-score-ring"><span className="onboarding-score-value">88</span></div>
                    <span>推荐值</span>
                    <div className="onboarding-stars">★★★★<span>★</span></div>
                    <b>非常推荐</b>
                  </section>
                </div>
              </article>

              <WizardActions note="请妥善保管 API Key，建议只在自己的设备上使用。">
                <button
                  className="gold-button onboarding-primary onboarding-external-button"
                  type="button"
                  onClick={() => void openExternalUrl(providerPortalUrl(llmProvider))}
                >
                  前往 {currentProviderLabel} 平台
                </button>
                <button className="ghost-button onboarding-secondary onboarding-next-button" type="button" onClick={() => void goToStep(5)}>
                  下一步
                </button>
                <button className="ghost-button onboarding-secondary" type="button" onClick={() => void goToStep(3)}>
                  上一步
                </button>
              </WizardActions>
            </div>
          )}

          {currentStep === 5 && (
            <div className="onboarding-panel onboarding-panel-complete">
              <div className="onboarding-copy">
                <h1>填写 {currentProviderLabel} Key 并完成配置</h1>
                <p>输入 {currentProviderLabel} API Key，选择默认模型并测试连接，完成后即可开始使用 AI 辅助推荐。</p>
              </div>

              <div className="onboarding-complete-layout">
                <div className="onboarding-complete-left">
                  <article className="onboarding-card onboarding-form-card onboarding-llm-form">
                    <label className="onboarding-field">
                      <span>{currentProviderLabel} API Key</span>
                      <div className="onboarding-input-with-button">
                        <input
                          type="password"
                          value={llmApiKey}
                          placeholder={
                            savedConfig.llmApiKeyConfigured ? "已配置，输入新值可覆盖" : "******************************"
                          }
                          onChange={(event) => {
                            setLlmApiKey(event.currentTarget.value);
                            llmSuccessVisibleUntilRef.current = 0;
                            setLlmValidationState(null);
                          }}
                        />
                        <button type="button" aria-label="粘贴 AI Key">
                          <Icon name="clipboard" />
                          粘贴
                        </button>
                      </div>
                    </label>

                    <label className="onboarding-field">
                      <span>默认模型</span>
                      <select
                        value={llmModel}
                        onChange={(event) => {
                          setLlmModel(event.currentTarget.value);
                          llmSuccessVisibleUntilRef.current = 0;
                          setLlmValidationState(null);
                        }}
                      >
                        <option value={llmModel}>{llmModel || getDefaultLlmModel(llmProvider)}</option>
                      </select>
                    </label>

                    <label className="onboarding-field">
                      <span>推荐分析模式</span>
                      <select defaultValue="standard">
                        <option value="standard">标准</option>
                        <option value="balanced">均衡</option>
                        <option value="strict">严格</option>
                      </select>
                    </label>

                    <label className="onboarding-checkbox-row">
                      <input type="checkbox" checked readOnly />
                      <span>允许在新游区自动生成 AI 推荐值</span>
                    </label>

                    <div className="onboarding-test-row">
                      <button
                        className="ghost-button onboarding-test-button"
                        type="button"
                        disabled={busyAction === "llm-test"}
                        onClick={() => void handleValidateLlm()}
                      >
                        {busyAction === "llm-test" ? "AI 测试中…" : "测试 AI 连接"}
                      </button>
                      <span>测试会发起一次最小模型调用，用来确认 Key、Base URL 与模型可用。</span>
                    </div>

                    {llmProvider !== "custom" ? (
                      <button
                        className="text-button onboarding-advanced-toggle"
                        type="button"
                        onClick={() => setShowAdvanced((current) => !current)}
                      >
                        {showAdvanced ? "收起高级设置" : "展开高级设置"}
                      </button>
                    ) : null}

                    {(showAdvanced || llmProvider === "custom") && (
                      <div className="onboarding-advanced">
                        <label className="onboarding-field">
                          <span>Base URL</span>
                          <input
                            value={llmBaseUrl}
                            onChange={(event) => {
                              setLlmBaseUrl(event.currentTarget.value);
                              llmSuccessVisibleUntilRef.current = 0;
                              setLlmValidationState(null);
                            }}
                          />
                        </label>
                      </div>
                    )}
                  </article>

                  <div className="onboarding-complete-art">
                    <img
                      src="/assets/onboarding-complete-hero.png"
                      alt="配置完成插画"
                      className="onboarding-complete-image"
                    />
                  </div>
                </div>

                <div className="onboarding-complete-side">
                  <LlmValidationPanel
                    result={llmValidation}
                    model={llmModel || getDefaultLlmModel(llmProvider)}
                    isTesting={busyAction === "llm-test"}
                  />

                  <article className="onboarding-card onboarding-benefit-summary">
                    <h3>配置完成后你可以：</h3>
                    <ul className="onboarding-dot-list">
                      <li>浏览新游区与精品老游区</li>
                      <li>使用 AI 智能推荐助手</li>
                      <li>根据评分 / 玩家数 / 发售时间筛选</li>
                    </ul>
                  </article>
                </div>
              </div>

              <WizardActions note="你也可以稍后在设置 → API 配置中修改这些内容。">
                <button
                  className="gold-button onboarding-primary"
                  type="button"
                  disabled={!canSaveLlm && !savedConfig.llmConfigValidated}
                  onClick={() =>
                    void handleSaveLlmAndFinish(
                      Boolean(llmValidation?.success) || llmCanReuseSavedValidation,
                    )
                  }
                >
                  {busyAction === "llm-save" ? "处理中…" : "下一步：进入应用"}
                </button>
                <button className="ghost-button onboarding-secondary" type="button" onClick={() => void goToStep(4)}>
                  上一步
                </button>
                <button className="text-button" type="button" onClick={() => void handleSkipAiAndFinish()}>
                  跳过 AI，先进入应用
                </button>
              </WizardActions>
            </div>
          )}

          {notice ? <p className="onboarding-notice">{notice}</p> : null}
        </div>
      </section>
    </main>
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

function WizardActions({
  children,
  note,
}: {
  children: ReactNode;
  note: string;
}) {
  return (
    <div className="onboarding-action-zone">
      <div className="wizard-button-row">{children}</div>
      <p className="onboarding-footer-note">{note}</p>
    </div>
  );
}

function ProcedureItem({
  index,
  iconSrc,
  title,
  description,
}: {
  index: number;
  iconSrc: string;
  title: string;
  description: string;
}) {
  return (
    <div className="onboarding-procedure-item">
      <span className="onboarding-procedure-index">{index}</span>
      <span className="onboarding-procedure-icon">
        <img src={iconSrc} alt="" />
      </span>
      <div>
        <strong>{title}</strong>
        <p>{description}</p>
      </div>
    </div>
  );
}

function LinkRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="onboarding-link-row">
      <span>{label}</span>
      <code>{value}</code>
    </div>
  );
}

function SteamValidationPanel({
  result,
  savedConfig,
  isTesting,
}: {
  result: ConnectionValidationResult | null;
  savedConfig: PublicConfig;
  isTesting: boolean;
}) {
  const savedSuccess = !result && savedConfig.steamApiKeyValidated;
  const success = Boolean(result?.success || savedSuccess);
  const failure = Boolean(result && !result.success);
  const stateClass = isTesting ? "testing" : failure ? "error" : success ? "success" : "neutral";
  const title = isTesting
    ? "连接测试：测试中"
    : failure
      ? "连接测试：失败"
      : success
        ? "连接测试：成功"
        : "连接测试：等待测试";
  const message = isTesting
    ? "正在请求 Steam AppList 预览接口。"
    : result?.message ??
      (savedSuccess ? "当前已保存的 Steam Key 最近一次测试成功。" : "输入 Steam Web API Key 后点击测试。");
  const diagnostic = isTesting
    ? "通常几秒内会返回结果，请保持网络连接。"
    : result?.diagnostic ??
      (success ? "如果你输入了新 Key，需要重新测试。" : "测试不会保存 Key，只会验证当前输入。");
  const latency = isTesting ? "测试中" : typeof result?.latencyMs === "number" ? `${result.latencyMs}ms` : "未测试";
  const appCount = typeof result?.appCount === "number" ? `${result.appCount} 个` : success ? "已获取" : "未获取";
  const statusIcon = isTesting ? <Icon name="timer" /> : success ? <Icon name="check" /> : failure ? "!" : <Icon name="timer" />;

  return (
    <article className={`onboarding-validation-wide ${stateClass}`}>
      <div className="onboarding-validation-status">
        <span className="onboarding-big-check">{statusIcon}</span>
        <div>
          <h3>{title}</h3>
          <p><Icon name={failure ? "help" : "checkSmall"} /> {message}</p>
          <p><Icon name={failure ? "help" : "checkSmall"} /> {diagnostic}</p>
        </div>
      </div>
      <dl className="onboarding-validation-meta">
        <div><dt>响应时间</dt><dd>{latency}</dd></div>
        <div><dt>读取数据</dt><dd>{appCount}</dd></div>
        <div><dt>测试状态</dt><dd>{failure ? "需要重试" : success ? "可以继续" : isTesting ? "正在测试" : "等待输入"}</dd></div>
      </dl>
    </article>
  );
}

function LlmValidationPanel({
  result,
  model,
  isTesting,
}: {
  result: ConnectionValidationResult | null;
  model: string;
  isTesting: boolean;
}) {
  const success = Boolean(result?.success);
  const failure = Boolean(result && !result.success);
  const stateClass = isTesting ? "testing" : failure ? "error" : success ? "success" : "neutral";
  const latency = isTesting ? "测试中" : typeof result?.latencyMs === "number" ? `${result.latencyMs}ms` : "未测试";
  const title = isTesting ? "AI 连接测试中" : failure ? "AI 连接测试失败" : success ? "AI 连接测试成功" : "等待 AI 连接测试";
  const diagnostic = isTesting
    ? "正在发起一次最小模型调用。"
    : result?.diagnostic ?? (success ? "已完成最小模型调用探测。" : "输入 API Key 后点击测试。");

  return (
    <article className={`onboarding-llm-validation ${stateClass}`}>
      <div className="onboarding-llm-validation-head">
        <span className="onboarding-big-check">{isTesting ? <Icon name="timer" /> : success ? <Icon name="check" /> : failure ? "!" : <Icon name="timer" />}</span>
        <div>
          <h3>{title}</h3>
          <p>{result?.message ?? diagnostic}</p>
        </div>
      </div>
      <dl>
        <div><dt><Icon name="cube" /> 模型：</dt><dd>{result?.model ?? model}</dd></div>
        <div><dt><Icon name="timer" /> 响应延迟：</dt><dd>{latency}</dd></div>
        <div><dt><Icon name="shieldCheck" /> 测试状态：</dt><dd>{failure ? "需要重试" : success ? "可以继续" : isTesting ? "正在测试" : "等待输入"}</dd></div>
        {result?.diagnostic ? <div><dt><Icon name="help" /> 诊断：</dt><dd>{result.diagnostic}</dd></div> : null}
      </dl>
    </article>
  );
}

type IconName =
  | "appList"
  | "bulb"
  | "calendar"
  | "chart"
  | "chat"
  | "check"
  | "checkSmall"
  | "chevronRight"
  | "clipboard"
  | "clipboardKey"
  | "copy"
  | "cube"
  | "external"
  | "filter"
  | "globe"
  | "help"
  | "key"
  | "keyLarge"
  | "link"
  | "lock"
  | "more"
  | "person"
  | "shieldCheck"
  | "sparkle"
  | "star"
  | "steam"
  | "timer"
  | "users"
  | "whale";

function Icon({ name }: { name: IconName }) {
  switch (name) {
    case "appList":
      return <span className="onboarding-icon" aria-hidden="true">▤</span>;
    case "bulb":
      return <span className="onboarding-icon" aria-hidden="true">◐</span>;
    case "calendar":
      return <span className="onboarding-icon" aria-hidden="true">▣</span>;
    case "chart":
      return <span className="onboarding-icon" aria-hidden="true">⌁</span>;
    case "chat":
      return <span className="onboarding-icon" aria-hidden="true">●</span>;
    case "check":
      return <span className="onboarding-icon" aria-hidden="true">✓</span>;
    case "checkSmall":
      return <span className="onboarding-icon" aria-hidden="true">✓</span>;
    case "chevronRight":
      return <span className="onboarding-icon" aria-hidden="true">›</span>;
    case "clipboard":
      return <span className="onboarding-icon" aria-hidden="true">▧</span>;
    case "clipboardKey":
      return <span className="onboarding-icon" aria-hidden="true">▣</span>;
    case "copy":
      return <span className="onboarding-icon" aria-hidden="true">⧉</span>;
    case "cube":
      return <span className="onboarding-icon" aria-hidden="true">◇</span>;
    case "external":
      return <span className="onboarding-icon" aria-hidden="true">↗</span>;
    case "filter":
      return <span className="onboarding-icon" aria-hidden="true">▽</span>;
    case "globe":
      return <span className="onboarding-icon" aria-hidden="true">◉</span>;
    case "help":
      return <span className="onboarding-icon" aria-hidden="true">?</span>;
    case "key":
    case "keyLarge":
      return <span className="onboarding-icon" aria-hidden="true">⚿</span>;
    case "link":
      return <span className="onboarding-icon" aria-hidden="true">ↄ</span>;
    case "lock":
      return <span className="onboarding-icon" aria-hidden="true">▢</span>;
    case "more":
      return <span className="onboarding-icon" aria-hidden="true">•••</span>;
    case "person":
      return <span className="onboarding-icon" aria-hidden="true">●</span>;
    case "shieldCheck":
      return <span className="onboarding-icon" aria-hidden="true">▾</span>;
    case "sparkle":
      return <span className="onboarding-icon" aria-hidden="true">✦</span>;
    case "star":
      return <span className="onboarding-icon" aria-hidden="true">★</span>;
    case "steam":
      return <span className="onboarding-icon" aria-hidden="true">◍</span>;
    case "timer":
      return <span className="onboarding-icon" aria-hidden="true">◷</span>;
    case "users":
      return <span className="onboarding-icon" aria-hidden="true">●●</span>;
    case "whale":
      return <span className="onboarding-icon" aria-hidden="true">◥</span>;
  }
}

function resolveEntryStep(config: PublicConfig, source: OnboardingSource) {
  if (config.onboardingCompleted) {
    return source === "settings" ? clampStep(config.onboardingCurrentStep) : 1;
  }

  const incompleteStep = resolveNextIncompleteStep(config);
  const currentStep = clampStep(config.onboardingCurrentStep);

  // If the user hasn't started or finished, respect the last step they were on
  // unless that step is effectively finished.
  if (currentStep > 1 && currentStep <= incompleteStep) {
    return currentStep;
  }

  return incompleteStep;
}

function resolveNextIncompleteStep(config: PublicConfig) {
  if (!config.steamApiKeyValidated) {
    return config.steamApiKeyConfigured ? 3 : 2;
  }

  if (!config.llmConfigValidated) {
    return config.llmApiKeyConfigured ? 5 : 4;
  }

  return 1;
}

function resolveSteamValidationFromConfig(config: PublicConfig): ConnectionValidationResult | null {
  return config.steamApiKeyValidated
    ? {
        success: true,
        message: "当前已保存的 Steam Key 最近一次测试成功。",
        diagnostic: "如果你输入了新 Key，需要重新测试。",
      }
    : null;
}

function resolveLlmValidationFromConfig(config: PublicConfig): ConnectionValidationResult | null {
  return config.llmConfigValidated
    ? {
        success: true,
        message: "当前已保存的 AI 配置最近一次测试成功。",
        diagnostic: "如果你修改了提供方、Key、Base URL 或模型，需要重新测试。",
        provider: config.llmProvider,
        baseUrl: config.llmBaseUrl,
        model: config.llmModel,
      }
    : null;
}

function getSuccessFeedbackRemainingMs(
  result: ConnectionValidationResult | null,
  visibleUntil: number,
) {
  if (!result?.success) {
    return 0;
  }

  return Math.max(0, visibleUntil - Date.now());
}

function resolveInitialBaseUrl(config: PublicConfig, provider: LlmProvider) {
  return config.llmProvider === provider ? config.llmBaseUrl : getDefaultLlmBaseUrl(provider);
}

function resolveInitialModel(config: PublicConfig, provider: LlmProvider) {
  return config.llmProvider === provider ? config.llmModel : getDefaultLlmModel(provider);
}

function clampStep(step: number) {
  return Math.min(5, Math.max(1, Math.round(step || 1)));
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

function providerPortalUrl(provider: LlmProvider) {
  switch (provider) {
    case "openai":
      return "https://platform.openai.com/api-keys";
    case "anthropic":
      return "https://console.anthropic.com/settings/keys";
    case "custom":
      return "https://platform.openai.com/api-keys";
    case "deepseek":
    default:
      return "https://platform.deepseek.com/api_keys";
  }
}

function providerSetupSteps(provider: LlmProvider) {
  switch (provider) {
    case "openai":
      return [
        { id: 1, title: "登录 OpenAI 平台", description: "使用你的 OpenAI 账号进入控制台。" },
        { id: 2, title: "打开 API Keys 页面", description: "在平台中找到 API Keys 入口。" },
        { id: 3, title: "创建新 Key 并保存", description: "生成新 Key 后立即复制并妥善保存。" },
      ];
    case "anthropic":
      return [
        { id: 1, title: "登录 Anthropic 控制台", description: "进入 Anthropic Console 并完成登录。" },
        { id: 2, title: "找到 Keys 设置", description: "打开控制台中的 API Keys 页面。" },
        { id: 3, title: "创建 Key 并复制", description: "生成后复制保存，再回到 Co-Play 粘贴。" },
      ];
    case "custom":
      return [
        { id: 1, title: "确认兼容接口", description: "准备一个 OpenAI-compatible 的服务地址。" },
        { id: 2, title: "拿到 API Key", description: "在服务方后台创建可调用的 API Key。" },
        { id: 3, title: "准备 Base URL 与模型名", description: "记录 Base URL 和你要使用的模型。" },
      ];
    case "deepseek":
    default:
      return [
        { id: 1, title: "注册 / 登录 DeepSeek 平台", description: "使用手机号或邮箱完成注册并登录。" },
        { id: 2, title: "进入 API Keys 页面", description: "在控制台中找到 API Keys 入口。" },
        { id: 3, title: "创建新的 API Key", description: "创建后立即复制并妥善保存 Key。" },
      ];
  }
}

function displayUrl(url: string) {
  return url.replace(/^https?:\/\//, "").replace(/\/$/, "");
}

async function openExternalUrl(url: string) {
  if (isTauriRuntime()) {
    await openUrl(url);
    return;
  }

  window.open(url, "_blank", "noopener,noreferrer");
}

function scrollWindowToTop() {
  if (typeof navigator !== "undefined" && /jsdom/i.test(navigator.userAgent)) {
    document.documentElement.scrollTop = 0;
    document.body.scrollTop = 0;
    return;
  }

  if (typeof window.scrollTo !== "function") {
    return;
  }

  try {
    window.scrollTo({ top: 0, behavior: "auto" });
  } catch {
    document.documentElement.scrollTop = 0;
    document.body.scrollTop = 0;
  }
}
