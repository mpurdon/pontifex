//! Watch mode: define what to listen for, arm the poller, read what it caught.

use crate::aws::log_scan::{scan_striped, ScanRequest, DEFAULT_STRIPES};
use crate::error::{Error, Result};
use crate::schema::model::EventIdentity;
use crate::state::AppState;
use crate::watch::{pattern, Watch, WatchHit, WatchMark, WatchStatus};
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::Duration;
use tauri::{AppHandle, State};

#[tauri::command]
pub async fn list_watches(
    state: State<'_, AppState>,
    env_id: Option<String>,
) -> Result<Vec<Watch>> {
    let env = state.resolve_environment(env_id.as_deref()).await?;
    Ok(state.watch.watches_for(&env.id).await)
}

/// Create or update a watch. The pattern is compiled first so a path CloudWatch
/// would reject fails here, with the form open, rather than on the first poll.
#[tauri::command]
pub async fn save_watch(state: State<'_, AppState>, mut watch: Watch) -> Result<Watch> {
    state.settings_snapshot().await.environment(&watch.env_id)?;
    pattern::compile(&watch)?;
    if watch.id.trim().is_empty() {
        watch.id = uuid::Uuid::new_v4().to_string();
    }
    if watch.label.trim().is_empty() {
        watch.label = watch.summary();
    }
    if let Some(group) = watch.log_group.as_deref().map(str::trim) {
        if group.is_empty() {
            watch.log_group = None;
        }
    }
    let saved = state.watch.upsert(watch).await?;
    linfo!(
        crate::logging::cat::WATCH,
        "saved watch '{}' ({})",
        saved.label,
        saved.summary()
    );
    Ok(saved)
}

#[tauri::command]
pub async fn delete_watch(state: State<'_, AppState>, id: String) -> Result<()> {
    if state.watch.watch(&id).await.is_none() {
        return Err(Error::NotFound(format!("No watch with id {id}")));
    }
    state.watch.remove(&id).await
}

/// What the editor shows about a watch as it is typed: the filter pattern
/// it compiles to (`None` matches everything) and its one-line summary. The
/// summary comes from here so the list and the label fallback cannot drift
/// from what the backend saves.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledWatch {
    pub pattern: Option<String>,
    pub summary: String,
}

#[tauri::command]
pub fn compile_watch_pattern(watch: Watch) -> Result<CompiledWatch> {
    Ok(CompiledWatch {
        pattern: pattern::compile(&watch)?,
        summary: watch.summary(),
    })
}

/// Arm or disarm an environment's poller. Persisted, so it survives a restart.
#[tauri::command]
pub async fn set_watching(
    app: AppHandle,
    state: State<'_, AppState>,
    env_id: Option<String>,
    enabled: bool,
) -> Result<WatchStatus> {
    // Stopping must work for an environment that no longer resolves, or a
    // stale armed flag could never be cleared.
    let env_id = match (enabled, env_id) {
        (false, Some(id)) => id,
        (_, id) => state.resolve_environment(id.as_deref()).await?.id,
    };
    state.watch.set_armed(&env_id, enabled).await?;
    if enabled {
        state.watcher.start(&app, &env_id).await;
    } else {
        state.watcher.stop(&app, &env_id).await;
    }
    Ok(crate::watch::runtime::broadcast(&app, &env_id).await)
}

#[tauri::command]
pub async fn watch_status(state: State<'_, AppState>) -> Result<Vec<WatchStatus>> {
    Ok(state.watcher.status_all(&state).await)
}

#[tauri::command]
pub async fn set_watch_poll_seconds(state: State<'_, AppState>, seconds: u64) -> Result<u64> {
    state.watch.set_poll_seconds(seconds).await
}

#[tauri::command]
pub async fn set_watch_idle_timeout(state: State<'_, AppState>, minutes: u64) -> Result<u64> {
    state.watch.set_idle_timeout_minutes(minutes).await
}

/// Bring the main window back, for the idle check: a question nobody can see
/// is not a question. Hidden-by-close windows are the normal state of an
/// app that is watching in the background.
#[tauri::command]
pub fn show_main_window(app: AppHandle) -> Result<()> {
    crate::watch::show_main_window(&app)
}

