import { describe, expect, it } from "vitest";
import { ApiClient } from "../src/api/client";
import { FeedbackQueue } from "../src/api/feedbackQueue";
import { deleteServiceNamespace, ScopedStorage } from "../src/api/scopedStorage";
import { activateServiceOrigin, serviceNamespacePrefix } from "../src/api/serverOrigin";
import { jsonResponse, makeFetchStub, seedAccountSession, sessionBody } from "./helpers";
import { MemoryStorage } from "./helpers";

const ORIGIN_A = "https://a.example.com";
const ORIGIN_B = "https://b.example.com";

describe("ScopedStorage", () => {
  it("isolates reads and writes per origin", () => {
    const base = new MemoryStorage();
    const a = new ScopedStorage(base, ORIGIN_A);
    const b = new ScopedStorage(base, ORIGIN_B);

    a.setItem("mpgs.session.v1", "token-a");
    b.setItem("mpgs.session.v1", "token-b");

    expect(a.getItem("mpgs.session.v1")).toBe("token-a");
    expect(b.getItem("mpgs.session.v1")).toBe("token-b");
    // The underlying store keeps the namespaced physical keys.
    expect(base.getItem(serviceNamespacePrefix(ORIGIN_A) + "mpgs.session.v1")).toBe("token-a");
  });

  it("enumerates only its own namespace with unprefixed keys", () => {
    const base = new MemoryStorage();
    const a = new ScopedStorage(base, ORIGIN_A);
    const b = new ScopedStorage(base, ORIGIN_B);
    a.setItem("k1", "1");
    a.setItem("k2", "2");
    b.setItem("k3", "3");
    base.setItem("mpgs.global.v1", "g");

    expect(a.length).toBe(2);
    const keys = [a.key(0), a.key(1)].sort();
    expect(keys).toEqual(["k1", "k2"]);
  });

  it("deleteServiceNamespace removes only the target origin's data", () => {
    const base = new MemoryStorage();
    const a = new ScopedStorage(base, ORIGIN_A);
    const b = new ScopedStorage(base, ORIGIN_B);
    a.setItem("k1", "1");
    a.setItem("k2", "2");
    b.setItem("k3", "3");

    const removed = deleteServiceNamespace(base, ORIGIN_A);
    expect(removed).toBe(2);
    expect(a.getItem("k1")).toBeNull();
    expect(b.getItem("k3")).toBe("3");
  });
});

describe("per-service isolation (PRD CS-007 / CS-008)", () => {
  it("a session saved under origin A is invisible under origin B", async () => {
    const base = new MemoryStorage();
    const storageA = new ScopedStorage(base, ORIGIN_A);
    const storageB = new ScopedStorage(base, ORIGIN_B);
    seedAccountSession(storageA);

    const clientA = new ApiClient({ baseUrl: ORIGIN_A, storage: storageA });
    const clientB = new ApiClient({ baseUrl: ORIGIN_B, storage: storageB });

    expect(clientA.isAccountAuthenticated()).toBe(true);
    expect(clientB.hasSession()).toBe(false);
    expect(clientB.isAccountAuthenticated()).toBe(false);
  });

  it("the old service token is never sent to the new service (CS-008)", async () => {
    const base = new MemoryStorage();
    seedAccountSession(new ScopedStorage(base, ORIGIN_A));

    // After a service switch the app rebuilds the client against the new
    // origin and its namespace: no session is found, so no Authorization
    // header can leak into requests bound for ORIGIN_B.
    const { fetchFn, calls } = makeFetchStub({
      "GET /v1/meta": () => jsonResponse({ api_version: "v1" }),
    });
    const clientB = new ApiClient({
      baseUrl: ORIGIN_B,
      storage: new ScopedStorage(base, ORIGIN_B),
      fetchFn,
    });
    await clientB.meta();

    expect(calls).toHaveLength(1);
    expect(calls[0]!.url.startsWith(ORIGIN_B)).toBe(true);
    expect(calls[0]!.headers.authorization).toBeUndefined();
  });

  it("pending feedback queued under origin A never replays against origin B", async () => {
    const base = new MemoryStorage();
    const storageA = new ScopedStorage(base, ORIGIN_A);
    seedAccountSession(storageA);

    const posted: string[] = [];
    const { fetchFn } = makeFetchStub({
      "POST /v1/feedback": (call) => {
        posted.push(call.url);
        return jsonResponse({ feedback_id: 1 });
      },
    });
    const clientA = new ApiClient({ baseUrl: ORIGIN_A, storage: storageA, fetchFn });
    const queueA = new FeedbackQueue(clientA, storageA);
    queueA.submit(42, "like");
    await queueA.flush();
    expect(posted.every((url) => url.startsWith(ORIGIN_A))).toBe(true);

    // Switch to origin B: the queue for B starts empty, nothing to replay.
    const clientB = new ApiClient({ baseUrl: ORIGIN_B, storage: new ScopedStorage(base, ORIGIN_B) });
    const queueB = new FeedbackQueue(clientB, new ScopedStorage(base, ORIGIN_B));
    expect(queueB.pendingCount()).toBe(0);
    await queueB.flush();
    expect(posted.every((url) => url.startsWith(ORIGIN_A))).toBe(true);
  });

  it("switching back restores the old origin's session and cache (PRD §5.3)", async () => {
    const base = new MemoryStorage();
    activateServiceOrigin(base, ORIGIN_A);
    const storageA = new ScopedStorage(base, ORIGIN_A);
    storageA.setItem("mpgs.session.v1", JSON.stringify(sessionBody({ account: false })));

    // "switch" to B and back: A's namespace is untouched.
    activateServiceOrigin(base, ORIGIN_B);
    activateServiceOrigin(base, ORIGIN_A);
    const restored = new ApiClient({ baseUrl: ORIGIN_A, storage: new ScopedStorage(base, ORIGIN_A) });
    expect(restored.hasSession()).toBe(true);
  });
});
