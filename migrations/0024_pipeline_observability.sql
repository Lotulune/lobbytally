CREATE INDEX idx_feature_evidence_active_source
    ON feature_evidence (app_id, feature_name, source_type, is_active, evidence_id DESC);

CREATE TABLE pipeline_status_snapshot (
    snapshot_id INTEGER PRIMARY KEY CHECK (snapshot_id = 1),
    generated_at_ms INTEGER NOT NULL,
    snapshot_json TEXT NOT NULL CHECK (json_valid(snapshot_json))
);

INSERT OR IGNORE INTO data_refresh_state (
    task_name, last_success_at_ms, next_run_at_ms, last_error_category,
    cursor_value, coverage_ratio, updated_at_ms
)
SELECT 'candidate_continuation', last_success_at_ms, next_run_at_ms,
       last_error_category, cursor_value, NULL, updated_at_ms
FROM data_refresh_state WHERE task_name = 'candidate_collection';

DELETE FROM data_refresh_state WHERE task_name = 'candidate_collection';
