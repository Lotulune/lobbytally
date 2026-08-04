// Typed HTTP client for the MPGS public API.
//
// Responsibilities:
// - anonymous session bootstrap + refresh-on-401 (single-flight)
// - ETag revalidation with a durable client snapshot cache (offline browsing)
// - stable error envelope parsing -> ApiError
// - x-device-id header for rate limiting fairness
//
// The client never touches server keys; everything here ships inside the desktop bundle.

import type {
  AccountProfile,
  AiSettings,
  CalendarResponse,
  CalendarPeriod,
  CommunityResponse,
  CommunityFilters,
  CommunitySort,
  DataStatusResponse,
  ErrorEnvelope,
  EvidenceResponse,
  FeedbackRecord,
  FeedbackType,
  FeedResponse,
  FeedSection,
  FeedSort,
  FeedSortOrder,
  GameDetail,
  MetaResponse,
  NaturalLanguageRecommendationResponse,
  PipelineAppPresence,
  PlayIntentResult,
  RecommendationEventType,
  SearchResponse,
  SessionTokens,
  StorageLike,
  UserPreferences,
} from "./types";
import { getClientStorage } from "./storage";

const SESSION_KEY = "mpgs.session.v1";
const DEVICE_KEY = "mpgs.device.v1";
// Bump when cached feed/detail payload shape or ranking semantics change.
const CACHE_PREFIX = "mpgs.cache.v3:";

export type ApiErrorCode =
  | "account_conflict"
  | "ai_connection_failed"
  | "invalid_argument"
  | "invalid_avatar"
  | "merge_choice_required"
  | "unauthenticated"
  | "forbidden"
  | "not_found"
  | "version_conflict"
  | "cursor_stale"
  | "unsupported_constraint"
  | "rate_limited"
  | "internal"
  | "temporarily_unavailable"
  | "network"
  | "unknown";

export class ApiError extends Error {
  readonly code: ApiErrorCode;
  readonly status: number;
  readonly requestId: string | null;
  /** True when the failure is connectivity-level, not a server verdict. */
  readonly offline: boolean;

  constructor(args: {
    code: ApiErrorCode;
    status: number;
    message: string;
    requestId?: string | null;
    offline?: boolean;
  }) {
    super(args.message);
    this.name = "ApiError";
    this.code = args.code;
    this.status = args.status;
    this.requestId = args.requestId ?? null;
    this.offline = args.offline ?? false;
  }
}

export interface CachedResult<T> {
  data: T;
  /** Unix ms when this payload was last confirmed fresh by the server. */
  fetchedAtMs: number;
  /** True when served from the local snapshot because the network failed. */
  fromOfflineCache: boolean;
}

interface CacheEntry<T> {
  etag: string | null;
  fetchedAtMs: number;
  data: T;
}

export interface ApiClientOptions {
  baseUrl?: string;
  fetchFn?: typeof fetch;
  storage?: StorageLike;
  now?: () => number;
}

interface ExpectedPrincipal {
  userId: string;
  account: boolean;
}

