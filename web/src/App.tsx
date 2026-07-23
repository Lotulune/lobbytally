import { isTauri } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { activateServiceOrigin } from "./api/serverOrigin";
import { getClientStorage } from "./api/storage";
import { getConnectionManager } from "./app/connection";
import { activeServiceOrigin, isOnboarded, requiresServiceConnect } from "./app/runtime";
import { ThemeProvider } from "./app/ThemeProvider";
import { ToastProvider } from "./app/ToastProvider";
import { ConnectScreen } from "./screens/ConnectScreen";
import { OnboardingScreen } from "./screens/OnboardingScreen";
import { Shell } from "./screens/Shell";
import { WindowControls } from "./components/WindowTitlebar";

export function App() {
  const [onboarded, setOnboarded] = useState(isOnboarded);
  // The packaged desktop client must connect to a user-confirmed MPGS Server
  // before any business UI (PRD_CS CS-001). Dev/e2e builds skip the gate.
  // Setter intentionally unused: successful connect reloads instead of flipping
  // this flag in-place (avoids mounting business UI with empty apiClient).
  const [connected] = useState(
    () => !requiresServiceConnect || activeServiceOrigin !== null,
  );
  const desktop = isTauri();

  // Background recheck for a previously saved origin (PRD §5.2). The result
  // only updates the status banner; failures never drop the saved origin.
  useEffect(() => {
    if (connected && requiresServiceConnect) {
      void getConnectionManager().recheck();
    }
  }, [connected]);

  const handleConnected = (origin: string) => {
    activateServiceOrigin(getClientStorage(), origin);
    // Do not setConnected(true) before reload — that would mount business UI
    // with an empty apiClient.baseUrl in the same document. Reload rebuilds
    // every origin-scoped singleton against the persisted origin (CS-008).
    window.location.replace(window.location.href);
  };

  const gated = !connected;

  return (
    <ThemeProvider>
      <ToastProvider>
        {desktop && <div className="window-frame" aria-hidden="true" />}
        {desktop && (gated || !onboarded) && <WindowControls floating />}
        {gated ? (
          <ConnectScreen onConnected={handleConnected} />
        ) : onboarded ? (
          <Shell />
        ) : (
          <OnboardingScreen onDone={() => setOnboarded(true)} />
        )}
      </ToastProvider>
    </ThemeProvider>
  );
}
