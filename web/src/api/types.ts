// DTO types mirroring apps/server/src/api.rs response shapes.
// Field names and semantics follow docs/API.md; unknown fields must be ignored.

export type FeedSection =
  | "recent_release"
  | "upcoming"
  | "popular_legacy"
  | "classic_legacy";

export const FEED_SECTIONS: FeedSection[] = [
  "recent_release",
  "upcoming",
  "popular_legacy",
  "classic_legacy",
];

/** Feed list sort after recommendation scoring. */
export type FeedSort = "recommended" | "ccu" | "reviews" | "release_date";
export type FeedSortOrder = "asc" | "desc";

export const FEED_SORT_OPTIONS: { id: FeedSort; label: string }[] = [
  { id: "recommended", label: "推荐分" },
  { id: "ccu", label: "在线人数" },
  { id: "reviews", label: "评论数" },
  { id: "release_date", label: "发售日期" },
];

export type FeedbackType =
  | "like"
  | "not_interested"
  | "played"
  | "too_competitive"
  | "party_size_mismatch"
  | "hosting_friction";

/** Community play-intent state for a game (embedded in feed items and detail). */
export interface PlayIntentSummary {
  count: number;
  voted: boolean;
  voters_preview?: PublicVoter[];
  omitted_count?: number;
}

export interface PublicVoter {
  display_name: string;
  avatar_url: string;
}

export interface SessionTokens {
  access_token: string;
  refresh_token: string;
  user_id: string;
  expires_at_ms: number;
  refresh_expires_at_ms: number;
  /** False for an anonymous migration session, true for an account session. */
  account: boolean;
}

export interface UserPreferences {
  version: number;
  party_size: number;
  /** 0 = pure coop preference, 1 = strong competitive preference. */
  coop_competitive: number;
  session_minutes_min: number;
  session_minutes_max: number;
  budget_currency: string;
  budget_max_each_minor: number | null;
  platforms: string[];
  self_hosting_willingness: number;
  languages: string[];
  excluded_modes: string[];
}

export interface MetaResponse {
  api_version: string;
  service_version: string;
  algorithm_version: string;
  supported_sections: string[];
  ai_available: boolean;
  storage_enabled: boolean;
  demo_mode: boolean;
}

export type AiStatus = "pending" | "used" | "cached" | "fallback" | "disabled";

export interface FeedItem {
  app_id: number;
  name: string;
  section: FeedSection;
  release_date: string | null;
  release_date_raw: string | null;
  release_date_precision: string | null;
  cover_url: string | null;
  cover_updated_at_ms: number | null;
  total_reviews: number | null;
  total_positive: number | null;
  latest_ccu: number | null;
  typical_ccu_7d: number | null;
  score: number;
  confidence: number;
  party: {
    recommended_min: number | null;
    recommended_max: number | null;
  };
  multiplayer: {
    dominant_mode: string | null;
  };
  play_intent: PlayIntentSummary;
  reasons: string[];
  cautions: string[];
  evidence_ids: string[];
  components: {
    friend_fit: number;
    section_score: number;
    personalized_score: number;
    final_score: number;
  };
  algorithm_version: string;
  hybrid_score?: number;
  ai_fit?: number;
  ai_confidence?: number;
  ai_reasons?: string[];
}

export interface FeedResponse {
  items: FeedItem[];
  next_cursor: string | null;
  total: number;
  limit: number;
  offset: number;
  page: number;
  total_pages: number;
  snapshot_at_ms: number;
  algorithm_version: string;
  data_updated_at_ms: number;
  sort?: FeedSort;
  order?: FeedSortOrder;
}

export interface CalendarItem {
  app_id: number;
  app_type: string;
  canonical_name: string;
  cover_url?: string | null;
  release_state: string;
  release_date: string | null;
  release_date_raw: string | null;
  release_date_precision: string | null;
  is_early_access: boolean | null;
  current_data_confidence: number | null;
  review_total: number | null;
  early_data: boolean;
  source_modified_at_ms: number | null;
  created_at_ms: number;
  updated_at_ms: number;
}

export interface CalendarResponse {
  dated_items: CalendarItem[];
  undated_items: CalendarItem[];
  data_updated_at_ms: number;
}

export type CalendarPeriod = "upcoming" | "recent";

export interface SearchItem {
  app_id: number;
  name: string;
  release_state: string;
  release_date: string | null;
}

export interface SearchResponse {
  items: SearchItem[];
  algorithm_version: string;
}

