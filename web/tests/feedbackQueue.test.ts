import { describe, expect, it } from "vitest";
import { ApiClient } from "../src/api/client";
import { FeedbackQueue } from "../src/api/feedbackQueue";
import { jsonResponse, makeFetchStub, MemoryStorage, seedAccountSession, sessionBody } from "./helpers";

function accountClient(storage: MemoryStorage, fetchFn: typeof fetch) {
  if (!storage.getItem("mpgs.session.v1")) seedAccountSession(storage);
  return new ApiClient({ baseUrl: "http://x", fetchFn, storage });
}

function feedbackRecord(id: number, appId: number, type: string) {
  return jsonResponse(
    {
      feedback_id: id,
      app_id: appId,
      type,
      recommendation_run_id: null,
      created_at_ms: Date.now(),
    },
    { status: 201 },
  );
}

function makeClient(storage: MemoryStorage, online: () => boolean) {
  const { fetchFn, calls } = makeFetchStub({
    "POST /v1/session/anonymous": () => jsonResponse(sessionBody()),
    "POST /v1/feedback": (call) => {
      if (!online()) throw new TypeError("offline");
      const body = call.body as { app_id: number; type: string };
      return feedbackRecord(calls.length, body.app_id, body.type);
    },
    "POST /v1/feedback/10/undo": () =>
      jsonResponse({ feedback_id: 10, app_id: 1, type: "like", recommendation_run_id: null, created_at_ms: 1 }),
  });
  const client = accountClient(storage, fetchFn);
  return { client, calls };
}

