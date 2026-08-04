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
  FeedScreen: ({
    section,
    onOpenGame,
  }: {
    section: string;
    onOpenGame: (appId: number, recommendationRunId?: string | null) => void;
  }) => (
    <div data-testid="feed-state" data-section={section} data-run-id="feed-run-123">
      <input data-testid="feed-sort-state" defaultValue="recommended:desc:page-3" />
      <article data-app-id="42" tabIndex={0}>
        <button type="button" onClick={() => onOpenGame(42, "feed-run-123")}>
          open feed game
        </button>
      </article>
    </div>
  ),
}));
vi.mock("../src/screens/GameDetailScreen", () => ({
  GameDetailScreen: ({
    onBack,
    recommendationRunId,
  }: {
    onBack: () => void;
    recommendationRunId?: string | null;
  }) => (
    <div data-testid="detail-state" data-run-id={recommendationRunId ?? ""}>
      detail
      <button type="button" onClick={onBack}>back to list</button>
    </div>
  ),
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
vi.mock("../src/screens/DataOpsScreen", () => ({
  DataOpsScreen: () => <div>data-ops</div>,
}));
vi.mock("../src/screens/NaturalLanguageScreen", () => ({
  NaturalLanguageScreen: ({
    onOpenGame,
  }: {
    onOpenGame: (appId: number, recommendationRunId?: string | null) => void;
  }) => (
    <div data-testid="nl-state" data-run-id="nl-run-456">
      <input data-testid="nl-query-state" defaultValue="" />
      <div data-testid="nl-loaded-results">loaded recommendations</div>
      <article data-app-id="84" tabIndex={0}>
        <button type="button" onClick={() => onOpenGame(84, "nl-run-456")}>
          open natural-language game
        </button>
      </article>
    </div>
  ),
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
  Topbar: ({
    profile,
    onNavigate,
  }: {
    profile: AccountProfile | null;
    onNavigate: (view: { kind: "natural-language" }) => void;
  }) => (
    <div>
      <div data-testid="shell-profile">{profile?.username ?? "signed-out"}</div>
      <button type="button" onClick={() => onNavigate({ kind: "natural-language" })}>
        show natural language
      </button>
    </div>
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

describe("Shell game-detail return state", () => {
  it("keeps feed controls/run state mounted and restores scroll and card focus", () => {
    shellTestState.authenticated = false;
    shellTestState.userId = null;
    const { host, unmount } = mountShell();
    const main = host.querySelector<HTMLElement>("main.main")!;
    const stateNode = host.querySelector<HTMLElement>('[data-testid="feed-state"]')!;
    const sortState = host.querySelector<HTMLInputElement>('[data-testid="feed-sort-state"]')!;
    sortState.value = "ccu:asc:page-3";
    main.scrollTop = 420;

    act(() => {
      Array.from(host.querySelectorAll("button"))
        .find((button) => button.textContent === "open feed game")
        ?.click();
    });
    expect(host.textContent).toContain("detail");
    expect(host.querySelector<HTMLElement>('[data-testid="detail-state"]')?.dataset.runId).toBe(
      "feed-run-123",
    );
    main.scrollTop = 0;

    act(() => {
      Array.from(host.querySelectorAll("button"))
        .find((button) => button.textContent === "back to list")
        ?.click();
    });
    expect(host.querySelector('[data-testid="feed-state"]')).toBe(stateNode);
    expect(sortState.value).toBe("ccu:asc:page-3");
    expect(stateNode.dataset.section).toBe("recent_release");
    expect(stateNode.dataset.runId).toBe("feed-run-123");
    expect(main.scrollTop).toBe(420);
    expect(document.activeElement).toBe(stateNode.querySelector('[data-app-id="42"]'));
    unmount();
  });

  it("keeps a natural-language query, loaded results and run id across detail", () => {
    shellTestState.authenticated = false;
    shellTestState.userId = null;
    const { host, unmount } = mountShell();

    act(() => {
      Array.from(host.querySelectorAll("button"))
        .find((button) => button.textContent === "show natural language")
        ?.click();
    });
    const stateNode = host.querySelector<HTMLElement>('[data-testid="nl-state"]')!;
    const query = host.querySelector<HTMLInputElement>('[data-testid="nl-query-state"]')!;
    query.value = "四个人短局合作";

    act(() => {
      Array.from(host.querySelectorAll("button"))
        .find((button) => button.textContent === "open natural-language game")
        ?.click();
    });
    expect(host.querySelector<HTMLElement>('[data-testid="detail-state"]')?.dataset.runId).toBe(
      "nl-run-456",
    );
    act(() => {
      Array.from(host.querySelectorAll("button"))
        .find((button) => button.textContent === "back to list")
        ?.click();
    });

    expect(host.querySelector('[data-testid="nl-state"]')).toBe(stateNode);
    expect(query.value).toBe("四个人短局合作");
    expect(stateNode.dataset.runId).toBe("nl-run-456");
    expect(stateNode.textContent).toContain("loaded recommendations");
    expect(document.activeElement).toBe(stateNode.querySelector('[data-app-id="84"]'));
    unmount();
  });
});
