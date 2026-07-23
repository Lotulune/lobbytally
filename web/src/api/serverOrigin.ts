// Service origin handling for the pure-client desktop model (PRD_CS §5, §10).
//
// The desktop client never embeds a server. It connects to exactly one
// user-confirmed MPGS Server origin at a time, and every piece of persisted
// per-service state (session, caches, queues) is namespaced by the normalized
// origin so data and credentials can never leak across services.
//
// This module is pure: persistence functions take a StorageLike so they are
// testable without the Tauri/localStorage plumbing.

import type { StorageLike } from "./types";

/**
 * Placeholder / help examples only — never used as the controlled input value.
 * Users must type or paste an address themselves.
 */
export const SERVICE_ORIGIN_PLACEHOLDER = "https://mpgs.example.com 或 192.168.1.10:17880";

/** Short help line under the address field. */
export const SERVICE_ORIGIN_HINT =
  "示例：https://mpgs.example.com、https://mpgs.example.com:8443、127.0.0.1:17880；勿填路径或账号密码。";

const CURRENT_KEY = "mpgs.server.current.v1";
const KNOWN_KEY = "mpgs.server.known.v1";

/** URL validation failures, mapped to the PRD §10 `invalid_url` semantics. */
export type OriginRejection =
  | "empty"
  | "unparseable"
  | "unsupported_protocol"
  | "credentials_not_allowed"
  | "path_not_allowed"
  | "query_not_allowed"
  | "fragment_not_allowed";

export type NormalizeResult =
  | { ok: true; origin: string }
  | { ok: false; reason: OriginRejection };

function isLoopbackHostname(hostname: string): boolean {
  // url.hostname for IPv6 is unbracketed (`::1`), not `[::1]`.
  return (
    hostname === "localhost" ||
    hostname === "127.0.0.1" ||
    hostname === "::1" ||
    hostname === "0:0:0:0:0:0:0:1"
  );
}

/** True for IPv4 or IPv6 hostnames (URL-standard unbracketed IPv6). */
export function isIpHostname(hostname: string): boolean {
  if (/^(?:\d{1,3}\.){3}\d{1,3}$/.test(hostname)) {
    return hostname.split(".").every((part) => {
      const n = Number(part);
      return Number.isInteger(n) && n >= 0 && n <= 255;
    });
  }
  // IPv6 literals contain colons once brackets are stripped by the URL parser.
  if (hostname.includes(":")) return true;
  return false;
}

/**
 * Whether an input string already includes a URI scheme (`https://…`).
 * Bare `host:port` / `IP:port` intentionally have no scheme.
 */
function hasUriScheme(input: string): boolean {
  return /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(input);
}

/**
 * Infer a scheme for bare `host[:port]` / `IP[:port]` forms.
 * - IP addresses → http (LAN / self-hosted)
 * - hostnames → https
 */
function coerceBareAddress(input: string): string {
  const trimmed = input.trim();
  if (hasUriScheme(trimmed)) return trimmed;

  // IPv6 with brackets: [::1]:8080 or [2001:db8::1]
  if (trimmed.startsWith("[")) {
    return `http://${trimmed}`;
  }

  // host:port — split on last colon when the host is not pure IPv6 without brackets
  // (unbracketed IPv6 bare forms are ambiguous and rejected as unparseable later).
  let hostPart = trimmed;
  const lastColon = trimmed.lastIndexOf(":");
  if (lastColon > 0 && /^\d{1,5}$/.test(trimmed.slice(lastColon + 1))) {
    hostPart = trimmed.slice(0, lastColon);
  }

  if (isLoopbackHostname(hostPart.toLowerCase()) || isIpHostname(hostPart)) {
    return `http://${trimmed}`;
  }
  return `https://${trimmed}`;
}

/**
 * Build a fetch-safe origin string. Prefer `url.origin` (correct IPv6 brackets)
 * and only re-append a non-default port if needed (url.origin already handles it).
 */
function originFromUrl(url: URL): string {
  // `url.origin` is the canonical form: scheme://host[:port] with IPv6 brackets
  // and default ports stripped.
  return url.origin;
}

/**
 * Validate and normalize a user-entered service address.
 *
 * Rules:
 * - accepts `https://host[:port]`, bare `host[:port]`, bare `IP[:port]`
 * - bare IP → `http://IP[:port]`; bare hostname → `https://host[:port]`
 * - explicit `http:` allowed for loopback (dev) and for any IP address (LAN)
 * - user info, query, fragment and any non-empty path are rejected outright
 * - hostname is lowercased via the URL parser; default ports dropped
 *
 * `allowHttpLoopback` still gates non-IP loopback http in packaged builds
 * when the user types an explicit `http://localhost` URL.
 */
