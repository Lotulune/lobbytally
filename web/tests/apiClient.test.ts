import { describe, expect, it } from "vitest";
import { ApiClient } from "../src/api/client";
import type { CommunityResponse, FeedResponse } from "../src/api/types";
import { jsonResponse, makeFetchStub, MemoryStorage, seedAccountSession, sessionBody } from "./helpers";

function feedBody(etagItems: string): FeedResponse {
  return {
    items: [
      {
        app_id: 1,
        name: etagItems,
        section: "classic_legacy",
        score: 0.9,
        confidence: 0.9,
        release_date: null,
        release_date_raw: null,
        release_date_precision: null,
        cover_url: null,
        cover_updated_at_ms: null,
        total_reviews: null,
        total_positive: null,
        latest_ccu: null,
        typical_ccu_7d: null,
        party: { recommended_min: 1, recommended_max: 4 },
        multiplayer: { dominant_mode: "private_coop" },
        play_intent: { count: 0, voted: false },
        reasons: ["r"],
        cautions: [],
        evidence_ids: [],
        components: { friend_fit: 0.9, section_score: 0.9, personalized_score: 0.9, final_score: 0.9 },
        algorithm_version: "rules-0.1.0",
      },
    ],
    next_cursor: null,
    total: 1,
    limit: 20,
    offset: 0,
    page: 1,
    total_pages: 1,
    snapshot_at_ms: 1,
    algorithm_version: "rules-0.1.0",
    data_updated_at_ms: 1,
  };
}

function communityBody(): CommunityResponse {
  return {
    items: [],
    next_cursor: null,
    snapshot_revision: 1,
    data_updated_at_ms: 1,
  };
}

describe("ApiClient session", () => {
  it("bootstraps an anonymous session for browsing-only natural-language calls", async () => {
    const storage = new MemoryStorage();
    const { fetchFn, calls } = makeFetchStub({
      "POST /v1/session/anonymous": () => jsonResponse(sessionBody({ account: false })),
      "POST /v1/recommendations/natural-language": (call) => {
        expect(call.headers.authorization).toBe("Bearer access-1");
        return jsonResponse({
          query: "4 人合作",
          interpreted: { party_size: 4, session_minutes_max: null, coop_competitive: 0.2 },
          items: [], ai_status: "fallback", fallback_reason: "AI unavailable",
          algorithm_version: "rules-0.1.0", data_updated_at_ms: 1,
        });
      },
    });
    const client = new ApiClient({ baseUrl: "http://x", fetchFn, storage });
    await client.naturalLanguageRecommendations("4 人合作");
    expect(calls.some((c) => c.url.endsWith("/v1/session/anonymous"))).toBe(true);
    expect(client.hasSession()).toBe(true);
    expect(client.isAccountAuthenticated()).toBe(false);
  });

  it("refreshes then retries once on a 401", async () => {
    const storage = new MemoryStorage();
    seedAccountSession(storage, { access_token: "old" });
    let prefsCalls = 0;
    const { fetchFn } = makeFetchStub({
      "POST /v1/auth/refresh": () => jsonResponse(sessionBody({ access_token: "new", account: true })),
      "GET /v1/preferences": (call) => {
        prefsCalls += 1;
        if (call.headers.authorization === "Bearer new") {
          return jsonResponse({ version: 2 });
        }
        return jsonResponse({ error: { code: "unauthenticated", message: "nope" } }, {
          status: 401,
        });
      },
    });
    const client = new ApiClient({ baseUrl: "http://x", fetchFn, storage });
    const result = await client.getPreferences();
    expect((result as { version: number }).version).toBe(2);
    expect(prefsCalls).toBe(2);
  });

  it("preserves the identity when refresh fails transiently", async () => {
    const storage = new MemoryStorage();
    storage.setItem(
      "mpgs.session.v1",
      JSON.stringify(sessionBody({ access_token: "expired", expires_at_ms: 0, user_id: "u_old", account: false })),
    );
    const { fetchFn, calls } = makeFetchStub({
      "POST /v1/session/refresh": () =>
        jsonResponse(
          { error: { code: "temporarily_unavailable", message: "retry later" } },
          { status: 503 },
        ),
      "POST /v1/session/anonymous": () => jsonResponse(sessionBody({ user_id: "u_new" })),
    });
    const client = new ApiClient({ baseUrl: "http://x", fetchFn, storage });

    await expect(client.ensureSession()).rejects.toMatchObject({ status: 503 });
    expect(client.sessionUserId()).toBe("u_old");
    expect(calls.some((call) => call.url.endsWith("/v1/session/anonymous"))).toBe(false);
    expect(JSON.parse(storage.getItem("mpgs.session.v1") ?? "{}").user_id).toBe("u_old");
  });

  it("creates a new identity when the refresh token is explicitly rejected", async () => {
    const storage = new MemoryStorage();
    storage.setItem(
      "mpgs.session.v1",
      JSON.stringify(sessionBody({ access_token: "expired", expires_at_ms: 0, user_id: "u_old", account: false })),
    );
    const { fetchFn } = makeFetchStub({
      "POST /v1/session/refresh": () =>
        jsonResponse({ error: { code: "unauthenticated", message: "expired" } }, { status: 401 }),
      "POST /v1/session/anonymous": () => jsonResponse(sessionBody({ user_id: "u_new" })),
    });
    const client = new ApiClient({ baseUrl: "http://x", fetchFn, storage });

    await client.ensureSession();
    expect(client.sessionUserId()).toBe("u_new");
  });

  it("sends a stable x-device-id header", async () => {
    const storage = new MemoryStorage();
    const { fetchFn, calls } = makeFetchStub({
      "GET /v1/meta": () => jsonResponse({ api_version: "v1" }),
    });
    const client = new ApiClient({ baseUrl: "http://x", fetchFn, storage });
    await client.meta();
    await client.meta();
    const ids = calls.map((c) => c.headers["x-device-id"]);
    expect(ids[0]).toBeDefined();
    expect(new Set(ids).size).toBe(1);
  });
});

