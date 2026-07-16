//! SQLite storage, migrations, ingest, jobs, curation, and backup for MPGS.
//!
//! Network I/O never holds a write transaction. Source proposals from
//! `mpgs-steam-source` are normalized before repository writes.

#![forbid(unsafe_code)]

pub mod backup;
pub mod catalog;
pub mod clock;
pub mod curation;
pub mod db;
pub mod error;
pub mod feedback;
pub mod ingest;
pub mod jobs;
pub mod migrate;
pub mod models;
pub mod offline_features;
pub mod play_intent;
pub mod quality;
pub mod query;
pub mod repo;
pub mod retrieval;
pub mod seed;
pub mod source_state;
pub mod users;
pub mod util;

pub use clock::{Clock, FakeClock, SystemClock};
pub use db::Database;
pub use error::{StorageError, StorageResult};
pub use migrate::{MIGRATIONS, latest_version};
pub use models::*;
pub use quality::QualityFinding;
pub use repo::Repository;
pub use offline_features::{
    OFFLINE_FEATURE_MODEL, OFFLINE_FEATURE_PROMPT_VERSION, OFFLINE_FEATURE_PROVIDER,
    OFFLINE_FEATURE_TASK, OfflineFeatureStats,
};
pub use retrieval::{
    AiCacheEntry, FtsHit, GameDocument, HASH_EMBED_DIMENSIONS, HASH_EMBED_MODEL,
    HASH_EMBED_PROVIDER, HybridHit, PutEmbedding, RetrievalSyncStats, StoredEmbedding,
    UpsertGameDocument,
};

#[cfg(test)]
mod tests;
