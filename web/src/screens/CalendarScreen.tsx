// Release calendar: recent/upcoming periods, early-data context and app-type filters.
// Layout styles live in styles/screens/calendar-search.css, scoped under .calendar-screen.

import { useEffect, useMemo, useState } from "react";
import { ApiError } from "../api/client";
import type { CalendarItem, CalendarPeriod, CalendarResponse } from "../api/types";
import { apiClient } from "../app/runtime";
import {
  appTypeLabel,
  calendarWhen,
  defaultWindow,
  groupByMonth,
  recentWindow,
} from "../app/calendar";
import { formatAgo, formatCount, isStale } from "../app/format";
import { Button } from "../components/Button";
import { Chip } from "../components/Chip";
import { EmptyState } from "../components/EmptyState";
import { GameMedia } from "../components/GameMedia";
import { Skeleton } from "../components/Skeleton";

interface CalendarState {
  data: CalendarResponse | null;
  loading: boolean;
  error: ApiError | null;
  fromOfflineCache: boolean;
}

type CalendarTypeFilter = "all" | "game" | "demo" | "playtest";

const TYPE_FILTER_LABELS: Record<CalendarTypeFilter, string> = {
  all: "全部",
  game: "正式游戏",
  demo: "Demo",
  playtest: "Playtest",
};

function matchesType(item: CalendarItem, typeFilter: CalendarTypeFilter): boolean {
  if (typeFilter === "all") return true;
  if (typeFilter === "game") return !["demo", "playtest"].includes(item.app_type);
  return item.app_type === typeFilter;
}

function CalendarRow({
  item,
  period,
  onOpenGame,
}: {
  item: CalendarItem;
  period: CalendarPeriod;
  onOpenGame: (appId: number) => void;
}) {
  const when = calendarWhen(item, Date.now());
  // Freshness stays available on hover without occupying row space; the
  // screen header already surfaces the overall data age.
  const freshness = `资料刷新于 ${formatAgo(item.updated_at_ms)}${
    item.source_modified_at_ms != null && item.source_modified_at_ms !== item.updated_at_ms
      ? ` · 来源观测 ${formatAgo(item.source_modified_at_ms)}`
      : ""
  }`;
  const showReviewCount =
    period === "recent" && typeof item.review_total === "number" && item.review_total > 0;
  const hasTags = item.app_type !== "game" || item.is_early_access === true || showReviewCount;
  return (
    <button
      type="button"
      className="cal-row"
      data-app-id={item.app_id}
      title={freshness}
      onClick={() => onOpenGame(item.app_id)}
    >
      <span className="cal-when">
        <span className="cal-when-primary">{when.primary}</span>
        {when.secondary && <span className="cal-when-secondary">{when.secondary}</span>}
      </span>
      <GameMedia coverUrl={item.cover_url ?? null} name={item.canonical_name} appId={item.app_id} compact />
      <span className="cal-name">{item.canonical_name}</span>
      {hasTags && (
        <span className="cal-tags">
          {item.app_type !== "game" && <Chip tone="accent">{appTypeLabel(item.app_type)}</Chip>}
          {item.is_early_access && <Chip tone="warn">抢先体验</Chip>}
          {showReviewCount && <Chip>{formatCount(item.review_total)} 条评价</Chip>}
        </span>
      )}
    </button>
  );
}

