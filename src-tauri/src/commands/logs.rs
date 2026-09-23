use crate::aws::clients::map_sdk_error;
use crate::error::Result;
use crate::logging::cat;
use crate::state::AppState;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogGroupSummary {
    pub name: String,
    pub stored_bytes: Option<i64>,
    pub retention_days: Option<i32>,
    /// True when the group is one the active environment declares. Lets the UI
    /// show configured groups first and other `/aws/events/*` groups after.
    pub configured: bool,
}

/// List the environment's declared log groups plus any other `/aws/events/*`
/// groups in the account, so a mis-typed group name in settings is obvious.
#[tauri::command]
pub async fn list_log_groups(
    state: State<'_, AppState>,
    env_id: Option<String>,
) -> Result<Vec<LogGroupSummary>> {
    let (env, cfg) = state.env_config(env_id.as_deref()).await?;
    let client = aws_sdk_cloudwatchlogs::Client::new(&cfg);

    let mut discovered = Vec::new();
    let mut pages = client
        .describe_log_groups()
        .log_group_name_prefix("/aws/events/")
        .into_paginator()
        .send();

    while let Some(page) = pages.next().await {
        let page = page.map_err(map_sdk_error)?;
        for g in page.log_groups() {
            let Some(name) = g.log_group_name() else { continue };
            discovered.push(LogGroupSummary {
                configured: env.log_groups.iter().any(|c| c == name),
                name: name.to_string(),
                stored_bytes: g.stored_bytes(),
                retention_days: g.retention_in_days(),
            });
        }
    }

    // Declared-but-absent groups still need to appear, flagged as configured,
    // so the user can see the mismatch rather than an empty list.
    for configured in &env.log_groups {
        if !discovered.iter().any(|g| &g.name == configured) {
            discovered.push(LogGroupSummary {
                name: configured.clone(),
                stored_bytes: None,
                retention_days: None,
                configured: true,
            });
        }
    }

    discovered.sort_by(|a, b| b.configured.cmp(&a.configured).then(a.name.cmp(&b.name)));
    Ok(discovered)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogQuery {
    pub log_group: String,
    /// Epoch milliseconds.
    pub start_time: i64,
    pub end_time: Option<i64>,
    /// Structured filters, compiled into a CloudWatch JSON filter pattern.
    pub source: Option<String>,
    pub detail_type: Option<String>,
    /// A raw filter pattern. Takes precedence over the structured filters.
    pub filter_pattern: Option<String>,
    /// The most events to return; the newest matches win.
    pub limit: Option<i32>,
}

/// Compile the structured filters into a CloudWatch Logs JSON filter pattern.
///
/// Events land in these groups as the raw EventBridge envelope, so `$.source`
/// and `$.detail-type` address the fields directly. Returns `None` when there
/// is nothing to filter on, which is faster than sending a match-everything
/// pattern.
fn build_filter_pattern(query: &LogQuery) -> Option<String> {
    if let Some(raw) = query.filter_pattern.as_ref().filter(|p| !p.trim().is_empty()) {
        return Some(raw.trim().to_string());
    }

    let mut clauses = Vec::new();
    if let Some(source) = query.source.as_ref().filter(|s| !s.trim().is_empty()) {
        clauses.push(source_clause(source.trim()));
    }
    if let Some(detail_type) = query.detail_type.as_ref().filter(|s| !s.trim().is_empty()) {
        clauses.push(detail_type_clause(detail_type.trim()));
    }

    (!clauses.is_empty()).then(|| wrap_clauses(&clauses))
}

/// Pattern matching exactly one event type.
///
/// The single place that knows how an `EventIdentity` narrows a log scan, so
/// the reality checks and the log viewer cannot drift apart on filter grammar.
pub fn identity_pattern(source: &str, detail_type: &str) -> String {
    wrap_clauses(&[source_clause(source), detail_type_clause(detail_type)])
}

pub(crate) fn source_clause(source: &str) -> String {
    format!("$.source = \"{}\"", escape(source))
}

/// Unquoted, despite the hyphen: CloudWatch's filter grammar rejects
/// `$."detail-type"` outright with `Invalid character(s) in term`.
pub(crate) fn detail_type_clause(detail_type: &str) -> String {
    format!("$.detail-type = \"{}\"", escape(detail_type))
}

pub(crate) fn wrap_clauses(clauses: &[String]) -> String {
    format!("{{ {} }}", clauses.join(" && "))
}

/// Quote-escape a user-supplied value so it cannot break out of the pattern.
///
/// `*` is deliberately left alone: CloudWatch treats it as a wildcard inside a
/// quoted string value, so `orders*`, `*assigned` and `*Notification*` all work
/// as leading/trailing/both-ends matches.
pub(crate) fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEvent {
    pub timestamp: Option<i64>,
    pub ingestion_time: Option<i64>,
    pub log_stream: Option<String>,
    pub message: String,
    /// Parsed event envelope, when the message is JSON. Lets the table show
    /// source/detail-type columns without the frontend re-parsing every row.
    pub event: Option<Value>,
    pub source: Option<String>,
    pub detail_type: Option<String>,
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    /// Newest first.
    pub events: Vec<LogEvent>,
    /// The pattern we actually sent, surfaced so the UI can show what ran.
    pub filter_pattern: Option<String>,
    /// How far back the scan got, epoch milliseconds. Everything between here
    /// and the end of the window was searched; nothing before it was.
    pub searched_from: i64,
    /// True when the whole window was searched.
    pub complete: bool,
    /// Why the scan stopped short, when it did.
    pub scan_note: Option<String>,
}

/// How many slices are read at once. The report's stripes are wider; here
/// the newest slices matter most, so a few at a time, newest first.
const SLICE_CONCURRENCY: usize = 4;
/// Pages followed within one slice before giving up on it.
const MAX_PAGES: usize = 100;
const PAGE_LIMIT: i32 = 2_000;

/// The window cut into slices, newest first.
///
/// `FilterLogEvents` reads forward from its start time and stops when it has
/// looked at enough data, matched or not, so one call over a week of a busy
/// group looks at the first few hours and reports nothing. Reading the window
/// in slices means every slice is finished before the next is begun, and the
/// answer to "how far back did you look" is a time rather than a token.
fn slices(start: i64, end: i64) -> Vec<(i64, i64)> {
    const MIN: i64 = 5 * 60 * 1000;
    const MAX: i64 = 6 * 60 * 60 * 1000;
    let window = (end - start).max(1);
    let width = (window / 16).clamp(MIN, MAX);
    let mut out = Vec::new();
    let mut to = end;
    while to > start {
        let from = (to - width).max(start);
        out.push((from, to));
        to = from;
    }
    out
}

/// One slice, read to the end or the page cap.
async fn read_slice(
    client: &aws_sdk_cloudwatchlogs::Client,
    group: &str,
    pattern: Option<&str>,
    from: i64,
    to: i64,
) -> Result<(Vec<LogEvent>, bool)> {
    let mut req = client
        .filter_log_events()
        .log_group_name(group)
        .start_time(from)
        .end_time(to)
        .limit(PAGE_LIMIT);
    if let Some(pattern) = pattern {
        req = req.filter_pattern(pattern);
    }
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..MAX_PAGES {
        let response = req
            .clone()
            .set_next_token(token.take())
            .send()
            .await
            .map_err(map_sdk_error)?;
        out.extend(response.events().iter().map(log_event));
        token = response.next_token().map(str::to_string);
        if token.is_none() {
            return Ok((out, true));
        }
        if page + 1 == MAX_PAGES {
            lwarn!(cat::EVENTS, "{group}: a slice hit the {MAX_PAGES}-page cap");
        }
    }
    Ok((out, false))
}

fn log_event(e: &aws_sdk_cloudwatchlogs::types::FilteredLogEvent) -> LogEvent {
    let message = e.message().unwrap_or_default().to_string();
    let parsed = serde_json::from_str::<Value>(&message).ok();
    let get = |key: &str| {
        parsed
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    LogEvent {
        timestamp: e.timestamp(),
        ingestion_time: e.ingestion_time(),
        log_stream: e.log_stream_name().map(str::to_string),
        source: get("source"),
        detail_type: get("detail-type"),
        event_id: get("id"),
        event: parsed,
        message,
    }
}

#[tauri::command]
pub async fn query_logs(
    state: State<'_, AppState>,
    query: LogQuery,
    env_id: Option<String>,
) -> Result<LogPage> {
    let (_, cfg) = state.env_config(env_id.as_deref()).await?;
    let client = aws_sdk_cloudwatchlogs::Client::new(&cfg);

    let filter_pattern = build_filter_pattern(&query);
    let limit = query.limit.unwrap_or(200).clamp(1, 10_000) as usize;
    // Settings → Scanning owns the time budget, as it does for every scan.
    let budget =
        std::time::Duration::from_secs(state.settings_snapshot().await.scan.seconds_or(None));
    let end = query.end_time.unwrap_or_else(crate::events_cache::now_ms);
    let started = std::time::Instant::now();

    let plan = slices(query.start_time, end);
    let mut events: Vec<LogEvent> = Vec::new();
    let mut searched_from = end;
    let mut complete = true;
    let mut scan_note = None;

    for batch in plan.chunks(SLICE_CONCURRENCY) {
        if events.len() >= limit {
            complete = false;
            scan_note = Some(format!(
                "Showing the newest {limit} matches; continue for older ones"
            ));
            break;
        }
        if started.elapsed() >= budget {
            complete = false;
            scan_note = Some(format!(
                "Stopped at the {}s scanning budget (Settings → Scanning); continue to search further back",
                budget.as_secs()
            ));
            break;
        }
        let reads = batch.iter().map(|(from, to)| {
            read_slice(
                &client,
                &query.log_group,
                filter_pattern.as_deref(),
                *from,
                *to,
            )
        });
        for (result, (from, _)) in join_all(reads).await.into_iter().zip(batch) {
            let (found, whole) = result?;
            events.extend(found);
            if !whole {
                scan_note = Some(
                    "A slice was too dense to read fully; some events in it may be missing".into(),
                );
            }
            searched_from = *from;
        }
    }

    // Newest first, and no more than asked for.
    events.sort_by_key(|e| std::cmp::Reverse(e.timestamp.unwrap_or(0)));
    if events.len() > limit {
        events.truncate(limit);
        complete = false;
    }

    Ok(LogPage {
        events,
        filter_pattern,
        searched_from,
        complete,
        scan_note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_cover_the_window_newest_first() {
        let hour = 60 * 60 * 1000;
        let cut = slices(0, 7 * 24 * hour);
        // 7 days at the 6h ceiling: 28 slices, the first ending at the end.
        assert_eq!(cut.len(), 28);
        assert_eq!(cut[0], (7 * 24 * hour - 6 * hour, 7 * 24 * hour));
        assert_eq!(cut[27], (0, 6 * hour));
        for pair in cut.windows(2) {
            assert_eq!(pair[0].0, pair[1].1, "contiguous");
        }
        // A short window is not cut below five minutes.
        let short = slices(0, 20 * 60 * 1000);
        assert_eq!(short.len(), 4);
    }

    fn query() -> LogQuery {
        LogQuery {
            log_group: "/aws/events/dev-global-events".into(),
            start_time: 0,
            end_time: None,
            source: None,
            detail_type: None,
            filter_pattern: None,
            limit: None,
        }
    }

    #[test]
    fn no_filters_produces_no_pattern() {
        assert!(build_filter_pattern(&query()).is_none());
        let mut q = query();
        q.source = Some("   ".into());
        assert!(build_filter_pattern(&q).is_none());
    }

    #[test]
    fn compiles_source_filter() {
        let mut q = query();
        q.source = Some("orders-api".into());
        assert_eq!(
            build_filter_pattern(&q).unwrap(),
            r#"{ $.source = "orders-api" }"#
        );
    }

    #[test]
    fn compiles_combined_filters_with_an_unquoted_member_name() {
        // Verified against the real API: quoting the hyphenated key as
        // `$."detail-type"` is rejected with `Invalid character(s) in term`.
        let mut q = query();
        q.source = Some("orders-api".into());
        q.detail_type = Some("orderNotification-assigned".into());
        assert_eq!(
            build_filter_pattern(&q).unwrap(),
            r#"{ $.source = "orders-api" && $.detail-type = "orderNotification-assigned" }"#
        );
    }

    #[test]
    fn wildcards_pass_through_to_cloudwatch() {
        let mut q = query();
        q.source = Some("orders*".into());
        q.detail_type = Some("*Notification*".into());
        assert_eq!(
            build_filter_pattern(&q).unwrap(),
            r#"{ $.source = "orders*" && $.detail-type = "*Notification*" }"#
        );
    }

    #[test]
    fn raw_pattern_wins_over_structured_filters() {
        let mut q = query();
        q.source = Some("orders-api".into());
        q.filter_pattern = Some("{ $.detail.veteranId = \"123\" }".into());
        assert_eq!(
            build_filter_pattern(&q).unwrap(),
            "{ $.detail.veteranId = \"123\" }"
        );
    }

    #[test]
    fn escapes_quotes_so_values_cannot_break_the_pattern() {
        let mut q = query();
        q.source = Some(r#"evil" || $.x = ""#.into());
        let pattern = build_filter_pattern(&q).unwrap();
        assert_eq!(pattern, r#"{ $.source = "evil\" || $.x = \"" }"#);
    }
}
