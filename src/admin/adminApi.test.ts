import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  applyAdminReviewAction,
  completeSetup,
  createAdminTask,
  getAdminAuditEvents,
  getAdminConnectionShare,
  getAdminDiagnostics,
  getAdminOverview,
  getAdminReviewQueue,
  getAdminTasks,
  getAdminConfigState,
  getSetupStatus,
  loginAdmin,
  requestRestart,
} from "./adminApi";

describe("admin API client", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it("reads first-run setup status from the same-origin API", async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ configured: false }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(getSetupStatus()).resolves.toEqual({ configured: false });

    expect(fetchMock).toHaveBeenCalledWith("/api/v1/setup/status", {
      headers: { Accept: "application/json" },
    });
  });

  it("posts first-run setup completion without storing tokens in the browser", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(jsonResponse({ configured: true, restartRequired: true }));
    vi.stubGlobal("fetch", fetchMock);

    await completeSetup({
      setupToken: "setup-token",
      serviceName: "MPGS Public",
      publicBaseUrl: "https://mpgs.example.test",
      databaseUrl: "postgres://mpgs:secret@postgres:5432/mpgs",
      adminToken: "admin-token",
      steamApiKey: "steam-key",
    });

    expect(fetchMock).toHaveBeenCalledWith("/api/v1/setup/complete", {
      body: JSON.stringify({
        setupToken: "setup-token",
        serviceName: "MPGS Public",
        publicBaseUrl: "https://mpgs.example.test",
        databaseUrl: "postgres://mpgs:secret@postgres:5432/mpgs",
        adminToken: "admin-token",
        steamApiKey: "steam-key",
      }),
      headers: {
        Accept: "application/json",
        "Content-Type": "application/json",
      },
      method: "POST",
    });
  });

  it("exchanges an admin token for an HttpOnly same-origin session cookie", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(jsonResponse({ authenticated: true }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(loginAdmin("admin-token")).resolves.toEqual({ authenticated: true });

    expect(fetchMock).toHaveBeenCalledWith("/api/v1/admin/session", {
      body: JSON.stringify({ token: "admin-token" }),
      credentials: "same-origin",
      headers: {
        Accept: "application/json",
        "Content-Type": "application/json",
      },
      method: "POST",
    });
  });

  it("uses same-origin credentials for authenticated management reads and restart", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        jsonResponse({
          serviceName: "MPGS Public",
          publicCatalogStatus: "empty",
          publicGameCount: 0,
          pendingReviewCount: 0,
          latestTask: null,
          failureSummary: {
            openFailureCount: 0,
            retryableFailureCount: 0,
            latestFailure: null,
          },
          restartRequired: false,
          connectionShareConfigured: true,
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          postgres: "ok",
          activeConfig: "ok",
          safeMode: false,
          publicBaseUrlStatus: "configured",
          httpsStatus: "ok",
          publicCors: "disabled",
          restartPolicy: "configured",
          steam: "configured",
          llm: "missing",
          r2: "missing",
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          activeConfigVersion: "sha256:active",
          restartRequired: false,
          lastStartupStatus: "ok",
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          serviceName: "MPGS Public",
          serviceInstanceId: "018fb770-8998-7699-a6e4-b7b59f2f9c01",
          apiVersion: "v1",
          baseUrl: "https://mpgs.example.test",
          serviceInfoUrl: "https://mpgs.example.test/api/v1/service-info",
          capabilities: ["public_catalog_read"],
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          events: [
            {
              eventType: "admin.session.login",
              actor: "admin",
              outcome: "success",
            },
          ],
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          recentTasks: [
            {
              id: 7,
              taskType: "manual_appid_discovery",
              status: "failed",
              target: "appid:440",
              targetAppid: 440,
              createdAt: "2026-06-08 03:00:00+00",
              updatedAt: "2026-06-08 03:05:00+00",
            },
          ],
          failureSummary: {
            openFailureCount: 1,
            retryableFailureCount: 1,
            latestFailure: {
              taskId: 7,
              stage: "steam_lookup",
              target: "appid:440",
              provider: "steam",
              retryable: true,
              attempt: 2,
              reason: "Steam lookup timed out.",
              createdAt: "2026-06-08 03:05:00+00",
            },
          },
          failures: [],
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          items: [
            {
              appid: 440,
              name: "Team Fortress 2",
              reviewStatus: "needs_review",
              visibility: "hidden",
              recommendationScore: 86,
              updatedAt: "2026-06-08 04:00:00+00",
              reviewNote: "Needs moderator confirmation.",
            },
          ],
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          game: {
            appid: 440,
            name: "Team Fortress 2",
            reviewStatus: "accepted",
            visibility: "public",
            recommendationScore: 86,
            updatedAt: "2026-06-08 04:02:00+00",
            reviewNote: "Looks good.",
          },
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          task: {
            id: 8,
            taskType: "manual_appid_discovery",
            status: "queued",
            target: "appid:730",
            targetAppid: 730,
            createdAt: "2026-06-08 04:00:00+00",
            updatedAt: "2026-06-08 04:00:00+00",
          },
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({ restartScheduled: true, mode: "self_exit" }, 202),
      );
    vi.stubGlobal("fetch", fetchMock);

    await getAdminOverview();
    await getAdminDiagnostics();
    await getAdminConfigState();
    await getAdminConnectionShare();
    await getAdminAuditEvents();
    await getAdminTasks();
    await getAdminReviewQueue();
    await applyAdminReviewAction(440, {
      action: "accept_public",
      note: "Looks good.",
    });
    await createAdminTask({
      taskType: "manual_appid_discovery",
      appid: 730,
    });
    await requestRestart();

    expect(fetchMock).toHaveBeenNthCalledWith(1, "/api/v1/admin/overview", {
      credentials: "same-origin",
      headers: { Accept: "application/json" },
    });
    expect(fetchMock).toHaveBeenNthCalledWith(2, "/api/v1/admin/diagnostics", {
      credentials: "same-origin",
      headers: { Accept: "application/json" },
    });
    expect(fetchMock).toHaveBeenNthCalledWith(3, "/api/v1/admin/config-state", {
      credentials: "same-origin",
      headers: { Accept: "application/json" },
    });
    expect(fetchMock).toHaveBeenNthCalledWith(
      4,
      "/api/v1/admin/connection-share",
      {
        credentials: "same-origin",
        headers: { Accept: "application/json" },
      },
    );
    expect(fetchMock).toHaveBeenNthCalledWith(5, "/api/v1/admin/audit-events", {
      credentials: "same-origin",
      headers: { Accept: "application/json" },
    });
    expect(fetchMock).toHaveBeenNthCalledWith(6, "/api/v1/admin/tasks", {
      credentials: "same-origin",
      headers: { Accept: "application/json" },
    });
    expect(fetchMock).toHaveBeenNthCalledWith(7, "/api/v1/admin/review-queue", {
      credentials: "same-origin",
      headers: { Accept: "application/json" },
    });
    expect(fetchMock).toHaveBeenNthCalledWith(
      8,
      "/api/v1/admin/review-queue/440/action",
      {
        body: JSON.stringify({
          action: "accept_public",
          note: "Looks good.",
        }),
        credentials: "same-origin",
        headers: {
          Accept: "application/json",
          "Content-Type": "application/json",
        },
        method: "POST",
      },
    );
    expect(fetchMock).toHaveBeenNthCalledWith(9, "/api/v1/admin/tasks", {
      body: JSON.stringify({
        taskType: "manual_appid_discovery",
        appid: 730,
      }),
      credentials: "same-origin",
      headers: {
        Accept: "application/json",
        "Content-Type": "application/json",
      },
      method: "POST",
    });
    expect(fetchMock).toHaveBeenNthCalledWith(10, "/api/v1/admin/restart", {
      body: JSON.stringify({ confirm: true }),
      credentials: "same-origin",
      headers: {
        Accept: "application/json",
        "Content-Type": "application/json",
      },
      method: "POST",
    });
  });
});

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    headers: { "Content-Type": "application/json" },
    status,
  });
}
