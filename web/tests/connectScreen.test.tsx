import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { jsonResponse, makeFetchStub } from "./helpers";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const DISCOVERY = {
  service: "mpgs-server",
  discovery_version: 1,
  service_version: "0.1.0",
  api_version: "v1",
  api_base_path: "/v1",
  readiness_path: "/health/ready",
  openapi_path: "/openapi.json",
  authentication: ["anonymous", "account"],
};

import { ConnectScreen } from "../src/screens/ConnectScreen";

function mount(onConnected: (origin: string) => void) {
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  act(() => root.render(<ConnectScreen onConnected={onConnected} />));
  return { host, root };
}

function input(host: HTMLElement): HTMLInputElement {
  const el = host.querySelector<HTMLInputElement>("#service-address");
  if (!el) throw new Error("missing service address input");
  return el;
}

function connectButton(host: HTMLElement): HTMLButtonElement {
  const btn = Array.from(host.querySelectorAll("button")).find(
    (b) => b.textContent?.trim() === "连接",
  );
  if (!btn) throw new Error("missing connect button");
  return btn as HTMLButtonElement;
}

function setInputValue(el: HTMLInputElement, value: string) {
  // React controlled inputs need the native setter for the change to register.
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  setter.call(el, value);
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

describe("ConnectScreen (PRD §5.1 first connect)", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("starts with an empty address and only shows examples as placeholder", () => {
    const { host } = mount(() => undefined);
    const el = input(host);
    expect(el.value).toBe("");
    expect(el.placeholder).toMatch(/example\.com|192\.168/);
  });

  it("blocks an invalid address in place without issuing any request", async () => {
    const fetchSpy = vi.fn();
    vi.stubGlobal("fetch", fetchSpy);
    const onConnected = vi.fn();
    const { host } = mount(onConnected);

    act(() => setInputValue(input(host), "https://user:pw@mpgs.example.com/path"));
    await act(async () => {
      connectButton(host).click();
    });

    expect(fetchSpy).not.toHaveBeenCalled();
    expect(onConnected).not.toHaveBeenCalled();
    expect(host.querySelector("[role='alert']")?.textContent).toContain("不允许");
  });

  it("persists nothing and reports not_mpgs for a plain website", async () => {
    const { fetchFn } = makeFetchStub({
      "GET /.well-known/mpgs": () =>
        new Response("<html></html>", { status: 200, headers: { "content-type": "text/html" } }),
    });
    vi.stubGlobal("fetch", fetchFn);
    const onConnected = vi.fn();
    const { host } = mount(onConnected);

    act(() => setInputValue(input(host), "https://mpgs.example.com"));
    await act(async () => {
      connectButton(host).click();
    });

    expect(onConnected).not.toHaveBeenCalled();
    expect(host.querySelector("[role='alert']")?.textContent).toContain("不是 MPGS Server");
  });

  it("shows the maintenance copy when readiness returns 503", async () => {
    const { fetchFn } = makeFetchStub({
      "GET /.well-known/mpgs": () => jsonResponse(DISCOVERY),
      "GET /health/ready": () => jsonResponse({ status: "not_ready" }, { status: 503 }),
    });
    vi.stubGlobal("fetch", fetchFn);
    const { host } = mount(() => undefined);

    act(() => setInputValue(input(host), "https://mpgs.example.com"));
    await act(async () => {
      connectButton(host).click();
    });

    expect(host.querySelector("[role='alert']")?.textContent).toContain("服务维护中");
  });

  it("completes the full handshake and reports the normalized origin", async () => {
    const { fetchFn, calls } = makeFetchStub({
      "GET /.well-known/mpgs": () => jsonResponse(DISCOVERY),
      "GET /health/ready": () => jsonResponse({ status: "ready" }),
      "GET /v1/meta": () => jsonResponse({ api_version: "v1" }),
    });
    vi.stubGlobal("fetch", fetchFn);
    const onConnected = vi.fn();
    const { host } = mount(onConnected);

    act(() => setInputValue(input(host), "https://MPGS.example.com:443/"));
    await act(async () => {
      connectButton(host).click();
    });

    expect(onConnected).toHaveBeenCalledTimes(1);
    expect(onConnected).toHaveBeenCalledWith("https://mpgs.example.com");
    expect(calls.map((c) => c.url)).toEqual([
      "https://mpgs.example.com/.well-known/mpgs",
      "https://mpgs.example.com/health/ready",
      "https://mpgs.example.com/v1/meta",
    ]);
    // A failed-then-succeeded connect clears the alert.
    expect(host.querySelector("[role='alert']")).toBeNull();
  });

  it("accepts bare IP:port and connects over http", async () => {
    const { fetchFn, calls } = makeFetchStub({
      "GET /.well-known/mpgs": () => jsonResponse(DISCOVERY),
      "GET /health/ready": () => jsonResponse({ status: "ready" }),
      "GET /v1/meta": () => jsonResponse({ api_version: "v1" }),
    });
    vi.stubGlobal("fetch", fetchFn);
    const onConnected = vi.fn();
    const { host } = mount(onConnected);

    act(() => setInputValue(input(host), "192.168.1.10:17880"));
    await act(async () => {
      connectButton(host).click();
    });

    expect(onConnected).toHaveBeenCalledWith("http://192.168.1.10:17880");
    expect(calls[0]?.url).toBe("http://192.168.1.10:17880/.well-known/mpgs");
  });
});
