import type {
  AdminAuditEventsResponse,
  AdminDiagnosticsResponse,
  AdminOverviewResponse,
  AdminSessionResponse,
  ConfigStateResponse,
  RestartResponse,
  ServiceConnectionFileResponse,
  SetupCompleteRequest,
  SetupCompleteResponse,
  SetupStatusResponse,
} from "../api/generated/mpgsServerApi";

const jsonHeaders = {
  Accept: "application/json",
  "Content-Type": "application/json",
};

const readHeaders = {
  Accept: "application/json",
};

export async function getSetupStatus(): Promise<SetupStatusResponse> {
  return readJson("/api/v1/setup/status", {
    headers: readHeaders,
  });
}

export async function completeSetup(
  request: SetupCompleteRequest,
): Promise<SetupCompleteResponse> {
  return readJson("/api/v1/setup/complete", {
    body: JSON.stringify(request),
    headers: jsonHeaders,
    method: "POST",
  });
}

export async function loginAdmin(token: string): Promise<AdminSessionResponse> {
  return readJson("/api/v1/admin/session", {
    body: JSON.stringify({ token }),
    credentials: "same-origin",
    headers: jsonHeaders,
    method: "POST",
  });
}

export async function getAdminOverview(): Promise<AdminOverviewResponse> {
  return readAdminJson("/api/v1/admin/overview");
}

export async function getAdminDiagnostics(): Promise<AdminDiagnosticsResponse> {
  return readAdminJson("/api/v1/admin/diagnostics");
}

export async function getAdminConfigState(): Promise<ConfigStateResponse> {
  return readAdminJson("/api/v1/admin/config-state");
}

export async function getAdminConnectionShare(): Promise<ServiceConnectionFileResponse> {
  return readAdminJson("/api/v1/admin/connection-share");
}

export async function getAdminAuditEvents(): Promise<AdminAuditEventsResponse> {
  return readAdminJson("/api/v1/admin/audit-events");
}

export async function requestRestart(): Promise<RestartResponse> {
  return readJson("/api/v1/admin/restart", {
    body: JSON.stringify({ confirm: true }),
    credentials: "same-origin",
    headers: jsonHeaders,
    method: "POST",
  });
}

async function readAdminJson<T>(url: string): Promise<T> {
  return readJson(url, {
    credentials: "same-origin",
    headers: readHeaders,
  });
}

async function readJson<T>(url: string, init: RequestInit): Promise<T> {
  const response = await fetch(url, init);
  const payload = await response.json().catch(() => null);

  if (!response.ok) {
    const message =
      typeof payload?.error?.message === "string"
        ? payload.error.message
        : `请求失败：HTTP ${response.status}`;
    throw new Error(message);
  }

  return payload as T;
}
