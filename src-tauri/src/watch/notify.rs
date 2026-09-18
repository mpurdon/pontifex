//! Getting a notification onto the screen.
//!
//! On macOS 26 the old `NSUserNotificationCenter` route — what the Tauri
//! notification plugin, `terminal-notifier` and `osascript display
//! notification` all use — reports success and shows nothing. The modern
//! `UNUserNotificationCenter` API works, but only from a real app bundle with
//! its own identifier, which a `tauri dev` binary is not.
//!
//! So notifications go through a tiny helper app (`notifier/main.swift`,
//! shipped as a bundle resource). On first use it is copied into the app's
//! data directory, registered with LaunchServices, and launched through
//! `open` with the text as arguments — launching the binary directly makes
//! macOS attribute it to the parent process and refuse it. The helper posts
//! one notification and exits, writing its outcome beside the app log so the
//! Test button can report what actually happened.

use crate::error::{Error, Result};
use crate::logging::cat;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const HELPER_NAME: &str = "Pontifex Notifier.app";
const BASE_BUNDLE_ID: &str = "dev.codenaked.pontifex.notify";
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// What the helper reported after the last notification.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifierOutcome {
    pub status: NotifierStatus,
    pub message: String,
    /// Where the helper is installed, for the Developer tab.
    pub helper: String,
}

/// The helper's verdict, as it writes it to `notifier-last.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifierStatus {
    Delivered,
    Denied,
    Error,
    Timeout,
    /// No answer yet — a permission prompt is probably waiting.
    Pending,
}

/// The file the helper writes its verdict to.
#[derive(Debug, Clone, serde::Deserialize)]
struct HelperVerdict {
    status: NotifierStatus,
    message: String,
    /// Epoch milliseconds, so a fresh verdict can be told from the last one.
    at: f64,
}

/// The built helper, wherever this build keeps it.
///
/// A bundled app has it under Resources. A development binary has no bundle,
/// so it falls back to the build output in the source tree.
fn helper_source(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(resources) = app.path().resource_dir() {
        candidates.push(resources.join("notifier").join("build").join(HELPER_NAME));
        candidates.push(resources.join(HELPER_NAME));
    }
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("notifier")
            .join("build")
            .join(HELPER_NAME),
    );
    candidates
        .into_iter()
        .find(|p| p.join("Contents").join("MacOS").join("notifier").is_file())
}

fn helper_dest(app: &AppHandle) -> Result<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| Error::Internal(format!("no app data dir: {e}")))?
        .join(HELPER_NAME))
}

/// One string value out of a bundle's Info.plist, read as text: the plist is
/// the one the build script writes, and a parser for two keys is more code
/// than the scan.
fn plist_string(bundle: &Path, key: &str) -> Option<String> {
    let plist = std::fs::read_to_string(bundle.join("Contents").join("Info.plist")).ok()?;
    let after = plist.split(&format!("<key>{key}</key>")).nth(1)?;
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")? + start;
    Some(after[start..end].to_string())
}

fn bundle_version(bundle: &Path) -> Option<String> {
    plist_string(bundle, "CFBundleVersion")
}

fn installed_bundle_id(bundle: &Path) -> Option<String> {
    plist_string(bundle, "CFBundleIdentifier")
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// The bundle identifier for a generation. Generation 0 is the one shipped;
/// each "ask again" moves to a new one, because macOS ties its refusal to the
/// identifier and offers no way to ask a refused identifier again.
fn bundle_id(generation: u32) -> String {
    if generation == 0 {
        BASE_BUNDLE_ID.to_string()
    } else {
        format!("{BASE_BUNDLE_ID}.g{generation}")
    }
}

/// The installed helper, installing or refreshing it first when needed.
///
/// Installed into the app's own data directory rather than run from the
/// resources: the bundle needs a stable, registered location for macOS to
/// treat it as an app in its own right, and a fresh build must replace a
/// stale copy or the permission macOS granted stays tied to the old one.
/// `generation` selects the bundle identifier — see [`bundle_id`].
pub fn ensure_installed(app: &AppHandle, generation: u32) -> Result<PathBuf> {
    let source = helper_source(app).ok_or_else(|| {
        Error::Internal(
            "The notification helper was not built. Run scripts/build-notifier.sh (needs swiftc) and restart."
                .into(),
        )
    })?;
    let dest = helper_dest(app)?;
    let wanted = bundle_version(&source);
    let wanted_id = bundle_id(generation);
    let current_id = installed_bundle_id(&dest);
    if !dest.exists()
        || bundle_version(&dest) != wanted
        || current_id.as_deref() != Some(&wanted_id)
    {
        if dest.exists() {
            std::fs::remove_dir_all(&dest)?;
        }
        copy_dir(&source, &dest)?;
        if wanted_id != BASE_BUNDLE_ID {
            // Rewrite the identifier and re-sign: a changed Info.plist
            // invalidates the ad-hoc signature, and an unsigned bundle is
            // refused outright.
            let plist_path = dest.join("Contents").join("Info.plist");
            let plist = std::fs::read_to_string(&plist_path)?;
            std::fs::write(&plist_path, plist.replace(BASE_BUNDLE_ID, &wanted_id))?;
            let signed = std::process::Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(&dest)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !signed {
                lwarn!(
                    cat::WATCH,
                    "could not re-sign the notification helper; macOS may refuse it"
                );
            }
        }
        linfo!(
            cat::WATCH,
            "installed notification helper {} as {wanted_id} at {}",
            wanted.as_deref().unwrap_or("?"),
            dest.display()
        );
        // Tell LaunchServices about it; without this the first `open` can
        // fail to find the bundle by path on a fresh install.
        let _ = std::process::Command::new(LSREGISTER)
            .arg("-f")
            .arg(&dest)
            .status();
    }
    Ok(dest)
}

/// Where the helper writes its outcome.
fn last_outcome_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app
        .path()
        .app_log_dir()
        .map_err(|e| Error::Internal(format!("no log dir: {e}")))?
        .join("notifier-last.json"))
}

