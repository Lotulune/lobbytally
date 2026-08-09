CREATE TABLE game_ingestion_lane_state (
    lane TEXT PRIMARY KEY CHECK (length(trim(lane)) > 0),
    pause_until_ms INTEGER NOT NULL CHECK (pause_until_ms >= 0),
    last_error_category TEXT NOT NULL
        CHECK (last_error_category IN ('auth', 'config')),
    last_error_summary TEXT NOT NULL
        CHECK (length(last_error_summary) <= 512),
    paused_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
