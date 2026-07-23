// App-level singletons and light global state (no external state library).
//
// Service addressing (PRD_CS §12.2):
// - The packaged desktop client has NO baked-in server address. The API base
//   URL comes from the persisted, user-verified service origin, and all
//   per-service state lives in that origin's storage namespace.
// - Browser dev and Tauri dev load through the Vite proxy (same-origin "").
// - E2E builds may still bake VITE_MPGS_API_BASE to pin a test server.

import { isTauri } from "@tauri-apps/api/core";
import { ApiClient } from "../api/client";
import { FeedbackQueue } from "../api/feedbackQueue";
import { PlayIntentStore } from "../api/playIntentStore";
import { getCurrentServiceOrigin } from "../api/serverOrigin";
import { activateServiceScope, getClientStorage, getServiceStorage } from "../api/storage";
import type { StorageLike } from "../api/types";
import { flushPendingPreferencePatch } from "./preferences";

const BAKED_API_BASE = import.meta.env.VITE_MPGS_API_BASE;
const baseStorage = getClientStorage();

// The persisted origin only matters when no base is baked at build time.
const persistedOrigin = BAKED_API_BASE ? null : getCurrentServiceOrigin(baseStorage);

let resolvedBaseUrl: string;
if (BAKED_API_BASE !== undefined) {
  resolvedBaseUrl = BAKED_API_BASE;
} else if (import.meta.env.DEV) {
  // Vite dev server proxies /v1 etc. to the local mpgs-server.
  resolvedBaseUrl = "";
} else if (isTauri()) {
  // Packaged desktop: talk to the user-confirmed service only. Empty means
  // "not connected yet" — the connect gate keeps the app out of business UI.
  resolvedBaseUrl = persistedOrigin ?? "";
} else {
  // Browser production (optional full deployment): same-origin API.
  resolvedBaseUrl = "";
}

// Scope per-service state to the active origin in the packaged desktop app.
// Dev/e2e modes keep the historical unscoped storage so local workflows and
// existing tests are unaffected.
const serviceScoped = !BAKED_API_BASE && !import.meta.env.DEV && isTauri() && persistedOrigin;
const storage = serviceScoped
  ? activateServiceScope(persistedOrigin as string)
  : baseStorage;

/** The service origin this build/session is bound to (null in dev/e2e). */
export const activeServiceOrigin: string | null = serviceScoped ? persistedOrigin : null;

/** True when the app must show the connect page before any business UI. */
export const requiresServiceConnect: boolean =
  !BAKED_API_BASE && !import.meta.env.DEV && isTauri();

export const apiClient = new ApiClient({ baseUrl: resolvedBaseUrl, storage });
export const feedbackQueue = new FeedbackQueue(apiClient, storage);
export const playIntentStore = new PlayIntentStore(apiClient, storage);

// Replay pending feedback and votes when connectivity returns. The storage
// namespace guarantees these writes only ever go to the service that queued
// them (PRD §5.2: revalidate origin before replay).
if (typeof window !== "undefined") {
  void feedbackQueue.flush();
  void playIntentStore.flush();
  void flushPendingPreferencePatch(apiClient).catch(() => undefined);
  window.addEventListener("online", () => {
    void feedbackQueue.flush();
    void playIntentStore.flush();
    void flushPendingPreferencePatch(apiClient).catch(() => undefined);
  });
}

const ONBOARDED_KEY = "mpgs.onboarded.v1";

// Onboarding + FX are device preferences (not per-service credentials/caches).
export function isOnboarded(storage: StorageLike = getClientStorage()): boolean {
  try {
    return storage.getItem(ONBOARDED_KEY) === "true";
  } catch {
    return false;
  }
}

export function markOnboarded(storage: StorageLike = getClientStorage()): void {
  try {
    storage.setItem(ONBOARDED_KEY, "true");
  } catch {
    // best effort
  }
}

const FX_KEY = "mpgs.fx.v1";

export function loadFxIntensity(storage: StorageLike = getClientStorage()): string | null {
  try {
    return storage.getItem(FX_KEY);
  } catch {
    return null;
  }
}

export function saveFxIntensity(value: string, storage: StorageLike = getClientStorage()): void {
  try {
    storage.setItem(FX_KEY, value);
  } catch {
    // best effort
  }
}
