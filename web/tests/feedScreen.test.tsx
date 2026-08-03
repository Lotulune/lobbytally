import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const runtime = vi.hoisted(() => ({
  goToPage: vi.fn(),
  subscribeRankingChanged: vi.fn(() => () => undefined),
}));

vi.mock("../src/app/runtime", () => ({
  feedbackQueue: { subscribeRankingChanged: runtime.subscribeRankingChanged },
}));

vi.mock("../src/app/useFeed", () => ({
  defaultOrderForSort: () => "desc",
  useFeed: () => ({
    items: [{ app_id: 1, name: "Test Game" }],
    loading: false,
    error: null,
    page: 1,
    total: 24,
    totalPages: 2,
    dataUpdatedAtMs: null,
    fromOfflineCache: false,
    algorithmVersion: null,
    reload: vi.fn(),
    goToPage: runtime.goToPage,
  }),
}));

vi.mock("../src/screens/GameCard", () => ({
  GameCard: () => <article className="card">Test Game</article>,
}));

import { FeedScreen } from "../src/screens/FeedScreen";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("FeedScreen", () => {
  afterEach(() => {
    runtime.goToPage.mockReset();
    runtime.subscribeRankingChanged.mockClear();
  });

  it("scrolls the actual main container to the top after pagination", () => {
    const main = document.createElement("main");
    main.className = "main";
    const scrollTo = vi.fn();
    main.scrollTo = scrollTo;
    document.body.append(main);
    const root = createRoot(main);
    try {
      act(() => root.render(<FeedScreen section="recent_release" onOpenGame={() => undefined} />));
      const next = Array.from(main.querySelectorAll("button")).find(
        (button) => button.textContent?.trim() === "下一页",
      );
      expect(next).toBeTruthy();

      act(() => next?.click());

      expect(runtime.goToPage).toHaveBeenCalledWith(2);
      expect(scrollTo).toHaveBeenCalledWith({ top: 0, behavior: "smooth" });
    } finally {
      act(() => root.unmount());
      main.remove();
    }
  });

  it("does not offer a meaningless direction toggle for recommended order", () => {
    const main = document.createElement("main");
    document.body.append(main);
    const root = createRoot(main);
    try {
      act(() => root.render(<FeedScreen section="recent_release" onOpenGame={() => undefined} />));

      expect(main.querySelector('[aria-label*="当前降序"]')).toBeNull();

      const ccu = Array.from(main.querySelectorAll("button")).find(
        (button) => button.textContent?.trim() === "在线人数",
      );
      act(() => ccu?.click());

      expect(main.querySelector('[aria-label*="当前降序"]')).not.toBeNull();
    } finally {
      act(() => root.unmount());
      main.remove();
    }
  });

  it("offers a strict fit-index sort with an explicit direction", () => {
    const main = document.createElement("main");
    document.body.append(main);
    const root = createRoot(main);
    try {
      act(() => root.render(<FeedScreen section="recent_release" onOpenGame={() => undefined} />));

      const fit = Array.from(main.querySelectorAll("button")).find(
        (button) => button.textContent?.trim() === "适配指数",
      );
      expect(fit).toBeTruthy();

      act(() => fit?.click());
      expect(main.querySelector('[aria-label*="当前降序"]')).not.toBeNull();
    } finally {
      act(() => root.unmount());
      main.remove();
    }
  });
});
