import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryStorage } from "./helpers";

const runtime = vi.hoisted(() => ({
  markOnboarded: vi.fn(),
  getPreferences: vi.fn(),
  putPreferences: vi.fn(),
}));

vi.mock("../src/app/runtime", () => ({
  apiClient: {
    getPreferences: runtime.getPreferences,
    putPreferences: runtime.putPreferences,
    sessionUserId: () => "u_onboarding",
    isAccountAuthenticated: () => true,
  },
  markOnboarded: runtime.markOnboarded,
}));

vi.mock("../src/app/ThemeProvider", () => ({
  useTheme: () => ({
    themeId: "steam",
    setTheme: vi.fn(),
    fireAction: vi.fn(),
  }),
}));

vi.mock("../src/app/ToastProvider", () => ({
  useToast: () => ({ show: vi.fn() }),
}));

import { OnboardingScreen } from "../src/screens/OnboardingScreen";
import { defaultPreferences, setPreferenceOwnerResolver } from "../src/app/preferences";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function button(host: HTMLElement, text: string): HTMLButtonElement {
  const match = Array.from(host.querySelectorAll("button")).find(
    (candidate) => candidate.textContent?.trim() === text,
  );
  if (!match) throw new Error(`missing button: ${text}`);
  return match;
}

describe("OnboardingScreen", () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    host = document.createElement("div");
    document.body.append(host);
    vi.stubGlobal("localStorage", new MemoryStorage());
    runtime.markOnboarded.mockReset();
    runtime.getPreferences.mockReset();
    runtime.getPreferences.mockResolvedValue(defaultPreferences());
    runtime.putPreferences.mockReset();
    runtime.putPreferences.mockImplementation(async (preferences) => ({
      ...preferences,
      version: preferences.version + 1,
    }));
  });

  afterEach(() => {
    setPreferenceOwnerResolver(null);
    vi.unstubAllGlobals();
    host.remove();
  });

  it("marks explicitly confirmed onboarding preferences as high-confidence", async () => {
    const onDone = vi.fn();
    const root = createRoot(host);
    try {
      act(() => root.render(<OnboardingScreen onDone={onDone} />));
      act(() => button(host, "继续 →").click());

      await act(async () => button(host, "开始探索").click());

      expect(runtime.putPreferences).toHaveBeenCalledWith(
        expect.objectContaining({ preference_confidence: 1 }),
      );
      expect(onDone).toHaveBeenCalledTimes(1);
    } finally {
      act(() => root.unmount());
    }
  });

  it("lets an uncertain player finish onboarding without asserting default preferences", () => {
    const onDone = vi.fn();
    const root = createRoot(host);
    try {
      act(() => root.render(<OnboardingScreen onDone={onDone} />));
      act(() => button(host, "继续 →").click());

      act(() => button(host, "稍后设置/不确定").click());

      expect(runtime.getPreferences).not.toHaveBeenCalled();
      expect(runtime.putPreferences).not.toHaveBeenCalled();
      expect(runtime.markOnboarded).toHaveBeenCalledTimes(1);
      expect(onDone).toHaveBeenCalledTimes(1);
    } finally {
      act(() => root.unmount());
    }
  });
});
