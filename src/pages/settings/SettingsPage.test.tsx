// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import { AboutPage } from "../about/AboutPage";
import { SettingsPage } from "./SettingsPage";

function openSettingsSection(title: string) {
  fireEvent.click(screen.getByRole("button", { name: new RegExp(title) }));
}

function renderSettingsPage(
  props: Partial<React.ComponentProps<typeof SettingsPage>> = {},
) {
  const defaultProps: React.ComponentProps<typeof SettingsPage> = {
    config: mockDashboard.config,
    isBusy: false,
    onOpenOnboarding: vi.fn(),
    onRefreshAllAnalyses: vi.fn(async () => undefined),
    onRetryAiAnalysisJob: vi.fn(async () => undefined),
    onStartClassicDiscovery: vi.fn(async () => undefined),
    stats: mockDashboard.stats,
    aiAnalysisQueueFailures: mockDashboard.aiAnalysisQueueFailures,
    onRefreshDashboard: vi.fn(async () => undefined),
    onSave: vi.fn(async () => undefined),
    onStatus: vi.fn(),
    onImportServiceConnectionFile: vi.fn(async () => undefined),
    onSync: vi.fn(),
    status: "当前库已就绪。",
  };

  return render(<SettingsPage {...defaultProps} {...props} />);
}

afterEach(() => {
  cleanup();
});

