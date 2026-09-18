//! Everything watch mode remembers between runs, in one JSON file next to
//! settings.
//!
//! Kept out of `settings.json` on purpose: the Settings screen edits a copy of
//! that file and saves it wholesale, so a hit that landed while the form was
//! open would be written straight back over. Watches, hits and cursors change
//! on their own schedule and get their own file.
//!
//! Writes are deliberate rather than automatic: the cursor moves every poll,
//! and writing a file holding a thousand event payloads that often would be
//! the most expensive thing the poller does. It is kept in memory and lands
//! on disk with the next change that matters — a hit, a mark, a setting.

use super::{Watch, WatchHit, WatchMark, WatchMarkKind};
use crate::error::{Error, Result};
use crate::events_cache::now_ms;
use crate::logging::cat;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;

/// Hits kept per environment. Oldest are dropped past this.
pub const MAX_HITS_PER_ENV: usize = 1_000;
/// Start/stop marks kept per environment.
pub const MAX_MARKS_PER_ENV: usize = 200;

pub const DEFAULT_POLL_SECONDS: u64 = 15;
/// Minutes without a person touching the app before it asks whether to keep
/// watching. 0 disables the check.
pub const DEFAULT_IDLE_MINUTES: u64 = 240;
pub const MAX_IDLE_MINUTES: u64 = 24 * 60;
pub const MIN_POLL_SECONDS: u64 = 5;
pub const MAX_POLL_SECONDS: u64 = 900;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchFile {
    #[serde(default)]
    pub watches: Vec<Watch>,
    /// Environments whose poller should be running. Persisted so a restart
    /// picks up where it left off without being asked.
    #[serde(default)]
    pub armed: Vec<String>,
    #[serde(default)]
    pub hits: Vec<WatchHit>,
    /// When watching started and stopped, newest first. See `WatchMark`.
    #[serde(default)]
    pub marks: Vec<WatchMark>,
    /// Per environment: the point the poller has read up to, epoch millis.
    #[serde(default)]
    pub cursors: BTreeMap<String, i64>,
    /// Per environment: hits received at or before this are read.
    #[serde(default)]
    pub seen_at: BTreeMap<String, i64>,
    #[serde(default = "default_poll")]
    pub poll_seconds: u64,
    /// See `DEFAULT_IDLE_MINUTES`. Persisted as-is; 0 means never ask.
    #[serde(default = "default_idle")]
    pub idle_timeout_minutes: u64,
    /// Bumped by "ask again": the notification helper is reinstalled under a
    /// fresh bundle identifier so macOS raises the permission prompt afresh.
    /// See `notify::ensure_installed`.
    #[serde(default)]
    pub notifier_generation: u32,
}

fn default_poll() -> u64 {
    DEFAULT_POLL_SECONDS
}

fn default_idle() -> u64 {
    DEFAULT_IDLE_MINUTES
}

// Hand-written so an empty file and a missing file agree: `derive(Default)`
// would give zero poll seconds and "never ask", not the documented defaults.
impl Default for WatchFile {
    fn default() -> Self {
        WatchFile {
            watches: Vec::new(),
            armed: Vec::new(),
            hits: Vec::new(),
            marks: Vec::new(),
            cursors: BTreeMap::new(),
            seen_at: BTreeMap::new(),
            poll_seconds: default_poll(),
            idle_timeout_minutes: default_idle(),
            notifier_generation: 0,
        }
    }
}

/// The per-environment facts the UI's status line needs, read under one
/// lock in one pass over the hits.
pub struct StoreSnapshot {
    pub armed: bool,
    pub poll_seconds: u64,
    pub idle_timeout_minutes: u64,
    pub hits: usize,
    pub unread: usize,
    pub seen_at: i64,
}

pub struct WatchStore {
    path: PathBuf,
    data: RwLock<WatchFile>,
    /// One write at a time, in order: two pollers persisting together must
    /// not let an older snapshot land after a newer one.
    writing: tokio::sync::Mutex<()>,
}

