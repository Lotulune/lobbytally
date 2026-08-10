-- Worker enrichment only needs active, credible category hints. Keep this
-- candidate source small so each quota pass does not probe the full evidence
-- history once for every catalog app.
CREATE INDEX idx_feature_evidence_enrichment_candidates
    ON feature_evidence (app_id)
    WHERE feature_name = 'category_hint'
      AND is_active = 1
      AND confidence >= 0.3;
