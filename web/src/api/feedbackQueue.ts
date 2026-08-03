// Pending-feedback queue: optimistic local record + replay when connectivity returns.
// Stored separately from the response cache so clearing cached responses can never
// drop unsynced user feedback (DEVELOPMENT.md §11).

import { ApiClient, ApiError, newIdempotencyKey } from "./client";
import { getServiceStorage } from "./storage";
import type { FeedbackRecord, FeedbackType, StorageLike } from "./types";

const QUEUE_KEY = "mpgs.feedback.v1";

export interface PendingFeedback {
  localId: string;
  /** Account that created this write; null quarantines an unowned legacy record. */
  ownerUserId: string | null;
  appId: number;
  type: FeedbackType;
  recommendationRunId?: string | null;
  idempotencyKey: string;
  clientCreatedAtMs: number;
  /** Set once the server acknowledged the write. */
  feedbackId: number | null;
  /** The POST started, so a transport failure may have hidden a committed write. */
  submissionAttempted?: boolean;
  /** True when the user undid the feedback before it ever synced. */
  cancelled: boolean;
  /** Server-acknowledged undo. */
  undone: boolean;
  syncError: string | null;
}

export type FeedbackListener = (entries: PendingFeedback[]) => void;

export interface ActiveFeedbackDimensions {
  sentiment: PendingFeedback | null;
  ownership: PendingFeedback | null;
  reasons: PendingFeedback[];
}

interface QueueStorageShape {
  entries: Array<Omit<PendingFeedback, "ownerUserId"> & { ownerUserId?: string | null }>;
}

function isRetryable(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    (error.offline ||
      error.status === 408 ||
      error.status === 429 ||
      error.status >= 500 ||
      error.code === "rate_limited" ||
      error.code === "temporarily_unavailable" ||
      error.code === "internal")
  );
}

export class FeedbackQueue {
  private readonly client: ApiClient;
  private readonly storage: StorageLike;
  private entries: PendingFeedback[] = [];
  private listeners = new Set<FeedbackListener>();
  private rankingListeners = new Set<() => void>();
  /** In-flight flush; concurrent callers await the same promise. */
  private flushPromise: Promise<void> | null = null;

  constructor(client: ApiClient, storage: StorageLike = getServiceStorage()) {
    this.client = client;
    this.storage = storage;
    this.load();
  }

  private load(): void {
    try {
      const raw = this.storage.getItem(QUEUE_KEY);
      if (!raw) return;
      const parsed = JSON.parse(raw) as QueueStorageShape;
      if (Array.isArray(parsed.entries)) {
        const legacyOwner = this.currentOwnerUserId();
        let migrated = false;
        this.entries = parsed.entries.map((entry) => {
          if (entry.ownerUserId !== undefined) {
            return {
              ...entry,
              ownerUserId:
                typeof entry.ownerUserId === "string" ? entry.ownerUserId : null,
            };
          }
          migrated = true;
          // A legacy queue loaded while its account session is still present can
          // be safely adopted. Logged-out legacy records remain quarantined.
          return { ...entry, ownerUserId: legacyOwner };
        });
        if (migrated) this.persist();
      }
    } catch {
      this.entries = [];
    }
  }

  private persist(): void {
    try {
      this.storage.setItem(QUEUE_KEY, JSON.stringify({ entries: this.entries }));
    } catch {
      // Persist failures leave the in-memory queue intact for this run.
    }
    const snapshot = this.snapshot();
    for (const listener of this.listeners) listener(snapshot);
  }

  subscribe(listener: FeedbackListener): () => void {
    this.listeners.add(listener);
    listener(this.snapshot());
    return () => this.listeners.delete(listener);
  }

  /** Fires only after a feedback or undo mutation is acknowledged by the server. */
  subscribeRankingChanged(listener: () => void): () => void {
    this.rankingListeners.add(listener);
    return () => this.rankingListeners.delete(listener);
  }

  private notifyRankingChanged(): void {
    for (const listener of this.rankingListeners) listener();
  }

  snapshot(): PendingFeedback[] {
    return this.entries.map((e) => ({ ...e }));
  }

  private currentOwnerUserId(): string | null {
    return this.client.isAccountAuthenticated() ? this.client.sessionUserId() : null;
  }

  private belongsToCurrentOwner(entry: PendingFeedback): boolean {
    const owner = this.currentOwnerUserId();
    return owner !== null && entry.ownerUserId === owner;
  }

  /** Latest effective (not cancelled/undone) feedback per app, for optimistic UI. */
  activeByApp(): Map<number, PendingFeedback> {
    const map = new Map<number, PendingFeedback>();
    for (const entry of this.entries) {
      if (!this.belongsToCurrentOwner(entry)) continue;
      if (entry.cancelled || entry.undone) continue;
      map.set(entry.appId, entry);
    }
    return map;
  }

  /**
   * Effective feedback split by the server's independent dimensions. A newer
   * sentiment replaces only the previous sentiment; ownership and each reason
   * remain independently reversible.
   */
  activeDimensionsForApp(appId: number): ActiveFeedbackDimensions {
    let sentiment: PendingFeedback | null = null;
    let ownership: PendingFeedback | null = null;
    const reasons = new Map<FeedbackType, PendingFeedback>();
    for (const entry of this.entries) {
      if (!this.belongsToCurrentOwner(entry) || entry.appId !== appId) continue;
      if (entry.cancelled || entry.undone) continue;
      if (entry.type === "like" || entry.type === "not_interested") {
        sentiment = entry;
      } else if (entry.type === "played") {
        ownership = entry;
      } else {
        reasons.set(entry.type, entry);
      }
    }
    return {
      sentiment: sentiment ? { ...sentiment } : null,
      ownership: ownership ? { ...ownership } : null,
      reasons: Array.from(reasons.values(), (entry) => ({ ...entry })),
    };
  }

