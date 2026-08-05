-- Feed queries resolve the newest active evidence for several features on
-- every candidate. Include the freshness/order columns so SQLite can satisfy
-- each lookup directly instead of sorting an app's evidence history.
CREATE INDEX idx_feature_evidence_feed_latest
    ON feature_evidence (
        app_id,
        feature_name,
        is_active,
        observed_at_ms DESC,
        evidence_id DESC
    );

-- Steam prices are regional. The previous index omitted country_code and
-- could scan another country's snapshots before finding the requested row.
CREATE INDEX idx_price_snapshots_region_latest
    ON price_snapshots (
        app_id,
        currency,
        country_code,
        captured_at_ms DESC
    );
