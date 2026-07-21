// App shell: brand, primary nav (4 feed sections + search/calendar/settings),
// theme switcher, FX intensity, connectivity status. Hosts all view screens.

import { useCallback, useEffect, useRef, useState } from "react";
import type { AccountProfile, FeedSection } from "../api/types";
import { FEED_SECTIONS } from "../api/types";
import { subscribeAccountGate } from "../app/auth";
import { apiClient, feedbackQueue } from "../app/runtime";
import { useTheme } from "../app/ThemeProvider";
import { SECTION_META } from "../app/format";
import { THEME_ORDER, THEMES } from "../theme/registry";
import type { FxIntensity } from "../fx/types";
import { FeedScreen } from "./FeedScreen";
import { GameDetailScreen } from "./GameDetailScreen";
import { SearchScreen } from "./SearchScreen";
import { CalendarScreen } from "./CalendarScreen";
import { SettingsScreen } from "./SettingsScreen";
import { NaturalLanguageScreen } from "./NaturalLanguageScreen";
import { AccountMenu } from "./AccountMenu";
import { AiSettingsScreen } from "./AiSettingsScreen";
import { AuthDialog } from "./AuthDialog";
import { CommunityScreen } from "./CommunityScreen";
import { ProfileScreen } from "./ProfileScreen";

type ListView =
  | { kind: "feed"; section: FeedSection }
  | { kind: "search" }
  | { kind: "natural-language" }
  | { kind: "community" }
  | { kind: "calendar" }
  | { kind: "settings" }
  | { kind: "profile" }
  | { kind: "ai-settings" };

type View = ListView | { kind: "game"; appId: number };

const FX_LABELS: Record<FxIntensity, string> = { off: "特效关", low: "特效低", full: "特效全" };
const FX_CYCLE: FxIntensity[] = ["full", "low", "off"];

const AUX_TABS: { view: ListView; label: string; glyph: string }[] = [
  { view: { kind: "community" }, label: "大家想玩", glyph: "▲" },
  { view: { kind: "natural-language" }, label: "描述推荐", glyph: "✦" },
  { view: { kind: "search" }, label: "搜索", glyph: "⌕" },
  { view: { kind: "calendar" }, label: "日历", glyph: "▦" },
  { view: { kind: "settings" }, label: "设置", glyph: "⚙" },
];

