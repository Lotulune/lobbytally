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
}

export interface SessionTokens {
  access_token: string;
  refresh_token: string;
  user_id: string;
  expires_at_ms: number;
  refresh_expires_at_ms: number;
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
}

export type AiStatus = "used" | "cached" | "fallback" | "disabled";

export interface FeedItem {
  app_id: number;
  name: string;
  section: FeedSection;
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
  snapshot_at_ms: number;
  algorithm_version: string;
  data_updated_at_ms: number;
}

export interface CalendarItem {
  app_id: number;
  app_type: string;
  canonical_name: string;
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
  };
  items: FeedItem[];
  ai_status: AiStatus;
  fallback_reason: string | null;
  ai_summary?: string | null;
  algorithm_version: string;
  data_updated_at_ms: number;
}

export interface GameDetail {
  app_id: number;
  name: string;
  app_type: string;
  release_state: string;
  release_date: string | null;
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