describe("FeedbackQueue", () => {
  it("optimistically records and syncs feedback", async () => {
    const storage = new MemoryStorage();
    const { client } = makeClient(storage, () => true);
    const queue = new FeedbackQueue(client, storage);
    queue.submit(548430, "like");
    // active immediately, before network settles
    expect(queue.activeByApp().get(548430)?.type).toBe("like");
    await queue.flush();
    const entry = queue.activeByApp().get(548430);
    expect(entry?.feedbackId).not.toBeNull();
  });

  it("keeps unsynced feedback across a simulated reload and replays it", async () => {
    const storage = new MemoryStorage();
    let online = false;
    const first = makeClient(storage, () => online);
    const queueA = new FeedbackQueue(first.client, storage);
    queueA.submit(1, "like");
    await queueA.flush();
    expect(queueA.pendingCount()).toBe(1); // still offline

    // "reload": new queue from the same storage
    online = true;
    const second = makeClient(storage, () => online);
    const queueB = new FeedbackQueue(second.client, storage);
    expect(queueB.pendingCount()).toBe(1);
    await queueB.flush();
    expect(queueB.pendingCount()).toBe(0);
  });

  it("cancels an unsynced entry locally on undo without calling the API", async () => {
    const storage = new MemoryStorage();
    const { client, calls } = makeClient(storage, () => false);
    const queue = new FeedbackQueue(client, storage);
    const entry = queue.submit(1, "like");
    await queue.flush(); // fails offline, remains pending, feedbackId null
    await queue.undo(entry.localId);
    expect(queue.activeByApp().has(1)).toBe(false);
    expect(calls.some((c) => c.url.includes("/undo"))).toBe(false);
  });

  it("does not resurrect cancelled feedback on later flushes", async () => {
    const storage = new MemoryStorage();
    let online = false;
    const { client } = makeClient(storage, () => online);
    const queue = new FeedbackQueue(client, storage);
    const entry = queue.submit(1, "like");
    await queue.undo(entry.localId); // cancelled while unsynced
    online = true;
    await queue.flush();
    expect(queue.activeByApp().has(1)).toBe(false);
  });

  it("sends a compensating undo when cancelled while POST is in flight", async () => {
    const storage = new MemoryStorage();
    let resolvePost!: (response: Response) => void;
    let postStarted!: () => void;
    const started = new Promise<void>((resolve) => {
      postStarted = resolve;
    });
    const postResponse = new Promise<Response>((resolve) => {
      resolvePost = resolve;
    });
    const { fetchFn, calls } = makeFetchStub({
      "POST /v1/session/anonymous": () => jsonResponse(sessionBody()),
      "POST /v1/feedback": () => {
        postStarted();
        return postResponse;
      },
      "POST /v1/feedback/77/undo": () =>
        jsonResponse({ feedback_id: 77, app_id: 1, type: "like", created_at_ms: 1 }),
    });
    const queue = new FeedbackQueue(accountClient(storage, fetchFn), storage);
    const entry = queue.submit(1, "like");
    await started;

    await queue.undo(entry.localId);
    resolvePost(feedbackRecord(77, 1, "like"));
    await queue.flush();

    expect(calls.some((call) => call.url.endsWith("/v1/feedback/77/undo"))).toBe(true);
    expect(queue.pendingCount()).toBe(0);
    expect(queue.activeByApp().has(1)).toBe(false);
  });

  it("retries a transient server rejection instead of cancelling feedback", async () => {
    const storage = new MemoryStorage();
    let attempts = 0;
    const { fetchFn } = makeFetchStub({
      "POST /v1/session/anonymous": () => jsonResponse(sessionBody()),
      "POST /v1/feedback": () => {
        attempts += 1;
        return attempts === 1
          ? jsonResponse({ error: { code: "temporarily_unavailable", message: "busy" } }, { status: 503 })
          : feedbackRecord(88, 1, "like");
      },
    });
    const queue = new FeedbackQueue(accountClient(storage, fetchFn), storage);
    queue.submit(1, "like");
    await queue.flush();
    expect(queue.pendingCount()).toBe(1);
    expect(queue.snapshot()[0]?.cancelled).toBe(false);

    await queue.flush();
    expect(attempts).toBe(2);
    expect(queue.pendingCount()).toBe(0);
  });

  it("keeps a transiently rejected undo pending for replay", async () => {
    const storage = new MemoryStorage();
    let undoAttempts = 0;
    const { fetchFn } = makeFetchStub({
      "POST /v1/session/anonymous": () => jsonResponse(sessionBody()),
      "POST /v1/feedback": () => feedbackRecord(10, 1, "like"),
      "POST /v1/feedback/10/undo": () => {
        undoAttempts += 1;
        return undoAttempts === 1
          ? jsonResponse({ error: { code: "internal", message: "busy" } }, { status: 500 })
          : jsonResponse({ feedback_id: 10, app_id: 1, type: "like", created_at_ms: 1 });
      },
    });
    const queue = new FeedbackQueue(accountClient(storage, fetchFn), storage);
    const entry = queue.submit(1, "like");
    await queue.flush();

    await queue.undo(entry.localId);
    expect(queue.pendingCount()).toBe(1);
    await queue.flush();
    expect(undoAttempts).toBe(2);
    expect(queue.pendingCount()).toBe(0);
  });

  it("notifies ranking consumers after server-acknowledged feedback and undo", async () => {
    const storage = new MemoryStorage();
    const { fetchFn } = makeFetchStub({
      "POST /v1/session/anonymous": () => jsonResponse(sessionBody()),
      "POST /v1/feedback": () => feedbackRecord(10, 1, "like"),
      "POST /v1/feedback/10/undo": () =>
        jsonResponse({ feedback_id: 10, app_id: 1, type: "like", created_at_ms: 1 }),
    });
    const queue = new FeedbackQueue(accountClient(storage, fetchFn), storage);
    let changes = 0;
    queue.subscribeRankingChanged(() => {
      changes += 1;
    });

    const entry = queue.submit(1, "like");
    await queue.flush();
    expect(changes).toBe(1);

    await queue.undo(entry.localId);
    expect(changes).toBe(2);
  });

  it("quarantines account A feedback instead of replaying it as account B", async () => {
    const storage = new MemoryStorage();
    storage.setItem(
      "mpgs.session.v1",
      JSON.stringify(sessionBody({ user_id: "u_a", access_token: "token-a" })),
    );
    const { fetchFn: fetchA } = makeFetchStub({
      "POST /v1/feedback": () => {
        throw new TypeError("offline");
      },
    });
    const queueA = new FeedbackQueue(
      new ApiClient({ baseUrl: "http://x", fetchFn: fetchA, storage }),
      storage,
    );
    queueA.submit(7, "like");
    await queueA.flush();
    expect(queueA.pendingCount()).toBe(1);

    storage.setItem(
      "mpgs.session.v1",
      JSON.stringify(sessionBody({ user_id: "u_b", access_token: "token-b" })),
    );
    const { fetchFn: fetchB, calls: callsB } = makeFetchStub({
      "POST /v1/feedback": () => feedbackRecord(99, 7, "like"),
    });
    const queueB = new FeedbackQueue(
      new ApiClient({ baseUrl: "http://x", fetchFn: fetchB, storage }),
      storage,
    );

    await queueB.flush();

    expect(callsB.filter((call) => call.url.endsWith("/v1/feedback"))).toHaveLength(0);
    expect(queueB.pendingCount()).toBe(0);
    expect(queueB.snapshot()).toEqual(
      expect.arrayContaining([expect.objectContaining({ ownerUserId: "u_a", cancelled: false })]),
    );
  });

  it("adopts a legacy ownerless queue only when the persisted account is present", () => {
    const storage = new MemoryStorage();
    seedAccountSession(storage, { user_id: "legacy-owner" });
    storage.setItem(
      "mpgs.feedback.v1",
      JSON.stringify({
        entries: [
          {
            localId: "legacy-local",
            appId: 4,
            type: "like",
            idempotencyKey: "legacy-idem",
            clientCreatedAtMs: 1,
            feedbackId: null,
            cancelled: false,
            undone: false,
            syncError: null,
          },
        ],
      }),
    );
    const { fetchFn } = makeFetchStub({});
    const queue = new FeedbackQueue(
      new ApiClient({ baseUrl: "http://x", fetchFn, storage }),
      storage,
    );

    expect(queue.snapshot()[0]?.ownerUserId).toBe("legacy-owner");
    expect(storage.getItem("mpgs.feedback.v1")).toContain('"ownerUserId":"legacy-owner"');
  });
});
