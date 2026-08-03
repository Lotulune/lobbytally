import { useEffect, useMemo, useRef, useState } from "react";
import { ApiError } from "../api/client";
import type {
  CommunityFilters,
  CommunityItem,
  CommunityPlatform,
  CommunityReleaseState,
  CommunityResponse,
  CommunitySort,
} from "../api/types";
import { apiClient, playIntentStore } from "../app/runtime";
import { formatAgo, formatCount, formatReleaseDate, releaseStateLabel } from "../app/format";
import { Button } from "../components/Button";
import { EmptyState } from "../components/EmptyState";
import { Facepile } from "../components/Facepile";
import { GameMedia } from "../components/GameMedia";
import { Skeleton } from "../components/Skeleton";
import { VoteButton } from "../components/VoteButton";

export function CommunityScreen({ onOpenGame }: { onOpenGame: (appId: number) => void }) {
  const [sort, setSort] = useState<CommunitySort>("trending");
  const [releaseState, setReleaseState] = useState<CommunityReleaseState | "">("");
  const [demoOnly, setDemoOnly] = useState(false);
  const [platform, setPlatform] = useState<CommunityPlatform | "">("");
  const [partySize, setPartySize] = useState("");
  const [items, setItems] = useState<CommunityItem[]>([]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Generation guard for loadMore: bumped on filter/sort change and unmount so
  // an in-flight "load more" request can't apply stale items after the view
  // moved on.
  const loadMoreGen = useRef(0);
  const loadingMoreRef = useRef(false);
  const filters = useMemo<CommunityFilters>(() => ({
    ...(releaseState ? { releaseState } : {}),
    ...(demoOnly ? { demoOnly: true } : {}),
    ...(platform ? { platform } : {}),
    ...(partySize && Number(partySize) >= 1 && Number(partySize) <= 64
      ? { partySize: Number(partySize) }
      : {}),
  }), [demoOnly, partySize, platform, releaseState]);

  const apply = (response: CommunityResponse, append: boolean) => {
    setItems((current) => {
      if (!append) return response.items;
      const seen = new Set(current.map((item) => item.app_id));
      return [...current, ...response.items.filter((item) => !seen.has(item.app_id))];
    });
    setNextCursor(response.next_cursor);
    setUpdatedAt(response.data_updated_at_ms);
  };

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    void apiClient
      .community(sort, filters)
      .then((result) => {
        if (!cancelled) apply(result.data, false);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        setError(cause instanceof ApiError && cause.offline ? "当前离线，无法加载社区榜单。" : "无法加载社区榜单。");
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [filters, sort]);

  useEffect(() => {
    let cancelled = false;
    const unsubscribe = playIntentStore.subscribe(() => {
      // The authoritative response invalidates cached community snapshots after
      // a successful optimistic vote. A no-op observer keeps this screen fresh.
      if (playIntentStore.pendingCount()) return;
      void apiClient
        .community(sort, filters)
        .then((result) => {
          if (!cancelled) apply(result.data, false);
        })
        .catch(() => undefined);
    });
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, [filters, sort]);

  // Invalidate any in-flight "load more" when the query changes or on unmount.
  useEffect(() => {
    return () => {
      loadMoreGen.current += 1;
    };
  }, [filters, sort]);

  const loadMore = async () => {
    if (!nextCursor || loadingMoreRef.current) return;
    loadingMoreRef.current = true;
    setLoadingMore(true);
    const gen = loadMoreGen.current;
    const stale = () => gen !== loadMoreGen.current;
    try {
      const result = await apiClient.community(sort, filters, nextCursor);
      if (stale()) return;
      apply(result.data, true);
    } catch (cause) {
      if (stale()) return;
      if (cause instanceof ApiError && cause.code === "cursor_stale") {
        // Nested try: a failing first-page refresh must not become an
        // unhandled rejection from the outer catch.
        try {
          const first = await apiClient.community(sort, filters);
          if (stale()) return;
          apply(first.data, false);
        } catch (refreshCause) {
          if (stale()) return;
          setError(
            refreshCause instanceof ApiError && refreshCause.offline
              ? "当前离线，无法加载社区榜单。"
              : "无法加载更多条目。",
          );
        }
      } else {
        setError(
          cause instanceof ApiError && cause.offline
            ? "当前离线，无法加载社区榜单。"
            : "无法加载更多条目。",
        );
      }
    } finally {
      loadingMoreRef.current = false;
      setLoadingMore(false);
    }
  };

  return (
    <section className="community-screen" aria-label="大家想玩">
      <header className="community-toolbar">
        <div className="community-toolbar-head">
          <h2>大家想玩</h2>
          {updatedAt !== null && <p className="community-updated">数据更新于 {formatAgo(updatedAt)}</p>}
        </div>
        <div className="community-sort">
          <span className="community-toolbar-label" aria-hidden="true">排序</span>
          <div className="seg" aria-label="社区排序">
            <Button size="small" aria-pressed={sort === "trending"} onClick={() => setSort("trending")}>正在升温</Button>
            <Button size="small" aria-pressed={sort === "most_voted"} onClick={() => setSort("most_voted")}>最多人想玩</Button>
          </div>
        </div>
      </header>
      <div className="community-filters" aria-label="社区筛选">
        <label>
          <span>发售状态</span>
          <select value={releaseState} onChange={(event) => setReleaseState(event.target.value as CommunityReleaseState | "")}>
            <option value="">全部</option>
            <option value="upcoming">即将发售</option>
            <option value="coming_soon">敬请期待</option>
            <option value="released">已发售</option>
            <option value="retired">已下架</option>
            <option value="unknown">状态未知</option>
          </select>
        </label>
        <label>
          <span>平台</span>
          <select value={platform} onChange={(event) => setPlatform(event.target.value as CommunityPlatform | "")}>
            <option value="">全部</option>
            <option value="windows">Windows</option>
            <option value="macos">macOS</option>
            <option value="linux">Linux</option>
          </select>
        </label>
        <label>
          <span>人数</span>
          <input
            type="number"
            min={1}
            max={64}
            inputMode="numeric"
            value={partySize}
            onChange={(event) => setPartySize(event.target.value)}
          />
        </label>
        <label className="community-demo-filter">
          <input type="checkbox" checked={demoOnly} onChange={(event) => setDemoOnly(event.target.checked)} />
          <span>仅 Demo</span>
        </label>
      </div>
      {loading && (
        <div className="community-skeleton-list" aria-busy="true">
          {Array.from({ length: 4 }, (_, i) => (
            <Skeleton key={i} height={106} />
          ))}
        </div>
      )}
      {!loading && error && items.length === 0 && (
        <EmptyState glyph="!" alert>
          <span>{error}</span>
        </EmptyState>
      )}
      {!loading && !error && items.length === 0 && (
        <EmptyState glyph="∅">
          <span>还没有公开的想玩投票。</span>
        </EmptyState>
      )}
      <div className="community-list">
        {items.map((item) => (
          <article key={item.app_id} className="community-row">
            <button
              type="button"
              className="community-main"
              data-app-id={item.app_id}
              onClick={() => onOpenGame(item.app_id)}
            >
              <GameMedia coverUrl={item.cover_url} name={item.name} appId={item.app_id} compact />
              <span className="community-copy">
                <strong className="community-name">{item.name}</strong>
                <span className="community-meta">{releaseStateLabel(item.release_state)} · {formatReleaseDate(item.release_date, item.release_date_raw, item.release_date_precision)}</span>
                <span className="community-trend">{sort === "trending" ? `近 7 天 ${formatCount(item.trending_count)} 人加入` : `${formatCount(item.play_intent.count)} 人想玩`}</span>
              </span>
            </button>
            <div className="community-social">
              <Facepile voters={item.play_intent.voters_preview ?? []} omittedCount={item.play_intent.omitted_count ?? 0} total={item.play_intent.count} />
              <VoteButton appId={item.app_id} intent={item.play_intent} />
            </div>
          </article>
        ))}
      </div>
      {error && items.length > 0 && (
        <p className="community-error-note" role="alert">{error}</p>
      )}
      {nextCursor && (
        <div className="community-more">
          <Button onClick={() => void loadMore()} disabled={loadingMore}>
            {loadingMore ? "加载中…" : "加载更多"}
          </Button>
        </div>
      )}
    </section>
  );
}
