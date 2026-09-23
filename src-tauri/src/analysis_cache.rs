//! The last reality check per schema, kept so that coming back to a schema
//! shows what was found last time and when, rather than a blank panel until
//! Run is pressed again.
//!
//! Stored as the result the frontend already renders, keyed by environment
//! and schema name, for a month.

use crate::commands::reality::RealityCheckResult;
use crate::ttl_cache::{TtlCache, THIRTY_DAYS_MS};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Which grading produced a stored result.
///
/// An issue key names a finding to everything that refers to one: the resolved
/// list, a filed ticket, and the drawer that goes looking for the events
/// behind it. So a result graded by a build that spelled those keys
/// differently cannot be shown as if this build had produced it — the rows
/// would look right and the buttons on them would find nothing.
///
/// Bump this whenever `schema::events` changes what an issue is called.
const GRADING: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedAnalysis {
    pub result: RealityCheckResult,
    /// When it was run, epoch ms.
    pub analysed_at: i64,
    /// The [`GRADING`] that produced `result`. Absent on anything stored
    /// before this was recorded, which is exactly what it exists to reject.
    #[serde(default)]
    pub grading: u32,
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

    /// The stored analysis, unless an older build graded it — in which case
    /// there is nothing to show but Run, which is where a schema with no
    /// stored analysis starts anyway.
    pub async fn get(&self, env_id: &str, name: &str) -> Option<CachedAnalysis> {
        self.0
            .get(&Self::key(env_id, name))
            .await
            .filter(|cached| cached.grading == GRADING)
    }

    pub async fn put(&self, env_id: &str, name: &str, result: RealityCheckResult) {
        let entry = CachedAnalysis {
            result,
            analysed_at: crate::events_cache::now_ms(),
            grading: GRADING,
        };
        self.0.put(Self::key(env_id, name), entry).await
    }
}
