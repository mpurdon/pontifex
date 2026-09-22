//! The poller: one background task per armed environment.
//!
//! Each pass reads the slice of the log group since the last pass (plus an
//! overlap, because CloudWatch ingests late), works out which watches each
//! returned event satisfied, records the hits, and tells the webview and the
//! desktop about them. Then it sleeps.
//!
//! Failure is expected to be routine over a working day — an SSO token
//! expires, the laptop sleeps, the network drops — so the loop never exits on
//! its own. An auth failure parks it at a slow cadence until credentials come
//! back, and a sign-in wakes it at once; anything else backs off
//! exponentially and keeps trying.

use super::pattern::{self, Compiled};
use super::{HitGrade, Watch, WatchHit, WatchMarkKind};
use crate::aws::clients::map_sdk_error;
use crate::error::{Error, Result};
use crate::events_cache::now_ms;
use crate::logging::cat;
use crate::schema::events::{self, EventCheckReport, IssueSeverity};
use crate::schema::model::{self, EventIdentity};
use crate::state::AppState;
use aws_config::SdkConfig;
use futures::future::join_all;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Notify, RwLock};

/// How far back past the cursor each pass re-reads. CloudWatch can ingest an
/// event well after its timestamp; anything later than this is missed.
const OVERLAP_MS: i64 = 90_000;

/// The most a pass will reach back after a sleep or restart. Beyond this the
/// gap is skipped and the status says so, rather than replaying an entire
/// night's traffic on wake.
const CATCH_UP_MAX_MS: i64 = 60 * 60 * 1000;

/// How far the first pass after arming looks back.
///
/// A watch on a bursty source can sit silent for an hour, and a screen that
/// stays empty says nothing about whether the watch works or the bus is quiet.
/// Filling in the last hour answers that on arrival. These hits are marked as
/// backfill and do not notify — they are context, not news.
const BACKFILL_MS: i64 = 60 * 60 * 1000;

/// Pages a single call may follow in one pass. An empty page still carries a
/// token, so a busy group without matches would otherwise be read to the end.
const MAX_PAGES: usize = 30;
const PAGE_LIMIT: i32 = 2_000;

/// Cadence while credentials are missing. A sign-in wakes the poller early,
/// so this only bounds how long a token fixed some other way goes unnoticed.
const AUTH_RETRY: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Above this many hits for one watch in one pass, a single summary
/// notification stands in for a pile of individual ones.
const NOTIFY_INDIVIDUALLY_UP_TO: usize = 3;

pub const EVENT_HITS: &str = "watch:hits";
pub const EVENT_STATUS: &str = "watch:status";

/// What the UI shows about one environment's poller.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchStatus {
    pub env_id: String,
    /// Persisted intent: the poller should be running.
    pub armed: bool,
    /// Live fact: the task exists.
    pub running: bool,
    pub poll_seconds: u64,
    /// Minutes of no one touching the app before it asks whether to keep
    /// watching; 0 never asks.
    pub idle_timeout_minutes: u64,
    pub last_poll_at: Option<i64>,
    pub next_poll_at: Option<i64>,
    /// The last pass failed with this. Cleared by the next success.
    pub last_error: Option<String>,
    /// Set while the poller is parked on an auth failure.
    pub paused: Option<String>,
    /// Something worth knowing that is not an error — a skipped gap, say.
    pub note: Option<String>,
    pub polls: u64,
    /// Events the last pass fetched, before local matching.
    pub last_fetched: usize,
    /// Events fetched since this poller started, before local matching.
    pub fetched_total: u64,
    /// CloudWatch calls each pass makes: one per log group of structured
    /// watches, plus one per raw-pattern watch.
    pub calls_per_pass: usize,
    /// How long the last pass spent, wall clock.
    pub last_pass_ms: u64,
    /// Watches that match every event, so their call has no server-side filter.
    pub unfiltered: usize,
    pub hits: usize,
    pub unread: usize,
    pub seen_at: i64,
    /// When this poller was started, epoch ms — the `at` of its start mark.
    /// Hits older than this came from the look-back; newer were caught live.
    pub watching_since: Option<i64>,
}

#[derive(Debug, Clone, Default)]
struct Live {
    last_poll_at: Option<i64>,
    next_poll_at: Option<i64>,
    last_error: Option<String>,
    paused: Option<String>,
    note: Option<String>,
    polls: u64,
    last_fetched: usize,
    fetched_total: u64,
    calls_per_pass: usize,
    last_pass_ms: u64,
    unfiltered: usize,
    started_at: Option<i64>,
}

