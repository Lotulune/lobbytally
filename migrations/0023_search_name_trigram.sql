-- Keep the public name search fast without changing its substring semantics.
-- FTS5's trigram tokenizer supports Unicode (including Chinese) middle-word
-- matches; the API still verifies every candidate with the original LIKE
-- predicate before returning it.

CREATE VIRTUAL TABLE app_name_fts USING fts5(
    app_id UNINDEXED,
    language UNINDEXED,
    name,
    tokenize = 'trigram'
);

INSERT INTO app_name_fts (app_id, language, name)
SELECT app_id, 'canonical', canonical_name
FROM apps
WHERE trim(canonical_name) <> '';

INSERT INTO app_name_fts (app_id, language, name)
SELECT app_id, lower(language), name
FROM app_localizations
WHERE lower(language) IN ('schinese', 'english', 'en')
  AND name IS NOT NULL
  AND trim(name) <> '';

CREATE TRIGGER trg_app_name_fts_apps_insert
AFTER INSERT ON apps
WHEN trim(NEW.canonical_name) <> ''
BEGIN
    INSERT INTO app_name_fts (app_id, language, name)
    VALUES (NEW.app_id, 'canonical', NEW.canonical_name);
END;

CREATE TRIGGER trg_app_name_fts_apps_update
AFTER UPDATE OF canonical_name ON apps
WHEN OLD.canonical_name IS NOT NEW.canonical_name
BEGIN
    DELETE FROM app_name_fts
    WHERE app_id = OLD.app_id AND language = 'canonical';
    INSERT INTO app_name_fts (app_id, language, name)
    SELECT NEW.app_id, 'canonical', NEW.canonical_name
    WHERE trim(NEW.canonical_name) <> '';
END;

CREATE TRIGGER trg_app_name_fts_localization_insert
AFTER INSERT ON app_localizations
WHEN lower(NEW.language) IN ('schinese', 'english', 'en')
  AND NEW.name IS NOT NULL
  AND trim(NEW.name) <> ''
BEGIN
    INSERT INTO app_name_fts (app_id, language, name)
    VALUES (NEW.app_id, lower(NEW.language), NEW.name);
END;

CREATE TRIGGER trg_app_name_fts_localization_update
AFTER UPDATE OF language, name ON app_localizations
WHEN lower(OLD.language) IN ('schinese', 'english', 'en')
  OR lower(NEW.language) IN ('schinese', 'english', 'en')
BEGIN
    DELETE FROM app_name_fts
    WHERE app_id = OLD.app_id AND language = lower(OLD.language);
    INSERT INTO app_name_fts (app_id, language, name)
    SELECT NEW.app_id, lower(NEW.language), NEW.name
    WHERE lower(NEW.language) IN ('schinese', 'english', 'en')
      AND NEW.name IS NOT NULL
      AND trim(NEW.name) <> '';
END;

CREATE TRIGGER trg_app_name_fts_localization_delete
AFTER DELETE ON app_localizations
WHEN lower(OLD.language) IN ('schinese', 'english', 'en')
BEGIN
    DELETE FROM app_name_fts
    WHERE app_id = OLD.app_id AND language = lower(OLD.language);
END;
