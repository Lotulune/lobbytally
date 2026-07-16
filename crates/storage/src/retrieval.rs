//! Game retrieval documents, FTS sync, embeddings, and AI cache.

use rusqlite::{OptionalExtension, params};

use crate::error::{StorageError, StorageResult};
use crate::repo::Repository;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameDocument {
    pub document_id: String,
    pub app_id: u32,
    pub doc_type: String,
    pub language: String,
    pub title: String,
    pub body: String,
    pub content_hash: String,
    pub visibility: String,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertGameDocument {
    pub document_id: String,
    pub app_id: u32,
    pub doc_type: String,
    pub language: String,
    pub title: String,
    pub body: String,
    pub content_hash: String,
    pub aliases: String,
    pub tags: String,
    pub visibility: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FtsHit {
    pub document_id: String,
    pub app_id: u32,
    pub rank: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiCacheEntry {
    pub cache_key: String,
    pub task_type: String,
    pub provider: String,
    pub model: String,
    pub prompt_version: String,
    pub input_hash: String,
    pub output_json: String,
    pub validation_status: String,
    pub usage_input: i64,
    pub usage_output: i64,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEmbedding {
    pub document_id: String,
    pub app_id: u32,
    pub vector_blob: Vec<u8>,
    pub dimensions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutEmbedding {
    pub document_id: String,
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
    pub vector_blob: Vec<u8>,
    pub is_l2_normalized: bool,
    pub content_hash: String,
}

impl Repository {
    pub fn upsert_game_document(&self, doc: &UpsertGameDocument) -> StorageResult<()> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO game_documents(
                    document_id, app_id, doc_type, language, title, body,
                    content_hash, visibility, updated_at_ms
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(document_id) DO UPDATE SET
                    app_id=excluded.app_id,
                    doc_type=excluded.doc_type,
                    language=excluded.language,
                    title=excluded.title,
                    body=excluded.body,
                    content_hash=excluded.content_hash,
                    visibility=excluded.visibility,
                    updated_at_ms=excluded.updated_at_ms",
                params![
                    doc.document_id,
                    doc.app_id,
                    doc.doc_type,
                    doc.language,
                    doc.title,
                    doc.body,
                    doc.content_hash,
                    doc.visibility,
                    now
                ],
            )?;
            tx.execute(
                "DELETE FROM game_fts WHERE document_id = ?1",
                params![doc.document_id],
            )?;
            tx.execute(
                "INSERT INTO game_fts(document_id, app_id, title, aliases, tags, body)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    doc.document_id,
                    doc.app_id as i64,
                    doc.title,
                    doc.aliases,
                    doc.tags,
                    doc.body
                ],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn search_game_fts(&self, query: &str, limit: u32) -> StorageResult<Vec<FtsHit>> {
        let limit = limit.clamp(1, 100);
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT document_id, app_id, bm25(game_fts) AS rank
                 FROM game_fts
                 WHERE game_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![q, limit as i64], |row| {
                Ok(FtsHit {
                    document_id: row.get(0)?,
                    app_id: row.get::<_, i64>(1)? as u32,
                    rank: row.get(2)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    pub fn put_embedding(&self, embedding: &PutEmbedding) -> StorageResult<()> {
        if embedding.dimensions == 0
            || embedding.vector_blob.len() != embedding.dimensions * 4
        {
            return Err(StorageError::validation(
                "embedding dimensions do not match vector blob length",
            ));
        }
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO game_embeddings(
                    document_id, provider, model, dimensions, vector_blob,
                    is_l2_normalized, content_hash, created_at_ms
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(document_id, provider, model, content_hash) DO UPDATE SET
                    dimensions=excluded.dimensions,
                    vector_blob=excluded.vector_blob,
                    is_l2_normalized=excluded.is_l2_normalized,
                    created_at_ms=excluded.created_at_ms",
                params![
                    embedding.document_id,
                    embedding.provider,
                    embedding.model,
                    embedding.dimensions as i64,
                    embedding.vector_blob,
                    i64::from(embedding.is_l2_normalized),
                    embedding.content_hash,
                    now
                ],
            )?;
            Ok(())
        })
    }

    pub fn list_embeddings_for_provider(
        &self,
        provider: &str,
        model: &str,
        limit: u32,
    ) -> StorageResult<Vec<StoredEmbedding>> {
        let limit = limit.clamp(1, 10_000);
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT e.document_id, d.app_id, e.vector_blob, e.dimensions
                 FROM game_embeddings e
                 JOIN game_documents d ON d.document_id = e.document_id
                 WHERE e.provider = ?1 AND e.model = ?2
                 ORDER BY e.created_at_ms DESC
                 LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![provider, model, limit as i64], |row| {
                Ok(StoredEmbedding {
                    document_id: row.get(0)?,
                    app_id: row.get::<_, i64>(1)? as u32,
                    vector_blob: row.get(2)?,
                    dimensions: row.get::<_, i64>(3)? as usize,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    pub fn get_ai_cache(&self, cache_key: &str, now_ms: i64) -> StorageResult<Option<AiCacheEntry>> {
        self.db.with_conn(|conn| {
            conn.query_row(
                "SELECT cache_key, task_type, provider, model, prompt_version, input_hash,
                        output_json, validation_status, usage_input, usage_output,
                        created_at_ms, expires_at_ms
                 FROM ai_analysis_cache
                 WHERE cache_key = ?1 AND expires_at_ms > ?2",
                params![cache_key, now_ms],
                |row| {
                    Ok(AiCacheEntry {
                        cache_key: row.get(0)?,
                        task_type: row.get(1)?,
                        provider: row.get(2)?,
                        model: row.get(3)?,
                        prompt_version: row.get(4)?,
                        input_hash: row.get(5)?,
                        output_json: row.get(6)?,
                        validation_status: row.get(7)?,
                        usage_input: row.get(8)?,
                        usage_output: row.get(9)?,
                        created_at_ms: row.get(10)?,
                        expires_at_ms: row.get(11)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn put_ai_cache(&self, entry: &AiCacheEntry) -> StorageResult<()> {
        self.db.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO ai_analysis_cache(
                    cache_key, task_type, provider, model, prompt_version, input_hash,
                    output_json, validation_status, usage_input, usage_output,
                    created_at_ms, expires_at_ms
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
                 ON CONFLICT(cache_key) DO UPDATE SET
                    task_type=excluded.task_type,
                    provider=excluded.provider,
                    model=excluded.model,
                    prompt_version=excluded.prompt_version,
                    input_hash=excluded.input_hash,
                    output_json=excluded.output_json,
                    validation_status=excluded.validation_status,
                    usage_input=excluded.usage_input,
                    usage_output=excluded.usage_output,
                    created_at_ms=excluded.created_at_ms,
                    expires_at_ms=excluded.expires_at_ms",
                params![
                    entry.cache_key,
                    entry.task_type,
                    entry.provider,
                    entry.model,
                    entry.prompt_version,
                    entry.input_hash,
                    entry.output_json,
                    entry.validation_status,
                    entry.usage_input,
                    entry.usage_output,
                    entry.created_at_ms,
                    entry.expires_at_ms
                ],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn repo() -> Repository {
        let db = Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        repo.seed_demo_if_empty().unwrap();
        repo
    }

    #[test]
    fn fts_roundtrip_and_search() {
        let repo = repo();
        // Use a seeded app id if present; otherwise skip dependency by using first app.
        let app_id = repo
            .database()
            .with_conn(|conn| {
                conn.query_row("SELECT app_id FROM apps LIMIT 1", [], |row| row.get::<_, i64>(0))
                    .map(|v| v as u32)
                    .map_err(StorageError::from)
            })
            .unwrap();
        repo.upsert_game_document(&UpsertGameDocument {
            document_id: format!("doc-{app_id}-identity"),
            app_id,
            doc_type: "identity".into(),
            language: "en".into(),
            title: "Cozy Co-op Adventure".into(),
            body: "private lobby cooperative replayable friends".into(),
            content_hash: "h1".into(),
            aliases: "cozycoop".into(),
            tags: "coop multiplayer".into(),
            visibility: "public".into(),
        })
        .unwrap();
        let hits = repo.search_game_fts("cooperative", 10).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].app_id, app_id);
    }

    #[test]
    fn embedding_and_cache_roundtrip() {
        let repo = repo();
        let app_id = repo
            .database()
            .with_conn(|conn| {
                conn.query_row("SELECT app_id FROM apps LIMIT 1", [], |row| row.get::<_, i64>(0))
                    .map(|v| v as u32)
                    .map_err(StorageError::from)
            })
            .unwrap();
        let doc_id = format!("doc-{app_id}-store");
        repo.upsert_game_document(&UpsertGameDocument {
            document_id: doc_id.clone(),
            app_id,
            doc_type: "store_summary".into(),
            language: "en".into(),
            title: "Game".into(),
            body: "body".into(),
            content_hash: "h2".into(),
            aliases: String::new(),
            tags: String::new(),
            visibility: "public".into(),
        })
        .unwrap();
        let blob = 1.0f32.to_le_bytes().to_vec();
        repo.put_embedding(&PutEmbedding {
            document_id: doc_id.clone(),
            provider: "hash-embed".into(),
            model: "hash-embed-v1".into(),
            dimensions: 1,
            vector_blob: blob,
            is_l2_normalized: true,
            content_hash: "h2".into(),
        })
        .unwrap();
        let listed = repo
            .list_embeddings_for_provider("hash-embed", "hash-embed-v1", 10)
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].document_id, doc_id);

        let entry = AiCacheEntry {
            cache_key: "k1".into(),
            task_type: "rank_analysis".into(),
            provider: "fake".into(),
            model: "fake-model".into(),
            prompt_version: "v1".into(),
            input_hash: "ih".into(),
            output_json: "{\"ok\":true}".into(),
            validation_status: "accepted".into(),
            usage_input: 1,
            usage_output: 2,
            created_at_ms: 100,
            expires_at_ms: 9_999_999_999_999,
        };
        repo.put_ai_cache(&entry).unwrap();
        let loaded = repo.get_ai_cache("k1", 200).unwrap().unwrap();
        assert_eq!(loaded.output_json, entry.output_json);
        assert!(repo.get_ai_cache("k1", 10_000_000_000_000).unwrap().is_none());
    }
}
