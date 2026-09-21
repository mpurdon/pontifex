//! Checking a registered schema against the events actually on the bus.

use crate::aws::clients::map_sdk_error;
use crate::aws::log_scan;
use crate::commands::origin;
use crate::error::{Error, Result};
use crate::events_cache::{cache_key, parse_event, CachedEvent};
use crate::logging::cat;
use crate::schema::events::{self, EventCheckReport, FieldObservation};
use crate::schema::model::{self, EventIdentity};
use crate::schema::repair::{self, Repair};
use crate::settings::Environment;
use crate::state::AppState;
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use tauri::{AppHandle, Emitter, State};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RealityCheckRequest {
    /// Registry schema name, `source@detail-type`.
    pub name: String,
    /// The document to check. Sent from the frontend so an *unsaved* draft can
    /// be checked against real traffic before it is registered.
    pub content: Value,
    /// Type within the document to validate event details against. Defaults to
    /// whatever the envelope's `detail` refs.
    pub type_name: Option<String>,
    /// How far back to sample.
    pub minutes: Option<i64>,
    pub limit: Option<i32>,
    /// Log group to sample from; defaults to the environment's first.
    pub log_group: Option<String>,
    /// Force a fetch even when the cache already covers the window.
    #[serde(default)]
    pub refresh: bool,
    /// Never touch AWS — validate against whatever is cached.
    ///
    /// This is what lets the panel re-check a draft on every edit.
    #[serde(default)]
    pub cached_only: bool,
    /// Wall-clock ceiling on the CloudWatch scan, in seconds.
    pub max_seconds: Option<u64>,
    /// Keep the result as the schema's last analysis, shown when the schema
    /// is opened again. Set by an explicit Run, not by live re-checks of a
    /// draft as it is typed.
    #[serde(default)]
    pub persist: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RealityCheckResult {
    #[serde(flatten)]
    pub report: EventCheckReport,
    pub log_group: String,
    pub source: String,
    pub detail_type: String,
    pub minutes: i64,
    /// Set when the sample was empty, explaining why rather than showing a
    /// misleadingly clean report.
    pub note: Option<String>,
    /// True when the events came from the cache rather than a fresh fetch.
    pub from_cache: bool,
    /// Age of the cached sample in milliseconds, when cached.
    pub cache_age_ms: Option<i64>,
}

/// The type an event's `detail` should be validated against.
fn detail_type_name(document: &Value, override_name: Option<&str>) -> Result<String> {
    if let Some(name) = override_name.filter(|n| !n.trim().is_empty()) {
        return Ok(name.to_string());
    }

    model::detail_type_name(document).ok_or_else(|| {
        Error::Invalid(
            "Could not determine which type to validate against — the envelope's `detail` has no $ref"
                .into(),
        )
    })
}

/// A registered schema as the report graded it: the document, the type its
/// events are checked against, and which version that was.
struct FetchedSchema {
    content: Value,
    type_name: String,
    version: Option<String>,
    /// As AWS formats it — the same string the schema list carries, so the
    /// two can be compared for equality rather than parsed.
    last_modified: Option<String>,
}

/// A registered schema's document and the type its events are checked
/// against, with the version stamp it was read at. A registry without the
/// schema is `Error::NotFound`, which some callers treat as a finding rather
/// than a failure.
async fn fetch_schema_versioned(
    schemas: &aws_sdk_schemas::Client,
    registry: &str,
    name: &str,
) -> Result<FetchedSchema> {
    let described = schemas
        .describe_schema()
        .registry_name(registry)
        .schema_name(name)
        .send()
        .await
        .map_err(map_sdk_error)?;
    let content: Value = serde_json::from_str(described.content().unwrap_or("{}"))?;
    let type_name = detail_type_name(&content, None)?;
    Ok(FetchedSchema {
        content,
        type_name,
        version: described.schema_version().map(str::to_string),
        last_modified: described
            .last_modified()
            .and_then(|d| d.fmt(aws_smithy_types::date_time::Format::DateTime).ok()),
    })
}

/// [`fetch_schema_versioned`] for callers that only want the document.
async fn fetch_schema(
    schemas: &aws_sdk_schemas::Client,
    registry: &str,
    name: &str,
) -> Result<(Value, String)> {
    let fetched = fetch_schema_versioned(schemas, registry, name).await?;
    Ok((fetched.content, fetched.type_name))
}

/// Every log group a scan should read, or just the one asked for.
///
/// An environment normally carries two — `{stage}-global-events` and
/// `{stage}-external-events` — because events reaching the bus over the bridge
/// land in the second. Reading only the first made every report a statement
/// about half the traffic: an external event type with no schema was invisible
/// rather than listed as unregistered, so "0 missing" was not the claim it
/// looked like.
fn resolve_log_groups(env: &Environment, requested: Option<String>) -> Result<Vec<String>> {
    if let Some(one) = requested.filter(|g| !g.trim().is_empty()) {
        return Ok(vec![one]);
    }
    let groups: Vec<String> = env
        .log_groups
        .iter()
        .filter(|g| !g.trim().is_empty())
        .cloned()
        .collect();
    if groups.is_empty() {
        return Err(Error::Invalid(format!(
            "Environment '{}' has no log groups configured",
            env.label
        )));
    }
    Ok(groups)
}

/// Bucket keys indexed by the schema name they would have to be registered
/// under, for the ones whose event source is not itself a legal schema name.
///
/// A source like `Atomic Forms` cannot be a schema name — EventBridge allows
/// only letters, digits, `_ . -` and `@` — so its schema is registered as
/// `Atomic-Forms@…`. The bucket, built from the event's own source, is still
/// keyed with the space, so a literal comparison never matches.
fn sanitized_bucket_index(keys: &[String]) -> BTreeMap<String, String> {
    let mut index = BTreeMap::new();
    for key in keys {
        let sanitized = model::sanitize_schema_name(key);
        if &sanitized != key {
            // First writer wins: if both `Atomic Forms@X` and `Atomic-Forms@X`
            // are on the bus, the exact match below claims the latter anyway.
            index.entry(sanitized).or_insert_with(|| key.clone());
        }
    }
    index
}

/// The bucket of events a registered schema should be graded against.
///
/// Three spellings, because a schema name is not always a byte-for-byte copy
/// of the identity its events carry:
///  1. the name as registered,
///  2. the PascalCase detail type discovered schemas are registered under,
///  3. the name a source containing illegal characters was sanitized into.
fn match_bucket(
    identity: &EventIdentity,
    buckets: &BTreeMap<String, Vec<Value>>,
    sanitized: &BTreeMap<String, String>,
) -> Option<String> {
    let exact = identity.schema_name();
    if buckets.contains_key(&exact) {
        return Some(exact);
    }

    let pascal = format!("{}@{}", identity.source, identity.detail_title());
    if buckets.contains_key(&pascal) {
        return Some(pascal);
    }

    sanitized
        .get(&exact)
        .or_else(|| sanitized.get(&pascal))
        .cloned()
}

/// Sample recent events for a schema and report how well it describes them.
///
/// Validation answers "would these events be rejected". The drift half answers
/// the more useful question — where the schema and reality have diverged — by
/// diffing the fields present in real traffic against what the schema declares.
#[tauri::command]
pub async fn check_against_events(
    state: State<'_, AppState>,
    request: RealityCheckRequest,
    env_id: Option<String>,
) -> Result<RealityCheckResult> {
    let persist = request.persist;
    let name = request.name.clone();
    let result = run_check(&state, request, env_id.as_deref()).await?;
    if persist {
        let env = state.resolve_environment(env_id.as_deref()).await?;
        state.analyses.put(&env.id, &name, result.clone()).await;
    }
    Ok(result)
}

/// The schema's last persisted analysis for this environment, if one was run
/// in the last month.
#[tauri::command]
pub async fn cached_analysis(
    state: State<'_, AppState>,
    name: String,
    env_id: Option<String>,
) -> Result<Option<crate::analysis_cache::CachedAnalysis>> {
    let env = state.resolve_environment(env_id.as_deref()).await?;
    let Some(mut cached) = state.analyses.get(&env.id, &name).await else {
        return Ok(None);
    };
    // Graded against the lookup as it is now, not as it was when the
    // analysis ran: the consumers may have been found since.
    let identity = EventIdentity::from_schema_name(&name)?;
    origin::grade_by_consumers(&state, &identity, &mut cached.result.report.issues).await;
    Ok(Some(cached))
}

/// `check_events`, then the consumer grade. The one way a report is made,
/// so no screen shows a validator-only grade beside a consumer-graded one.
async fn graded_check(
    state: &AppState,
    identity: &EventIdentity,
    document: &Value,
    type_name: &str,
    payloads: &[Value],
) -> Result<EventCheckReport> {
    let mut report = events::check_events(document, type_name, payloads)?;
    origin::grade_by_consumers(state, identity, &mut report.issues).await;
    Ok(report)
}

async fn run_check(
    state: &AppState,
    request: RealityCheckRequest,
    env_id: Option<&str>,
) -> Result<RealityCheckResult> {
    let identity = EventIdentity::from_schema_name(&request.name)?;
    let document = model::parse_content(&request.content)?;
    let type_name = detail_type_name(&document, request.type_name.as_deref())?;

    let (env, cfg) = state.env_config(env_id).await?;
    // Every configured group, because a schema's events may arrive over the
    // bridge rather than directly, and checking only the first reported an
    // external event type as having no traffic at all.
    let log_groups = resolve_log_groups(&env, request.log_group.clone())?;

    let minutes = request.minutes.unwrap_or(60 * 24).max(1);
    let limit = request.limit.unwrap_or(200).clamp(1, 10_000);
    // Settings → Scanning owns the default, same as the registry report.
    let budget_seconds = state
        .settings_snapshot()
        .await
        .scan
        .seconds_or(request.max_seconds);
    let start_time = (time::OffsetDateTime::now_utc() - time::Duration::minutes(minutes))
        .unix_timestamp()
        * 1000;

    let now = crate::events_cache::now_ms();

    // Serve from cache when it already covers the window. The whole point is
    // that re-checking a draft mid-edit costs nothing.
    let cached = state
        .events
        .sample_across(
            &env.id,
            &log_groups,
            &identity.source,
            &identity.detail_type,
            start_time,
            now,
        )
        .await;

    let log_group = cached
        .found_in
        .clone()
        .or_else(|| log_groups.first().cloned())
        .unwrap_or_default();

    if cached.age_ms.is_some() && (request.cached_only || (!request.refresh && cached.covered)) {
        let report =
            graded_check(state, &identity, &document, &type_name, &cached.payloads).await?;
        let note = if cached.payloads.is_empty() {
            Some(format!(
                "Nothing cached for {} in this window. Run a fresh check to fetch it.",
                identity.schema_name()
            ))
        } else {
            None
        };
        return Ok(RealityCheckResult {
            report,
            log_group,
            source: identity.source,
            detail_type: identity.detail_type,
            minutes,
            note,
            from_cache: true,
            cache_age_ms: cached.age_ms,
        });
    }

    if request.cached_only {
        // Asked not to hit AWS and there is nothing cached: say so rather than
        // reporting a clean bill of health from zero events.
        let report = graded_check(state, &identity, &document, &type_name, &[]).await?;
        return Ok(RealityCheckResult {
            report,
            log_group,
            source: identity.source,
            detail_type: identity.detail_type,
            minutes,
            note: Some("No cached events yet — run a check to fetch them.".into()),
            from_cache: true,
            cache_age_ms: None,
        });
    }

    let client = aws_sdk_cloudwatchlogs::Client::new(&cfg);
    let scan = crate::logging::Timed::start(
        cat::EVENTS,
        format!(
            "scanning {} log group(s) for {} over {minutes}m",
            log_groups.len(),
            identity.schema_name(),
        ),
    );

    let mut payloads: Vec<Value> = Vec::new();
    let mut scan_capped = false;
    let mut scan_note: Option<String> = None;
    let mut scanned_group: Option<String> = None;

    for group in &log_groups {
        // Striped, same as the registry report: a rare event type spread thinly
        // across a 24h window is exactly what a linear scan misses.
        let outcome = log_scan::scan_striped(
            &client,
            log_scan::ScanRequest {
                log_group: group.clone(),
                pattern: Some(super::logs::identity_pattern(
                    &identity.source,
                    &identity.detail_type,
                )),
                start_time,
                end_time: now,
                max_events: limit as usize,
                // The full budget each, not a share of it: this loop stops at
                // the first group that has the events, so the second scan only
                // happens when the first found nothing.
                budget: std::time::Duration::from_secs(budget_seconds),
                stripes: log_scan::DEFAULT_STRIPES,
            },
            |_| {},
        )
        .await?;

        scan_capped |= outcome.truncated;
        scan_note = scan_note.or(outcome.note.clone());
        if !outcome.events.is_empty() && scanned_group.is_none() {
            scanned_group = Some(group.clone());
        }

        let mut fetched: Vec<CachedEvent> = Vec::with_capacity(outcome.events.len());
        for (_, _, cached) in outcome.events {
            // Only the detail is validated; the envelope is AWS's, not ours.
            payloads.push(cached.detail.clone());
            fetched.push(cached);
        }

        // Cached per group, so a later cache-only re-check finds it wherever
        // it actually came from.
        let key = cache_key(&env.id, group, &identity.source, &identity.detail_type);
        state
            .events
            .merge(&key, fetched, start_time, now, scan_capped)
            .await;

        // An event type lives in one group in practice; stop once it is found
        // rather than spending the rest of the budget proving the other empty.
        if payloads.len() >= limit as usize {
            break;
        }
    }

    let log_group = scanned_group.unwrap_or(log_group);
    state.events.persist().await;

    scan.done(format!(
        "{} events{}",
        payloads.len(),
        if scan_capped { ", capped" } else { "" }
    ));

    let checking = crate::logging::Timed::start(
        cat::SCHEMA,
        format!(
            "checking {} against {} events",
            identity.schema_name(),
            payloads.len()
        ),
    );
    let report = graded_check(state, &identity, &document, &type_name, &payloads).await?;
    checking.done(format!(
        "{} passed, {} failed, {} undeclared",
        report.passed,
        report.failed,
        report.drift.undeclared.len()
    ));

    let note = if payloads.is_empty() {
        Some(format!(
            "No events matching {} were found in {log_group} over the last {minutes} minutes. \
             Widen the window, or check that this event is actually being published.",
            identity.schema_name()
        ))
    } else if scan_capped {
        scan_note
    } else {
        None
    };

    Ok(RealityCheckResult {
        report,
        log_group,
        source: identity.source,
        detail_type: identity.detail_type,
        minutes,
        note,
        from_cache: false,
        cache_age_ms: None,
    })
}

/// Cached-event totals, for the settings screen.
#[tauri::command]
pub async fn event_cache_stats(state: State<'_, AppState>) -> Result<(usize, usize)> {
    Ok(state.events.stats().await)
}

/// Drop cached samples, for the current environment or all of them.
#[tauri::command]
pub async fn clear_event_cache(
    state: State<'_, AppState>,
    env_id: Option<String>,
    all: Option<bool>,
) -> Result<usize> {
    if all.unwrap_or(false) {
        return Ok(state.events.clear(None).await);
    }
    let env = state.resolve_environment(env_id.as_deref()).await?;
    Ok(state.events.clear(Some(&env.id)).await)
}

// ---------------------------------------------------------------------------
// Registry-wide health report
// ---------------------------------------------------------------------------

/// One schema's standing against real traffic.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportRow {
    pub name: String,
    pub source: String,
    pub detail_type: String,
    pub sampled: usize,
    /// Every matching event seen in the window, not just the graded sample.
    ///
    /// `sampled` stops at the per-type bucket cap, so it says nothing about
    /// volume — fifty is fifty whether the type fired 50 times or 50,000.
    pub observed: usize,
    pub passed: usize,
    pub failed: usize,
    pub undeclared: usize,
    pub type_mismatches: usize,
    pub enum_drift: usize,
    pub missing_required: usize,
    pub status: RowStatus,
    /// The most useful single line about this row.
    pub headline: Option<String>,
    /// The identity the events themselves carry, when it differs from the name
    /// the schema is registered under.
    ///
    /// A source may hold characters a registry name cannot — a space, say — so
    /// `Atomic Forms` is registered as `Atomic-Forms`. Anything that derives
    /// the schema name from an event's own `source` is then looking for a name
    /// that does not exist, which is why this is worth reporting rather than
    /// quietly matching through.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire_identity: Option<String>,
    /// The schema version this row graded, so the screen can tell when the
    /// schema has been saved since — that row is dealt with, without a re-run.
    pub version: Option<String>,
    /// When that version was written, as the schema list also reports it.
    pub last_modified: Option<String>,
}

