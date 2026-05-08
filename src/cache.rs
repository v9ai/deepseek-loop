use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// In-memory cache with TTL, max entries, and in-flight deduplication.
#[derive(Clone)]
pub struct Cache {
    entries: Arc<DashMap<String, CacheEntry>>,
    ttl: Duration,
    max_entries: usize,
}

struct CacheEntry {
    value: String,
    expires_at: Instant,
}

impl Cache {
    pub fn new(ttl_secs: u64, max_entries: usize) -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
            ttl: Duration::from_secs(ttl_secs),
            max_entries,
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let entry = self.entries.get(key)?;
        if Instant::now() < entry.expires_at {
            Some(entry.value.clone())
        } else {
            drop(entry);
            self.entries.remove(key);
            None
        }
    }

    pub fn set(&self, key: String, value: String) {
        if self.entries.len() >= self.max_entries {
            self.evict_expired();
        }
        self.entries.insert(key, CacheEntry {
            value,
            expires_at: Instant::now() + self.ttl,
        });
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn evict_expired(&self) {
        let now = Instant::now();
        self.entries.retain(|_, v| v.expires_at > now);
    }
}

/// In-flight deduplication — prevents duplicate concurrent API calls for the same key.
#[derive(Clone)]
pub struct InFlightDedup<T: Clone + Send + 'static> {
    map: Arc<DashMap<String, broadcast::Sender<T>>>,
}

impl<T: Clone + Send + 'static> InFlightDedup<T> {
    pub fn new() -> Self {
        Self { map: Arc::new(DashMap::with_shard_amount(32)) }
    }

    /// Returns `Some(receiver)` if another call is already in flight for this key.
    /// Returns `None` and registers this call as in-flight.
    pub fn try_join(&self, key: &str) -> Option<broadcast::Receiver<T>> {
        use dashmap::mapref::entry::Entry;
        match self.map.entry(key.to_string()) {
            Entry::Occupied(entry) => Some(entry.get().subscribe()),
            Entry::Vacant(entry) => {
                let (tx, _) = broadcast::channel(4);
                entry.insert(tx);
                None
            }
        }
    }

    /// Complete an in-flight call — broadcasts result to waiters and removes the entry.
    pub fn complete(&self, key: &str, value: T) {
        if let Some((_, tx)) = self.map.remove(key) {
            let _ = tx.send(value);
        }
    }

    /// Remove an in-flight entry without broadcasting (e.g. on error).
    pub fn cancel(&self, key: &str) {
        self.map.remove(key);
    }
}

impl<T: Clone + Send + 'static> Default for InFlightDedup<T> {
    fn default() -> Self { Self::new() }
}
