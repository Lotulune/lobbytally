import { describe, expect, it } from "vitest";
import {
  FEED_ROWS_PER_PAGE,
  feedColumnsForWidth,
  pageSizeForColumns,
} from "../src/app/useFeedPageSize";

const CARD_MIN = 330;
const GAP = 16;

describe("responsive feed page size", () => {
  it("mirrors the CSS auto-fill column count", () => {
    // Column fits when width >= n*cardMin + (n-1)*gap.
    expect(feedColumnsForWidth(330, CARD_MIN, GAP)).toBe(1);
    expect(feedColumnsForWidth(675, CARD_MIN, GAP)).toBe(1); // 2 cols need 676
    expect(feedColumnsForWidth(676, CARD_MIN, GAP)).toBe(2);
    expect(feedColumnsForWidth(1236, CARD_MIN, GAP)).toBe(3); // 1280 window
    expect(feedColumnsForWidth(1876, CARD_MIN, GAP)).toBe(5); // 1920 window
    expect(feedColumnsForWidth(2160, CARD_MIN, GAP)).toBe(6); // capped content
  });

  it("falls back to one column for degenerate widths", () => {
    expect(feedColumnsForWidth(0, CARD_MIN, GAP)).toBe(1);
    expect(feedColumnsForWidth(-100, CARD_MIN, GAP)).toBe(1);
    expect(feedColumnsForWidth(Number.NaN, CARD_MIN, GAP)).toBe(1);
  });

  it("keeps every page a whole number of rows, capped at the server limit", () => {
    expect(pageSizeForColumns(1)).toBe(FEED_ROWS_PER_PAGE);
    expect(pageSizeForColumns(3)).toBe(3 * FEED_ROWS_PER_PAGE);
    expect(pageSizeForColumns(6)).toBe(6 * FEED_ROWS_PER_PAGE);
    expect(pageSizeForColumns(80)).toBe(100);
  });
});
