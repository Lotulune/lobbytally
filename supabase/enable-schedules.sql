-- Run in the Supabase SQL editor after the two Vault secrets are configured.
-- Named schedules are updated in place by pg_cron when this is run again.
BEGIN;
DO $preflight$
DECLARE
    site_origin TEXT;
    cron_secret TEXT;
BEGIN
    SELECT rtrim(decrypted_secret, '/') INTO site_origin
      FROM vault.decrypted_secrets WHERE name = 'lobbytally_site_origin';
    SELECT decrypted_secret INTO cron_secret
      FROM vault.decrypted_secrets WHERE name = 'lobbytally_cron_secret';
    IF site_origin IS NULL OR site_origin !~ '^https://[A-Za-z0-9][A-Za-z0-9.-]*(:443)?$' THEN
        RAISE EXCEPTION 'Configure lobbytally_site_origin in Vault before enabling schedules';
    END IF;
    IF cron_secret IS NULL OR length(cron_secret) < 32 OR cron_secret ~ '[[:space:]]' THEN
        RAISE EXCEPTION 'Configure lobbytally_cron_secret in Vault before enabling schedules';
    END IF;
END;
$preflight$;

SELECT cron.schedule(
    'lobbytally-maintenance', '* * * * *',
    $job$SELECT mpgs.dispatch_scheduled_request('maintenance');$job$
);
SELECT cron.schedule(
    'lobbytally-avatar-cleanup', '*/5 * * * *',
    $job$SELECT mpgs.dispatch_scheduled_request('avatar-cleanup');$job$
);
COMMIT;
