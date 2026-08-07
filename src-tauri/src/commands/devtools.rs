//! Diagnostics for the app itself.
//!
//! Everything here answers "is gebman behaving as expected" rather than
//! anything about EventBridge: where its files are, how large they have grown,
//! what it has logged, and what state its caches are in.

use crate::error::{Error, Result};
use crate::state::AppState;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};

/// A directory gebman writes to, with what it is currently using.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageLocation {
    /// Stable id the UI passes back to `open_app_path`.
    pub id: String,
    pub label: String,
    pub path: String,
    pub exists: bool,
    pub bytes: u64,
    pub files: usize,
    /// What lives here, so the label is not the only explanation.
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    pub app_version: String,
    pub tauri_version: String,
    pub os: String,
    pub arch: String,
    /// `debug` or `release` — which build of the Rust side is running.
    pub profile: &'static str,
    pub log_level: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheInfo {
    /// Cached (profile, region) SDK configs.
    pub sdk_configs: usize,
    /// Cached event types and total events.
    pub event_types: usize,
    pub cached_events: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevInfo {
    pub runtime: RuntimeInfo,
    pub storage: Vec<StorageLocation>,
    pub caches: CacheInfo,
    /// Path of the file logs are written to, when there is one.
    pub log_file: Option<String>,
}

/// Subdirectories that live under our paths but are not ours.
///
/// The webview keeps its own cache inside the app cache dir and it grows with
/// ordinary browsing. Counting it would make "Cache" mean something other than
/// gebman's own footprint, which is the number this screen exists to show.
const NOT_OURS: &[&str] = &["WebKit", "WebView2", "webkitgtk"];

/// Recursively total a directory's size and file count.
///
/// Bounded by depth rather than unbounded recursion: these are our own
/// directories, but a symlink loop should not hang the diagnostics screen.
fn measure(path: &Path, depth: usize) -> (u64, usize) {
    if depth > 8 || !path.exists() {
        return (0, 0);
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return (0, 0);
    };

    let mut bytes = 0u64;
    let mut files = 0usize;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            let name = entry.file_name();
            if NOT_OURS.iter().any(|skip| *skip == name) {
                continue;
            }
            let (b, f) = measure(&entry.path(), depth + 1);
            bytes += b;
            files += f;
        } else {
            bytes += meta.len();
            files += 1;
        }
    }
    (bytes, files)
}

fn location(id: &str, label: &str, note: &str, path: PathBuf) -> StorageLocation {
    let (bytes, files) = measure(&path, 0);
    StorageLocation {
        id: id.to_string(),
        label: label.to_string(),
        note: note.to_string(),
        exists: path.exists(),
        path: path.to_string_lossy().into_owned(),
        bytes,
        files,
    }
}

fn app_paths(app: &AppHandle) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let resolver = app.path();
    Ok((
        resolver.app_config_dir().map_err(Error::internal)?,
        resolver.app_cache_dir().map_err(Error::internal)?,
        resolver.app_log_dir().map_err(Error::internal)?,
    ))
}

/// The file `tauri-plugin-log` writes to, if it exists yet.
fn log_file(log_dir: &Path) -> Option<PathBuf> {
    let candidates = ["gebman.log", "app.log"];
    for name in candidates {
        let path = log_dir.join(name);
        if path.exists() {
            return Some(path);
        }
    }
    // Fall back to whatever `.log` is in there.
    std::fs::read_dir(log_dir).ok().and_then(|entries| {
        entries
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().and_then(|e| e.to_str()) == Some("log"))
    })
}

#[tauri::command]
pub async fn dev_info(app: AppHandle, state: State<'_, AppState>) -> Result<DevInfo> {
    let (config_dir, cache_dir, log_dir) = app_paths(&app)?;
    let (event_types, cached_events) = state.events.stats().await;

    Ok(DevInfo {
        runtime: RuntimeInfo {
            app_version: app.package_info().version.to_string(),
            tauri_version: tauri::VERSION.to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            profile: if cfg!(debug_assertions) { "debug" } else { "release" },
            log_level: log::max_level().to_string(),
        },
        // Walking three directory trees is blocking I/O, and this command is
        // polled while the Developer tab is open — keep it off the runtime.
        storage: tokio::task::spawn_blocking({
            let log_dir = log_dir.clone();
            move || {
                vec![
                    location(
                        "config",
                        "Config",
                        "settings.json — environments, credentials, panel sizes",
                        config_dir,
                    ),
                    location(
                        "cache",
                        "Cache",
                        "event-samples.json — sampled events for reality checks",
                        cache_dir,
                    ),
                    location("logs", "Logs", "Application log files", log_dir),
                ]
            }
        })
        .await
        .map_err(Error::internal)?,
        caches: CacheInfo {
            sdk_configs: state.clients.count().await,
            event_types,
            cached_events,
        },
        log_file: log_file(&log_dir).map(|p| p.to_string_lossy().into_owned()),
    })
}

