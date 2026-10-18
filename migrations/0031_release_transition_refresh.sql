-- Dynamic review/CCU freshness is anchored to the latest actual transition
-- into the released state. Keep that lookup small and index-only so the
-- enrichment candidate scan does not regress to a release_events table scan.
CREATE INDEX idx_release_events_to_released
    ON release_events (app_id, observed_at_ms DESC)
    WHERE new_release_state = 'released'
      AND old_release_state <> 'released';
