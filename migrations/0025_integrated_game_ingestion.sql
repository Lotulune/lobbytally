ALTER TABLE store_detail_refresh_state
    ADD COLUMN checked_empty INTEGER NOT NULL DEFAULT 0
    CHECK (checked_empty IN (0, 1));

CREATE TABLE game_ingestion_queue (
    app_id INTEGER PRIMARY KEY REFERENCES apps(app_id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    stage TEXT NOT NULL DEFAULT 'store_details'
        CHECK (stage IN ('store_details', 'review_summary', 'popular_reviews', 'ccu', 'complete')),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'retry', 'complete')),
    stage_attempts INTEGER NOT NULL DEFAULT 0 CHECK (stage_attempts >= 0),
    total_attempts INTEGER NOT NULL DEFAULT 0 CHECK (total_attempts >= 0),
    next_attempt_at_ms INTEGER NOT NULL,
    last_error_category TEXT,
    lease_owner TEXT,
    lease_expires_at_ms INTEGER,
    discovered_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    CHECK (
        (lease_owner IS NULL AND lease_expires_at_ms IS NULL)
        OR (lease_owner IS NOT NULL AND lease_expires_at_ms IS NOT NULL)
    )
);

CREATE INDEX idx_game_ingestion_queue_claim
    ON game_ingestion_queue(status, next_attempt_at_ms, priority DESC, discovered_at_ms, app_id);

CREATE TABLE pipeline_retention_summary (
    status TEXT PRIMARY KEY CHECK (status IN ('completed', 'dead')),
    deleted_count INTEGER NOT NULL DEFAULT 0 CHECK (deleted_count >= 0),
    last_cutoff_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