export interface NaturalLanguageRecommendationResponse {
  query: string;
  interpreted: {
    party_size: number | null;
    session_minutes_max: number | null;
    coop_competitive: number | null;
    self_hosting_willingness?: number | null;
    platforms?: string[];
    hard_constraints?: string[];
    intent_confidence?: number | null;
    max_price_minor?: number | null;
    currency?: string | null;
  };
  items: FeedItem[];
  ai_status: AiStatus;
  ai_provider?: string;
  ai_model?: string | null;
  ai_protocol?: string | null;
  ai_route_version?: string | null;
  ai_used_model_fallback?: boolean;
  ai_attempted_models?: string[];
  ai_multi_model?: boolean;
  ai_routes?: {
    rank_explain?: { primary: string; fallbacks: string[] } | null;
    intent_parse?: { primary: string; fallbacks: string[] } | null;
  };
  ai_latency_ms?: number;
  fallback_reason: string | null;
  ai_summary?: string | null;
  ai_summary_evidence_ids?: string[];
  analysis_id?: string;
  algorithm_version: string;
  data_updated_at_ms: number;
}

export interface GameDetail {
  app_id: number;
  name: string;
  app_type: string;
  release_state: string;
  release_date: string | null;
  release_date_raw: string | null;
  release_date_precision: string | null;
  cover_url: string | null;
  cover_updated_at_ms: number | null;
  short_description: string | null;
  steam_url: string;
  multiplayer: {
    dominant_mode: string | null;
    private_session: boolean | null;
    online_coop: boolean | null;
    self_hosted_server: boolean | null;
    recommended_min: number | null;
    recommended_max: number | null;
    profile_confidence: number | null;
  };
  play_intent: PlayIntentSummary;
  reviews: {
    total: number | null;
    positive: number | null;
    featured: PopularReview[];
  };
  latest_ccu: number | null;
  availability: {
    platforms: string[];
    languages: string[];
    typical_session_minutes_min: number | null;
    typical_session_minutes_max: number | null;
    is_free: boolean | null;
    final_price_minor: number | null;
    price_currency: string | null;
    has_demo: boolean;
  };
  algorithm_version: string;
  data_updated_at_ms: number;
}

export interface PopularReview {
  recommendation_id: string;
  rank: number;
  author_name: string | null;
  author_profile_url: string | null;
  text: string;
  voted_up: boolean;
  votes_up: number;
  votes_funny: number;
  comment_count: number;
  playtime_forever_minutes: number | null;
  playtime_at_review_minutes: number | null;
  created_at_ms: number;
  written_during_early_access: boolean;
}

export interface EvidenceItem {
  evidence_id: string;
  feature: string;
  value: unknown;
  source_type: string;
  source_label: string;
  confidence: number;
  observed_at_ms: number;
}

export interface EvidenceResponse {
  items: EvidenceItem[];
}

export interface FeedbackRecord {
  feedback_id: number;
  app_id: number;
  type: string;
  recommendation_run_id: string | null;
  created_at_ms: number;
}

export interface PlayIntentResult {
  app_id: number;
  count: number;
  voted: boolean;
  voters_preview: PublicVoter[];
  omitted_count: number;
}

export interface AccountProfile {
  username: string;
  display_name: string;
  avatar_url: string;
  avatar_version: number;
}

export interface AiTaskRouteInfo {
  task: string;
  primary_model: string;
  fallback_models: string[];
  protocol_preference: string[];
  timeout_ms: number;
  max_output_tokens: number;
  enabled: boolean;
  route_version: string;
  primary_available: boolean;
}

export interface AiSettings {
  mode: "builtin" | "custom" | "off";
  provider: string | null;
  base_url: string | null;
  model: string | null;
  configured: boolean;
  key_mask: string | null;
  updated_at_ms: number | null;
  builtin: {
    available: boolean;
    /** @deprecated use provider + routes; kept for older servers */
    model?: string;
    provider?: string;
    multi_model?: boolean;
    route_version?: string;
    routes?: AiTaskRouteInfo[];
    discovered_models?: string[];
    daily_remaining: number | null;
  };
}

export type CommunitySort = "trending" | "most_voted";
export type CommunityReleaseState = "released" | "upcoming" | "coming_soon" | "retired" | "unknown";
export type CommunityPlatform = "windows" | "macos" | "linux";

export interface CommunityFilters {
  releaseState?: CommunityReleaseState;
  demoOnly?: boolean;
  platform?: CommunityPlatform;
  partySize?: number;
}

export interface CommunityItem {
  app_id: number;
  name: string;
  app_type: string;
  release_state: string;
  release_date: string | null;
  release_date_raw: string | null;
  release_date_precision: string | null;
  cover_url: string | null;
  cover_updated_at_ms: number | null;
  trending_count: number;
  play_intent: PlayIntentSummary;
}

export interface CommunityResponse {
  items: CommunityItem[];
  next_cursor: string | null;
  snapshot_revision: number;
  data_updated_at_ms: number;
}

export interface ErrorEnvelope {
  error: {
    code: string;
    message: string;
    request_id?: string | null;
  };
}

/** Minimal synchronous surface used by the hydrated SQLite mirror and test doubles. */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}
