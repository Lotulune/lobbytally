import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DataStatusResponse } from "../src/api/types";

const runtime = vi.hoisted(() => ({
  adminDataStatus: vi.fn(),
}));

vi.mock("../src/app/runtime", () => ({
  apiClient: {
    adminDataStatus: runtime.adminDataStatus,
    adminAppPresence: vi.fn(),
    search: vi.fn(),
  },
}));

import { DataOpsScreen } from "../src/screens/DataOpsScreen";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function dataStatus(current: number): DataStatusResponse {
  return {
    tasks: [
      {
        task_name: "candidate_enrichment",
        last_success_at_ms: null,
        next_run_at_ms: null,
        last_error_category: null,
        cursor_value: null,
        coverage_ratio: null,
        updated_at_ms: 1_000,
      },
    ],
    coverage: {
      normalized_multiplayer_candidates: 100,
      category_evidence_candidates: 100,
      recommendation_ready_profiles: 50,
      trusted_familiar_profiles: 0,
      with_platforms: 0,
      with_languages: 80,
      with_typical_session: 0,
      with_price: 90,
      with_reviews: 20,
      with_ccu: 18,
    },
    m7_coverage: {
      normalized_multiplayer_candidates: 100,
      trusted_friend_multiplayer_profiles: 0,
      candidates_with_date: 99,
      candidates_with_cover: 100,
      upcoming_candidates: 8,
      recent_release_candidates: 66,
      popular_legacy_candidates: 5,
      classic_legacy_candidates: 21,
      trusted_profiles_with_seven_day_reviews: 0,
      trusted_profiles_with_seven_day_ccu: 0,
    },
    dimension_coverage: {
      candidates: 100,
      released_candidates: 80,
      store_details_checked: 100,
      store_details: 100,
      release_date: 99,
      reviews_checked: 20,
      reviews: 20,
      ccu_checked: 22,
      ccu: 18,
      price_checked: 100,
      price: 90,
      languages: 80,
      retrieval_index: 100,
    },
    latest_runs: [
      {
        task_type: "candidate_enrichment",
        status: "running",
        started_at_ms: 1_000,
        finished_at_ms: null,
        request_count: current * 4,
        success_count: current * 3,
        error_category: null,
        notes: `phase=enrichment;apps_attempted=${current};apps_total=20`,
      },
    ],
    worker_queue: {
      pending: 3,
      pending_due: 2,
      leased: 1,
      active_jobs: [
        {
          job_id: 7,
          task_type: "enrich_catalog",
          entity_key: "scheduled",
          attempts: 1,
          max_attempts: 3,
          lease_expires_at_ms: 100_000,
          updated_at_ms: 1_000,
        },
      ],
    },
    integrated_ingestion: {
      pending: 12,
      retry: 1,
      leased: 1,
      dead: 0,
      store_details: 4,
      review_summary: 4,
      popular_reviews: 4,
      ccu: 4,
      oldest_dead_at_ms: null,
      dead_by_stage: [],
      dead_by_category: [],
      recent_dead: [],
    },
    inventory: {
      apps_total: 200,
      multiplayer_profiles: 100,
      released_with_date: 99,
      released_last_14_days: 10,
      coming_soon_total: 10,
      coming_soon_dated: 8,
      unknown_named_stubs: 0,
      max_release_date: "2026-08-10",
      max_release_date_app_id: 42,
      max_release_date_name: "Test Game",
      jobs_pending: 12,
      jobs_leased: 1,
      jobs_dead: 0,
      jobs_dead_recent: 0,
    },
    generated_at_ms: 1_000,
  };
}

describe("DataOpsScreen live worker progress", () => {
  afterEach(() => {
    vi.useRealTimers();
    localStorage.clear();
    runtime.adminDataStatus.mockReset();
  });

  it("refreshes the active batch progress every five seconds", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(10_000);
    localStorage.setItem("mpgs.admin_token.v1", "test-token");
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "visible",
    });
    runtime.adminDataStatus
      .mockResolvedValueOnce(dataStatus(2))
      .mockResolvedValueOnce(dataStatus(7));

    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    try {
      await act(async () => {
        root.render(<DataOpsScreen />);
        await Promise.resolve();
      });

      expect(runtime.adminDataStatus).toHaveBeenCalledWith("test-token");
      expect(host.textContent).toContain("2 / 20 款");
      expect(host.textContent).toContain("当前工作：补发售日与详情");
      expect(host.textContent).toContain("待领取 3");
      expect(host.textContent).toContain("可立即领取 2");
      expect(host.textContent).toContain("已租约 1");
      expect(host.textContent).toContain("玩家评价已采集（已发售）20 / 80 · 25%");
      expect(host.textContent).toContain("价格状态已检查100 / 100 · 100%");
      expect(host.textContent).toContain("有具体价格90款");
      expect(host.querySelector('[role="progressbar"]')?.getAttribute("aria-valuenow")).toBe("2");
      expect(
        host.querySelectorAll(".section-metrics:not(.data-ops-availability) .section-metric"),
      ).toHaveLength(4);
      expect(host.querySelectorAll(".task-row")).toHaveLength(1);

      await act(async () => {
        vi.advanceTimersByTime(5_000);
        await Promise.resolve();
      });

      expect(runtime.adminDataStatus).toHaveBeenCalledTimes(2);
      expect(host.textContent).toContain("7 / 20 款");
      expect(host.querySelector('[role="progressbar"]')?.getAttribute("aria-valuenow")).toBe("7");
    } finally {
      act(() => root.unmount());
      host.remove();
    }
  });
});
