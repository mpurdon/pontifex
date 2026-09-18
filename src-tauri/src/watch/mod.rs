//! Watch mode: sit on the bus all day and say when something interesting
//! goes past.
//!
//! Passive by construction. The bus already writes every event to its
//! `/aws/events/*` log group through a catch-all rule, so a watch is nothing
//! more than a `FilterLogEvents` call repeated on a timer with a moving
//! cursor. No rule, queue or target is created; the Topology view stays clean;
//! and the laptop's side of the work is one small HTTP call per interval.
//!
//! - [`pattern`] compiles a watch into the filter CloudWatch evaluates, and
//!   re-evaluates it locally so several watches can share one call.
//! - [`store`] is the on-disk record: the watches, which environments are
//!   armed, the hits so far, and the cursor to resume from.
//! - [`runtime`] is the poller itself — one task per armed environment.

pub mod notify;
pub mod pattern;
pub mod runtime;
pub mod store;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use runtime::{WatchStatus, Watcher};
pub use store::WatchStore;

/// Show and focus the main window — for the idle check and for the Dock
/// icon on macOS, since a window closed while watching is hidden, not gone.
pub fn show_main_window(app: &tauri::AppHandle) -> crate::error::Result<()> {
    use tauri::Manager;
    if let Some(window) = app.get_webview_window("main") {
        window.show().map_err(crate::error::Error::internal)?;
        window.set_focus().map_err(crate::error::Error::internal)?;
    }
    Ok(())
}

/// A trimmed, non-empty string, or nothing: how every optional text field on
/// a watch is read, so blank and whitespace-only mean the same as absent.
pub(crate) fn trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

/// How a condition compares the field against its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WatchOp {
    Eq,
    Ne,
}

/// One `field = value` test on the event envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchCondition {
    /// Dotted path from the envelope root, e.g. `detail.clientId`. A leading
    /// `$.` is optional.
    pub path: String,
    pub op: WatchOp,
    /// Written as typed. Unquoted numbers compare numerically; `*` is a
    /// wildcard inside strings; wrapping in quotes forces a string match.
    pub value: String,
}

/// Something to be told about.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Watch {
    pub id: String,
    pub env_id: String,
    pub label: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Which of the environment's log groups to poll. Defaults to the first
    /// one that carries the bus's own events.
    #[serde(default)]
    pub log_group: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub detail_type: Option<String>,
    #[serde(default)]
    pub conditions: Vec<WatchCondition>,
    /// A complete CloudWatch filter pattern. Overrides the structured fields
    /// and is always polled in its own call, since it cannot be `||`-ed
    /// safely with others.
    #[serde(default)]
    pub raw_pattern: Option<String>,
    /// Raise a desktop notification on a hit, on top of recording it.
    #[serde(default = "default_true")]
    pub notify: bool,
    /// A CSS colour for this watch's hits; the UI picks one when unset.
    /// Purely presentational; the backend stores it and nothing more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Watch {
    /// The raw filter pattern, when one is set. A watch with one is polled
    /// on its own and never `||`-ed with others.
    pub fn raw(&self) -> Option<&str> {
        trimmed(self.raw_pattern.as_deref())
    }

    /// A one-line description for lists and notifications.
    pub fn summary(&self) -> String {
        if let Some(raw) = self.raw() {
            return raw.to_string();
        }
        let mut parts: Vec<String> = Vec::new();
        parts.extend(trimmed(self.source.as_deref()).map(str::to_string));
        parts.extend(trimmed(self.detail_type.as_deref()).map(str::to_string));
        for c in &self.conditions {
            let Some(path) = trimmed(Some(&c.path)) else {
                continue;
            };
            let op = match c.op {
                WatchOp::Eq => "=",
                WatchOp::Ne => "≠",
            };
            parts.push(format!("{path} {op} {}", c.value.trim()));
        }
        if parts.is_empty() {
            "every event".to_string()
        } else {
            parts.join(" · ")
        }
    }

    /// A watch with nothing set, for tests.
    #[cfg(test)]
    pub fn blank(env_id: &str) -> Self {
        Watch {
            id: "w".into(),
            env_id: env_id.into(),
            label: "w".into(),
            enabled: true,
            log_group: None,
            source: None,
            detail_type: None,
            conditions: Vec::new(),
            raw_pattern: None,
            notify: true,
            color: None,
        }
    }
}

/// A moment watching started or stopped, kept so the hit list can show the
/// sessions as groups: everything between a start and the next stop was
/// watched live; everything else was found by a look-back or not at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchMark {
    pub id: String,
    pub env_id: String,
    pub kind: WatchMarkKind,
    /// Epoch milliseconds.
    pub at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WatchMarkKind {
    Start,
    Stop,
}

/// One event that satisfied a watch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchHit {
    pub id: String,
    pub watch_id: String,
    pub watch_label: String,
    pub env_id: String,
    pub log_group: String,
    /// The EventBridge event id, or the CloudWatch event id when the envelope
    /// carried none. Always present; it is what de-duplicates re-reads.
    pub event_id: String,
    /// The log event's timestamp, epoch milliseconds.
    pub timestamp: i64,
    /// When the poller saw it, epoch milliseconds.
    pub received_at: i64,
    pub source: Option<String>,
    pub detail_type: Option<String>,
    /// The full envelope as it landed in CloudWatch.
    pub event: Value,
    /// Recorded by the look-back pass that runs when watching starts, so the
    /// screen is not empty on arrival. Backfilled hits never notify.
    #[serde(default)]
    pub backfill: bool,
}
