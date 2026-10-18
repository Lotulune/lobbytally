-- A dedicated backend login avoids distributing the project's postgres password.
-- The generated password stays encrypted in Vault; no credential is in Git/history.
CREATE ROLE lobbytally_runtime NOLOGIN NOINHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS;

DO $runtime_password$
DECLARE
    runtime_password TEXT;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM vault.secrets WHERE name = 'lobbytally_database_password') THEN
        PERFORM vault.create_secret(
            replace(gen_random_uuid()::text, '-', '') || replace(gen_random_uuid()::text, '-', ''),
            'lobbytally_database_password',
            'Database password for the LobbyTally backend role'
        );
    END IF;
    SELECT decrypted_secret INTO STRICT runtime_password
    FROM vault.decrypted_secrets WHERE name = 'lobbytally_database_password';
    EXECUTE format('ALTER ROLE lobbytally_runtime LOGIN PASSWORD %L', runtime_password);
END;
$runtime_password$;

GRANT USAGE ON SCHEMA mpgs, extensions TO lobbytally_runtime;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA mpgs TO lobbytally_runtime;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA mpgs TO lobbytally_runtime;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA mpgs TO lobbytally_runtime;
REVOKE EXECUTE ON FUNCTION mpgs.dispatch_scheduled_request(TEXT) FROM lobbytally_runtime;

-- Only the trusted server login can use these policies. Browser API roles remain denied.
DO $backend_policies$
DECLARE
    app_table RECORD;
BEGIN
    FOR app_table IN
        SELECT c.relname
        FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'mpgs' AND c.relkind IN ('r', 'p')
    LOOP
        EXECUTE format(
            'CREATE POLICY backend_runtime_access ON mpgs.%I FOR ALL TO lobbytally_runtime USING (true) WITH CHECK (true)',
            app_table.relname
        );
    END LOOP;
END;
$backend_policies$;
