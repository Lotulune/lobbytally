-- M5: retrieval documents, FTS5, embeddings, and AI analysis cache.

CREATE TABLE IF NOT EXISTS game_documents (
    document_id TEXT PRIMARY KEY,
    app_id INTEGER NOT NULL REFERENCES apps(app_id) ON DELETE CASCADE,
    doc_type TEXT NOT NULL,
    language TEXT NOT NULL DEFAULT 'und',
    title TEXT NOT NULL DEFAULT '',
    body TEXT NOT NULL DEFAULT '',
    content_hash TEXT NOT NULL,
    visibility TEXT NOT NULL DEFAULT 'public',
    updated_at_ms INTEGER NOT NULL,
    CHECK (doc_type IN (
        'identity',
        'store_summary',
        'multiplayer_profile',
        'review_topics',
        'curation_notes'
    )),
    CHECK (visibility IN ('public', 'internal'))
);

CREATE INDEX IF NOT EXISTS idx_game_documents_app
    ON game_documents(app_id, doc_type);

-- Explicitly synchronized FTS index (no hidden content triggers).
CREATE VIRTUAL TABLE IF NOT EXISTS game_fts USING fts5(
    document_id UNINDEXED,
    app_id UNINDEXED,
    title,
    aliases,
    tags,
    body,
    tokenize = 'unicode61'
);

CREATE TABLE IF NOT EXISTS game_embeddings (
    document_id TEXT NOT NULL REFERENCES game_documents(document_id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    dimensions INTEGER NOT NULL,
    vector_blob BLOB NOT NULL,
    is_l2_normalized INTEGER NOT NULL DEFAULT 1,
    content_hash TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (document_id, provider, model, content_hash),
    CHECK (dimensions > 0),
    CHECK (is_l2_normalized IN (0, 1))
);

CREATE INDEX IF NOT EXISTS idx_game_embeddings_doc
    ON game_embeddings(document_id, provider, model);

CREATE TABLE IF NOT EXISTS ai_analyses (
    analysis_id TEXT PRIMARY KEY,
    app_id INTEGER NOT NULL REFERENCES apps(app_id) ON DELETE CASCADE,
    task_type TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt_version TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    raw_output_json TEXT NOT NULL,
    accepted_json TEXT,
    validation_status TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    CHECK (validation_status IN ('accepted', 'rejected', 'pending_review'))
);

CREATE INDEX IF NOT EXISTS idx_ai_analyses_app
    ON ai_analyses(app_id, task_type, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS ai_analysis_cache (
    cache_key TEXT PRIMARY KEY,
    task_type TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt_version TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    output_json TEXT NOT NULL,
    validation_status TEXT NOT NULL,
    usage_input INTEGER NOT NULL DEFAULT 0,
    usage_output INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    CHECK (validation_status IN ('accepted', 'rejected', 'pending_review'))
);

CREATE INDEX IF NOT EXISTS idx_ai_analysis_cache_expiry
    ON ai_analysis_cache(expires_at_ms);