  /** Record feedback locally and try to sync immediately. Returns the local entry. */
  submit(
    appId: number,
    type: FeedbackType,
    recommendationRunId: string | null = null,
  ): PendingFeedback {
    const ownerUserId = this.currentOwnerUserId();
    if (!ownerUserId) {
      throw new ApiError({ code: "unauthenticated", status: 401, message: "sign in to continue" });
    }
    const entry: PendingFeedback = {
      localId: newIdempotencyKey(),
      ownerUserId,
      appId,
      type,
      recommendationRunId,
      idempotencyKey: newIdempotencyKey(),
      clientCreatedAtMs: Date.now(),
      feedbackId: null,
      submissionAttempted: false,
      cancelled: false,
      undone: false,
      syncError: null,
    };
    this.entries.push(entry);
    this.persist();
    void this.flush();
    return { ...entry };
  }

  /** Undo by local id. Unsynced entries are cancelled locally; synced ones call the API. */
  async undo(localId: string): Promise<void> {
    const entry = this.entries.find((e) => e.localId === localId);
    if (!entry || !this.belongsToCurrentOwner(entry) || entry.cancelled || entry.undone) return;
    if (entry.feedbackId === null) {
      entry.cancelled = true;
      this.persist();
      return;
    }
    entry.undone = true;
    entry.syncError = "undo_pending";
    this.persist();
    try {
      await this.client.undoFeedback(entry.feedbackId);
      entry.syncError = null;
      this.notifyRankingChanged();
    } catch (error) {
      if (!isRetryable(error)) {
        entry.undone = false;
        entry.syncError = error instanceof ApiError ? error.code : "unknown";
        this.persist();
        throw error;
      }
    }
    this.persist();
  }

  /** Number of entries still waiting for server acknowledgement. */
  pendingCount(): number {
    return this.entries.filter(
      (e) =>
        this.belongsToCurrentOwner(e) &&
        ((!e.cancelled && e.feedbackId === null) ||
          (e.cancelled && e.feedbackId === null && e.submissionAttempted === true) ||
          e.syncError === "undo_pending"),
    ).length;
  }

  /** Push all unsynced entries to the server. Safe to call repeatedly. */
  async flush(): Promise<void> {
    if (this.flushPromise) return this.flushPromise;
    this.flushPromise = this.runFlush().finally(() => {
      this.flushPromise = null;
    });
    return this.flushPromise;
  }

  private async runFlush(): Promise<void> {
    for (const entry of this.entries) {
      if (!this.belongsToCurrentOwner(entry)) continue;
      if (entry.cancelled && entry.feedbackId === null && !entry.submissionAttempted) continue;
      if (entry.feedbackId === null) {
        try {
          entry.submissionAttempted = true;
          this.persist();
          const record: FeedbackRecord = await this.client.postFeedback({
            appId: entry.appId,
            type: entry.type,
            idempotencyKey: entry.idempotencyKey,
            clientCreatedAtMs: entry.clientCreatedAtMs,
            recommendationRunId: entry.recommendationRunId ?? null,
          });
          entry.feedbackId = record.feedback_id;
          if (entry.cancelled) entry.undone = true;
          entry.syncError = entry.undone ? "undo_pending" : null;
          if (!entry.cancelled && this.belongsToCurrentOwner(entry)) this.notifyRankingChanged();
        } catch (error) {
          if (!this.belongsToCurrentOwner(entry)) {
            // The request belongs to the account that started it. Preserve the
            // record for that account if the active identity changed meanwhile.
            continue;
          }
          if (isRetryable(error)) {
            entry.syncError = error instanceof ApiError ? error.code : "unknown";
            break; // retry later without hammering the same unavailable service
          }
          // Permanent rejection: keep the record but stop retrying it.
          entry.syncError = error instanceof ApiError ? error.code : "unknown";
          entry.cancelled = true;
        }
      }
      if (
        this.belongsToCurrentOwner(entry) &&
        entry.feedbackId !== null &&
        entry.undone &&
        entry.syncError === "undo_pending"
      ) {
        try {
          await this.client.undoFeedback(entry.feedbackId);
          entry.syncError = null;
          if (this.belongsToCurrentOwner(entry)) this.notifyRankingChanged();
        } catch (error) {
          if (!this.belongsToCurrentOwner(entry)) continue;
          if (isRetryable(error)) {
            entry.syncError = "undo_pending";
            break;
          }
          entry.syncError = error instanceof ApiError ? error.code : "unknown";
        }
      }
    }
    // Drop fully-settled entries older than 7 days to bound storage.
    const cutoff = Date.now() - 7 * 24 * 60 * 60 * 1000;
    this.entries = this.entries.filter(
      (e) =>
        !(e.clientCreatedAtMs < cutoff && (e.cancelled || (e.feedbackId !== null && e.syncError === null))),
    );
    this.persist();
  }
}
