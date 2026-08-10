-- Feed reads resolve the same four active features for every candidate. The
-- authoritative evidence history contains nearly two million rows, while the
-- active Feed subset is only about twenty-five thousand rows. Mirror that
-- subset so cold projections avoid thousands of lookups in the large history
-- table and its indexes.
CREATE TABLE feed_feature_evidence (
    evidence_id INTEGER PRIMARY KEY,
    app_id INTEGER NOT NULL REFERENCES apps (app_id) ON DELETE CASCADE,
    feature_name TEXT NOT NULL CHECK (
        feature_name IN (
            'matchmaking_core',
            'public_world_dependency',
            'service_shutdown_risk',
            'catalog_taxonomy'
        )
    ),
    value_json TEXT NOT NULL,
    confidence REAL NOT NULL CHECK (confidence >= 0 AND confidence <= 1),
    observed_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER
);

INSERT INTO feed_feature_evidence (
    evidence_id, app_id, feature_name, value_json, confidence,
    observed_at_ms, expires_at_ms
)
SELECT evidence_id, app_id, feature_name, value_json, confidence,
       observed_at_ms, expires_at_ms
FROM feature_evidence
WHERE is_active = 1
  AND feature_name IN (
      'matchmaking_core',
      'public_world_dependency',
      'service_shutdown_risk',
      'catalog_taxonomy'
  );

CREATE INDEX idx_feed_feature_evidence_latest
    ON feed_feature_evidence (
        app_id,
        feature_name,
        observed_at_ms DESC,
        evidence_id DESC,
        expires_at_ms,
        value_json,
        confidence
    );

CREATE TRIGGER trg_feed_feature_evidence_insert
AFTER INSERT ON feature_evidence
WHEN NEW.is_active = 1
  AND NEW.feature_name IN (
      'matchmaking_core',
      'public_world_dependency',
      'service_shutdown_risk',
      'catalog_taxonomy'
  )
BEGIN
    INSERT OR REPLACE INTO feed_feature_evidence (
        evidence_id, app_id, feature_name, value_json, confidence,
        observed_at_ms, expires_at_ms
    ) VALUES (
        NEW.evidence_id, NEW.app_id, NEW.feature_name, NEW.value_json,
        NEW.confidence, NEW.observed_at_ms, NEW.expires_at_ms
    );
END;

CREATE TRIGGER trg_feed_feature_evidence_update
AFTER UPDATE ON feature_evidence
WHEN OLD.feature_name IN (
         'matchmaking_core',
         'public_world_dependency',
         'service_shutdown_risk',
         'catalog_taxonomy'
     )
  OR NEW.feature_name IN (
         'matchmaking_core',
         'public_world_dependency',
         'service_shutdown_risk',
         'catalog_taxonomy'
     )
BEGIN
    DELETE FROM feed_feature_evidence
    WHERE evidence_id = OLD.evidence_id;

    INSERT OR REPLACE INTO feed_feature_evidence (
        evidence_id, app_id, feature_name, value_json, confidence,
        observed_at_ms, expires_at_ms
    )
    SELECT NEW.evidence_id, NEW.app_id, NEW.feature_name, NEW.value_json,
           NEW.confidence, NEW.observed_at_ms, NEW.expires_at_ms
    WHERE NEW.is_active = 1
      AND NEW.feature_name IN (
          'matchmaking_core',
          'public_world_dependency',
          'service_shutdown_risk',
          'catalog_taxonomy'
      );
END;

CREATE TRIGGER trg_feed_feature_evidence_delete
AFTER DELETE ON feature_evidence
WHEN OLD.feature_name IN (
    'matchmaking_core',
    'public_world_dependency',
    'service_shutdown_risk',
    'catalog_taxonomy'
)
BEGIN
    DELETE FROM feed_feature_evidence
    WHERE evidence_id = OLD.evidence_id;
END;
