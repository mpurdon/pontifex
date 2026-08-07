//! Categories and helpers for the application log.
//!
//! The log had seven statements in the whole backend, which made the Developer
//! tab a window onto nothing. The problem with fixing that by logging more is
//! volume: a log that records everything is only useful if you can narrow it
//! back down, so every line is tagged with a category and the UI filters on it.
//!
//! Category rides in the `log` crate's `target` field. That field is normally
//! the module path — accurate but useless for filtering, since it splits
//! "anything to do with AWS" across a dozen modules and changes whenever a file
//! moves. A fixed vocabulary is stable and matches how you actually search.

/// The category vocabulary. Anything logged outside this list cannot be
/// filtered from the UI, so add a constant rather than passing a literal.
pub mod cat {
    /// Startup, shutdown, settings load — the app's own lifecycle.
    pub const APP: &str = "app";
    /// Command entry and exit, with timings. The spine of the log.
    pub const IPC: &str = "ipc";
    /// Anything the webview logs, forwarded so it lands in the same file.
    pub const UI: &str = "ui";
    /// Credential resolution, STS identity, SDK config caching.
    pub const AWS: &str = "aws";
    /// Device-code flow, token cache, session discovery.
    pub const SSO: &str = "sso";
    /// EventBridge Schemas API: list, describe, create, update, delete.
    pub const REGISTRY: &str = "registry";
    /// Parsing, validating, simplifying and inferring schema documents.
    pub const SCHEMA: &str = "schema";
    /// CloudWatch scans and reality checks against real traffic.
    pub const EVENTS: &str = "events";
    /// The on-disk event sample cache.
    pub const CACHE: &str = "cache";
    /// Bedrock model calls.
    pub const BEDROCK: &str = "bedrock";
    /// Bus and rule topology.
    pub const TOPOLOGY: &str = "topology";
    /// Reading and writing settings.
    pub const SETTINGS: &str = "settings";

    /// Filing producer bugs: OAuth, ticket creation, routing.
    pub const JIRA: &str = "jira";

    /// Every category, for the UI's filter list and for validation.
    pub const ALL: &[&str] = &[
        APP, IPC, UI, AWS, SSO, REGISTRY, SCHEMA, EVENTS, CACHE, BEDROCK, TOPOLOGY, SETTINGS,
    ];
}

/// Log at a category. Mirrors `log::info!` etc. but requires a category.
///
/// `log::info!(target: cat::AWS, "…")` already works; these wrappers exist so
/// the category argument cannot be forgotten and so call sites read uniformly.
#[macro_export]
macro_rules! log_at {
    ($level:expr, $category:expr, $($arg:tt)+) => {
        log::log!(target: $category, $level, $($arg)+)
    };
}

#[macro_export]
macro_rules! linfo {
    ($category:expr, $($arg:tt)+) => { log::info!(target: $category, $($arg)+) };
}

#[macro_export]
macro_rules! lwarn {
    ($category:expr, $($arg:tt)+) => { log::warn!(target: $category, $($arg)+) };
}

#[macro_export]
macro_rules! lerror {
    ($category:expr, $($arg:tt)+) => { log::error!(target: $category, $($arg)+) };
}

#[macro_export]
macro_rules! ldebug {
    ($category:expr, $($arg:tt)+) => { log::debug!(target: $category, $($arg)+) };
}

/// Times a unit of work and logs how long it took.
///
/// Duration is the single most useful thing a log can carry here — nearly every
/// question about this app ("is it hung, is it slow, did it hit the cache") is
/// really a question about elapsed time.
pub struct Timed {
    category: &'static str,
    what: String,
    started: std::time::Instant,
}

impl Timed {
    pub fn start(category: &'static str, what: impl Into<String>) -> Self {
        let what = what.into();
        log::debug!(target: category, "{what} — started");
        Timed {
            category,
            what,
            started: std::time::Instant::now(),
        }
    }

    pub fn elapsed_ms(&self) -> u128 {
        self.started.elapsed().as_millis()
    }

    /// Finish with a note. Slow work is logged louder, because a log you have
    /// to read every line of to spot a stall is not doing its job.
    pub fn done(self, detail: impl std::fmt::Display) {
        let ms = self.elapsed_ms();
        if ms >= 3_000 {
            log::warn!(target: self.category, "{} — {detail} ({ms}ms, slow)", self.what);
        } else {
            log::info!(target: self.category, "{} — {detail} ({ms}ms)", self.what);
        }
    }

    pub fn failed(self, error: impl std::fmt::Display) {
        let ms = self.elapsed_ms();
        log::error!(target: self.category, "{} — failed after {ms}ms: {error}", self.what);
    }
}

/// Parse a level name from settings. Unknown values fall back to `Info` rather
/// than refusing to start.
pub fn level_from_str(name: &str) -> log::LevelFilter {
    match name.trim().to_ascii_lowercase().as_str() {
        "off" => log::LevelFilter::Off,
        "error" => log::LevelFilter::Error,
        "warn" => log::LevelFilter::Warn,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_is_lowercase_and_unique() {
        // The UI filters on exact strings, and renders them as-is.
        let mut seen = std::collections::HashSet::new();
        for c in cat::ALL {
            assert_eq!(*c, c.to_ascii_lowercase(), "{c} should be lowercase");
            assert!(!c.contains(' '), "{c} should not contain spaces");
            assert!(seen.insert(*c), "{c} is listed twice");
        }
    }

    #[test]
    fn level_names_round_trip() {
        assert_eq!(level_from_str("trace"), log::LevelFilter::Trace);
        assert_eq!(level_from_str("DEBUG"), log::LevelFilter::Debug);
        assert_eq!(level_from_str(" warn "), log::LevelFilter::Warn);
        assert_eq!(level_from_str("off"), log::LevelFilter::Off);
    }

    #[test]
    fn an_unknown_level_falls_back_to_info_rather_than_failing() {
        assert_eq!(level_from_str("verbose"), log::LevelFilter::Info);
        assert_eq!(level_from_str(""), log::LevelFilter::Info);
    }
}
