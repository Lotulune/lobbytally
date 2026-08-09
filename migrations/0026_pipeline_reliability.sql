ALTER TABLE store_detail_refresh_state
    ADD COLUMN store_core_empty INTEGER NOT NULL DEFAULT 0
    CHECK (store_core_empty IN (0, 1));
ALTER TABLE store_detail_refresh_state
    ADD COLUMN price_empty INTEGER NOT NULL DEFAULT 0
    CHECK (price_empty IN (0, 1));
ALTER TABLE store_detail_refresh_state
    ADD COLUMN store_checked_at_ms INTEGER NOT NULL DEFAULT 0;
ALTER TABLE store_detail_refresh_state
    ADD COLUMN price_checked_at_ms INTEGER NOT NULL DEFAULT 0;

-- Migration 25 combined unrelated empty dimensions. Preserve fresh successful
-- rows, but force one bounded re-check for ambiguous checked-empty rows.
UPDATE store_detail_refresh_state
SET store_checked_at_ms = CASE WHEN checked_empty = 1 THEN 0 ELSE captured_at_ms END,
    price_checked_at_ms = CASE WHEN checked_empty = 1 THEN 0 ELSE captured_at_ms END;

ALTER TABLE game_ingestion_queue RENAME TO game_ingestion_queue_v25;

CREATE TABLE game_ingestion_queue (
    app_id INTEGER PRIMARY KEY REFERENCES apps(app_id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    stage TEXT NOT NULL DEFAULT 'store_details'
        CHECK (stage IN ('store_details', 'review_summary', 'popular_reviews', 'ccu', 'complete')),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'retry', 'complete', 'dead')),
    stage_failure_attempts INTEGER NOT NULL DEFAULT 0 CHECK (stage_failure_attempts >= 0),
    total_failure_attempts INTEGER NOT NULL DEFAULT 0 CHECK (total_failure_attempts >= 0),
    lease_count INTEGER NOT NULL DEFAULT 0 CHECK (lease_count >= 0),
    next_attempt_at_ms INTEGER NOT NULL,
    last_error_category TEXT,
    last_error_summary TEXT CHECK (last_error_summary IS NULL OR length(last_error_summary) <= 512),
    lease_owner TEXT,
    lease_expires_at_ms INTEGER,
    enrichment_profile TEXT NOT NULL DEFAULT 'full_released'
        CHECK (enrichment_profile IN ('basic_upcoming', 'basic_demo', 'full_released', 'full_override')),
    profile_version INTEGER NOT NULL DEFAULT 1 CHECK (profile_version > 0),
    dead_at_ms INTEGER,
    discovered_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    CHECK (
        (lease_owner IS NULL AND lease_expires_at_ms IS NULL)
        OR (lease_owner IS NOT NULL AND lease_expires_at_ms IS NOT NULL)
    ),
    CHECK ((status = 'dead') = (dead_at_ms IS NOT NULL))
);

INSERT INTO game_ingestion_queue (
    app_id, source, priority, stage, status, stage_failure_attempts,
    total_failure_attempts, lease_count, next_attempt_at_ms,
    last_error_category, lease_owner, lease_expires_at_ms,
    enrichment_profile, profile_version, dead_at_ms, discovered_at_ms, updated_at_ms
)
SELECT queue.app_id, queue.source, queue.priority, queue.stage, queue.status, 0,
       0, queue.total_attempts, queue.next_attempt_at_ms,
       queue.last_error_category, queue.lease_owner, queue.lease_expires_at_ms,
       CASE
           WHEN apps.app_type IN ('demo', 'playtest') THEN 'basic_demo'
           WHEN apps.release_state IN ('upcoming', 'coming_soon') THEN 'basic_upcoming'
           ELSE 'full_released'
       END,
       1, NULL, queue.discovered_at_ms, queue.updated_at_ms
FROM game_ingestion_queue_v25 queue
JOIN apps ON apps.app_id = queue.app_id;

DROP TABLE game_ingestion_queue_v25;

CREATE INDEX idx_game_ingestion_queue_claim
    ON game_ingestion_queue(status, next_attempt_at_ms, priority DESC, discovered_at_ms, app_id);

CREATE TABLE game_ingestion_requeue_audit (
    audit_id INTEGER PRIMARY KEY AUTOINCREMENT,
    app_id INTEGER NOT NULL REFERENCES apps(app_id) ON DELETE CASCADE,
    stage TEXT NOT NULL,
    previous_status TEXT NOT NULL,
    operator TEXT NOT NULL CHECK (length(trim(operator)) BETWEEN 1 AND 128),
    reason TEXT NOT NULL CHECK (length(trim(reason)) BETWEEN 1 AND 512),
    requeued_at_ms INTEGER NOT NULL
);

CREATE INDEX idx_game_ingestion_requeue_audit_app
    ON game_ingestion_requeue_audit(app_id, requeued_at_ms DESC);