/// How a schema is faring against real traffic.
///
/// An enum rather than a string so the sort order below has to handle every
/// variant: a bare `&str` let a renamed status fall into a catch-all and sort
/// last with no error anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RowStatus {
    /// Real events are being rejected by the schema.
    Failing,
    /// The schema could not be read or graded.
    Error,
    /// Events pass, but the schema and traffic have diverged.
    Drifting,
    Ok,
    /// Nothing on the bus for this schema in the window.
    #[default]
    NoTraffic,
}

impl RowStatus {
    /// Sort key: worst first. Exhaustive by construction.
    fn rank(self) -> u8 {
        match self {
            RowStatus::Failing => 0,
            RowStatus::Error => 1,
            RowStatus::Drifting => 2,
            RowStatus::Ok => 3,
            RowStatus::NoTraffic => 4,
        }
    }
}

/// An event type on the bus with no schema registered for it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnregisteredEvent {
    pub source: String,
    pub detail_type: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryReport {
    pub rows: Vec<ReportRow>,
    pub unregistered: Vec<UnregisteredEvent>,
    pub scanned_events: usize,
    pub minutes: i64,
    /// Every log group this report read. More than one for an environment that
    /// takes events over the bridge as well as directly.
    pub log_groups: Vec<String>,
    pub registry: String,
    /// True when any time slice stopped early, so absence of traffic is not proof.
    pub truncated: bool,
    /// Why the scan stopped early, when it did — a deadline and an event cap
    /// call for different remedies.
    pub scan_note: Option<String>,
    /// How the window was sampled, so the coverage claim is checkable.
    pub stripes: usize,
    pub scan_ms: u128,
    /// When the scan finished, epoch millis — so a report kept on screen can
    /// say how old it is relative to the window it covers.
    pub generated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryReportRequest {
    pub minutes: Option<i64>,
    pub log_group: Option<String>,
    /// Cap on events scanned. Higher gives better coverage of rare events.
    pub max_events: Option<usize>,
    /// Wall-clock ceiling on the CloudWatch scan, in seconds.
    pub max_seconds: Option<u64>,
}

/// Grade every schema in the registry against recent traffic.
///
/// Scans the log group **once** and buckets events by `source@detail-type`,
/// rather than querying per schema: a registry of 300+ schemas would otherwise
/// mean 300+ CloudWatch calls, most of them finding nothing. The single pass
/// also surfaces something per-schema checks structurally cannot — event types
/// flowing on the bus with no schema registered at all.
#[tauri::command]
pub async fn registry_report(
    app: AppHandle,
    state: State<'_, AppState>,
    request: RegistryReportRequest,
    env_id: Option<String>,
) -> Result<RegistryReport> {
    let (env, cfg) = state.env_config(env_id.as_deref()).await?;
    let log_groups = resolve_log_groups(&env, request.log_group)?;

    let minutes = request.minutes.unwrap_or(60 * 24).max(1);
    // Defaults come from Settings → Scanning, so the limits are configured in
    // one place rather than passed from whichever screen happened to call.
    let scan_settings = state.settings_snapshot().await.scan;
    let max_events = scan_settings.events_or(request.max_events);
    let budget_seconds = scan_settings.seconds_or(request.max_seconds);
    // The event cap is a ceiling on the whole report, so it divides. The time
    // budget does not: the groups are scanned concurrently below, so each can
    // have the full deadline without the report taking any longer.
    let per_group_events = (max_events / log_groups.len()).max(50);
    let report_timer = crate::logging::Timed::start(
        cat::EVENTS,
        format!(
            "registry report for {} over {minutes}m (max {max_events} events, {budget_seconds}s budget)",
            env.registry_name
        ),
    );
    let start_time = (time::OffsetDateTime::now_utc() - time::Duration::minutes(minutes))
        .unix_timestamp()
        * 1000;

    // --- one striped pass over the log group -------------------------------
    //
    // Striped rather than linear: `FilterLogEvents` paginates from the start
    // time, so a capped linear scan reads a contiguous recent slice and every
    // event type that only fires overnight reads as "no traffic". Slicing the
    // window and scanning the slices concurrently samples the whole period.
    let logs = aws_sdk_cloudwatchlogs::Client::new(&cfg);
    let scan_end = crate::events_cache::now_ms();

    let mut scanned = 0usize;
    let mut truncated = false;
    let mut scan_note: Option<String> = None;
    let mut scan_stripes = 0usize;
    let mut scan_ms = 0u128;

    let mut buckets: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    // Cacheable events per (log group, type): the cache is keyed by group, so
    // events from the bridge and events from the bus stay distinguishable.
    let mut cacheable: BTreeMap<(String, String), Vec<CachedEvent>> = BTreeMap::new();
    // Counted before the cap below, so volume survives the sampling.
    let mut observed: BTreeMap<String, usize> = BTreeMap::new();

    // Concurrent: the groups are independent AWS calls, and scanning them one
    // after another spent the whole budget twice over in wall-clock for no
    // extra coverage.
    let scanned_so_far = std::sync::atomic::AtomicUsize::new(0);
    let outcomes = futures::future::join_all(log_groups.iter().map(|group| {
        let logs = logs.clone();
        let group = group.clone();
        let progress_app = app.clone();
        let scanned_so_far = &scanned_so_far;
        async move {
            log_scan::scan_striped(
                &logs,
                log_scan::ScanRequest {
                    log_group: group.clone(),
                    pattern: None,
                    start_time,
                    end_time: scan_end,
                    max_events: per_group_events,
                    budget: std::time::Duration::from_secs(budget_seconds),
                    stripes: log_scan::DEFAULT_STRIPES,
                },
                move |count| {
                    // Cumulative across groups, or the counter would jump
                    // backwards as the other group reports its own total.
                    let total = scanned_so_far
                        .fetch_add(count, std::sync::atomic::Ordering::Relaxed)
                        + count;
                    let _ = progress_app
                        .emit("report://progress", serde_json::json!({ "scanned": total }));
                },
            )
            .await
            .map(|outcome| (group, outcome))
        }
    }))
    .await;

    for outcome in outcomes {
        let (group, outcome) = outcome?;

        scanned += outcome.scanned;
        truncated |= outcome.truncated;
        scan_note = scan_note.or(outcome.note.clone());
        scan_stripes = scan_stripes.max(outcome.stripes);
        scan_ms = scan_ms.max(outcome.elapsed_ms);

        for (source, detail_type, cached) in outcome.events {
            let key = format!("{source}@{detail_type}");
            *observed.entry(key.clone()).or_default() += 1;
            let bucket = buckets.entry(key.clone()).or_default();
            // A handful of events characterises a type; keeping thousands of
            // identical payloads would blow memory for no extra signal.
            if bucket.len() < 50 {
                bucket.push(cached.detail.clone());
                cacheable
                    .entry((group.clone(), key))
                    .or_default()
                    .push(cached);
            }
        }
    }

    // Warm the per-schema cache from this scan, so opening any of these
    // schemas afterwards needs no further CloudWatch call.
    for ((group, key), events) in cacheable {
        let Some((source, detail_type)) = key.split_once('@') else {
            continue;
        };
        let cache_id = cache_key(&env.id, &group, source, detail_type);
        state
            .events
            .merge(&cache_id, events, start_time, scan_end, truncated)
            .await;
    }
    // One write for the whole scan, not one per event type.
    state.events.persist().await;

    // --- grade each schema against its bucket -------------------------------
    let schemas = aws_sdk_schemas::Client::new(&cfg);
    let mut names = Vec::new();
    let mut schema_pages = schemas
        .list_schemas()
        .registry_name(&env.registry_name)
        .into_paginator()
        .send();
    while let Some(page) = schema_pages.next().await {
        let page = page.map_err(map_sdk_error)?;
        for summary in page.schemas() {
            if let Some(name) = summary.schema_name() {
                names.push(name.to_string());
            }
        }
    }

    // Decide which bucket each schema is graded against before touching AWS —
    // this half is pure and lets the describes below run concurrently.
    let mut rows: Vec<ReportRow> = Vec::with_capacity(names.len());
    let mut matched: BTreeSet<String> = BTreeSet::new();
    let mut to_grade: Vec<(String, EventIdentity, String)> = Vec::new();

    let bucket_keys: Vec<String> = buckets.keys().cloned().collect();
    let sanitized_buckets = sanitized_bucket_index(&bucket_keys);

    for name in &names {
        let Ok(identity) = EventIdentity::from_schema_name(name) else {
            continue;
        };

        let key = match_bucket(&identity, &buckets, &sanitized_buckets);

        match key {
            Some(key) => {
                matched.insert(key.clone());
                to_grade.push((name.clone(), identity, key));
            }
            None => rows.push(ReportRow {
                name: name.clone(),
                source: identity.source,
                detail_type: identity.detail_type,
                status: RowStatus::NoTraffic,
                ..Default::default()
            }),
        }
    }

    // One describe per schema, run concurrently. Serially this was the
    // dominant cost of the whole report — 300 schemas at a ~100ms round trip
    // is half a minute of pure latency, all of it independent work.
    const GRADE_CONCURRENCY: usize = 16;
    let total = to_grade.len();
    let graded_count = std::sync::atomic::AtomicUsize::new(0);

    let mut graded_rows: Vec<ReportRow> = futures::stream::iter(to_grade)
        .map(|(name, identity, key)| {
            let schemas = schemas.clone();
            let registry = env.registry_name.clone();
            let app = app.clone();
            let buckets = &buckets;
            let observed = &observed;
            let graded_count = &graded_count;
            let state = &state;
            async move {
                let key_for_row = key.clone();
                let payloads: &[Value] = buckets.get(&key).map(Vec::as_slice).unwrap_or(&[]);
                let seen = observed.get(&key).copied().unwrap_or(payloads.len());

                // The version stamp is kept even when grading fails: an
                // error row is still one you might fix by saving the schema.
                let mut version = None;
                let mut last_modified = None;
                let graded = async {
                    let fetched = fetch_schema_versioned(&schemas, &registry, &name).await?;
                    version = fetched.version.clone();
                    last_modified = fetched.last_modified.clone();
                    graded_check(state, &identity, &fetched.content, &fetched.type_name, payloads)
                        .await
                }
                .await;

                let done = graded_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if done % 10 == 0 || done == total {
                    let _ = app.emit(
                        "report://progress",
                        serde_json::json!({ "graded": done, "total": total }),
                    );
                }

                // The bucket key is the identity the events carry; the schema
                // name is what it was registered as. They differ exactly when
                // the source or detail-type held something a name cannot.
                let wire_identity =
                    (key_for_row != identity.schema_name()).then(|| key_for_row.clone());

                match graded {
                    Ok(report) => {
                        let drift = &report.drift;
                        let status = if report.failed > 0 {
                            RowStatus::Failing
                        } else if !drift.undeclared.is_empty()
                            || !drift.type_mismatches.is_empty()
                            || !drift.enum_drift.is_empty()
                        {
                            RowStatus::Drifting
                        } else {
                            RowStatus::Ok
                        };

                        // The ranked issue list already decided what the most
                        // useful single line about this schema is; deriving a
                        // second, cruder ranking here made one schema read
                        // three different ways across three screens.
                        let headline = report.issues.first().map(|issue| issue.summary.clone());

                        ReportRow {
                            name: name.clone(),
                            source: identity.source,
                            detail_type: identity.detail_type,
                            sampled: report.sampled,
                            observed: seen,
                            passed: report.passed,
                            failed: report.failed,
                            undeclared: drift.undeclared.len(),
                            type_mismatches: drift.type_mismatches.len(),
                            enum_drift: drift.enum_drift.len(),
                            missing_required: drift.missing_required.len(),
                            status,
                            headline,
                            wire_identity,
                            version,
                            last_modified,
                        }
                    }
                    Err(e) => ReportRow {
                        name: name.clone(),
                        source: identity.source,
                        detail_type: identity.detail_type,
                        sampled: payloads.len(),
                        observed: seen,
                        status: RowStatus::Error,
                        headline: Some(e.to_string()),
                        version,
                        last_modified,
                        ..Default::default()
                    },
                }
            }
        })
        .buffer_unordered(GRADE_CONCURRENCY)
        .collect()
        .await;

    rows.append(&mut graded_rows);

    // Anything on the bus that no schema claimed.
    let mut unregistered: Vec<UnregisteredEvent> = buckets
        .iter()
        .filter(|(key, _)| !matched.contains(*key))
        .filter_map(|(key, payloads)| {
            key.split_once('@')
                .map(|(source, detail_type)| UnregisteredEvent {
                    source: source.to_string(),
                    detail_type: detail_type.to_string(),
                    count: observed.get(key).copied().unwrap_or(payloads.len()),
                })
        })
        .collect();
    unregistered.sort_by_key(|e| std::cmp::Reverse(e.count));

    // Worst first: failing, then drifting, then everything else. Also imposes a
    // deterministic order on rows that were graded concurrently.
    rows.sort_by(|a, b| {
        a.status
            .rank()
            .cmp(&b.status.rank())
            .then(b.failed.cmp(&a.failed))
            .then(a.name.cmp(&b.name))
    });

    report_timer.done(format!(
        "{} schemas graded, {scanned} events scanned across {scan_stripes} stripes in {scan_ms}ms, {} unregistered types",
        rows.len(),
        unregistered.len()
    ));

    Ok(RegistryReport {
        rows,
        unregistered,
        scanned_events: scanned,
        scan_note,
        stripes: scan_stripes,
        scan_ms,
        minutes,
        log_groups,
        registry: env.registry_name,
        truncated,
        generated_at: crate::events_cache::now_ms(),
    })
}

/// A schema drafted from observed traffic, ready to review and register.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InferredDraft {
    pub name: String,
    pub content: Value,
    pub file_name: String,
    pub validation: crate::schema::validate::ValidationReport,
    /// How many events the shape was inferred from.
    pub sampled: usize,
    pub from_cache: bool,
}

