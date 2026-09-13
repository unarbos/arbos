//! On-device dictation for the composer microphone.
//!
//! The mic is on this Mac — the window — not on a remote kernel. A tiny
//! signed app bundle talks to Apple Speech. Live partials land in a file.
//! Stop writes a sentinel and waits for the transcript.

use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use std::hash::{Hash, Hasher};
#[cfg(target_os = "macos")]
use std::process::Command;

const SWIFT: &str = include_str!("voice_dictate.swift");

const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>dictate</string>
  <key>CFBundleIdentifier</key>
  <string>com.unarbos.arbos.voice</string>
  <key>CFBundleName</key>
  <string>arbos voice</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>CFBundleShortVersionString</key>
  <string>1.0</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>arbos transcribes your speech into the chat composer.</string>
  <key>NSSpeechRecognitionUsageDescription</key>
  <string>arbos transcribes your speech into the chat composer.</string>
</dict>
</plist>
"#;

const STOP_TIMEOUT: Duration = Duration::from_secs(30);
const DONE_POLL: Duration = Duration::from_millis(100);

struct Rec {
    dir: PathBuf,
    out: PathBuf,
    stop: PathBuf,
}

fn hold() -> &'static Mutex<Option<Rec>> {
    static HOLD: OnceLock<Mutex<Option<Rec>>> = OnceLock::new();
    HOLD.get_or_init(|| Mutex::new(None))
}

/// Begin capture. A leftover take from a crashed start is dropped first.
pub fn start() -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    {
        Err(anyhow!("voice dictation is macOS-only for now"))
    }
    #[cfg(target_os = "macos")]
    {
        let mut hold = hold().lock().map_err(|_| anyhow!("voice lock poisoned"))?;
        if let Some(old) = hold.take() {
            abort(old);
        }
        *hold = Some(launch()?);
        Ok(())
    }
}

/// Latest partial text. Empty when idle or before the first word.
pub fn peek() -> Result<String> {
    let hold = hold().lock().map_err(|_| anyhow!("voice lock poisoned"))?;
    let Some(rec) = hold.as_ref() else {
        return Ok(String::new());
    };
    Ok(fs::read_to_string(&rec.out)
        .unwrap_or_default()
        .trim()
        .to_string())
}

/// End capture and return the transcript.
pub fn stop() -> Result<String> {
    let mut hold = hold().lock().map_err(|_| anyhow!("voice lock poisoned"))?;
    let Some(rec) = hold.take() else {
        return Err(anyhow!("no recording in progress"));
    };
    drop(hold);
    finish(rec)
}

#[cfg(target_os = "macos")]
fn launch() -> Result<Rec> {
    let app = helper_app()?;
    let dir = tempfile_dir()?;
    let out = dir.join("transcript.txt");
    let stop = dir.join("stop");
    let combined = Command::new("open")
        .args(["-n", "-g"])
        .arg(&app)
        .args(["--args", "--out"])
        .arg(&out)
        .arg("--stop")
        .arg(&stop)
        .output()
        .context("launch voice helper")?;
    if !combined.status.success() {
        let _ = fs::remove_dir_all(&dir);
        return Err(anyhow!(
            "launch voice helper: {}",
            String::from_utf8_lossy(&combined.stderr).trim()
        ));
    }
    Ok(Rec { dir, out, stop })
}

fn finish(rec: Rec) -> Result<String> {
    let _ = fs::write(&rec.stop, b"");
    let done = format!("{}.done", rec.out.display());
    let errp = format!("{}.err", rec.out.display());
    let deadline = Instant::now() + STOP_TIMEOUT;
    while !Path::new(&done).exists() {
        if Instant::now() >= deadline {
            let _ = fs::remove_dir_all(&rec.dir);
            return Err(anyhow!("voice transcription timed out"));
        }
        thread::sleep(DONE_POLL);
    }
    let err = fs::read_to_string(&errp).ok();
    let text = fs::read_to_string(&rec.out).unwrap_or_default();
    let _ = fs::remove_dir_all(&rec.dir);
    if let Some(msg) = err {
        let msg = msg.trim();
        if !msg.is_empty() {
            return Err(anyhow!("{msg}"));
        }
    }
    Ok(text.trim().to_string())
}

#[cfg(target_os = "macos")]
fn abort(rec: Rec) {
    let _ = fs::write(&rec.stop, b"");
    let done = format!("{}.done", rec.out.display());
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && !Path::new(&done).exists() {
        thread::sleep(Duration::from_millis(50));
    }
    let _ = fs::remove_dir_all(&rec.dir);
}

#[cfg(target_os = "macos")]
fn helper_app() -> Result<PathBuf> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    SWIFT.hash(&mut hasher);
    PLIST.hash(&mut hasher);
    let tag = format!("{:016x}", hasher.finish());
    let root = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("arbos")
        .join("voice");
    fs::create_dir_all(&root).context("voice cache")?;
    let app = root.join(format!("arbos-dictate-{tag}.app"));
    let bin = app.join("Contents/MacOS/dictate");
    if bin.is_file() {
        return Ok(app);
    }

    let tmp = root.join(format!("arbos-dictate-{tag}.building.app"));
    let _ = fs::remove_dir_all(&tmp);
    let macos = tmp.join("Contents/MacOS");
    fs::create_dir_all(&macos).context("voice helper dirs")?;
    let src = tmp.join("dictate.swift");
    fs::write(&src, SWIFT).context("write voice helper source")?;
    fs::write(tmp.join("Contents/Info.plist"), PLIST).context("write voice helper plist")?;

    let compiled = Command::new("swiftc")
        .args(["-O", "-o"])
        .arg(macos.join("dictate"))
        .arg(&src)
        .output()
        .context("compile voice helper")?;
    if !compiled.status.success() {
        let _ = fs::remove_dir_all(&tmp);
        return Err(anyhow!(
            "compile voice helper: {}",
            String::from_utf8_lossy(&compiled.stderr).trim()
        ));
    }
    let _ = fs::remove_file(&src);

    let sign = Command::new("codesign")
        .args(["--force", "--sign", &signing_identity()])
        .arg(&tmp)
        .output()
        .context("sign voice helper")?;
    if !sign.status.success() {
        let _ = fs::remove_dir_all(&tmp);
        return Err(anyhow!(
            "sign voice helper: {}",
            String::from_utf8_lossy(&sign.stderr).trim()
        ));
    }
    fs::rename(&tmp, &app).context("install voice helper")?;
    Ok(app)
}

#[cfg(target_os = "macos")]
fn signing_identity() -> String {
    if let Ok(id) = std::env::var("ARBOS_SIGN_IDENTITY") {
        if !id.is_empty() {
            return id;
        }
    }
    let out = Command::new("security")
        .args(["find-identity", "-v", "-p", "codesigning"])
        .output();
    if let Ok(out) = out {
        if String::from_utf8_lossy(&out.stdout).contains("\"Arbos Dev\"") {
            return "Arbos Dev".into();
        }
    }
    "-".into()
}

#[cfg(target_os = "macos")]
fn tempfile_dir() -> Result<PathBuf> {
    let stamp = format!(
        "arbos-voice-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let dir = std::env::temp_dir().join(stamp);
    fs::create_dir_all(&dir).context("voice rec dir")?;
    Ok(dir)
}
