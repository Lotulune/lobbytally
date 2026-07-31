import { invoke, isTauri } from "@tauri-apps/api/core";
import type { CustomRoutingPreset, CustomTaskRoute } from "./customAiRoutes";

const BROWSER_SESSION_KEY = "mpgs.ai.custom.session.v3";
const LEGACY_BROWSER_KEYS = [
  "mpgs.ai.custom.session.v2",
  "mpgs.ai.custom.session.v1",
] as const;

export interface LocalCustomAiSettings {
  userId: string;
  baseUrl: string;
  /** Default / construction-time model (also primary for single/easy mode). */
  model: string;
  apiKey: string;
  /**
   * easy = all tasks use one selected model
   * advanced = per-task models (power users)
   * single = multi_model false, one model field only
   */
  routingPreset: CustomRoutingPreset;
  fallbackModel?: string | null;
  routes?: CustomTaskRoute[];
}

interface LocalCredentialStore {
  v: 3;
  entries: Record<string, LocalCustomAiSettings>;
}

function parsePreset(value: unknown): CustomRoutingPreset {
  if (value === "single" || value === "easy" || value === "advanced") return value;
  return "single";
}

function parseSettings(value: unknown, userId: string): LocalCustomAiSettings | null {
  if (typeof value !== "object" || value === null) return null;
  const parsed = value as Partial<LocalCustomAiSettings>;
  if (
    parsed.userId !== userId ||
    typeof parsed.baseUrl !== "string" ||
    typeof parsed.model !== "string" ||
    typeof parsed.apiKey !== "string" ||
    !parsed.apiKey
  ) {
    return null;
  }
  return {
    userId,
    baseUrl: parsed.baseUrl,
    model: parsed.model,
    apiKey: parsed.apiKey,
    routingPreset: parsePreset(parsed.routingPreset),
    fallbackModel: parsed.fallbackModel ?? null,
    routes: Array.isArray(parsed.routes) ? parsed.routes : undefined,
  };
}

function parseJson(raw: string | null): unknown {
  if (!raw) return null;
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    return null;
  }
}

function parseStore(raw: string | null): LocalCredentialStore | null {
  const parsed = parseJson(raw);
  if (
    typeof parsed !== "object" ||
    parsed === null ||
    (parsed as { v?: unknown }).v !== 3 ||
    typeof (parsed as { entries?: unknown }).entries !== "object" ||
    (parsed as { entries?: unknown }).entries === null
  ) {
    return null;
  }
  return parsed as LocalCredentialStore;
}

function canonicalScopeOrigin(serviceOrigin: string): string {
  try {
    return new URL(serviceOrigin).origin;
  } catch {
    return serviceOrigin.trim().replace(/\/+$/, "");
  }
}

function credentialEntryKey(serviceOrigin: string, userId: string): string {
  return JSON.stringify([canonicalScopeOrigin(serviceOrigin), userId]);
}

async function readModernRaw(): Promise<string | null> {
  if (isTauri()) return invoke<string | null>("ai_credential_load");
  return globalThis.sessionStorage.getItem(BROWSER_SESSION_KEY);
}

async function writeModernStore(store: LocalCredentialStore): Promise<void> {
  const value = JSON.stringify(store);
  if (isTauri()) {
    await invoke("ai_credential_save", { value });
  } else {
    globalThis.sessionStorage.setItem(BROWSER_SESSION_KEY, value);
  }
}

async function removeModernStore(): Promise<void> {
  if (isTauri()) {
    await invoke("ai_credential_remove");
  } else {
    globalThis.sessionStorage.removeItem(BROWSER_SESSION_KEY);
  }
}

async function readLegacyRaw(): Promise<{ raw: string; browserKey?: string } | null> {
  if (isTauri()) {
    const raw = await invoke<string | null>("ai_credential_load");
    return raw && !parseStore(raw) ? { raw } : null;
  }
  for (const key of LEGACY_BROWSER_KEYS) {
    const raw = globalThis.sessionStorage.getItem(key);
    if (raw) return { raw, browserKey: key };
  }
  return null;
}

export async function loadLocalCustomAiSettings(
  userId: string,
  serviceOrigin: string,
): Promise<LocalCustomAiSettings | null> {
  const raw = await readModernRaw();
  const store = parseStore(raw);
  if (store) {
    return parseSettings(store.entries[credentialEntryKey(serviceOrigin, userId)], userId);
  }

  // Migrate the former single-record credential only when it belongs to the
  // active account. A mismatched legacy secret remains untouched.
  const legacy = await readLegacyRaw();
  const legacySettings = parseSettings(parseJson(legacy?.raw ?? null), userId);
  if (!legacySettings) return null;
  await saveLocalCustomAiSettings(legacySettings, serviceOrigin);
  if (!isTauri() && legacy?.browserKey) {
    globalThis.sessionStorage.removeItem(legacy.browserKey);
  }
  return legacySettings;
}

export async function saveLocalCustomAiSettings(
  settings: LocalCustomAiSettings,
  serviceOrigin: string,
): Promise<void> {
  const existing = parseStore(await readModernRaw());
  const store: LocalCredentialStore = existing ?? { v: 3, entries: {} };
  store.entries[credentialEntryKey(serviceOrigin, settings.userId)] = settings;
  await writeModernStore(store);
}

export async function removeLocalCustomAiSettings(
  userId: string,
  serviceOrigin: string,
): Promise<void> {
  const store = parseStore(await readModernRaw());
  if (!store) return;
  delete store.entries[credentialEntryKey(serviceOrigin, userId)];
  if (Object.keys(store.entries).length === 0) {
    await removeModernStore();
  } else {
    await writeModernStore(store);
  }
}