describe("Settings and About pages", () => {
  it("starts with the onboarding wizard section collapsed", () => {
    renderSettingsPage();

    expect(screen.getByRole("button", { name: /初始化向导/ })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    expect(screen.queryByRole("button", { name: "继续初始化" })).not.toBeInTheDocument();
  });

  it("keeps manually expanded sections open across config refreshes", () => {
    const { rerender } = renderSettingsPage();

    openSettingsSection("LLM 配置");
    expect(screen.getByRole("button", { name: /LLM 配置/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );

    rerender(
      <SettingsPage
        config={{ ...mockDashboard.config }}
        isBusy={false}
        onOpenOnboarding={vi.fn()}
        onRefreshAllAnalyses={vi.fn(async () => undefined)}
        onRetryAiAnalysisJob={vi.fn(async () => undefined)}
        onStartClassicDiscovery={vi.fn(async () => undefined)}
        stats={mockDashboard.stats}
        aiAnalysisQueueFailures={mockDashboard.aiAnalysisQueueFailures}
        onRefreshDashboard={vi.fn(async () => undefined)}
        onSave={vi.fn(async () => undefined)}
        onStatus={vi.fn()}
        onImportServiceConnectionFile={vi.fn(async () => undefined)}
        onSync={vi.fn()}
        status="配置已刷新。"
      />,
    );

    expect(screen.getByRole("button", { name: /LLM 配置/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(screen.getByRole("combobox", { name: "AI 提供方" })).toBeInTheDocument();
  });

  it("restores expanded sections supplied by the app session after remounting", () => {
    const settingsSessionExpanded = {
      serviceConnection: false,
      onboarding: false,
      apiKeys: false,
      llmConfig: false,
      sync: false,
      aiBatch: false,
      discovery: false,
    };
    const handleExpandedChange = vi.fn((next: typeof settingsSessionExpanded) => {
      Object.assign(settingsSessionExpanded, next);
    });
    const firstRender = renderSettingsPage({
      expandedSections: settingsSessionExpanded,
      onExpandedSectionsChange: handleExpandedChange,
    });

    openSettingsSection("数据同步");
    expect(handleExpandedChange).toHaveBeenLastCalledWith({
      ...settingsSessionExpanded,
      sync: true,
    });

    firstRender.unmount();
    renderSettingsPage({
      expandedSections: settingsSessionExpanded,
      onExpandedSectionsChange: handleExpandedChange,
    });

    expect(screen.getByRole("button", { name: /数据同步/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(screen.getByRole("button", { name: "完整同步" })).toBeInTheDocument();
  });

  it("shows DeepSeek defaults while supporting provider switching and onboarding entry", () => {
    render(
      <SettingsPage
        config={mockDashboard.config}
        isBusy={false}
        onOpenOnboarding={vi.fn()}
        onRefreshAllAnalyses={vi.fn(async () => undefined)}
        onRetryAiAnalysisJob={vi.fn(async () => undefined)}
        onStartClassicDiscovery={vi.fn(async () => undefined)}
        stats={mockDashboard.stats}
        aiAnalysisQueueFailures={mockDashboard.aiAnalysisQueueFailures}
        onRefreshDashboard={vi.fn(async () => undefined)}
        onSave={vi.fn(async () => undefined)}
        onStatus={vi.fn()}
        onImportServiceConnectionFile={vi.fn(async () => undefined)}
        onSync={vi.fn()}
        status="当前库已就绪。"
      />,
    );

    openSettingsSection("LLM 配置");
    openSettingsSection("初始化向导");

    expect(screen.getByRole("button", { name: "继续初始化" })).toBeInTheDocument();
    expect(screen.getByText("默认提供方")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "AI 提供方" })).toHaveValue("deepseek");
    expect(screen.getByRole("combobox", { name: "AI 提供方" })).toHaveDisplayValue("DeepSeek");
    expect(screen.getByDisplayValue("https://api.deepseek.com")).toBeInTheDocument();
    expect(screen.getByDisplayValue("deepseek-v4-flash")).toBeInTheDocument();
  });

  it("allows testing saved credentials without entering replacement keys", async () => {
    const onStatus = vi.fn();
    const config = {
      ...mockDashboard.config,
      steamApiKeyConfigured: true,
      steamApiKeyValidated: false,
      llmApiKeyConfigured: true,
      llmConfigValidated: false,
    };

    render(
      <SettingsPage
        config={config}
        isBusy={false}
        onOpenOnboarding={vi.fn()}
        onRefreshAllAnalyses={vi.fn(async () => undefined)}
        onRetryAiAnalysisJob={vi.fn(async () => undefined)}
        onStartClassicDiscovery={vi.fn(async () => undefined)}
        stats={mockDashboard.stats}
        aiAnalysisQueueFailures={mockDashboard.aiAnalysisQueueFailures}
        onRefreshDashboard={vi.fn(async () => undefined)}
        onSave={vi.fn(async () => undefined)}
        onStatus={onStatus}
        onImportServiceConnectionFile={vi.fn(async () => undefined)}
        onSync={vi.fn()}
        status="当前库已就绪。"
      />,
    );

    openSettingsSection("初始化向导");

    const steamTestButton = screen.getByRole("button", { name: "测试 Steam 连接" });
    const aiTestButton = screen.getByRole("button", { name: "测试 AI 连接" });
    expect(steamTestButton).toBeEnabled();
    expect(aiTestButton).toBeEnabled();

    fireEvent.click(steamTestButton);
    await waitFor(() => {
      expect(onStatus).toHaveBeenCalledWith("浏览器预览模式：已模拟 Steam 连接成功。");
    });

    fireEvent.click(aiTestButton);
    await waitFor(() => {
      expect(onStatus).toHaveBeenCalledWith("浏览器预览模式：已模拟 AI 连接成功。");
    });
  });

  it("keeps a newly entered AI key when saving after provider switching", async () => {
    const onSave = vi.fn(async () => undefined);

    render(
      <SettingsPage
        config={{
          ...mockDashboard.config,
          llmApiKeyConfigured: true,
        }}
        isBusy={false}
        onOpenOnboarding={vi.fn()}
        onRefreshAllAnalyses={vi.fn(async () => undefined)}
        onRetryAiAnalysisJob={vi.fn(async () => undefined)}
        onStartClassicDiscovery={vi.fn(async () => undefined)}
        stats={mockDashboard.stats}
        aiAnalysisQueueFailures={mockDashboard.aiAnalysisQueueFailures}
        onRefreshDashboard={vi.fn(async () => undefined)}
        onSave={onSave}
        onStatus={vi.fn()}
        onImportServiceConnectionFile={vi.fn(async () => undefined)}
        onSync={vi.fn()}
        status="当前库已就绪。"
      />,
    );

    openSettingsSection("API 密钥");
    openSettingsSection("LLM 配置");

    fireEvent.change(screen.getByRole("combobox", { name: "AI 提供方" }), {
      target: { value: "openai" },
    });
    fireEvent.change(screen.getByLabelText("OpenAI API Key"), {
      target: { value: "openai-test-key" },
    });
    fireEvent.click(screen.getByRole("button", { name: "保存设置" }));

    await waitFor(() => {
      expect(onSave).toHaveBeenCalledWith(
        expect.objectContaining({
          llmProvider: "openai",
          llmApiKey: "openai-test-key",
          clearLlmApiKey: undefined,
        }),
      );
    });
  });

  it("shows both sync and discovery operations in settings", () => {
    const onSync = vi.fn();
    const onRefreshAllAnalyses = vi.fn(async () => undefined);
    const onStartClassicDiscovery = vi.fn(async () => undefined);

    render(
      <SettingsPage
        config={mockDashboard.config}
        isBusy={false}
        onOpenOnboarding={vi.fn()}
        onRefreshAllAnalyses={onRefreshAllAnalyses}
        onRetryAiAnalysisJob={vi.fn(async () => undefined)}
        onStartClassicDiscovery={onStartClassicDiscovery}
        stats={mockDashboard.stats}
        aiAnalysisQueueFailures={mockDashboard.aiAnalysisQueueFailures}
        onRefreshDashboard={vi.fn(async () => undefined)}
        onSave={vi.fn(async () => undefined)}
        onStatus={vi.fn()}
        onImportServiceConnectionFile={vi.fn(async () => undefined)}
        onSync={onSync}
        status="当前库已就绪。"
      />,
    );

    openSettingsSection("数据同步");
    openSettingsSection("AI 批量重算");
    openSettingsSection("发现任务");

    fireEvent.click(screen.getByRole("button", { name: "完整同步" }));
    fireEvent.click(screen.getByRole("button", { name: "快速同步" }));
    fireEvent.click(screen.getByRole("button", { name: "批量重算 AI 评分" }));
    fireEvent.click(screen.getByRole("button", { name: "启动老游补库" }));

    expect(onSync).toHaveBeenNthCalledWith(1, "full");
    expect(onSync).toHaveBeenNthCalledWith(2, "quick");
    expect(onRefreshAllAnalyses).toHaveBeenCalledTimes(1);
    expect(onStartClassicDiscovery).toHaveBeenCalledWith(3);
    expect(screen.getByRole("button", { name: "完整同步" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "快速同步" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "批量重算 AI 评分" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "发现任务控制台" })).toBeInTheDocument();
    expect(screen.getByText("Steam 同步")).toBeInTheDocument();
    expect(
      screen.getByText(
        "老游补库会在新游发现结束且新游补全清空后启动；不必等待新游 AI 清空，但老游 AI 仍会排在新游 AI 后面。",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("当前库已就绪。")).toBeInTheDocument();
  });

  it("hides local maintenance sections in public service mode", () => {
    renderSettingsPage({
      stats: {
        ...mockDashboard.stats,
        sourceKind: "public_service",
        dataSource: "公共发现服务：MPGS Test Service",
        totalGames: 42,
      },
      status: "公共服务已连接。",
    });

    expect(screen.getByText("公共发现服务")).toBeInTheDocument();
    expect(screen.getByText("公共发现服务：MPGS Test Service")).toBeInTheDocument();
    expect(screen.getByText("42")).toBeInTheDocument();
    expect(screen.getByText("本地保存")).toBeInTheDocument();
    expect(screen.getByText("公共服务已连接。")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /初始化向导/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /API 密钥/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /LLM 配置/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /数据同步/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /AI 批量重算/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /发现任务/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "发现任务控制台" })).not.toBeInTheDocument();
  });

  it("imports a service connection file from settings", async () => {
    const onImportServiceConnectionFile = vi.fn(async () => undefined);
    renderSettingsPage({
      onImportServiceConnectionFile,
      status: "等待导入服务连接。",
    });

    openSettingsSection("公共发现服务连接");
    const connectionFile = new File(
      [
        JSON.stringify({
          serviceName: "MPGS Public",
          serviceInstanceId: "018fb770-8998-7699-a6e4-b7b59f2f9c01",
          apiVersion: "v1",
          baseUrl: "https://mpgs.example.test",
          serviceInfoUrl: "https://mpgs.example.test/api/v1/service-info",
          capabilities: ["public_catalog_read"],
        }),
      ],
      "mpgs-service-connection.json",
      { type: "application/json" },
    );

    fireEvent.change(screen.getByLabelText("导入服务连接文件"), {
      target: { files: [connectionFile] },
    });

    await waitFor(() => {
      expect(onImportServiceConnectionFile).toHaveBeenCalledWith(
        expect.stringContaining('"baseUrl":"https://mpgs.example.test"'),
      );
    });
  });

  it("passes the selected batch refresh concurrency to the refresh action", () => {
    const onRefreshAllAnalyses = vi.fn(async (_concurrency: number) => undefined);

    render(
      <SettingsPage
        config={mockDashboard.config}
        isBusy={false}
        onOpenOnboarding={vi.fn()}
        onRefreshAllAnalyses={onRefreshAllAnalyses}
        onRetryAiAnalysisJob={vi.fn(async () => undefined)}
        onStartClassicDiscovery={vi.fn(async () => undefined)}
        stats={mockDashboard.stats}
        aiAnalysisQueueFailures={mockDashboard.aiAnalysisQueueFailures}
        onRefreshDashboard={vi.fn(async () => undefined)}
        onSave={vi.fn(async () => undefined)}
        onStatus={vi.fn()}
        onImportServiceConnectionFile={vi.fn(async () => undefined)}
        onSync={vi.fn()}
        status="当前库已就绪。"
      />,
    );

    openSettingsSection("AI 批量重算");

    fireEvent.change(screen.getByLabelText("AI 批量重算并发数"), {
      target: { value: "10" },
    });
    fireEvent.click(screen.getByRole("button", { name: "批量重算 AI 评分" }));

    expect(onRefreshAllAnalyses).toHaveBeenCalledWith(10);
  });

  it("shows batch refresh progress when AI score recompute is running", () => {
    const stats = {
      ...mockDashboard.stats,
      aiBatchRefreshRunning: true,
      aiBatchRefreshConcurrency: 5,
      aiBatchRefreshPendingCount: 12,
      aiBatchRefreshActiveCount: 5,
      aiBatchRefreshTotalCount: 20,
      aiBatchRefreshProcessedCount: 8,
      aiBatchRefreshUpdatedCount: 7,
      aiBatchRefreshFailedCount: 1,
      aiBatchRefreshLastError: "7301: upstream timeout",
      aiBatchRefreshLastErrorAppid: 7301,
    } as typeof mockDashboard.stats;

    render(
      <SettingsPage
        config={mockDashboard.config}
        isBusy={false}
        onOpenOnboarding={vi.fn()}
        onRefreshAllAnalyses={vi.fn(async () => undefined)}
        onRetryAiAnalysisJob={vi.fn(async () => undefined)}
        onStartClassicDiscovery={vi.fn(async () => undefined)}
        stats={stats}
        aiAnalysisQueueFailures={mockDashboard.aiAnalysisQueueFailures}
        onRefreshDashboard={vi.fn(async () => undefined)}
        onSave={vi.fn(async () => undefined)}
        onStatus={vi.fn()}
        onImportServiceConnectionFile={vi.fn(async () => undefined)}
        onSync={vi.fn()}
        status="AI 批量重算进行中。"
      />,
    );

    openSettingsSection("AI 批量重算");

    expect(screen.getAllByText("AI 批量重算").length).toBeGreaterThan(0);
    expect(screen.getByText("进度 40%")).toBeInTheDocument();
    expect(screen.getByText("8/20")).toBeInTheDocument();
    expect(screen.getByText("7301: upstream timeout")).toBeInTheDocument();
  });

  it("renders AI failure queue entries and retry actions", () => {
    const onRetryAiAnalysisJob = vi.fn(async () => undefined);

    render(
      <SettingsPage
        config={mockDashboard.config}
        isBusy={false}
        onOpenOnboarding={vi.fn()}
        onRefreshAllAnalyses={vi.fn(async () => undefined)}
        onRetryAiAnalysisJob={onRetryAiAnalysisJob}
        onStartClassicDiscovery={vi.fn(async () => undefined)}
        stats={mockDashboard.stats}
        aiAnalysisQueueFailures={mockDashboard.aiAnalysisQueueFailures}
        onRefreshDashboard={vi.fn(async () => undefined)}
        onSave={vi.fn(async () => undefined)}
        onStatus={vi.fn()}
        onImportServiceConnectionFile={vi.fn(async () => undefined)}
        onSync={vi.fn()}
        status="当前库已就绪。"
      />,
    );

    openSettingsSection("AI 批量重算");
    expect(screen.getByText(/待人工处理失败项：1/)).toBeInTheDocument();
    expect(screen.getByText(/AppID 548430/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    expect(onRetryAiAnalysisJob).toHaveBeenCalledWith(548430);
  });

  it("passes the entered classic discovery page budget to the manual start action", () => {
    const onStartClassicDiscovery = vi.fn(async (_maxPages: number) => undefined);

    render(
      <SettingsPage
        config={mockDashboard.config}
        isBusy={false}
        onOpenOnboarding={vi.fn()}
        onRefreshAllAnalyses={vi.fn(async () => undefined)}
        onRetryAiAnalysisJob={vi.fn(async () => undefined)}
        onStartClassicDiscovery={onStartClassicDiscovery}
        stats={mockDashboard.stats}
        aiAnalysisQueueFailures={mockDashboard.aiAnalysisQueueFailures}
        onRefreshDashboard={vi.fn(async () => undefined)}
        onSave={vi.fn(async () => undefined)}
        onStatus={vi.fn()}
        onImportServiceConnectionFile={vi.fn(async () => undefined)}
        onSync={vi.fn()}
        status="当前库已就绪。"
      />,
    );

    openSettingsSection("发现任务");

    fireEvent.change(screen.getByLabelText("老游补库页数"), {
      target: { value: "2" },
    });
    fireEvent.click(screen.getByRole("button", { name: "启动老游补库" }));

    expect(onStartClassicDiscovery).toHaveBeenCalledWith(2);
  });

  it("renders a real about/diagnostic surface", () => {
    render(
      <AboutPage
        config={mockDashboard.config}
        stats={mockDashboard.stats}
      />,
    );

    expect(screen.getByRole("heading", { name: "关于 Co-Play" })).toBeInTheDocument();
    expect(screen.getByText(/Steam Key：未配置/)).toBeInTheDocument();
    expect(
      screen.getByText(new RegExp(`库内 ${mockDashboard.stats.totalGames} 款游戏`)),
    ).toBeInTheDocument();
  });

  it("shows public service identity instead of local credentials on the about page in service mode", () => {
    render(
      <AboutPage
        config={{
          ...mockDashboard.config,
          steamApiKeyConfigured: true,
          llmApiKeyConfigured: true,
        }}
        stats={{
          ...mockDashboard.stats,
          sourceKind: "public_service",
          dataSource: "公共发现服务：MPGS Test Service",
          totalGames: 42,
          newGamesCount: 8,
        }}
      />,
    );

    expect(screen.getByRole("heading", { name: "关于 Co-Play" })).toBeInTheDocument();
    expect(screen.getByText("公共发现服务：MPGS Test Service")).toBeInTheDocument();
    expect(screen.getByText(/匿名只读/)).toBeInTheDocument();
    expect(screen.getByText("本地个人状态")).toBeInTheDocument();
    expect(screen.queryByText(/Steam Key/)).not.toBeInTheDocument();
    expect(screen.queryByText(/LLM Key/)).not.toBeInTheDocument();
    expect(screen.queryByText("同步与诊断")).not.toBeInTheDocument();
    expect(screen.queryByText(/最近同步/)).not.toBeInTheDocument();
    expect(screen.queryByText(/最近处理 AppID/)).not.toBeInTheDocument();
  });
});
