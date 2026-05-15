//! LanceDB-backed [`ConversationMemory`] + local `fastembed` [`Embedder`].
//!
//! Entirely behind `#[cfg(feature = "memory")]`. The LanceDB calls mirror the
//! verified working store in `crates/knowledge-ml/src/store.rs` (lancedb
//! 0.27): connect → `table_names` → `create_table(RecordBatch::new_empty)` →
//! `FixedSizeListArray::try_new` → `vector_search(..)?…execute()` →
//! `_distance` column. Rows are namespaced by a `session_id` column and the
//! search applies a `session_id = '…'` `only_if` predicate (single quotes
//! escaped, since `RunOptions::session_id` is caller-supplied).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arrow_array::{
    Array, ArrayRef, FixedSizeListArray, Float32Array, RecordBatch, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::Connection;

use super::{ConversationMemory, Embedder, MemoryError, MemoryHit, MemoryRecord};

const TABLE: &str = "turns";

/// Output dimensionality of `BAAI/bge-small-en-v1.5` (the `fastembed`
/// default English model used by [`FastEmbedEmbedder`]).
pub const MEMORY_DIM: usize = 384;

fn store_err(e: impl std::fmt::Display) -> MemoryError {
    MemoryError::Store(e.to_string())
}

fn schema(dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("session_id", DataType::Utf8, false),
        Field::new("role", DataType::Utf8, false),
        Field::new("tool_name", DataType::Utf8, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("created_at", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dim as i32,
            ),
            true,
        ),
    ]))
}

/// Escape a session id for safe interpolation into a LanceDB SQL predicate.
fn sql_quote(session_id: &str) -> String {
    format!("'{}'", session_id.replace('\'', "''"))
}

/// Local ONNX embedder via `fastembed` (bge-small-en-v1.5, 384-dim).
///
/// The first construction downloads the model into `fastembed`'s cache.
/// `TextEmbedding::embed` takes `&mut self`, so the model lives behind a
/// `Mutex` and inference runs on a blocking thread pool.
pub struct FastEmbedEmbedder {
    model: Arc<Mutex<TextEmbedding>>,
}

impl FastEmbedEmbedder {
    /// Construct the default English embedder. Downloads the model on first
    /// use; not called from tests.
    pub fn new() -> Result<Self, MemoryError> {
        let model = TextEmbedding::try_new(InitOptions::new(EmbeddingModel::BGESmallENV15))
            .map_err(|e| MemoryError::Embed(e.to_string()))?;
        Ok(Self {
            model: Arc::new(Mutex::new(model)),
        })
    }
}

#[async_trait]
impl Embedder for FastEmbedEmbedder {
    fn dim(&self) -> usize {
        MEMORY_DIM
    }

    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, MemoryError> {
        let model = Arc::clone(&self.model);
        tokio::task::spawn_blocking(move || {
            let mut m = model
                .lock()
                .map_err(|_| MemoryError::Embed("fastembed mutex poisoned".into()))?;
            m.embed(texts, None)
                .map_err(|e| MemoryError::Embed(e.to_string()))
        })
        .await
        .map_err(|e| MemoryError::Embed(format!("embed task join error: {e}")))?
    }
}

/// LanceDB-backed conversation memory.
pub struct LanceMemory {
    conn: Connection,
    embedder: Arc<dyn Embedder>,
    dim: usize,
}

impl LanceMemory {
    /// Open (or create) a store at `dir` with an explicit embedder. The
    /// table schema is built from `embedder.dim()`; a store must always be
    /// reopened with an embedder of the same dimensionality. Used directly by
    /// tests with a deterministic fake.
    pub async fn new_with_embedder(
        dir: &Path,
        embedder: Arc<dyn Embedder>,
    ) -> Result<Self, MemoryError> {
        let dim = embedder.dim();
        std::fs::create_dir_all(dir).map_err(store_err)?;
        let path = dir
            .to_str()
            .ok_or_else(|| MemoryError::Store("non-UTF-8 memory path".into()))?;
        let conn = lancedb::connect(path)
            .execute()
            .await
            .map_err(store_err)?;

        let tables = conn.table_names().execute().await.map_err(store_err)?;
        if !tables.contains(&TABLE.to_string()) {
            let empty = RecordBatch::new_empty(schema(dim));
            conn.create_table(TABLE, empty)
                .execute()
                .await
                .map_err(store_err)?;
        }

        Ok(Self {
            conn,
            embedder,
            dim,
        })
    }

