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
  appId: number;
  voted: boolean;
  /** True until the desired state is acknowledged by the server. */
  pending: boolean;
  /** Account that owns this override; null quarantines an unowned legacy record. */
  ownerUserId: string | null;
  /** Run that exposed this game; retained until positive intent attribution succeeds. */
  recommendationRunId: string | null;
}

export type PlayIntentListener = () => void;

interface StoreShape {
  v?: number;
  entries: Record<string, LegacyVoteEntry> | VoteEntry[];
}

interface LegacyVoteEntry {
  voted: boolean;
  pending?: boolean;
  userId?: string | null;
  ownerUserId?: string | null;
  appId?: number;
  recommendationRunId?: string | null;
}

export class PlayIntentStore {
  private readonly client: ApiClient;
  private readonly storage: StorageLike;
  private entries = new Map<string, VoteEntry>();
  private listeners = new Set<PlayIntentListener>();
  private syncPromises = new Map<string, Promise<void>>();

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
      const legacyOwner = this.currentOwnerUserId();
      const stored = parsed.entries ?? {};
      const rows: Array<[string, LegacyVoteEntry]> = Array.isArray(stored)
        ? stored.map((entry, index) => [String(index), entry])
        : Object.entries(stored);
      let migrated = !Array.isArray(stored) || parsed.v !== 3;
      for (const [storageKey, entry] of rows) {
        if (typeof entry?.voted !== "boolean") continue;
        const appId =
          typeof entry.appId === "number" && Number.isFinite(entry.appId)
            ? entry.appId
            : Number(storageKey);
        if (!Number.isFinite(appId)) continue;
        let ownerUserId: string | null;
        if (typeof entry.ownerUserId === "string") {
          ownerUserId = entry.ownerUserId;
        } else if (entry.ownerUserId === null) {
          ownerUserId = null;
        } else if (typeof entry.userId === "string") {
          ownerUserId = entry.userId;
          migrated = true;
        } else {
          ownerUserId = legacyOwner;
          migrated = true;
        }
        const normalized: VoteEntry = {
          appId,
          voted: entry.voted,
          pending: entry.pending ?? false,
          ownerUserId,
          recommendationRunId:
            typeof entry.recommendationRunId === "string" ? entry.recommendationRunId : null,
        };
        this.entries.set(this.entryKey(appId, ownerUserId), normalized);
      }
      if (migrated) this.persist(false);
    } catch {
      this.entries.clear();
    }
  }

  private persist(notify = true): void {
    try {
      this.storage.setItem(
        STORE_KEY,
        JSON.stringify({ v: 3, entries: Array.from(this.entries.values()) }),
      );
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
    return this.currentEntry(appId)?.pending ?? false;
  }

  pendingCount(): number {
    let count = 0;
    const owner = this.currentOwnerUserId();
    for (const entry of this.entries.values()) {
      if (owner !== null && entry.ownerUserId === owner && entry.pending) count += 1;
    }
    return count;
  }

  /** Flip the vote optimistically and sync. `serverVoted` is the latest known flag. */
  toggle(appId: number, serverVoted: boolean, recommendationRunId: string | null = null): void {
    const ownerUserId = this.currentOwnerUserId();
    if (!ownerUserId) return;
    const key = this.entryKey(appId, ownerUserId);
    const current = this.entries.get(key)?.voted ?? serverVoted;
    const voted = !current;
    this.entries.set(key, {
      appId,
      voted,
      pending: true,
      ownerUserId,
      recommendationRunId: voted ? recommendationRunId : null,
    });
    this.persist();
    void this.sync(key);
  }

  private sync(key: string): Promise<void> {
    const existing = this.syncPromises.get(key);
    if (existing) return existing;
    const promise = this.runSync(key).finally(() => {
      this.syncPromises.delete(key);
    });
    this.syncPromises.set(key, promise);
    return promise;
  }

  private async runSync(key: string): Promise<void> {
    while (true) {
      const entry = this.entries.get(key);
      if (!entry?.pending) return;
      if (!this.belongsToCurrentOwner(entry)) return;
      const desired = entry.voted;
      let result;
      try {
        result = await this.client.setPlayIntent(entry.appId, desired);
      } catch (error) {
        const current = this.entries.get(key);
        if (!current) return;
        if (current.voted !== desired) continue;
        // A logout/account switch freezes this owner's write for a future
        // matching session instead of attributing or deleting it under another.
        if (!this.belongsToCurrentOwner(current)) return;
        if (
          error instanceof ApiError &&
          (error.offline || error.status === 408 || error.status === 429 || error.status >= 500)
        ) {
          return; // stay pending; flush() retries
        }
        // Permanent failure: drop the optimistic override so the UI reverts.
        this.entries.delete(key);
        this.persist();
        return;
      }

      let current = this.entries.get(key);
      if (!current) return;
      if (current.voted !== desired) continue;
      if (result.voted && current.recommendationRunId) {
        try {
          await this.client.postRecommendationEvent({
            recommendationRunId: current.recommendationRunId,
            appId: current.appId,
            eventType: "play_intent",
            idempotencyKey: `play_intent:${current.appId}`,
          });
        } catch (error) {
          current = this.entries.get(key);
          if (!current || current.voted !== desired) continue;
          if (
            error instanceof ApiError &&
            (error.offline || error.status === 408 || error.status === 429 || error.status >= 500)
          ) {
            return;
          }
          // The vote itself succeeded. A permanently invalid/stale run must not
          // undo that user action; discard only its attribution context.
          current = { ...current, recommendationRunId: null };
          this.entries.set(key, current);
        }
      }
      current = this.entries.get(key);
      if (!current || current.voted !== desired) continue;
      // Keep a short-lived override until a server payload reflects the ack.
      this.entries.set(key, {
        appId: current.appId,
        voted: result.voted,
        pending: false,
        ownerUserId: current.ownerUserId,
        recommendationRunId: null,
      });
      this.persist();
      return;
    }
  }

  private reconciledEntry(appId: number, serverVoted: boolean): VoteEntry | undefined {
    const owner = this.currentOwnerUserId();
    if (!owner) return undefined;
    const key = this.entryKey(appId, owner);
    const entry = this.entries.get(key);
    if (!entry || entry.pending) return entry;
    if (entry.voted === serverVoted) {
      this.entries.delete(key);
      // Reconciliation happens while rendering; persist quietly to avoid a
      // listener-driven state update during another component's render.
      this.persist(false);
      return undefined;
    }
    return entry;
  }

  /** Retry every unsynced vote. Safe to call on reconnect. */
  async flush(): Promise<void> {
    const pending = [...this.entries.entries()]
      .filter(([, entry]) => entry.pending && this.belongsToCurrentOwner(entry))
      .map(([key]) => key);
    for (const key of pending) {
      await this.sync(key);
    }
  }

  private currentOwnerUserId(): string | null {
    return this.client.isAccountAuthenticated() ? this.client.sessionUserId() : null;
  }

  private belongsToCurrentOwner(entry: VoteEntry): boolean {
    const owner = this.currentOwnerUserId();
    return owner !== null && entry.ownerUserId === owner;
  }

  private currentEntry(appId: number): VoteEntry | undefined {
    const owner = this.currentOwnerUserId();
    return owner ? this.entries.get(this.entryKey(appId, owner)) : undefined;
  }

  private entryKey(appId: number, ownerUserId: string | null): string {
    return JSON.stringify([ownerUserId, appId]);
  }
}
