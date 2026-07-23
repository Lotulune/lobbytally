import { invoke, isTauri } from "@tauri-apps/api/core";
import { ScopedStorage } from "./scopedStorage";
import type { StorageLike } from "./types";

const MPGS_KEY_PREFIX = "mpgs.";
const SESSION_KEY = "mpgs.session.v1";

/** Unscoped and origin-scoped session keys (ScopedStorage prefixes the key). */
export function isSessionStorageKey(key: string): boolean {
  return key === SESSION_KEY || key.endsWith(`.${SESSION_KEY}`) || key.endsWith(SESSION_KEY);
}

/** v2 keyring blob: multi-origin session map. Legacy is a raw session JSON string. */
function encodeSessionMap(sessions: Record<string, string>): string {
  return JSON.stringify({ v: 2, sessions });
}

function decodeSessionMap(raw: string | null): Record<string, string> {
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw) as { v?: number; sessions?: Record<string, string> };
    if (parsed && parsed.v === 2 && parsed.sessions && typeof parsed.sessions === "object") {
      const out: Record<string, string> = {};
      for (const [key, value] of Object.entries(parsed.sessions)) {
        if (typeof value === "string" && isSessionStorageKey(key)) out[key] = value;
      }
      return out;
    }
  } catch {
    // fall through — legacy single-session blob
  }
  return { [SESSION_KEY]: raw };
}

class SqliteBackedStorage implements StorageLike {
  private readonly values = new Map<string, string>();
  private writeChain: Promise<void> = Promise.resolve();
  private lastWriteError: unknown = null;

  hydrate(values: Record<string, string>, sessions: Record<string, string>): void {
    this.values.clear();
    for (const [key, value] of Object.entries(values)) {
      if (isSessionStorageKey(key)) continue; // never mirror session material in SQLite
      this.values.set(key, value);
    }
    for (const [key, value] of Object.entries(sessions)) {
      this.values.set(key, value);
    }
  }

  get length(): number {
    return this.values.size;
  }

  key(index: number): string | null {
    return Array.from(this.values.keys())[index] ?? null;
  }

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    const normalized = String(value);
    this.values.set(key, normalized);
    if (isSessionStorageKey(key)) {
      this.enqueue(() => this.persistSessionMap());
      return;
    }
    this.enqueue(() => invoke("client_store_set", { key, value: normalized }));
  }

  removeItem(key: string): void {
    this.values.delete(key);
    if (isSessionStorageKey(key)) {
      this.enqueue(() => this.persistSessionMap());
      return;
    }
    this.enqueue(() => invoke("client_store_remove", { key }));
  }

  private collectSessions(): Record<string, string> {
    const sessions: Record<string, string> = {};
    for (const [key, value] of this.values) {
      if (isSessionStorageKey(key)) sessions[key] = value;
    }
    return sessions;
  }

  private async persistSessionMap(): Promise<void> {
    const sessions = this.collectSessions();
    if (Object.keys(sessions).length === 0) {
      await invoke("auth_session_remove");
      return;
    }
    await invoke("auth_session_save", { value: encodeSessionMap(sessions) });
  }

  private enqueue(write: () => Promise<unknown>): void {
    this.writeChain = this.writeChain
      .then(write)
      .then(() => undefined)
      .catch((error: unknown) => {
        // Keep later writes runnable. flush() still surfaces the last failure.
        this.lastWriteError = error;
      });
  }

  async flush(): Promise<void> {
    await this.writeChain;
    if (this.lastWriteError !== null) {
      const error = this.lastWriteError;
      this.lastWriteError = null;
      throw error;
    }
  }
}

let activeStorage: StorageLike | null = null;
let sqliteStorage: SqliteBackedStorage | null = null;

async function installDesktopCloseGuard(): Promise<void> {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const appWindow = getCurrentWindow();
  let closing = false;
  await appWindow.onCloseRequested((event) => {
    event.preventDefault();
    if (closing) return;
    closing = true;
    void flushClientStorage().finally(() => appWindow.destroy());
  });
}

/**
 * Hydrate the desktop key/value mirror before React and app singletons load.
 * Browser development keeps the native Web Storage fallback; packaged Tauri
 * builds persist non-secret client state in the private application SQLite DB.
 * Session tokens use the operating system credential store instead.
 */
export async function initializeClientStorage(): Promise<void> {
  if (!isTauri()) {
    activeStorage = globalThis.localStorage;
    return;
  }

  const store = new SqliteBackedStorage();
  const [persisted, secureSession, sqliteLegacySession] = await Promise.all([
    invoke<Record<string, string>>("client_store_load"),
    invoke<string | null>("auth_session_load"),
    invoke<string | null>("client_store_take_legacy_session"),
  ]);
  const sessions = decodeSessionMap(secureSession);
  // One-time: legacy unscoped session that lived in SQLite client_kv.
  if (sqliteLegacySession !== null && sessions[SESSION_KEY] === undefined) {
    sessions[SESSION_KEY] = sqliteLegacySession;
  }
  // Strip any session-shaped keys that slipped into client_kv before the fix.
  const kvOnly: Record<string, string> = {};
  for (const [key, value] of Object.entries(persisted)) {
    if (isSessionStorageKey(key)) {
      if (sessions[key] === undefined) sessions[key] = value;
      continue;
    }
    kvOnly[key] = value;
  }
  store.hydrate(kvOnly, sessions);
  // Ensure keyring holds the v2 multi-session map (migrates legacy single blob).
  if (Object.keys(sessions).length > 0) {
    const [firstKey, firstValue] = Object.entries(sessions)[0]!;
    store.setItem(firstKey, firstValue);
  }

  // One-time migration for users of the previous localStorage-backed builds.
  const legacy = globalThis.localStorage;
  for (let index = 0; index < legacy.length; index += 1) {
    const key = legacy.key(index);
    if (!key?.startsWith(MPGS_KEY_PREFIX) || store.getItem(key) !== null) continue;
    const value = legacy.getItem(key);
    if (value !== null) store.setItem(key, value);
  }
  await store.flush();
  for (let index = legacy.length - 1; index >= 0; index -= 1) {
    const key = legacy.key(index);
    if (key?.startsWith(MPGS_KEY_PREFIX)) legacy.removeItem(key);
  }

  sqliteStorage = store;
  activeStorage = store;
  await installDesktopCloseGuard();
}

export function getClientStorage(): StorageLike {
  if (activeStorage) return activeStorage;
  if (!isTauri()) return globalThis.localStorage;
  throw new Error("desktop client storage was accessed before SQLite hydration");
}

/** Primarily useful before a controlled desktop shutdown or in persistence tests. */
export function flushClientStorage(): Promise<void> {
  return sqliteStorage?.flush() ?? Promise.resolve();
}

// --- per-service namespace (PRD_CS CS-007) ---

let serviceScopedStorage: ScopedStorage | null = null;

/**
 * Point `getServiceStorage()` at the namespace of the active service origin.
 * Called once during app bootstrap after the persisted origin is read, and
 * again after a service switch (which normally reloads the app).
 */
export function activateServiceScope(origin: string): StorageLike {
  serviceScopedStorage = new ScopedStorage(getClientStorage(), origin);
  return serviceScopedStorage;
}

/**
 * Storage for per-service state (session, caches, queues, pending writes).
 * Falls back to the unscoped client storage in browser dev / e2e modes where
 * the API base is fixed and no service selection takes place.
 */
export function getServiceStorage(): StorageLike {
  return serviceScopedStorage ?? getClientStorage();
}

/** Test hook: drop the service namespace between cases. */
export function resetServiceScopeForTests(): void {
  serviceScopedStorage = null;
}
