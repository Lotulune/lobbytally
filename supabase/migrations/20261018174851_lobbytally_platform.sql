-- Supabase-hosted platform resources. Apply after the PostgreSQL app baseline.
-- Cron is enabled separately, once the deployment URL and secret are in Vault.
CREATE EXTENSION IF NOT EXISTS pg_cron WITH SCHEMA pg_catalog;
CREATE EXTENSION IF NOT EXISTS pg_net WITH SCHEMA extensions;
CREATE EXTENSION IF NOT EXISTS supabase_vault WITH SCHEMA vault;

INSERT INTO storage.buckets (id, name, public, file_size_limit, allowed_mime_types)
VALUES ('avatars', 'avatars', false, 262144, ARRAY['image/webp'])
ON CONFLICT (id) DO UPDATE SET
    public = false,
    file_size_limit = excluded.file_size_limit,
    allowed_mime_types = excluded.allowed_mime_types;

CREATE OR REPLACE FUNCTION mpgs.dispatch_scheduled_request(endpoint TEXT)
RETURNS BIGINT
LANGUAGE plpgsql
SECURITY INVOKER
SET search_path = pg_catalog
AS $function$
DECLARE
    site_origin TEXT;
    cron_secret TEXT;
BEGIN
    IF endpoint NOT IN ('maintenance', 'avatar-cleanup') OR endpoint IS NULL THEN
        RAISE EXCEPTION 'Unsupported LobbyTally scheduled endpoint';
    END IF;
    SELECT rtrim(decrypted_secret, '/') INTO site_origin
      FROM vault.decrypted_secrets WHERE name = 'lobbytally_site_origin';
    SELECT decrypted_secret INTO cron_secret
      FROM vault.decrypted_secrets WHERE name = 'lobbytally_cron_secret';
    IF site_origin IS NULL OR site_origin !~ '^https://[A-Za-z0-9][A-Za-z0-9.-]*(:443)?$' THEN
        RAISE EXCEPTION 'Set lobbytally_site_origin in Vault to the public production HTTPS origin';
    END IF;
    IF cron_secret IS NULL OR length(cron_secret) < 32 OR cron_secret ~ '[[:space:]]' THEN
        RAISE EXCEPTION 'Set lobbytally_cron_secret in Vault to MPGS_CRON_SECRET';
    END IF;
    RETURN net.http_post(
        url := site_origin || '/internal/v1/' || endpoint,
        headers := jsonb_build_object(
            'Content-Type', 'application/json',
            'Authorization', 'Bearer ' || cron_secret
        ),
        body := '{}'::jsonb,
        timeout_milliseconds := 120000
    );
END;
$function$;

REVOKE ALL ON FUNCTION mpgs.dispatch_scheduled_request(TEXT) FROM PUBLIC, anon, authenticated;
