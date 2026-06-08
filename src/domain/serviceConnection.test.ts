import { describe, expect, it } from "vitest";
import {
  buildDiscoveryHomeUrl,
  buildServiceInfoUrl,
  evaluateServiceAddressPolicy,
  evaluateServiceInfoCompatibility,
  normalizeServiceBaseUrl,
  validateServiceConnectionFileText,
  validateServiceAddress,
} from "./serviceConnection";
import type { ServiceInfo } from "../types";

const compatibleInfo: ServiceInfo = {
  serviceInstanceId: "018fb770-8998-7699-a6e4-b7b59f2f9c01",
  serviceName: "MPGS Test Service",
  serviceVersion: "0.1.0",
  apiVersion: "v1",
  publicCatalogStatus: "empty",
  capabilities: ["public_catalog_read"],
};

describe("service connection validation model", () => {
  it("normalizes a base URL and builds the service-info endpoint", () => {
    expect(normalizeServiceBaseUrl(" https://mpgs.example.test/// ")).toBe(
      "https://mpgs.example.test",
    );
    expect(buildServiceInfoUrl("https://mpgs.example.test/")).toBe(
      "https://mpgs.example.test/api/v1/service-info",
    );
    expect(buildDiscoveryHomeUrl("https://mpgs.example.test/")).toBe(
      "https://mpgs.example.test/api/v1/discovery-home",
    );
  });

  it("accepts compatible v1 service identity information", () => {
    expect(evaluateServiceInfoCompatibility(compatibleInfo)).toEqual({
      compatible: true,
      reason: "服务兼容。",
      info: compatibleInfo,
    });
  });

  it("rejects missing public read capability and incompatible API versions", () => {
    expect(
      evaluateServiceInfoCompatibility({
        ...compatibleInfo,
        apiVersion: "v2",
      }),
    ).toMatchObject({
      compatible: false,
      reason: "当前客户端只支持 MPGS API v1。",
    });

    expect(
      evaluateServiceInfoCompatibility({
        ...compatibleInfo,
        capabilities: [],
      }),
    ).toMatchObject({
      compatible: false,
      reason: "服务未声明公共库只读能力。",
    });
  });

  it("allows HTTPS and local HTTP but rejects public HTTP by default", () => {
    expect(evaluateServiceAddressPolicy("https://mpgs.example.test")).toMatchObject({
      allowed: true,
    });
    expect(evaluateServiceAddressPolicy("http://localhost:4310")).toMatchObject({
      allowed: true,
    });
    expect(evaluateServiceAddressPolicy("http://192.168.1.10:4310")).toMatchObject({
      allowed: false,
      reason: "公网服务地址必须使用 HTTPS。",
    });
    expect(
      evaluateServiceAddressPolicy("http://192.168.1.10:4310", {
        allowPrivateHttp: true,
      }),
    ).toMatchObject({
      allowed: true,
    });
  });

  it("fetches service-info and an anonymous public read endpoint before succeeding", async () => {
    const seenUrls: string[] = [];
    const result = await validateServiceAddress(
      "https://mpgs.example.test/",
      async (url) => {
        seenUrls.push(url);
        if (url.endsWith("/api/v1/discovery-home")) {
          return {
            ok: true,
            status: 200,
            json: async () => ({ status: "empty", totalGames: 0, sections: {} }),
          };
        }

        return {
          ok: true,
          status: 200,
          json: async () => compatibleInfo,
        };
      },
    );

    expect(seenUrls).toEqual([
      "https://mpgs.example.test/api/v1/service-info",
      "https://mpgs.example.test/api/v1/discovery-home",
    ]);
    expect(result).toMatchObject({
      success: true,
      message: "服务地址验证通过。",
      baseUrl: "https://mpgs.example.test",
    });
  });

  it("rejects a service when the anonymous public read probe fails", async () => {
    const result = await validateServiceAddress(
      "https://mpgs.example.test/",
      async (url) => {
        if (url.endsWith("/api/v1/discovery-home")) {
          return {
            ok: false,
            status: 503,
            json: async () => ({}),
          };
        }

        return {
          ok: true,
          status: 200,
          json: async () => compatibleInfo,
        };
      },
    );

    expect(result).toMatchObject({
      success: false,
      message: "匿名公共读取验证失败：HTTP 503。",
      baseUrl: "https://mpgs.example.test",
      serviceInfoUrl: "https://mpgs.example.test/api/v1/service-info",
      publicReadProbeUrl: "https://mpgs.example.test/api/v1/discovery-home",
    });
  });

  it("validates an imported connection file against live service identity and public reads", async () => {
    const seenUrls: string[] = [];
    const result = await validateServiceConnectionFileText(
      JSON.stringify({
        serviceName: "MPGS Test Service",
        serviceInstanceId: "018fb770-8998-7699-a6e4-b7b59f2f9c01",
        apiVersion: "v1",
        baseUrl: "https://mpgs.example.test",
        serviceInfoUrl: "https://mpgs.example.test/api/v1/service-info",
        capabilities: ["public_catalog_read"],
      }),
      async (url) => {
        seenUrls.push(url);
        if (url.endsWith("/api/v1/discovery-home")) {
          return {
            ok: true,
            status: 200,
            json: async () => ({ status: "empty", totalGames: 0, sections: {} }),
          };
        }

        return {
          ok: true,
          status: 200,
          json: async () => compatibleInfo,
        };
      },
    );

    expect(seenUrls).toEqual([
      "https://mpgs.example.test/api/v1/service-info",
      "https://mpgs.example.test/api/v1/discovery-home",
    ]);
    expect(result).toMatchObject({
      success: true,
      baseUrl: "https://mpgs.example.test",
      serviceInfoUrl: "https://mpgs.example.test/api/v1/service-info",
      info: compatibleInfo,
    });
  });

  it("rejects imported connection files with secret-looking fields", async () => {
    const result = await validateServiceConnectionFileText(
      JSON.stringify({
        serviceName: "MPGS Test Service",
        serviceInstanceId: "018fb770-8998-7699-a6e4-b7b59f2f9c01",
        apiVersion: "v1",
        baseUrl: "https://mpgs.example.test",
        serviceInfoUrl: "https://mpgs.example.test/api/v1/service-info",
        capabilities: ["public_catalog_read"],
        adminToken: "must-not-import",
      }),
      async () => {
        throw new Error("secret-bearing connection files must not be fetched");
      },
    );

    expect(result).toMatchObject({
      success: false,
      message: "服务连接文件不能包含密钥或管理凭据。",
    });
  });

  it("rejects imported connection files when service identity does not match", async () => {
    const result = await validateServiceConnectionFileText(
      JSON.stringify({
        serviceName: "MPGS Test Service",
        serviceInstanceId: "018fb770-8998-7699-a6e4-b7b59f2f9c01",
        apiVersion: "v1",
        baseUrl: "https://mpgs.example.test",
        serviceInfoUrl: "https://mpgs.example.test/api/v1/service-info",
        capabilities: ["public_catalog_read"],
      }),
      async (url) => {
        if (url.endsWith("/api/v1/discovery-home")) {
          return {
            ok: true,
            status: 200,
            json: async () => ({ status: "empty", totalGames: 0, sections: {} }),
          };
        }

        return {
          ok: true,
          status: 200,
          json: async () => ({
            ...compatibleInfo,
            serviceInstanceId: "018fb770-8998-7699-a6e4-b7b59f2f9c02",
          }),
        };
      },
    );

    expect(result).toMatchObject({
      success: false,
      message: "服务身份与连接文件不一致。",
      baseUrl: "https://mpgs.example.test",
      serviceInfoUrl: "https://mpgs.example.test/api/v1/service-info",
    });
  });
});
