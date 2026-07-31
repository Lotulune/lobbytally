import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AccountProfile } from "../src/api/types";
import { Shell } from "../src/screens/Shell";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const shellTestState = vi.hoisted(() => ({
  authenticated: true,
  userId: "user-a" as string | null,
  authListener: null as (() => void) | null,
  profileRequests: [] as Array<Promise<AccountProfile>>,
}));

vi.mock("../src/app/runtime", () => ({
  apiClient: {
    isAccountAuthenticated: () => shellTestState.authenticated,
    sessionUserId: () => shellTestState.userId,
    getMe: vi.fn(() => {
      const request = shellTestState.profileRequests.shift();
      if (!request) throw new Error("missing queued profile request");
      return request;
    }),
    subscribeAuth: (listener: () => void) => {
      shellTestState.authListener = listener;
      return () => {
        if (shellTestState.authListener === listener) shellTestState.authListener = null;
      };
    },
    meta: vi.fn(async () => ({ data: { demo_mode: false } })),
  },
  feedbackQueue: {
    pendingCount: () => 0,
    subscribe: () => () => {},
  },
  requiresServiceConnect: false,
}));

vi.mock("../src/app/auth", () => ({
  subscribeAccountGate: () => () => {},
}));

vi.mock("../src/app/connection", () => ({
  getConnectionManager: () => ({
    subscribe: () => () => {},
    recheck: async () => {},
  }),
}));

vi.mock("../src/screens/FeedScreen", () => ({
  FeedScreen: () => <div>feed</div>,
}));
vi.mock("../src/screens/GameDetailScreen", () => ({
  GameDetailScreen: () => <div>detail</div>,
}));
vi.mock("../src/screens/SearchScreen", () => ({
  SearchScreen: () => <div>search</div>,
}));
vi.mock("../src/screens/CalendarScreen", () => ({
  CalendarScreen: () => <div>calendar</div>,
}));
vi.mock("../src/screens/SettingsScreen", () => ({
  SettingsScreen: () => <div>settings</div>,
}));
vi.mock("../src/screens/NaturalLanguageScreen", () => ({
  NaturalLanguageScreen: () => <div>natural language</div>,
}));
vi.mock("../src/screens/AiSettingsScreen", () => ({
  AiSettingsScreen: () => <div>ai settings</div>,
}));
vi.mock("../src/screens/AuthDialog", () => ({
  AuthDialog: () => null,
}));
vi.mock("../src/screens/CommunityScreen", () => ({
  CommunityScreen: () => <div>community</div>,
}));
vi.mock("../src/screens/ProfileScreen", () => ({
  ProfileScreen: () => <div>profile screen</div>,
}));
vi.mock("../src/screens/shell/useNavShortcuts", () => ({
  useNavShortcuts: () => {},
}));
vi.mock("../src/screens/shell/Topbar", () => ({
  Topbar: ({ profile }: { profile: AccountProfile | null }) => (
    <div data-testid="shell-profile">{profile?.username ?? "signed-out"}</div>
  ),
}));

function profile(username: string): AccountProfile {
  return {
    username,
    display_name: username,
    avatar_url: "",
    avatar_version: 0,
  };
}

function mountShell() {
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  act(() => root.render(<Shell />));
  return {
    host,
    unmount() {
      act(() => root.unmount());
      host.remove();
    },
  };
}

beforeEach(() => {
  shellTestState.authenticated = true;
  shellTestState.userId = "user-a";
  shellTestState.authListener = null;
  shellTestState.profileRequests = [];
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("Shell account profile loading", () => {
  it("ignores a profile response that resolves after logout", async () => {
    const first = deferred<AccountProfile>();
    shellTestState.profileRequests.push(first.promise);
    const { host, unmount } = mountShell();

    act(() => {
      shellTestState.authenticated = false;
      shellTestState.userId = null;
      shellTestState.authListener?.();
    });
    expect(host.querySelector('[data-testid="shell-profile"]')?.textContent).toBe(
      "signed-out",
    );

    await act(async () => {
      first.resolve(profile("account-a"));
      await Promise.resolve();
    });
    expect(host.querySelector('[data-testid="shell-profile"]')?.textContent).toBe(
      "signed-out",
    );
    unmount();
  });

  it("keeps the new account profile when the old account response resolves last", async () => {
    const first = deferred<AccountProfile>();
    const second = deferred<AccountProfile>();
    shellTestState.profileRequests.push(first.promise, second.promise);
    const { host, unmount } = mountShell();

    act(() => {
      shellTestState.userId = "user-b";
      shellTestState.authListener?.();
    });
    await act(async () => {
      second.resolve(profile("account-b"));
      await Promise.resolve();
    });
    expect(host.querySelector('[data-testid="shell-profile"]')?.textContent).toBe(
      "account-b",
    );

    await act(async () => {
      first.resolve(profile("account-a"));
      await Promise.resolve();
    });
    expect(host.querySelector('[data-testid="shell-profile"]')?.textContent).toBe(
      "account-b",
    );
    unmount();
  });
});