/// Read the last `wanted` lines of a file without loading the whole thing.
///
/// The log is capped at 5 MB and the Developer page polls it every few seconds,
/// so reading it end to end to show the last 300 lines would move megabytes a
/// minute. Seeks from the end instead, widening until enough newlines are in
/// hand or the whole file has been read.
fn tail_lines(path: &Path, wanted: usize) -> std::io::Result<Vec<String>> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();

    // Roughly a log line, doubled per attempt. Starting generous keeps the
    // common case to a single read. Never clamp with a floor above `len` —
    // `clamp` panics when min exceeds max, and a fresh log is a few bytes.
    let mut window = (wanted as u64)
        .saturating_mul(256)
        .max(8 * 1024)
        .min(len.max(1));

    loop {
        let start = len.saturating_sub(window);
        file.seek(SeekFrom::Start(start))?;
        let mut buf = Vec::with_capacity(window as usize);
        file.by_ref().take(window).read_to_end(&mut buf)?;

        let text = String::from_utf8_lossy(&buf);
        // Unless we started at byte 0, the first line is a fragment of a line
        // that began before the window.
        let mut lines: Vec<&str> = text.lines().collect();
        if start > 0 && !lines.is_empty() {
            lines.remove(0);
        }

        if lines.len() >= wanted || start == 0 {
            return Ok(lines
                .iter()
                .skip(lines.len().saturating_sub(wanted))
                .map(|l| l.to_string())
                .collect());
        }
        window = window.saturating_mul(2).min(len);
    }
}

/// Tail the application log.
///
/// Reads from the end so a log that has grown large does not have to be loaded
/// in full just to see what happened a moment ago.
#[tauri::command]
pub async fn read_app_logs(app: AppHandle, lines: Option<usize>) -> Result<Vec<String>> {
    let (_, _, log_dir) = app_paths(&app)?;
    let Some(path) = log_file(&log_dir) else {
        return Ok(Vec::new());
    };

    let wanted = lines.unwrap_or(500).clamp(10, 10_000);
    // Blocking file I/O: kept off the async runtime's worker threads.
    tokio::task::spawn_blocking(move || {
        tail_lines(&path, wanted)
            .map_err(|e| Error::Internal(format!("Could not read {}: {e}", path.display())))
    })
    .await
    .map_err(Error::internal)?
}

/// Write a line from the webview into the application log.
///
/// Without this the frontend's view of events — which command it called, how
/// long it waited, what error it rendered — lives only in the devtools console
/// and is gone the moment the window closes. Routing it here puts both halves
/// of every interaction in one ordered file.
#[tauri::command]
pub fn ui_log(level: String, category: String, message: String) {
    let category = resolve_category(&category);
    match level.as_str() {
        "error" => log::error!(target: category, "{message}"),
        "warn" => log::warn!(target: category, "{message}"),
        "debug" => log::debug!(target: category, "{message}"),
        "trace" => log::trace!(target: category, "{message}"),
        _ => log::info!(target: category, "{message}"),
    }
}

/// Map a category from the webview onto the known vocabulary.
///
/// The result becomes the log's target field, which the Developer tab filters
/// on — so an arbitrary string from the webview would create a category that
/// no filter chip can select, making those lines effectively invisible.
/// Unknown ones collapse to `ui`.
fn resolve_category(requested: &str) -> &'static str {
    crate::logging::cat::ALL
        .iter()
        .find(|c| **c == requested)
        .copied()
        .unwrap_or(crate::logging::cat::UI)
}

/// The category vocabulary, so the UI's filter list cannot drift from the
/// backend's.
#[tauri::command]
pub fn log_categories() -> Vec<&'static str> {
    crate::logging::cat::ALL.to_vec()
}

/// Reveal one of gebman's directories in the system file manager.
#[tauri::command]
pub async fn open_app_path(app: AppHandle, id: String) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;

    let (config_dir, cache_dir, log_dir) = app_paths(&app)?;
    let path = match id.as_str() {
        "config" => config_dir,
        "cache" => cache_dir,
        "logs" => log_dir,
        other => return Err(Error::Invalid(format!("Unknown location '{other}'"))),
    };

    std::fs::create_dir_all(&path).ok();
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| Error::Internal(format!("Could not open the folder: {e}")))
}

