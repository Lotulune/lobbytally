//! Game retrieval documents, FTS sync, embeddings, hybrid search, and AI cache.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::time::Duration;

const MAX_FTS_HITS: u32 = 900;
const MAX_HYBRID_RESULTS: u32 = 300;
const HYBRID_SOURCE_OVERSAMPLE: u32 = 3;
const RETRIEVAL_WRITER_HANDOFF: Duration = Duration::from_secs(1);

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::error::{StorageError, StorageResult};
use crate::repo::Repository;

pub const HASH_EMBED_PROVIDER: &str = "hash-embed";
pub const HASH_EMBED_MODEL: &str = "hash-embed-v2";
pub const HASH_EMBED_DIMENSIONS: usize = 64;

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetrievalSyncStats {
    pub apps_scanned: u32,
    pub documents_written: u32,
    pub documents_unchanged: u32,
    pub embeddings_written: u32,
    pub embeddings_unchanged: u32,
    /// Last catalog app processed by this batch.
    pub last_app_id: Option<u32>,
    /// Cursor to pass to the next batch. Zero means start a new full pass.
    pub next_after_app_id: u32,
    pub has_more: bool,
    pub catalog_apps: u32,
    pub apps_covered: u32,
}

impl RetrievalSyncStats {
    pub fn coverage_ratio(self) -> f64 {
        if self.catalog_apps == 0 {
            1.0
        } else {
            (f64::from(self.apps_covered) / f64::from(self.catalog_apps)).clamp(0.0, 1.0)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HybridHit {
    pub app_id: u32,
    pub score: f64,
    pub fts_rank: Option<f64>,
    pub vector_score: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct VectorRank {
    app_id: u32,
    score: f64,
}

impl PartialEq for VectorRank {
    fn eq(&self, other: &Self) -> bool {
        self.app_id == other.app_id && self.score.to_bits() == other.score.to_bits()
    }
}

impl Eq for VectorRank {}

impl PartialOrd for VectorRank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VectorRank {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            // For equal scores, the lower app id is the deterministic better rank.
            .then_with(|| other.app_id.cmp(&self.app_id))
    }
}

fn upsert_game_document_on_conn(
    conn: &Connection,
    doc: &UpsertGameDocument,
    now_ms: i64,
) -> StorageResult<bool> {
    if game_document_matches(conn, doc)? {
        return Ok(false);
    }
    conn.execute(
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
            now_ms
        ],
    )?;
    conn.execute(
        "DELETE FROM game_embeddings
         WHERE document_id = ?1 AND content_hash <> ?2",
        params![doc.document_id, doc.content_hash],
    )?;
    conn.execute(
        "DELETE FROM game_fts WHERE document_id = ?1",
        params![doc.document_id],
    )?;
    conn.execute(
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
    Ok(true)
}

fn game_document_matches(conn: &Connection, doc: &UpsertGameDocument) -> StorageResult<bool> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT content_hash FROM game_documents WHERE document_id = ?1",
            params![doc.document_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(existing.as_deref() == Some(doc.content_hash.as_str()))
}

fn validate_embedding(embedding: &PutEmbedding) -> StorageResult<()> {
    if embedding.dimensions == 0 || embedding.vector_blob.len() != embedding.dimensions * 4 {
        return Err(StorageError::validation(
            "embedding dimensions do not match vector blob length",
        ));
    }
    Ok(())
}

fn put_embedding_on_conn(
    conn: &Connection,
    embedding: &PutEmbedding,
    now_ms: i64,
) -> StorageResult<bool> {
    if embedding_matches(conn, embedding)? {
        return Ok(false);
    }
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
            now_ms
        ],
    )?;
    Ok(true)
}

fn embedding_matches(conn: &Connection, embedding: &PutEmbedding) -> StorageResult<bool> {
    let existing: Option<(i64, Vec<u8>, i64)> = conn
        .query_row(
            "SELECT dimensions, vector_blob, is_l2_normalized FROM game_embeddings
             WHERE document_id = ?1 AND provider = ?2 AND model = ?3 AND content_hash = ?4",
            params![
                embedding.document_id,
                embedding.provider,
                embedding.model,
                embedding.content_hash
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    Ok(existing
        .as_ref()
        .is_some_and(|(dimensions, blob, normalized)| {
            *dimensions == embedding.dimensions as i64
                && blob == &embedding.vector_blob
                && *normalized == i64::from(embedding.is_l2_normalized)
        }))
}

fn prune_managed_documents_on_conn(
    conn: &Connection,
    app_id: u32,
    keep_ids: &HashSet<String>,
) -> StorageResult<()> {
    for document_id in stale_managed_document_ids(conn, app_id, keep_ids)? {
        conn.execute(
            "DELETE FROM game_fts WHERE document_id = ?1",
            params![document_id],
        )?;
        conn.execute(
            "DELETE FROM game_documents WHERE document_id = ?1",
            params![document_id],
        )?;
    }
    Ok(())
}

fn stale_managed_document_ids(
    conn: &Connection,
    app_id: u32,
    keep_ids: &HashSet<String>,
) -> StorageResult<Vec<String>> {
    let managed_ids = HashSet::from([
        format!("app:{app_id}:identity"),
        format!("app:{app_id}:multiplayer_profile"),
        format!("app:{app_id}:store_summary"),
    ]);
    let stale_ids = {
        let mut stmt = conn.prepare(
            "SELECT document_id FROM game_documents
             WHERE app_id = ?1
               AND doc_type IN ('identity', 'multiplayer_profile', 'store_summary')",
        )?;
        let rows = stmt.query_map(params![app_id as i64], |row| row.get(0))?;
        let mut stale = Vec::new();
        for row in rows {
            let document_id: String = row?;
            if managed_ids.contains(&document_id) && !keep_ids.contains(&document_id) {
                stale.push(document_id);
            }
        }
        stale
    };
    Ok(stale_ids)
}

fn retrieval_app_needs_write(
    conn: &Connection,
    app_id: u32,
    keep_ids: &HashSet<String>,
    docs: &[(UpsertGameDocument, Option<PutEmbedding>)],
) -> StorageResult<bool> {
    if !stale_managed_document_ids(conn, app_id, keep_ids)?.is_empty() {
        return Ok(true);
    }
    for (doc, embedding) in docs {
        if !game_document_matches(conn, doc)? {
            return Ok(true);
        }
        if let Some(embedding) = embedding
            && !embedding_matches(conn, embedding)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

impl Repository {
    /// Upsert a retrieval document and keep FTS in sync.
    /// Returns `true` when content changed (or was newly inserted).
    pub fn upsert_game_document(&self, doc: &UpsertGameDocument) -> StorageResult<bool> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            if game_document_matches(conn, doc)? {
                return Ok(false);
            }
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let changed = upsert_game_document_on_conn(&tx, doc, now)?;
            tx.commit()?;
            Ok(changed)
        })
    }

    pub fn search_game_fts(&self, query: &str, limit: u32) -> StorageResult<Vec<FtsHit>> {
        // Hybrid recall may need more raw document hits than final app IDs because
        // one app can own several indexed documents. Keep the public repository
        // method bounded while allowing a 3x source window for a 300-app pool.
        let limit = limit.clamp(1, MAX_FTS_HITS);
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT game_fts.document_id, game_fts.app_id, bm25(game_fts) AS rank
                 FROM game_fts
                 JOIN game_documents d ON d.document_id = game_fts.document_id
                 WHERE game_fts MATCH ?1 AND d.visibility = 'public'
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

    /// Returns `true` when a new embedding row was inserted (same hash is a no-op).
    pub fn put_embedding(&self, embedding: &PutEmbedding) -> StorageResult<bool> {
        validate_embedding(embedding)?;
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            if embedding_matches(conn, embedding)? {
                return Ok(false);
            }
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let changed = put_embedding_on_conn(&tx, embedding, now)?;
            tx.commit()?;
            Ok(changed)
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
                   AND e.content_hash = d.content_hash
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

    pub fn get_ai_cache(
        &self,
        cache_key: &str,
        now_ms: i64,
    ) -> StorageResult<Option<AiCacheEntry>> {
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

    /// Incrementally rebuild retrieval documents (and optional hash embeddings) from catalog rows.
    pub fn sync_retrieval_from_catalog(
        &self,
        limit: u32,
        after_app_id: u32,
        write_embeddings: bool,
    ) -> StorageResult<RetrievalSyncStats> {
        let limit = limit.clamp(1, 50_000);
        let now_ms = self.db.now_ms();
        let (mut rows, catalog_apps): (Vec<CatalogDocSource>, u32) = self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT a.app_id, a.canonical_name, a.app_type, a.release_state,
                        COALESCE(p.dominant_mode, ''),
                        p.private_session, p.online_coop, p.self_hosted_server,
                        p.recommended_min_players, p.recommended_max_players,
                        COALESCE(v.platforms_json, '[]'),
                        COALESCE(v.languages_json, '[]'),
                        COALESCE(loc.name, ''),
                        COALESCE(loc.short_description, ''),
                        COALESCE((
                            SELECT evidence.value_json
                            FROM feature_evidence evidence
                            WHERE evidence.app_id = a.app_id
                              AND evidence.feature_name IN ('catalog_taxonomy', 'catalog_tags')
                              AND evidence.is_active = 1
                              AND (evidence.expires_at_ms IS NULL OR evidence.expires_at_ms > ?3)
                            ORDER BY CASE evidence.feature_name
                                         WHEN 'catalog_taxonomy' THEN 0 ELSE 1 END,
                                     evidence.observed_at_ms DESC, evidence.evidence_id DESC
                            LIMIT 1
                        ), '[]')
                 FROM apps a
                 LEFT JOIN multiplayer_profiles p ON p.app_id = a.app_id
                 LEFT JOIN app_availability v ON v.app_id = a.app_id
                 LEFT JOIN app_localizations loc ON loc.app_id = a.app_id AND loc.language = (
                    SELECT language FROM app_localizations l2
                    WHERE l2.app_id = a.app_id
                    ORDER BY CASE l2.language
                        WHEN 'schinese' THEN 0
                        WHEN 'english' THEN 1
                        WHEN 'en' THEN 2
                        ELSE 9 END
                    LIMIT 1
                 )
                 WHERE a.app_id > ?1
                 ORDER BY a.app_id ASC
                 LIMIT ?2",
            )?;
            let mapped = stmt.query_map(
                params![
                    after_app_id as i64,
                    i64::from(limit).saturating_add(1),
                    now_ms
                ],
                |row| {
                    Ok(CatalogDocSource {
                        app_id: row.get::<_, i64>(0)? as u32,
                        canonical_name: row.get(1)?,
                        app_type: row.get(2)?,
                        release_state: row.get(3)?,
                        dominant_mode: row.get(4)?,
                        private_session: row.get::<_, Option<i64>>(5)?.map(|v| v != 0),
                        online_coop: row.get::<_, Option<i64>>(6)?.map(|v| v != 0),
                        self_hosted_server: row.get::<_, Option<i64>>(7)?.map(|v| v != 0),
                        recommended_min: row.get::<_, Option<i64>>(8)?.map(|v| v as u8),
                        recommended_max: row.get::<_, Option<i64>>(9)?.map(|v| v as u8),
                        platforms_json: row.get(10)?,
                        languages_json: row.get(11)?,
                        localized_name: row.get(12)?,
                        short_description: row.get(13)?,
                        catalog_taxonomy_json: row.get(14)?,
                    })
                },
            )?;
            let mut out = Vec::new();
            for row in mapped {
                out.push(row?);
            }
            let catalog_apps =
                conn.query_row("SELECT COUNT(*) FROM apps", [], |row| row.get::<_, i64>(0))?;
            Ok((out, u32::try_from(catalog_apps).unwrap_or(u32::MAX)))
        })?;

        // Read one sentinel row to determine whether another page exists, but
        // never perform more than `limit` apps of derived work in one call.
        let has_more = rows.len() > limit as usize;
        if has_more {
            rows.truncate(limit as usize);
        }
        let last_app_id = rows.last().map(|row| row.app_id);
        let next_after_app_id = if has_more {
            last_app_id.unwrap_or(after_app_id)
        } else {
            0
        };

        let mut stats = RetrievalSyncStats {
            apps_scanned: rows.len() as u32,
            last_app_id,
            next_after_app_id,
            has_more,
            catalog_apps,
            ..RetrievalSyncStats::default()
        };

        // Hash and compare through a WAL reader before taking the writer. Only
        // changed apps need a short transaction; yielding between them prevents
        // background FTS maintenance from starving the external Steam worker.
        let prepared = rows
            .iter()
            .map(|source| {
                let docs = source.build_documents();
                let keep_ids: HashSet<String> =
                    docs.iter().map(|doc| doc.document_id.clone()).collect();
                let docs = docs
                    .into_iter()
                    .map(|doc| {
                        let embedding = write_embeddings.then(|| {
                            let text = format!("{} {}", doc.title, doc.body);
                            let vector = hash_embed_text(&text, HASH_EMBED_DIMENSIONS);
                            PutEmbedding {
                                document_id: doc.document_id.clone(),
                                provider: HASH_EMBED_PROVIDER.into(),
                                model: HASH_EMBED_MODEL.into(),
                                dimensions: HASH_EMBED_DIMENSIONS,
                                vector_blob: encode_f32_le(&vector),
                                is_l2_normalized: true,
                                content_hash: doc.content_hash.clone(),
                            }
                        });
                        (doc, embedding)
                    })
                    .collect::<Vec<_>>();
                (source.app_id, keep_ids, docs)
            })
            .collect::<Vec<_>>();
        let needs_write = self.db.with_conn(|conn| {
            prepared
                .iter()
                .map(|(app_id, keep_ids, docs)| {
                    retrieval_app_needs_write(conn, *app_id, keep_ids, docs)
                })
                .collect::<StorageResult<Vec<_>>>()
        })?;
        for ((app_id, keep_ids, docs), needs_write) in prepared.iter().zip(needs_write) {
            if !needs_write {
                stats.documents_unchanged += docs.len() as u32;
                stats.embeddings_unchanged += docs
                    .iter()
                    .filter(|(_, embedding)| embedding.is_some())
                    .count() as u32;
                continue;
            }
            self.db.with_conn_mut(|conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                prune_managed_documents_on_conn(&tx, *app_id, keep_ids)?;
                for (doc, embedding) in docs {
                    if upsert_game_document_on_conn(&tx, doc, now_ms)? {
                        stats.documents_written += 1;
                    } else {
                        stats.documents_unchanged += 1;
                    }
                    if let Some(embedding) = embedding {
                        if put_embedding_on_conn(&tx, embedding, now_ms)? {
                            stats.embeddings_written += 1;
                        } else {
                            stats.embeddings_unchanged += 1;
                        }
                    }
                }
                tx.commit()?;
                Ok(())
            })?;
            // SQLite's busy handler does not queue waiting writers fairly. A
            // short pause can let this connection reacquire the writer before
            // the external Steam worker wakes up, so leave a full handoff
            // window after every file-backed commit. In-memory databases
            // cannot have a second-process writer and should not slow tests.
            if self.db.path() != std::path::Path::new(":memory:") {
                std::thread::sleep(RETRIEVAL_WRITER_HANDOFF);
            }
        }
        stats.apps_covered = self.db.with_conn(|conn| {
            let count = conn.query_row(
                "SELECT COUNT(*)
                 FROM apps app
                 WHERE EXISTS (
                     SELECT 1 FROM game_documents document
                     WHERE document.app_id = app.app_id
                       AND document.document_id = 'app:' || app.app_id || ':identity'
                 )",
                [],
                |row| row.get::<_, i64>(0),
            )?;
            Ok(u32::try_from(count).unwrap_or(u32::MAX))
        })?;
        Ok(stats)
    }

    /// Hybrid retrieval over the default local hash-embed index.
    pub fn hybrid_search(&self, query: &str, limit: u32) -> StorageResult<Vec<HybridHit>> {
        self.hybrid_search_with_vector(query, &[], HASH_EMBED_PROVIDER, HASH_EMBED_MODEL, limit)
    }

    pub fn document_count(&self) -> StorageResult<i64> {
        self.db.with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM game_documents", [], |row| row.get(0))
                .map_err(StorageError::from)
        })
    }

    pub fn embedding_count(&self) -> StorageResult<i64> {
        self.db.with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM game_embeddings", [], |row| row.get(0))
                .map_err(StorageError::from)
        })
    }

    /// Documents whose current content_hash is not embedded for the given provider/model.
    pub fn list_documents_missing_embedding(
        &self,
        provider: &str,
        model: &str,
        dimensions: usize,
        limit: u32,
    ) -> StorageResult<Vec<DocumentEmbedTarget>> {
        let limit = limit.clamp(1, 10_000);
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT d.document_id, d.app_id, d.title, d.body, d.content_hash
                 FROM game_documents d
                 WHERE NOT EXISTS (
                    SELECT 1 FROM game_embeddings e
                    WHERE e.document_id = d.document_id
                      AND e.provider = ?1
                      AND e.model = ?2
                      AND e.content_hash = d.content_hash
                      AND e.dimensions = ?3
                 )
                 ORDER BY d.app_id ASC, d.document_id ASC
                 LIMIT ?4",
            )?;
            let rows = stmt.query_map(
                params![provider, model, dimensions as i64, limit as i64],
                |row| {
                    Ok(DocumentEmbedTarget {
                        document_id: row.get(0)?,
                        app_id: row.get::<_, i64>(1)? as u32,
                        title: row.get(2)?,
                        body: row.get(3)?,
                        content_hash: row.get(4)?,
                    })
                },
            )?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    /// Hybrid search using an explicit query vector and embedding provider/model.
    /// When `query_vector` is empty, falls back to local hash embedding of `query`.
    pub fn hybrid_search_with_vector(
        &self,
        query: &str,
        query_vector: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> StorageResult<Vec<HybridHit>> {
        let limit = limit.clamp(1, MAX_HYBRID_RESULTS);
        let source_limit = limit
            .saturating_mul(HYBRID_SOURCE_OVERSAMPLE)
            .min(MAX_FTS_HITS);
        let fts_query = fts_match_query(query);
        let fts_hits = if fts_query.is_empty() {
            Vec::new()
        } else {
            self.search_game_fts(&fts_query, source_limit)?
        };

        let mut fts_best: HashMap<u32, f64> = HashMap::new();
        let mut fts_order: Vec<u32> = Vec::new();
        for hit in &fts_hits {
            fts_best.entry(hit.app_id).or_insert_with(|| {
                fts_order.push(hit.app_id);
                -hit.rank
            });
        }

        let qvec: Vec<f32> = if query_vector.is_empty() {
            hash_embed_text(query, HASH_EMBED_DIMENSIONS)
        } else {
            query_vector.to_vec()
        };
        let dims = qvec.len();
        let vector_capacity = source_limit as usize;
        let mut vector_best: HashMap<u32, f64> = HashMap::new();
        let mut vector_min_heap: BinaryHeap<Reverse<VectorRank>> = BinaryHeap::new();
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT d.app_id, e.vector_blob, e.dimensions
                 FROM game_embeddings e
                 JOIN game_documents d ON d.document_id = e.document_id
                 WHERE e.provider = ?1 AND e.model = ?2
                   AND e.content_hash = d.content_hash
                   AND d.visibility = 'public'
                 ORDER BY e.rowid ASC",
            )?;
            let rows = stmt.query_map(params![provider, model], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u32,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)? as usize,
                ))
            })?;
            for row in rows {
                let (app_id, vector_blob, dimensions) = row?;
                if dimensions != dims {
                    continue;
                }
                let Ok(vector) = decode_f32_le(&vector_blob, dimensions) else {
                    continue;
                };
                let score = cosine_similarity(&qvec, &vector);
                if !score.is_finite() {
                    continue;
                }
                offer_bounded_vector_score(
                    &mut vector_best,
                    &mut vector_min_heap,
                    vector_capacity,
                    app_id,
                    score,
                );
            }
            Ok(())
        })?;
        let mut vector_order: Vec<(u32, f64)> = vector_best.iter().map(|(k, v)| (*k, *v)).collect();
        vector_order.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let vector_ids: Vec<u32> = vector_order
            .into_iter()
            .take(vector_capacity)
            .map(|(id, _)| id)
            .collect();

        let fused = reciprocal_rank_fusion(&[fts_order, vector_ids], 60);
        let mut out = Vec::new();
        for (app_id, score) in fused.into_iter().take(limit as usize) {
            out.push(HybridHit {
                app_id,
                score,
                fts_rank: fts_best.get(&app_id).copied(),
                vector_score: vector_best.get(&app_id).copied(),
            });
        }
        Ok(out)
    }
}

