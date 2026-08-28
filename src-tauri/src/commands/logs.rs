use crate::aws::clients::map_sdk_error;
use crate::error::Result;
use crate::state::AppState;
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
    pub limit: Option<i32>,
    /// Continue a previous page.
    pub next_token: Option<String>,
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

fn source_clause(source: &str) -> String {
    format!("$.source = \"{}\"", escape(source))
}

/// Unquoted, despite the hyphen: CloudWatch's filter grammar rejects
/// `$."detail-type"` outright with `Invalid character(s) in term`.
fn detail_type_clause(detail_type: &str) -> String {
    format!("$.detail-type = \"{}\"", escape(detail_type))
}

fn wrap_clauses(clauses: &[String]) -> String {
    format!("{{ {} }}", clauses.join(" && "))
}

/// Quote-escape a user-supplied value so it cannot break out of the pattern.
///
/// `*` is deliberately left alone: CloudWatch treats it as a wildcard inside a
/// quoted string value, so `milo*`, `*assigned` and `*Notification*` all work
/// as leading/trailing/both-ends matches.
fn escape(value: &str) -> String {
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
    pub events: Vec<LogEvent>,
    pub next_token: Option<String>,
    /// The pattern we actually sent, surfaced so the UI can show what ran.
    pub filter_pattern: Option<String>,
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

    let mut req = client
        .filter_log_events()
        .log_group_name(&query.log_group)
        .start_time(query.start_time)
        .limit(query.limit.unwrap_or(200).clamp(1, 10_000));

    if let Some(end) = query.end_time {
        req = req.end_time(end);
    }
    if let Some(pattern) = &filter_pattern {
        req = req.filter_pattern(pattern);
    }
    if let Some(token) = &query.next_token {
        req = req.next_token(token);
    }

    let out = req.send().await.map_err(map_sdk_error)?;

    let events = out
        .events()
        .iter()
        .map(|e| {
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
        })
        .collect();

    Ok(LogPage {
        events,
        next_token: out.next_token().map(str::to_string),
        filter_pattern,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query() -> LogQuery {
        LogQuery {
            log_group: "/aws/events/dev-global-events".into(),
            start_time: 0,
            end_time: None,
            source: None,
            detail_type: None,
            filter_pattern: None,
            limit: None,
            next_token: None,
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
        q.source = Some("milo-medical".into());
        assert_eq!(
            build_filter_pattern(&q).unwrap(),
            r#"{ $.source = "milo-medical" }"#
        );
    }

    #[test]
    fn compiles_combined_filters_with_an_unquoted_member_name() {
        // Verified against the real API: quoting the hyphenated key as
        // `$."detail-type"` is rejected with `Invalid character(s) in term`.
        let mut q = query();
        q.source = Some("milo-medical".into());
        q.detail_type = Some("packetNotification-assigned".into());
        assert_eq!(
            build_filter_pattern(&q).unwrap(),
            r#"{ $.source = "milo-medical" && $.detail-type = "packetNotification-assigned" }"#
        );
    }

    #[test]
    fn wildcards_pass_through_to_cloudwatch() {
        let mut q = query();
        q.source = Some("milo*".into());
        q.detail_type = Some("*Notification*".into());
        assert_eq!(
            build_filter_pattern(&q).unwrap(),
            r#"{ $.source = "milo*" && $.detail-type = "*Notification*" }"#
        );
    }

    #[test]
    fn raw_pattern_wins_over_structured_filters() {
        let mut q = query();
        q.source = Some("milo-medical".into());
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
