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
): FeedState & {
  reload: () => void;
  goToPage: (page: number) => void;
} {
  const [state, setState] = useState<FeedState>(INITIAL);
  const [page, setPage] = useState(1);
  const generation = useRef(0);
  const resolvedOrder = order ?? defaultOrderForSort(sort, section);

  const load = useCallback(
    (targetPage: number) => {
      const gen = generation.current + 1;
      generation.current = gen;
      setState((prev) => ({ ...INITIAL, loading: true, page: targetPage, total: prev.total, totalPages: prev.totalPages }));
      apiClient
        .feed(section, {
          limit: FEED_PAGE_SIZE,
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
    [section, sort, resolvedOrder],
  );

  useEffect(() => {
    setPage(1);
    load(1);
  }, [load]);

  const goToPage = useCallback(
    (targetPage: number) => {
      if (targetPage < 1) return;
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