/// Delete the application log file.
#[tauri::command]
pub async fn clear_app_logs(app: AppHandle) -> Result<()> {
    let (_, _, log_dir) = app_paths(&app)?;
    if let Some(path) = log_file(&log_dir) {
        std::fs::write(&path, "")
            .map_err(|e| Error::Internal(format!("Could not clear {}: {e}", path.display())))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_category_from_the_webview_is_kept() {
        assert_eq!(resolve_category("ipc"), "ipc");
        assert_eq!(resolve_category("ui"), "ui");
        assert_eq!(resolve_category("cache"), "cache");
    }

    #[test]
    fn an_unknown_category_collapses_to_ui_rather_than_becoming_unfilterable() {
        // A category outside the vocabulary would render a chip nothing lists,
        // so those lines could never be filtered to.
        assert_eq!(resolve_category("whatever"), "ui");
        assert_eq!(resolve_category(""), "ui");
        assert_eq!(resolve_category("IPC"), "ui", "matching is exact, not case-insensitive");
    }

    #[test]
    fn every_category_survives_a_round_trip() {
        for category in crate::logging::cat::ALL {
            assert_eq!(resolve_category(category), *category);
        }
    }

    #[test]
    fn measuring_a_missing_directory_is_zero_not_an_error() {
        let (bytes, files) = measure(Path::new("/definitely/not/here"), 0);
        assert_eq!((bytes, files), (0, 0));
    }

    #[test]
    fn measures_files_recursively() {
        let base = std::env::temp_dir().join(format!("gebman-devtools-{}", std::process::id()));
        let nested = base.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(base.join("one.txt"), "12345").unwrap();
        std::fs::write(nested.join("two.txt"), "1234567890").unwrap();

        let (bytes, files) = measure(&base, 0);
        assert_eq!(files, 2);
        assert_eq!(bytes, 15);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Each test gets its own directory; timestamped names collide when tests
    /// run in parallel and one test's cleanup deletes another's files.
    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("gebman-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn tails_only_the_requested_lines() {
        let dir = temp_dir("tail");
        let path = dir.join("big.log");
        let body: String = (0..5_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, &body).unwrap();

        let tail = tail_lines(&path, 300).unwrap();
        assert_eq!(tail.len(), 300);
        assert_eq!(tail.first().unwrap(), "line 4700");
        assert_eq!(tail.last().unwrap(), "line 4999");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_partial_first_line_is_dropped_not_returned_truncated() {
        // The seek lands mid-line; returning "ne 4700" would be a silent
        // corruption of the oldest line shown.
        let dir = temp_dir("partial");
        let path = dir.join("wide.log");
        // Long lines force the initial window to land inside one.
        let body: String = (0..2_000).map(|i| format!("{i} {}\n", "x".repeat(400))).collect();
        std::fs::write(&path, &body).unwrap();

        let tail = tail_lines(&path, 10).unwrap();
        assert_eq!(tail.len(), 10);
        for (offset, line) in tail.iter().enumerate() {
            let expected = 1_990 + offset;
            assert!(
                line.starts_with(&format!("{expected} ")),
                "line {offset} was truncated: {:?}",
                &line[..line.len().min(20)]
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn asking_for_more_lines_than_exist_returns_the_whole_file() {
        let dir = temp_dir("short");
        let path = dir.join("small.log");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();

        let tail = tail_lines(&path, 500).unwrap();
        assert_eq!(tail, vec!["one", "two", "three"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_webview_cache_is_not_counted_as_ours() {
        // The webview keeps its cache inside our cache dir; counting it would
        // report someone else's growth as gebman's.
        let dir = temp_dir("webkit");
        std::fs::write(dir.join("event-samples.json"), "12345").unwrap();
        std::fs::create_dir_all(dir.join("WebKit").join("blobs")).unwrap();
        std::fs::write(dir.join("WebKit").join("blobs").join("junk"), "x".repeat(9_000)).unwrap();

        let (bytes, files) = measure(&dir, 0);
        assert_eq!((bytes, files), (5, 1));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stops_before_recursing_without_bound() {
        // Depth is capped so a symlink loop cannot hang diagnostics.
        let base = std::env::temp_dir().join(format!("gebman-depth-{}", std::process::id()));
        let mut deep = base.clone();
        for i in 0..12 {
            deep = deep.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("far.txt"), "x").unwrap();

        let (_, files) = measure(&base, 0);
        assert_eq!(files, 0, "a file past the depth cap must not be counted");

        let _ = std::fs::remove_dir_all(&base);
    }
}
