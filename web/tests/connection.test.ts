import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { activateServiceOrigin } from "../src/api/serverOrigin";
import { getConnectionManager, resetConnectionManagerForTests } from "../src/app/connection";
import { jsonResponse, makeFetchStub } from "./helpers";

const ORIGIN = "https://mpgs.example.com";

const DISCOVERY = {
  service: "mpgs-server",
  discovery_version: 1,
  service_version: "0.1.0",
  api_version: "v1",
  api_base_path: "/v1",
  readiness_path: "/health/ready",
};

describe("ConnectionManager (PRD §5.2 daily startup / offline)", () => {
  beforeEach(() => {
    localStorage.clear();
    resetConnectionManagerForTests();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reports connected after a healthy recheck and keeps the origin", async () => {
    const { fetchFn } = makeFetchStub({
      "GET /.well-known/mpgs": () => jsonResponse(DISCOVERY),
      "GET /health/ready": () => jsonResponse({ status: "ready" }),
      "GET /v1/meta": () => jsonResponse({ api_version: "v1" }),
    });
    vi.stubGlobal("fetch", fetchFn);
    activateServiceOrigin(localStorage, ORIGIN);

    const manager = getConnectionManager();
    const status = await manager.recheck();

    expect(status).toBe("connected");
    expect(manager.get()).toMatchObject({ origin: ORIGIN, status: "connected", lastError: null });
  });

  it("enters offline state on network failure without dropping the saved origin", async () => {
    vi.stubGlobal("fetch", () => Promise.reject(new TypeError("fetch failed")));
    activateServiceOrigin(localStorage, ORIGIN);

    const manager = getConnectionManager();
    const status = await manager.recheck();

    expect(status).toBe("offline");
    // 已有配置保留：可以离线进入该服务的缓存（PRD §5.2）。
    expect(manager.get().origin).toBe(ORIGIN);
    expect(localStorage.getItem("mpgs.server.current.v1")).toBe(ORIGIN);
  });

  it("distinguishes maintenance (503) from an address error", async () => {
    const { fetchFn } = makeFetchStub({
      "GET /.well-known/mpgs": () => jsonResponse(DISCOVERY),
      "GET /health/ready": () => jsonResponse({}, { status: 503 }),
    });
    vi.stubGlobal("fetch", fetchFn);
    activateServiceOrigin(localStorage, ORIGIN);

    const manager = getConnectionManager();
    const status = await manager.recheck();

    expect(status).toBe("maintenance");
    expect(manager.get().lastError).toBe("not_ready");
  });

  it("deleteServiceData clears the namespace and the current pointer", async () => {
    activateServiceOrigin(localStorage, ORIGIN);
    const manager = getConnectionManager();
    manager.scopedStorageFor(ORIGIN).setItem("mpgs.session.v1", "secret");

    const { removedKeys, wasCurrent } = manager.deleteServiceData(ORIGIN);

    expect(wasCurrent).toBe(true);
    expect(removedKeys).toBe(1);
    expect(manager.get().origin).toBeNull();
    expect(localStorage.getItem("mpgs.server.current.v1")).toBeNull();
    expect(manager.knownServices()).toEqual([]);
  });
});
