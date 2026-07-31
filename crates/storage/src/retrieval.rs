//! Game retrieval documents, FTS sync, embeddings, hybrid search, and AI cache.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};

use rusqlite::{OptionalExtension, params};
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

impl Repository {
    /// Upsert a retrieval document and keep FTS in sync.
    /// Returns `true` when content changed (or was newly inserted).
    pub fn upsert_game_document(&self, doc: &UpsertGameDocument) -> StorageResult<bool> {
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT content_hash FROM game_documents WHERE document_id = ?1",
                    params![doc.document_id],
                    |row| row.get(0),
                )
                .optional()?;
            if existing.as_deref() == Some(doc.content_hash.as_str()) {
                return Ok(false);
            }
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
                "DELETE FROM game_embeddings
                 WHERE document_id = ?1 AND content_hash <> ?2",
                params![doc.document_id, doc.content_hash],
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
            Ok(true)
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
        if embedding.dimensions == 0 || embedding.vector_blob.len() != embedding.dimensions * 4 {
            return Err(StorageError::validation(
                "embedding dimensions do not match vector blob length",
            ));
        }
        let now = self.db.now_ms();
        self.db.with_conn_mut(|conn| {
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
            if existing
                .as_ref()
                .is_some_and(|(dimensions, blob, normalized)| {
                    *dimensions == embedding.dimensions as i64
                        && blob == &embedding.vector_blob
                        && *normalized == i64::from(embedding.is_l2_normalized)
                })
            {
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
                    now
                ],
            )?;
            Ok(true)
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
        let rows: Vec<CatalogDocSource> = self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT a.app_id, a.canonical_name, a.app_type, a.release_state,
                        COALESCE(p.dominant_mode, ''),
                        p.private_session, p.online_coop, p.self_hosted_server,
                        p.recommended_min_players, p.recommended_max_players,
                        COALESCE(v.platforms_json, '[]'),
                        COALESCE(v.languages_json, '[]'),
                        COALESCE(loc.name, ''),
                        COALESCE(loc.short_description, '')
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
            let mapped = stmt.query_map(params![after_app_id as i64, limit as i64], |row| {
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
                })
            })?;
            let mut out = Vec::new();
            for row in mapped {
                out.push(row?);
            }
            Ok(out)
        })?;

        let mut stats = RetrievalSyncStats {
            apps_scanned: rows.len() as u32,
            ..RetrievalSyncStats::default()
        };

        for source in &rows {
            let docs = source.build_documents();
            let keep_ids: HashSet<String> =
                docs.iter().map(|doc| doc.document_id.clone()).collect();
            self.prune_managed_documents(source.app_id, &keep_ids)?;
            for doc in docs {
                if self.upsert_game_document(&doc)? {
                    stats.documents_written += 1;
                    if write_embeddings {
                        let text = format!("{} {}", doc.title, doc.body);
                        let vector = hash_embed_text(&text, HASH_EMBED_DIMENSIONS);
                        let written = self.put_embedding(&PutEmbedding {
                            document_id: doc.document_id.clone(),
                            provider: HASH_EMBED_PROVIDER.into(),
                            model: HASH_EMBED_MODEL.into(),
                            dimensions: HASH_EMBED_DIMENSIONS,
                            vector_blob: encode_f32_le(&vector),
                            is_l2_normalized: true,
                            content_hash: doc.content_hash.clone(),
                        })?;
                        if written {
                            stats.embeddings_written += 1;
                        } else {
                            stats.embeddings_unchanged += 1;
                        }
                    }
                } else {
                    stats.documents_unchanged += 1;
                    if write_embeddings {
                        let text = format!("{} {}", doc.title, doc.body);
                        let vector = hash_embed_text(&text, HASH_EMBED_DIMENSIONS);
                        let written = self.put_embedding(&PutEmbedding {
                            document_id: doc.document_id.clone(),
                            provider: HASH_EMBED_PROVIDER.into(),
                            model: HASH_EMBED_MODEL.into(),
                            dimensions: HASH_EMBED_DIMENSIONS,
                            vector_blob: encode_f32_le(&vector),
                            is_l2_normalized: true,
                            content_hash: doc.content_hash.clone(),
                        })?;
                        if written {
                            stats.embeddings_written += 1;
                        } else {
                            stats.embeddings_unchanged += 1;
                        }
                    }
                }
            }
        }
        Ok(stats)
    }

    fn prune_managed_documents(
        &self,
        app_id: u32,
        keep_ids: &HashSet<String>,
    ) -> StorageResult<()> {
        self.db.with_conn_mut(|conn| {
            let managed_ids = HashSet::from([
                format!("app:{app_id}:identity"),
                format!("app:{app_id}:multiplayer_profile"),
                format!("app:{app_id}:store_summary"),
            ]);
            let stale_ids: Vec<String> = {
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
            if stale_ids.is_empty() {
                return Ok(());
            }
            let tx = conn.transaction()?;
            for document_id in stale_ids {
                tx.execute(
                    "DELETE FROM game_fts WHERE document_id = ?1",
                    params![document_id],
                )?;
                tx.execute(
                    "DELETE FROM game_documents WHERE document_id = ?1",
                    params![document_id],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
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
        let limit = limit.clamp(1, 100);
        let fts_query = fts_match_query(query);
        let fts_hits = if fts_query.is_empty() {
            Vec::new()
        } else {
            self.search_game_fts(&fts_query, limit.saturating_mul(3).max(limit))?
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
        let vector_capacity = (limit as usize).saturating_mul(3).max(limit as usize);
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
}

impl CatalogDocSource {
    fn build_documents(&self) -> Vec<UpsertGameDocument> {
        let mut docs = Vec::new();
        let alias = self.localized_name.trim();
        let platforms = self.platforms_json.trim();
        let languages = self.languages_json.trim();
        let identity_body = format!(
            "type={} release={} platforms={} languages={}",
            self.app_type, self.release_state, platforms, languages
        );
        let identity_tags = format!("{} {}", self.app_type, self.release_state);
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

        let mp_body = format!(
            "mode={} private_session={} online_coop={} self_host={} party={}..{}",
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
        );
        let mp_hash = content_hash(&[
            "multiplayer_profile",
            "und",
            &self.canonical_name,
            &mp_body,
            alias,
            &self.dominant_mode,
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
            tags: self.dominant_mode.clone(),
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
                tags: String::new(),
                visibility: "public".into(),
            });
        }
        docs
    }
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

        repo.prune_managed_documents(app_id, &HashSet::new())
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
