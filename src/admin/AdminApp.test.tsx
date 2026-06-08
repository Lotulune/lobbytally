// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AdminApp } from "./AdminApp";

describe("AdminApp", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it("shows first-run setup when the service has not been configured", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ configured: false }))
      .mockResolvedValueOnce(
        jsonResponse({ configured: true, restartRequired: true }),
      );
    vi.stubGlobal("fetch", fetchMock);

    render(<AdminApp />);

    expect(await screen.findByRole("heading", { name: "首次配置" })).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("引导令牌"), {
      target: { value: "setup-token" },
    });
    fireEvent.change(screen.getByLabelText("服务名称"), {
      target: { value: "MPGS Public" },
    });
    fireEvent.change(screen.getByLabelText("公开服务地址"), {
      target: { value: "https://mpgs.example.test" },
    });
    fireEvent.change(screen.getByLabelText("Postgres 地址"), {
      target: { value: "postgres://mpgs:secret@postgres:5432/mpgs" },
    });
    fireEvent.change(screen.getByLabelText("管理员令牌"), {
      target: { value: "admin-token" },
    });
    fireEvent.change(screen.getByLabelText("Steam API Key"), {
      target: { value: "steam-key" },
    });
    fireEvent.click(screen.getByRole("button", { name: "完成配置" }));

    expect(await screen.findByText("配置已写入，重启后生效。")).toBeInTheDocument();
  });

  it("shows admin login when setup is already configured", async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ configured: true }));
    vi.stubGlobal("fetch", fetchMock);

    render(<AdminApp />);

    expect(await screen.findByRole("heading", { name: "管理员登录" })).toBeInTheDocument();
    expect(screen.getByLabelText("管理员令牌")).toBeInTheDocument();
  });

  it("loads overview, diagnostics, config state, and connection share after login", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ configured: true }))
      .mockResolvedValueOnce(jsonResponse({ authenticated: true }))
      .mockResolvedValueOnce(
        jsonResponse({
          serviceName: "MPGS Public",
          publicCatalogStatus: "empty",
          publicGameCount: 0,
          pendingReviewCount: 0,
          latestTask: {
            id: 7,
            taskType: "manual_appid_discovery",
            status: "failed",
            target: "appid:440",
            targetAppid: 440,
            createdAt: "2026-06-08 03:00:00+00",
            updatedAt: "2026-06-08 03:05:00+00",
          },
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
          restartRequired: true,
          connectionShareConfigured: true,
          latestAuditEvent: {
            eventType: "admin.restart.requested",
            actor: "admin",
            outcome: "success",
          },
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
          pendingConfigVersion: "sha256:pending",
          restartRequired: true,
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
              eventType: "admin.restart.requested",
              actor: "admin",
              outcome: "success",
            },
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
          failures: [
            {
              taskId: 7,
              stage: "steam_lookup",
              target: "appid:440",
              provider: "steam",
              retryable: true,
              attempt: 2,
              reason: "Steam lookup timed out.",
              createdAt: "2026-06-08 03:05:00+00",
            },
          ],
        }),
      );
    vi.stubGlobal("fetch", fetchMock);

    render(<AdminApp />);

    fireEvent.change(await screen.findByLabelText("管理员令牌"), {
      target: { value: "admin-token" },
    });
    fireEvent.click(screen.getByRole("button", { name: "登录" }));

    expect(await screen.findByRole("heading", { name: "管理概览" })).toBeInTheDocument();
    expect(screen.getByText("MPGS Public")).toBeInTheDocument();
    expect(screen.getByText("公共库为空")).toBeInTheDocument();
    expect(screen.getByText("Postgres")).toBeInTheDocument();
    expect(screen.getByText("sha256:pending")).toBeInTheDocument();
    expect(screen.getByText("https://mpgs.example.test")).toBeInTheDocument();
    expect(screen.getByText("最近审计")).toBeInTheDocument();
    expect(screen.getAllByText("admin.restart.requested").length).toBeGreaterThan(0);
    expect(screen.getAllByText("admin").length).toBeGreaterThan(0);
    expect(screen.getAllByText("success").length).toBeGreaterThan(0);
    expect(screen.getByText("运维日志")).toBeInTheDocument();
    expect(screen.getByText("admin.session.login")).toBeInTheDocument();
    expect(screen.getByText("待审核游戏")).toBeInTheDocument();
    expect(screen.getByText("Team Fortress 2")).toBeInTheDocument();
    expect(screen.getByText(/AppID 440/)).toBeInTheDocument();
    expect(screen.getByText("任务控制")).toBeInTheDocument();
    expect(screen.getByText("任务 #7")).toBeInTheDocument();
    expect(screen.getByText("失败摘要")).toBeInTheDocument();
    expect(screen.getByText("Steam lookup timed out.")).toBeInTheDocument();

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith("/api/v1/admin/overview", {
        credentials: "same-origin",
        headers: { Accept: "application/json" },
      });
    });
  });

  it("copies the service address and downloads a keyless connection file", async () => {
    const connectionShare = {
      serviceName: "MPGS Public",
      serviceInstanceId: "018fb770-8998-7699-a6e4-b7b59f2f9c01",
      apiVersion: "v1",
      baseUrl: "https://mpgs.example.test",
      serviceInfoUrl: "https://mpgs.example.test/api/v1/service-info",
      capabilities: ["public_catalog_read"],
    };
    const clipboardWrite = vi.fn().mockResolvedValue(undefined);
    const downloadedBlobs: Blob[] = [];
    const createObjectUrl = vi.fn((blob: Blob) => {
      downloadedBlobs.push(blob);
      return "blob:mpgs-service-connection";
    });
    const revokeObjectUrl = vi.fn();
    const anchorClick = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(() => undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: clipboardWrite },
    });
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      value: createObjectUrl,
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      configurable: true,
      value: revokeObjectUrl,
    });

    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ configured: true }))
      .mockResolvedValueOnce(jsonResponse({ authenticated: true }))
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
          latestAuditEvent: null,
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
          pendingConfigVersion: null,
          restartRequired: false,
          lastStartupStatus: "ok",
        }),
      )
      .mockResolvedValueOnce(jsonResponse(connectionShare))
      .mockResolvedValueOnce(jsonResponse({ events: [] }))
      .mockResolvedValueOnce(jsonResponse({ items: [] }))
      .mockResolvedValueOnce(jsonResponse(emptyTasksResponse()));
    vi.stubGlobal("fetch", fetchMock);

    render(<AdminApp />);

    fireEvent.change(await screen.findByLabelText("管理员令牌"), {
      target: { value: "admin-token" },
    });
    fireEvent.click(screen.getByRole("button", { name: "登录" }));

    await screen.findByRole("heading", { name: "管理概览" });
    fireEvent.click(screen.getByRole("button", { name: "复制服务地址" }));

    await waitFor(() => {
      expect(clipboardWrite).toHaveBeenCalledWith("https://mpgs.example.test");
    });

    fireEvent.click(screen.getByRole("button", { name: "下载连接文件" }));

    expect(createObjectUrl).toHaveBeenCalledTimes(1);
    expect(anchorClick).toHaveBeenCalledTimes(1);
    const downloadedBlob = downloadedBlobs[0];
    if (!downloadedBlob) {
      throw new Error("connection file blob should be created");
    }
    const downloadedText = await downloadedBlob.text();
    const downloaded = JSON.parse(downloadedText);

    expect(downloaded).toEqual(connectionShare);
    expect(downloadedText).not.toMatch(
      /setupToken|adminToken|token|secret|steamApiKey|llmApiKey/i,
    );
    expect(revokeObjectUrl).toHaveBeenCalledWith("blob:mpgs-service-connection");
  });

  it("submits review actions from the admin review queue", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ configured: true }))
      .mockResolvedValueOnce(jsonResponse({ authenticated: true }))
      .mockResolvedValueOnce(
        jsonResponse({
          serviceName: "MPGS Public",
          publicCatalogStatus: "empty",
          publicGameCount: 0,
          pendingReviewCount: 1,
          latestTask: null,
          failureSummary: {
            openFailureCount: 0,
            retryableFailureCount: 0,
            latestFailure: null,
          },
          restartRequired: false,
          connectionShareConfigured: true,
          latestAuditEvent: null,
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
          pendingConfigVersion: null,
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
      .mockResolvedValueOnce(jsonResponse({ events: [] }))
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
              reviewNote: null,
            },
          ],
        }),
      )
      .mockResolvedValueOnce(jsonResponse(emptyTasksResponse()))
      .mockResolvedValueOnce(
        jsonResponse({
          game: {
            appid: 440,
            name: "Team Fortress 2",
            reviewStatus: "accepted",
            visibility: "public",
            recommendationScore: 86,
            updatedAt: "2026-06-08 04:02:00+00",
            reviewNote: null,
          },
        }),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          serviceName: "MPGS Public",
          publicCatalogStatus: "ready",
          publicGameCount: 1,
          pendingReviewCount: 0,
          latestTask: null,
          failureSummary: {
            openFailureCount: 0,
            retryableFailureCount: 0,
            latestFailure: null,
          },
          restartRequired: false,
          connectionShareConfigured: true,
          latestAuditEvent: {
            eventType: "admin.review.accept_public",
            actor: "admin",
            outcome: "success",
          },
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
          pendingConfigVersion: null,
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
      .mockResolvedValueOnce(jsonResponse({ events: [] }))
      .mockResolvedValueOnce(jsonResponse({ items: [] }))
      .mockResolvedValueOnce(jsonResponse(emptyTasksResponse()));
    vi.stubGlobal("fetch", fetchMock);

    render(<AdminApp />);

    fireEvent.change(await screen.findByLabelText("管理员令牌"), {
      target: { value: "admin-token" },
    });
    fireEvent.click(screen.getByRole("button", { name: "登录" }));

    expect(await screen.findByText("Team Fortress 2")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "公开" }));

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/api/v1/admin/review-queue/440/action",
        {
          body: JSON.stringify({ action: "accept_public" }),
          credentials: "same-origin",
          headers: {
            Accept: "application/json",
            "Content-Type": "application/json",
          },
          method: "POST",
        },
      );
    });
    expect(await screen.findByText("审核动作已提交。")).toBeInTheDocument();
  });

  it("queues manual AppID discovery tasks from the admin task controls", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ configured: true }))
      .mockResolvedValueOnce(jsonResponse({ authenticated: true }))
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
          latestAuditEvent: null,
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
          pendingConfigVersion: null,
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
      .mockResolvedValueOnce(jsonResponse({ events: [] }))
      .mockResolvedValueOnce(jsonResponse({ items: [] }))
      .mockResolvedValueOnce(jsonResponse(emptyTasksResponse()))
      .mockResolvedValueOnce(
        jsonResponse(
          {
            task: {
              id: 8,
              taskType: "manual_appid_discovery",
              status: "queued",
              target: "appid:730",
              targetAppid: 730,
              createdAt: "2026-06-08 04:00:00+00",
              updatedAt: "2026-06-08 04:00:00+00",
            },
          },
          201,
        ),
      )
      .mockResolvedValueOnce(
        jsonResponse({
          serviceName: "MPGS Public",
          publicCatalogStatus: "empty",
          publicGameCount: 0,
          pendingReviewCount: 0,
          latestTask: {
            id: 8,
            taskType: "manual_appid_discovery",
            status: "queued",
            target: "appid:730",
            targetAppid: 730,
            createdAt: "2026-06-08 04:00:00+00",
            updatedAt: "2026-06-08 04:00:00+00",
          },
          failureSummary: {
            openFailureCount: 0,
            retryableFailureCount: 0,
            latestFailure: null,
          },
          restartRequired: false,
          connectionShareConfigured: true,
          latestAuditEvent: {
            eventType: "admin.task.manual_appid_discovery.created",
            actor: "admin",
            outcome: "success",
          },
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
          pendingConfigVersion: null,
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
      .mockResolvedValueOnce(jsonResponse({ events: [] }))
      .mockResolvedValueOnce(jsonResponse({ items: [] }))
      .mockResolvedValueOnce(
        jsonResponse({
          recentTasks: [
            {
              id: 8,
              taskType: "manual_appid_discovery",
              status: "queued",
              target: "appid:730",
              targetAppid: 730,
              createdAt: "2026-06-08 04:00:00+00",
              updatedAt: "2026-06-08 04:00:00+00",
            },
          ],
          failureSummary: {
            openFailureCount: 0,
            retryableFailureCount: 0,
            latestFailure: null,
          },
          failures: [],
        }),
      );
    vi.stubGlobal("fetch", fetchMock);

    render(<AdminApp />);

    fireEvent.change(await screen.findByLabelText("管理员令牌"), {
      target: { value: "admin-token" },
    });
    fireEvent.click(screen.getByRole("button", { name: "登录" }));

    fireEvent.change(await screen.findByLabelText("手动 AppID"), {
      target: { value: "730" },
    });
    fireEvent.click(screen.getByRole("button", { name: "加入发现队列" }));

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith("/api/v1/admin/tasks", {
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
    });
    expect(await screen.findByText("手动 AppID 任务已入队。")).toBeInTheDocument();
    expect(screen.getByText("任务 #8")).toBeInTheDocument();
  });
});

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    headers: { "Content-Type": "application/json" },
    status,
  });
}

function emptyTasksResponse() {
  return {
    recentTasks: [],
    failureSummary: {
      openFailureCount: 0,
      retryableFailureCount: 0,
      latestFailure: null,
    },
    failures: [],
  };
}
