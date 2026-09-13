//! The fixture runner from `docs/filesystem-state-design.md` "Testing by
//! authored states": a test is a folder under `tests/fixtures/`.
//!
//! ```text
//! tests/fixtures/<name>/
//! ├── dot-arbos/…       the authored state (agents, plans, transcripts);
//! │                     copied to `.arbos/` in the scratch place — the
//! │                     repository ignores `.arbos/`, so the folder must
//! │                     not carry that name in git
//! ├── now               optional: the kernel's clock at start (RFC 3339 UTC)
//! ├── replies.jsonl     optional: the scripted model (--provider replay)
//! └── expect.sh         run in the place after the kernel exits; 0 = pass
//! ```
//!
//! For each folder: copy it to a scratch place, `arbos-kernel check` it,
//! run `arbos-kernel serve <place> --until-idle [--now …] [--provider
//! replay --replies …]` with a config home of its own and no network, then
//! run `expect.sh` there. A fixture fails on a lint error, a non-zero
//! kernel exit, a kernel that does not go idle within the limit, or a
//! failing `expect.sh`; the kernel's output is printed for that fixture.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const KERNEL: &str = env!("CARGO_BIN_EXE_arbos-kernel");
const LIMIT: Duration = Duration::from_secs(90);

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

struct Outcome {
    ok: bool,
    log: String,
}

fn run_fixture(dir: &Path) -> Outcome {
    let name = dir.file_name().unwrap().to_string_lossy().into_owned();
    let scratch = std::env::temp_dir().join(format!(
        "arbos-fixture-{name}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    let place = scratch.join("place");
    copy_dir(dir, &place);
    // The authored state travels as `dot-arbos/` (the repo root ignores
    // `.arbos/`); it is `.arbos/` where the kernel runs.
    let authored = place.join("dot-arbos");
    if authored.is_dir() {
        std::fs::rename(&authored, place.join(".arbos")).unwrap();
    } else {
        panic!(
            "fixture {name} has no dot-arbos/ folder (an `.arbos/` folder would be ignored by git)"
        );
    }
    // A config home of its own: a provider that is never reached, so a
    // fixture that forgets replies.jsonl fails fast and clearly.
    let xdg = scratch.join("xdg");
    std::fs::create_dir_all(xdg.join("arbos")).unwrap();
    std::fs::write(
        xdg.join("arbos").join("config.toml"),
        "provider = \"custom\"\nmodel = \"fixture/none\"\napi_base = \"http://127.0.0.1:9/v1\"\napi_key = \"fixture-key-not-a-secret-000000\"\nmax_attempts = 1\nstream_idle_ms = 5000\ntrace = false\n",
    )
    .unwrap();
    let home = scratch.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut log = String::new();

    // 1. Lint the authored state before a kernel touches it.
    let check = Command::new(KERNEL)
        .arg("check")
        .arg(&place)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("HOME", &home)
        .output()
        .unwrap();
    log.push_str(&format!(
        "== check (exit {:?})\n{}{}",
        check.status.code(),
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    ));
    // A fresh fixture has no agent.md (bootstrap writes root); the lint
    // warns about that, and only errors count.
    if !check.status.success() {
        return Outcome { ok: false, log };
    }

    // 2. Run the kernel to idle.
    let mut cmd = Command::new(KERNEL);
    cmd.arg("serve")
        .arg(&place)
        .arg("--until-idle")
        .arg("--horizon")
        .arg("1h");
    if let Ok(now) = std::fs::read_to_string(dir.join("now")) {
        cmd.arg("--now").arg(now.trim());
    }
    let replies = dir.join("replies.jsonl");
    if replies.exists() {
        cmd.arg("--provider")
            .arg("replay")
            .arg("--replies")
            .arg(&replies);
    }
    cmd.env("XDG_CONFIG_HOME", &xdg)
        .env("HOME", &home)
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .current_dir(&place)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if started.elapsed() > LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut out = String::new();
    let mut err = String::new();
    if let Some(mut s) = child.stdout.take() {
        use std::io::Read;
        let _ = s.read_to_string(&mut out);
    }
    if let Some(mut s) = child.stderr.take() {
        use std::io::Read;
        let _ = s.read_to_string(&mut err);
    }
    log.push_str(&format!(
        "== serve (exit {:?}, {:.1}s)\n{out}{err}",
        status.map(|s| s.code()),
        started.elapsed().as_secs_f32()
    ));
    let kernel_ok = status.is_some_and(|s| s.success());
    if !kernel_ok {
        if status.is_none() {
            log.push_str("kernel did not go idle within the limit\n");
        }
        return Outcome { ok: false, log };
    }

    // 3. What the kernel left.
    let expect = dir.join("expect.sh");
    if expect.exists() {
        let e = Command::new("sh")
            .arg(&expect)
            .current_dir(&place)
            .output()
            .unwrap();
        log.push_str(&format!(
            "== expect.sh (exit {:?})\n{}{}",
            e.status.code(),
            String::from_utf8_lossy(&e.stdout),
            String::from_utf8_lossy(&e.stderr)
        ));
        if !e.status.success() {
            for agent in std::fs::read_dir(place.join(".arbos/agents"))
                .into_iter()
                .flatten()
                .flatten()
            {
                let t = agent.path().join("transcript.jsonl");
                if let Ok(text) = std::fs::read_to_string(&t) {
                    log.push_str(&format!("-- {}\n{text}", t.display()));
                }
            }
            return Outcome { ok: false, log };
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    Outcome { ok: true, log }
}

#[test]
fn every_fixture_passes() {
    let root = fixtures_dir();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty(), "no fixtures under {}", root.display());
    let mut failed = Vec::new();
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let outcome = run_fixture(dir);
        if outcome.ok {
            eprintln!("fixture {name}: ok");
        } else {
            eprintln!("fixture {name}: FAILED\n{}", outcome.log);
            failed.push(name);
        }
    }
    assert!(failed.is_empty(), "fixtures failed: {}", failed.join(", "));
}
