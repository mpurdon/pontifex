//! A disk-backed cache of sampled event payloads.
//!
//! CloudWatch's `FilterLogEvents` bills per GB scanned and takes seconds over a
//! busy log group, so re-fetching the same window while iterating on a schema
//! is both slow and expensive. Caching the sample turns the reality check into
//! something that can re-run on every keystroke: validate the draft against
//! events already in hand, with no AWS call at all.
//!
//! The registry-wide report warms this for *every* event type it sees, so one
//! report run makes every schema's check instant.

use crate::logging::cat;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;

/// Most events we keep per event type. Enough to characterise a payload shape;
/// beyond this the marginal signal is nil and the file just grows.
const MAX_EVENTS_PER_TYPE: usize = 200;

/// Cap on cached event types, evicting least-recently-fetched first.
const MAX_TYPES: usize = 500;

/// Default disk budget for the cache, overridable in Settings → Scanning.
///
/// The count caps alone are not enough: real payloads vary hugely in size —
/// some carry presigned URLs over a kilobyte long — so 500 types × 200 events
/// could run to hundreds of megabytes. This bounds what the app actually
/// occupies, which is what a user would object to.
///
/// Derived from the settings default rather than restated, so the two cannot
/// disagree. `AppState::new` overwrites it from the user's setting immediately;
/// this only covers the window before that.
fn default_cache_bytes() -> usize {
    crate::settings::ScanSettings::default().cache_bytes()
}

/// One sampled event, keyed by its CloudWatch id so repeated fetches of an
/// overlapping window do not double-count.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedEvent {
    pub id: String,
    /// Epoch milliseconds.
    pub timestamp: i64,
    /// The event `detail` — the only part a schema describes.
    pub detail: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedSample {
    /// Epoch millis of the earliest point this sample is known to cover.
    pub window_start: i64,
    /// Epoch millis of the latest point covered.
    pub window_end: i64,
    /// When the sample was last fetched, epoch millis.
    pub fetched_at: i64,
    pub events: Vec<CachedEvent>,
    /// True when the fetch stopped at its page cap, so the sample is partial.
    pub truncated: bool,
}

impl CachedSample {
    /// Whether this sample already covers a requested window.
    ///
    /// A truncated sample is never treated as covering anything: it stopped
    /// early, so absence of an event proves nothing about the window.
    pub fn covers(&self, start: i64, end: i64) -> bool {
        !self.truncated && self.window_start <= start && self.window_end >= end
    }

    /// Events falling inside a window, newest first.
    pub fn events_in(&self, start: i64, end: i64) -> Vec<Value> {
        let mut selected: Vec<&CachedEvent> = self
            .events
            .iter()
            .filter(|e| e.timestamp >= start && e.timestamp <= end)
            .collect();
        selected.sort_by_key(|e| std::cmp::Reverse(e.timestamp));
        selected.iter().map(|e| e.detail.clone()).collect()
    }

    pub fn age_ms(&self, now: i64) -> i64 {
        (now - self.fetched_at).max(0)
    }
}

/// What the cache holds for one event type across an environment's log groups.
#[derive(Debug, Clone, Default)]
pub struct CachedAcross {
    /// Event details, newest first within each group.
    pub payloads: Vec<Value>,
    /// The first group that had anything, for saying where a sample came from.
    pub found_in: Option<String>,
    /// Age of the oldest sample consulted; `None` when nothing was cached.
    pub age_ms: Option<i64>,
    /// True when every group's sample spans the whole window untruncated.
    pub covered: bool,
}

/// Cache key: an event type within one environment's log group.
///
/// Encoded as a single string so the persisted form is a plain JSON object.
pub fn cache_key(env_id: &str, log_group: &str, source: &str, detail_type: &str) -> String {
    format!("{env_id}\u{1f}{log_group}\u{1f}{source}@{detail_type}")
}

