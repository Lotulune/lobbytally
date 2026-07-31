import { beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn(),
  onCloseRequested: vi.fn(async () => () => undefined),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauri.invoke,
  isTauri: () => true,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    destroy: vi.fn(async () => undefined),
    onCloseRequested: tauri.onCloseRequested,
  }),
}));

interface NativeState {
  legacySession: string | null;
  secureSession: string | null;
  saveMode: "persist" | "ignore" | "fail";
  calls: string[];
}

function installNativeState(state: NativeState): void {
  tauri.invoke.mockImplementation(async (command: string, args?: unknown) => {
    state.calls.push(command);
    switch (command) {
      case "client_store_load":
        return {};
      case "client_store_read_legacy_session":
        return state.legacySession;
      case "auth_session_load":
        return state.secureSession;
      case "auth_session_save": {
        if (state.saveMode === "fail") throw new Error("keyring unavailable");
        if (state.saveMode === "persist") {
          state.secureSession = (args as { value: string }).value;
        }
        return null;
      }
      case "auth_session_remove":
        state.secureSession = null;
        return null;
      case "client_store_acknowledge_legacy_session": {
        const expected = (args as { expectedValue: string }).expectedValue;
        if (state.legacySession !== expected) return false;
        state.legacySession = null;
        return true;
      }
      case "client_store_set":
      case "client_store_remove":
        return null;
      default:
        throw new Error(`unexpected native command: ${command}`);
    }
  });
}

describe("desktop legacy session migration", () => {
  beforeEach(() => {
    vi.resetModules();
    tauri.invoke.mockReset();
    tauri.onCloseRequested.mockClear();
    localStorage.clear();
  });

  it("writes and verifies the keyring before acknowledging the SQLite copy", async () => {
    const state: NativeState = {
      legacySession: "legacy-session-json",
      secureSession: null,
      saveMode: "persist",
      calls: [],
    };
    installNativeState(state);

    const { initializeClientStorage } = await import("../src/api/storage");
    await initializeClientStorage();

    expect(JSON.parse(state.secureSession!)).toEqual({
      v: 2,
      sessions: { "mpgs.session.v1": "legacy-session-json" },
    });
    expect(state.legacySession).toBeNull();

    const saveIndex = state.calls.indexOf("auth_session_save");
    const acknowledgeIndex = state.calls.indexOf(
      "client_store_acknowledge_legacy_session",
    );
    const verificationIndex = state.calls.lastIndexOf("auth_session_load");
    expect(saveIndex).toBeGreaterThanOrEqual(0);
    expect(verificationIndex).toBeGreaterThan(saveIndex);
    expect(acknowledgeIndex).toBeGreaterThan(verificationIndex);
  });

  it("retains the SQLite copy when the keyring write fails", async () => {
    const state: NativeState = {
      legacySession: "only-copy",
      secureSession: null,
      saveMode: "fail",
      calls: [],
    };
    installNativeState(state);

    const { initializeClientStorage } = await import("../src/api/storage");
    await expect(initializeClientStorage()).rejects.toThrow("keyring unavailable");

    expect(state.legacySession).toBe("only-copy");
    expect(state.calls).not.toContain("client_store_acknowledge_legacy_session");
  });

  it("retains the SQLite copy when keyring verification fails", async () => {
    const state: NativeState = {
      legacySession: "only-copy",
      secureSession: null,
      saveMode: "ignore",
      calls: [],
    };
    installNativeState(state);

    const { initializeClientStorage } = await import("../src/api/storage");
    await expect(initializeClientStorage()).rejects.toThrow(
      "secure session verification failed",
    );

    expect(state.legacySession).toBe("only-copy");
    expect(state.calls).not.toContain("client_store_acknowledge_legacy_session");
  });
});
