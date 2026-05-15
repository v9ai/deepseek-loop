//! Conversation memory: archive turns the history-compactor would otherwise
//! discard, and recall them semantically on demand.
//!
//! The core loop only ever touches the [`ConversationMemory`] trait, so the
//! heavy LanceDB / `fastembed` machinery stays behind the `memory` cargo
//! feature ([`lance`], `#[cfg(feature = "memory")]`). The traits, record
//! types, and error here are unconditional and dependency-free (only
//! `async-trait` + `std`), so default and `wasm` builds compile them at zero
//! cost and they remain unit-testable with the in-memory fakes below.

use async_trait::async_trait;
use thiserror::Error;

#[cfg(feature = "memory")]
pub mod lance;

/// One archived conversation message.
///
/// Produced from a single [`crate::types::ChatMessage`] in the slice the
/// compactor is about to drop. `text` is already byte-bounded by the caller.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryRecord {
    /// `"assistant"`, `"tool"`, `"user"`, or `"system"`.
    pub role: String,
    /// Tool name when the row is an assistant tool-call or a tool result;
    /// empty string otherwise.
    pub tool_name: String,
    /// Bounded message content (see `truncate_for_transcript` at the call
    /// site).
    pub text: String,
    /// When the row was archived — UNIX epoch seconds as a string (the loop
    /// avoids a `chrono` dependency). Free-form; treated as opaque metadata.
    pub created_at: String,
}

/// A search hit: an archived record plus a relevance score in `[0, 1]`
/// (higher is closer).
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryHit {
    pub role: String,
    pub tool_name: String,
    pub text: String,
    pub created_at: String,
    pub score: f32,
}

/// Errors from the memory subsystem. Deliberately *not* a variant of
/// [`crate::error::DeepSeekError`] — archival is best-effort and must never
/// pull LanceDB/Arrow types into the crate-wide error enum.
#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("embedding error: {0}")]
    Embed(String),

    #[error("vector store error: {0}")]
    Store(String),

    #[error("{0}")]
    Other(String),
}

/// Produces embedding vectors for text. Implemented by `FastEmbedEmbedder`
/// (local ONNX, behind the `memory` feature) and by the deterministic
/// `FakeEmbedder` used in tests.
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Embedding dimensionality. The vector store schema is built from this,
    /// so a given store must always be paired with an embedder of the same
    /// `dim()`.
    fn dim(&self) -> usize;

    /// Embed a batch of texts, preserving order. Returns one `dim()`-length
    /// vector per input.
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, MemoryError>;
}

/// A persistent, session-scoped semantic store for compacted-out turns.
///
/// Implementations namespace rows by `session_id`. Callers MUST use the same
/// `session_id` for [`archive`](Self::archive) and
/// [`search`](Self::search) — see the invariant documented on
/// [`crate::agent::RunOptions::memory`].
#[async_trait]
pub trait ConversationMemory: Send + Sync {
    /// Persist `records` under `session_id`. Best-effort from the loop's
    /// perspective: a returned `Err` is logged and the compaction proceeds
    /// anyway.
    async fn archive(
        &self,
        session_id: &str,
        records: Vec<MemoryRecord>,
    ) -> Result<(), MemoryError>;

    /// Return up to `k` archived rows for `session_id` most relevant to
    /// `query`, best first.
    async fn search(
        &self,
        session_id: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<MemoryHit>, MemoryError>;
}

// ---------------------------------------------------------------------------
// Test doubles — compiled only for the crate's test target (`cfg(test)` is
// true for `cargo test` *and* `cargo test --features memory`, so the
// feature-gated LanceMemory roundtrip test can reuse `FakeEmbedder`). Never
// compiled by a plain `cargo build`, so no dead-code warnings.
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) use fakes::InMemoryMemory;
// `FakeEmbedder` is only referenced externally by the feature-gated
// `LanceMemory` roundtrip test; scope the re-export so a plain `cargo test`
// doesn't flag it as unused.
#[cfg(all(test, feature = "memory"))]
pub(crate) use fakes::FakeEmbedder;

#[cfg(test)]
mod fakes {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Deterministic, dependency-free embedder. Identical input text always
    /// yields an identical unit vector, so an exact-match query scores 1.0
    /// against its archived row.
    pub(crate) struct FakeEmbedder {
        dim: usize,
    }