/// Read a key back into its parts.
///
/// The counterpart to [`cache_key`], here rather than at the caller so the two
/// halves of the format cannot drift apart. The event type splits on the
/// *last* `@`, because a source may legitimately contain one.
pub fn parse_cache_key(key: &str) -> Option<(&str, &str, &str, &str)> {
    let mut parts = key.split('\u{1f}');
    let env_id = parts.next()?;
    let log_group = parts.next()?;
    let (source, detail_type) = parts.next()?.rsplit_once('@')?;
    Some((env_id, log_group, source, detail_type))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CacheFile {
    #[serde(default)]
    samples: HashMap<String, CachedSample>,
}

/// Borrowing counterpart to [`CacheFile`], so writing the cache does not have to
/// deep-clone it first. The cache reaches tens of megabytes, and a clone of that
/// tree costs millions of small allocations.
#[derive(Serialize)]
struct CacheFileRef<'a> {
    samples: &'a HashMap<String, CachedSample>,
}

/// Cache of sampled events, held in memory and mirrored to disk.
pub struct EventCache {
    path: PathBuf,
    samples: RwLock<HashMap<String, CachedSample>>,
    /// Disk budget in bytes. Atomic so a settings save can change it without
    /// taking the sample lock or rebuilding the cache.
    budget_bytes: AtomicUsize,
}

impl EventCache {
    /// Load from disk, or start empty if there is nothing (or it is corrupt —
    /// a cache is disposable, so a bad file is discarded rather than fatal).
    pub fn load(cache_dir: &Path) -> Self {
        let path = cache_dir.join("event-samples.json");
        let started = std::time::Instant::now();
        let raw = std::fs::read_to_string(&path).ok();
        let bytes = raw.as_ref().map(String::len).unwrap_or(0);
        let samples = raw
            .and_then(|raw| match serde_json::from_str::<CacheFile>(&raw) {
                Ok(file) => Some(file.samples),
                Err(e) => {
                    lwarn!(cat::CACHE, "discarding unreadable cache at {}: {e}", path.display());
                    None
                }
            })
            .unwrap_or_default();
        let events: usize = samples.values().map(|s| s.events.len()).sum();
        linfo!(
            cat::CACHE,
            "loaded {} event types / {events} events ({} KB) in {}ms from {}",
            samples.len(),
            bytes / 1024,
            started.elapsed().as_millis(),
            path.display()
        );

        EventCache {
            path,
            samples: RwLock::new(samples),
            budget_bytes: AtomicUsize::new(default_cache_bytes()),
        }
    }

    /// Change the disk budget. Takes effect on the next persist.
    pub fn set_budget_bytes(&self, bytes: usize) {
        let previous = self.budget_bytes.swap(bytes, Ordering::Relaxed);
        if previous != bytes {
            linfo!(
                cat::CACHE,
                "cache budget set to {} MB (was {} MB)",
                bytes / (1024 * 1024),
                previous / (1024 * 1024)
            );
        }
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes.load(Ordering::Relaxed)
    }

    pub async fn get(&self, key: &str) -> Option<CachedSample> {
        self.samples.read().await.get(key).cloned()
    }

    /// Everything cached for one event type, across every log group it may
    /// have arrived in.
    ///
    /// An environment normally reads two log groups — events published to the
    /// bus, and events bridged in — and a type can legitimately appear in
    /// either. Three callers had each written this loop, and they had already
    /// diverged over whether truncation and cache age were tracked.
    ///
    /// Selects under the read guard, so the caller gets one copy of the
    /// payloads it asked for rather than a deep clone of every cached sample
    /// followed by a second clone of the selection.
    pub async fn sample_across(
        &self,
        env_id: &str,
        log_groups: &[String],
        source: &str,
        detail_type: &str,
        start: i64,
        end: i64,
    ) -> CachedAcross {
        let samples = self.samples.read().await;
        let mut across = CachedAcross {
            // Vacuously covered: `covered` narrows as groups are inspected, so
            // a caller with no groups is not told the window is missing.
            covered: true,
            ..Default::default()
        };

        for group in log_groups {
            let key = cache_key(env_id, group, source, detail_type);
            let Some(sample) = samples.get(&key) else {
                across.covered = false;
                continue;
            };

            let events = sample.events_in(start, end);
            if !events.is_empty() && across.found_in.is_none() {
                across.found_in = Some(group.clone());
            }
            across.payloads.extend(events);

            let age = sample.age_ms(end);
            across.age_ms = Some(across.age_ms.map_or(age, |current: i64| current.max(age)));
            across.covered &= sample.covers(start, end);
        }

        across
    }

