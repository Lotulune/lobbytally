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
        <img
          className="brand-icon"
          src="/app-icon-192.png?v=transparent-v1"
          alt=""
          aria-hidden="true"
          draggable={false}
          data-tauri-drag-region
        />
        <span className="brand-copy" data-tauri-drag-region>
          LobbyTally
          <small data-tauri-drag-region>熟人联机推荐</small>
        </span>
      </div>
      <NavTabs view={view} onNavigate={onNavigate} />
      <div className="topbar-controls">
        <StatusChips online={online} demoMode={demoMode} pendingCount={pendingCount} />
        <a
          className="tab github-link"
          href="https://github.com/Lotulune/lobbytally"
          target="_blank"
          rel="noopener noreferrer"
          aria-label="查看 LobbyTally 的 GitHub 仓库（在新窗口打开）"
          title="GitHub · Lotulune/lobbytally"
        >
          <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" focusable="false">
            <path d="M12 .75a11.25 11.25 0 0 0-3.558 21.923c.563.104.768-.244.768-.542 0-.267-.01-.974-.015-1.912-3.13.68-3.791-1.508-3.791-1.508-.512-1.3-1.25-1.646-1.25-1.646-1.022-.699.078-.684.078-.684 1.13.08 1.725 1.16 1.725 1.16 1.004 1.72 2.633 1.223 3.275.935.102-.727.393-1.223.715-1.504-2.498-.284-5.124-1.249-5.124-5.562 0-1.229.44-2.233 1.16-3.02-.116-.284-.503-1.429.11-2.978 0 0 .944-.302 3.094 1.154A10.79 10.79 0 0 1 12 6.188c.956.004 1.919.129 2.818.379 2.148-1.456 3.09-1.154 3.09-1.154.615 1.549.228 2.694.112 2.978.722.787 1.158 1.791 1.158 3.02 0 4.324-2.63 5.275-5.136 5.553.404.349.766 1.034.766 2.084 0 1.505-.014 2.719-.014 3.088 0 .301.203.651.774.541A11.252 11.252 0 0 0 12 .75Z" />
          </svg>
        </a>
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
