-- Feed and search resolve demo/playtest relations by target app. The primary
-- key starts with source_app_id, so target lookups otherwise scan the table for
-- every candidate.
CREATE INDEX idx_app_relations_target_type
    ON app_relations (target_app_id, relation_type);

-- Feed and calendar select the newest review across region/language scopes.
-- The primary key places those scopes before captured_at_ms and cannot satisfy
-- this ordering without a per-app temporary sort.
CREATE INDEX idx_review_snapshots_app_latest
    ON review_snapshots (app_id, captured_at_ms DESC, language_scope ASC);
