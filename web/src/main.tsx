import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { initializeClientStorage } from "./api/storage";
import "./styles/base.css";
import "./styles/themes.css";
import "./styles/screens/game-detail.css";
import "./styles/screens/nl.css";
import "./styles/screens/community.css";
import "./styles/screens/calendar-search.css";
import "./styles/screens/settings.css";
import "./styles/screens/data-ops.css";

const container = document.getElementById("root");
if (!container) {
  throw new Error("missing #root element");
}

async function bootstrap(): Promise<void> {
  await initializeClientStorage();
  const [{ App }, { loadSavedTheme }] = await Promise.all([
    import("./App"),
    import("./theme/registry"),
  ]);

  // Apply the saved theme before mounting. ThemeProvider then installs FX.
  document.documentElement.dataset.theme = loadSavedTheme() ?? "steam";
  createRoot(container as HTMLElement).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void bootstrap().catch((error: unknown) => {
  container.textContent = `客户端存储初始化失败：${error instanceof Error ? error.message : String(error)}`;
});