impl WatchStore {
    pub fn path_in(config_dir: &Path) -> PathBuf {
        config_dir.join("watches.json")
    }

    pub fn load(config_dir: &Path) -> Self {
        let path = Self::path_in(config_dir);
        let data = match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str::<WatchFile>(&raw).unwrap_or_else(|e| {
                lwarn!(
                    cat::WATCH,
                    "{} is corrupt ({e}); starting with no watches",
                    path.display()
                );
                WatchFile::default()
            }),
            Err(_) => WatchFile::default(),
        };
        WatchStore {
            path,
            data: RwLock::new(data),
            writing: tokio::sync::Mutex::new(()),
        }
    }

    /// Write the file. Serialised under the read lock, written off the async
    /// runtime: the file carries every kept hit's payload, so this is the
    /// heaviest thing the store does and must not stall a poller.
    async fn persist(&self) -> Result<()> {
        let _turn = self.writing.lock().await;
        let snapshot = self.data.read().await.clone();
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || crate::settings::write_json_atomic(&path, &snapshot))
            .await
            .map_err(Error::internal)?
    }

    /// Forget environments that no longer exist: their armed flag, watches,
    /// hits, marks and cursors. Returns the ids that were armed, so their
    /// pollers can be stopped.
    pub async fn retain_environments(&self, keep: &[String]) -> Result<Vec<String>> {
        let dropped_armed = {
            let mut data = self.data.write().await;
            let gone = |env: &str| !keep.iter().any(|k| k == env);
            let dropped: Vec<String> = data.armed.iter().filter(|e| gone(e)).cloned().collect();
            if dropped.is_empty()
                && !data.watches.iter().any(|w| gone(&w.env_id))
                && !data.hits.iter().any(|h| gone(&h.env_id))
            {
                return Ok(Vec::new());
            }
            data.armed.retain(|e| !gone(e));
            data.watches.retain(|w| !gone(&w.env_id));
            data.hits.retain(|h| !gone(&h.env_id));
            data.marks.retain(|m| !gone(&m.env_id));
            data.cursors.retain(|e, _| !gone(e));
            data.seen_at.retain(|e, _| !gone(e));
            dropped
        };
        self.persist().await?;
        Ok(dropped_armed)
    }

    // --- watches ---------------------------------------------------------

    pub async fn watches_for(&self, env_id: &str) -> Vec<Watch> {
        self.data
            .read()
            .await
            .watches
            .iter()
            .filter(|w| w.env_id == env_id)
            .cloned()
            .collect()
    }

    pub async fn watch(&self, id: &str) -> Option<Watch> {
        self.data
            .read()
            .await
            .watches
            .iter()
            .find(|w| w.id == id)
            .cloned()
    }

    /// Insert or replace, keeping list order for an existing watch.
    pub async fn upsert(&self, watch: Watch) -> Result<Watch> {
        {
            let mut data = self.data.write().await;
            match data.watches.iter_mut().find(|w| w.id == watch.id) {
                Some(existing) => *existing = watch.clone(),
                None => data.watches.push(watch.clone()),
            }
        }
        self.persist().await?;
        Ok(watch)
    }

    pub async fn remove(&self, id: &str) -> Result<()> {
        {
            let mut data = self.data.write().await;
            data.watches.retain(|w| w.id != id);
            data.hits.retain(|h| h.watch_id != id);
        }
        self.persist().await
    }

    // --- arming and settings ---------------------------------------------

    pub async fn armed(&self) -> Vec<String> {
        self.data.read().await.armed.clone()
    }

    pub async fn set_armed(&self, env_id: &str, armed: bool) -> Result<()> {
        {
            let mut data = self.data.write().await;
            data.armed.retain(|e| e != env_id);
            if armed {
                data.armed.push(env_id.to_string());
            }
        }
        self.persist().await
    }

    pub async fn poll_seconds(&self) -> u64 {
        clamp_poll(self.data.read().await.poll_seconds)
    }

    pub async fn set_poll_seconds(&self, seconds: u64) -> Result<u64> {
        let seconds = clamp_poll(seconds);
        self.data.write().await.poll_seconds = seconds;
        self.persist().await?;
        Ok(seconds)
    }

    pub async fn idle_timeout_minutes(&self) -> u64 {
        self.data
            .read()
            .await
            .idle_timeout_minutes
            .min(MAX_IDLE_MINUTES)
    }

    pub async fn set_idle_timeout_minutes(&self, minutes: u64) -> Result<u64> {
        let minutes = minutes.min(MAX_IDLE_MINUTES);
        self.data.write().await.idle_timeout_minutes = minutes;
        self.persist().await?;
        Ok(minutes)
    }

    pub async fn notifier_generation(&self) -> u32 {
        self.data.read().await.notifier_generation
    }

    pub async fn bump_notifier_generation(&self) -> Result<u32> {
        let next = {
            let mut data = self.data.write().await;
            data.notifier_generation += 1;
            data.notifier_generation
        };
        self.persist().await?;
        Ok(next)
    }

    // --- cursor ------------------------------------------------------------

    pub async fn cursor(&self, env_id: &str) -> Option<i64> {
        self.data.read().await.cursors.get(env_id).copied()
    }

    /// Memory only — see the module note. A cursor lost to a crash costs one
    /// look-back on the next start, which happens anyway.
    pub async fn set_cursor(&self, env_id: &str, at: i64) {
        self.data
            .write()
            .await
            .cursors
            .insert(env_id.to_string(), at);
    }

    // --- hits and marks ----------------------------------------------------

    /// Record hits, newest first, dropping any already recorded for the same
    /// watch and event — the poller re-reads an overlap on every pass, and a
    /// restart re-reads even more.
    ///
    /// Returns the hits that were genuinely new.
    pub async fn push_hits(&self, hits: Vec<WatchHit>) -> Result<Vec<WatchHit>> {
        let fresh = {
            let mut data = self.data.write().await;
            let known: HashSet<(&str, &str)> = data
                .hits
                .iter()
                .map(|h| (h.watch_id.as_str(), h.event_id.as_str()))
                .collect();
            let fresh: Vec<WatchHit> = hits
                .into_iter()
                .filter(|h| !known.contains(&(h.watch_id.as_str(), h.event_id.as_str())))
                .collect();
            if !fresh.is_empty() {
                // Newest first: the new batch goes in front, in one move.
                data.hits.splice(0..0, fresh.iter().cloned());
                trim_hits(&mut data.hits);
            }
            fresh
        };
        if !fresh.is_empty() {
            self.persist().await?;
        }
        Ok(fresh)
    }

    /// Record that watching started or stopped just now.
    pub async fn push_mark(&self, env_id: &str, kind: WatchMarkKind) -> Result<WatchMark> {
        let mark = WatchMark {
            id: uuid::Uuid::new_v4().to_string(),
            env_id: env_id.to_string(),
            kind,
            at: now_ms(),
        };
        {
            let mut data = self.data.write().await;
            data.marks.insert(0, mark.clone());
            let mut kept = 0;
            data.marks.retain(|m| {
                if m.env_id != env_id {
                    return true;
                }
                kept += 1;
                kept <= MAX_MARKS_PER_ENV
            });
        }
        self.persist().await?;
        Ok(mark)
    }

    pub async fn marks_for(&self, env_id: &str) -> Vec<WatchMark> {
        self.data
            .read()
            .await
            .marks
            .iter()
            .filter(|m| m.env_id == env_id)
            .cloned()
            .collect()
    }

    pub async fn hits_for(&self, env_id: &str, limit: usize) -> Vec<WatchHit> {
        self.data
            .read()
            .await
            .hits
            .iter()
            .filter(|h| h.env_id == env_id)
            .take(limit)
            .cloned()
            .collect()
    }

    /// Forget an environment's hits — all of them, or one watch's.
    ///
    /// Session marks live as long as their group does: a start/stop pair
    /// with no hit left between them is dropped, so clearing a group's
    /// events clears its rules too. The session still running keeps its
    /// start mark regardless — it is not over yet.
    pub async fn clear_hits(
        &self,
        env_id: &str,
        watch_id: Option<&str>,
        live_since: Option<i64>,
    ) -> Result<()> {
        {
            let mut data = self.data.write().await;
            data.hits
                .retain(|h| h.env_id != env_id || watch_id.is_some_and(|w| h.watch_id != w));
            let hit_times: Vec<i64> = data
                .hits
                .iter()
                .filter(|h| h.env_id == env_id)
                .map(|h| h.timestamp)
                .collect();
            let keep = surviving_marks(
                data.marks.iter().filter(|m| m.env_id == env_id),
                &hit_times,
                live_since,
            );
            data.marks
                .retain(|m| m.env_id != env_id || keep.contains(&m.id));
        }
        self.persist().await
    }

    pub async fn mark_seen(&self, env_id: &str, at: i64) -> Result<()> {
        self.data
            .write()
            .await
            .seen_at
            .insert(env_id.to_string(), at);
        self.persist().await
    }

    /// Everything the status line needs about one environment, in one read.
    pub async fn snapshot(&self, env_id: &str) -> StoreSnapshot {
        let data = self.data.read().await;
        let seen_at = data.seen_at.get(env_id).copied().unwrap_or(0);
        let (hits, unread) = data
            .hits
            .iter()
            .filter(|h| h.env_id == env_id)
            .fold((0, 0), |(hits, unread), h| {
                (hits + 1, unread + usize::from(is_unread(h, seen_at)))
            });
        StoreSnapshot {
            armed: data.armed.iter().any(|e| e == env_id),
            poll_seconds: clamp_poll(data.poll_seconds),
            idle_timeout_minutes: data.idle_timeout_minutes.min(MAX_IDLE_MINUTES),
            hits,
            unread,
            seen_at,
        }
    }

    /// Unread hits across every environment, for the Dock badge.
    pub async fn unread_total(&self) -> usize {
        let data = self.data.read().await;
        data.hits
            .iter()
            .filter(|h| is_unread(h, data.seen_at.get(&h.env_id).copied().unwrap_or(0)))
            .count()
    }
}

