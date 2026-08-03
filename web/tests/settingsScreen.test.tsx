import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryStorage } from "./helpers";

const runtime = vi.hoisted(() => ({
  getPreferences: vi.fn(),
  putPreferences: vi.fn(),
  isAccountAuthenticated: vi.fn(() => true),
  subscribeAuth: vi.fn(() => () => undefined),
  clearCachedResponses: vi.fn(() => 0),
}));

vi.mock("../src/app/runtime", () => ({
  apiClient: {
    getPreferences: runtime.getPreferences,
    putPreferences: runtime.putPreferences,
    isAccountAuthenticated: runtime.isAccountAuthenticated,
    sessionUserId: () => "u_settings",
    subscribeAuth: runtime.subscribeAuth,
    clearCachedResponses: runtime.clearCachedResponses,
  },
  feedbackQueue: {
    pendingCount: () => 0,
    subscribe: () => () => undefined,
    flush: vi.fn(),
  },
  requiresServiceConnect: false,
}));

vi.mock("../src/app/ThemeProvider", () => ({
  useTheme: () => ({
    themeId: "steam",
    setTheme: vi.fn(),
    intensity: "full",
    setIntensity: vi.fn(),
    fireAction: vi.fn(),
  }),
}));

vi.mock("../src/app/ToastProvider", () => ({
  useToast: () => ({ show: vi.fn() }),
}));

vi.mock("../src/app/auth", () => ({ requestAccountSignIn: vi.fn() }));
vi.mock("../src/screens/AiSettingsScreen", () => ({ AiSettingsScreen: () => null }));
vi.mock("../src/screens/ServicePanel", () => ({ ServicePanel: () => null }));

import { defaultPreferences, setPreferenceOwnerResolver } from "../src/app/preferences";
import { SettingsScreen } from "../src/screens/SettingsScreen";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function button(host: HTMLElement, text: string): HTMLButtonElement {
  const match = Array.from(host.querySelectorAll("button")).find(
    (candidate) => candidate.textContent?.trim() === text,
  );
  if (!match) throw new Error(`missing button: ${text}`);
  return match;
}

describe("SettingsScreen", () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    host = document.createElement("div");
    document.body.append(host);
    vi.stubGlobal("localStorage", new MemoryStorage());
    const preferences = defaultPreferences();
    runtime.getPreferences.mockReset();
    runtime.getPreferences.mockResolvedValue(preferences);
    runtime.putPreferences.mockReset();
    runtime.putPreferences.mockImplementation(async (nextPreferences) => ({
      ...nextPreferences,
      version: nextPreferences.version + 1,
    }));
  });

  afterEach(() => {
    setPreferenceOwnerResolver(null);
    vi.unstubAllGlobals();
    host.remove();
  });

  it("marks a player-confirmed settings edit as high-confidence in the saved payload", async () => {
    const root = createRoot(host);
    try {
      await act(async () => root.render(<SettingsScreen />));
      await vi.waitFor(() => expect(host.textContent).toContain("常用人数"));

      act(() => button(host, "6 人").click());
      await act(async () => button(host, "保存偏好").click());

      expect(runtime.putPreferences).toHaveBeenCalledWith(
        expect.objectContaining({ party_size: 6, preference_confidence: 1 }),
      );
    } finally {
      act(() => root.unmount());
    }
  });
});
