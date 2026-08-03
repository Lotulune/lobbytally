// Feed screen: section tabs live in the shell; this renders one section's list
// with page-based pagination and optional sort controls.

import { useEffect, useState } from "react";
import type { FeedSection, FeedSort, FeedSortOrder } from "../api/types";
import { FEED_SORT_OPTIONS } from "../api/types";
import { formatAgo, isStale, SECTION_META } from "../app/format";
import { feedbackQueue } from "../app/runtime";
import { defaultOrderForSort, useFeed } from "../app/useFeed";
import { Button } from "../components/Button";
import { Chip } from "../components/Chip";
import { EmptyState } from "../components/EmptyState";
import { Pagination } from "../components/Pagination";
import { Skeleton } from "../components/Skeleton";
import { GameCard } from "./GameCard";

function defaultSortFor(section: FeedSection): FeedSort {
  return section === "upcoming" ? "release_date" : "recommended";
}

function RankedFeedPanel({
  section,
  onOpenGame,
}: {
  section: FeedSection;
  onOpenGame: (appId: number, recommendationRunId?: string | null) => void;
}) {
  // Parent remounts this panel with key={section}, so sort/order always match
  // the active tab without a mid-render reset.
  const [sort, setSort] = useState<FeedSort>(() => defaultSortFor(section));
  const [order, setOrder] = useState<FeedSortOrder>(() =>
    defaultOrderForSort(defaultSortFor(section), section),
  );

  const feed = useFeed(section, sort, order);
  const meta = SECTION_META[section];

  useEffect(() => feedbackQueue.subscribeRankingChanged(feed.reload), [feed.reload]);

  const onSelectSort = (next: FeedSort) => {
    setSort(next);
    setOrder(defaultOrderForSort(next, section));
  };

  const toggleOrder = () => {
    setOrder((current) => (current === "asc" ? "desc" : "asc"));
  };

  const changePage = (p: number) => {
    feed.goToPage(p);
    document.querySelector<HTMLElement>("main.main")?.scrollTo({ top: 0, behavior: "smooth" });
  };

  return (
    <section aria-label={meta.label}>
      <header className="feed-head">
        <div className="statusline">
          <span>{meta.hint}</span>
          {section === "upcoming" && (
            <Chip>按推荐流展示；完整按日日历见「日历」页</Chip>
          )}
          {feed.dataUpdatedAtMs !== null && (
            <Chip tone={isStale(feed.dataUpdatedAtMs) ? "warn" : undefined}>
              数据更新于 {formatAgo(feed.dataUpdatedAtMs)}
            </Chip>
          )}
          {feed.fromOfflineCache && <Chip tone="danger">离线快照</Chip>}
          {feed.algorithmVersion && <Chip>{feed.algorithmVersion}</Chip>}
        </div>

        <div className="feed-sortbar" role="toolbar" aria-label="排序">
          {FEED_SORT_OPTIONS.map((option) => (
            <Button
              key={option.id}
              size="small"
              variant={sort === option.id ? "primary" : "ghost"}
              aria-pressed={sort === option.id}
              onClick={() => onSelectSort(option.id)}
            >
              {option.label}
            </Button>
          ))}
          {sort !== "recommended" && (
            <Button
              size="small"
              variant="ghost"
              onClick={toggleOrder}
              title={order === "asc" ? "升序" : "降序"}
              aria-label={order === "asc" ? "当前升序，点击切换为降序" : "当前降序，点击切换为升序"}
            >
              {order === "asc" ? "升序 ↑" : "降序 ↓"}
            </Button>
          )}
        </div>
      </header>

      {feed.loading && (
        <div className="feed-grid" aria-busy="true">
          {Array.from({ length: 6 }, (_, i) => (
            <Skeleton key={i} />
          ))}
        </div>
      )}

      {!feed.loading && feed.error && (
        <EmptyState glyph={feed.error.offline ? "⌁" : "!"} alert>
          <span>
            {feed.error.offline
              ? "网络不可用，且本地没有可用的缓存快照。"
              : `加载失败：${feed.error.message}`}
          </span>
          {feed.error.requestId && (
            <span style={{ fontSize: 11, opacity: 0.6 }}>request_id: {feed.error.requestId}</span>
          )}
          <Button onClick={feed.reload}>重试</Button>
        </EmptyState>
      )}

      {!feed.loading && !feed.error && feed.items.length === 0 && (
        <EmptyState glyph="∅">
          <span>
            {section === "upcoming"
              ? "暂无符合条件的即将发售多人候选。可在「日历」页查看发售窗口，或等待商店采集写入 coming soon 记录。"
              : "该分区暂时没有符合条件的候选。数据允许时会如实展示，不会伪造推荐。"}
          </span>
          <Button onClick={feed.reload}>刷新</Button>
        </EmptyState>
      )}

      {feed.items.length > 0 && (
        <>
          <Pagination
            page={feed.page}
            totalPages={feed.totalPages}
            total={feed.total}
            loading={feed.loading}
            onPage={changePage}
          />
          <div className="feed-grid">
            {feed.items.map((item) => (
              <GameCard
                key={item.app_id}
                item={item}
                onOpen={onOpenGame}
                recommendationRunId={feed.recommendationRunId}
              />
            ))}
          </div>
          <Pagination
            page={feed.page}
            totalPages={feed.totalPages}
            total={feed.total}
            loading={feed.loading}
            onPage={changePage}
          />
        </>
      )}
    </section>
  );
}

export function FeedScreen({
  section,
  onOpenGame,
}: {
  section: FeedSection;
  onOpenGame: (appId: number, recommendationRunId?: string | null) => void;
}) {
  return <RankedFeedPanel key={section} section={section} onOpenGame={onOpenGame} />;
}