fn read_outcome(path: &Path) -> Option<HelperVerdict> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Whether a notification was launched, or held back.
pub enum Sent {
    Launched,
    /// A helper instance is alive and waiting on the permission prompt.
    /// Launching another would raise a second request against the same
    /// prompt, which macOS refuses.
    HeldForPrompt,
}

async fn prompt_pending(app: &AppHandle) -> bool {
    let pending = last_outcome_path(app)
        .ok()
        .and_then(|p| read_outcome(&p))
        .is_some_and(|v| v.status == NotifierStatus::Pending);
    if !pending {
        return false;
    }
    tokio::process::Command::new("/usr/bin/pgrep")
        .arg("-f")
        .arg(format!("{HELPER_NAME}/Contents/MacOS/notifier"))
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Show a notification. Returns once macOS has taken the launch request; the
/// helper itself finishes on its own.
pub async fn send(app: &AppHandle, title: &str, body: &str) -> Result<Sent> {
    if prompt_pending(app).await {
        linfo!(
            cat::WATCH,
            "notification held: the permission prompt is still waiting to be answered"
        );
        return Ok(Sent::HeldForPrompt);
    }
    let generation = app
        .state::<crate::state::AppState>()
        .watch
        .notifier_generation()
        .await;
    let helper = ensure_installed(app, generation)?;
    // `-g`: do not bring the helper to the foreground. A foreground app's own
    // notifications are suppressed unless it opts in, and the helper should
    // never steal focus from whatever the person is doing anyway.
    let output = tokio::process::Command::new("/usr/bin/open")
        .arg("-g")
        .arg("-n")
        .arg("-a")
        .arg(&helper)
        .arg("--args")
        .arg(title)
        .arg(body)
        .arg("default")
        .output()
        .await
        .map_err(|e| Error::Internal(format!("could not launch the notification helper: {e}")))?;
    if !output.status.success() {
        return Err(Error::Internal(format!(
            "notification helper launch failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    ldebug!(cat::WATCH, "notification: {title} — {body}");
    Ok(Sent::Launched)
}

/// Ask macOS again under a fresh identifier, then test.
///
/// For the case where the first prompt was missed: macOS withdraws an
/// unanswered request after a few minutes and records the app as refused.
/// System Settings is the proper way back; this is the way back when the
/// refused entry is not offered there.
pub async fn ask_again(app: &AppHandle) -> Result<NotifierOutcome> {
    let state = app.state::<crate::state::AppState>();
    let generation = state.watch.bump_notifier_generation().await?;
    linfo!(
        cat::WATCH,
        "notification helper moving to generation {generation}"
    );
    if let Ok(path) = last_outcome_path(app) {
        let _ = std::fs::remove_file(path);
    }
    // `test` installs the new generation on its way through `send`.
    test(app).await
}

/// Send a test and wait briefly for the helper's verdict.
pub async fn test(app: &AppHandle) -> Result<NotifierOutcome> {
    let outcome_path = last_outcome_path(app)?;
    let helper = helper_dest(app)?.display().to_string();
    let before = read_outcome(&outcome_path).map(|v| v.at).unwrap_or(0.0);
    let pending = |message: &str| NotifierOutcome {
        status: NotifierStatus::Pending,
        message: message.into(),
        helper: helper.clone(),
    };
    let launched = send(
        app,
        "Pontifex is watching",
        "This is what a hit looks like. If you can read this, notifications reach you.",
    )
    .await?;
    if let Sent::HeldForPrompt = launched {
        return Ok(pending(
            "The “Pontifex” Notifications prompt is still waiting at the top right of the main display. Click it and choose Allow.",
        ));
    }
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Some(verdict) = read_outcome(&outcome_path) {
            if verdict.at > before {
                return Ok(NotifierOutcome {
                    status: verdict.status,
                    message: verdict.message,
                    helper,
                });
            }
        }
    }
    Ok(pending(
        "No answer from macOS yet. If a “Pontifex” Notifications prompt is showing, click it and choose Allow, then test again.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_bundle_version_out_of_a_plist() {
        let dir = std::env::temp_dir().join(format!("pontifex-notifier-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("Contents")).unwrap();
        std::fs::write(
            dir.join("Contents").join("Info.plist"),
            "<plist><dict><key>CFBundleName</key><string>Pontifex</string><key>CFBundleVersion</key><string>abc123</string></dict></plist>",
        )
        .unwrap();
        assert_eq!(bundle_version(&dir).as_deref(), Some("abc123"));
        std::fs::remove_dir_all(dir).ok();
    }
}
