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

/** `YYYY-MM-DD` -> `M月D日`, else the raw string. */
export function dayLabel(day: string | null): string {
  if (!day) return "日期未定";
  const match = /^\d{4}-(\d{2})-(\d{2})$/.exec(day);
  if (!match) return day;
  return `${Number(match[1])}月${Number(match[2])}日`;
}

const PRECISION_LABELS: Record<string, string> = {
  day: "具体日期",
  month: "预计月份",
  quarter: "预计季度",
  year: "预计年份",
  unknown: "日期未定",
};

export function precisionLabel(precision: string | null): string | null {
  if (!precision) return null;
  return PRECISION_LABELS[precision] ?? precision;
}

export function confidenceLabel(confidence: number | null): string {
  if (confidence === null) return "置信度未知";
  if (confidence >= 0.8) return `高置信 ${Math.round(confidence * 100)}%`;
  if (confidence >= 0.5) return `中置信 ${Math.round(confidence * 100)}%`;
  return `低置信 ${Math.round(confidence * 100)}%`;
}

export function appTypeLabel(appType: string): string {
  if (appType === "demo") return "Demo";
  if (appType === "playtest") return "Playtest";
  return "正式游戏";
}

export function earlyDataLabel(earlyData: boolean, reviewTotal: number | null): string | null {
  if (!earlyData) return null;
  return reviewTotal === null ? "早期数据" : `早期数据 · ${reviewTotal} 条评价`;
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
