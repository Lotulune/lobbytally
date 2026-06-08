import type {
  PublicCatalogStatus,
  ServiceAddressPolicyResult,
  ServiceAddressValidationResult,
  ServiceCapability,
  ServiceInfo,
  ServiceInfoCompatibilityResult,
} from "../types";

const SUPPORTED_API_VERSION = "v1";
const PUBLIC_READ_CAPABILITY = "public_catalog_read";
const SECRET_CONNECTION_FILE_FIELDS = new Set([
  "setupToken",
  "adminToken",
  "token",
  "secret",
  "steamApiKey",
  "llmApiKey",
  "apiKey",
]);

export interface ServiceAddressPolicyOptions {
  allowPrivateHttp?: boolean;
}

export interface ServiceInfoFetchResponse {
  ok: boolean;
  status: number;
  json: () => Promise<unknown>;
}

export type ServiceInfoFetch = (
  url: string,
  init?: { method?: string },
) => Promise<ServiceInfoFetchResponse>;

export function normalizeServiceBaseUrl(value: string): string {
  const trimmed = value.trim();
  if (!trimmed) {
    throw new Error("请输入服务地址。");
  }

  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    throw new Error("服务地址必须是完整的 URL。");
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new Error("服务地址只支持 HTTP 或 HTTPS。");
  }

  url.pathname = url.pathname.replace(/\/+$/, "");
  url.search = "";
  url.hash = "";

  return url.toString().replace(/\/+$/, "");
}

export function buildServiceInfoUrl(baseUrl: string): string {
  return `${normalizeServiceBaseUrl(baseUrl)}/api/v1/service-info`;
}

export function buildDiscoveryHomeUrl(baseUrl: string): string {
  return `${normalizeServiceBaseUrl(baseUrl)}/api/v1/discovery-home`;
}

export function evaluateServiceAddressPolicy(
  baseUrl: string,
  options: ServiceAddressPolicyOptions = {},
): ServiceAddressPolicyResult {
  let normalizedBaseUrl: string;
  try {
    normalizedBaseUrl = normalizeServiceBaseUrl(baseUrl);
  } catch (error) {
    return {
      allowed: false,
      reason: error instanceof Error ? error.message : String(error),
    };
  }

  const url = new URL(normalizedBaseUrl);
  if (url.protocol === "https:") {
    return {
      allowed: true,
      reason: "服务地址协议可用。",
      normalizedBaseUrl,
    };
  }

  if (isLocalhost(url.hostname)) {
    return {
      allowed: true,
      reason: "本机 HTTP 地址允许用于开发验证。",
      normalizedBaseUrl,
    };
  }

  if (options.allowPrivateHttp && isPrivateIpv4(url.hostname)) {
    return {
      allowed: true,
      reason: "已允许局域网 HTTP 服务地址。",
      normalizedBaseUrl,
    };
  }

  return {
    allowed: false,
    reason: "公网服务地址必须使用 HTTPS。",
    normalizedBaseUrl,
  };
}

export function evaluateServiceInfoCompatibility(
  info: ServiceInfo,
): ServiceInfoCompatibilityResult {
  if (info.apiVersion !== SUPPORTED_API_VERSION) {
    return {
      compatible: false,
      reason: "当前客户端只支持 MPGS API v1。",
      info,
    };
  }

  if (!info.capabilities.includes(PUBLIC_READ_CAPABILITY)) {
    return {
      compatible: false,
      reason: "服务未声明公共库只读能力。",
      info,
    };
  }

  return {
    compatible: true,
    reason: "服务兼容。",
    info,
  };
}

export async function validateServiceAddress(
  baseUrl: string,
  fetcher: ServiceInfoFetch = fetchServiceInfo,
  options: ServiceAddressPolicyOptions = {},
): Promise<ServiceAddressValidationResult> {
  const policy = evaluateServiceAddressPolicy(baseUrl, options);
  if (!policy.allowed || !policy.normalizedBaseUrl) {
    return {
      success: false,
      message: policy.reason,
      baseUrl: policy.normalizedBaseUrl,
    };
  }

  const serviceInfoUrl = buildServiceInfoUrl(policy.normalizedBaseUrl);
  const publicReadProbeUrl = buildDiscoveryHomeUrl(policy.normalizedBaseUrl);
  try {
    const response = await fetcher(serviceInfoUrl, { method: "GET" });
    if (!response.ok) {
      return {
        success: false,
        message: `服务身份信息读取失败：HTTP ${response.status}。`,
        baseUrl: policy.normalizedBaseUrl,
        serviceInfoUrl,
      };
    }

    const payload = await response.json();
    if (!isServiceInfo(payload)) {
      return {
        success: false,
        message: "服务身份信息格式不兼容。",
        baseUrl: policy.normalizedBaseUrl,
        serviceInfoUrl,
        diagnostic: "缺少 MPGS service-info 所需字段。",
      };
    }

    const compatibility = evaluateServiceInfoCompatibility(payload);
    if (!compatibility.compatible) {
      return {
        success: false,
        message: compatibility.reason,
        baseUrl: policy.normalizedBaseUrl,
        serviceInfoUrl,
        info: payload,
      };
    }

    const publicReadProbe = await fetcher(publicReadProbeUrl, { method: "GET" });
    if (!publicReadProbe.ok) {
      return {
        success: false,
        message: `匿名公共读取验证失败：HTTP ${publicReadProbe.status}。`,
        baseUrl: policy.normalizedBaseUrl,
        serviceInfoUrl,
        publicReadProbeUrl,
        info: payload,
      };
    }

    return {
      success: true,
      message: "服务地址验证通过。",
      baseUrl: policy.normalizedBaseUrl,
      serviceInfoUrl,
      publicReadProbeUrl,
      info: payload,
    };
  } catch (error) {
    return {
      success: false,
      message: "无法连接公共发现服务。",
      baseUrl: policy.normalizedBaseUrl,
      serviceInfoUrl,
      diagnostic: error instanceof Error ? error.message : String(error),
    };
  }
}

