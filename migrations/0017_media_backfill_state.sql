-- Bounded media-gallery backfill bookkeeping.
-- Used when multiplayer candidates lack app_media_assets rows after the media
-- feature shipped. Coverage-gated and attempt-limited so workers do not loop.

CREATE TABLE app_media_backfill_state (
    app_id INTEGER PRIMARY KEY REFERENCES apps (app_id) ON DELETE CASCADE,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_attempt_at_ms INTEGER,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'complete', 'none', 'failed', 'exhausted')),
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX idx_app_media_backfill_status
    ON app_media_backfill_state (status, last_attempt_at_ms, app_id);
