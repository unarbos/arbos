//! What a crash leaves behind. A Rust panic on the main thread unwinds into AppKit and ends
//! as `abort()` — the macOS crash report then shows `SIGABRT` with stripped frames and no
//! panic message (Jacob's 0.6.268 reports of 2026-10-26, incidents 080D4A8B… and A9065D42…:
//! two headers, nothing to go on). The hook installed by [`install`] writes the panic message,
//! its source location and a captured backtrace to `~/Library/Logs/Arbos/panic.log` before the
//! abort, and the next launch reports the newest entry once through the silent-diagnostic
//! feedback path ([`take_unreported`]), so the relay has the trace even when nobody finds the
//! `.ips`. Best effort everywhere: nothing here may panic or block.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

/// `~/Library/Logs/Arbos/panic.log` (macOS), `$XDG_STATE_HOME/arbos/panic.log` elsewhere.
///
/// In test builds the path is under [`std::env::temp_dir`] (`arbos-test-<pid>/panic.log`) so a
/// deliberate test panic never lands in the real Logs folder and is not uploaded on the next
/// launch of an installed app.
pub fn log_path() -> PathBuf {
    #[cfg(test)]
    {
        std::env::temp_dir()
            .join(format!("arbos-test-{}", std::process::id()))
            .join("panic.log")
    }
    #[cfg(not(test))]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        if cfg!(target_os = "macos") {
            home.join("Library/Logs/Arbos/panic.log")
        } else {
            std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/state"))
                .join("arbos/panic.log")
        }
    }
}

/// Marker next to the log: the byte length of `panic.log` the last report covered.
fn reported_marker() -> PathBuf {
    log_path().with_extension("reported")
}

const ENTRY_START: &str = "==== panic ";
/// At most this much of the log is kept (the newest entries); old crashes roll off.
const MAX_LOG_BYTES: u64 = 512 * 1024;

/// Install the hook. Called first thing in `main`, before any thread or window exists. The
/// previous hook (Rust's default: message + backtrace on stderr) still runs after ours.
pub fn install() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            record(info);
            previous(info);
        }));
    });
}

/// One entry: header line, version and OS, the panic message, the backtrace, a blank line.
fn record(info: &std::panic::PanicHookInfo<'_>) {
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_owned());
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown location>".to_owned());
    let thread = std::thread::current();
    let backtrace = std::backtrace::Backtrace::force_capture();
    let entry = format!(
        "{ENTRY_START}{} ====\nversion {} · {}\nthread {}\nat {location}\n{message}\n{backtrace}\n\n",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ"),
        crate::update::current_version(),
        crate::update::os_version(),
        thread.name().unwrap_or("<unnamed>"),
    );
    log::error!(target: "arbos_mac::panic", "{location}: {message}");
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(len) = fs::metadata(&path).map(|m| m.len()) {
        if len > MAX_LOG_BYTES {
            // Keep the newest half: drop whole entries from the front.
            if let Ok(old) = fs::read_to_string(&path) {
                let keep_from = old.len().saturating_sub((MAX_LOG_BYTES / 2) as usize);
                let cut = old[keep_from..]
                    .find(ENTRY_START)
                    .map(|i| keep_from + i)
                    .unwrap_or(keep_from);
                let _ = fs::write(&path, &old[cut..]);
                let _ = fs::remove_file(reported_marker());
            }
        }
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(entry.as_bytes());
    }
}

/// True when this entry is from a deliberate unit/smoke-test panic, not a real crash.
/// Those must not go through the silent-diagnostic path — otherwise running the
/// transcript smoke test leaves a line in `panic.log` that the next app launch reports
/// as feedback (app-01M5WZDJQ5D1S90566WD5FMJQ1: Ken's Mac, "test poison event").
fn is_test_entry(entry: &str) -> bool {
    // Message line written by AppState::apply_inner's #[cfg(test)] poison assert.
    if entry.lines().any(|l| l.trim() == "test poison event") {
        return true;
    }
    // Rust test threads are named like `ui::transcript::transcript_smoke_tests::…`.
    entry.lines().any(|l| {
        let l = l.trim();
        l.starts_with("thread ")
            && (l.contains("_tests::") || l.contains("::tests::") || l.contains("smoke_tests"))
    })
}

/// The newest panic entry not yet reported (if any), and mark the log as reported up to its
/// current length. Called once per launch when the relay session is up; the caller sends it
/// through the silent-diagnostic feedback path.
pub fn take_unreported() -> Option<String> {
    let path = log_path();
    let text = fs::read_to_string(&path).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    let reported_to: usize = fs::read_to_string(reported_marker())
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let _ = fs::write(reported_marker(), text.len().to_string());
    if reported_to >= text.len() {
        return None;
    }
    let fresh = text.get(reported_to.min(text.len())..).unwrap_or("");
    let last = fresh
        .rfind(ENTRY_START)
        .map(|i| &fresh[i..])
        .unwrap_or(fresh);
    let last = last.trim();
    if last.is_empty() {
        return None;
    }
    if is_test_entry(last) {
        return None;
    }
    // The feedback note has a size limit; keep the head (message, location, top frames).
    Some(last.chars().take(6_000).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_log_path_is_under_temp_dir_not_home() {
        let path = log_path();
        let temp = std::env::temp_dir();
        assert!(
            path.starts_with(&temp),
            "test log_path must be under temp_dir ({temp:?}), got {path:?}"
        );
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            assert!(
                !path.starts_with(&home),
                "test log_path must not be under HOME ({home:?}), got {path:?}"
            );
        }
    }

    #[test]
    fn an_entry_is_reported_once_and_the_next_one_again() {
        let path = log_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).unwrap();
        }
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(reported_marker());
        assert_eq!(take_unreported(), None, "no log, nothing to report");
        fs::write(
            &path,
            format!("{ENTRY_START}2026-10-26T15:00:00Z ====\nversion 0.6.268\nat a.rs:1:1\nindex out of bounds\n   0: frame\n\n"),
        )
        .unwrap();
        let first = take_unreported().expect("the entry");
        assert!(first.contains("index out of bounds") && first.contains("a.rs:1:1"));
        assert_eq!(take_unreported(), None, "reported once");
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(
            format!(
                "{ENTRY_START}2026-10-26T16:00:00Z ====\nversion 0.6.269\nat b.rs:2:2\nsecond\n\n"
            )
            .as_bytes(),
        )
        .unwrap();
        let second = take_unreported().expect("the new entry");
        assert!(second.contains("second") && !second.contains("index out of bounds"));
        // A smoke-test poison entry is marked reported but never returned — so a cargo test
        // run cannot open a silent-diagnostic feedback item on the next real launch.
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(
            format!(
                "{ENTRY_START}2026-10-26T20:11:34Z ====\nversion 0.6.276 · 27.0.1\n\
                 thread ui::transcript::transcript_smoke_tests::a_panicking_sync_event_is_skipped_not_fatal\n\
                 at crates/arbos-mac/src/app.rs:1439:13\ntest poison event\n   0: frame\n\n"
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(
            take_unreported(),
            None,
            "test poison must not become a silent diagnostic"
        );
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(reported_marker());
    }
}