/// Draft a schema for an event type that has none, inferred from its traffic.
///
/// The health report finds event types flowing with no schema registered; the
/// events themselves already describe the shape, so this turns that finding
/// into something reviewable rather than a blank form. Cached samples are used
/// when available, so clicking through from a report costs nothing.
#[tauri::command]
pub async fn draft_from_events(
    app: AppHandle,
    state: State<'_, AppState>,
    source: String,
    detail_type: String,
    env_id: Option<String>,
    minutes: Option<i64>,
    log_group: Option<String>,
    // Centre the window on this instant (epoch ms) instead of ending it now:
    // one minute either side. For drafting from one known event — a watch
    // hit — that is a two-minute scan instead of a day's.
    around_ms: Option<i64>,
) -> Result<InferredDraft> {
    let identity = EventIdentity {
        source: source.trim().to_string(),
        detail_type: detail_type.trim().to_string(),
    };
    if identity.source.is_empty() || identity.detail_type.is_empty() {
        return Err(Error::Invalid(
            "Both a source and a detail-type are required".into(),
        ));
    }

    let (env, cfg) = state.env_config(env_id.as_deref()).await?;
    // Every group: an undocumented event type is *more* likely than most to
    // have arrived over the bridge, which is exactly the traffic reading only
    // the first group misses.
    let groups = resolve_log_groups(&env, log_group)?;

    let minutes = minutes.unwrap_or(60 * 24).max(1);
    let now = crate::events_cache::now_ms();
    let (start_time, end_time, window_label) = match around_ms {
        Some(at) => (
            at - 60_000,
            // The cache records this as covered, so it must not reach into
            // the future and claim events that have not happened yet.
            (at + 60_000).min(now),
            "within a minute of that event".to_string(),
        ),
        None => (
            (time::OffsetDateTime::now_utc() - time::Duration::minutes(minutes)).unix_timestamp()
                * 1000,
            now,
            format!("over the last {minutes} minutes"),
        ),
    };

    // Every step emits, so the UI can say what is happening instead of
    // showing an indefinite spinner.
    let stage = |label: &str| {
        let _ = app.emit("draft://progress", label);
    };

    stage("Looking for cached events…");

    let mut from_cache = false;
    let cached = state
        .events
        .sample_across(
            &env.id,
            &groups,
            &identity.source,
            &identity.detail_type,
            start_time,
            end_time,
        )
        .await
        .payloads;
    let payloads: Vec<Value> = match cached {
        hit if !hit.is_empty() => {
            from_cache = true;
            hit
        }
        _ => {
            // Nothing cached — fetch just this event type, from each group in
            // turn until it turns up.
            let client = aws_sdk_cloudwatchlogs::Client::new(&cfg);
            let pattern = super::logs::identity_pattern(&identity.source, &identity.detail_type);
            let mut details = Vec::new();

            for candidate in &groups {
                stage(&format!("Fetching events from {candidate}…"));
                let page = client
                    .filter_log_events()
                    .log_group_name(candidate)
                    .start_time(start_time)
                    .end_time(end_time)
                    .filter_pattern(&pattern)
                    .limit(200)
                    .send()
                    .await
                    .map_err(map_sdk_error)?;

                let mut fetched = Vec::new();
                for event in page.events() {
                    let Some(message) = event.message() else {
                        continue;
                    };
                    if let Some((_, _, cached)) = parse_event(
                        event.event_id().unwrap_or_default(),
                        event.timestamp().unwrap_or(0),
                        message,
                    ) {
                        details.push(cached.detail.clone());
                        fetched.push(cached);
                    }
                }

                if !fetched.is_empty() {
                    stage(&format!("Caching {} events…", fetched.len()));
                    let key =
                        cache_key(&env.id, candidate, &identity.source, &identity.detail_type);
                    state
                        .events
                        .merge(&key, fetched, start_time, end_time, false)
                        .await;
                    state.events.persist().await;
                    break;
                }
            }
            details
        }
    };

    if payloads.is_empty() {
        return Err(Error::NotFound(format!(
            "No events for {} in {} {window_label}, so there is nothing to infer a shape from.",
            identity.schema_name(),
            groups.join(" or ")
        )));
    }

    stage(&format!(
        "Inferring a shape from {} events…",
        payloads.len()
    ));
    let inferring = crate::logging::Timed::start(
        cat::SCHEMA,
        format!(
            "inferring a shape for {} from {} events",
            identity.schema_name(),
            payloads.len()
        ),
    );
    let detail = crate::schema::infer::infer_payload_schema(&payloads);
    let content = model::document_with_detail(&identity, detail);
    let name = identity.schema_name();
    let validation = crate::schema::validate::validate(&content, Some(&name));
    inferring.done(format!(
        "drafted, {} findings, from_cache={from_cache}",
        validation.findings.len()
    ));

    Ok(InferredDraft {
        file_name: model::file_name_for(&identity),
        name,
        content,
        validation,
        sampled: payloads.len(),
        from_cache,
    })
}