export function CalendarScreen({ onOpenGame }: { onOpenGame: (appId: number) => void }) {
  const [period, setPeriod] = useState<CalendarPeriod>("upcoming");
  const [typeFilter, setTypeFilter] = useState<CalendarTypeFilter>("all");
  const [state, setState] = useState<CalendarState>({
    data: null,
    loading: true,
    error: null,
    fromOfflineCache: false,
  });

  useEffect(() => {
    let cancelled = false;
    const { from, to } =
      period === "upcoming" ? defaultWindow(Date.now()) : recentWindow(Date.now(), 6);
    setState((prev) => ({ ...prev, loading: true, error: null }));
    apiClient
      .calendar(from, to, period)
      .then((result) => {
        if (cancelled) return;
        setState({
          data: result.data,
          loading: false,
          error: null,
          fromOfflineCache: result.fromOfflineCache,
        });
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setState({
          data: null,
          loading: false,
          error:
            error instanceof ApiError
              ? error
              : new ApiError({ code: "unknown", status: 0, message: String(error) }),
          fromOfflineCache: false,
        });
      });
    return () => {
      cancelled = true;
    };
  }, [period]);

  const dated = useMemo(
    () =>
      (state.data?.dated_items ?? []).filter((item) => matchesType(item, typeFilter)),
    [state.data, typeFilter],
  );
  const months = useMemo(() => groupByMonth(dated), [dated]);
  const undated = (state.data?.undated_items ?? []).filter((item) =>
    matchesType(item, typeFilter),
  );

  return (
    <section className="calendar-screen" aria-label="发售日历">
      <header className="screen-head calendar-head">
        <div className="calendar-head-title">
          <h2>发售日历</h2>
          <p>{period === "upcoming" ? "未来 60 天已知的发售窗口" : "近 6 个月已发售的条目"}</p>
        </div>
        <div className="calendar-head-meta">
          {state.data && (
            <Chip tone={isStale(state.data.data_updated_at_ms) ? "warn" : undefined}>
              数据更新于 {formatAgo(state.data.data_updated_at_ms)}
            </Chip>
          )}
          {state.fromOfflineCache && <Chip tone="danger">离线快照</Chip>}
        </div>
      </header>

      <div className="calendar-filters">
        <div className="calendar-filter-group">
          <span className="calendar-filter-label" aria-hidden="true">时间范围</span>
          <div className="seg" aria-label="日历时间范围">
            <Button
              size="small"
              aria-pressed={period === "upcoming"}
              onClick={() => setPeriod("upcoming")}
            >
              即将发售
            </Button>
            <Button
              size="small"
              aria-pressed={period === "recent"}
              onClick={() => setPeriod("recent")}
            >
              近期发售
            </Button>
          </div>
        </div>
        <div className="calendar-filter-group">
          <span className="calendar-filter-label" aria-hidden="true">类型</span>
          <div className="seg" aria-label="条目类型筛选">
            {(Object.keys(TYPE_FILTER_LABELS) as CalendarTypeFilter[]).map((type) => (
              <Button
                key={type}
                size="small"
                aria-pressed={typeFilter === type}
                onClick={() => setTypeFilter(type)}
              >
                {TYPE_FILTER_LABELS[type]}
              </Button>
            ))}
          </div>
        </div>
      </div>

      {state.loading && (
        <div className="calendar-skeleton-list" aria-busy="true">
          {Array.from({ length: 5 }, (_, i) => (
            <Skeleton key={i} height={64} />
          ))}
        </div>
      )}

      {!state.loading && state.error && (
        <EmptyState glyph={state.error.offline ? "⌁" : "!"} alert>
          <span>
            {state.error.offline ? "网络不可用，且没有可用的日历缓存。" : `加载失败：${state.error.message}`}
          </span>
        </EmptyState>
      )}

      {!state.loading && !state.error && months.length === 0 && undated.length === 0 && (
        <EmptyState glyph="∅">
          <span>当前时间与类型筛选下没有已知的发售条目。</span>
        </EmptyState>
      )}

      {months.map((group) => (
        <section key={group.key} className="cal-month" aria-label={group.label}>
          <header className="cal-month-head">
            <h3 className="cal-month-title">{group.label}</h3>
            <span className="cal-month-count">{group.items.length} 款</span>
          </header>
          <div className="cal-list">
            {group.items.map((item) => (
              <CalendarRow key={item.app_id} item={item} period={period} onOpenGame={onOpenGame} />
            ))}
          </div>
        </section>
      ))}

      {undated.length > 0 && (
        <section className="cal-month cal-month-undated" aria-label="日期未定">
          <header className="cal-month-head">
            <h3 className="cal-month-title">日期未定</h3>
            <span className="cal-month-count">{undated.length} 款</span>
          </header>
          <p className="cal-note">这些游戏尚未公布确切日期，不会被伪造成具体日期。</p>
          <div className="cal-list">
            {undated.map((item) => (
              <CalendarRow key={item.app_id} item={item} period={period} onOpenGame={onOpenGame} />
            ))}
          </div>
        </section>
      )}
    </section>
  );
}