#[tauri::command]
pub async fn list_watch_hits(
    state: State<'_, AppState>,
    env_id: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<WatchHit>> {
    let env = state.resolve_environment(env_id.as_deref()).await?;
    Ok(state
        .watch
        .hits_for(&env.id, limit.unwrap_or(usize::MAX))
        .await)
}

#[tauri::command]
pub async fn clear_watch_hits(
    app: AppHandle,
    state: State<'_, AppState>,
    env_id: Option<String>,
    watch_id: Option<String>,
) -> Result<()> {
    let env = state.resolve_environment(env_id.as_deref()).await?;
    let live_since = state.watcher.started_at(&env.id).await;
    state
        .watch
        .clear_hits(&env.id, watch_id.as_deref(), live_since)
        .await?;
    crate::watch::runtime::broadcast(&app, &env.id).await;
    Ok(())
}

/// Everything received so far counts as read.
#[tauri::command]
pub async fn mark_watch_hits_seen(
    app: AppHandle,
    state: State<'_, AppState>,
    env_id: Option<String>,
) -> Result<()> {
    let env = state.resolve_environment(env_id.as_deref()).await?;
    state
        .watch
        .mark_seen(&env.id, crate::events_cache::now_ms())
        .await?;
    crate::watch::runtime::broadcast(&app, &env.id).await;
    Ok(())
}

/// Show a notification now and report what macOS did with it, so "do these
/// reach me?" has an answer before a real hit is the first test.
#[tauri::command]
pub async fn test_notification(app: AppHandle) -> Result<crate::watch::notify::NotifierOutcome> {
    crate::watch::notify::test(&app).await
}

/// Reinstall the helper under a fresh identifier and test, so macOS asks
/// for permission again after a missed prompt.
#[tauri::command]
pub async fn ask_notification_permission_again(
    app: AppHandle,
) -> Result<crate::watch::notify::NotifierOutcome> {
    crate::watch::notify::ask_again(&app).await
}

/// Open System Settings on the Notifications pane, for when the helper
/// reports that Pontifex has been turned off there.
#[tauri::command]
pub fn open_notification_settings(app: AppHandle) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(
            "x-apple.systempreferences:com.apple.Notifications-Settings.extension",
            None::<&str>,
        )
        .map_err(Error::internal)
}

/// Cut the current wait short and poll immediately. The interval restarts
/// from the poll that results.
#[tauri::command]
pub async fn poll_now(state: State<'_, AppState>, env_id: Option<String>) -> Result<()> {
    let env = state.resolve_environment(env_id.as_deref()).await?;
    state.watcher.poll_now(&env.id)
}

/// How often a watch would have fired recently.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchProbe {
    pub hours: u64,
    /// Matching events found. A truncated probe stopped at its cap, so this
    /// is a floor.
    pub count: usize,
    pub truncated: bool,
    /// Epoch millis of the newest match.
    pub last_at: Option<i64>,
    /// `source@detail-type` → count, most frequent first.
    pub by_type: Vec<(String, usize)>,
}

/// Run a watch's pattern over recent history, sampled across the whole window
/// so a bursty source that fired three hours ago still shows up.
///
/// This is what turns "nothing yet" from a mystery into a fact: a watch that
/// probes to zero over a day is one to widen, and one that probes to hundreds
/// is one to narrow before it starts notifying.
#[tauri::command]
pub async fn probe_watch(
    state: State<'_, AppState>,
    watch: Watch,
    hours: Option<u64>,
) -> Result<WatchProbe> {
    let (env, cfg) = state.env_config(Some(&watch.env_id)).await?;
    let client = aws_sdk_cloudwatchlogs::Client::new(&cfg);
    let pattern = pattern::compile(&watch)?;
    let log_group = watch
        .log_group
        .clone()
        .or_else(|| env.bus_log_group().map(str::to_string))
        .ok_or_else(|| Error::Invalid(format!("Environment '{}' has no log groups", env.label)))?;

    let hours = hours.unwrap_or(24).clamp(1, 24 * 7);
    let end = crate::events_cache::now_ms();
    let start = end - (hours as i64) * 3_600_000;
    let outcome = scan_striped(
        &client,
        ScanRequest {
            log_group,
            pattern,
            start_time: start,
            end_time: end,
            max_events: 400,
            budget: Duration::from_secs(12),
            stripes: DEFAULT_STRIPES,
        },
        |_| {},
    )
    .await?;

    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    let mut last_at: Option<i64> = None;
    for (source, detail_type, event) in &outcome.events {
        let identity = EventIdentity {
            source: source.clone(),
            detail_type: detail_type.clone(),
        };
        *by_type.entry(identity.schema_name()).or_default() += 1;
        last_at = Some(last_at.map_or(event.timestamp, |t| t.max(event.timestamp)));
    }
    let mut by_type: Vec<(String, usize)> = by_type.into_iter().collect();
    by_type.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    Ok(WatchProbe {
        hours,
        count: outcome.events.len(),
        truncated: outcome.truncated,
        last_at,
        by_type,
    })
}

/// When watching started and stopped for an environment, newest first.
#[tauri::command]
pub async fn list_watch_marks(
    state: State<'_, AppState>,
    env_id: Option<String>,
) -> Result<Vec<WatchMark>> {
    let env = state.resolve_environment(env_id.as_deref()).await?;
    Ok(state.watch.marks_for(&env.id).await)
}