export function normalizeServiceOrigin(
  input: string,
  options: { allowHttpLoopback?: boolean } = {},
): NormalizeResult {
  const trimmed = input.trim();
  if (!trimmed) return { ok: false, reason: "empty" };

  let url: URL;
  try {
    url = new URL(coerceBareAddress(trimmed));
  } catch {
    return { ok: false, reason: "unparseable" };
  }

  if (url.username || url.password) return { ok: false, reason: "credentials_not_allowed" };
  if (url.search) return { ok: false, reason: "query_not_allowed" };
  if (url.hash) return { ok: false, reason: "fragment_not_allowed" };
  if (url.pathname !== "" && url.pathname !== "/") {
    return { ok: false, reason: "path_not_allowed" };
  }

  const hostname = url.hostname.toLowerCase();
  if (url.protocol === "https:") {
    // always allowed
  } else if (url.protocol === "http:") {
    const httpOk =
      isIpHostname(hostname) ||
      (Boolean(options.allowHttpLoopback) && isLoopbackHostname(hostname));
    if (!httpOk) {
      return { ok: false, reason: "unsupported_protocol" };
    }
  } else {
    return { ok: false, reason: "unsupported_protocol" };
  }

  // url.origin is the canonical form (IPv6 brackets, default port dropped,
  // hostname lowercased). Do not rebuild by hand — re-bracketing hostname is
  // engine-dependent and can produce `[[::1]]`.
  return { ok: true, origin: originFromUrl(url) };
}

/** True when `candidate` equals the normalized form of the current origin. */
export function sameServiceOrigin(a: string, b: string): boolean {
  return a === b;
}

// --- origin registry (global, NOT namespaced) ---

export interface KnownService {
  origin: string;
  /** Unix ms when this origin first passed the full connection check. */
  addedAtMs: number;
  /** Unix ms of the most recent successful connection. */
  lastConnectedAtMs: number;
}

/** The active service origin, or null when the client has never connected. */
export function getCurrentServiceOrigin(storage: StorageLike): string | null {
  try {
    const raw = storage.getItem(CURRENT_KEY);
    if (!raw || !raw.trim()) return null;
    // Re-validate so corrupted storage cannot become apiClient.baseUrl.
    const normalized = normalizeServiceOrigin(raw, { allowHttpLoopback: true });
    if (!normalized.ok) return null;
    return normalized.origin;
  } catch {
    return null;
  }
}

export function listKnownServices(storage: StorageLike): KnownService[] {
  try {
    const raw = storage.getItem(KNOWN_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as KnownService[];
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (entry) =>
        entry &&
        typeof entry.origin === "string" &&
        typeof entry.addedAtMs === "number" &&
        typeof entry.lastConnectedAtMs === "number",
    );
  } catch {
    return [];
  }
}

function saveKnownServices(storage: StorageLike, services: KnownService[]): void {
  storage.setItem(KNOWN_KEY, JSON.stringify(services));
}

/**
 * Persist `origin` as the active service after a successful connection check,
 * creating or refreshing its registry entry. Previous origin data is kept
 * untouched so the user can switch back later (PRD §5.3).
 */
export function activateServiceOrigin(
  storage: StorageLike,
  origin: string,
  nowMs: number = Date.now(),
): void {
  const known = listKnownServices(storage);
  const existing = known.find((entry) => entry.origin === origin);
  if (existing) {
    existing.lastConnectedAtMs = nowMs;
  } else {
    known.push({ origin, addedAtMs: nowMs, lastConnectedAtMs: nowMs });
  }
  saveKnownServices(storage, known);
  storage.setItem(CURRENT_KEY, origin);
}

/** Forget a known origin without touching its namespaced data. */
export function forgetKnownService(storage: StorageLike, origin: string): void {
  saveKnownServices(
    storage,
    listKnownServices(storage).filter((entry) => entry.origin !== origin),
  );
}

/** Clear the active-origin pointer (used after deleting the current service). */
export function clearCurrentServiceOrigin(storage: StorageLike): void {
  storage.removeItem(CURRENT_KEY);
}

// --- namespacing ---

function base64UrlEncode(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/**
 * Storage key prefix isolating every piece of per-service state. The origin is
 * encoded (never hashed) so diagnostics can map a key back to its service
 * without leaking anything beyond the origin itself, which PRD §9 allows.
 */
export function serviceNamespacePrefix(origin: string): string {
  return `mpgs.svc.${base64UrlEncode(origin)}.`;
}