    /// Production constructor: local `fastembed` at `dir`, or the default
    /// cache path `${cache_dir}/deepseek-loop/memory` when `dir` is `None`.
    pub async fn open(dir: Option<PathBuf>) -> Result<Self, MemoryError> {
        let dir = dir
            .or_else(default_dir)
            .ok_or_else(|| MemoryError::Store("no cache dir on this platform".into()))?;
        let embedder = Arc::new(FastEmbedEmbedder::new()?);
        Self::new_with_embedder(&dir, embedder).await
    }
}

/// `${cache_dir}/deepseek-loop/memory` — one DB dir; `session_id` is a row
/// filter column, mirroring the scheduler's `${cache_dir}/deepseek-loop`
/// convention.
fn default_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("deepseek-loop").join("memory"))
}

#[async_trait]
impl ConversationMemory for LanceMemory {
    async fn archive(
        &self,
        session_id: &str,
        records: Vec<MemoryRecord>,
    ) -> Result<(), MemoryError> {
        if records.is_empty() {
            return Ok(());
        }
        let n = records.len();
        let vectors = self
            .embedder
            .embed(records.iter().map(|r| r.text.clone()).collect())
            .await?;
        if vectors.len() != n {
            return Err(MemoryError::Embed(format!(
                "embedder returned {} vectors for {n} rows",
                vectors.len()
            )));
        }

        let ids: Vec<String> = (0..n).map(|_| uuid::Uuid::new_v4().to_string()).collect();
        let sessions: Vec<&str> = vec![session_id; n];
        let roles: Vec<&str> = records.iter().map(|r| r.role.as_str()).collect();
        let tools: Vec<&str> = records.iter().map(|r| r.tool_name.as_str()).collect();
        let texts: Vec<&str> = records.iter().map(|r| r.text.as_str()).collect();
        let created: Vec<&str> = records.iter().map(|r| r.created_at.as_str()).collect();

        let mut flat: Vec<f32> = Vec::with_capacity(n * self.dim);
        for v in &vectors {
            if v.len() != self.dim {
                return Err(MemoryError::Embed(format!(
                    "embedding dim {} != store dim {}",
                    v.len(),
                    self.dim
                )));
            }
            flat.extend_from_slice(v);
        }
        let item = Arc::new(Field::new("item", DataType::Float32, true));
        let vecs = FixedSizeListArray::try_new(
            item,
            self.dim as i32,
            Arc::new(Float32Array::from(flat)),
            None,
        )
        .map_err(store_err)?;

        let batch = RecordBatch::try_new(
            schema(self.dim),
            vec![
                Arc::new(StringArray::from(ids)) as ArrayRef,
                Arc::new(StringArray::from(sessions)) as ArrayRef,
                Arc::new(StringArray::from(roles)) as ArrayRef,
                Arc::new(StringArray::from(tools)) as ArrayRef,
                Arc::new(StringArray::from(texts)) as ArrayRef,
                Arc::new(StringArray::from(created)) as ArrayRef,
                Arc::new(vecs) as ArrayRef,
            ],
        )
        .map_err(store_err)?;

        let table = self
            .conn
            .open_table(TABLE)
            .execute()
            .await
            .map_err(store_err)?;
        table
            .add(vec![batch])
            .execute()
            .await
            .map_err(store_err)?;
        Ok(())
    }

