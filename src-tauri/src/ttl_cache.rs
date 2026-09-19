//! A small on-disk cache of answers that go stale slowly: keyed, stamped,
//! dropped after a fixed age, and written whole after every change.
//!
//! The answers are the point, not the cache — an origin lookup costs API
//! calls and an analysis costs a scan, so remembering them for a month is
//! what lets the panels open already filled in. Entries older than the TTL
//! are dropped on load and on every write, so the file cannot grow without
//! bound and a stale answer is never mistaken for a current one.

use crate::events_cache::now_ms;
use crate::logging::cat;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::sync::RwLock;

pub const THIRTY_DAYS_MS: i64 = 30 * 24 * 60 * 60 * 1000;

pub struct TtlCache<T> {
    path: PathBuf,
    ttl_ms: i64,
    /// When an entry was made, epoch ms.
    stamp: fn(&T) -> i64,
    /// What the entries are, for the log.
    what: &'static str,
    entries: RwLock<BTreeMap<String, T>>,
}

impl<T> TtlCache<T>
where
    T: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    pub fn load(path: PathBuf, ttl_ms: i64, stamp: fn(&T) -> i64, what: &'static str) -> Self {
        let mut entries: BTreeMap<String, T> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        let before = entries.len();
        prune(&mut entries, ttl_ms, stamp);
        if entries.len() < before {
            linfo!(
                cat::CACHE,
                "dropped {} {what} older than {} days",
                before - entries.len(),
                ttl_ms / (24 * 60 * 60 * 1000)
            );
        }
        TtlCache {
            path,
            ttl_ms,
            stamp,
            what,
            entries: RwLock::new(entries),
        }
    }

    pub async fn get(&self, key: &str) -> Option<T> {
        self.entries
            .read()
            .await
            .get(key)
            .filter(|entry| now_ms() - (self.stamp)(entry) < self.ttl_ms)
            .cloned()
    }

    /// Insert, prune, and write the file. The bytes are produced under the
    /// lock so nothing is cloned; only the write leaves the async runtime.
    pub async fn put(&self, key: String, value: T) {
        let bytes = {
            let mut entries = self.entries.write().await;
            entries.insert(key, value);
            prune(&mut entries, self.ttl_ms, self.stamp);
            serde_json::to_vec(&*entries)
        };
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(e) => {
                lwarn!(
                    cat::CACHE,
                    "could not serialise the {} cache: {e}",
                    self.what
                );
                return;
            }
        };
        let path = self.path.clone();
        let written =
            tokio::task::spawn_blocking(move || crate::settings::write_bytes_atomic(&path, &bytes))
                .await;
        let failed = match written {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e.to_string()),
            Err(e) => Some(e.to_string()),
        };
        if let Some(e) = failed {
            lwarn!(cat::CACHE, "could not persist the {} cache: {e}", self.what);
        }
    }
}

fn prune<T>(entries: &mut BTreeMap<String, T>, ttl_ms: i64, stamp: fn(&T) -> i64) {
    let cutoff = now_ms() - ttl_ms;
    entries.retain(|_, entry| stamp(entry) >= cutoff);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_past_the_ttl_is_pruned_and_a_fresh_one_kept() {
        let mut entries: BTreeMap<String, i64> = BTreeMap::new();
        entries.insert("old".into(), now_ms() - THIRTY_DAYS_MS - 1);
        entries.insert("new".into(), now_ms());
        prune(&mut entries, THIRTY_DAYS_MS, |at| *at);
        assert_eq!(entries.keys().collect::<Vec<_>>(), vec!["new"]);
    }
}
