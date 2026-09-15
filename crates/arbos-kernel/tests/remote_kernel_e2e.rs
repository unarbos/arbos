//! The install and stop shells that put `arbos-kernel` on another machine
//! and replace a running one, run here against a local "remote": a temp
//! HOME, a fake `curl` on PATH that serves the release tarball from disk,
//! and this build's own kernel as the asset. No network, no ssh.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arbos-remote-kernel-{tag}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A release tarball of this build's kernel, named as the workflow names
/// it, with its `.sha256`, under `dir`.
fn fake_release(dir: &Path, version: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let name = format!("arbos-kernel-v{version}-linux-x86_64");
    let inner = dir.join(&name);
    std::fs::create_dir_all(&inner).unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_arbos-kernel"),
        inner.join("arbos-kernel"),
    )
    .unwrap();
    let tar = dir.join(format!("{name}.tar.gz"));
    assert!(
        Command::new("tar")
            .args(["-C"])
            .arg(dir)
            .args(["-czf"])
            .arg(&tar)
            .arg(&name)
            .status()
            .unwrap()
            .success()
    );
    let sum = Command::new("sha256sum").arg(&tar).output().unwrap();
    let sha = dir.join(format!("{name}.tar.gz.sha256"));
    std::fs::write(&sha, String::from_utf8_lossy(&sum.stdout).as_bytes()).unwrap();
    (tar, sha)
}

/// `curl -fsSL … -o <out> <url>`: copies the file whose name ends the URL
/// from `serve_dir`; 22 (curl's "not found") otherwise.
fn fake_curl(bin_dir: &Path, serve_dir: &Path) {
    let script = format!(
        "#!/bin/sh\nout=\"\"; url=\"\"\nwhile [ $# -gt 0 ]; do case \"$1\" in -o) out=\"$2\"; shift 2;; -*) shift;; *) url=\"$1\"; shift;; esac; done\nf=\"{}/$(basename \"$url\")\"\nif [ -f \"$f\" ]; then cp \"$f\" \"$out\"; exit 0; fi\nexit 22\n",
        serve_dir.display()
    );
    let path = bin_dir.join("curl");
    std::fs::write(&path, script).unwrap();
    Command::new("chmod").arg("+x").arg(&path).status().unwrap();
}

fn sh(script: &str, home: &Path, path_prefix: &Path) -> (i32, String) {
    let path = format!(
        "{}:{}",
        path_prefix.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new("sh")
        .arg("-c")
        .arg(script)
        .env("HOME", home)
        .env("PATH", path)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn the_install_script_places_a_release_kernel_and_refuses_a_bad_checksum() {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        eprintln!("release assets are linux-x86_64 only here; skipping");
        return;
    }
    let dir = scratch("install");
    let home = dir.join("home");
    let shim = dir.join("shim");
    let serve = dir.join("serve");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&shim).unwrap();
    std::fs::create_dir_all(&serve).unwrap();
    fake_release(&serve, "0.2.1");
    fake_curl(&shim, &serve);
    let bin = format!("{}/.cargo/bin/arbos-kernel", home.display());

    // Fresh machine: the release asset lands, checked, and runs.
    let script =
        arbos_core::remote_kernel::install_script(&bin, "0.2.1", "c6e7698", "linux-amd64", false);
    let (code, out) = sh(&script, &home, &shim);
    assert_eq!(code, 0, "{out}");
    let steps = arbos_core::remote_kernel::steps_in(&out);
    assert!(
        steps.iter().any(|s| s.starts_with("downloading ")),
        "{steps:?}"
    );
    assert_eq!(
        steps.last().map(String::as_str),
        Some("installed from release"),
        "{steps:?}"
    );
    let version = Command::new(&bin).arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("arbos-kernel "));
    assert!(
        !Path::new(&format!("{bin}.new")).exists(),
        "the temp name is gone"
    );
    assert!(
        !home.join(".cache/arbos/kernel-0.2.1.download").exists(),
        "the download dir is cleaned"
    );

    // A tampered checksum: nothing is installed over the good binary, and
    // with no source build allowed the script says what to do.
    let good = std::fs::read(&bin).unwrap();
    let sha = serve.join("arbos-kernel-v0.2.1-linux-x86_64.tar.gz.sha256");
    std::fs::write(
        &sha,
        "0000000000000000000000000000000000000000000000000000000000000000  x\n",
    )
    .unwrap();
    let (code, out) = sh(&script, &home, &shim);
    assert_eq!(code, 3, "{out}");
    let steps = arbos_core::remote_kernel::steps_in(&out);
    assert!(
        steps.iter().any(|s| s.contains("checksum mismatch")),
        "{steps:?}"
    );
    assert!(
        steps.iter().any(|s| s.contains("source build not allowed")),
        "{steps:?}"
    );
    assert_eq!(
        std::fs::read(&bin).unwrap(),
        good,
        "the good binary is untouched"
    );

    // No release for this machine type, no build allowed: exit 3, said.
    let none =
        arbos_core::remote_kernel::install_script(&bin, "0.2.1", "c6e7698", "linux-arm64", false);
    let (code, out) = sh(&none, &home, &shim);
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("no release is cut for linux-arm64"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_stop_script_ends_a_running_kernel_and_its_jobs_and_nothing_else() {
    // A kernel with a background job that would outlive a careless stop.
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"starting a long job\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 600\",\"background\":true}}]}\n",
        "{\"agent\":\"root\",\"content\":\"running\"}\n",
    );
    let mut k = start_kernel_replay_prepared("remote-stop", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"s\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "start it"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    // The job is alive: a `sleep 600` whose command line names it.
    let job_alive = || {
        Command::new("pgrep")
            .args(["-f", "^sleep 600$"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    assert!(job_alive(), "the background job runs");
    // A bystander process that must survive: not the kernel's.
    let mut bystander = Command::new("sleep").arg("30").spawn().unwrap();

    let script = arbos_core::remote_kernel::stop_script(&k.place.display().to_string());
    // The stop script waits for the pid to leave. This test is the
    // kernel's parent, so the exited kernel would sit as a zombie (still
    // answering `kill -0`) until reaped: reap while the script runs. On a
    // real machine the kernel is detached and init reaps it.
    let runner = {
        let script = script.clone();
        std::thread::spawn(move || sh(&script, &std::env::temp_dir(), &std::env::temp_dir()))
    };
    while !runner.is_finished() {
        let _ = k.child.try_wait();
        std::thread::sleep(Duration::from_millis(100));
    }
    let (code, out) = runner.join().unwrap();
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("arbos-stop: stopped"), "{out}");
    // The kernel is gone (its live file with it), and so is its job.
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) && job_alive() {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(!job_alive(), "the job died with the kernel (its leash)");
    assert!(
        k.child.try_wait().unwrap().is_some(),
        "the kernel process exited"
    );
    // The bystander lives.
    assert!(
        bystander.try_wait().unwrap().is_none(),
        "an unrelated process is untouched"
    );
    let _ = bystander.kill();
    // A second stop: nothing to do.
    let (code, out) = sh(&script, &std::env::temp_dir(), &std::env::temp_dir());
    assert_eq!(code, 0);
    assert!(out.contains("not running"), "{out}");
    let _ = k.child.kill();
}