/// A schema as the grader last saw it, or the fact that there is none.
#[derive(Clone)]
struct SchemaEntry {
    fetched_at: i64,
    /// The document and the type its events are checked against; `None`
    /// when the registry has no schema by that name.
    found: Option<(Value, String)>,
}

/// How long a fetched schema is trusted for grading. A save through this app
/// clears the cache outright; this bounds staleness from edits made elsewhere.
const SCHEMA_TTL_MS: i64 = 10 * 60 * 1000;

#[derive(Default)]
pub struct Watcher {
    tasks: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
    live: RwLock<HashMap<String, Live>>,
    /// Schemas fetched for grading, keyed by environment and name. Grading
    /// happens per hit, and a busy source would otherwise describe the same
    /// schema on every poll.
    schemas: tokio::sync::Mutex<HashMap<String, SchemaEntry>>,
    /// One per environment: poked by "poll now" to cut the sleep short.
    wake: Mutex<HashMap<String, Arc<Notify>>>,
    /// Start and stop take turns: two "start" clicks landing together must
    /// not each pass the running check and spawn a poller, orphaning one.
    lifecycle: tokio::sync::Mutex<()>,
}

impl Watcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget every cached schema, so the next hit grades against what is
    /// registered now. Called after a schema is written or deleted.
    pub async fn forget_schemas(&self) {
        self.schemas.lock().await.clear();
    }

    /// The schema a hit's events are graded against, fetched at most once per
    /// TTL. `Ok(None)` is a registry that has no such schema — cached too,
    /// because an unregistered type fires as often as a registered one.
    async fn schema_for_grading(
        &self,
        client: &aws_sdk_schemas::Client,
        env_id: &str,
        registry: &str,
        name: &str,
    ) -> Result<Option<(Value, String)>> {
        let key = format!("{env_id}\u{1f}{name}");
        let now = now_ms();
        if let Some(entry) = self.schemas.lock().await.get(&key) {
            if now - entry.fetched_at < SCHEMA_TTL_MS {
                return Ok(entry.found.clone());
            }
        }
        let found = match client
            .describe_schema()
            .registry_name(registry)
            .schema_name(name)
            .send()
            .await
            .map_err(map_sdk_error)
        {
            Ok(described) => {
                let content: Value = serde_json::from_str(described.content().unwrap_or("{}"))?;
                let type_name = model::detail_type_name(&content).ok_or_else(|| {
                    Error::Invalid(format!("{name}: the envelope's `detail` has no $ref"))
                })?;
                Some((content, type_name))
            }
            Err(Error::NotFound(_)) => None,
            Err(e) => return Err(e),
        };
        self.schemas.lock().await.insert(
            key,
            SchemaEntry {
                fetched_at: now,
                found: found.clone(),
            },
        );
        Ok(found)
    }

    fn is_running(&self, env_id: &str) -> bool {
        self.tasks
            .lock()
            .expect("watcher task table poisoned")
            .get(env_id)
            .is_some_and(|t| !t.inner().is_finished())
    }

    /// Start polling an environment. A no-op when it is already running.
    ///
    /// The start mark is the session's record: its time is what the status
    /// reports and what the hit list draws the rule at, so there is exactly
    /// one clock reading for "when this session began".
    pub async fn start(&self, app: &AppHandle, env_id: &str) {
        let _turn = self.lifecycle.lock().await;
        if self.is_running(env_id) {
            return;
        }
        let state = app.state::<AppState>();
        let started_at = match state.watch.push_mark(env_id, WatchMarkKind::Start).await {
            Ok(mark) => mark.at,
            Err(e) => {
                lwarn!(cat::WATCH, "could not record the start mark: {e}");
                now_ms()
            }
        };
        self.update(env_id, |l| l.started_at = Some(started_at))
            .await;
        let handle = tauri::async_runtime::spawn(run(app.clone(), env_id.to_string(), started_at));
        self.tasks
            .lock()
            .expect("watcher task table poisoned")
            .insert(env_id.to_string(), handle);
        linfo!(cat::WATCH, "watching {env_id}");
    }

    pub async fn stop(&self, app: &AppHandle, env_id: &str) {
        let _turn = self.lifecycle.lock().await;
        let handle = self
            .tasks
            .lock()
            .expect("watcher task table poisoned")
            .remove(env_id);
        if let Some(handle) = handle {
            handle.abort();
            let state = app.state::<AppState>();
            if let Err(e) = state.watch.push_mark(env_id, WatchMarkKind::Stop).await {
                lwarn!(cat::WATCH, "could not record the stop mark: {e}");
            }
            linfo!(cat::WATCH, "stopped watching {env_id}");
        }
        self.update(env_id, |l| {
            l.next_poll_at = None;
            l.started_at = None;
        })
        .await;
    }

    /// When the running session began, if one is running.
    pub async fn started_at(&self, env_id: &str) -> Option<i64> {
        self.live
            .read()
            .await
            .get(env_id)
            .and_then(|l| l.started_at)
    }

    pub async fn status(&self, state: &AppState, env_id: &str) -> WatchStatus {
        let live = self
            .live
            .read()
            .await
            .get(env_id)
            .cloned()
            .unwrap_or_default();
        let store = state.watch.snapshot(env_id).await;
        WatchStatus {
            env_id: env_id.to_string(),
            armed: store.armed,
            running: self.is_running(env_id),
            poll_seconds: store.poll_seconds,
            idle_timeout_minutes: store.idle_timeout_minutes,
            last_poll_at: live.last_poll_at,
            next_poll_at: live.next_poll_at,
            last_error: live.last_error,
            paused: live.paused,
            note: live.note,
            polls: live.polls,
            last_fetched: live.last_fetched,
            fetched_total: live.fetched_total,
            calls_per_pass: live.calls_per_pass,
            last_pass_ms: live.last_pass_ms,
            unfiltered: live.unfiltered,
            hits: store.hits,
            unread: store.unread,
            seen_at: store.seen_at,
            watching_since: live.started_at,
        }
    }

    /// Status for every environment that has watches, is armed, or has run.
    pub async fn status_all(&self, state: &AppState) -> Vec<WatchStatus> {
        let mut ids: Vec<String> = state
            .settings_snapshot()
            .await
            .environments
            .iter()
            .map(|e| e.id.clone())
            .collect();
        for id in self.live.read().await.keys() {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        join_all(ids.iter().map(|id| self.status(state, id))).await
    }

    /// The handle "poll now" pokes and the loop sleeps on.
    fn waker(&self, env_id: &str) -> Arc<Notify> {
        self.wake
            .lock()
            .expect("watcher wake table poisoned")
            .entry(env_id.to_string())
            .or_default()
            .clone()
    }

    /// Run the next pass immediately. Errors when nothing is running.
    pub fn poll_now(&self, env_id: &str) -> Result<()> {
        if !self.is_running(env_id) {
            return Err(Error::Invalid(format!("Not watching '{env_id}'")));
        }
        self.waker(env_id).notify_one();
        Ok(())
    }

    async fn update(&self, env_id: &str, f: impl FnOnce(&mut Live)) {
        let mut live = self.live.write().await;
        f(live.entry(env_id.to_string()).or_default());
    }
}