describe("ApiClient ETag cache", () => {
  it("revalidates with If-None-Match and serves cached data on 304", async () => {
    const storage = new MemoryStorage();
    let hits = 0;
    const { fetchFn } = makeFetchStub({
      "GET /v1/feeds/classic_legacy": (call) => {
        hits += 1;
        if (call.headers["if-none-match"] === '"v1"') {
          return new Response(null, { status: 304 });
        }
        return jsonResponse(feedBody("first"), { headers: { etag: '"v1"' } });
      },
    });
    const client = new ApiClient({ baseUrl: "http://x", fetchFn, storage });
    const a = await client.feed("classic_legacy");
    expect(a.data.items[0]?.name).toBe("first");
    expect(a.fromOfflineCache).toBe(false);
    const b = await client.feed("classic_legacy");
    expect(b.data.items[0]?.name).toBe("first"); // from cache after 304
    expect(hits).toBe(2);
  });

  it("falls back to the offline snapshot when the network fails", async () => {
    const storage = new MemoryStorage();
    let online = true;
    const { fetchFn } = makeFetchStub({
      "GET /v1/feeds/classic_legacy": () => {
        if (!online) throw new TypeError("network down");
        return jsonResponse(feedBody("cached"), { headers: { etag: '"v1"' } });
      },
    });
    const client = new ApiClient({ baseUrl: "http://x", fetchFn, storage });
    await client.feed("classic_legacy");
    online = false;
    const offlineResult = await client.feed("classic_legacy");
    expect(offlineResult.fromOfflineCache).toBe(true);
    expect(offlineResult.data.items[0]?.name).toBe("cached");
  });

  it("rethrows when offline and no snapshot exists", async () => {
    const storage = new MemoryStorage();
    const { fetchFn } = makeFetchStub({
      "GET /v1/feeds/upcoming": () => {
        throw new TypeError("network down");
      },
    });
    const client = new ApiClient({ baseUrl: "http://x", fetchFn, storage });
    await expect(client.feed("upcoming")).rejects.toMatchObject({ offline: true });
  });

  it("clears only cached responses, preserving session/device/feedback keys", async () => {
    const storage = new MemoryStorage();
    const { fetchFn } = makeFetchStub({
      "GET /v1/feeds/classic_legacy": () =>
        jsonResponse(feedBody("cached"), { headers: { etag: '"v1"' } }),
    });
    const client = new ApiClient({ baseUrl: "http://x", fetchFn, storage });
    client.deviceId(); // writes mpgs.device.v1
    storage.setItem("mpgs.session.v1", "{}");
    storage.setItem("mpgs.feedback.v1", "{}");
    await client.feed("classic_legacy"); // writes a mpgs.cache.v1:* entry

    const removed = client.clearCachedResponses();
    expect(removed).toBeGreaterThan(0);
    expect(storage.getItem("mpgs.session.v1")).toBe("{}");
    expect(storage.getItem("mpgs.feedback.v1")).toBe("{}");
    expect(storage.getItem("mpgs.device.v1")).not.toBeNull();
    // A subsequent feed load must re-fetch (cache gone) rather than 304.
    const after = await client.feed("classic_legacy");
    expect(after.fromOfflineCache).toBe(false);
  });

  it("keeps community filter snapshots in separate cache scopes", async () => {
    const storage = new MemoryStorage();
    const { fetchFn, calls } = makeFetchStub({
      "GET /v1/community/play-intents": () =>
        jsonResponse(communityBody(), { headers: { etag: '"community"' } }),
    });
    const client = new ApiClient({ baseUrl: "http://x", fetchFn, storage });

    await client.community("trending", {
      releaseState: "released",
      platform: "windows",
      partySize: 4,
    });
    await client.community("trending", { releaseState: "upcoming", demoOnly: true });

    expect(calls).toHaveLength(2);
    expect(calls[0]?.url).toContain("release_state=released");
    expect(calls[0]?.url).toContain("platform=windows");
    expect(calls[0]?.url).toContain("party_size=4");
    expect(calls[1]?.url).toContain("release_state=upcoming");
    expect(calls[1]?.url).toContain("demo_only=true");
    expect(calls[1]?.headers["if-none-match"]).toBeUndefined();
  });
});

