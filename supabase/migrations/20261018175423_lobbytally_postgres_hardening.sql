-- Keep optional extension code outside the exposed public schema.
CREATE SCHEMA IF NOT EXISTS extensions;
DO $extension_location$
DECLARE
    extension_schema TEXT;
BEGIN
    SELECT n.nspname INTO extension_schema
    FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace
    WHERE e.extname = 'pg_trgm';
    IF extension_schema IS NULL THEN
        RAISE EXCEPTION 'Apply the LobbyTally baseline before this migration';
    ELSIF extension_schema <> 'extensions' THEN
        ALTER EXTENSION pg_trgm SET SCHEMA extensions;
    END IF;
END;
$extension_location$;

-- Cover referencing columns used by account/game cleanup and foreign-key checks.
CREATE INDEX idx_account_sessions_replaced_by_session
    ON mpgs.account_sessions (replaced_by_session_id);
CREATE INDEX idx_feedback_events_app
    ON mpgs.feedback_events (app_id);
CREATE INDEX idx_release_events_app
    ON mpgs.release_events (app_id);
