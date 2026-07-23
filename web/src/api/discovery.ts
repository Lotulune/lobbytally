// MPGS service discovery + compatibility handshake (PRD_CS §5.1, §7, §10).
//
// The client must never mistake a plain website for an MPGS Server, so a
// service origin only becomes usable after three checks succeed in order:
//   1. GET /.well-known/mpgs     -> identity + protocol versions
//   2. GET <readiness_path>      -> 200 ready / 503 maintenance
//   3. GET /v1/meta              -> dynamic capabilities reachable
// Only then may the origin be persisted and a session created.

export const DISCOVERY_PATH = "/.well-known/mpgs";
export const SUPPORTED_DISCOVERY_VERSION = 1;
export const SUPPORTED_API_VERSION = "v1";
/** PRD §5.1 step 4: suggested discovery timeout. */
export const DEFAULT_CONNECT_TIMEOUT_MS = 8_000;

/** PRD §10 connection states (client-local verdicts, no server error codes). */
export type ConnectErrorKind =
  | "invalid_url"
  | "tls_error"
  | "not_mpgs"
  | "incompatible"
  | "not_ready"
  | "timeout"
  | "network";

export interface ServiceDiscovery {
  service: string;
  discovery_version: number;
  service_version: string;
  api_version: string;
  api_base_path: string;
  readiness_path: string;
  openapi_path?: string;
  authentication?: string[];
}

export interface ConnectSuccess {
  ok: true;
  discovery: ServiceDiscovery;
}

export interface ConnectFailure {
  ok: false;
  kind: ConnectErrorKind;
  /** Human-readable detail for logs/diagnostics; UI maps `kind` to copy. */
  detail: string;
}

export type ConnectResult = ConnectSuccess | ConnectFailure;

export interface ConnectOptions {
  fetchFn?: typeof fetch;
  timeoutMs?: number;
  /** Progress callback fired before each handshake step. */
  onStep?: (step: "discovery" | "readiness" | "meta") => void;
}

function failure(kind: ConnectErrorKind, detail: string): ConnectFailure {
  return { ok: false, kind, detail };
}

/** Fetch with an AbortController timeout; distinguishes aborts from failures. */
async function fetchWithTimeout(
  fetchFn: typeof fetch,
  url: string,
  timeoutMs: number,
): Promise<Response> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetchFn(url, {
      method: "GET",
      headers: { accept: "application/json" },
      signal: controller.signal,
      // Manual: never follow redirects — a 302 to another host would make the
      // handshake succeed while we still persist the user-entered origin.
      redirect: "manual",
    });
  } finally {
    clearTimeout(timer);
  }
}

/** Relative path only: single absolute path, no scheme-relative / query / host. */
function isSafeRelativePath(path: string): boolean {
  if (!path.startsWith("/") || path.startsWith("//")) return false;
  if (path.includes("://") || path.includes("?") || path.includes("#") || path.includes("\\")) {
    return false;
  }
  if (/\s/.test(path)) return false;
  return true;
}

function isRedirectStatus(status: number): boolean {
  return status >= 300 && status < 400;
}

/** TLS failures surface as TypeError from fetch; detect the common messages. */
function isTlsFailure(error: unknown): boolean {
  const message = (error instanceof Error ? error.message : String(error)).toLowerCase();
  return (
    message.includes("certificate") ||
    message.includes("ssl") ||
    message.includes("tls") ||
    message.includes("cert")
  );
}

function classifyFetchError(error: unknown): ConnectFailure {
  if (error instanceof DOMException && error.name === "AbortError") {
    return failure("timeout", "request timed out");
  }
  if (error instanceof Error && error.name === "AbortError") {
    return failure("timeout", "request timed out");
  }
  if (isTlsFailure(error)) {
    return failure("tls_error", error instanceof Error ? error.message : "tls failure");
  }
  return failure("network", error instanceof Error ? error.message : "network request failed");
}

/**
 * Validate the discovery document shape. Unknown fields are ignored per the
 * contract; unrecognized versions must block the connection (PRD §7).
 */
