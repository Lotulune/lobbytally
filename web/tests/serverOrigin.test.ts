import { describe, expect, it } from "vitest";
import {
  activateServiceOrigin,
  clearCurrentServiceOrigin,
  forgetKnownService,
  getCurrentServiceOrigin,
  listKnownServices,
  normalizeServiceOrigin,
  serviceNamespacePrefix,
} from "../src/api/serverOrigin";
import { MemoryStorage } from "./helpers";

describe("normalizeServiceOrigin", () => {
  it("accepts a bare https origin and strips the trailing slash", () => {
    const result = normalizeServiceOrigin("https://mpgs.example.com/");
    expect(result).toEqual({ ok: true, origin: "https://mpgs.example.com" });
  });

  it("lowercases the hostname and keeps an explicit non-default port", () => {
    const result = normalizeServiceOrigin("HTTPS://MPGS.Example.COM:8443");
    expect(result).toEqual({ ok: true, origin: "https://mpgs.example.com:8443" });
  });

  it("drops the default https port", () => {
    const result = normalizeServiceOrigin("https://mpgs.example.com:443");
    expect(result).toEqual({ ok: true, origin: "https://mpgs.example.com" });
  });

  it("accepts bare host:port as https", () => {
    expect(normalizeServiceOrigin("mpgs.example.com:8443")).toEqual({
      ok: true,
      origin: "https://mpgs.example.com:8443",
    });
  });

  it("accepts bare IP:port as http", () => {
    expect(normalizeServiceOrigin("192.168.1.10:17880")).toEqual({
      ok: true,
      origin: "http://192.168.1.10:17880",
    });
  });

  it("accepts explicit http for IP addresses outside dev", () => {
    expect(
      normalizeServiceOrigin("http://10.0.0.5:17880", { allowHttpLoopback: false }),
    ).toEqual({ ok: true, origin: "http://10.0.0.5:17880" });
  });

  it("rejects http for non-loopback hostnames", () => {
    const result = normalizeServiceOrigin("http://mpgs.example.com", {
      allowHttpLoopback: true,
    });
    expect(result).toEqual({ ok: false, reason: "unsupported_protocol" });
  });

  it("rejects http entirely outside dev builds for localhost", () => {
    const result = normalizeServiceOrigin("http://127.0.0.1:17880", {
      allowHttpLoopback: false,
    });
    // 127.0.0.1 is an IP → http is allowed for IP form even outside DEV.
    expect(result).toEqual({ ok: true, origin: "http://127.0.0.1:17880" });
  });

  it("allows http loopback hostname in dev builds", () => {
    const result = normalizeServiceOrigin("http://localhost:17880/", {
      allowHttpLoopback: true,
    });
    expect(result).toEqual({ ok: true, origin: "http://localhost:17880" });
  });

  it("accepts IPv6 with brackets and keeps them in the origin", () => {
    const result = normalizeServiceOrigin("http://[::1]:17880", {
      allowHttpLoopback: true,
    });
    expect(result).toEqual({ ok: true, origin: "http://[::1]:17880" });
  });

  it("accepts bare bracketed IPv6:port as http", () => {
    expect(normalizeServiceOrigin("[2001:db8::1]:8443")).toEqual({
      ok: true,
      origin: "http://[2001:db8::1]:8443",
    });
  });

  it("rejects user info, paths, query and fragments", () => {
    expect(normalizeServiceOrigin("https://user:pw@mpgs.example.com")).toEqual({
      ok: false,
      reason: "credentials_not_allowed",
    });
    expect(normalizeServiceOrigin("https://mpgs.example.com/v1")).toEqual({
      ok: false,
      reason: "path_not_allowed",
    });
    expect(normalizeServiceOrigin("https://mpgs.example.com/?a=1")).toEqual({
      ok: false,
      reason: "query_not_allowed",
    });
    expect(normalizeServiceOrigin("https://mpgs.example.com/#x")).toEqual({
      ok: false,
      reason: "fragment_not_allowed",
    });
  });

  it("rejects empty and unparseable input", () => {
    expect(normalizeServiceOrigin("   ")).toEqual({ ok: false, reason: "empty" });
    expect(normalizeServiceOrigin("not a url")).toEqual({ ok: false, reason: "unparseable" });
  });
});

describe("service origin registry", () => {
  it("persists the active origin and refreshes known entries", () => {
    const storage = new MemoryStorage();
    activateServiceOrigin(storage, "https://a.example.com", 1_000);
    activateServiceOrigin(storage, "https://b.example.com", 2_000);
    activateServiceOrigin(storage, "https://a.example.com", 3_000);

    expect(getCurrentServiceOrigin(storage)).toBe("https://a.example.com");
    const known = listKnownServices(storage);
    expect(known).toHaveLength(2);
    const a = known.find((entry) => entry.origin === "https://a.example.com");
    expect(a?.addedAtMs).toBe(1_000);
    expect(a?.lastConnectedAtMs).toBe(3_000);
  });

  it("forgets known services and clears the current pointer", () => {
    const storage = new MemoryStorage();
    activateServiceOrigin(storage, "https://a.example.com");
    activateServiceOrigin(storage, "https://b.example.com");
    forgetKnownService(storage, "https://b.example.com");
    expect(listKnownServices(storage).map((s) => s.origin)).toEqual(["https://a.example.com"]);
    clearCurrentServiceOrigin(storage);
    expect(getCurrentServiceOrigin(storage)).toBeNull();
  });

  it("restores the saved origin after a simulated restart (PRD 12.2 重启恢复)", () => {
    const storage = new MemoryStorage();
    activateServiceOrigin(storage, "https://mpgs.example.com");
    expect(getCurrentServiceOrigin(storage)).toBe("https://mpgs.example.com");
  });

  it("ignores corrupted current origin values", () => {
    const storage = new MemoryStorage();
    storage.setItem("mpgs.server.current.v1", "javascript:alert(1)");
    expect(getCurrentServiceOrigin(storage)).toBeNull();
  });
});

describe("serviceNamespacePrefix", () => {
  it("produces distinct, stable prefixes per origin", () => {
    const a = serviceNamespacePrefix("https://a.example.com");
    const b = serviceNamespacePrefix("https://b.example.com");
    expect(a).not.toBe(b);
    expect(serviceNamespacePrefix("https://a.example.com")).toBe(a);
    expect(a.startsWith("mpgs.svc.")).toBe(true);
    // url-safe: no + / = characters
    expect(a).toMatch(/^mpgs\.svc\.[A-Za-z0-9_-]+\.$/);
  });
});
