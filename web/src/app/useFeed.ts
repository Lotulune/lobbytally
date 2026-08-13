// Feed loading hook: page-based navigation with offline cache for first page,
// stale/offline surfacing. State resets when section changes.

import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "../api/client";
import type {
  FeedItem,
  FeedResponse,
  FeedSection,
  FeedSort,
  FeedSortOrder,
} from "../api/types";
import { apiClient } from "./runtime";

export const FEED_PAGE_SIZE = 12;

export interface FeedState {
  items: FeedItem[];
  loading: boolean;
  error: ApiError | null;
  page: number;
  total: number;
  totalPages: number;
  dataUpdatedAtMs: number | null;
  fromOfflineCache: boolean;
  algorithmVersion: string | null;
  recommendationRunId: string | null;
}

const INITIAL: FeedState = {
  items: [],
  loading: true,
  error: null,
  page: 1,
  total: 0,
  totalPages: 0,
  dataUpdatedAtMs: null,
  fromOfflineCache: false,
  algorithmVersion: null,
  recommendationRunId: null,
};

function toApiError(error: unknown): ApiError {
  return error instanceof ApiError
    ? error
    : new ApiError({
        code: "unknown",
        status: 0,
        message: error instanceof Error ? error.message : "unknown error",
      });
}

export function defaultOrderForSort(sort: FeedSort, section: FeedSection): FeedSortOrder {
  if (sort === "release_date") {
    // Upcoming: soonest first. Recent release: newest store day first.
    return section === "upcoming" ? "asc" : "desc";
  }
  return "desc";
}

export function useFeed(
  section: FeedSection,
  sort: FeedSort = "recommended",
  order?: FeedSortOrder,
  pageSize: number | null = FEED_PAGE_SIZE,
): FeedState & {
  reload: () => void;
  goToPage: (page: number) => void;
} {
  const [state, setState] = useState<FeedState>(INITIAL);
  const [page, setPage] = useState(1);
  const pageRef = useRef(1);
  const generation = useRef(0);
  const lastPageSize = useRef<number | null>(null);
  const resolvedOrder = order ?? defaultOrderForSort(sort, section);

  const load = useCallback(
    (targetPage: number) => {
      // null = responsive page size not measured yet; the first fetch waits
      // for the real value instead of loading a guess and reloading.
      if (pageSize === null) return;
      const gen = generation.current + 1;
      generation.current = gen;
      setState((prev) => ({ ...INITIAL, loading: true, page: targetPage, total: prev.total, totalPages: prev.totalPages }));
      apiClient
        .feed(section, {
          limit: pageSize,
          page: targetPage,
          sort,
          order: resolvedOrder,
        })
        .then((result) => {
          if (generation.current !== gen) return;
          const data: FeedResponse = result.data;
          setState({
            items: data.items,
            loading: false,
            error: null,
            page: data.page ?? targetPage,
            total: data.total ?? data.items.length,
            totalPages: data.total_pages ?? (data.items.length > 0 ? 1 : 0),
            dataUpdatedAtMs: data.data_updated_at_ms,
            fromOfflineCache: result.fromOfflineCache,
            algorithmVersion: data.algorithm_version,
            recommendationRunId: data.recommendation_run_id ?? null,
          });
        })
        .catch((error: unknown) => {
          if (generation.current !== gen) return;
          setState((prev) => ({
            ...prev,
            loading: false,
            error: toApiError(error),
            page: targetPage,
          }));
        });
    },
    [section, sort, resolvedOrder, pageSize],
  );

  useEffect(() => {
    if (pageSize === null) return;
    const previousSize = lastPageSize.current;
    lastPageSize.current = pageSize;
    let targetPage = 1;
    if (previousSize !== null && previousSize !== pageSize) {
      // Window resize changed the page size: stay near the same items instead
      // of snapping back to page 1.
      const firstItemIndex = (pageRef.current - 1) * previousSize;
      targetPage = Math.floor(firstItemIndex / pageSize) + 1;
    }
    pageRef.current = targetPage;
    setPage(targetPage);
    load(targetPage);
  }, [load, pageSize]);

  const goToPage = useCallback(
    (targetPage: number) => {
      if (targetPage < 1) return;
      pageRef.current = targetPage;
      setPage(targetPage);
      load(targetPage);
    },
    [load],
  );

  const reload = useCallback(() => {
    load(page);
  }, [load, page]);

  return { ...state, reload, goToPage };
}