    /// Every event source seen in this environment's sampled traffic.
    ///
    /// Read off the cache rather than the registry, because the two disagree
    /// exactly where it matters: a producer with no schema is absent from the
    /// registry, and an event source containing characters a schema name
    /// cannot hold — a space, say — is registered under a different spelling
    /// than the one its events carry.
    pub async fn sources(&self, env_id: &str) -> std::collections::BTreeSet<String> {
        self.samples
            .read()
            .await
            .keys()
            .filter_map(|key| parse_cache_key(key))
            .filter(|(env, _, _, _)| *env == env_id)
            .map(|(_, _, source, _)| source.to_string())
            .collect()
    }

    /// Merge freshly fetched events into whatever is already cached.
    ///
    /// Merging rather than replacing is what makes an extended window cheap:
    /// only the new slice has to come from CloudWatch, and re-fetching an
    /// overlapping range costs nothing because ids dedupe.
    ///
    /// Updates memory only — call [`persist`] once after a batch, so warming
    /// 80 event types from one scan is one disk write rather than 80.
    pub async fn merge(
        &self,
        key: &str,
        fetched: Vec<CachedEvent>,
        window_start: i64,
        window_end: i64,
        truncated: bool,
    ) {
        let now = now_ms();
        {
            let mut samples = self.samples.write().await;
            let entry = samples.entry(key.to_string()).or_insert(CachedSample {
                window_start,
                window_end,
                fetched_at: now,
                events: Vec::new(),
                truncated,
            });

            let existing: std::collections::HashSet<String> =
                entry.events.iter().map(|e| e.id.clone()).collect();
            for event in fetched {
                if !existing.contains(&event.id) {
                    entry.events.push(event);
                }
            }

            // Newest first, then trim: an old event is the one worth dropping.
            entry.events.sort_by_key(|e| std::cmp::Reverse(e.timestamp));
            entry.events.truncate(MAX_EVENTS_PER_TYPE);

            entry.window_start = entry.window_start.min(window_start);
            entry.window_end = entry.window_end.max(window_end);
            entry.fetched_at = now;
            // Once a fetch covers a window cleanly, the sample stops being
            // partial; a truncated fetch makes it partial again.
            entry.truncated = truncated;

            let dropped = evict(&mut samples);
            if dropped > 0 {
                linfo!(cat::CACHE, "evicted {dropped} event types over the {MAX_TYPES} type cap");
            }
        }
        ldebug!(cat::CACHE, "merged events into {key}");
    }

    /// Drop cached samples, optionally only for one environment.
    pub async fn clear(&self, env_id: Option<&str>) -> usize {
        linfo!(cat::CACHE, "clearing cached events for {}", env_id.unwrap_or("every environment"));
        let removed = {
            let mut samples = self.samples.write().await;
            let before = samples.len();
            match env_id {
                Some(env) => {
                    let prefix = format!("{env}\u{1f}");
                    samples.retain(|key, _| !key.starts_with(&prefix));
                }
                None => samples.clear(),
            }
            before - samples.len()
        };
        self.persist().await;
        removed
    }

    /// Write the cache to disk, awaiting completion.
    pub async fn persist(&self) {
        self.write_to_disk().await;
    }

    /// Total cached events across every type, for the settings screen.
    pub async fn stats(&self) -> (usize, usize) {
        let samples = self.samples.read().await;
        (samples.len(), samples.values().map(|s| s.events.len()).sum())
    }