export function Shell() {
  const { themeId, setTheme, intensity, setIntensity } = useTheme();
  const [view, setView] = useState<View>({ kind: "feed", section: "recent_release" });
  const [online, setOnline] = useState(() => navigator.onLine);
  const [pendingCount, setPendingCount] = useState(() => feedbackQueue.pendingCount());
  const [profile, setProfile] = useState<AccountProfile | null>(null);
  const [authOpen, setAuthOpen] = useState(false);
  const [demoMode, setDemoMode] = useState(false);
  // Where the game detail returns to (the list the user opened it from).
  const lastListView = useRef<ListView>({ kind: "feed", section: "recent_release" });
  useEffect(() => {
    if (view.kind !== "game") lastListView.current = view;
  }, [view]);

  useEffect(() => {
    const update = () => setOnline(navigator.onLine);
    window.addEventListener("online", update);
    window.addEventListener("offline", update);
    return () => {
      window.removeEventListener("online", update);
      window.removeEventListener("offline", update);
    };
  }, []);

  useEffect(() => {
    const loadProfile = () => {
      if (!apiClient.isAccountAuthenticated()) {
        setProfile(null);
        return;
      }
      void apiClient.getMe().then(setProfile).catch(() => setProfile(null));
    };
    loadProfile();
    return apiClient.subscribeAuth(loadProfile);
  }, []);

  useEffect(() => subscribeAccountGate(() => setAuthOpen(true)), []);

  useEffect(() => {
    let cancelled = false;
    void apiClient.meta().then((result) => {
      if (!cancelled) setDemoMode(result.data.demo_mode);
    }).catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    return feedbackQueue.subscribe(() => setPendingCount(feedbackQueue.pendingCount()));
  }, []);

  // Keyboard: 1-4 switch feed sections; "/" opens search. Ignored while typing.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target && ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName)) return;
      if (event.key === "/") {
        event.preventDefault();
        setView({ kind: "search" });
        return;
      }
      const idx = Number(event.key) - 1;
      const next = FEED_SECTIONS[idx];
      if (idx >= 0 && idx < FEED_SECTIONS.length && next) {
        setView({ kind: "feed", section: next });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const openGame = useCallback((appId: number) => setView({ kind: "game", appId }), []);
  const backToList = useCallback(() => setView(lastListView.current), []);
  const leaveAccountArea = useCallback(() => {
    setProfile(null);
    setView({ kind: "feed", section: "recent_release" });
  }, []);

  const cycleFx = () => {
    const idx = FX_CYCLE.indexOf(intensity);
    const next = FX_CYCLE[(idx + 1) % FX_CYCLE.length] ?? "full";
    setIntensity(next);
  };

  const activeSection = view.kind === "feed" ? view.section : null;

  return (
    <div className="shell">
      <header className="topbar">
        <div className="brand">
          MPGS
          <small>熟人联机推荐</small>
        </div>
        <nav className="tabs" role="tablist" aria-label="主导航">
          {FEED_SECTIONS.map((s, idx) => (
            <button
              key={s}
              type="button"
              role="tab"
              className="tab"
              data-testid={`nav-feed-${s}`}
              aria-selected={activeSection === s}
              onClick={() => setView({ kind: "feed", section: s })}
            >
              <span className="tab-key">{idx + 1}</span>
              {SECTION_META[s].label}
            </button>
          ))}
          <span className="tab-sep" aria-hidden="true" />
          {AUX_TABS.map((tab) => (
            <button
              key={tab.view.kind}
              type="button"
              role="tab"
              className="tab"
              data-testid={`nav-${tab.view.kind}`}
              aria-selected={view.kind === tab.view.kind}
              onClick={() => setView(tab.view)}
            >
              <span className="tab-glyph" aria-hidden="true">
                {tab.glyph}
              </span>
              {tab.label}
            </button>
          ))}
        </nav>
        <div className="topbar-controls">
          {!online && <span className="chip danger">离线</span>}
          {demoMode && <span className="chip warn">演示数据</span>}
          {pendingCount > 0 && <span className="chip warn">{pendingCount} 条待同步</span>}
          <label className="sr-label">
            <span className="sr-only">切换主题</span>
            <select
              className="btn small"
              value={themeId}
              onChange={(event) => {
                const next = event.target.value;
                if (next in THEMES) setTheme(next as keyof typeof THEMES);
              }}
              aria-label="切换主题"
            >
              {THEME_ORDER.map((id) => (
                <option key={id} value={id}>
                  {THEMES[id].label}
                </option>
              ))}
            </select>
          </label>
          <button type="button" className="btn small ghost" onClick={cycleFx} aria-label="切换特效强度">
            {FX_LABELS[intensity]}
          </button>
          <AccountMenu
            profile={profile}
            onLogin={() => setAuthOpen(true)}
            onProfile={() => setView({ kind: "profile" })}
            onAiSettings={() => setView({ kind: "ai-settings" })}
            onLogout={leaveAccountArea}
          />
        </div>
      </header>

      <main className="main">
        {view.kind === "feed" && <FeedScreen section={view.section} onOpenGame={openGame} />}
        {view.kind === "search" && <SearchScreen onOpenGame={openGame} />}
        {view.kind === "natural-language" && <NaturalLanguageScreen onOpenGame={openGame} />}
        {view.kind === "community" && <CommunityScreen onOpenGame={openGame} />}
        {view.kind === "calendar" && <CalendarScreen onOpenGame={openGame} />}
        {view.kind === "settings" && <SettingsScreen />}
        {view.kind === "profile" && profile && <ProfileScreen profile={profile} onUpdated={setProfile} onDeleted={leaveAccountArea} />}
        {view.kind === "ai-settings" && profile && <AiSettingsScreen />}
        {view.kind === "game" && <GameDetailScreen appId={view.appId} onBack={backToList} />}
      </main>
      <AuthDialog open={authOpen} onClose={() => setAuthOpen(false)} />
    </div>
  );
}
