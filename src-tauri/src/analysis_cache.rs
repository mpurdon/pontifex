//! The last reality check per schema, kept so that coming back to a schema
//! shows what was found last time and when, rather than a blank panel until
//! Run is pressed again.
//!
//! Stored as the JSON the frontend already renders, keyed by environment and
//! schema name, for a month.

use crate::ttl_cache::{TtlCache, THIRTY_DAYS_MS};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedAnalysis {
    /// A `RealityCheckResult`, as serialised for the frontend.
    pub result: Value,
    /// When it was run, epoch ms.
    pub analysed_at: i64,
}

pub struct AnalysisCache(TtlCache<CachedAnalysis>);

impl AnalysisCache {
    pub fn load(cache_dir: &Path) -> Self {
        AnalysisCache(TtlCache::load(
            cache_dir.join("analyses.json"),
            THIRTY_DAYS_MS,
            |a| a.analysed_at,
            "analyses",
        ))
    }

    fn key(env_id: &str, name: &str) -> String {
        format!("{env_id}|{name}")
    }

    pub async fn get(&self, env_id: &str, name: &str) -> Option<CachedAnalysis> {
        self.0.get(&Self::key(env_id, name)).await
    }

    pub async fn put(&self, env_id: &str, name: &str, result: Value) {
        let entry = CachedAnalysis {
            result,
            analysed_at: crate::events_cache::now_ms(),
        };
        self.0.put(Self::key(env_id, name), entry).await
    }
}