    /// Drop the oldest tenth of the cache, returning how many types went.
    ///
    /// A tenth at a time because the budget is only knowable once serialized,
    /// and re-serializing after every single eviction would be quadratic.
    /// Returns 0 when there is nothing left to give up.
    async fn evict_oldest_tenth(&self) -> usize {
        let mut samples = self.samples.write().await;
        if samples.len() <= 1 {
            return 0;
        }
        let count = (samples.len() / 10).max(1);
        drop_oldest(&mut samples, count)
    }

    /// Serialize the cache, evicting until it fits its budget.
    ///
    /// The string that gets measured is the string that gets written — the
    /// cache runs to tens of megabytes, so serializing separately to size it
    /// would double the cost of every persist.
    async fn serialize_within_budget(&self) -> Option<(String, usize)> {
        let mut evicted = 0usize;
        loop {
            let json = {
                let samples = self.samples.read().await;
                serde_json::to_string(&CacheFileRef { samples: &samples }).ok()?
            };
            if json.len() <= self.budget_bytes() {
                return Some((json, evicted));
            }
            match self.evict_oldest_tenth().await {
                0 => return Some((json, evicted)),
                dropped => evicted += dropped,
            }
        }
    }

    async fn write_to_disk(&self) {
        let Some((json, evicted)) = self.serialize_within_budget().await else {
            return;
        };
        if evicted > 0 {
            lwarn!(
                cat::CACHE,
                "cache exceeded {} MB; evicted {evicted} least-recently-used event types",
                self.budget_bytes() / (1024 * 1024)
            );
        }
        let kb = json.len() / 1024;

        let path = self.path.clone();
        // Writing is best-effort: a cache that fails to persist is a slower
        // next launch, not an error worth surfacing. Awaited so callers (and
        // tests) can rely on it having happened.
        let _ = tokio::task::spawn_blocking(move || {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let tmp = path.with_extension("json.tmp");
            match std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, &path)) {
                Ok(()) => ldebug!(cat::CACHE, "persisted {kb} KB to {}", path.display()),
                Err(e) => lwarn!(cat::CACHE, "could not persist cache to {}: {e}", path.display()),
            }
        })
        .await;
    }
}

/// Drop the `count` least-recently-fetched types, returning how many went.
///
/// The one place that decides what "least recently used" means for this cache;
/// both the count cap and the size budget spend it.
fn drop_oldest(samples: &mut HashMap<String, CachedSample>, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    let mut by_age: Vec<(String, i64)> = samples
        .iter()
        .map(|(k, v)| (k.clone(), v.fetched_at))
        .collect();
    by_age.sort_by_key(|(_, fetched_at)| *fetched_at);

    let mut evicted = 0usize;
    for (key, _) in by_age.into_iter().take(count) {
        samples.remove(&key);
        evicted += 1;
    }
    evicted
}

/// Evict least-recently-fetched types once the cache grows past its cap.
fn evict(samples: &mut HashMap<String, CachedSample>) -> usize {
    drop_oldest(samples, samples.len().saturating_sub(MAX_TYPES))
}

pub fn now_ms() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp() * 1000
}