/// Declare a single observed field on the type that owns its path.
#[tauri::command]
pub fn add_observed_field(
    content: Value,
    type_name: String,
    field: FieldObservation,
) -> Result<Value> {
    let document = model::parse_content(&content)?;
    events::add_observed_field(&document, &type_name, &field)
}

/// Add every top-level undeclared field to the type, typed from what was seen.
///
/// Returns the patched document for review — it is applied to the editor's
/// draft, never written straight to AWS.
#[tauri::command]
pub fn apply_field_suggestions(
    content: Value,
    type_name: String,
    fields: Vec<FieldObservation>,
) -> Result<Value> {
    let document = model::parse_content(&content)?;
    Ok(events::suggest_additions(&document, &type_name, &fields))
}

/// Apply one issue's repair to the draft.
///
/// The repair travels out to the panel on the issue and comes back with the
/// click, so what is applied is what was offered — the alternative, re-deriving
/// it here from a path and a kind, would let the two disagree with nothing
/// failing to say so. Returns the patched document for review; nothing is
/// written to AWS.
#[tauri::command]
pub fn apply_issue_repair(
    content: Value,
    type_name: String,
    path: String,
    repair: Repair,
) -> Result<Value> {
    let document = model::parse_content(&content)?;
    repair::apply(&document, &type_name, &path, &repair)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env_with(groups: Vec<&str>) -> Environment {
        Environment {
            id: "prd".into(),
            label: "prd".into(),
            aws_profile: "p".into(),
            sso: None,
            region: "us-east-2".into(),
            registry_name: "prd-global-registry".into(),
            log_groups: groups.into_iter().map(str::to_string).collect(),
            protected: true,
        }
    }

    #[test]
    fn scans_every_configured_log_group_not_just_the_first() {
        // Events reaching the bus over the bridge land in the external group.
        // Reading only the first made an unregistered external event type
        // invisible, so the report claimed zero missing schemas while Slack
        // was alerting about one.
        let env = env_with(vec![
            "/aws/events/prd-global-events",
            "/aws/events/prd-external-events",
        ]);
        assert_eq!(
            resolve_log_groups(&env, None).unwrap(),
            vec![
                "/aws/events/prd-global-events".to_string(),
                "/aws/events/prd-external-events".to_string(),
            ],
        );
    }

    #[test]
    fn an_explicit_choice_wins_over_the_configured_list() {
        let env = env_with(vec!["/aws/events/prd-global-events", "/other"]);
        assert_eq!(
            resolve_log_groups(&env, Some("/just-this-one".into())).unwrap(),
            vec!["/just-this-one".to_string()],
        );
        // A blank choice is not a choice.
        assert_eq!(
            resolve_log_groups(&env, Some("  ".into())).unwrap().len(),
            2
        );
    }

    #[test]
    fn an_environment_with_no_log_groups_says_so() {
        assert!(resolve_log_groups(&env_with(vec![]), None).is_err());
        assert!(resolve_log_groups(&env_with(vec!["  "]), None).is_err());
    }

    #[test]
    fn resolves_the_detail_type_from_the_envelope() {
        let document = json!({
            "components": { "schemas": {
                "AWSEvent": {
                    "properties": { "detail": { "$ref": "#/components/schemas/Payload" } }
                },
                "Payload": { "type": "object" }
            }}
        });
        assert_eq!(detail_type_name(&document, None).unwrap(), "Payload");
    }

    #[test]
    fn prefers_an_explicit_type_override() {
        let document = json!({
            "components": { "schemas": {
                "AWSEvent": {
                    "properties": { "detail": { "$ref": "#/components/schemas/Payload" } }
                }
            }}
        });
        assert_eq!(detail_type_name(&document, Some("Other")).unwrap(), "Other");
        // A blank override falls through rather than being taken literally.
        assert_eq!(detail_type_name(&document, Some("  ")).unwrap(), "Payload");
    }

    /// Buckets keyed the way `registry_report` keys them: `source@detail-type`
    /// straight off the event.
    fn buckets_for(keys: &[&str]) -> BTreeMap<String, Vec<Value>> {
        keys.iter()
            .map(|k| (k.to_string(), vec![json!({})]))
            .collect()
    }

    fn matched_key(schema_name: &str, bucket_keys: &[&str]) -> Option<String> {
        let identity = EventIdentity::from_schema_name(schema_name).unwrap();
        let buckets = buckets_for(bucket_keys);
        let keys: Vec<String> = buckets.keys().cloned().collect();
        match_bucket(&identity, &buckets, &sanitized_bucket_index(&keys))
    }

    #[test]
    fn matches_a_schema_to_the_bucket_named_exactly_after_it() {
        assert_eq!(
            matched_key("orders-api@order-assigned", &["orders-api@order-assigned"]),
            Some("orders-api@order-assigned".into())
        );
    }

    #[test]
    fn matches_a_discovered_schemas_pascal_case_detail_type() {
        assert_eq!(
            matched_key("orders-api@order-assigned", &["orders-api@OrderAssigned"]),
            Some("orders-api@OrderAssigned".into())
        );
    }

    #[test]
    fn matches_a_source_that_had_to_be_sanitized_to_be_a_legal_schema_name() {
        // The real case: events carry `Atomic Forms`, which cannot be a schema
        // name, so the schema is registered as `Atomic-Forms@…`. Without this
        // the schema reported "no traffic" *and* its events reported "missing"
        // — the same event type counted twice, and a draft offered for a
        // schema already registered at v3.
        assert_eq!(
            matched_key(
                "Atomic-Forms@CASE_STATUS_CHANGED",
                &["Atomic Forms@CASE_STATUS_CHANGED"]
            ),
            Some("Atomic Forms@CASE_STATUS_CHANGED".into())
        );
    }

    #[test]
    fn prefers_an_exact_bucket_over_a_sanitized_one() {
        // Both spellings on the bus at once: the schema owns the one that
        // matches it literally.
        assert_eq!(
            matched_key(
                "Atomic-Forms@CASE_STATUS_CHANGED",
                &[
                    "Atomic Forms@CASE_STATUS_CHANGED",
                    "Atomic-Forms@CASE_STATUS_CHANGED",
                ]
            ),
            Some("Atomic-Forms@CASE_STATUS_CHANGED".into())
        );
    }

    #[test]
    fn reports_no_traffic_when_nothing_matches() {
        assert_eq!(
            matched_key("orders-api@order-assigned", &["other@thing"]),
            None
        );
    }

    #[test]
    fn does_not_index_bucket_keys_that_are_already_legal_names() {
        // Only names that actually changed under sanitization are indexed, so
        // a legal name cannot be claimed by an unrelated schema.
        let index = sanitized_bucket_index(&["orders-api@order-assigned".to_string()]);
        assert!(index.is_empty());
    }

    #[test]
    fn explains_itself_when_the_envelope_has_no_detail_ref() {
        let document = json!({
            "components": { "schemas": { "AWSEvent": { "properties": {} } } }
        });
        let err = detail_type_name(&document, None).unwrap_err();
        assert!(matches!(err, Error::Invalid(_)));
    }
}

