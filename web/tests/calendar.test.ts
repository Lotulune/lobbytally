import { describe, expect, it } from "vitest";
import type { CalendarItem } from "../src/api/types";
import {
  appTypeLabel,
  calendarWhen,
  countdownLabel,
  defaultWindow,
  groupByMonth,
  monthLabel,
  recentWindow,
  toDayString,
  weekdayLabel,
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

  it("labels months", () => {
    expect(monthLabel("2026-08-02")).toBe("2026年 8 月");
    expect(monthLabel("bad")).toBeNull();
  });

  it("labels app types", () => {
    expect(appTypeLabel("demo")).toBe("Demo");
    expect(appTypeLabel("playtest")).toBe("Playtest");
    expect(appTypeLabel("game")).toBe("正式游戏");
  });

  it("labels weekdays and countdowns in local calendar days", () => {
    expect(weekdayLabel("2026-08-14")).toBe("周五");
    expect(weekdayLabel("bad")).toBeNull();
    const now = Date.UTC(2026, 7, 13, 15, 30); // 2026-08-13
    expect(countdownLabel("2026-08-13", now)).toBe("今天");
    expect(countdownLabel("2026-08-14", now)).toBe("明天");
    expect(countdownLabel("2026-08-20", now)).toBe("7 天后");
    expect(countdownLabel("2026-08-12", now)).toBe("昨天");
    expect(countdownLabel("2026-08-01", now)).toBe("12 天前");
  });

  it("uses the viewer date when local midnight differs from UTC", () => {
    const afterMidnightInShanghai = Date.parse("2026-08-12T16:30:00Z");
    expect(countdownLabel("2026-08-13", afterMidnightInShanghai, -480)).toBe("今天");
    expect(countdownLabel("2026-08-14", afterMidnightInShanghai, -480)).toBe("明天");
  });

  it("renders the date cell faithfully to source precision", () => {
    const now = Date.UTC(2026, 7, 13);
    const when = (date: string | null, precision: string | null) =>
      calendarWhen({ release_date: date, release_date_precision: precision }, now);

    expect(when("2026-08-14", "day")).toEqual({
      primary: "8 月 14 日",
      secondary: "周五 · 明天",
    });
    // Coarse precision must stay visibly fuzzy instead of a fake exact day.
    expect(when("2026-09-01", "month")).toEqual({ primary: "预计 9 月", secondary: null });
    expect(when("2026-10-01", "quarter")).toEqual({ primary: "预计 Q4", secondary: null });
    expect(when("2026-12-01", "year")).toEqual({ primary: "预计 2026 年", secondary: null });
    expect(when(null, "tba")).toEqual({ primary: "日期未定", secondary: null });
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