/// Look-back hits are context, not news: they never count as unread, so the
/// badges only ever say how many hits were caught live.
fn is_unread(hit: &WatchHit, seen_at: i64) -> bool {
    !hit.backfill && hit.received_at > seen_at
}

fn clamp_poll(seconds: u64) -> u64 {
    seconds.clamp(MIN_POLL_SECONDS, MAX_POLL_SECONDS)
}

/// Which marks still have a reason to exist: every start whose session —
/// up to the next stop, or open-ended — still contains a hit, plus that
/// session's stop; and the start of the session running now.
fn surviving_marks<'a>(
    marks: impl Iterator<Item = &'a WatchMark>,
    hit_times: &[i64],
    live_since: Option<i64>,
) -> HashSet<String> {
    let mut ordered: Vec<&WatchMark> = marks.collect();
    ordered.sort_by_key(|m| m.at);
    let mut keep = HashSet::new();
    for (i, mark) in ordered.iter().enumerate() {
        if mark.kind != WatchMarkKind::Start {
            // A stop with no start before it belongs to nothing.
            continue;
        }
        let stop = ordered[i + 1..]
            .iter()
            .find(|m| m.kind == WatchMarkKind::Stop)
            .copied();
        let end = stop.map(|m| m.at).unwrap_or(i64::MAX);
        let has_hit = hit_times.iter().any(|t| *t >= mark.at && *t <= end);
        if has_hit || live_since == Some(mark.at) {
            keep.insert(mark.id.clone());
            keep.extend(stop.map(|m| m.id.clone()));
        }
    }
    keep
}