/// Push the current status to the webview and the unread total to the Dock,
/// and hand the status back for callers that answer with it.
pub async fn broadcast(app: &AppHandle, env_id: &str) -> WatchStatus {
    let state = app.state::<AppState>();
    let status = state.watcher.status(&state, env_id).await;
    if let Err(e) = app.emit(EVENT_STATUS, &status) {
        lwarn!(cat::WATCH, "could not emit status: {e}");
    }
    set_badge(app, state.watch.unread_total().await);
    status
}

pub fn set_badge(app: &AppHandle, unread: usize) {
    if let Some(window) = app.get_webview_window("main") {
        let count = (unread > 0).then_some(unread as i64);
        // Not every platform has a badge; a failure here is cosmetic.
        let _ = window.set_badge_count(count);
    }
}

#[derive(Default)]
struct Pass {
    fetched: usize,
    hits: usize,
    calls: usize,
    unfiltered: usize,
    elapsed_ms: u64,
    note: Option<String>,
}

/// The CloudWatch client for one environment, rebuilt only when the SDK
/// config it came from is replaced — after a sign-in, say. A client is a
/// connection pool; one per pass would mean a TLS handshake per pass.
struct Connection {
    cfg: Arc<SdkConfig>,
    client: aws_sdk_cloudwatchlogs::Client,
}

impl Connection {
    fn for_config(cfg: Arc<SdkConfig>) -> Self {
        let client = aws_sdk_cloudwatchlogs::Client::new(&cfg);
        Connection { cfg, client }
    }
}