export async function validateServiceConnectionFileText(
  fileText: string,
  fetcher: ServiceInfoFetch = fetchServiceInfo,
  options: ServiceAddressPolicyOptions = {},
): Promise<ServiceAddressValidationResult> {
  let payload: unknown;
  try {
    payload = JSON.parse(fileText);
  } catch {
    return {
      success: false,
      message: "服务连接文件不是有效 JSON。",
    };
  }

  if (containsSecretConnectionField(payload)) {
    return {
      success: false,
      message: "服务连接文件不能包含密钥或管理凭据。",
    };
  }

  if (!isServiceConnectionFile(payload)) {
    return {
      success: false,
      message: "服务连接文件格式不兼容。",
    };
  }

  const expectedServiceInfoUrl = buildServiceInfoUrl(payload.baseUrl);
  const declaredServiceBaseUrl = normalizeServiceBaseUrl(
    payload.serviceInfoUrl.replace(/\/api\/v1\/service-info$/, ""),
  );
  const normalizedBaseUrl = normalizeServiceBaseUrl(payload.baseUrl);
  if (
    normalizedBaseUrl !== declaredServiceBaseUrl ||
    payload.serviceInfoUrl !== expectedServiceInfoUrl
  ) {
    return {
      success: false,
      message: "服务连接文件中的服务身份地址不一致。",
      baseUrl: normalizedBaseUrl,
      serviceInfoUrl: expectedServiceInfoUrl,
    };
  }

  const result = await validateServiceAddress(payload.baseUrl, fetcher, options);
  if (!result.success) {
    return result;
  }

  if (
    result.info.serviceInstanceId !== payload.serviceInstanceId ||
    result.info.apiVersion !== payload.apiVersion ||
    result.info.serviceName !== payload.serviceName ||
    !payload.capabilities.every((capability) =>
      result.info.capabilities.includes(capability),
    )
  ) {
    return {
      success: false,
      message: "服务身份与连接文件不一致。",
      baseUrl: result.baseUrl,
      serviceInfoUrl: result.serviceInfoUrl,
      publicReadProbeUrl: result.publicReadProbeUrl,
      info: result.info,
    };
  }

  return result;
}

async function fetchServiceInfo(
  url: string,
  init?: { method?: string },
): Promise<ServiceInfoFetchResponse> {
  const response = await fetch(url, { method: init?.method ?? "GET" });

  return {
    ok: response.ok,
    status: response.status,
    json: () => response.json(),
  };
}

function isServiceInfo(value: unknown): value is ServiceInfo {
  if (!value || typeof value !== "object") {
    return false;
  }

  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.serviceInstanceId === "string" &&
    typeof candidate.serviceName === "string" &&
    typeof candidate.serviceVersion === "string" &&
    candidate.apiVersion === SUPPORTED_API_VERSION &&
    isPublicCatalogStatus(candidate.publicCatalogStatus) &&
    Array.isArray(candidate.capabilities) &&
    candidate.capabilities.every(isServiceCapability)
  );
}

function isServiceConnectionFile(value: unknown): value is {
  serviceName: string;
  serviceInstanceId: string;
  apiVersion: string;
  baseUrl: string;
  serviceInfoUrl: string;
  capabilities: ServiceCapability[];
} {
  if (!value || typeof value !== "object") {
    return false;
  }

  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.serviceName === "string" &&
    typeof candidate.serviceInstanceId === "string" &&
    candidate.apiVersion === SUPPORTED_API_VERSION &&
    typeof candidate.baseUrl === "string" &&
    typeof candidate.serviceInfoUrl === "string" &&
    Array.isArray(candidate.capabilities) &&
    candidate.capabilities.every(isServiceCapability)
  );
}

function containsSecretConnectionField(value: unknown): boolean {
  if (!value || typeof value !== "object") {
    return false;
  }

  if (Array.isArray(value)) {
    return value.some(containsSecretConnectionField);
  }

  return Object.entries(value as Record<string, unknown>).some(([key, nested]) => {
    if (SECRET_CONNECTION_FILE_FIELDS.has(key)) {
      return true;
    }

    return containsSecretConnectionField(nested);
  });
}

function isPublicCatalogStatus(value: unknown): value is PublicCatalogStatus {
  return (
    value === "empty" ||
    value === "ready" ||
    value === "updating" ||
    value === "unavailable"
  );
}

function isServiceCapability(value: unknown): value is ServiceCapability {
  return value === PUBLIC_READ_CAPABILITY;
}

function isLocalhost(hostname: string): boolean {
  const normalized = hostname.toLowerCase();
  return normalized === "localhost" || normalized === "127.0.0.1" || normalized === "::1";
}

function isPrivateIpv4(hostname: string): boolean {
  const parts = hostname.split(".").map((part) => Number(part));
  if (parts.length !== 4 || parts.some((part) => !Number.isInteger(part) || part < 0 || part > 255)) {
    return false;
  }

  const [first, second] = parts;
  return (
    first === 10 ||
    (first === 172 && second >= 16 && second <= 31) ||
    (first === 192 && second === 168)
  );
}