/// Keep the newest `MAX_HITS_PER_ENV` per environment. `hits` is newest-first.
fn trim_hits(hits: &mut Vec<WatchHit>) {
    let mut kept: BTreeMap<String, usize> = BTreeMap::new();
    hits.retain(|hit| {
        let n = kept.entry(hit.env_id.clone()).or_insert(0);
        *n += 1;
        *n <= MAX_HITS_PER_ENV
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hit(env: &str, watch: &str, event: &str, at: i64) -> WatchHit {
        WatchHit {
            id: format!("{env}-{watch}-{event}"),
            watch_id: watch.into(),
            watch_label: watch.into(),
            env_id: env.into(),
            log_group: "g".into(),
            event_id: event.into(),
            timestamp: at,
            received_at: at,
            source: None,
            detail_type: None,
            event: json!({}),
            backfill: false,
        }
    }

    fn store() -> (WatchStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "pontifex-watch-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        (WatchStore::load(&dir), dir)
    }

    #[tokio::test]
    async fn duplicate_hits_are_dropped_and_unread_counts_the_rest() {
        let (store, dir) = store();
        let fresh = store
            .push_hits(vec![hit("dev", "w", "e1", 10), hit("dev", "w", "e2", 20)])
            .await
            .unwrap();
        assert_eq!(fresh.len(), 2);
        let again = store
            .push_hits(vec![hit("dev", "w", "e1", 10)])
            .await
            .unwrap();
        assert!(again.is_empty());
        assert_eq!(store.snapshot("dev").await.unread, 2);
        store.mark_seen("dev", 10).await.unwrap();
        assert_eq!(store.snapshot("dev").await.unread, 1);

        // A look-back hit is stored but never unread.
        let mut old = hit("dev", "w", "e0", 30);
        old.backfill = true;
        store.push_hits(vec![old]).await.unwrap();
        assert_eq!(store.snapshot("dev").await.hits, 3);
        assert_eq!(store.snapshot("dev").await.unread, 1);
        assert_eq!(store.unread_total().await, 1);

        // Survives a reload, and a fresh file carries the documented defaults.
        let reloaded = WatchStore::load(&dir);
        assert_eq!(reloaded.hits_for("dev", 10).await.len(), 3);
        assert_eq!(reloaded.poll_seconds().await, DEFAULT_POLL_SECONDS);
        assert_eq!(reloaded.idle_timeout_minutes().await, DEFAULT_IDLE_MINUTES);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn marks_outlive_only_the_hits_of_their_session() {
        let mark = |id: &str, kind: WatchMarkKind, at: i64| WatchMark {
            id: id.into(),
            env_id: "dev".into(),
            kind,
            at,
        };
        use WatchMarkKind::{Start, Stop};
        let marks = vec![
            mark("s1", Start, 100),
            mark("e1", Stop, 200),
            mark("s2", Start, 300),
            mark("e2", Stop, 400),
            mark("s3", Start, 500),
        ];
        // Only session 2 still has a hit; session 3 is the live one.
        let keep = surviving_marks(marks.iter(), &[350], Some(500));
        assert_eq!(keep.len(), 3);
        assert!(keep.contains("s2") && keep.contains("e2") && keep.contains("s3"));
        // Nothing live, nothing hit: everything goes.
        assert!(surviving_marks(marks.iter(), &[], None).is_empty());
        // A look-back hit before any session keeps no mark.
        assert!(surviving_marks(marks.iter(), &[50], None).is_empty());
    }

    #[test]
    fn trimming_is_per_environment() {
        let mut hits: Vec<WatchHit> = (0..(MAX_HITS_PER_ENV + 5))
            .map(|i| hit("dev", "w", &i.to_string(), i as i64))
            .chain((0..3).map(|i| hit("prd", "w", &i.to_string(), i as i64)))
            .collect();
        trim_hits(&mut hits);
        assert_eq!(
            hits.iter().filter(|h| h.env_id == "dev").count(),
            MAX_HITS_PER_ENV
        );
        assert_eq!(hits.iter().filter(|h| h.env_id == "prd").count(), 3);
    }
}