describe("ApiClient natural-language recommendations", () => {
  it("posts the query and exposes deterministic fallback metadata", async () => {
    const storage = new MemoryStorage();
    const { fetchFn, calls } = makeFetchStub({
      "POST /v1/session/anonymous": () => jsonResponse(sessionBody({ account: false })),
      "POST /v1/recommendations/natural-language": () =>
        jsonResponse({
          query: "4 人合作",
          interpreted: { party_size: 4, session_minutes_max: null, coop_competitive: 0.2 },
          items: [],
          ai_status: "fallback",
          fallback_reason: "AI unavailable",
          algorithm_version: "rules-0.1.0",
          data_updated_at_ms: 1,
        }),
    });
    const client = new ApiClient({ baseUrl: "http://x", fetchFn, storage });

    const response = await client.naturalLanguageRecommendations("4 人合作");

    expect(response.ai_status).toBe("fallback");
    const request = calls.find((call) => call.url.endsWith("/v1/recommendations/natural-language"));
    expect(request?.body).toEqual({ query: "4 人合作", limit: 6 });
    expect(request?.headers.authorization).toBe("Bearer access-1");
  });

  it("sends a device-local custom key only with the live recommendation request", async () => {
    const storage = new MemoryStorage();
    seedAccountSession(storage);
    const { fetchFn, calls } = makeFetchStub({
      "POST /v1/recommendations/natural-language": () =>
        jsonResponse({
          query: "自建服合作",
          interpreted: {},
          items: [],
          ai_status: "used",
          algorithm_version: "rules-0.2.0",
          data_updated_at_ms: 1,
        }),
    });
    const client = new ApiClient({ baseUrl: "http://x", fetchFn, storage });

    await client.naturalLanguageRecommendations("自建服合作", 6, {
      provider: "openai_compat",
      baseUrl: "https://provider.example/v1",
      model: "model-a",
      apiKey: "local-secret",
    });

    const request = calls[0];
    expect(request?.body).toEqual({
      query: "自建服合作",
      limit: 6,
      custom_ai: {
        provider: "openai_compat",
        base_url: "https://provider.example/v1",
        model: "model-a",
        api_key: "local-secret",
        multi_model: false,
      },
    });
  });
});
