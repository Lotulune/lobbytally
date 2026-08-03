-- Distinguish untouched onboarding defaults from preferences a player has
-- actually confirmed. Existing rows predate the distinction and therefore
-- retain full confidence; newly-created rows explicitly write the domain
-- default (0.0) until onboarding/settings are confirmed.

ALTER TABLE user_preferences
    ADD COLUMN preference_confidence REAL NOT NULL DEFAULT 1.0
    CHECK (preference_confidence BETWEEN 0 AND 1);