/// Parse a raw CloudWatch log message into a cacheable event.
pub fn parse_event(id: &str, timestamp: i64, message: &str) -> Option<(String, String, CachedEvent)> {
    let envelope: Value = serde_json::from_str(message).ok()?;
    let source = envelope.get("source")?.as_str()?.to_string();
    let detail_type = envelope.get("detail-type")?.as_str()?.to_string();
    let detail = envelope.get("detail")?.clone();
    Some((
        source,
        detail_type,
        CachedEvent {
            id: id.to_string(),
            timestamp,
            detail,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cache_key_survives_the_round_trip_including_awkward_sources() {
        // A source may hold a space (registered under a different spelling) or
        // an `@`, so the type splits on the last one.
        for (source, detail) in [
            ("Atomic Forms", "CASE_FILED"),
            ("milo-disability", "lead-unreached"),
            ("weird@source", "thing-happened"),
        ] {
            let key = cache_key("prd", "/aws/events/prd-global-events", source, detail);
            assert_eq!(
                parse_cache_key(&key),
                Some(("prd", "/aws/events/prd-global-events", source, detail)),
            );
        }
    }

    #[test]
    fn a_key_that_is_not_one_parses_to_nothing() {
        assert!(parse_cache_key("nonsense").is_none());
        assert!(parse_cache_key("prd\u{1f}group\u{1f}no-at-sign").is_none());
    }

    use super::*;
    use serde_json::json;

    fn event(id: &str, ts: i64) -> CachedEvent {
        CachedEvent {
            id: id.into(),
            timestamp: ts,
            detail: json!({ "n": id }),
        }
    }

    fn temp_cache() -> (EventCache, tempdir::Dir) {
        let dir = tempdir::Dir::new();
        (EventCache::load(dir.path()), dir)
    }

    /// Minimal scratch directory helper — avoids a dev-dependency for two tests.
    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU64, Ordering};

        /// Tests run concurrently in one process, so a timestamp is not unique
        /// enough: two directories colliding means one test's cleanup deletes
        /// another's files mid-run.
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        pub struct Dir(PathBuf);
        impl Dir {
            pub fn new() -> Self {
                let base = std::env::temp_dir().join(format!(
                    "gebman-cache-test-{}-{}",
                    std::process::id(),
                    COUNTER.fetch_add(1, Ordering::SeqCst),
                ));
                let _ = std::fs::remove_dir_all(&base);
                std::fs::create_dir_all(&base).unwrap();
                Dir(base)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn key_separates_environment_log_group_and_event_type() {
        let a = cache_key("prd", "/aws/events/prd", "billing", "update-client");
        let b = cache_key("dev", "/aws/events/dev", "billing", "update-client");
        assert_ne!(a, b);
        // The separator cannot appear in a real name, so keys cannot collide.
        assert!(a.contains('\u{1f}'));
    }

    #[test]
    fn covers_only_a_fully_contained_window() {
        let sample = CachedSample {
            window_start: 100,
            window_end: 200,
            fetched_at: 200,
            events: vec![],
            truncated: false,
        };
        assert!(sample.covers(120, 180));
        assert!(sample.covers(100, 200));
        assert!(!sample.covers(90, 180), "starts before the cached window");
        assert!(!sample.covers(120, 210), "ends after the cached window");
    }

    #[test]
    fn a_truncated_sample_never_counts_as_covering() {
        // It stopped early, so a missing event proves nothing.
        let sample = CachedSample {
            window_start: 100,
            window_end: 200,
            fetched_at: 200,
            events: vec![],
            truncated: true,
        };
        assert!(!sample.covers(120, 180));
    }

    #[test]
    fn selects_events_inside_a_window_newest_first() {
        let sample = CachedSample {
            window_start: 0,
            window_end: 500,
            fetched_at: 500,
            events: vec![event("a", 100), event("b", 300), event("c", 200)],
            truncated: false,
        };
        let inside = sample.events_in(150, 400);
        assert_eq!(inside.len(), 2);
        assert_eq!(inside[0]["n"], "b");
        assert_eq!(inside[1]["n"], "c");
    }

    #[tokio::test]
    async fn merge_dedupes_by_id_and_widens_the_window() {
        let (cache, _dir) = temp_cache();
        let key = "k";

        cache
            .merge(key, vec![event("a", 100), event("b", 200)], 100, 200, false)
            .await;
        // An overlapping refetch must not double-count.
        cache
            .merge(key, vec![event("b", 200), event("c", 300)], 200, 300, false)
            .await;

        let sample = cache.get(key).await.unwrap();
        assert_eq!(sample.events.len(), 3);
        assert_eq!(sample.window_start, 100);
        assert_eq!(sample.window_end, 300);
    }

    #[tokio::test]
    async fn merge_keeps_the_newest_events_when_capped() {
        let (cache, _dir) = temp_cache();
        let many: Vec<CachedEvent> = (0..(MAX_EVENTS_PER_TYPE + 50) as i64)
            .map(|i| event(&format!("e{i}"), i))
            .collect();

        cache.merge("k", many, 0, 1_000, false).await;

        let sample = cache.get("k").await.unwrap();
        assert_eq!(sample.events.len(), MAX_EVENTS_PER_TYPE);
        // Newest survive; the oldest are the ones dropped.
        assert!(sample.events.iter().all(|e| e.timestamp >= 50));
    }

    #[tokio::test]
    async fn evicts_oldest_types_to_stay_within_the_size_budget() {
        let (cache, _dir) = temp_cache();

        // Deliberately over the budget: 120 × 400 KB is ~48 MB against a
        // 32 MB cap, so eviction has to happen.
        let bulky = |n: i64| CachedEvent {
            id: format!("e{n}"),
            timestamp: n,
            detail: json!({ "blob": "x".repeat(400_000) }),
        };

        for i in 0..120i64 {
            cache
                .merge(&format!("type-{i}"), vec![bulky(i)], 0, 1_000, false)
                .await;
        }
        let (before, _) = cache.stats().await;

        cache.persist().await;
        let (after, _) = cache.stats().await;

        assert!(
            after < before,
            "expected eviction: {before} types before, {after} after"
        );
        let serialized = std::fs::read_to_string(_dir.path().join("event-samples.json")).unwrap();
        assert!(
            serialized.len() <= default_cache_bytes(),
            "cache file is {} bytes, over the {} budget",
            serialized.len(),
            default_cache_bytes()
        );
    }

    #[tokio::test]
    async fn a_cache_within_budget_is_left_alone() {
        let (cache, _dir) = temp_cache();
        cache.merge("k", vec![event("a", 1)], 0, 10, false).await;
        cache.persist().await;
        assert_eq!(cache.stats().await.0, 1);
    }

    #[tokio::test]
    async fn clearing_one_environment_leaves_the_others() {
        let (cache, _dir) = temp_cache();
        let prd = cache_key("prd", "lg", "s", "d");
        let dev = cache_key("dev", "lg", "s", "d");

        cache.merge(&prd, vec![event("a", 1)], 0, 10, false).await;
        cache.merge(&dev, vec![event("b", 1)], 0, 10, false).await;

        assert_eq!(cache.clear(Some("prd")).await, 1);
        assert!(cache.get(&prd).await.is_none());
        assert!(cache.get(&dev).await.is_some());
    }

    #[tokio::test]
    async fn survives_a_reload_from_disk() {
        let dir = tempdir::Dir::new();
        {
            let cache = EventCache::load(dir.path());
            cache.merge("k", vec![event("a", 100)], 0, 200, false).await;
            cache.persist().await;
        }

        let reloaded = EventCache::load(dir.path());
        let sample = reloaded.get("k").await.expect("sample should persist");
        assert_eq!(sample.events.len(), 1);
    }

    #[test]
    fn a_corrupt_cache_file_starts_empty_rather_than_failing() {
        let dir = tempdir::Dir::new();
        std::fs::write(dir.path().join("event-samples.json"), "{ not json").unwrap();
        let cache = EventCache::load(dir.path());
        assert!(futures::executor::block_on(cache.get("k")).is_none());
    }

    #[test]
    fn parses_an_envelope_into_a_cacheable_event() {
        let message = r#"{"source":"billing","detail-type":"update-client","detail":{"id":"x"}}"#;
        let (source, detail_type, event) = parse_event("id-1", 1234, message).unwrap();
        assert_eq!(source, "billing");
        assert_eq!(detail_type, "update-client");
        assert_eq!(event.detail["id"], "x");
        assert_eq!(event.timestamp, 1234);
    }

    #[test]
    fn skips_messages_that_are_not_events() {
        assert!(parse_event("i", 0, "not json").is_none());
        assert!(parse_event("i", 0, r#"{"source":"a"}"#).is_none());
    }
}
