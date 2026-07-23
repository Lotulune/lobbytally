import { describe, expect, it, vi } from "vitest";
import {
  checkServiceConnection,
  parseDiscovery,
  type ServiceDiscovery,
} from "../src/api/discovery";
import { jsonResponse, makeFetchStub } from "./helpers";

const ORIGIN = "https://mpgs.example.com";

const DISCOVERY: ServiceDiscovery = {
  service: "mpgs-server",
  discovery_version: 1,
  service_version: "0.1.0",
  api_version: "v1",
  api_base_path: "/v1",
  readiness_path: "/health/ready",
  openapi_path: "/openapi.json",
  authentication: ["anonymous", "account"],
};

function healthyRoutes(overrides: Record<string, unknown> = {}) {
  return {
    "GET /.well-known/mpgs": () => jsonResponse(DISCOVERY),
    "GET /health/ready": () => jsonResponse({ status: "ready" }),
    "GET /v1/meta": () => jsonResponse({ api_version: "v1" }),
    ...overrides,
  } as Parameters<typeof makeFetchStub>[0];
}

describe("checkServiceConnection", () => {
  it("succeeds with the documented three-step handshake", async () => {
    const { fetchFn, calls } = makeFetchStub(healthyRoutes());
    const onStep = vi.fn();
    const result = await checkServiceConnection(ORIGIN, { fetchFn, onStep });

    expect(result.ok).toBe(true);
    if (result.ok) expect(result.discovery.service).toBe("mpgs-server");
    expect(calls.map((c) => c.url)).toEqual([
      `${ORIGIN}/.well-known/mpgs`,
      `${ORIGIN}/health/ready`,
      `${ORIGIN}/v1/meta`,
    ]);
    // every request stays on the verified origin
    expect(calls.every((c) => c.url.startsWith(ORIGIN))).toBe(true);
    expect(onStep.mock.calls.map(([s]) => s)).toEqual(["discovery", "readiness", "meta"]);
  });

  it("maps a 404 discovery endpoint to not_mpgs", async () => {
    const { fetchFn } = makeFetchStub({});
    const result = await checkServiceConnection(ORIGIN, { fetchFn });
    expect(result).toMatchObject({ ok: false, kind: "not_mpgs" });
  });

  it("refuses discovery redirects instead of following them", async () => {
    const { fetchFn } = makeFetchStub({
      "GET /.well-known/mpgs": () =>
        new Response(null, { status: 302, headers: { location: "https://evil.example/.well-known/mpgs" } }),
    });
    const result = await checkServiceConnection(ORIGIN, { fetchFn });
    expect(result).toMatchObject({ ok: false, kind: "not_mpgs" });
  });

  it("rejects scheme-relative readiness paths in discovery documents", () => {
    expect(
      parseDiscovery({
        ...DISCOVERY,
        readiness_path: "//evil.example/ready",
      }),
    ).toBeNull();
  });

  it("maps a non-JSON discovery response to not_mpgs (SPA fallback must not pass)", async () => {
    const { fetchFn } = makeFetchStub({
      "GET /.well-known/mpgs": () =>
        new Response("<html>index</html>", {
          status: 200,
          headers: { "content-type": "text/html" },
        }),
    });
    const result = await checkServiceConnection(ORIGIN, { fetchFn });
    expect(result).toMatchObject({ ok: false, kind: "not_mpgs" });
  });

  it("maps a foreign service document to not_mpgs", async () => {
    const { fetchFn } = makeFetchStub({
      "GET /.well-known/mpgs": () => jsonResponse({ service: "wordpress" }),
    });
    const result = await checkServiceConnection(ORIGIN, { fetchFn });
    expect(result).toMatchObject({ ok: false, kind: "not_mpgs" });
  });

  it("maps unsupported protocol versions to incompatible", async () => {
    const { fetchFn } = makeFetchStub({
      "GET /.well-known/mpgs": () =>
        jsonResponse({ ...DISCOVERY, discovery_version: 2 }),
    });
    const result = await checkServiceConnection(ORIGIN, { fetchFn });
    expect(result).toMatchObject({ ok: false, kind: "incompatible" });

    const { fetchFn: fetchFn2 } = makeFetchStub({
      "GET /.well-known/mpgs": () => jsonResponse({ ...DISCOVERY, api_version: "v2" }),
    });
    const result2 = await checkServiceConnection(ORIGIN, { fetchFn: fetchFn2 });
    expect(result2).toMatchObject({ ok: false, kind: "incompatible" });
  });

  it("maps a readiness 503 to not_ready, never to an address error", async () => {
    const { fetchFn } = makeFetchStub(
      healthyRoutes({
        "GET /health/ready": () => jsonResponse({ status: "not_ready" }, { status: 503 }),
      }),
    );
    const result = await checkServiceConnection(ORIGIN, { fetchFn });
    expect(result).toMatchObject({ ok: false, kind: "not_ready" });
  });

  it("maps request timeouts to timeout", async () => {
    const fetchFn = (() => {
      throw new DOMException("The operation was aborted.", "AbortError");
    }) as unknown as typeof fetch;
    const result = await checkServiceConnection(ORIGIN, { fetchFn });
    expect(result).toMatchObject({ ok: false, kind: "timeout" });
  });

  it("maps TLS handshake failures to tls_error", async () => {
    const fetchFn = (() => {
      throw new TypeError("certificate verify failed: self signed certificate");
    }) as unknown as typeof fetch;
    const result = await checkServiceConnection(ORIGIN, { fetchFn });
    expect(result).toMatchObject({ ok: false, kind: "tls_error" });
  });

  it("maps other connectivity failures to network", async () => {
    const fetchFn = (() => {
      throw new TypeError("fetch failed");
    }) as unknown as typeof fetch;
    const result = await checkServiceConnection(ORIGIN, { fetchFn });
    expect(result).toMatchObject({ ok: false, kind: "network" });
  });

  it("reports step progress for failures deep in the handshake", async () => {
    const { fetchFn } = makeFetchStub(
      healthyRoutes({
        "GET /v1/meta": () =>
          jsonResponse({ error: { code: "internal" } }, { status: 500 }),
      }),
    );
    const onStep = vi.fn();
    const result = await checkServiceConnection(ORIGIN, { fetchFn, onStep });
    expect(result.ok).toBe(false);
    expect(onStep.mock.calls.map(([s]) => s)).toEqual(["discovery", "readiness", "meta"]);
  });
});

describe("parseDiscovery", () => {
  it("ignores unknown fields per the contract", () => {
    const parsed = parseDiscovery({ ...DISCOVERY, future_field: { nested: true } });
    expect(parsed?.service).toBe("mpgs-server");
  });

  it("rejects documents with relative paths missing the leading slash", () => {
    expect(parseDiscovery({ ...DISCOVERY, readiness_path: "health/ready" })).toBeNull();
    expect(parseDiscovery({ ...DISCOVERY, api_base_path: "v1" })).toBeNull();
  });
});
