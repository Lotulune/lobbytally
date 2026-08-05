import { describe, expect, it } from "vitest";
import type { CalendarItem } from "../src/api/types";
import {
  appTypeLabel,
  confidenceLabel,
  dayLabel,
  defaultWindow,
  earlyDataLabel,
  groupByMonth,
  monthLabel,
  precisionLabel,
  recentWindow,
  toDayString,
} from "../src/app/calendar";

function item(appId: number, releaseDate: string | null): CalendarItem {
  return {
    app_id: appId,
    app_type: "game",
    canonical_name: `Game ${appId}`,
    release_state: releaseDate ? "coming_soon" : "unreleased",
    release_date: releaseDate,
    release_date_raw: releaseDate,
    release_date_precision: releaseDate ? "day" : "unknown",
    is_early_access: null,
    current_data_confidence: null,
    review_total: null,
    early_data: false,
    source_modified_at_ms: null,
    created_at_ms: 0,
    updated_at_ms: 0,
  };
}

describe("calendar helpers", () => {
  it("groups dated items by month in ascending order", () => {
    const groups = groupByMonth([
      item(1, "2026-09-15"),
      item(2, "2026-08-02"),
      item(3, "2026-08-20"),
    ]);
    expect(groups.map((g) => g.key)).toEqual(["2026-08", "2026-09"]);
    expect(groups[0]?.items.map((i) => i.app_id)).toEqual([2, 3]);
    expect(groups[1]?.items.map((i) => i.app_id)).toEqual([1]);
  });

  it("ignores items without a date in month grouping", () => {
    const groups = groupByMonth([item(1, null), item(2, "2026-08-02")]);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.key).toBe("2026-08");
  });

  it("labels months and days", () => {
    expect(monthLabel("2026-08-02")).toBe("2026年 8 月");
    expect(monthLabel("bad")).toBeNull();
    expect(dayLabel("2026-08-02")).toBe("8月2日");
    expect(dayLabel(null)).toBe("日期未定");
  });

  it("labels precision and returns null when absent", () => {
    expect(precisionLabel("month")).toBe("预计月份");
    expect(precisionLabel(null)).toBeNull();
    expect(precisionLabel("weird")).toBe("weird");
  });

  it("labels app types and data confidence honestly", () => {
    expect(appTypeLabel("demo")).toBe("Demo");
    expect(appTypeLabel("playtest")).toBe("Playtest");
    expect(appTypeLabel("game")).toBe("正式游戏");
    expect(confidenceLabel(null)).toBe("置信度未知");
    expect(confidenceLabel(0.42)).toBe("低置信 42%");
    expect(confidenceLabel(0.82)).toBe("高置信 82%");
    expect(earlyDataLabel(true, 42)).toBe("早期数据 · 42 条评价");
    expect(earlyDataLabel(false, 42)).toBeNull();
  });

  it("builds a 60-day upcoming window in UTC", () => {
    const now = Date.UTC(2026, 6, 15); // 2026-07-15
    expect(toDayString(new Date(now))).toBe("2026-07-15");
    const w = defaultWindow(now);
    expect(w.from).toBe("2026-07-15");
    expect(w.to).toBe("2026-09-13");
    const clamped = defaultWindow(now, 999);
    expect(clamped.to).toBe("2027-07-16"); // clamped to +366 days
    expect(recentWindow(now, 6)).toEqual({ from: "2026-01-15", to: "2026-07-15" });
  });
});
