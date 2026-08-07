-- Keep the latest review projection small and directly addressable. Feed and
-- calendar reads should not rebuild this projection from the full history on
-- every request.

CREATE TABLE latest_review_snapshots (
    app_id INTEGER PRIMARY KEY REFERENCES apps (app_id) ON DELETE CASCADE,
    region_scope TEXT NOT NULL,
    language_scope TEXT NOT NULL,
    captured_at_ms INTEGER NOT NULL,
    total_positive INTEGER NOT NULL CHECK (total_positive >= 0),
    total_negative INTEGER NOT NULL CHECK (total_negative >= 0),
    total_reviews INTEGER NOT NULL CHECK (total_reviews >= 0),
    wilson_lower REAL
);

INSERT INTO latest_review_snapshots (
    app_id, region_scope, language_scope, captured_at_ms,
    total_positive, total_negative, total_reviews, wilson_lower
)
SELECT app_id, region_scope, language_scope, captured_at_ms,
       total_positive, total_negative, total_reviews, wilson_lower
FROM (
    SELECT review.*,
           ROW_NUMBER() OVER (
               PARTITION BY app_id
               ORDER BY captured_at_ms DESC, language_scope ASC, region_scope ASC
           ) AS snapshot_rank
    FROM review_snapshots review
)
WHERE snapshot_rank = 1;

CREATE TRIGGER trg_review_snapshots_latest_insert
AFTER INSERT ON review_snapshots
BEGIN
    INSERT INTO latest_review_snapshots (
        app_id, region_scope, language_scope, captured_at_ms,
        total_positive, total_negative, total_reviews, wilson_lower
    ) VALUES (
        NEW.app_id, NEW.region_scope, NEW.language_scope, NEW.captured_at_ms,
        NEW.total_positive, NEW.total_negative, NEW.total_reviews, NEW.wilson_lower
    )
    ON CONFLICT(app_id) DO UPDATE SET
        region_scope = excluded.region_scope,
        language_scope = excluded.language_scope,
        captured_at_ms = excluded.captured_at_ms,
        total_positive = excluded.total_positive,
        total_negative = excluded.total_negative,
        total_reviews = excluded.total_reviews,
        wilson_lower = excluded.wilson_lower
    WHERE excluded.captured_at_ms > latest_review_snapshots.captured_at_ms
       OR (
           excluded.captured_at_ms = latest_review_snapshots.captured_at_ms
           AND excluded.language_scope < latest_review_snapshots.language_scope
       )
       OR (
           excluded.captured_at_ms = latest_review_snapshots.captured_at_ms
           AND excluded.language_scope = latest_review_snapshots.language_scope
           AND excluded.region_scope <= latest_review_snapshots.region_scope
       );
END;

CREATE TRIGGER trg_review_snapshots_latest_update
AFTER UPDATE ON review_snapshots
BEGIN
    DELETE FROM latest_review_snapshots WHERE app_id = OLD.app_id;
    INSERT INTO latest_review_snapshots (
        app_id, region_scope, language_scope, captured_at_ms,
        total_positive, total_negative, total_reviews, wilson_lower
    )
    SELECT app_id, region_scope, language_scope, captured_at_ms,
           total_positive, total_negative, total_reviews, wilson_lower
    FROM review_snapshots
    WHERE app_id = OLD.app_id
    ORDER BY captured_at_ms DESC, language_scope ASC, region_scope ASC
    LIMIT 1;
    INSERT INTO latest_review_snapshots (
        app_id, region_scope, language_scope, captured_at_ms,
        total_positive, total_negative, total_reviews, wilson_lower
    )
    SELECT app_id, region_scope, language_scope, captured_at_ms,
           total_positive, total_negative, total_reviews, wilson_lower
    FROM review_snapshots
    WHERE app_id = NEW.app_id
    ORDER BY captured_at_ms DESC, language_scope ASC, region_scope ASC
    LIMIT 1
    ON CONFLICT(app_id) DO UPDATE SET
        region_scope = excluded.region_scope,
        language_scope = excluded.language_scope,
        captured_at_ms = excluded.captured_at_ms,
        total_positive = excluded.total_positive,
        total_negative = excluded.total_negative,
        total_reviews = excluded.total_reviews,
        wilson_lower = excluded.wilson_lower;
END;

CREATE TRIGGER trg_review_snapshots_latest_delete
AFTER DELETE ON review_snapshots
BEGIN
    DELETE FROM latest_review_snapshots WHERE app_id = OLD.app_id;
    INSERT INTO latest_review_snapshots (
        app_id, region_scope, language_scope, captured_at_ms,
        total_positive, total_negative, total_reviews, wilson_lower
    )
    SELECT app_id, region_scope, language_scope, captured_at_ms,
           total_positive, total_negative, total_reviews, wilson_lower
    FROM review_snapshots
    WHERE app_id = OLD.app_id
    ORDER BY captured_at_ms DESC, language_scope ASC, region_scope ASC
    LIMIT 1;
END;
