// Bilibili-style adaptive paging: page size follows the card grid's column
// count so every page renders complete rows at any window width. The column
// math mirrors CSS `repeat(auto-fill, minmax(var(--feed-card-min), 1fr))`,
// with the shared tokens read from the stylesheet so CSS stays the single
// source of truth.

import { useLayoutEffect, useState } from "react";
import type { RefObject } from "react";

export const FEED_ROWS_PER_PAGE = 4;
/** Server-side feed limit cap (mpgs-server validates limit 1..=100). */
const MAX_PAGE_SIZE = 100;
const FALLBACK_CARD_MIN = 330;
const FALLBACK_GAP = 16;
const RESIZE_DEBOUNCE_MS = 200;

/** Column count the auto-fill grid produces for a given content width. */
export function feedColumnsForWidth(width: number, cardMin: number, gap: number): number {
  if (!(width > 0) || !(cardMin > 0)) return 1;
  return Math.max(1, Math.floor((width + gap) / (cardMin + gap)));
}

export function pageSizeForColumns(columns: number): number {
  return Math.min(Math.max(1, columns) * FEED_ROWS_PER_PAGE, MAX_PAGE_SIZE);
}

function readPx(style: CSSStyleDeclaration, name: string, fallback: number): number {
  const value = Number.parseFloat(style.getPropertyValue(name));
  return Number.isFinite(value) && value > 0 ? value : fallback;
}

/**
 * Measure the feed host element and derive the page size. Returns null until
 * the first measurement so the initial fetch can wait for the real value
 * instead of loading a guessed size and immediately reloading.
 */
export function useFeedPageSize(hostRef: RefObject<HTMLElement | null>): number | null {
  const [pageSize, setPageSize] = useState<number | null>(null);

  useLayoutEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const measure = () => {
      const style = getComputedStyle(host);
      const cardMin = readPx(style, "--feed-card-min", FALLBACK_CARD_MIN);
      const gap = readPx(style, "--feed-grid-gap", FALLBACK_GAP);
      const columns = feedColumnsForWidth(host.clientWidth, cardMin, gap);
      setPageSize(pageSizeForColumns(columns));
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const observer = new ResizeObserver(() => {
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(measure, RESIZE_DEBOUNCE_MS);
    });
    observer.observe(host);
    return () => {
      if (timer !== null) clearTimeout(timer);
      observer.disconnect();
    };
  }, [hostRef]);

  return pageSize;
}
