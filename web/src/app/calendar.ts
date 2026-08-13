// Calendar grouping + date helpers. Pure functions so the screen stays declarative.

import type { CalendarItem } from "../api/types";

export interface MonthGroup {
  /** YYYY-MM key. */
  key: string;
  label: string;
  items: CalendarItem[];
}

const MONTH_NAMES = [
  "1 月", "2 月", "3 月", "4 月", "5 月", "6 月",
  "7 月", "8 月", "9 月", "10 月", "11 月", "12 月",
];

/** `YYYY-MM-DD` -> `YYYY年 M月`. Returns null for unparseable input. */
export function monthLabel(day: string): string | null {
  const match = /^(\d{4})-(\d{2})/.exec(day);
  if (!match) return null;
  const year = match[1];
  const monthIdx = Number(match[2]) - 1;
  const name = MONTH_NAMES[monthIdx];
  if (!name) return null;
  return `${year}年 ${name}`;
}

/** Group dated calendar items by month, preserving ascending date order. */
export function groupByMonth(items: CalendarItem[]): MonthGroup[] {
  const sorted = [...items].sort((a, b) => (a.release_date ?? "").localeCompare(b.release_date ?? ""));
  const groups = new Map<string, MonthGroup>();
  for (const item of sorted) {
    const day = item.release_date;
    if (!day) continue;
    const key = day.slice(0, 7);
    let group = groups.get(key);
    if (!group) {
      group = { key, label: monthLabel(day) ?? key, items: [] };
      groups.set(key, group);
    }
    group.items.push(item);
  }
  return Array.from(groups.values());
}

export function appTypeLabel(appType: string): string {
  if (appType === "demo") return "Demo";
  if (appType === "playtest") return "Playtest";
  return "正式游戏";
}

const WEEKDAY_LABELS = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

/** `YYYY-MM-DD` -> 周几 (calendar days are interpreted in UTC, like the API). */
export function weekdayLabel(day: string): string | null {
  const ts = Date.parse(`${day}T00:00:00Z`);
  if (Number.isNaN(ts)) return null;
  return WEEKDAY_LABELS[new Date(ts).getUTCDay()] ?? null;
}

/** Whole-day distance between a calendar day and the viewer's local "today". */
export function countdownLabel(
  day: string,
  nowMs: number,
  timezoneOffsetMinutes = new Date(nowMs).getTimezoneOffset(),
): string | null {
  const target = Date.parse(`${day}T00:00:00Z`);
  if (Number.isNaN(target)) return null;
  const localNow = new Date(nowMs - timezoneOffsetMinutes * 60_000);
  const today = Date.UTC(
    localNow.getUTCFullYear(),
    localNow.getUTCMonth(),
    localNow.getUTCDate(),
  );
  const diff = Math.round((target - today) / 86_400_000);
  if (diff === 0) return "今天";
  if (diff === 1) return "明天";
  if (diff > 1) return `${diff} 天后`;
  if (diff === -1) return "昨天";
  return `${-diff} 天前`;
}

export interface CalendarWhen {
  /** Short date text for the row's date cell, faithful to source precision. */
  primary: string;
  /** Weekday + countdown context; only present for day-precision dates. */
  secondary: string | null;
}

/**
 * Row date cell. Month/quarter/year precision stays visibly fuzzy ("预计…")
 * instead of being dressed up as an exact day (PRD: 数据未知时显示未知).
 * The year is omitted for exact dates — the month group header carries it.
 */
export function calendarWhen(
  item: Pick<CalendarItem, "release_date" | "release_date_precision">,
  nowMs: number,
): CalendarWhen {
  const date = item.release_date;
  const precision = item.release_date_precision;
  if (!date) return { primary: "日期未定", secondary: null };
  const parts = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  if (!parts) return { primary: "日期未定", secondary: null };
  const year = parts[1];
  const month = Number(parts[2]);
  const dayOfMonth = Number(parts[3]);
  if (precision === "month") {
    return { primary: `预计 ${month} 月`, secondary: null };
  }
  if (precision === "quarter") {
    return { primary: `预计 Q${Math.floor((month - 1) / 3) + 1}`, secondary: null };
  }
  if (precision === "year") {
    return { primary: `预计 ${year} 年`, secondary: null };
  }
  // day precision (or unlabeled exact date from the store)
  const secondary =
    [weekdayLabel(date), countdownLabel(date, nowMs)].filter(Boolean).join(" · ") || null;
  return { primary: `${month} 月 ${dayOfMonth} 日`, secondary };
}

/** Format a Date as `YYYY-MM-DD` in UTC (calendar API uses calendar days). */
export function toDayString(date: Date): string {
  const y = date.getUTCFullYear();
  const m = String(date.getUTCMonth() + 1).padStart(2, "0");
  const d = String(date.getUTCDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

export const UPCOMING_WINDOW_DAYS = 60;

/** Default upcoming calendar window: today through +60 days. */
export function defaultWindow(
  now: number,
  days = UPCOMING_WINDOW_DAYS,
): { from: string; to: string } {
  const start = new Date(now);
  const end = new Date(now);
  const clamped = Math.min(Math.max(days, 1), 366);
  end.setUTCDate(end.getUTCDate() + clamped);
  return { from: toDayString(start), to: toDayString(end) };
}

/** Recent-release window: -months through today, clamped to one year. */
export function recentWindow(now: number, months = 6): { from: string; to: string } {
  const end = new Date(now);
  const start = new Date(now);
  const clamped = Math.min(Math.max(months, 1), 12);
  start.setUTCMonth(start.getUTCMonth() - clamped);
  return { from: toDayString(start), to: toDayString(end) };
}
