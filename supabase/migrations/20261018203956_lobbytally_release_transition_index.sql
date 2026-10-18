-- Additive performance change: keep mpgs.schema_migrations at version 1 so
-- the currently deployed application remains ready during a rolling update.
-- Native integration tests set mpgs.schema to an isolated schema; the linked
-- Supabase migration runner uses the production mpgs schema by default.
DO $release_transition_index$
DECLARE
    app_schema TEXT := coalesce(nullif(current_setting('mpgs.schema', true), ''), 'mpgs');
BEGIN
    EXECUTE format(
        'CREATE INDEX IF NOT EXISTS idx_release_events_to_released
         ON %I.release_events (app_id, observed_at_ms DESC)
         WHERE new_release_state = %L AND old_release_state <> %L',
        app_schema, 'released', 'released'
    );
END;
$release_transition_index$;
