-- Stops future scheduled calls; does not delete application data.
SELECT cron.unschedule(jobid)
FROM cron.job
WHERE jobname IN ('lobbytally-maintenance', 'lobbytally-avatar-cleanup');
