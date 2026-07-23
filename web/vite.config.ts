import { defineConfig } from "vitest/config";
import { loadEnv } from "vite";
import react from "@vitejs/plugin-react";

// Browser dev server proxies API calls to the local mpgs-server so the web app
// can be developed without CORS friction. The packaged Tauri client is a pure
// client (PRD_CS): it has NO baked-in API base and talks to the user-confirmed
// service origin directly; the desktop CSP allows https: plus local dev ports.
// VITE_MPGS_API_BASE remains only for e2e builds that pin a test server.
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "VITE_");
  const configuredApiBase =
    env.VITE_MPGS_API_BASE?.replace(/\/$/, "") ??
    (mode === "e2e" ? "http://127.0.0.1:18080" : undefined);
  const devApiProxyTarget =
    env.VITE_MPGS_DEV_PROXY_TARGET?.replace(/\/$/, "") ?? "http://127.0.0.1:17880";

  return {
    plugins: [react()],
    define: configuredApiBase
      ? { "import.meta.env.VITE_MPGS_API_BASE": JSON.stringify(configuredApiBase) }
      : undefined,
    server: {
      port: 5173,
      strictPort: true,
      proxy: {
        "/v1": { target: devApiProxyTarget, changeOrigin: true },
        "/health": { target: devApiProxyTarget, changeOrigin: true },
        "/.well-known": { target: devApiProxyTarget, changeOrigin: true },
        "/openapi.json": { target: devApiProxyTarget, changeOrigin: true },
      },
    },
    build: {
      target: "es2022",
      sourcemap: false,
    },
    test: {
      environment: "jsdom",
      globals: false,
      include: ["tests/**/*.test.ts", "tests/**/*.test.tsx"],
    },
  };
});
