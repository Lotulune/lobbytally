-- Privacy-preserving recommendation attribution.
--
-- Runs retain hashes of normalized structured request context and candidate
-- sets; raw natural-language queries and raw subject identifiers do not belong
-- in these tables. Every row carries an indexed expiry timestamp so the
-- application can enforce the bounded telemetry-retention policy efficiently.

CREATE TABLE recommendation_runs (
    recommendation_run_id TEXT PRIMARY KEY,
    subject_hash TEXT,
    request_kind TEXT NOT NULL,
    feed_section TEXT NOT NULL,
    algorithm_version TEXT NOT NULL,
    config_version TEXT NOT NULL,
    score_semantics TEXT NOT NULL,
    context_schema_version INTEGER NOT NULL CHECK (context_schema_version > 0),
    context_hash TEXT NOT NULL,
    candidate_set_hash TEXT NOT NULL,
    candidate_count INTEGER NOT NULL CHECK (candidate_count >= 0),
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    CHECK (length(recommendation_run_id) BETWEEN 1 AND 128),
    CHECK (
        subject_hash IS NULL
        OR (length(subject_hash) = 64 AND subject_hash NOT GLOB '*[^0-9a-f]*')
    ),
    CHECK (length(request_kind) BETWEEN 1 AND 32),
    CHECK (length(feed_section) BETWEEN 1 AND 64),
    CHECK (length(algorithm_version) BETWEEN 1 AND 128),
    CHECK (length(config_version) BETWEEN 1 AND 128),
    CHECK (length(score_semantics) BETWEEN 1 AND 64),
    CHECK (length(context_hash) = 64 AND context_hash NOT GLOB '*[^0-9a-f]*'),
    CHECK (length(candidate_set_hash) = 64 AND candidate_set_hash NOT GLOB '*[^0-9a-f]*'),
    CHECK (expires_at_ms > created_at_ms)
);

CREATE INDEX idx_recommendation_runs_expires
    ON recommendation_runs (expires_at_ms, recommendation_run_id);

CREATE INDEX idx_recommendation_runs_subject_created
    ON recommendation_runs (subject_hash, created_at_ms DESC)
    WHERE subject_hash IS NOT NULL;

CREATE TABLE recommendation_items (
    recommendation_run_id TEXT NOT NULL
        REFERENCES recommendation_runs (recommendation_run_id) ON DELETE CASCADE,
    app_id INTEGER NOT NULL REFERENCES apps (app_id) ON DELETE CASCADE,
    rank INTEGER NOT NULL CHECK (rank > 0),
    relevance_score REAL NOT NULL,
    recommendation_index INTEGER
        CHECK (recommendation_index IS NULL OR recommendation_index BETWEEN 0 AND 100),
    data_confidence REAL NOT NULL CHECK (data_confidence BETWEEN 0 AND 1),
    slot_reason TEXT NOT NULL,
    score_components_json TEXT NOT NULL
        CHECK (json_valid(score_components_json) AND json_type(score_components_json) = 'object'),
    recorded_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    PRIMARY KEY (recommendation_run_id, app_id),
    UNIQUE (recommendation_run_id, rank),
    CHECK (length(slot_reason) BETWEEN 1 AND 32),
    CHECK (expires_at_ms > recorded_at_ms)
);

CREATE INDEX idx_recommendation_items_expires
    ON recommendation_items (expires_at_ms, recommendation_run_id, app_id);

CREATE INDEX idx_recommendation_items_app_recorded
    ON recommendation_items (app_id, recorded_at_ms DESC);

CREATE TABLE recommendation_events (
    recommendation_event_id INTEGER PRIMARY KEY AUTOINCREMENT,
    recommendation_run_id TEXT NOT NULL,
    app_id INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    client_created_at_ms INTEGER,
    metadata_json TEXT NOT NULL DEFAULT '{}'
        CHECK (json_valid(metadata_json) AND json_type(metadata_json) = 'object'),
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    FOREIGN KEY (recommendation_run_id, app_id)
        REFERENCES recommendation_items (recommendation_run_id, app_id) ON DELETE CASCADE,
    UNIQUE (recommendation_run_id, idempotency_key),
    CHECK (length(event_type) BETWEEN 1 AND 64),
    CHECK (length(idempotency_key) BETWEEN 1 AND 128),
    CHECK (expires_at_ms > created_at_ms)
);

CREATE INDEX idx_recommendation_events_expires
    ON recommendation_events (expires_at_ms, recommendation_event_id);

CREATE INDEX idx_recommendation_events_run_app_created
    ON recommendation_events (
        recommendation_run_id,
        app_id,
        created_at_ms,
        recommendation_event_id
    );