export function parseDiscovery(body: unknown): ServiceDiscovery | null {
  if (typeof body !== "object" || body === null) return null;
  const doc = body as Record<string, unknown>;
  if (doc.service !== "mpgs-server") return null;
  if (typeof doc.discovery_version !== "number" || !Number.isInteger(doc.discovery_version)) {
    return null;
  }
  if (typeof doc.api_version !== "string") return null;
  if (typeof doc.api_base_path !== "string" || !isSafeRelativePath(doc.api_base_path)) return null;
  if (typeof doc.readiness_path !== "string" || !isSafeRelativePath(doc.readiness_path)) return null;
  if (
    doc.openapi_path !== undefined &&
    (typeof doc.openapi_path !== "string" || !isSafeRelativePath(doc.openapi_path))
  ) {
    return null;
  }
  return {
    service: "mpgs-server",
    discovery_version: doc.discovery_version,
    service_version: typeof doc.service_version === "string" ? doc.service_version : "unknown",
    api_version: doc.api_version,
    api_base_path: doc.api_base_path,
    readiness_path: doc.readiness_path,
    openapi_path: typeof doc.openapi_path === "string" ? doc.openapi_path : undefined,
    authentication: Array.isArray(doc.authentication)
      ? doc.authentication.filter((a): a is string => typeof a === "string")
      : undefined,
  };
}

/**
 * Run the full connection check against an already-normalized service origin.
 * The origin must come from `normalizeServiceOrigin`; this function never
 * follows the discovery document to a different origin.
 */
export async function checkServiceConnection(
  origin: string,
  options: ConnectOptions = {},
): Promise<ConnectResult> {
  const fetchFn = options.fetchFn ?? fetch.bind(globalThis);
  const timeoutMs = options.timeoutMs ?? DEFAULT_CONNECT_TIMEOUT_MS;

  // Step 1: identity + protocol discovery.
  options.onStep?.("discovery");
  let discoveryResponse: Response;
  try {
    discoveryResponse = await fetchWithTimeout(fetchFn, `${origin}${DISCOVERY_PATH}`, timeoutMs);
  } catch (error) {
    return classifyFetchError(error);
  }
  if (isRedirectStatus(discoveryResponse.status) || discoveryResponse.type === "opaqueredirect") {
    return failure("not_mpgs", "discovery endpoint redirected");
  }
  if (discoveryResponse.status === 404) {
    return failure("not_mpgs", "discovery endpoint not found");
  }
  if (!discoveryResponse.ok) {
    return failure("not_mpgs", `discovery endpoint returned HTTP ${discoveryResponse.status}`);
  }
  let discoveryBody: unknown;
  try {
    discoveryBody = await discoveryResponse.json();
  } catch {
    return failure("not_mpgs", "discovery endpoint did not return JSON");
  }
  const discovery = parseDiscovery(discoveryBody);
  if (!discovery) {
    return failure("not_mpgs", "discovery document is not an MPGS service");
  }
  if (
    discovery.discovery_version !== SUPPORTED_DISCOVERY_VERSION ||
    discovery.api_version !== SUPPORTED_API_VERSION
  ) {
    return failure(
      "incompatible",
      `unsupported discovery_version=${discovery.discovery_version} api_version=${discovery.api_version}`,
    );
  }

  // Step 2: readiness. A 503 here means maintenance, NOT a bad address.
  options.onStep?.("readiness");
  let readyResponse: Response;
  try {
    readyResponse = await fetchWithTimeout(
      fetchFn,
      `${origin}${discovery.readiness_path}`,
      timeoutMs,
    );
  } catch (error) {
    return classifyFetchError(error);
  }
  if (isRedirectStatus(readyResponse.status) || readyResponse.type === "opaqueredirect") {
    return failure("not_mpgs", "readiness endpoint redirected");
  }
  if (readyResponse.status === 503) {
    return failure("not_ready", "service reports maintenance");
  }
  if (!readyResponse.ok) {
    return failure("not_ready", `readiness check returned HTTP ${readyResponse.status}`);
  }

  // Step 3: dynamic capabilities. Paths always resolve against the verified
  // origin; the document must not redirect us elsewhere (PRD §7).
  options.onStep?.("meta");
  let metaResponse: Response;
  try {
    metaResponse = await fetchWithTimeout(
      fetchFn,
      `${origin}${discovery.api_base_path}/meta`,
      timeoutMs,
    );
  } catch (error) {
    return classifyFetchError(error);
  }
  if (isRedirectStatus(metaResponse.status) || metaResponse.type === "opaqueredirect") {
    return failure("not_mpgs", "meta endpoint redirected");
  }
  if (!metaResponse.ok) {
    return failure("not_mpgs", `meta endpoint returned HTTP ${metaResponse.status}`);
  }

  return { ok: true, discovery };
}
