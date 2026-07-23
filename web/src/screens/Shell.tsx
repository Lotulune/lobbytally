// App shell: hosts the Topbar and all view screens. The `view` state here is
// the single source of navigation truth; keyboard shortcuts and tab clicks
// all flow through the same `navigate` callback. Subscriptions (connectivity,
// account profile, demo mode, pending feedback) live here and are passed to
// the presentational Topbar as props.

import { useCallback, useEffect, useRef, useState } from "react";
import type { AccountProfile } from "../api/types";
import { subscribeAccountGate } from "../app/auth";
import { getConnectionManager, type ConnectionSnapshot } from "../app/connection";
import { apiClient, feedbackQueue, requiresServiceConnect } from "../app/runtime";
import { FeedScreen } from "./FeedScreen";
import { GameDetailScreen } from "./GameDetailScreen";
import { SearchScreen } from "./SearchScreen";
import { CalendarScreen } from "./CalendarScreen";
import { SettingsScreen } from "./SettingsScreen";
import { NaturalLanguageScreen } from "./NaturalLanguageScreen";
import { AiSettingsScreen } from "./AiSettingsScreen";
import { AuthDialog } from "./AuthDialog";
import { CommunityScreen } from "./CommunityScreen";
import { ProfileScreen } from "./ProfileScreen";
import { Topbar } from "./shell/Topbar";
import { useNavShortcuts } from "./shell/useNavShortcuts";
import { DEFAULT_VIEW, type ListView, type View } from "./shell/nav";

export function Shell() {
  const [view, setView] = useState<View>(DEFAULT_VIEW);
  const [online, setOnline] = useState(() => navigator.onLine);
  const [pendingCount, setPendingCount] = useState(() => feedbackQueue.pendingCount());
  const [profile, setProfile] = useState<AccountProfile | null>(null);
  const [authOpen, setAuthOpen] = useState(false);
  const [demoMode, setDemoMode] = useState(false);
  // Service link health for the offline/maintenance banner (PRD §5.2).
  const [connection, setConnection] = useState<ConnectionSnapshot | null>(null);
  // Where the game detail returns to (the list the user opened it from).
  const lastListView = useRef<ListView>(DEFAULT_VIEW);
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
    if (!requiresServiceConnect) return;
    return getConnectionManager().subscribe(setConnection);
  }, []);

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

  // Single navigation entry point: tabs, shortcuts, and menus all use this.
  const navigate = useCallback((next: View) => setView(next), []);
  useNavShortcuts(navigate);

  const openGame = useCallback((appId: number) => setView({ kind: "game", appId }), []);
  const backToList = useCallback(() => setView(lastListView.current), []);
  const leaveAccountArea = useCallback(() => {
    setProfile(null);
    setView(DEFAULT_VIEW);
  }, []);

  return (
    <div className="shell">
      <Topbar
        view={view}
        onNavigate={navigate}
        online={online}
        demoMode={demoMode}
        pendingCount={pendingCount}
        profile={profile}
        onLogin={() => setAuthOpen(true)}
        onProfile={() => navigate({ kind: "profile" })}
        onAiSettings={() => navigate({ kind: "ai-settings" })}
        onLogout={leaveAccountArea}
      />

      {connection?.status === "offline" && (
        <div className="connection-banner" role="status">
          离线模式：暂时无法连接服务，正在展示本机缓存；写操作将在恢复后同步。
          <button type="button" className="banner-retry" onClick={() => void getConnectionManager().recheck()}>
            重试连接
          </button>
        </div>
      )}
      {connection?.status === "maintenance" && (
        <div className="connection-banner" data-tone="maintenance" role="status">
          服务维护中：部分功能可能不可用，请稍后再试。
          <button type="button" className="banner-retry" onClick={() => void getConnectionManager().recheck()}>
            重新检查
          </button>
        </div>
      )}

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
