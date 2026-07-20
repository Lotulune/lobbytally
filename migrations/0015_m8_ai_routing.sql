-- M8: multi-model routing metadata, progressive AI analyses, dual-channel
-- web evidence / field proposals, and first-start bootstrap state.
-- Steam authority tables are unchanged; proposals never overwrite facts.

-- Task → model route configuration (optional durable overrides; runtime may
-- still apply env/discovery on top).
CREATE TABLE IF NOT EXISTS ai_model_routes (
    task_type TEXT PRIMARY KEY,
    primary_model TEXT NOT NULL,
    fallback_models_json TEXT NOT NULL DEFAULT '[]',
    protocol_preference_json TEXT NOT NULL DEFAULT '[]',
    timeout_ms INTEGER NOT NULL,
    max_output_tokens INTEGER NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    route_version TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    CHECK (enabled IN (0, 1)),
    CHECK (timeout_ms > 0),
    CHECK (max_output_tokens > 0)
);

-- Per-attempt observability. Never store API keys or full prompts.
CREATE TABLE IF NOT EXISTS ai_task_runs (
    run_id TEXT PRIMARY KEY,
    task_type TEXT NOT NULL,
    model TEXT NOT NULL,
    protocol TEXT,
    status TEXT NOT NULL,
    latency_ms INTEGER,
    usage_input INTEGER,
    usage_output INTEGER,
    error_category TEXT,
    cache_hit INTEGER NOT NULL DEFAULT 0,
    route_version TEXT,
    analysis_id TEXT,
    created_at_ms INTEGER NOT NULL,
    CHECK (status IN ('succeeded', 'failed', 'timeout', 'rate_limited', 'invalid_output', 'skipped')),
    CHECK (cache_hit IN (0, 1))
);

CREATE INDEX IF NOT EXISTS idx_ai_task_runs_task_time
    ON ai_task_runs(task_type, created_at_ms DESC);

-- Progressive / multi-candidate AI analysis (search, compare, group advice).
-- Distinct from per-app offline feature extracts in legacy ai_analyses.
CREATE TABLE IF NOT EXISTS ai_progressive_analyses (
    analysis_id TEXT PRIMARY KEY,
    task_type TEXT NOT NULL,
    status TEXT NOT NULL,
    provider TEXT,
    model TEXT,
    protocol TEXT,
    route_version TEXT,
    prompt_version TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    preference_hash TEXT NOT NULL DEFAULT '',
    data_snapshot_hash TEXT NOT NULL DEFAULT '',
    request_json TEXT NOT NULL,
    base_result_json TEXT,
    result_json TEXT,
    error_category TEXT,
    fallback_reason TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    expires_at_ms INTEGER NOT NULL,
    CHECK (status IN ('pending', 'used', 'cached', 'fallback', 'disabled'))
);

CREATE INDEX IF NOT EXISTS idx_ai_progressive_analyses_status
    ON ai_progressive_analyses(status, expires_at_ms);

-- Offline game summaries (M8.3 storage ready; workers fill later).
CREATE TABLE IF NOT EXISTS game_ai_summaries (
    app_id INTEGER NOT NULL REFERENCES apps(app_id) ON DELETE CASCADE,
    input_hash TEXT NOT NULL,
    prompt_version TEXT NOT NULL,
    summary_json TEXT NOT NULL,
    evidence_ids_json TEXT NOT NULL DEFAULT '[]',
    review_status TEXT NOT NULL DEFAULT 'pending_review',
    model TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    PRIMARY KEY (app_id, prompt_version),
    CHECK (review_status IN ('pending_review', 'accepted', 'rejected'))
);

CREATE INDEX IF NOT EXISTS idx_game_ai_summaries_expiry
    ON game_ai_summaries(expires_at_ms);

-- Web discovery evidence (low confidence; never overwrites Steam authority).
CREATE TABLE IF NOT EXISTS web_discovery_evidence (
    evidence_id TEXT PRIMARY KEY,
    app_id INTEGER REFERENCES apps(app_id) ON DELETE SET NULL,
    query_text TEXT NOT NULL,
    source_url TEXT NOT NULL,
    source_host TEXT NOT NULL,
    source_tier TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    snippet TEXT NOT NULL DEFAULT '',
    content_hash TEXT NOT NULL,
    fetched_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    UNIQUE (source_url, content_hash),
    CHECK (source_tier IN ('official', 'developer', 'community', 'unknown'))
);

CREATE INDEX IF NOT EXISTS idx_web_discovery_app
    ON web_discovery_evidence(app_id, fetched_at_ms DESC);

CREATE INDEX IF NOT EXISTS idx_web_discovery_host
    ON web_discovery_evidence(source_host, fetched_at_ms DESC);

-- AI/Web structured proposals that must not overwrite authority fields.
CREATE TABLE IF NOT EXISTS field_proposals (
    proposal_id TEXT PRIMARY KEY,
    app_id INTEGER NOT NULL REFERENCES apps(app_id) ON DELETE CASCADE,
    field_name TEXT NOT NULL,
    proposed_value_json TEXT NOT NULL,
    confidence REAL NOT NULL,
    evidence_ids_json TEXT NOT NULL DEFAULT '[]',
    source_channel TEXT NOT NULL,
    review_status TEXT NOT NULL DEFAULT 'pending_review',
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    CHECK (confidence >= 0 AND confidence <= 1),
    CHECK (source_channel IN ('web_discovery', 'ai_extract', 'manual')),
    CHECK (review_status IN ('pending_review', 'accepted', 'rejected')),
    -- Authority fields must never be auto-applied from proposals.
    CHECK (field_name NOT IN (
        'price', 'platforms', 'party_size_min', 'party_size_max',
        'service_status', 'release_state'
    ))
);

CREATE INDEX IF NOT EXISTS idx_field_proposals_app_status
    ON field_proposals(app_id, review_status, created_at_ms DESC);

-- First-start / bootstrap progress (AI-015).
CREATE TABLE IF NOT EXISTS bootstrap_state (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

-- Host-level rate limit / breaker snapshot for dual-channel collectors.
CREATE TABLE IF NOT EXISTS source_host_limits (
    host TEXT PRIMARY KEY,
    tokens REAL NOT NULL DEFAULT 0,
    max_tokens REAL NOT NULL,
    refill_per_sec REAL NOT NULL,
    max_concurrency INTEGER NOT NULL,
    circuit_open_until_ms INTEGER,
    consecutive_429 INTEGER NOT NULL DEFAULT 0,
    updated_at_ms INTEGER NOT NULL,
    CHECK (max_tokens > 0),
    CHECK (refill_per_sec > 0),
    CHECK (max_concurrency > 0)
);