async fn run(app: AppHandle, env_id: String, started_at: i64) {
    let state = app.state::<AppState>();
    let watcher = &state.watcher;
    let credentials_changed = state.clients.subscribe();
    let mut backoff: u32 = 0;
    let mut connection: Option<Connection> = None;
    // Event ids already turned into hits (or rejected), with their timestamps
    // so the set can be pruned as the cursor moves on.
    let mut seen: HashMap<String, i64> = HashMap::new();
    let mut first_pass = true;

    loop {
        let interval = Duration::from_secs(state.watch.poll_seconds().await);
        let watches: Vec<Watch> = state
            .watch
            .watches_for(&env_id)
            .await
            .into_iter()
            .filter(|w| w.enabled)
            .collect();

        let outcome = if watches.is_empty() {
            Ok(Pass {
                note: Some("No enabled watches".into()),
                ..Pass::default()
            })
        } else {
            let backfill = first_pass.then_some(started_at);
            pass(
                &app,
                &state,
                &env_id,
                &watches,
                &mut seen,
                &mut connection,
                backfill,
            )
            .await
        };
        // The look-back is spent only by a pass that had watches to look
        // for; arming an empty environment and adding a watch later still
        // gets its hour.
        if outcome.is_ok() && !watches.is_empty() {
            first_pass = false;
        }

        let now = now_ms();
        let delay = match outcome {
            Ok(pass) => {
                let was_paused = watcher
                    .live
                    .read()
                    .await
                    .get(&env_id)
                    .is_some_and(|l| l.paused.is_some());
                if was_paused {
                    linfo!(cat::WATCH, "{env_id}: credentials are back, resuming");
                }
                backoff = 0;
                watcher
                    .update(&env_id, |l| {
                        l.polls += 1;
                        l.last_poll_at = Some(now);
                        l.last_error = None;
                        l.paused = None;
                        l.note = pass.note;
                        l.last_fetched = pass.fetched;
                        l.fetched_total += pass.fetched as u64;
                        l.calls_per_pass = pass.calls;
                        l.last_pass_ms = pass.elapsed_ms;
                        l.unfiltered = pass.unfiltered;
                    })
                    .await;
                if pass.hits > 0 {
                    linfo!(
                        cat::WATCH,
                        "{env_id}: {} hit(s) from {} fetched",
                        pass.hits,
                        pass.fetched
                    );
                } else {
                    ldebug!(cat::WATCH, "{env_id}: no hits, {} fetched", pass.fetched);
                }
                interval
            }
            Err(Error::Auth(message)) => {
                let already = watcher
                    .live
                    .read()
                    .await
                    .get(&env_id)
                    .is_some_and(|l| l.paused.is_some());
                if !already {
                    lwarn!(
                        cat::WATCH,
                        "{env_id}: paused, credentials unavailable: {message}"
                    );
                    notify(
                        &app,
                        &format!("Pontifex paused watching {env_id}"),
                        "Sign in again to resume. Nothing is lost: polling picks up from where it stopped.",
                    )
                    .await;
                }
                watcher
                    .update(&env_id, |l| {
                        l.polls += 1;
                        l.last_poll_at = Some(now);
                        l.paused = Some("Sign in to resume".into());
                        l.last_error = Some(message);
                    })
                    .await;
                AUTH_RETRY
            }
            Err(e) => {
                backoff = backoff.saturating_add(1);
                let delay = (interval * 2u32.saturating_pow(backoff)).min(MAX_BACKOFF);
                let message = e.to_string();
                let repeat = watcher
                    .live
                    .read()
                    .await
                    .get(&env_id)
                    .is_some_and(|l| l.last_error.as_deref() == Some(message.as_str()));
                if !repeat {
                    lwarn!(
                        cat::WATCH,
                        "{env_id}: poll failed, retrying in {}s: {message}",
                        delay.as_secs()
                    );
                }
                watcher
                    .update(&env_id, |l| {
                        l.polls += 1;
                        l.last_poll_at = Some(now);
                        l.last_error = Some(message);
                    })
                    .await;
                delay
            }
        };

        watcher
            .update(&env_id, |l| {
                l.next_poll_at = Some(now_ms() + delay.as_millis() as i64)
            })
            .await;
        broadcast(&app, &env_id).await;
        let waker = watcher.waker(&env_id);
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = waker.notified() => {
                ldebug!(cat::WATCH, "{env_id}: polling now on request");
            }
            _ = credentials_changed.notified() => {
                // A sign-in, or any settings save: worth retrying now. The
                // paused state clears when the retry succeeds, not before —
                // clearing it here would re-arm the "paused" notification
                // for a retry that fails the same way.
                linfo!(cat::WATCH, "{env_id}: credentials changed, retrying now");
            }
        }
    }
}

