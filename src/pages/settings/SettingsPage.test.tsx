// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mockDashboard } from "../../data/mockDashboard";
import { AboutPage } from "../about/AboutPage";
import { SettingsPage } from "./SettingsPage";

afterEach(() => {
  cleanup();
});

describe("Settings and About pages", () => {
  it("shows both sync and discovery operations in settings", () => {
    const onSync = vi.fn();

    render(
      <SettingsPage
        config={mockDashboard.config}
        isBusy={false}
        stats={mockDashboard.stats}
        onRefreshDashboard={vi.fn(async () => undefined)}
        onSave={vi.fn(async () => undefined)}
        onStatus={vi.fn()}
        onSync={onSync}
        status="当前库已就绪。"
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "完整同步" }));
    fireEvent.click(screen.getByRole("button", { name: "快速同步" }));

    expect(onSync).toHaveBeenNthCalledWith(1, "full");
    expect(onSync).toHaveBeenNthCalledWith(2, "quick");
    expect(screen.getByRole("button", { name: "完整同步" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "快速同步" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "发现任务控制台" })).toBeInTheDocument();
    expect(screen.getByText("Steam 同步")).toBeInTheDocument();
    expect(screen.getByText("当前库已就绪。")).toBeInTheDocument();
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