    impl FakeEmbedder {
        pub(crate) fn new() -> Self {
            Self { dim: 16 }
        }

        fn embed_one(&self, text: &str) -> Vec<f32> {
            let mut v = vec![0f32; self.dim];
            for (j, b) in text.bytes().enumerate() {
                v[(j + b as usize) % self.dim] += (b as f32) + 1.0;
            }
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for x in &mut v {
                    *x /= norm;
                }
            }
            v
        }
    }

    #[async_trait]
    impl Embedder for FakeEmbedder {
        fn dim(&self) -> usize {
            self.dim
        }

        async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, MemoryError> {
            Ok(texts.iter().map(|t| self.embed_one(t)).collect())
        }
    }

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if na == 0.0 || nb == 0.0 {
            0.0
        } else {
            dot / (na * nb)
        }
    }

    /// Brute-force, session-filtered in-memory store. Faithful enough to
    /// exercise the loop integration and the `MemorySearch` tool without
    /// LanceDB.
    pub(crate) struct InMemoryMemory {
        embedder: Arc<dyn Embedder>,
        rows: Mutex<Vec<(String, MemoryRecord, Vec<f32>)>>,
        fail_archive: bool,
    }

    impl InMemoryMemory {
        pub(crate) fn new() -> Self {
            Self {
                embedder: Arc::new(FakeEmbedder::new()),
                rows: Mutex::new(Vec::new()),
                fail_archive: false,
            }
        }

        /// An instance whose `archive` always errors — drives the loop's
        /// non-fatal path.
        pub(crate) fn failing() -> Self {
            Self {
                fail_archive: true,
                ..Self::new()
            }
        }

        pub(crate) fn archived_for(&self, session_id: &str) -> Vec<MemoryRecord> {
            self.rows
                .lock()
                .unwrap()
                .iter()
                .filter(|(s, _, _)| s == session_id)
                .map(|(_, r, _)| r.clone())
                .collect()
        }
    }

    #[async_trait]
    impl ConversationMemory for InMemoryMemory {
        async fn archive(
            &self,
            session_id: &str,
            records: Vec<MemoryRecord>,
        ) -> Result<(), MemoryError> {
            if self.fail_archive {
                return Err(MemoryError::Store("injected archive failure".into()));
            }
            let texts: Vec<String> = records.iter().map(|r| r.text.clone()).collect();
            let vecs = self.embedder.embed(texts).await?;
            let mut rows = self.rows.lock().unwrap();
            for (r, v) in records.into_iter().zip(vecs) {
                rows.push((session_id.to_string(), r, v));
            }
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
            let rows = self.rows.lock().unwrap();
            let mut scored: Vec<(f32, MemoryRecord)> = rows
                .iter()
                .filter(|(s, _, _)| s == session_id)
                .map(|(_, r, v)| (cosine(&qv, v), r.clone()))
                .collect();
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(k);
            Ok(scored
                .into_iter()
                .map(|(score, r)| MemoryHit {
                    role: r.role,
                    tool_name: r.tool_name,
                    text: r.text,
                    created_at: r.created_at,
                    score,
                })
                .collect())
        }
    }

    #[tokio::test]
    async fn exact_query_scores_highest() {
        let mem = InMemoryMemory::new();
        mem.archive(
            "s1",
            vec![
                MemoryRecord {
                    role: "tool".into(),
                    tool_name: "Grep".into(),
                    text: "auth.rs line 42 token check".into(),
                    created_at: "t".into(),
                },
                MemoryRecord {
                    role: "assistant".into(),
                    tool_name: String::new(),
                    text: "unrelated chatter about the weather".into(),
                    created_at: "t".into(),
                },
            ],
        )
        .await
        .unwrap();

        let hits = mem.search("s1", "auth.rs line 42 token check", 2).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].text, "auth.rs line 42 token check");
        assert!(hits[0].score >= hits[1].score);
    }

    #[tokio::test]
    async fn search_is_session_scoped() {
        let mem = InMemoryMemory::new();
        mem.archive(
            "s1",
            vec![MemoryRecord {
                role: "tool".into(),
                tool_name: String::new(),
                text: "only in s1".into(),
                created_at: "t".into(),
            }],
        )
        .await
        .unwrap();
        assert!(mem.search("s2", "only in s1", 5).await.unwrap().is_empty());
        assert_eq!(mem.search("s1", "only in s1", 5).await.unwrap().len(), 1);
    }
}
