// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import { AboutPage } from "../about/AboutPage";
import { SettingsPage } from "./SettingsPage";

function openSettingsSection(title: string) {
  fireEvent.click(screen.getByRole("button", { name: new RegExp(title) }));
}

afterEach(() => {
  cleanup();
});

describe("Settings and About pages", () => {
  it("shows DeepSeek defaults while advertising OpenAI and Anthropic compatibility", () => {
    render(
      <SettingsPage
        config={mockDashboard.config}
        isBusy={false}
        onRefreshAllAnalyses={vi.fn(async () => undefined)}
        stats={mockDashboard.stats}
        onRefreshDashboard={vi.fn(async () => undefined)}
        onSave={vi.fn(async () => undefined)}
        onStatus={vi.fn()}
        onSync={vi.fn()}
        status="当前库已就绪。"
      />,
    );

    openSettingsSection("API 密钥");

    expect(
      screen.getByPlaceholderText("输入 DeepSeek / OpenAI / Anthropic API Key"),
    ).toBeInTheDocument();
    expect(screen.getByDisplayValue("https://api.deepseek.com")).toBeInTheDocument();
    expect(screen.getByDisplayValue("deepseek-v4-flash")).toBeInTheDocument();
  });

  it("shows both sync and discovery operations in settings", () => {
    const onSync = vi.fn();
    const onRefreshAllAnalyses = vi.fn(async () => undefined);

    render(
      <SettingsPage
        config={mockDashboard.config}
        isBusy={false}
        onRefreshAllAnalyses={onRefreshAllAnalyses}
        stats={mockDashboard.stats}
        onRefreshDashboard={vi.fn(async () => undefined)}
        onSave={vi.fn(async () => undefined)}
        onStatus={vi.fn()}
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

    expect(onSync).toHaveBeenNthCalledWith(1, "full");
    expect(onSync).toHaveBeenNthCalledWith(2, "quick");
    expect(onRefreshAllAnalyses).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "完整同步" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "快速同步" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "批量重算 AI 评分" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "发现任务控制台" })).toBeInTheDocument();
    expect(screen.getByText("Steam 同步")).toBeInTheDocument();
    expect(screen.getByText("当前库已就绪。")).toBeInTheDocument();
  });

  it("passes the selected batch refresh concurrency to the refresh action", () => {
    const onRefreshAllAnalyses = vi.fn(async (_concurrency: number) => undefined);

    render(
      <SettingsPage
        config={mockDashboard.config}
        isBusy={false}
        onRefreshAllAnalyses={onRefreshAllAnalyses}
        stats={mockDashboard.stats}
        onRefreshDashboard={vi.fn(async () => undefined)}
        onSave={vi.fn(async () => undefined)}
        onStatus={vi.fn()}
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
        onRefreshAllAnalyses={vi.fn(async () => undefined)}
        stats={stats}
        onRefreshDashboard={vi.fn(async () => undefined)}
        onSave={vi.fn(async () => undefined)}
        onStatus={vi.fn()}
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
});
