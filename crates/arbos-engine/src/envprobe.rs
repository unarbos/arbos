//! What the machine offers a coding task, found once per directory and put
//! in the prompt as `Environment:` — so the agent does not spend its first
//! calls on `which python`, and does not `pip install` into the wrong
//! interpreter (SWE-bench run of 2026-09-13: 6 of 8 easy instances lost
//! 3–8 bash calls to this).
//!
//! The probe runs in the same login shell the `bash` tool uses, so what
//! it reports is what a command will see.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Wall time the probe may take before it is dropped and the line says so.
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// The probe script, run as one login shell. Each line prints one `key=value`.
const SCRIPT: &str = r#"
py=$(command -v python 2>/dev/null || command -v python3 2>/dev/null)
[ -n "$py" ] && printf 'python=%s\n' "$py" && printf 'python_version=%s\n' "$("$py" -V 2>&1 | head -1)"
printf 'venv=%s\n' "${VIRTUAL_ENV:-}"
printf 'conda=%s\n' "${CONDA_DEFAULT_ENV:-}"
tools=""
for t in uv pip pip3 poetry pipenv conda pytest tox npm pnpm yarn bun node cargo go make cmake; do command -v "$t" >/dev/null 2>&1 && tools="$tools $t"; done
printf 'tools=%s\n' "$tools"
"#;

/// Files at the root of the project that say how it is built and tested.
const PROJECT_FILES: &[&str] = &[
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "requirements.txt",
    "pytest.ini",
    "tox.ini",
    "conftest.py",
    "environment.yml",
    "uv.lock",
    "poetry.lock",
    "package.json",
    "Cargo.toml",
    "go.mod",
    "Makefile",
    "justfile",
    ".venv",
];

/// The `Environment:` line for `cwd`, computed once per directory for the
/// life of this process.
pub fn line(cwd: &Path) -> String {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().unwrap().get(cwd) {
        return hit.clone();
    }
    let computed = compute(cwd);
    cache
        .lock()
        .unwrap()
        .insert(cwd.to_path_buf(), computed.clone());
    computed
}

fn compute(cwd: &Path) -> String {
    let mut parts = Vec::new();
    match probe(cwd) {
        Ok(found) => {
            if let Some(py) = found.get("python").filter(|s| !s.is_empty()) {
                let version = found
                    .get("python_version")
                    .filter(|s| !s.is_empty())
                    .map(|v| format!(" ({v})"))
                    .unwrap_or_default();
                parts.push(format!("python={py}{version}"));
            } else {
                parts.push("no python on PATH".to_string());
            }
            if let Some(v) = found.get("venv").filter(|s| !s.is_empty()) {
                parts.push(format!("venv={v}"));
            }
            if let Some(c) = found.get("conda").filter(|s| !s.is_empty()) {
                parts.push(format!("conda env={c}"));
            }
            if let Some(t) = found
                .get("tools")
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
            {
                parts.push(format!("tools: {t}"));
            }
        }
        Err(e) => parts.push(format!("probe failed: {e}")),
    }
    let files: Vec<&str> = PROJECT_FILES
        .iter()
        .copied()
        .filter(|f| cwd.join(f).exists())
        .collect();
    if !files.is_empty() {
        parts.push(format!("project files: {}", files.join(" ")));
    }
    parts.join(" · ")
}

/// Run the probe in the job shell (a login shell when it is bash). Killed
/// and reported when it takes longer than `PROBE_TIMEOUT`.
fn probe(cwd: &Path) -> Result<HashMap<String, String>, String> {
    let (program, args) = crate::jobs::shell_command(SCRIPT);
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });
    let out = match rx.recv_timeout(PROBE_TIMEOUT) {
        Ok(out) => out,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("took over {}s", PROBE_TIMEOUT.as_secs()));
        }
    };
    let _ = child.wait();
    Ok(out
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("arbos-envprobe-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_line_names_the_interpreter_and_project_files() {
        let dir = scratch("line");
        std::fs::write(dir.join("pyproject.toml"), "[project]\n").unwrap();
        let line = compute(&dir);
        eprintln!("environment line: {line}");
        assert!(
            line.contains("python=") || line.contains("no python"),
            "{line}"
        );
        assert!(line.contains("project files: pyproject.toml"), "{line}");
    }

    #[test]
    fn a_directory_is_probed_once() {
        let dir = scratch("once");
        let a = line(&dir);
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        let b = line(&dir);
        assert_eq!(a, b);
    }
}
