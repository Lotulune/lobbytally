import { act } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, it, expect, vi } from "vitest";
import { OnboardingWizard } from "./OnboardingWizard";
import * as clientApi from "../../api/client";

const baseConfig = {
  steamApiKeyConfigured: false,
  steamApiKeyValidated: false,
  llmProvider: "deepseek" as const,
  llmApiKeyConfigured: false,
  llmConfigValidated: false,
  llmBaseUrl: "https://api.deepseek.com",
  llmModel: "deepseek-v4-flash",
  country: "CN",
  language: "schinese",
  aiBatchRefreshConcurrency: 2,
  onboardingCompleted: false,
  onboardingCurrentStep: 2,
  onboardingLlmProviderDraft: "deepseek" as const,
};

async function flushValidation() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

describe("OnboardingWizard", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("renders correctly with default config", () => {
    render(
      <OnboardingWizard
        config={baseConfig}
        source="auto"
        onExit={() => {}}
      />
    );
    expect(screen.getByRole("heading", { level: 1, name: "准备 Steam Web API" })).toBeDefined();
  });

  it("does not reset to the welcome step when completed settings onboarding config refreshes", async () => {
    const completedConfig = {
      ...baseConfig,
      steamApiKeyConfigured: true,
      steamApiKeyValidated: true,
      llmApiKeyConfigured: true,
      llmConfigValidated: true,
      onboardingCompleted: true,
      onboardingCurrentStep: 1,
    };
    const { rerender } = render(
      <OnboardingWizard
        config={completedConfig}
        source="settings"
        onExit={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1, name: "准备 Steam Web API" })).toBeInTheDocument();
    });

    rerender(
      <OnboardingWizard
        config={{ ...completedConfig, onboardingCurrentStep: 2 }}
        source="settings"
        onExit={() => {}}
      />,
    );

    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1, name: "准备 Steam Web API" })).toBeInTheDocument();
    });
    expect(screen.queryByRole("heading", { level: 1, name: "欢迎使用 Co-Play" })).not.toBeInTheDocument();
  });

  it("tests the Steam API key from the onboarding form", async () => {
    render(
      <OnboardingWizard
        config={{ ...baseConfig, steamApiKeyConfigured: true, onboardingCurrentStep: 3 }}
        source="auto"
        onExit={() => {}}
      />,
    );

    fireEvent.change(screen.getByLabelText("Steam Web API Key"), {
      target: { value: "steam-test-key" },
    });
    fireEvent.click(screen.getByRole("button", { name: "测试 Steam 连接" }));

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "连接测试：成功" })).toBeInTheDocument();
    });
    expect(screen.getAllByText("浏览器预览模式：已模拟 Steam 连接成功。").length).toBeGreaterThan(0);
  });

  it("tests the saved Steam API key when the field is empty but already configured", async () => {
    render(
      <OnboardingWizard
        config={{ ...baseConfig, steamApiKeyConfigured: true, onboardingCurrentStep: 3 }}
        source="auto"
        onExit={() => {}}
      />,
    );

    const testButton = screen.getByRole("button", { name: "测试 Steam 连接" });
    expect(testButton).toBeEnabled();
    fireEvent.click(testButton);

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "连接测试：成功" })).toBeInTheDocument();
    });
    expect(screen.getAllByText("浏览器预览模式：已模拟 Steam 连接成功。").length).toBeGreaterThan(0);
  });

  it("does not reuse a previously validated Steam key when a new draft key was not retested", async () => {
    const saveConfigSpy = vi.spyOn(clientApi, "saveConfig");
    saveConfigSpy.mockResolvedValue({
      ...baseConfig,
      steamApiKeyConfigured: true,
      steamApiKeyValidated: false,
      onboardingCurrentStep: 4,
    });

    render(
      <OnboardingWizard
        config={{
          ...baseConfig,
          steamApiKeyConfigured: true,
          steamApiKeyValidated: true,
          onboardingCurrentStep: 3,
        }}
        source="auto"
        onExit={() => {}}
      />,
    );

    fireEvent.change(screen.getByLabelText("Steam Web API Key"), {
      target: { value: "replacement-key" },
    });
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));

    await waitFor(() => {
      expect(saveConfigSpy).toHaveBeenCalledWith(
        expect.objectContaining({
          steamApiKey: "replacement-key",
          steamApiKeyValidated: false,
        }),
      );
    });
  });

  it("keeps a successful Steam test visible for at least 10 seconds after config refresh", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-05-08T00:00:00.000Z"));
    const config = { ...baseConfig, steamApiKeyConfigured: true, onboardingCurrentStep: 3 };
    const { rerender } = render(
      <OnboardingWizard
        config={config}
        source="auto"
        onExit={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "测试 Steam 连接" }));
    await flushValidation();
    expect(screen.getByRole("heading", { name: "连接测试：成功" })).toBeInTheDocument();

    rerender(
      <OnboardingWizard
        config={{ ...config }}
        source="auto"
        onExit={() => {}}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(9_999);
    });
    expect(screen.getByRole("heading", { name: "连接测试：成功" })).toBeInTheDocument();

    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(screen.getByRole("heading", { name: "连接测试：等待测试" })).toBeInTheDocument();
  });

  it("shows a Steam validation message when no key is configured or entered", async () => {
    render(
      <OnboardingWizard
        config={{ ...baseConfig, steamApiKeyConfigured: false, onboardingCurrentStep: 2 }}
        source="auto"
        onExit={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1, name: "填写 Steam Key" })).toBeInTheDocument();
    });

    const testButton = screen.getByRole("button", { name: "测试 Steam 连接" });
    expect(testButton).toBeEnabled();
    fireEvent.click(testButton);

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "连接测试：失败" })).toBeInTheDocument();
    });
    expect(screen.getAllByText("请先输入当前要测试的 Steam Web API Key。").length).toBeGreaterThan(0);
  });

  it("tests the AI API key from the onboarding form", async () => {
    render(
      <OnboardingWizard
        config={{
          ...baseConfig,
          steamApiKeyConfigured: true,
          steamApiKeyValidated: true,
          llmApiKeyConfigured: true,
          onboardingCurrentStep: 5,
        }}
        source="auto"
        onExit={() => {}}
      />,
    );

    fireEvent.change(screen.getByLabelText("DeepSeek API Key"), {
      target: { value: "deepseek-test-key" },
    });
    fireEvent.click(screen.getByRole("button", { name: "测试 AI 连接" }));

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "AI 连接测试成功" })).toBeInTheDocument();
    });
    expect(screen.getAllByText("浏览器预览模式：已模拟 AI 连接成功。").length).toBeGreaterThan(0);
  });

  it("tests the saved AI API key when the field is empty but already configured", async () => {
    render(
      <OnboardingWizard
        config={{
          ...baseConfig,
          steamApiKeyConfigured: true,
          steamApiKeyValidated: true,
          llmApiKeyConfigured: true,
          onboardingCurrentStep: 5,
        }}
        source="auto"
        onExit={() => {}}
      />,
    );

    const testButton = screen.getByRole("button", { name: "测试 AI 连接" });
    expect(testButton).toBeEnabled();
    fireEvent.click(testButton);

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "AI 连接测试成功" })).toBeInTheDocument();
    });
    expect(screen.getAllByText("浏览器预览模式：已模拟 AI 连接成功。").length).toBeGreaterThan(0);
  });

  it("keeps a successful AI test visible for at least 10 seconds after config refresh", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-05-08T00:00:00.000Z"));
    const config = {
      ...baseConfig,
      steamApiKeyConfigured: true,
      steamApiKeyValidated: true,
      llmApiKeyConfigured: true,
      llmConfigValidated: false,
      onboardingCurrentStep: 5,
    };
    const { rerender } = render(
      <OnboardingWizard
        config={config}
        source="auto"
        onExit={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "测试 AI 连接" }));
    await flushValidation();
    expect(screen.getByRole("heading", { name: "AI 连接测试成功" })).toBeInTheDocument();

    rerender(
      <OnboardingWizard
        config={{ ...config }}
        source="auto"
        onExit={() => {}}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(9_999);
    });
    expect(screen.getByRole("heading", { name: "AI 连接测试成功" })).toBeInTheDocument();

    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(screen.getByRole("heading", { name: "等待 AI 连接测试" })).toBeInTheDocument();
  });

  it("shows an AI validation message when no key is configured or entered", async () => {
    render(
      <OnboardingWizard
        config={{
          ...baseConfig,
          steamApiKeyConfigured: true,
          steamApiKeyValidated: true,
          llmApiKeyConfigured: false,
          onboardingCurrentStep: 4,
        }}
        source="auto"
        onExit={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1, name: "填写 DeepSeek Key 并完成配置" })).toBeInTheDocument();
    });

    const testButton = screen.getByRole("button", { name: "测试 AI 连接" });
    expect(testButton).toBeEnabled();
    fireEvent.click(testButton);

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "AI 连接测试失败" })).toBeInTheDocument();
    });
    expect(screen.getAllByText("请先输入当前要测试的 AI API Key。").length).toBeGreaterThan(0);
  });
});
