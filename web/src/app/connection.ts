// Service connection state for the running app (PRD_CS §5.2, §5.3).
//
// Owns the runtime view of "which service are we talking to and how healthy
// is the link". The active origin itself is persisted by serverOrigin.ts;
// this module adds the background recheck, offline/maintenance detection and
// the switch/delete operations used by the settings screen.

import { checkServiceConnection, type ConnectErrorKind } from "../api/discovery";
import { ScopedStorage, deleteServiceNamespace } from "../api/scopedStorage";
import {
  activateServiceOrigin,
  clearCurrentServiceOrigin,
  forgetKnownService,
  getCurrentServiceOrigin,
  listKnownServices,
  type KnownService,
} from "../api/serverOrigin";
import { getClientStorage } from "../api/storage";
import type { StorageLike } from "../api/types";

/**
 * Runtime link health:
 * - `checking`: a recheck is in flight (startup or manual retry)
 * - `connected`: discovery + readiness + meta all succeed
 * - `maintenance`: discovery ok but readiness says 503 (NOT an address error)
 * - `offline`: unreachable/timeout; the app may serve this origin's cache
 */
export type ConnectionStatus = "checking" | "connected" | "maintenance" | "offline";

export interface ConnectionSnapshot {
  origin: string | null;
  status: ConnectionStatus;
  /** Set when the last check failed; drives the error copy in settings. */
  lastError: ConnectErrorKind | null;
}

type ConnectionListener = (snapshot: ConnectionSnapshot) => void;

class ConnectionManager {
  private readonly storage: StorageLike;
  private listeners = new Set<ConnectionListener>();
  private snapshot: ConnectionSnapshot;
  private recheckPromise: Promise<ConnectionStatus> | null = null;

  constructor(storage: StorageLike) {
    this.storage = storage;
    this.snapshot = {
      origin: getCurrentServiceOrigin(storage),
      status: "checking",
      lastError: null,
    };
  }

  get(): ConnectionSnapshot {
    return this.snapshot;
  }

  subscribe(listener: ConnectionListener): () => void {
    this.listeners.add(listener);
    listener(this.snapshot);
    return () => this.listeners.delete(listener);
  }

  private update(patch: Partial<ConnectionSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch };
    for (const listener of this.listeners) listener(this.snapshot);
  }

  /**
   * Background connection check for the saved origin (PRD §5.2). A failure
   * never clears the saved origin — the user keeps the offline cache.
   */
  recheck(): Promise<ConnectionStatus> {
    const origin = this.snapshot.origin;
    if (!origin) {
      this.update({ status: "offline", lastError: null });
      return Promise.resolve("offline");
    }
    this.recheckPromise ??= (async () => {
      this.update({ status: "checking" });
      const result = await checkServiceConnection(origin);
      if (result.ok) {
        activateServiceOrigin(this.storage, origin);
        this.update({ status: "connected", lastError: null });
      } else if (result.kind === "not_ready") {
        this.update({ status: "maintenance", lastError: "not_ready" });
      } else {
        this.update({ status: "offline", lastError: result.kind });
      }
      return this.snapshot.status;
    })().finally(() => {
      this.recheckPromise = null;
    });
    return this.recheckPromise;
  }

  knownServices(): KnownService[] {
    return listKnownServices(this.storage);
  }

  /** Storage view scoped to the active origin; null before first connect. */
  scopedStorageFor(origin: string): ScopedStorage {
    return new ScopedStorage(this.storage, origin);
  }

  /**
   * Delete all local data for one service (CS-009). When the target is the
   * active service the caller must reload into the connect flow afterwards.
   */
  deleteServiceData(origin: string): { removedKeys: number; wasCurrent: boolean } {
    const wasCurrent = this.snapshot.origin === origin;
    const removedKeys = deleteServiceNamespace(this.storage, origin);
    forgetKnownService(this.storage, origin);
    if (wasCurrent) {
      clearCurrentServiceOrigin(this.storage);
      this.update({ origin: null, status: "offline", lastError: null });
    }
    return { removedKeys, wasCurrent };
  }
}

let manager: ConnectionManager | null = null;

/** Lazily built so tests and the browser dev mode never touch Tauri storage. */
export function getConnectionManager(): ConnectionManager {
  manager ??= new ConnectionManager(getClientStorage());
  return manager;
}

/** Test hook: drop the singleton between cases. */
export function resetConnectionManagerForTests(): void {
  manager = null;
}