    async fn search(
        &self,
        session_id: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<MemoryHit>, MemoryError> {
        let qv = self
            .embedder
            .embed(vec![query.to_string()])
            .await?
            .pop()
            .ok_or_else(|| MemoryError::Embed("empty query embedding".into()))?;

        let table = self
            .conn
            .open_table(TABLE)
            .execute()
            .await
            .map_err(store_err)?;

        let filter = format!("session_id = {}", sql_quote(session_id));
        let stream = table
            .vector_search(qv)
            .map_err(store_err)?
            .only_if(&filter)
            .limit(k)
            .execute()
            .await
            .map_err(store_err)?;
        let batches: Vec<RecordBatch> = stream.try_collect().await.map_err(store_err)?;

        let mut hits = Vec::new();
        for batch in &batches {
            let col = |name: &str| -> Vec<String> {
                batch
                    .column_by_name(name)
                    .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                    .map(|a| (0..a.len()).map(|i| a.value(i).to_string()).collect())
                    .unwrap_or_default()
            };
            let roles = col("role");
            let tools = col("tool_name");
            let texts = col("text");
            let created = col("created_at");
            let dists: Vec<f32> = batch
                .column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>())
                .map(|a| (0..a.len()).map(|i| a.value(i)).collect())
                .unwrap_or_else(|| vec![1.0; batch.num_rows()]);

            for i in 0..batch.num_rows() {
                hits.push(MemoryHit {
                    role: roles.get(i).cloned().unwrap_or_default(),
                    tool_name: tools.get(i).cloned().unwrap_or_default(),
                    text: texts.get(i).cloned().unwrap_or_default(),
                    created_at: created.get(i).cloned().unwrap_or_default(),
                    score: 1.0 / (1.0 + dists.get(i).copied().unwrap_or(1.0)),
                });
            }
        }
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::memory::FakeEmbedder;

    fn rec(role: &str, text: &str) -> MemoryRecord {
        MemoryRecord {
            role: role.into(),
            tool_name: String::new(),
            text: text.into(),
            created_at: "2026-05-15T00:00:00Z".into(),
        }
    }

    #[tokio::test]
    async fn roundtrip_archive_then_search() {
        let dir = tempfile::tempdir().unwrap();
        let mem = LanceMemory::new_with_embedder(dir.path(), Arc::new(FakeEmbedder::new()))
            .await
            .unwrap();

        mem.archive(
            "sess-A",
            vec![
                rec("tool", "Grep auth.rs found token validation at line 42"),
                rec("assistant", "I will refactor the parser module next"),
            ],
        )
        .await
        .unwrap();

        let hits = mem
            .search("sess-A", "Grep auth.rs found token validation at line 42", 2)
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].text,
            "Grep auth.rs found token validation at line 42"
        );
        assert!(hits[0].score >= hits[1].score);
    }

    #[tokio::test]
    async fn search_is_session_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let mem = LanceMemory::new_with_embedder(dir.path(), Arc::new(FakeEmbedder::new()))
            .await
            .unwrap();
        mem.archive("sess-A", vec![rec("tool", "only in A")])
            .await
            .unwrap();

        assert!(mem.search("sess-B", "only in A", 5).await.unwrap().is_empty());
        assert_eq!(mem.search("sess-A", "only in A", 5).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn quote_in_session_id_is_escaped() {
        let dir = tempfile::tempdir().unwrap();
        let mem = LanceMemory::new_with_embedder(dir.path(), Arc::new(FakeEmbedder::new()))
            .await
            .unwrap();
        let sneaky = "s' OR '1'='1";
        mem.archive(sneaky, vec![rec("tool", "guarded row")])
            .await
            .unwrap();
        // The injection attempt must not leak rows from other sessions, and
        // the legitimately-escaped id must still match its own row.
        assert_eq!(mem.search(sneaky, "guarded row", 5).await.unwrap().len(), 1);
        assert!(mem
            .search("s", "guarded row", 5)
            .await
            .unwrap()
            .is_empty());
    }
}
