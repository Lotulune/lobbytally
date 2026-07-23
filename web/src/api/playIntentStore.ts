// Optimistic play-intent (community vote) store.
//
// The server holds authoritative counts and this user's vote (feed + authed
// detail carry `voted`). This store overlays the user's optimistic toggles and
// replays them when connectivity returns. Persisted separately from the response
// cache so clearing cache never drops an unsynced vote.

import { ApiClient, ApiError } from "./client";
import { getServiceStorage } from "./storage";
import type { StorageLike } from "./types";

const STORE_KEY = "mpgs.playintent.v1";

interface VoteEntry {
  voted: boolean;
  /** True until the desired state is acknowledged by the server. */
  pending: boolean;
  /** Identity that acknowledged a settled override. */
  userId?: string | null;
}

export type PlayIntentListener = () => void;

interface StoreShape {
  entries: Record<string, VoteEntry>;
}

export class PlayIntentStore {
  private readonly client: ApiClient;
  private readonly storage: StorageLike;
  private entries = new Map<number, VoteEntry>();
  private listeners = new Set<PlayIntentListener>();
  private syncPromises = new Map<number, Promise<void>>();

  constructor(client: ApiClient, storage: StorageLike = getServiceStorage()) {
    this.client = client;
    this.storage = storage;
    this.load();
  }

  private load(): void {
    try {
      const raw = this.storage.getItem(STORE_KEY);
      if (!raw) return;
      const parsed = JSON.parse(raw) as StoreShape;
      for (const [appId, entry] of Object.entries(parsed.entries ?? {})) {
        if (typeof entry?.voted === "boolean") {
          this.entries.set(Number(appId), {
            voted: entry.voted,
            pending: entry.pending ?? false,
            userId: entry.userId,
          });
        }
      }
    } catch {
      this.entries.clear();
    }
  }

  private persist(notify = true): void {
    try {
      const entries: Record<string, VoteEntry> = {};
      for (const [appId, entry] of this.entries) entries[String(appId)] = entry;
      this.storage.setItem(STORE_KEY, JSON.stringify({ entries }));
    } catch {
      // best effort
    }
    if (notify) {
      for (const listener of this.listeners) listener();
    }
  }

  subscribe(listener: PlayIntentListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** The user's effective vote, preferring a local override over the server flag. */
  effectiveVoted(appId: number, serverVoted: boolean): boolean {
    return this.reconciledEntry(appId, serverVoted)?.voted ?? serverVoted;
  }

  /** Count adjustment vs the server count given the current server vote flag. */
  countDelta(appId: number, serverVoted: boolean): number {
    const entry = this.reconciledEntry(appId, serverVoted);
    if (!entry || entry.voted === serverVoted) return 0;
    return entry.voted ? 1 : -1;
  }

  isPending(appId: number): boolean {
    return this.entries.get(appId)?.pending ?? false;
  }

  pendingCount(): number {
    let count = 0;
    for (const entry of this.entries.values()) if (entry.pending) count += 1;
    return count;
  }

  /** Flip the vote optimistically and sync. `serverVoted` is the latest known flag. */
  toggle(appId: number, serverVoted: boolean): void {
    const current = this.entries.get(appId)?.voted ?? serverVoted;
    this.entries.set(appId, { voted: !current, pending: true });
    this.persist();
    void this.sync(appId);
  }

  private sync(appId: number): Promise<void> {
    const existing = this.syncPromises.get(appId);
    if (existing) return existing;
    const promise = this.runSync(appId).finally(() => {
      this.syncPromises.delete(appId);
    });
    this.syncPromises.set(appId, promise);
    return promise;
  }

  private async runSync(appId: number): Promise<void> {
    while (true) {
      const entry = this.entries.get(appId);
      if (!entry?.pending) return;
      const desired = entry.voted;
      try {
        const result = await this.client.setPlayIntent(appId, desired);
        const current = this.entries.get(appId);
        if (!current) return;
        if (current.voted !== desired) continue;
        // Keep a short-lived override until a server payload reflects the ack.
        this.entries.set(appId, {
          voted: result.voted,
          pending: false,
          userId: this.client.sessionUserId(),
        });
        this.persist();
        return;
      } catch (error) {
        const current = this.entries.get(appId);
        if (!current) return;
        if (current.voted !== desired) continue;
        if (
          error instanceof ApiError &&
          (error.offline || error.status === 408 || error.status === 429 || error.status >= 500)
        ) {
          return; // stay pending; flush() retries
        }
        // Permanent failure: drop the optimistic override so the UI reverts.
        this.entries.delete(appId);
        this.persist();
        return;
      }
    }
  }

  private reconciledEntry(appId: number, serverVoted: boolean): VoteEntry | undefined {
    const entry = this.entries.get(appId);
    if (!entry || entry.pending) return entry;
    const wrongIdentity = entry.userId === undefined || entry.userId !== this.client.sessionUserId();
    if (wrongIdentity || entry.voted === serverVoted) {
      this.entries.delete(appId);
      // Reconciliation happens while rendering; persist quietly to avoid a
      // listener-driven state update during another component's render.
      this.persist(false);
      return undefined;
    }
    return entry;
  }

  /** Retry every unsynced vote. Safe to call on reconnect. */
  async flush(): Promise<void> {
    const pending = [...this.entries.entries()].filter(([, e]) => e.pending).map(([id]) => id);
    for (const appId of pending) {
      await this.sync(appId);
    }
  }
}
