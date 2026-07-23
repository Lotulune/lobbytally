// Topbar info hierarchy: brand | four feed sections | auxiliary entries |
// status chips | theme & FX controls | account menu.

import type { AccountProfile } from "../../api/types";
import { useTheme } from "../../app/ThemeProvider";
import type { FxIntensity } from "../../fx/types";
import { Button } from "../../components/Button";
import { AccountMenu } from "../AccountMenu";
import { NavTabs } from "./NavTabs";
import { StatusChips } from "./StatusChips";
import { ThemeMenu } from "./ThemeMenu";
import type { ListView, View } from "./nav";
import { WindowControls } from "../../components/WindowTitlebar";

const FX_LABELS: Record<FxIntensity, string> = { off: "特效关", low: "特效低", full: "特效全" };
const FX_CYCLE: FxIntensity[] = ["full", "low", "off"];

export function Topbar({
  view,
  onNavigate,
  online,
  demoMode,
  pendingCount,
  profile,
  onLogin,
  onProfile,
  onAiSettings,
  onLogout,
}: {
  view: View;
  onNavigate: (view: ListView) => void;
  online: boolean;
  demoMode: boolean;
  pendingCount: number;
  profile: AccountProfile | null;
  onLogin: () => void;
  onProfile: () => void;
  onAiSettings: () => void;
  onLogout: () => void;
}) {
  const { intensity, setIntensity } = useTheme();

  const cycleFx = () => {
    const idx = FX_CYCLE.indexOf(intensity);
    const next = FX_CYCLE[(idx + 1) % FX_CYCLE.length] ?? "full";
    setIntensity(next);
  };

  return (
    <header className="topbar">
      {/* Drag only on brand — keep nav/controls free of data-tauri-drag-region. */}
      <div className="brand" data-tauri-drag-region>
        MPGS
        <small data-tauri-drag-region>熟人联机推荐</small>
      </div>
      <NavTabs view={view} onNavigate={onNavigate} />
      <div className="topbar-controls">
        <StatusChips online={online} demoMode={demoMode} pendingCount={pendingCount} />
        <ThemeMenu />
        <Button size="small" variant="ghost" onClick={cycleFx} aria-label="切换特效强度">
          {FX_LABELS[intensity]}
        </Button>
        <AccountMenu
          profile={profile}
          onLogin={onLogin}
          onProfile={onProfile}
          onAiSettings={onAiSettings}
          onLogout={onLogout}
        />
        {/* Inline chrome: part of the topbar, next to 登录 (not floating OS-style). */}
        <WindowControls />
      </div>
    </header>
  );
}
