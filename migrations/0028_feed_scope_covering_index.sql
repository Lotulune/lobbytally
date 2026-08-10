-- Feed section scope scans only need app_id after applying type, release state,
-- and date predicates. The existing release index must visit the apps table to
-- reject non-candidate types. Keep app_type and app_id explicit as payload so
-- SQLite can evaluate and return the scope without reopening apps for each row.
CREATE INDEX idx_apps_feed_release_scope
    ON apps (release_state, release_date DESC, app_type, app_id)
    WHERE app_type IN ('game', 'demo', 'playtest');
