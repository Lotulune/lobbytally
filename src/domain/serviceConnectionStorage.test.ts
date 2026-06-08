import { beforeEach, describe, expect, it } from "vitest";
import type { ServiceInfo } from "../types";
import {
  clearCurrentServiceConnection,
  getCurrentServiceConnection,
  saveCurrentServiceConnection,
} from "./serviceConnectionStorage";

const compatibleInfo: ServiceInfo = {
  serviceInstanceId: "018fb770-8998-7699-a6e4-b7b59f2f9c01",
  serviceName: "MPGS Test Service",
  serviceVersion: "0.1.0",
  apiVersion: "v1",
  publicCatalogStatus: "ready",
  capabilities: ["public_catalog_read"],
};

describe("service connection storage", () => {
  beforeEach(() => {
    clearCurrentServiceConnection();
  });

  it("stores the single current service connection with a normalized base URL", () => {
    saveCurrentServiceConnection({
      baseUrl: " https://mpgs.example.test/// ",
      info: compatibleInfo,
      validatedAt: "2026-06-08T00:00:00.000Z",
    });

    expect(getCurrentServiceConnection()).toEqual({
      baseUrl: "https://mpgs.example.test",
      info: compatibleInfo,
      validatedAt: "2026-06-08T00:00:00.000Z",
    });
  });

  it("ignores incompatible stored data instead of returning a partial connection", () => {
    localStorage.setItem(
      "mpgs.currentServiceConnection.v1",
      JSON.stringify({
        baseUrl: "https://mpgs.example.test",
        info: { apiVersion: "v2" },
        validatedAt: "2026-06-08T00:00:00.000Z",
      }),
    );

    expect(getCurrentServiceConnection()).toBeNull();
  });
});
