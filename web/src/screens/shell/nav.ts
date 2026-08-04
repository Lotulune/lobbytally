// Navigation model for the app shell. The `view` state in Shell.tsx is the
// single source of truth; every entry point (tabs, keyboard shortcuts, menus)
// navigates through the same callback.

import type { FeedSection } from "../../api/types";

export type ListView =
  | { kind: "feed"; section: FeedSection }
  | { kind: "search" }
  | { kind: "natural-language" }
  | { kind: "community" }
  | { kind: "calendar" }
  | { kind: "settings" }
  | { kind: "data-ops" }
  | { kind: "profile" }
  | { kind: "ai-settings" };

export type View = ListView | { kind: "game"; appId: number };

export const DEFAULT_VIEW: ListView = { kind: "feed", section: "recent_release" };

interface AuxTab {
  view: ListView;
  label: string;
  glyph: string;
}

export const AUX_TABS: AuxTab[] = [
  { view: { kind: "community" }, label: "大家想玩", glyph: "▲" },
  { view: { kind: "natural-language" }, label: "描述推荐", glyph: "✦" },
  { view: { kind: "search" }, label: "搜索", glyph: "⌕" },
  { view: { kind: "calendar" }, label: "日历", glyph: "▦" },
  { view: { kind: "data-ops" }, label: "监测", glyph: "▣" },
  { view: { kind: "settings" }, label: "设置", glyph: "⚙" },
];
