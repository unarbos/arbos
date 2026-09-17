//! A notification on the desktop itself, for when the window is not the
//! one the person is looking at: an agent replied, asked, or failed while
//! they were elsewhere (#293). Cursor posts the same when an agent finishes
//! or asks in the background.
//!
//! Through the platform's command rather than a framework: `notify-send`
//! (libnotify) on Linux, `osascript`'s `display notification` on macOS. The
//! macOS path shows the notification under Script Editor's name until the
//! bundle registers with UNUserNotificationCenter, which needs a signed
//! build (see `permissions.rs`); the words still arrive. Fire and forget.

use std::process::{Command, Stdio};

/// Which command carries the notification on this platform.
pub const NOTIFIER: &str = if cfg!(target_os = "macos") {
    "osascript"
} else {
    "notify-send"
};

/// Post it. `Ok` means the command was started, not that a daemon showed
/// it; the driver reports the result so a test can check the daemon's own
/// record (dunst's history on the rig) against what the window claims.
pub fn post(title: &str, body: &str) -> Result<(), String> {
    let title = clip(title, 80);
    let body = clip(body, 240);
    let spawned = if cfg!(target_os = "macos") {
        Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "display notification \"{}\" with title \"{}\"",
                applescript(&body),
                applescript(&title)
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    } else {
        Command::new("notify-send")
            .arg("--app-name=Arbos")
            .arg(&title)
            .arg(&body)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };
    match spawned {
        Ok(_) => Ok(()),
        Err(err) => {
            eprintln!("notification: {err}");
            Err(err.to_string())
        }
    }
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim().replace('\n', " ");
    if text.chars().count() <= max {
        return text;
    }
    let cut: String = text.chars().take(max - 1).collect();
    format!("{cut}…")
}

/// Inside an AppleScript string literal: a backslash and a quote escaped.
fn applescript(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}