/// One read of the log group(s) and everything that follows from it.
async fn pass(
    app: &AppHandle,
    state: &AppState,
    env_id: &str,
    watches: &[Watch],
    seen: &mut HashMap<String, i64>,
    connection: &mut Option<Connection>,
    // When set, this is the first pass since arming: look back `BACKFILL_MS`
    // and treat anything older than this instant as context rather than news.
    backfill_since: Option<i64>,
) -> Result<Pass> {
    let started = std::time::Instant::now();
    let (env, cfg) = state.env_config(Some(env_id)).await?;
    let stale = connection
        .as_ref()
        .is_none_or(|c| !Arc::ptr_eq(&c.cfg, &cfg));
    if stale {
        *connection = Some(Connection::for_config(cfg));
    }
    let client = connection.as_ref().expect("set above").client.clone();

    let now = now_ms();
    let cursor = state.watch.cursor(env_id).await;
    let (from, note) = match backfill_since {
        Some(_) => (
            now - BACKFILL_MS,
            Some(format!("Showing the last {}", describe_gap(BACKFILL_MS))),
        ),
        None => {
            let from = match cursor {
                Some(c) => (c - OVERLAP_MS).max(now - CATCH_UP_MAX_MS),
                None => now - OVERLAP_MS,
            };
            let note = cursor.filter(|c| now - c > CATCH_UP_MAX_MS).map(|c| {
                format!(
                    "Skipped {} of backlog after a pause",
                    describe_gap(now - c - CATCH_UP_MAX_MS)
                )
            });
            (from, note)
        }
    };

    let default_group = env
        .bus_log_group()
        .ok_or_else(|| Error::Invalid(format!("Environment '{}' has no log groups", env.label)))?;

    // One call per (group, structured set) and one per raw watch.
    let mut calls: Vec<(String, Option<String>, Vec<&Watch>)> = Vec::new();
    let mut structured: BTreeMap<String, Vec<&Watch>> = BTreeMap::new();
    for watch in watches {
        let group = watch
            .log_group
            .clone()
            .unwrap_or_else(|| default_group.to_string());
        match pattern::classify(watch)? {
            Compiled::Raw(raw) => calls.push((group, Some(raw), vec![watch])),
            Compiled::Structured(_) | Compiled::Everything => {
                structured.entry(group).or_default().push(watch)
            }
        }
    }
    for (group, members) in structured {
        let combined = pattern::compile_group(&members)?;
        calls.push((group, combined, members));
    }

    // The calls are independent, so they run together; a pass is as slow as
    // its slowest call rather than the sum of them.
    for (group, filter, _) in &calls {
        ldebug!(
            cat::WATCH,
            "{env_id}: polling {group} from {from} to {now} with {}",
            filter.as_deref().unwrap_or("no pattern")
        );
    }
    let results = join_all(
        calls
            .iter()
            .map(|(group, filter, _)| fetch(&client, group, filter.as_deref(), from, now)),
    )
    .await;

    // One call failing must not take the others down with it: a raw pattern
    // with a typo or a deleted log group is that watch's problem, named in
    // the status, while the rest keep matching. Credentials failing is
    // everyone's problem and parks the whole poller.
    let mut fetched_calls: Vec<Option<Fetched>> = Vec::with_capacity(calls.len());
    let mut failures: Vec<String> = Vec::new();
    for ((group, filter, _), result) in calls.iter().zip(results) {
        match result {
            Ok(fetched) => fetched_calls.push(Some(fetched)),
            Err(Error::Auth(message)) => return Err(Error::Auth(message)),
            Err(e) => {
                lwarn!(cat::WATCH, "{env_id}: call on {group} failed: {e}");
                failures.push(format!(
                    "{group} with {}: {e}",
                    filter.as_deref().unwrap_or("no pattern")
                ));
                fetched_calls.push(None);
            }
        }
    }
    if !failures.is_empty() && failures.len() == calls.len() {
        return Err(Error::Aws(failures.join("; ")));
    }

    let mut fetched = 0;
    let mut hits: Vec<WatchHit> = Vec::new();
    // Ids first seen in this pass. Checked against earlier passes only: the
    // same event can come back from two calls on one group — a raw watch
    // and the structured set, say — and each call's watches must get their
    // look at it. The store de-duplicates per (watch, event) after.
    let mut seen_now: HashMap<String, i64> = HashMap::new();
    let mut truncated_at: Option<i64> = None;
    for ((group, _, candidates), fetched_call) in calls.iter().zip(fetched_calls) {
        let Some(Fetched { events, truncated }) = fetched_call else {
            continue;
        };
        fetched += events.len();
        if truncated {
            // Nothing past the last page was read; the cursor can only move
            // as far as this call actually got.
            let last = events.iter().map(|(_, ts, _)| *ts).max().unwrap_or(from);
            truncated_at = Some(truncated_at.map_or(last, |t: i64| t.min(last)));
        }
        for (id, timestamp, message) in events {
            if seen.contains_key(&id) {
                continue;
            }
            seen_now.insert(id.clone(), timestamp);
            let Ok(envelope) = serde_json::from_str::<Value>(&message) else {
                continue;
            };
            let text = |key: &str| {
                envelope
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            for watch in candidates {
                if !pattern::matches(watch, &envelope) {
                    continue;
                }
                hits.push(WatchHit {
                    id: uuid::Uuid::new_v4().to_string(),
                    watch_id: watch.id.clone(),
                    watch_label: watch.label.clone(),
                    env_id: env_id.to_string(),
                    log_group: group.clone(),
                    event_id: text("id").unwrap_or_else(|| id.clone()),
                    timestamp,
                    received_at: now,
                    source: text("source"),
                    detail_type: text("detail-type"),
                    event: envelope.clone(),
                    backfill: backfill_since.is_some_and(|since| timestamp < since),
                    grade: None,
                    headline: None,
                });
            }
        }
    }
    seen.extend(seen_now);

    // Where the next pass picks up. Every call complete: now. A call cut
    // short by the page cap: where it got to, so the rest is read next time.
    // A call that failed: nowhere — the window is re-read (bounded by the
    // catch-up cap) until the watch is fixed or disabled.
    let mut note = note;
    if failures.is_empty() {
        let cursor_to = truncated_at.unwrap_or(now);
        if truncated_at.is_some() {
            note = Some(format!(
                "A read hit the {MAX_PAGES}-page cap; the next pass continues from {} ago",
                describe_gap(now - cursor_to)
            ));
        }
        state.watch.set_cursor(env_id, cursor_to).await;
    } else {
        note = Some(format!(
            "Call failed, window will be re-read: {}",
            failures.join("; ")
        ));
    }
    // Ids older than the reach of the next overlap can never be re-fetched.
    seen.retain(|_, ts| *ts >= from - OVERLAP_MS);

    hits.sort_by_key(|h| std::cmp::Reverse(h.timestamp));
    let cfg = &connection.as_ref().expect("set above").cfg;
    grade_hits(state, cfg, env_id, &env.registry_name, &mut hits).await;
    let fresh = state.watch.push_hits(hits).await?;
    if !fresh.is_empty() {
        if let Err(e) = app.emit(EVENT_HITS, &fresh) {
            lwarn!(cat::WATCH, "could not emit hits: {e}");
        }
        let news: Vec<&WatchHit> = fresh.iter().filter(|h| !h.backfill).collect();
        announce(app, watches, &news).await;
    }

    Ok(Pass {
        fetched,
        hits: fresh.len(),
        calls: calls.len(),
        unfiltered: calls
            .iter()
            .filter(|(_, filter, _)| filter.is_none())
            .count(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        note,
    })
}

/// Grade every hit against its registered schema, in place.
///
/// The schema is looked up under the names the registry might hold it as:
/// the exact `source@detail-type`, then that name with the characters a
/// registry name cannot carry replaced, then the PascalCase title EventBridge's
/// own codegen would have used. A registry that has none of them is `Missing`.
/// Nothing here fails the pass — a hit that cannot be graded is `Unknown`, and
/// the reason is logged.
async fn grade_hits(
    state: &AppState,
    cfg: &SdkConfig,
    env_id: &str,
    registry: &str,
    hits: &mut [WatchHit],
) {
    if hits.is_empty() {
        return;
    }
    let client = aws_sdk_schemas::Client::new(cfg);
    for hit in hits.iter_mut() {
        let (grade, headline) = grade_hit(state, &client, env_id, registry, hit).await;
        hit.grade = Some(grade);
        hit.headline = headline;
    }
}

async fn grade_hit(
    state: &AppState,
    client: &aws_sdk_schemas::Client,
    env_id: &str,
    registry: &str,
    hit: &WatchHit,
) -> (HitGrade, Option<String>) {
    let (Some(source), Some(detail_type)) = (&hit.source, &hit.detail_type) else {
        return (HitGrade::Unknown, Some("The event carries no source or detail-type".into()));
    };
    let Some(detail) = hit.event.get("detail") else {
        return (HitGrade::Unknown, Some("The event has no `detail` to check".into()));
    };
    let identity = EventIdentity {
        source: source.clone(),
        detail_type: detail_type.clone(),
    };
    let exact = identity.schema_name();
    let mut candidates = vec![exact.clone()];
    let sanitized = model::sanitize_schema_name(&exact);
    if sanitized != exact {
        candidates.push(sanitized);
    }
    candidates.push(format!("{}@{}", identity.source, identity.detail_title()));

    for name in candidates {
        match state
            .watcher
            .schema_for_grading(client, env_id, registry, &name)
            .await
        {
            Ok(Some((document, type_name))) => {
                return match events::check_events(&document, &type_name, std::slice::from_ref(detail)) {
                    Ok(report) => grade_for(&report),
                    Err(e) => (HitGrade::Unknown, Some(e.to_string())),
                };
            }
            Ok(None) => continue,
            Err(e) => {
                lwarn!(cat::WATCH, "could not grade {exact}: {e}");
                return (HitGrade::Unknown, Some(e.to_string()));
            }
        }
    }
    (HitGrade::Missing, Some("No schema is registered for this event type".into()))
}

/// The grade a single event's report earns, and the line worth showing for it.
fn grade_for(report: &EventCheckReport) -> (HitGrade, Option<String>) {
    // The issues are already ranked, so the first one that matters is the
    // headline — the same choice the Health report makes.
    let headline = |worth: fn(&events::Issue) -> bool| {
        report.issues.iter().find(|i| worth(i)).map(|i| i.summary.clone())
    };
    if report.failed > 0 {
        (HitGrade::Failing, headline(|i| i.rejects).or_else(|| headline(|_| true)))
    } else if report.issues.iter().any(|i| i.severity != IssueSeverity::Info) {
        (
            HitGrade::Drifting,
            headline(|i| i.severity != IssueSeverity::Info),
        )
    } else {
        (HitGrade::Ok, None)
    }
}

/// What one call read, and whether it got to the end of the window.
struct Fetched {
    events: Vec<(String, i64, String)>,
    truncated: bool,
}

/// Read everything in `[from, to)` matching `filter`, following pagination
/// up to the page cap.
async fn fetch(
    client: &aws_sdk_cloudwatchlogs::Client,
    group: &str,
    filter: Option<&str>,
    from: i64,
    to: i64,
) -> Result<Fetched> {
    let mut req = client
        .filter_log_events()
        .log_group_name(group)
        .start_time(from)
        .end_time(to)
        .limit(PAGE_LIMIT);
    if let Some(pattern) = filter {
        req = req.filter_pattern(pattern);
    }

    let mut out = Vec::new();
    let mut token: Option<String> = None;
    let mut truncated = false;
    for page in 0..MAX_PAGES {
        let response = req
            .clone()
            .set_next_token(token.take())
            .send()
            .await
            .map_err(map_sdk_error)?;
        for e in response.events() {
            let (Some(id), Some(ts), Some(message)) = (e.event_id(), e.timestamp(), e.message())
            else {
                continue;
            };
            out.push((id.to_string(), ts, message.to_string()));
        }
        token = response.next_token().map(str::to_string);
        if token.is_none() {
            break;
        }
        if page + 1 == MAX_PAGES {
            lwarn!(cat::WATCH, "{group}: stopped after {MAX_PAGES} pages; the next pass continues from the last event read");
            truncated = true;
        }
    }
    Ok(Fetched {
        events: out,
        truncated,
    })
}

/// Desktop notifications for a batch of fresh hits, grouped per watch.
///
/// Each notification is a helper launch of a hundred milliseconds or more,
/// so they go out together rather than one after another.
async fn announce(app: &AppHandle, watches: &[Watch], fresh: &[&WatchHit]) {
    let mut by_watch: BTreeMap<&str, Vec<&WatchHit>> = BTreeMap::new();
    for hit in fresh {
        by_watch.entry(hit.watch_id.as_str()).or_default().push(hit);
    }
    let mut messages: Vec<(String, String)> = Vec::new();
    for (watch_id, hits) in by_watch {
        let Some(watch) = watches.iter().find(|w| w.id == watch_id) else {
            continue;
        };
        if !watch.notify {
            continue;
        }
        if hits.len() <= NOTIFY_INDIVIDUALLY_UP_TO {
            for hit in hits {
                messages.push((watch.label.clone(), describe_hit(watch, hit)));
            }
        } else {
            let latest = hits
                .first()
                .map(|h| describe_hit(watch, h))
                .unwrap_or_default();
            messages.push((
                watch.label.clone(),
                format!("{} events matched · latest: {latest}", hits.len()),
            ));
        }
    }
    join_all(
        messages
            .iter()
            .map(|(title, body)| notify(app, title, body)),
    )
    .await;
}

/// The notification body: what the event was, plus the payload fields the
/// watch was keyed on, so a `clientId` watch shows the id without opening
/// the app.
fn describe_hit(watch: &Watch, hit: &WatchHit) -> String {
    let mut text = match (&hit.source, &hit.detail_type) {
        (Some(s), Some(d)) => format!("{s} · {d}"),
        (Some(s), None) => s.clone(),
        (None, Some(d)) => d.clone(),
        (None, None) => hit.event_id.clone(),
    };
    for condition in &watch.conditions {
        let path = condition.path.trim();
        if path.is_empty() {
            continue;
        }
        if let Some(value) = pattern::lookup(&hit.event, path) {
            let shown = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let name = path.rsplit('.').next().unwrap_or(path);
            text.push_str(&format!(" · {name}={shown}"));
        }
    }
    text
}

async fn notify(app: &AppHandle, title: &str, body: &str) {
    if let Err(e) = super::notify::send(app, title, body).await {
        lwarn!(cat::WATCH, "notification failed: {e}");
    }
}

fn describe_gap(ms: i64) -> String {
    let minutes = ms / 60_000;
    if minutes < 60 {
        format!("{minutes}m")
    } else {
        format!("{}h {}m", minutes / 60, minutes % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch::{WatchCondition, WatchOp};
    use serde_json::json;

    #[test]
    fn grades_a_report_the_way_health_does() {
        use crate::schema::events::check_events;
        use serde_json::json;
        let doc = json!({ "components": { "schemas": { "T": {
            "type": "object",
            "required": ["id"],
            "properties": { "id": { "type": "string" }, "n": { "type": "integer" } }
        }}}});
        let ok = check_events(&doc, "T", &[json!({ "id": "a", "n": 1 })]).unwrap();
        assert_eq!(grade_for(&ok), (HitGrade::Ok, None));

        let failing = check_events(&doc, "T", &[json!({ "n": 1 })]).unwrap();
        let (grade, headline) = grade_for(&failing);
        assert_eq!(grade, HitGrade::Failing);
        assert!(headline.unwrap().contains("id"));

        // Blank required: accepted by the schema, so drifting rather than failing.
        let drifting = check_events(&doc, "T", &[json!({ "id": "" })]).unwrap();
        let (grade, headline) = grade_for(&drifting);
        assert_eq!(grade, HitGrade::Drifting);
        assert!(headline.unwrap().contains("blank"));
    }

    fn hit(event: Value) -> WatchHit {
        WatchHit {
            id: "h".into(),
            watch_id: "w".into(),
            watch_label: "w".into(),
            env_id: "dev".into(),
            log_group: "g".into(),
            event_id: "e".into(),
            timestamp: 0,
            received_at: 0,
            source: event
                .get("source")
                .and_then(Value::as_str)
                .map(str::to_string),
            detail_type: event
                .get("detail-type")
                .and_then(Value::as_str)
                .map(str::to_string),
            event,
            backfill: false,
            grade: None,
            headline: None,
        }
    }

    fn watch(conditions: Vec<WatchCondition>) -> Watch {
        Watch {
            conditions,
            ..Watch::blank("dev")
        }
    }

    #[test]
    fn the_notification_body_names_the_event_and_the_fields_it_was_keyed_on() {
        let event = json!({
            "source": "milo-disability",
            "detail-type": "create-client",
            "detail": {"clientId": "abc-123", "count": 5}
        });
        let w = watch(vec![
            WatchCondition {
                path: "detail.clientId".into(),
                op: WatchOp::Eq,
                value: "*".into(),
            },
            WatchCondition {
                path: "detail.count".into(),
                op: WatchOp::Ne,
                value: "0".into(),
            },
            WatchCondition {
                path: "detail.missing".into(),
                op: WatchOp::Eq,
                value: "x".into(),
            },
        ]);
        assert_eq!(
            describe_hit(&w, &hit(event)),
            "milo-disability · create-client · clientId=abc-123 · count=5"
        );
    }

    #[test]
    fn a_watch_without_conditions_describes_only_the_event() {
        let event = json!({"source": "s", "detail-type": "d", "detail": {}});
        assert_eq!(describe_hit(&watch(vec![]), &hit(event)), "s · d");
    }

    #[test]
    fn gaps_read_as_minutes_or_hours() {
        assert_eq!(describe_gap(5 * 60_000), "5m");
        assert_eq!(describe_gap(125 * 60_000), "2h 5m");
    }
}