/// Everything a bulk filing run needs about one schema.
///
/// Deliberately built from the *cached* sample the report just took, so
/// selecting twenty schemas costs twenty describes rather than twenty
/// CloudWatch scans.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaIssues {
    pub schema_name: String,
    pub context: crate::jira::ticket::TicketContext,
    pub issues: Vec<events::Issue>,
    /// Why this schema produced nothing, when it produced nothing.
    pub note: Option<String>,
    pub error: Option<Error>,
}

/// Re-derive the issues for a set of schemas, for filing them in one go.
#[tauri::command]
pub async fn issues_for_schemas(
    state: State<'_, AppState>,
    names: Vec<String>,
    minutes: Option<i64>,
    log_group: Option<String>,
    env_id: Option<String>,
) -> Result<Vec<SchemaIssues>> {
    let (env, cfg) = state.env_config(env_id.as_deref()).await?;
    let log_groups = resolve_log_groups(&env, log_group)?;
    let schemas = aws_sdk_schemas::Client::new(&cfg);
    let minutes = minutes.unwrap_or(60 * 24).max(1);

    let now = crate::events_cache::now_ms();
    let start_time = (time::OffsetDateTime::now_utc() - time::Duration::minutes(minutes))
        .unix_timestamp()
        * 1000;

    // Concurrent describes, same reasoning as the registry report: the calls
    // are independent and the latency is the whole cost.
    const CONCURRENCY: usize = 8;
    let results = futures::stream::iter(names)
        .map(|name| {
            let schemas = schemas.clone();
            let registry = env.registry_name.clone();
            let env = env.clone();
            let log_groups = log_groups.clone();
            let state = &state;
            async move {
                // One context, then whatever the successful path learned —
                // two literals side by side drift the moment a field is added.
                let mut context = crate::jira::ticket::TicketContext {
                    schema_name: name.clone(),
                    environment: env.label.clone(),
                    registry: Some(env.registry_name.clone()),
                    source: String::new(),
                    detail_type: String::new(),
                    log_group: log_groups.first().cloned(),
                    minutes: Some(minutes as u64),
                    type_name: None,
                    origin: None,
                };

                let gathered = async {
                    let identity = EventIdentity::from_schema_name(&name)?;
                    // Whichever group the report cached them under.
                    let cached = state
                        .events
                        .sample_across(
                            &env.id,
                            &log_groups,
                            &identity.source,
                            &identity.detail_type,
                            start_time,
                            now,
                        )
                        .await;
                    // No schema at all is the finding, not a failure to
                    // report: the type is on the bus and the registry is
                    // silent on it.
                    let (type_name, issues) =
                        match fetch_schema(&schemas, &registry, &name).await {
                            Ok((content, type_name)) => {
                                let report = graded_check(
                                    state,
                                    &identity,
                                    &content,
                                    &type_name,
                                    &cached.payloads,
                                )
                                .await?;
                                (Some(type_name), report.issues)
                            }
                            Err(Error::NotFound(_)) => {
                                let issue = events::unregistered_issue(
                                    &identity.source,
                                    &identity.detail_type,
                                    cached.payloads.len(),
                                    cached.payloads.first().cloned(),
                                );
                                (None, vec![issue])
                            }
                            Err(e) => return Err(e),
                        };
                    Ok::<_, Error>((
                        identity,
                        type_name,
                        issues,
                        cached.payloads.len(),
                        cached.found_in,
                    ))
                }
                .await;

                match gathered {
                    Ok((identity, type_name, issues, sampled, found_in)) => {
                        context.source = identity.source;
                        context.detail_type = identity.detail_type;
                        context.type_name = type_name;
                        if found_in.is_some() {
                            context.log_group = found_in;
                        }
                        SchemaIssues {
                            schema_name: name,
                            context,
                            issues,
                            note: (sampled == 0).then(|| {
                                "Nothing cached for this schema — run the report again to sample it."
                                    .to_string()
                            }),
                            error: None,
                        }
                    }
                    // One schema that cannot be described must not abandon the
                    // other nineteen.
                    Err(e) => SchemaIssues {
                        schema_name: name,
                        context,
                        issues: Vec::new(),
                        note: None,
                        error: Some(e),
                    },
                }
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect::<Vec<_>>()
        .await;

    Ok(results)
}

/// An event source that exists, and the spelling a rule has to match.
///
/// These differ when a source holds a character a schema name cannot: events
/// from `Atomic Forms` are registered under `Atomic-Forms`, and a ticket
/// filed about them carries the registered spelling — so that, not the one on
/// the wire, is what a routing rule is matched against.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventSource {
    /// As the events carry it.
    pub source: String,
    /// As a routing rule must spell it.
    pub pattern: String,
    /// Seen in sampled traffic, as opposed to only being registered.
    pub in_traffic: bool,
}

/// Every event source known for an environment, from traffic and the registry.
///
/// Traffic is the half the registry cannot supply: a producer nobody has
/// written a schema for is exactly the one worth routing before it is
/// registered. Reads the sampled-event cache, so it costs nothing and needs no
/// AWS call — but it only knows what has been scanned.
#[tauri::command]
pub async fn event_sources(
    state: State<'_, AppState>,
    env_id: Option<String>,
) -> Result<Vec<EventSource>> {
    let env = state.resolve_environment(env_id.as_deref()).await?;

    let mut sources: BTreeMap<String, EventSource> = BTreeMap::new();
    for source in state.events.sources(&env.id).await {
        let pattern = model::sanitize_schema_name(&source);
        sources.insert(
            pattern.clone(),
            EventSource {
                source,
                pattern,
                in_traffic: true,
            },
        );
    }

    Ok(sources.into_values().collect())
}

/// Check one event, as caught by a watch, against the schema registered for
/// its type. No schema at all is reported as the finding it is.
#[tauri::command]
pub async fn validate_event(
    state: State<'_, AppState>,
    name: String,
    event: Value,
    env_id: Option<String>,
) -> Result<Vec<events::Issue>> {
    let identity = EventIdentity::from_schema_name(&name)?;
    let (env, cfg) = state.env_config(env_id.as_deref()).await?;
    let schemas = aws_sdk_schemas::Client::new(&cfg);
    let detail = event.get("detail").cloned();
    let (content, type_name) = match fetch_schema(&schemas, &env.registry_name, &name).await {
        Ok(found) => found,
        Err(Error::NotFound(_)) => {
            return Ok(vec![events::unregistered_issue(
                &identity.source,
                &identity.detail_type,
                1,
                detail,
            )])
        }
        Err(e) => return Err(e),
    };
    let detail =
        detail.ok_or_else(|| Error::Invalid("The event has no `detail` to check".into()))?;
    let report = graded_check(&state, &identity, &content, &type_name, &[detail]).await?;
    Ok(report.issues)
}
