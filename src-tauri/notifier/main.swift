// Pontifex Notifier — the one route to the screen that still works.
//
// macOS 26 stopped delivering notifications posted through the old
// NSUserNotificationCenter API, which is what the Tauri notification plugin,
// terminal-notifier and `osascript display notification` all use. They report
// success; nothing appears. The modern UNUserNotificationCenter API works, but
// only from a process that is a real app bundle with its own identifier, and
// a `tauri dev` binary is not one. So this is a minimal app bundle that does
// exactly one thing: post one notification, wait for macOS to accept it, and
// exit. Pontifex launches it through LaunchServices with the text as arguments.
//
// Usage: notifier <title> <body> [sound|none]
// Launched with no arguments (macOS relaunches it when a notification is
// clicked), it brings Pontifex to the front and exits.
//
// It writes its outcome to ~/Library/Logs/dev.codenaked.pontifex/notifier.log
// and, for the Test button, to notifier-last.json beside it.

import AppKit
import Foundation
import UserNotifications

let args = CommandLine.arguments
let fm = FileManager.default
let logDir = fm.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/dev.codenaked.pontifex")
try? fm.createDirectory(at: logDir, withIntermediateDirectories: true)
let logURL = logDir.appendingPathComponent("notifier.log")
let lastURL = logDir.appendingPathComponent("notifier-last.json")

func log(_ line: String) {
    let text = "\(ISO8601DateFormatter().string(from: Date())) \(line)\n"
    if let h = try? FileHandle(forWritingTo: logURL) {
        h.seekToEndOfFile(); h.write(text.data(using: .utf8)!); h.closeFile()
    } else {
        try? text.write(to: logURL, atomically: true, encoding: .utf8)
    }
}

/// The outcome Pontifex reads back after a test.
func record(_ status: String, _ message: String) {
    let payload: [String: Any] = ["status": status, "message": message, "at": Date().timeIntervalSince1970 * 1000]
    if let data = try? JSONSerialization.data(withJSONObject: payload) {
        try? data.write(to: lastURL)
    }
    log("\(status): \(message)")
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)

/// Bring the main app forward. In a bundled install that is Pontifex.app; in
/// development it is a bare `pontifex` process, found by name.
func activatePontifex() {
    let running = NSWorkspace.shared.runningApplications
    let target = running.first { $0.bundleIdentifier == "dev.codenaked.pontifex" }
        ?? running.first { $0.localizedName?.lowercased() == "pontifex" }
    if let target = target {
        target.activate(options: [.activateAllWindows])
    } else if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "dev.codenaked.pontifex") {
        NSWorkspace.shared.openApplication(at: url, configuration: NSWorkspace.OpenConfiguration())
    }
}

/// Clicking a notification relaunches this helper with no arguments and
/// delivers the response here.
final class ClickDelegate: NSObject, UNUserNotificationCenterDelegate {
    /// macOS asks a *running* app whether to show its own notification, and
    /// without an answer shows nothing. This helper is running for exactly
    /// the instant the banner is due, so the answer must be given: banner,
    /// sound, and keep it in the list.
    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .sound, .list])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                didReceive response: UNNotificationResponse,
                                withCompletionHandler completionHandler: @escaping () -> Void) {
        activatePontifex()
        completionHandler()
        exit(0)
    }
}
let delegate = ClickDelegate()
let center = UNUserNotificationCenter.current()
center.delegate = delegate

if args.count < 3 {
    // Relaunched by a click (or run by hand): focus the app and go.
    activatePontifex()
    // Give the response callback a moment to arrive if this was a click.
    RunLoop.main.run(until: Date().addingTimeInterval(1.0))
    exit(0)
}

let title = args[1]
let body = args[2]
let sound = args.count > 3 ? args[3] : "default"
let done = DispatchSemaphore(value: 0)
var exitCode: Int32 = 0
var finished = false
func finish(_ code: Int32) { exitCode = code; finished = true; done.signal() }

/// Post the notification once permission is known to be granted.
func post() {
    let content = UNMutableNotificationContent()
    content.title = title
    content.body = body
    if sound != "none" { content.sound = .default }
    let request = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)
    center.add(request) { error in
        if let error = error {
            record("error", "could not post: \(error.localizedDescription)")
            finish(5)
        } else {
            record("delivered", "handed to Notification Center")
            // Stay alive a moment so the presentation callback above is
            // answered before the process goes away.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.75) { finish(0) }
        }
    }
}

/// A request that is never answered leaves the prompt on screen and later
/// requests refused until it is, so the state is checked first and the
/// prompt is only raised when macOS has genuinely not asked yet.
var waitingOnPrompt = false
center.getNotificationSettings { settings in
    switch settings.authorizationStatus {
    case .authorized, .provisional:
        post()
    case .denied:
        record("denied", "Notifications are turned off for Pontifex. Allow them in System Settings → Notifications → Pontifex.")
        finish(4)
    default:
        waitingOnPrompt = true
        record("pending", "Waiting for you to answer the “Pontifex” Notifications prompt at the top right of the main display. macOS withdraws it after a few minutes, so answer it now.")
        center.requestAuthorization(options: [.alert, .sound, .badge]) { granted, error in
            if let error = error {
                record("error", "authorization failed: \(error.localizedDescription)")
                finish(3); return
            }
            guard granted else {
                record("denied", "Notifications are turned off for Pontifex. Allow them in System Settings → Notifications → Pontifex.")
                finish(4); return
            }
            post()
        }
    }
}

// Pump the run loop until the callbacks land. While the permission prompt is
// up a person has to answer it, and exiting would abandon the prompt, so that
// case waits far longer.
let started = Date()
func deadline() -> Date { started.addingTimeInterval(waitingOnPrompt ? 6 * 60 * 60 : 30) }
while !finished && Date() < deadline() {
    RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.05))
}
if !finished {
    record("timeout", "no answer from macOS — the permission prompt may still be waiting")
    exitCode = 6
}
exit(exitCode)