fn offer_bounded_vector_score(
    best: &mut HashMap<u32, f64>,
    min_heap: &mut BinaryHeap<Reverse<VectorRank>>,
    capacity: usize,
    app_id: u32,
    score: f64,
) {
    if capacity == 0 {
        return;
    }
    if let Some(previous) = best.get_mut(&app_id) {
        if score.total_cmp(previous).is_gt() {
            *previous = score;
            min_heap.push(Reverse(VectorRank { app_id, score }));
        }
        return;
    }

    discard_stale_vector_ranks(best, min_heap);
    let candidate = VectorRank { app_id, score };
    if best.len() < capacity {
        best.insert(app_id, score);
        min_heap.push(Reverse(candidate));
        return;
    }
    let Some(Reverse(worst)) = min_heap.peek().copied() else {
        return;
    };
    if candidate <= worst {
        return;
    }
    min_heap.pop();
    best.remove(&worst.app_id);
    best.insert(app_id, score);
    min_heap.push(Reverse(candidate));
}

fn discard_stale_vector_ranks(
    best: &HashMap<u32, f64>,
    min_heap: &mut BinaryHeap<Reverse<VectorRank>>,
) {
    while let Some(Reverse(rank)) = min_heap.peek().copied() {
        if best
            .get(&rank.app_id)
            .is_some_and(|score| score.to_bits() == rank.score.to_bits())
        {
            break;
        }
        min_heap.pop();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentEmbedTarget {
    pub document_id: String,
    pub app_id: u32,
    pub title: String,
    pub body: String,
    pub content_hash: String,
}

#[derive(Debug, Clone)]
struct CatalogDocSource {
    app_id: u32,
    canonical_name: String,
    app_type: String,
    release_state: String,
    dominant_mode: String,
    private_session: Option<bool>,
    online_coop: Option<bool>,
    self_hosted_server: Option<bool>,
    recommended_min: Option<u8>,
    recommended_max: Option<u8>,
    platforms_json: String,
    languages_json: String,
    localized_name: String,
    short_description: String,
    catalog_taxonomy_json: String,
}

impl CatalogDocSource {
    fn build_documents(&self) -> Vec<UpsertGameDocument> {
        let mut docs = Vec::new();
        let alias = self.localized_name.trim();
        let platforms = self.platforms_json.trim();
        let languages = self.languages_json.trim();
        let catalog_tags = catalog_taxonomy_from_json(&self.catalog_taxonomy_json);
        let identity_body = format!(
            "type={} release={} platforms={} languages={} catalog_taxonomy={}",
            self.app_type, self.release_state, platforms, languages, catalog_tags
        );
        let identity_tags = format!("{} {} {}", self.app_type, self.release_state, catalog_tags);
        let identity_hash = content_hash(&[
            "identity",
            "und",
            &self.canonical_name,
            &identity_body,
            alias,
            &identity_tags,
            "public",
        ]);
        docs.push(UpsertGameDocument {
            document_id: format!("app:{}:identity", self.app_id),
            app_id: self.app_id,
            doc_type: "identity".into(),
            language: "und".into(),
            title: self.canonical_name.clone(),
            body: identity_body,
            content_hash: identity_hash,
            aliases: alias.to_owned(),
            tags: identity_tags,
            visibility: "public".into(),
        });

        let mp_tags = format!("{} {}", self.dominant_mode, catalog_tags);
        let mp_body = format!(
            "mode={} private_session={} online_coop={} self_host={} party={}..{} catalog_taxonomy={}",
            self.dominant_mode,
            fmt_opt_bool(self.private_session),
            fmt_opt_bool(self.online_coop),
            fmt_opt_bool(self.self_hosted_server),
            self.recommended_min
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".into()),
            self.recommended_max
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".into()),
            catalog_tags,
        );
        let mp_hash = content_hash(&[
            "multiplayer_profile",
            "und",
            &self.canonical_name,
            &mp_body,
            alias,
            &mp_tags,
            "public",
        ]);
        docs.push(UpsertGameDocument {
            document_id: format!("app:{}:multiplayer_profile", self.app_id),
            app_id: self.app_id,
            doc_type: "multiplayer_profile".into(),
            language: "und".into(),
            title: self.canonical_name.clone(),
            body: mp_body,
            content_hash: mp_hash,
            aliases: alias.to_owned(),
            tags: mp_tags,
            visibility: "public".into(),
        });

        let desc = self.short_description.trim();
        if !desc.is_empty() {
            let body: String = desc.chars().take(4_000).collect();
            let store_hash = content_hash(&[
                "store_summary",
                "und",
                &self.canonical_name,
                &body,
                alias,
                &catalog_tags,
                "public",
            ]);
            docs.push(UpsertGameDocument {
                document_id: format!("app:{}:store_summary", self.app_id),
                app_id: self.app_id,
                doc_type: "store_summary".into(),
                language: "und".into(),
                title: self.canonical_name.clone(),
                body,
                content_hash: store_hash,
                aliases: alias.to_owned(),
                tags: catalog_tags.clone(),
                visibility: "public".into(),
            });
        }
        docs
    }
}

fn catalog_taxonomy_from_json(value_json: &str) -> String {
    fn collect(value: &serde_json::Value, tags: &mut Vec<String>) {
        match value {
            serde_json::Value::String(value) => {
                let value = value.trim();
                if !value.is_empty() {
                    tags.push(value.chars().take(160).collect());
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    collect(value, tags);
                }
            }
            serde_json::Value::Object(values) => {
                for value in values.values() {
                    collect(value, tags);
                }
            }
            _ => {}
        }
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(value_json) else {
        return String::new();
    };
    let mut tags = Vec::new();
    collect(&value, &mut tags);
    tags.sort_unstable();
    tags.dedup();
    tags.truncate(64);
    tags.join(" ")
}

fn fmt_opt_bool(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    }
}

fn content_hash(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0xff]);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn fts_match_query(raw: &str) -> String {
    // Keep alphanumeric / CJK tokens; join with OR for recall on natural language.
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in raw.chars() {
        if ch.is_alphanumeric() || ch > '\u{2E80}' {
            current.push(ch);
        } else if !current.is_empty() {
            if current.chars().count() >= 2 {
                tokens.push(current.clone());
            }
            current.clear();
        }
    }
    if current.chars().count() >= 2 {
        tokens.push(current);
    }
    tokens.truncate(12);
    tokens
        .into_iter()
        .map(|t| {
            let escaped = t.replace('"', " ");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn hash_embed_text(text: &str, dimensions: usize) -> Vec<f32> {
    let dims = dimensions.max(1);
    let mut vector = vec![0.0f32; dims];
    for (i, ch) in text.chars().enumerate() {
        let idx = (ch as usize).wrapping_add(i).wrapping_mul(2654435761) % dims;
        vector[idx] += 1.0;
    }
    l2_normalize(&mut vector);
    vector
}

fn encode_f32_le(vector: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn decode_f32_le(blob: &[u8], expected_dimensions: usize) -> Result<Vec<f32>, ()> {
    if blob.len() != expected_dimensions * 4 {
        return Err(());
    }
    let mut out = Vec::with_capacity(expected_dimensions);
    for chunk in blob.chunks_exact(4) {
        out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Ok(out)
}

fn l2_normalize(vector: &mut [f32]) {
    let mut sum = 0.0f64;
    for value in vector.iter() {
        sum += f64::from(*value) * f64::from(*value);
    }
    if sum <= f64::EPSILON {
        return;
    }
    let norm = sum.sqrt() as f32;
    for value in vector.iter_mut() {
        *value /= norm;
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let xf = f64::from(*x);
        let yf = f64::from(*y);
        dot += xf * yf;
        na += xf * xf;
        nb += yf * yf;
    }
    if na <= f64::EPSILON || nb <= f64::EPSILON {
        return 0.0;
    }
    (dot / (na.sqrt() * nb.sqrt())).clamp(-1.0, 1.0)
}

fn reciprocal_rank_fusion(ranked_lists: &[Vec<u32>], k: u32) -> Vec<(u32, f64)> {
    let mut scores: HashMap<u32, f64> = HashMap::new();
    let k = f64::from(k.max(1));
    for list in ranked_lists {
        for (idx, id) in list.iter().enumerate() {
            let rank = (idx + 1) as f64;
            *scores.entry(*id).or_insert(0.0) += 1.0 / (k + rank);
        }
    }
    let mut items: Vec<(u32, f64)> = scores.into_iter().collect();
    items.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    items
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
                conn.query_row("SELECT app_id FROM apps LIMIT 1", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map(|v| v as u32)
                .map_err(StorageError::from)
            })
            .unwrap();
        assert!(
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
            .unwrap()
        );
        assert!(
            !repo
                .upsert_game_document(&UpsertGameDocument {
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
                .unwrap()
        );
        let hits = repo.search_game_fts("cooperative", 10).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].app_id, app_id);

        repo.upsert_game_document(&UpsertGameDocument {
            document_id: format!("doc-{app_id}-internal"),
            app_id,
            doc_type: "curation_notes".into(),
            language: "en".into(),
            title: "Internal".into(),
            body: "classifiedterm".into(),
            content_hash: "internal-hash".into(),
            aliases: String::new(),
            tags: String::new(),
            visibility: "internal".into(),
        })
        .unwrap();
        assert!(
            repo.search_game_fts("classifiedterm", 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn embedding_and_cache_roundtrip() {
        let repo = repo();
        let app_id = repo
            .database()
            .with_conn(|conn| {
                conn.query_row("SELECT app_id FROM apps LIMIT 1", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map(|v| v as u32)
                .map_err(StorageError::from)
            })
            .unwrap();
        let doc_id = format!("doc-{app_id}-store");
        assert!(
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
            .unwrap()
        );
        let blob = 1.0f32.to_le_bytes().to_vec();
        assert!(
            repo.put_embedding(&PutEmbedding {
                document_id: doc_id.clone(),
                provider: "hash-embed".into(),
                model: "hash-embed-v1".into(),
                dimensions: 1,
                vector_blob: blob.clone(),
                is_l2_normalized: true,
                content_hash: "h2".into(),
            })
            .unwrap()
        );
        assert!(
            !repo
                .put_embedding(&PutEmbedding {
                    document_id: doc_id.clone(),
                    provider: "hash-embed".into(),
                    model: "hash-embed-v1".into(),
                    dimensions: 1,
                    vector_blob: blob,
                    is_l2_normalized: true,
                    content_hash: "h2".into(),
                })
                .unwrap()
        );
        assert!(
            repo.put_embedding(&PutEmbedding {
                document_id: doc_id.clone(),
                provider: "hash-embed".into(),
                model: "hash-embed-v1".into(),
                dimensions: 2,
                vector_blob: [1.0f32.to_le_bytes(), 0.0f32.to_le_bytes()].concat(),
                is_l2_normalized: true,
                content_hash: "h2".into(),
            })
            .unwrap()
        );
        let listed = repo
            .list_embeddings_for_provider("hash-embed", "hash-embed-v1", 10)
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].document_id, doc_id);
        assert_eq!(listed[0].dimensions, 2);

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
        assert!(
            repo.get_ai_cache("k1", 10_000_000_000_000)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn catalog_sync_and_hybrid_search() {
        let repo = repo();
        let stats = repo.sync_retrieval_from_catalog(500, 0, true).unwrap();
        assert!(stats.apps_scanned > 0);
        assert_eq!(stats.last_app_id.is_some(), stats.apps_scanned > 0);
        assert!(!stats.has_more);
        assert_eq!(stats.next_after_app_id, 0);
        assert_eq!(stats.apps_covered, stats.catalog_apps);
        assert_eq!(stats.coverage_ratio(), 1.0);
        assert!(stats.documents_written > 0);
        assert!(stats.embeddings_written > 0);
        assert!(repo.document_count().unwrap() > 0);

        // Second pass is mostly unchanged.
        let again = repo.sync_retrieval_from_catalog(500, 0, true).unwrap();
        assert_eq!(again.apps_scanned, stats.apps_scanned);
        assert_eq!(again.documents_written, 0);
        assert!(again.documents_unchanged > 0);

        let hits = repo
            .hybrid_search("cooperative private lobby friends", 10)
            .unwrap();
        // Demo catalog may or may not match English tokens; at least search is stable.
        assert!(hits.len() <= 10);
        // Ensure identity docs are searchable by type tokens used in document body.
        let typed = repo.hybrid_search("game released", 10).unwrap();
        assert!(!typed.is_empty());

        let missing = repo
            .list_documents_missing_embedding(
                HASH_EMBED_PROVIDER,
                HASH_EMBED_MODEL,
                HASH_EMBED_DIMENSIONS,
                10,
            )
            .unwrap();
        // After sync with embeddings, current hashes should already be embedded.
        assert!(missing.is_empty());
        assert!(repo.embedding_count().unwrap() > 0);
    }

    #[test]
    fn unchanged_catalog_sync_does_not_wait_for_the_sqlite_writer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("retrieval-writer.db");
        let db = Database::open(&path).unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        repo.seed_demo_if_empty().unwrap();

        let initial = repo.sync_retrieval_from_catalog(500, 0, true).unwrap();
        assert!(initial.documents_written > 0);

        let blocker = Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let started = std::time::Instant::now();
        let unchanged = repo.sync_retrieval_from_catalog(500, 0, true).unwrap();
        let elapsed = started.elapsed();
        blocker.execute_batch("ROLLBACK").unwrap();

        assert_eq!(unchanged.documents_written, 0);
        assert_eq!(unchanged.embeddings_written, 0);
        assert!(unchanged.documents_unchanged > 0);
        assert!(elapsed < RETRIEVAL_WRITER_HANDOFF);
    }

    #[test]
    fn changed_catalog_sync_leaves_an_interprocess_writer_handoff_window() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("retrieval-handoff.db");
        let db = Database::open(&path).unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        repo.seed_demo_if_empty().unwrap();
        repo.sync_retrieval_from_catalog(500, 0, true).unwrap();

        repo.db
            .with_conn_mut(|conn| {
                conn.execute(
                    "UPDATE apps SET canonical_name = canonical_name || ' updated'
                     WHERE app_id = (SELECT MIN(app_id) FROM apps)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();

        let started = std::time::Instant::now();
        let changed = repo.sync_retrieval_from_catalog(500, 0, true).unwrap();

        assert!(changed.documents_written > 0);
        assert!(started.elapsed() >= RETRIEVAL_WRITER_HANDOFF);
    }

    #[test]
    fn hybrid_search_returns_more_than_the_legacy_one_hundred_result_cap() {
        let repo = repo();
        let first_app_id = 4_100_000_000_u32;
        let count = 125_u32;
        repo.database()
            .with_conn_mut(|conn| {
                let tx = conn.transaction()?;
                for offset in 0..count {
                    let app_id = first_app_id + offset;
                    tx.execute(
                        "INSERT INTO apps (
                             app_id, app_type, canonical_name, release_state,
                             created_at_ms, updated_at_ms
                         ) VALUES (?1, 'game', ?2, 'released', 1, 1)",
                        params![app_id, format!("Recall game {offset}")],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .unwrap();

        for offset in 0..count {
            let app_id = first_app_id + offset;
            let document_id = format!("wide-recall-{app_id}");
            let content_hash = format!("wide-recall-hash-{app_id}");
            repo.upsert_game_document(&UpsertGameDocument {
                document_id: document_id.clone(),
                app_id,
                doc_type: "identity".into(),
                language: "en".into(),
                title: format!("Recall game {offset}"),
                body: "massrecalltoken cooperative friends".into(),
                content_hash: content_hash.clone(),
                aliases: String::new(),
                tags: "cooperative".into(),
                visibility: "public".into(),
            })
            .unwrap();
            repo.put_embedding(&PutEmbedding {
                document_id,
                provider: "wide-provider".into(),
                model: "wide-model".into(),
                dimensions: 2,
                vector_blob: encode_f32_le(&[1.0, 0.0]),
                is_l2_normalized: true,
                content_hash,
            })
            .unwrap();
        }

        let hits = repo
            .hybrid_search_with_vector(
                "massrecalltoken",
                &[1.0, 0.0],
                "wide-provider",
                "wide-model",
                300,
            )
            .unwrap();
        let distinct: HashSet<u32> = hits.iter().map(|hit| hit.app_id).collect();
        assert_eq!(hits.len(), count as usize);
        assert_eq!(distinct.len(), count as usize);
    }

    #[test]
    fn catalog_sync_paginates_through_every_app_and_wraps() {
        let repo = repo();
        let catalog_apps = repo
            .database()
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM apps", [], |row| row.get::<_, i64>(0))
                    .map_err(StorageError::from)
            })
            .unwrap() as u32;
        assert!(catalog_apps > 2);

        let mut after_app_id = 0;
        let mut previous_coverage = 0.0;
        let mut batches = 0;
        loop {
            let stats = repo
                .sync_retrieval_from_catalog(2, after_app_id, false)
                .unwrap();
            batches += 1;
            assert!(stats.apps_scanned <= 2);
            assert_eq!(stats.catalog_apps, catalog_apps);
            assert!(stats.coverage_ratio() >= previous_coverage);

            if !stats.has_more {
                assert_eq!(stats.next_after_app_id, 0);
                assert_eq!(stats.apps_covered, catalog_apps);
                assert_eq!(stats.coverage_ratio(), 1.0);
                break;
            }

            let last_app_id = stats.last_app_id.expect("non-final page has an app");
            assert_eq!(stats.next_after_app_id, last_app_id);
            assert!(last_app_id > after_app_id);
            after_app_id = stats.next_after_app_id;
            previous_coverage = stats.coverage_ratio();
            assert!(batches <= catalog_apps, "cursor must always make progress");
        }

        assert_eq!(batches, catalog_apps.div_ceil(2));
        let indexed_apps = repo
            .database()
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(DISTINCT app_id) FROM game_documents",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(StorageError::from)
            })
            .unwrap() as u32;
        assert_eq!(indexed_apps, catalog_apps);

        let wrapped = repo.sync_retrieval_from_catalog(2, 0, false).unwrap();
        assert_eq!(wrapped.apps_scanned, 2);
        assert!(wrapped.has_more);
    }

    #[test]
    fn catalog_taxonomy_evidence_is_searchable_and_invalid_json_is_harmless() {
        let repo = repo();
        let app_ids = repo
            .database()
            .with_conn(|conn| {
                let mut statement =
                    conn.prepare("SELECT app_id FROM apps ORDER BY app_id LIMIT 2")?;
                let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(StorageError::from)
            })
            .unwrap();
        let tagged_app_id = app_ids[0] as u32;
        let invalid_app_id = app_ids[1] as u32;
        repo.database()
            .with_conn_mut(|conn| {
                conn.execute(
                    "INSERT INTO feature_evidence (
                         app_id, feature_name, value_json, source_type, source_ref,
                         confidence, observed_at_ms, is_active
                     ) VALUES
                         (?1, 'catalog_taxonomy', ?2, 'test', 'test', 1, 10, 1),
                         (?3, 'catalog_tags', 'not-json', 'test', 'test', 1, 10, 1)",
                    params![
                        tagged_app_id as i64,
                        r#"{"categories":["Co-op"],"genres":["Action Roguelike"],"developers":["Test Studio"],"publishers":["Test Publisher"]}"#,
                        invalid_app_id as i64
                    ],
                )?;
                Ok(())
            })
            .unwrap();

        repo.sync_retrieval_from_catalog(500, 0, false).unwrap();
        let hits = repo.hybrid_search("roguelike", 10).unwrap();
        assert!(hits.iter().any(|hit| hit.app_id == tagged_app_id));
    }

    #[test]
    fn hybrid_search_scans_embeddings_older_than_the_first_ten_thousand() {
        let repo = repo();
        let target_app_id = 4_200_000_000_u32;
        let filler_app_id = 4_200_000_001_u32;
        let target_blob = encode_f32_le(&[1.0, 0.0]);
        let filler_blob = encode_f32_le(&[-1.0, 0.0]);
        repo.database()
            .with_conn_mut(|conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO apps (
                         app_id, app_type, canonical_name, release_state,
                         created_at_ms, updated_at_ms
                     ) VALUES
                         (?1, 'game', 'Old Vector Target', 'released', 1, 1),
                         (?2, 'game', 'Recent Vector Fillers', 'released', 1, 1)",
                    params![target_app_id, filler_app_id],
                )?;
                tx.execute(
                    "INSERT INTO game_documents (
                         document_id, app_id, doc_type, language, title, body,
                         content_hash, visibility, updated_at_ms
                     ) VALUES (
                         'bulk-target', ?1, 'identity', 'en', 'target', '',
                         'bulk-target-hash', 'public', 1
                     )",
                    params![target_app_id],
                )?;
                tx.execute(
                    "INSERT INTO game_embeddings (
                         document_id, provider, model, dimensions, vector_blob,
                         is_l2_normalized, content_hash, created_at_ms
                     ) VALUES (
                         'bulk-target', 'bulk-provider', 'bulk-model', 2, ?1,
                         1, 'bulk-target-hash', 1
                     )",
                    params![target_blob],
                )?;
                for index in 0..10_000_i64 {
                    let document_id = format!("bulk-filler-{index:05}");
                    tx.execute(
                        "INSERT INTO game_documents (
                             document_id, app_id, doc_type, language, title, body,
                             content_hash, visibility, updated_at_ms
                         ) VALUES (
                             ?1, ?2, 'identity', 'en', 'filler', '',
                             'bulk-filler-hash', 'public', ?3
                         )",
                        params![document_id, filler_app_id, index + 2],
                    )?;
                    tx.execute(
                        "INSERT INTO game_embeddings (
                             document_id, provider, model, dimensions, vector_blob,
                             is_l2_normalized, content_hash, created_at_ms
                         ) VALUES (
                             ?1, 'bulk-provider', 'bulk-model', 2, ?2,
                             1, 'bulk-filler-hash', ?3
                         )",
                        params![document_id, filler_blob, index + 2],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .unwrap();

        let hits = repo
            .hybrid_search_with_vector("", &[1.0, 0.0], "bulk-provider", "bulk-model", 1)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].app_id, target_app_id);
        assert_eq!(hits[0].vector_score, Some(1.0));
    }

    #[test]
    fn stale_embeddings_and_removed_managed_documents_are_pruned() {
        let repo = repo();
        let app_id = repo
            .database()
            .with_conn(|conn| {
                conn.query_row("SELECT app_id FROM apps LIMIT 1", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map(|value| value as u32)
                .map_err(StorageError::from)
            })
            .unwrap();
        let document_id = format!("app:{app_id}:store_summary");
        let base = UpsertGameDocument {
            document_id: document_id.clone(),
            app_id,
            doc_type: "store_summary".into(),
            language: "und".into(),
            title: "Old title".into(),
            body: "retired description".into(),
            content_hash: "old-hash".into(),
            aliases: String::new(),
            tags: String::new(),
            visibility: "public".into(),
        };
        repo.upsert_game_document(&base).unwrap();
        repo.put_embedding(&PutEmbedding {
            document_id: document_id.clone(),
            provider: HASH_EMBED_PROVIDER.into(),
            model: HASH_EMBED_MODEL.into(),
            dimensions: 1,
            vector_blob: 1.0f32.to_le_bytes().to_vec(),
            is_l2_normalized: true,
            content_hash: "old-hash".into(),
        })
        .unwrap();

        let mut changed = base;
        changed.body = "replacement description".into();
        changed.content_hash = "new-hash".into();
        repo.upsert_game_document(&changed).unwrap();
        assert!(
            repo.list_embeddings_for_provider(HASH_EMBED_PROVIDER, HASH_EMBED_MODEL, 10)
                .unwrap()
                .is_empty()
        );

        repo.database()
            .with_conn_mut(|conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                prune_managed_documents_on_conn(&tx, app_id, &HashSet::new())?;
                tx.commit()?;
                Ok(())
            })
            .unwrap();
        assert!(repo.search_game_fts("replacement", 10).unwrap().is_empty());
        assert_eq!(
            repo.database()
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT COUNT(*) FROM game_documents WHERE document_id = ?1",
                        params![document_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(StorageError::from)
                })
                .unwrap(),
            0
        );
    }

    #[test]
    fn local_hash_embedding_uses_the_shared_v2_mapping() {
        let vector = hash_embed_text("a", 64);
        assert_eq!(vector[17], 1.0);
        assert_eq!(vector.iter().filter(|value| **value != 0.0).count(), 1);
    }
}
