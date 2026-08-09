import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { FeedItem } from "../src/api/types";

const runtime = vi.hoisted(() => ({
  subscribe: vi.fn(() => () => undefined),
  submit: vi.fn((appId: number, type: string, recommendationRunId: string | null) => ({
    localId: `${appId}-${type}`,
    appId,
    type,
    recommendationRunId,
  })),
  undo: vi.fn(async () => undefined),
  postRecommendationEvent: vi.fn(async () => undefined),
}));

vi.mock("../src/app/runtime", () => ({
  apiClient: {
    isAccountAuthenticated: () => true,
    postRecommendationEvent: runtime.postRecommendationEvent,
  },
  feedbackQueue: {
    activeDimensionsForApp: () => ({ sentiment: null, ownership: null, reasons: [] }),
    subscribe: runtime.subscribe,
    submit: runtime.submit,
    undo: runtime.undo,
  },
}));

vi.mock("../src/app/ThemeProvider", () => ({
  useTheme: () => ({ fireAction: vi.fn() }),
}));

vi.mock("../src/app/ToastProvider", () => ({
  useToast: () => ({ show: vi.fn() }),
}));

vi.mock("../src/components/GameMedia", () => ({
  GameMedia: ({ name }: { name: string }) => <div>{name} cover</div>,
}));

vi.mock("../src/components/VoteButton", () => ({
  VoteButton: () => <button type="button">想玩</button>,
}));

import { GameCard } from "../src/screens/GameCard";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function item(overrides: Partial<FeedItem> = {}): FeedItem {
  return {
    app_id: 42,
    name: "Test Game",
    section: "recent_release",
    release_date: null,
    release_date_raw: null,
    release_date_precision: null,
    cover_url: null,
    cover_updated_at_ms: null,
    total_reviews: null,
    total_positive: null,
    latest_ccu: null,
    typical_ccu_7d: null,
    score: 0.91,
    confidence: 0.1,
    rank: 3,
    recommendation_index: 86,
    fit_band: "excellent",
    data_confidence: 0.8,
    slot_reason: "diversity",
    party: { recommended_min: 1, recommended_max: 4 },
    multiplayer: { dominant_mode: "private_coop" },
    play_intent: { count: 7, voted: false },
    reasons: ["适合当前小队"],
    cautions: [],
    evidence_ids: [],
    components: {
      friend_fit: 0.9,
      section_score: 0.85,
      personalized_score: 0.88,
      final_score: 0.91,
    },
    algorithm_version: "rules-0.3",
    ...overrides,
  };
}

describe("GameCard recommendation semantics", () => {
  afterEach(() => {
    runtime.subscribe.mockClear();
    runtime.submit.mockClear();
    runtime.undo.mockClear();
    runtime.postRecommendationEvent.mockClear();
  });

  it("attributes exposure and detail-open to the served run", () => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    const onOpen = vi.fn();
    try {
      act(() =>
        root.render(
          <GameCard item={item()} onOpen={onOpen} recommendationRunId="run-123" />,
        ),
      );

      expect(runtime.postRecommendationEvent).toHaveBeenCalledWith({
        recommendationRunId: "run-123",
        appId: 42,
        eventType: "exposure",
        idempotencyKey: "exposure:42",
      });

      act(() => host.querySelector<HTMLElement>("article")?.click());
      expect(runtime.postRecommendationEvent).toHaveBeenCalledWith({
        recommendationRunId: "run-123",
        appId: 42,
        eventType: "detail_open",
      });
      expect(onOpen).toHaveBeenCalledWith(42, "run-123");
    } finally {
      act(() => root.unmount());
      host.remove();
    }
  });

  it("shows rank/index and global play intent using true data confidence", () => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    try {
      act(() => root.render(<GameCard item={item()} onOpen={() => undefined} />));

      expect(host.textContent).toContain("第 3 推荐 · 很适合 · 推荐指数 86");
      expect(host.textContent).toContain("很适合");
      expect(host.textContent).toContain("多样性");
      expect(host.textContent).toContain("数据可靠度高 · 80%");
      expect(host.textContent).toContain("全站玩家想玩人数会小幅影响推荐");
      // The deprecated confidence field is deliberately ignored.
      expect(host.textContent).not.toContain("低置信数据");
      expect(host.querySelector(".score-badge")?.textContent).not.toContain("%");
      expect(host.textContent).toContain("玩过 / 已拥有");
      expect(host.textContent).toContain("“想玩”在上方单独记录");
    } finally {
      act(() => root.unmount());
      host.remove();
    }
  });

  it("only marks the first five items in recommendation context", () => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    try {
      act(() => root.render(<GameCard item={item({ rank: 6 })} onOpen={() => undefined} />));
      expect(host.querySelector(".score-badge")).toBeNull();
      expect(host.textContent).not.toContain("第 6 推荐");

      act(() =>
        root.render(
          <GameCard
            item={item({ rank: 2 })}
            onOpen={() => undefined}
            recommendationContext={false}
          />,
        ),
      );
      expect(host.querySelector(".score-badge")).toBeNull();
    } finally {
      act(() => root.unmount());
      host.remove();
    }
  });

  it("withholds the index and warns when data confidence is low", () => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    try {
      act(() =>
        root.render(
          <GameCard
            item={item({ confidence: 0.99, data_confidence: 0.44 })}
            onOpen={() => undefined}
          />,
        ),
      );

      expect(host.textContent).toContain("第 3 推荐 · 资料较少，待观察");
      expect(host.textContent).toContain("数据可靠度低 · 44%");
    } finally {
      act(() => root.unmount());
      host.remove();
    }
  });

  it("shows medium and unknown data reliability independently from the index", () => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    try {
      act(() =>
        root.render(<GameCard item={item({ data_confidence: 0.63 })} onOpen={() => undefined} />),
      );
      expect(host.textContent).toContain("数据可靠度中等 · 63%");
      expect(host.textContent).toContain("推荐指数 86");

      act(() =>
        root.render(<GameCard item={item({ data_confidence: null })} onOpen={() => undefined} />),
      );
      expect(host.textContent).toContain("数据可靠度未知");
      expect(host.textContent).toContain("资料较少，待观察");
    } finally {
      act(() => root.unmount());
      host.remove();
    }
  });

  it("keeps sentiment, ownership, reasons and play intent as separate controls", () => {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    try {
      act(() =>
        root.render(
          <GameCard
            item={item()}
            onOpen={() => undefined}
            recommendationRunId="run-123"
          />,
        ),
      );

      const buttons = () => Array.from(host.querySelectorAll("button"));
      const click = (label: string) => {
        const button = buttons().find((candidate) => candidate.textContent?.trim() === label);
        expect(button).toBeDefined();
        act(() => button?.click());
      };

      click("不感兴趣");
      expect(host.textContent).toContain("原因（可多选）");
      expect(host.textContent).toContain("人数不合适");
      expect(host.textContent).toContain("竞技性不合适");
      expect(host.textContent).toContain("开服或匹配麻烦");
      click("人数不合适");
      click("玩过 / 已拥有");

      expect(runtime.submit).toHaveBeenCalledWith(42, "not_interested", "run-123");
      expect(runtime.submit).toHaveBeenCalledWith(42, "party_size_mismatch", "run-123");
      expect(runtime.submit).toHaveBeenCalledWith(42, "played", "run-123");
      expect(host.textContent).toContain("想玩");
    } finally {
      act(() => root.unmount());
      host.remove();
    }
  });
});
