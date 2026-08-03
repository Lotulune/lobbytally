import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const runtime = vi.hoisted(() => ({
  ensureSession: vi.fn(async () => null),
  isAccountAuthenticated: vi.fn(() => false),
  sessionUserId: vi.fn(() => "anon-user"),
  naturalLanguageRecommendations: vi.fn(),
  subscribeRankingChanged: vi.fn(() => () => undefined),
}));

vi.mock("../src/app/runtime", () => ({
  apiClient: {
    ensureSession: runtime.ensureSession,
    isAccountAuthenticated: runtime.isAccountAuthenticated,
    sessionUserId: runtime.sessionUserId,
    naturalLanguageRecommendations: runtime.naturalLanguageRecommendations,
  },
  feedbackQueue: {
    subscribeRankingChanged: runtime.subscribeRankingChanged,
  },
  localCredentialServiceOrigin: "https://service.example",
}));

import { NaturalLanguageScreen } from "../src/screens/NaturalLanguageScreen";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("NaturalLanguageScreen", () => {
  afterEach(() => {
    runtime.naturalLanguageRecommendations.mockReset();
    runtime.ensureSession.mockClear();
    runtime.isAccountAuthenticated.mockClear();
    runtime.sessionUserId.mockClear();
    runtime.subscribeRankingChanged.mockClear();
  });

  it("renders understood constraints without exposing backend field names", async () => {
    runtime.naturalLanguageRecommendations.mockResolvedValue({
      query: "4 人合作，单局一小时以内",
      interpreted: {
        party_size: 4,
        session_minutes_max: 60,
        coop_competitive: 0.2,
        self_hosting_willingness: null,
        platforms: [],
        hard_constraints: ["party_size", "session_minutes"],
        applied_constraints: ["party_size", "session_minutes"],
        unapplied_constraints: ["modes_preferred:public_world"],
        max_price_minor: null,
        currency: null,
      },
      items: [],
      ai_status: "fallback",
      fallback_reason: null,
      algorithm_version: "test",
      data_updated_at_ms: Date.now(),
    });
    const host = document.createElement("div");
    const root = createRoot(host);
    act(() => root.render(<NaturalLanguageScreen onOpenGame={() => undefined} />));

    const input = host.querySelector<HTMLInputElement>("#nl-input")!;
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    act(() => {
      setValue.call(input, "4 人合作，单局一小时以内");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => {
      host.querySelector<HTMLFormElement>("form")!.dispatchEvent(
        new Event("submit", { bubbles: true, cancelable: true }),
      );
      await Promise.resolve();
    });

    expect(host.textContent).toContain("4 人（硬性）");
    expect(host.textContent).toContain("最长 60 分钟（硬性）");
    expect(host.textContent).toContain("已应用的条件");
    expect(host.textContent).toContain("尚未应用");
    expect(host.textContent).toContain("玩法偏好：public_world");
    expect(host.textContent).not.toContain("party_size");
    expect(host.textContent).not.toContain("session_minutes");
    expect(runtime.naturalLanguageRecommendations).toHaveBeenCalledTimes(1);

    act(() => root.unmount());
  });
});