function randomId(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

export class ApiClient {
  private readonly baseUrl: string;
  private readonly fetchFn: typeof fetch;
  private readonly storage: StorageLike;
  private readonly now: () => number;
  private session: SessionTokens | null = null;
  private sessionPromise: Promise<SessionTokens | null> | null = null;
  /** Prevent an in-flight refresh/bootstrap from restoring a session after logout. */
  private sessionRevision = 0;
  private authListeners = new Set<() => void>();

  constructor(options: ApiClientOptions = {}) {
    this.baseUrl = (options.baseUrl ?? "").replace(/\/$/, "");
    this.fetchFn = options.fetchFn ?? fetch.bind(globalThis);
    this.storage = options.storage ?? getClientStorage();
    this.now = options.now ?? Date.now;
    this.session = this.loadSession();
  }

  // --- device / session persistence ---

  deviceId(): string {
    let id = this.storage.getItem(DEVICE_KEY);
    if (!id) {
      id = `dev-${randomId()}`;
      this.storage.setItem(DEVICE_KEY, id);
    }
    return id;
  }

  private loadSession(): SessionTokens | null {
    try {
      const raw = this.storage.getItem(SESSION_KEY);
      if (!raw) return null;
      const parsed = JSON.parse(raw) as SessionTokens;
      if (typeof parsed.access_token !== "string" || typeof parsed.refresh_token !== "string") {
        return null;
      }
      return { ...parsed, account: parsed.account === true };
    } catch {
      return null;
    }
  }

  private saveSession(session: SessionTokens | null): void {
    this.sessionRevision += 1;
    this.session = session;
    if (session) {
      this.storage.setItem(SESSION_KEY, JSON.stringify(session));
    } else {
      this.storage.removeItem(SESSION_KEY);
    }
    for (const listener of this.authListeners) listener();
  }

  subscribeAuth(listener: () => void): () => void {
    this.authListeners.add(listener);
    return () => this.authListeners.delete(listener);
  }

  /** Mark the access token expired but keep the refresh token for a refresh. */
  private invalidateAccess(): void {
    if (this.session) {
      this.saveSession({ ...this.session, expires_at_ms: 0 });
    }
  }

  hasSession(): boolean {
    return this.session !== null;
  }

  isAccountAuthenticated(): boolean {
    return this.session?.account === true;
  }

  /** Current opaque identity, used to scope persisted user-specific state. */
  sessionUserId(): string | null {
    return this.session?.user_id ?? null;
  }

  /**
   * Ensure a usable migration or account session. A rejected account refresh
   * falls back to an anonymous browsing session, never to an account guess.
   * Single-flight: concurrent callers share one bootstrap.
   */
  async ensureSession(): Promise<SessionTokens | null> {
    if (this.session && this.session.expires_at_ms > this.now() + 30_000) {
      return this.session;
    }
    const expectedRevision = this.sessionRevision;
    this.sessionPromise ??= this.bootstrapSession(expectedRevision).finally(() => {
      this.sessionPromise = null;
    });
    return this.sessionPromise;
  }

  private async bootstrapSession(expectedRevision: number): Promise<SessionTokens | null> {
    const current = this.session;
    if (current && current.refresh_expires_at_ms > this.now() + 30_000) {
      try {
        const refreshPath = current.account ? "/v1/auth/refresh" : "/v1/session/refresh";
        const refreshed = await this.rawJson<SessionTokens>("POST", refreshPath, {
          body: { refresh_token: current.refresh_token },
          auth: false,
        });
        if (this.sessionRevision !== expectedRevision) return this.session;
        this.saveSession(refreshed);
        return refreshed;
      } catch (error) {
        // A rejected refresh token cannot recover this identity. Transient
        // server and network failures must preserve it so a later retry can.
        if (!(error instanceof ApiError && error.status === 401)) throw error;
      }
    }
    if (this.sessionRevision !== expectedRevision) return this.session;
    const fresh = await this.rawJson<SessionTokens>("POST", "/v1/session/anonymous", {
      auth: false,
    });
    if (this.sessionRevision !== expectedRevision) return this.session;
    this.saveSession({ ...fresh, account: false });
    return fresh;
  }

  // --- low level ---

  private async rawResponse(
    method: string,
    path: string,
    args: {
      body?: unknown;
      auth?: boolean;
      headers?: Record<string, string>;
      /** Credential-verification endpoints use 401 as their business verdict. */
      retryAuthOn401?: boolean;
      /** Refuse to send or retry after the active identity changes. */
      expectedPrincipal?: ExpectedPrincipal;
    } = {},
  ): Promise<Response> {
    const headers: Record<string, string> = {
      "x-device-id": this.deviceId(),
      ...args.headers,
    };
    let requestSession: SessionTokens | null = null;
    if (args.body !== undefined) {
      headers["content-type"] = "application/json";
    }
    if (args.auth) {
      requestSession = await this.ensureSession();
      if (
        args.expectedPrincipal !== undefined &&
        (!requestSession ||
          requestSession.user_id !== args.expectedPrincipal.userId ||
          requestSession.account !== args.expectedPrincipal.account)
      ) {
        throw new ApiError({
          code: "unauthenticated",
          status: 401,
          message: "account changed before the request could be sent",
        });
      }
      if (requestSession) {
        headers.authorization = `Bearer ${requestSession.access_token}`;
      }
    }
    let response: Response;
    try {
      response = await this.fetchFn(`${this.baseUrl}${path}`, {
        method,
        headers,
        body: args.body === undefined ? null : JSON.stringify(args.body),
        // Tokens must never follow a cross-origin redirect (CS-008).
        redirect: "manual",
      });
    } catch (cause) {
      throw new ApiError({
        code: "network",
        status: 0,
        message: cause instanceof Error ? cause.message : "network request failed",
        offline: true,
      });
    }
    // 304 is not a redirect; only refuse true redirect statuses (CS-008).
    const redirected =
      response.type === "opaqueredirect" ||
      response.status === 301 ||
      response.status === 302 ||
      response.status === 303 ||
      response.status === 307 ||
      response.status === 308;
    if (redirected) {
      throw new ApiError({
        code: "network",
        status: response.status,
        message: "server redirected the request; refusing to follow with credentials",
        offline: false,
      });
    }
    if (response.status === 401 && args.auth && args.retryAuthOn401 !== false) {
      // A request started by one identity must never be replayed with another
      // identity's token after a logout, login, registration, or account switch.
      const current = this.session;
      if (!requestSession || !current || !this.samePrincipal(requestSession, current)) {
        return response;
      }
      // Access token rejected: refresh (keeping the refresh token) and retry once.
      // A concurrent request may already have refreshed this same principal.
      if (current.access_token === requestSession.access_token) {
        this.invalidateAccess();
      }
      const session = await this.ensureSession();
      if (session && this.samePrincipal(requestSession, session)) {
        headers.authorization = `Bearer ${session.access_token}`;
        try {
          response = await this.fetchFn(`${this.baseUrl}${path}`, {
            method,
            headers,
            body: args.body === undefined ? null : JSON.stringify(args.body),
            redirect: "manual",
          });
        } catch (cause) {
          throw new ApiError({
            code: "network",
            status: 0,
            message: cause instanceof Error ? cause.message : "network request failed",
            offline: true,
          });
        }
      }
    }
    return response;
  }

  private samePrincipal(left: SessionTokens, right: SessionTokens): boolean {
    return left.user_id === right.user_id && left.account === right.account;
  }

  private async parseError(response: Response): Promise<ApiError> {
    let code: ApiErrorCode = "unknown";
    let message = `HTTP ${response.status}`;
    let requestId: string | null = response.headers.get("x-request-id");
    try {
      const body = (await response.json()) as ErrorEnvelope;
      if (body && typeof body.error?.code === "string") {
        code = body.error.code as ApiErrorCode;
        message = body.error.message ?? message;
        requestId = body.error.request_id ?? requestId;
      }
    } catch {
      // keep defaults; error body is optional
    }
    return new ApiError({ code, status: response.status, message, requestId });
  }

  private async rawJson<T>(
    method: string,
    path: string,
    args: {
      body?: unknown;
      auth?: boolean;
      headers?: Record<string, string>;
      retryAuthOn401?: boolean;
      expectedPrincipal?: ExpectedPrincipal;
    } = {},
  ): Promise<T> {
    const response = await this.rawResponse(method, path, args);
    if (!response.ok) {
      throw await this.parseError(response);
    }
    return (await response.json()) as T;
  }

  private async accountResponse(
    method: string,
    path: string,
    body?: unknown,
    options: {
      headers?: Record<string, string>;
      expectedAccountUserId?: string;
    } = {},
  ): Promise<Response> {
    const activeUserId = this.sessionUserId();
    const expectedUserId = options.expectedAccountUserId ?? activeUserId;
    if (!this.isAccountAuthenticated() || !activeUserId || activeUserId !== expectedUserId) {
      throw new ApiError({
        code: "unauthenticated",
        status: 401,
        message: "sign in to continue",
      });
    }
    const response = await this.rawResponse(method, path, {
      auth: true,
      body,
      headers: options.headers,
      expectedPrincipal: { userId: expectedUserId, account: true },
    });
    if (!response.ok) throw await this.parseError(response);
    return response;
  }

  private async accountBinaryResponse(
    method: string,
    path: string,
    body: Blob,
    contentType?: string,
  ): Promise<Response> {
    const expectedUserId = this.sessionUserId();
    if (!this.isAccountAuthenticated() || !expectedUserId) {
      throw new ApiError({
        code: "unauthenticated",
        status: 401,
        message: "sign in to continue",
      });
    }
    const resolvedType = contentType || contentTypeForBlob(body);
    const send = async (): Promise<{ response: Response; session: SessionTokens }> => {
      const session = await this.ensureSession();
      if (!session?.account || session.user_id !== expectedUserId) {
        throw new ApiError({ code: "unauthenticated", status: 401, message: "sign in to continue" });
      }
      try {
        const response = await this.fetchFn(`${this.baseUrl}${path}`, {
          method,
          headers: {
            "x-device-id": this.deviceId(),
            authorization: `Bearer ${session.access_token}`,
            "content-type": resolvedType,
          },
          body,
          redirect: "manual",
        });
        if (
          response.type === "opaqueredirect" ||
          response.status === 301 ||
          response.status === 302 ||
          response.status === 303 ||
          response.status === 307 ||
          response.status === 308
        ) {
          throw new ApiError({
            code: "network",
            status: response.status,
            message: "server redirected the request; refusing to follow with credentials",
            offline: false,
          });
        }
        return { response, session };
      } catch (cause) {
        throw new ApiError({
          code: "network",
          status: 0,
          message: cause instanceof Error ? cause.message : "network request failed",
          offline: true,
        });
      }
    };
    let sent = await send();
    let response = sent.response;
    if (response.status === 401) {
      const current = this.session;
      if (
        !current?.account ||
        current.user_id !== expectedUserId ||
        !this.samePrincipal(sent.session, current)
      ) {
        throw await this.parseError(response);
      }
      if (current.access_token === sent.session.access_token) {
        this.invalidateAccess();
      }
      sent = await send();
      response = sent.response;
    }
    if (!response.ok) throw await this.parseError(response);
    return response;
  }

  // --- ETag snapshot cache ---

  private readCache<T>(key: string): CacheEntry<T> | null {
    try {
      const raw = this.storage.getItem(CACHE_PREFIX + key);
      if (!raw) return null;
      return JSON.parse(raw) as CacheEntry<T>;
    } catch {
      return null;
    }
  }

  private writeCache<T>(key: string, entry: CacheEntry<T>): void {
    try {
      this.storage.setItem(CACHE_PREFIX + key, JSON.stringify(entry));
    } catch {
      // Quota errors must never break the UI; stale cache is acceptable.
    }
  }

  /**
   * Remove every cached response snapshot. Session, device id and the pending
   * feedback queue live under different keys and are intentionally preserved
   * (clearing cache must never drop unsynced feedback).
   */
  clearCachedResponses(): number {
    const store = this.storage as Partial<Storage>;
    if (typeof store.length !== "number" || typeof store.key !== "function") {
      return 0; // storage without enumeration (e.g. minimal test doubles)
    }
    const keys: string[] = [];
    for (let i = 0; i < store.length; i += 1) {
      const key = store.key(i);
      if (key && key.startsWith(CACHE_PREFIX)) keys.push(key);
    }
    for (const key of keys) this.storage.removeItem(key);
    return keys.length;
  }

  /**
   * GET with ETag revalidation backed by the snapshot cache.
   * - 200: store payload + etag, return fresh data
   * - 304: refresh timestamp, return cached data
   * - network failure: return cached data flagged fromOfflineCache, else rethrow
   */
  private async cachedGet<T>(key: string, path: string, auth: boolean): Promise<CachedResult<T>> {
    const cached = this.readCache<T>(key);
    const headers: Record<string, string> = {};
    if (cached?.etag) {
      headers["if-none-match"] = cached.etag;
    }
    let response: Response;
    try {
      const expectedPrincipal =
        auth && this.session
          ? { userId: this.session.user_id, account: this.session.account }
          : undefined;
      response = await this.rawResponse("GET", path, { auth, headers, expectedPrincipal });
    } catch (error) {
      if (error instanceof ApiError && error.offline && cached) {
        return { data: cached.data, fetchedAtMs: cached.fetchedAtMs, fromOfflineCache: true };
      }
      throw error;
    }
    if (response.status === 304 && cached) {
      const entry: CacheEntry<T> = { ...cached, fetchedAtMs: this.now() };
      this.writeCache(key, entry);
      return { data: cached.data, fetchedAtMs: entry.fetchedAtMs, fromOfflineCache: false };
    }
    if (!response.ok) {
      throw await this.parseError(response);
    }
    const data = (await response.json()) as T;
    const entry: CacheEntry<T> = {
      etag: response.headers.get("etag"),
      fetchedAtMs: this.now(),
      data,
    };
    this.writeCache(key, entry);
    return { data, fetchedAtMs: entry.fetchedAtMs, fromOfflineCache: false };
  }

  // --- public endpoints ---

  meta(): Promise<CachedResult<MetaResponse>> {
    return this.cachedGet<MetaResponse>("meta", "/v1/meta", false);
  }

  feed(
    section: FeedSection,
    query: {
      limit?: number;
      page?: number;
      cursor?: string;
      partySize?: number;
      demoOnly?: boolean;
      sort?: FeedSort;
      order?: FeedSortOrder;
    } = {},
  ): Promise<CachedResult<FeedResponse>> {
    const params = new URLSearchParams();
    if (query.limit) params.set("limit", String(query.limit));
    if (query.page) params.set("page", String(query.page));
    if (query.cursor) params.set("cursor", query.cursor);
    if (query.partySize) params.set("party_size", String(query.partySize));
    if (query.demoOnly) params.set("demo_only", "true");
    if (query.sort && query.sort !== "recommended") params.set("sort", query.sort);
    if (query.order) params.set("order", query.order);
    const qs = params.toString();
    const path = `/v1/feeds/${section}${qs ? `?${qs}` : ""}`;
    // Only the first page is cached as an offline snapshot.
    const isFirstPage = !query.cursor && (query.page === undefined || query.page <= 1);
    if (!isFirstPage) {
      return this.uncachedGet<FeedResponse>(path, this.hasSession());
    }
    const cacheKey = `feed:v6:${section}:${query.limit ?? "d"}:${query.partySize ?? "p"}:${
      query.demoOnly ? 1 : 0
    }:${query.sort ?? "recommended"}:${query.order ?? "auto"}:${this.session?.user_id ?? "anon"}`;
    return this.cachedGet<FeedResponse>(cacheKey, path, this.hasSession());
  }

  private async uncachedGet<T>(path: string, auth: boolean): Promise<CachedResult<T>> {
    const expectedPrincipal =
      auth && this.session
        ? { userId: this.session.user_id, account: this.session.account }
        : undefined;
    const data = await this.rawJson<T>("GET", path, { auth, expectedPrincipal });
    return { data, fetchedAtMs: this.now(), fromOfflineCache: false };
  }

  calendar(
    fromDay: string,
    toDay: string,
    period: CalendarPeriod = "upcoming",
  ): Promise<CachedResult<CalendarResponse>> {
    const params = new URLSearchParams({ from: fromDay, to: toDay, state: period });
    return this.cachedGet<CalendarResponse>(
      `calendar:${period}:${fromDay}:${toDay}`,
      `/v1/calendar?${params}`,
      false,
    );
  }

  async search(q: string, limit = 20): Promise<SearchResponse> {
    const params = new URLSearchParams({ q, limit: String(limit) });
    return this.rawJson<SearchResponse>("GET", `/v1/search?${params}`, { auth: false });
  }

  /** Operator dashboard: inventory + refresh tasks + section coverage. */
  async adminDataStatus(adminToken: string): Promise<DataStatusResponse> {
    return this.adminJson<DataStatusResponse>("GET", "/admin/v1/data-status", adminToken);
  }

  /** Operator lookup: is this Steam app in the catalog / multiplayer pool? */
  async adminAppPresence(adminToken: string, appId: number): Promise<PipelineAppPresence> {
    try {
      const debug = await this.adminJson<{
        app: {
          app_id: number;
          canonical_name: string;
          release_date: string | null;
          release_state: string | null;
          app_type: string | null;
        } | null;
        multiplayer_profile: { app_id: number } | null;
      }>("GET", `/admin/v1/games/${appId}/debug`, adminToken);
      return {
        app_id: appId,
        in_apps: debug.app != null,
        has_multiplayer_profile: debug.multiplayer_profile != null,
        app: debug.app,
        note:
          debug.app == null
            ? "不在 apps 表：发现阶段没扫到（AppList/商店搜索），不是 Feed 排序藏起来的。"
            : debug.multiplayer_profile == null
              ? "在 apps 里，但不在联机池：还没被当成 multiplayer 候选或 profile 未写出。"
              : "已在联机池；若列表仍看不到，检查发售日/分区资格/排序。",
      };
    } catch (error) {
      if (error instanceof ApiError && error.code === "not_found") {
        return {
          app_id: appId,
          in_apps: false,
          has_multiplayer_profile: false,
          app: null,
          note: "服务端返回 not_found。",
        };
      }
      throw error;
    }
  }

  private async adminJson<T>(
    method: string,
    path: string,
    adminToken: string,
    body?: unknown,
  ): Promise<T> {
    const headers: Record<string, string> = {
      "x-device-id": this.deviceId(),
      authorization: `Bearer ${adminToken}`,
    };
    if (body !== undefined) headers["content-type"] = "application/json";
    let response: Response;
    try {
      response = await this.fetchFn(`${this.baseUrl}${path}`, {
        method,
        headers,
        body: body === undefined ? null : JSON.stringify(body),
        redirect: "manual",
      });
    } catch (cause) {
      throw new ApiError({
        code: "network",
        status: 0,
        message: cause instanceof Error ? cause.message : "network request failed",
        offline: true,
      });
    }
    if (!response.ok) throw await this.parseError(response);
    const text = await response.text();
    const trimmed = text.trimStart();
    if (trimmed.startsWith("<!") || trimmed.startsWith("<html")) {
      throw new ApiError({
        code: "internal",
        status: response.status,
        message:
          "admin API returned HTML instead of JSON — reverse proxy is not routing /admin/ to the server",
      });
    }
    try {
      return JSON.parse(text) as T;
    } catch {
      throw new ApiError({
        code: "internal",
        status: response.status,
        message: "admin API returned non-JSON response",
      });
    }
  }

  async naturalLanguageRecommendations(
    query: string,
    limit = 6,
    customAi?: {
      ownerUserId: string;
      provider: "openai_compat";
      baseUrl: string;
      model: string;
      apiKey: string;
      multiModel?: boolean;
      fallbackModel?: string | null;
      routes?: Array<{
        task: string;
        primary_model: string;
        fallback_models: string[];
      }>;
    },
  ): Promise<NaturalLanguageRecommendationResponse> {
    return this.rawJson<NaturalLanguageRecommendationResponse>(
      "POST",
      "/v1/recommendations/natural-language",
      {
        auth: true,
        expectedPrincipal: customAi
          ? { userId: customAi.ownerUserId, account: true }
          : undefined,
        body: {
          query,
          limit,
          custom_ai: customAi
            ? {
                provider: customAi.provider,
                base_url: customAi.baseUrl,
                model: customAi.model,
                api_key: customAi.apiKey,
                // Custom providers stay single-model by default; name heuristics are unsafe across vendors.
                multi_model: customAi.multiModel ?? false,
                fallback_model: customAi.fallbackModel ?? undefined,
                routes: customAi.routes,
              }
            : undefined,
        },
      },
    );
  }

  game(appId: number): Promise<CachedResult<GameDetail>> {
    // Authenticated when a session exists so the response carries this user's
    // play-intent vote state; falls back to anonymous (voted always false).
    return this.cachedGet<GameDetail>(
      `game:${appId}:${this.session?.user_id ?? "anon"}`,
      `/v1/games/${appId}`,
      this.hasSession(),
    );
  }

  evidence(appId: number, feature?: string): Promise<CachedResult<EvidenceResponse>> {
    const qs = feature ? `?feature=${encodeURIComponent(feature)}` : "";
    return this.cachedGet<EvidenceResponse>(
      `evidence:${appId}:${feature ?? "all"}`,
      `/v1/games/${appId}/evidence${qs}`,
      false,
    );
  }

  async getPreferences(): Promise<UserPreferences> {
    const response = await this.accountResponse("GET", "/v1/preferences");
    return (await response.json()) as UserPreferences;
  }

  async putPreferences(prefs: UserPreferences): Promise<UserPreferences> {
    const response = await this.accountResponse("PUT", "/v1/preferences", prefs);
    return (await response.json()) as UserPreferences;
  }

  async postFeedback(args: {
    appId: number;
    type: FeedbackType;
    idempotencyKey: string;
    clientCreatedAtMs: number;
    recommendationRunId?: string | null;
  }): Promise<FeedbackRecord> {
    const response = await this.accountResponse(
      "POST",
      "/v1/feedback",
      {
        app_id: args.appId,
        type: args.type,
        recommendation_run_id: args.recommendationRunId ?? undefined,
        client_created_at_ms: args.clientCreatedAtMs,
      },
      { headers: { "idempotency-key": args.idempotencyKey } },
    );
    return (await response.json()) as FeedbackRecord;
  }

  async postRecommendationEvent(args: {
    recommendationRunId: string;
    appId: number;
    eventType: RecommendationEventType;
    idempotencyKey?: string;
    clientCreatedAtMs?: number;
  }): Promise<void> {
    const idempotencyKey =
      args.idempotencyKey ??
      `${args.eventType}:${args.appId}:${this.now()}:${randomId()}`;
    await this.rawJson<unknown>("POST", "/v1/recommendation-events", {
      auth: false,
      headers: { "idempotency-key": idempotencyKey },
      body: {
        recommendation_run_id: args.recommendationRunId,
        app_id: args.appId,
        event_type: args.eventType,
        client_created_at_ms: args.clientCreatedAtMs,
      },
    });
  }

  async undoFeedback(feedbackId: number): Promise<FeedbackRecord> {
    const response = await this.accountResponse("POST", `/v1/feedback/${feedbackId}/undo`);
    return (await response.json()) as FeedbackRecord;
  }

  async setPlayIntent(appId: number, intent: boolean): Promise<PlayIntentResult> {
    const response = await this.accountResponse("POST", `/v1/games/${appId}/play-intent`, { intent });
    this.clearCachedResponses();
    return (await response.json()) as PlayIntentResult;
  }

  async register(args: {
    username: string;
    displayName: string;
    password: string;
    deviceLabel?: string;
  }): Promise<SessionTokens> {
    const expectedPrincipal = this.session
      ? { userId: this.session.user_id, account: this.session.account }
      : undefined;
    const session = await this.rawJson<SessionTokens>("POST", "/v1/auth/register", {
      auth: true,
      retryAuthOn401: false,
      expectedPrincipal,
      body: {
        username: args.username,
        display_name: args.displayName,
        password: args.password,
        device_label: args.deviceLabel ?? "LobbyTally web",
      },
    });
    const accountSession = { ...session, account: true };
    this.saveSession(accountSession);
    this.clearCachedResponses();
    return accountSession;
  }

  async login(args: {
    username: string;
    password: string;
    deviceLabel?: string;
    mergePreference?: "anonymous" | "account";
  }): Promise<SessionTokens> {
    const expectedPrincipal = this.session
      ? { userId: this.session.user_id, account: this.session.account }
      : undefined;
    const session = await this.rawJson<SessionTokens>("POST", "/v1/auth/login", {
      auth: true,
      retryAuthOn401: false,
      expectedPrincipal,
      body: {
        username: args.username,
        password: args.password,
        device_label: args.deviceLabel ?? "LobbyTally web",
        merge_preference: args.mergePreference,
      },
    });
    const accountSession = { ...session, account: true };
    this.saveSession(accountSession);
    this.clearCachedResponses();
    return accountSession;
  }

  async logout(): Promise<void> {
    await this.localLogoutWithBestEffortRevoke("/v1/auth/logout");
  }

  async logoutAll(): Promise<void> {
    await this.localLogoutWithBestEffortRevoke("/v1/auth/logout-all");
  }

  /**
   * Local sign-out must not depend on connectivity. Capture the current token,
   * clear it synchronously, then make a non-blocking best-effort revocation.
   */
  private async localLogoutWithBestEffortRevoke(path: string): Promise<void> {
    const session = this.session;
    this.saveSession(null);
    this.clearCachedResponses();
    if (!session?.account) return;
    try {
      const revoke = this.fetchFn(`${this.baseUrl}${path}`, {
        method: "POST",
        headers: {
          "x-device-id": this.deviceId(),
          authorization: `Bearer ${session.access_token}`,
        },
        body: null,
        redirect: "manual",
      });
      void revoke.catch(() => undefined);
    } catch {
      // The local credential is already gone. Remote expiry/revocation is best effort.
    }
  }

  async changePassword(oldPassword: string, newPassword: string): Promise<void> {
    await this.accountResponse("PUT", "/v1/auth/password", {
      old_password: oldPassword,
      new_password: newPassword,
    });
  }

  async getMe(): Promise<AccountProfile> {
    const response = await this.accountResponse("GET", "/v1/me");
    return (await response.json()) as AccountProfile;
  }

  async updateMe(displayName: string): Promise<AccountProfile> {
    const response = await this.accountResponse("PATCH", "/v1/me", { display_name: displayName });
    return (await response.json()) as AccountProfile;
  }

  async deleteMe(): Promise<void> {
    await this.accountResponse("DELETE", "/v1/me");
    this.saveSession(null);
    this.clearCachedResponses();
  }

  async uploadAvatar(file: Blob): Promise<AccountProfile> {
    const response = await this.accountBinaryResponse(
      "PUT",
      "/v1/me/avatar",
      file,
      contentTypeForBlob(file),
    );
    return (await response.json()) as AccountProfile;
  }

  async deleteAvatar(): Promise<void> {
    await this.accountResponse("DELETE", "/v1/me/avatar");
  }

  async getAiSettings(): Promise<AiSettings> {
    const response = await this.accountResponse("GET", "/v1/me/ai-settings");
    return (await response.json()) as AiSettings;
  }

  async putAiSettings(input: {
    mode: "builtin" | "custom" | "off";
    provider?: "openai_compat";
    baseUrl?: string;
    model?: string;
    apiKey?: string;
    expectedAccountUserId?: string;
  }): Promise<AiSettings> {
    const response = await this.accountResponse(
      "PUT",
      "/v1/me/ai-settings",
      {
        mode: input.mode,
        provider: input.provider,
        base_url: input.baseUrl,
        model: input.model,
        api_key: input.apiKey,
      },
      { expectedAccountUserId: input.expectedAccountUserId },
    );
    return (await response.json()) as AiSettings;
  }

  async testAiSettings(input: {
    ownerUserId: string;
    provider: "openai_compat";
    baseUrl: string;
    model: string;
    apiKey?: string;
  }): Promise<void> {
    await this.accountResponse(
      "POST",
      "/v1/me/ai-settings/test",
      {
        mode: "custom",
        provider: input.provider,
        base_url: input.baseUrl,
        model: input.model,
        api_key: input.apiKey,
      },
      { expectedAccountUserId: input.ownerUserId },
    );
  }

  async discoverCustomModels(input: {
    ownerUserId: string;
    baseUrl: string;
    apiKey: string;
  }): Promise<{ models: string[] }> {
    const response = await this.accountResponse(
      "POST",
      "/v1/me/ai-settings/discover",
      {
        base_url: input.baseUrl,
        api_key: input.apiKey,
      },
      { expectedAccountUserId: input.ownerUserId },
    );
    return (await response.json()) as { models: string[] };
  }

  async deleteCustomAiKey(expectedAccountUserId?: string): Promise<AiSettings> {
    const response = await this.accountResponse(
      "DELETE",
      "/v1/me/ai-settings/custom-key",
      undefined,
      { expectedAccountUserId },
    );
    return (await response.json()) as AiSettings;
  }

  community(
    sort: CommunitySort,
    filters: CommunityFilters = {},
    cursor?: string,
  ): Promise<CachedResult<CommunityResponse>> {
    const params = new URLSearchParams({ sort });
    if (filters.releaseState) params.set("release_state", filters.releaseState);
    if (filters.demoOnly) params.set("demo_only", "true");
    if (filters.platform) params.set("platform", filters.platform);
    if (filters.partySize) params.set("party_size", String(filters.partySize));
    if (cursor) params.set("cursor", cursor);
    const path = `/v1/community/play-intents?${params}`;
    if (cursor) return this.uncachedGet<CommunityResponse>(path, this.isAccountAuthenticated());
    const filterKey = [
      filters.releaseState ?? "any",
      filters.demoOnly ? "demo" : "all",
      filters.platform ?? "any",
      filters.partySize ?? "any",
    ].join(":");
    return this.cachedGet<CommunityResponse>(
      `community:${sort}:${filterKey}:${this.isAccountAuthenticated() ? this.session?.user_id ?? "account" : "public"}`,
      path,
      this.isAccountAuthenticated(),
    );
  }
}

/** Prefer browser MIME type; fall back to filename extension for empty File.type. */
export function contentTypeForBlob(file: Blob): string {
  const typed = file.type?.trim().toLowerCase() ?? "";
  if (typed.startsWith("image/")) {
    if (typed === "image/jpg") return "image/jpeg";
    return typed;
  }
  const name =
    typeof File !== "undefined" && file instanceof File
      ? file.name.trim().toLowerCase()
      : "";
  if (name.endsWith(".jpg") || name.endsWith(".jpeg")) return "image/jpeg";
  if (name.endsWith(".png")) return "image/png";
  if (name.endsWith(".webp")) return "image/webp";
  // Let the server sniff magic bytes for empty/generic types.
  return typed || "application/octet-stream";
}

export function newIdempotencyKey(): string {
  return `idem-${randomId()}`;
}
