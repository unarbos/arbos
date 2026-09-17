use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

use super::ToolOut;
use crate::access::Access;
use crate::tool::{BoxFuture, Plan, PlanCx, RunCx, Tool, blocking, simple_schema};

pub struct Changes;
pub struct Undo;

impl Tool for Changes {
    fn name(&self) -> &'static str {
        "changes"
    }
    fn schema(&self) -> Value {
        simple_schema("changes", "Show the git checkpoint for this turn.", &[])
    }
    fn plan(&self, cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::read_path(cx.cwd)))
    }
    fn run(&self, cx: RunCx, _args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || {
            let agent_dir = arbos_core::Layout::new(&cx.place, cx.agent.id.as_str()).dir;
            changes(&cx.cwd, Some(&agent_dir))
        })
    }
}

impl Tool for Undo {
    fn name(&self) -> &'static str {
        "undo"
    }
    fn schema(&self) -> Value {
        simple_schema("undo", "Restore the git checkpoint.", &[])
    }
    fn plan(&self, _cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::exclusive()))
    }
    fn run(&self, cx: RunCx, _args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || undo(&cx.cwd, cx.turn_line))
    }
}

const TAG: &str = "arbos-checkpoint";

/// One turn's starting point, for `arbos-kernel rewind`: the transcript
/// line the turn began on, HEAD, and a commit holding the working tree as
/// it was, untracked files included (None when the tree matched HEAD).
/// The commit is kept alive by `refs/arbos/cp/<agent>/<line>`. One JSON
/// line per turn in `<agent dir>/checkpoints.jsonl`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Checkpoint {
    pub line: u64,
    pub ts: i64,
    pub head: String,
    /// The working tree as it stood, as a commit (see `work_commit`).
    /// None with `clean: true`: the tree equalled HEAD's, nothing to save.
    /// None with `work_error`: it could not be saved, and the record says
    /// why. None with neither: a line from before this distinction, whose
    /// tree is unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<String>,
    /// The tree equalled HEAD's when the turn started: a restore to
    /// `head` alone is the whole truth.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clean: bool,
    /// Why no working-tree commit could be made. A checkpoint that could
    /// not be written is never silently empty (qal-j08: `commit-tree`
    /// failed for want of a git identity, every checkpoint carried HEAD
    /// alone, and a restore `clean`ed the kept turns' files away).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_error: Option<String>,
}

/// A restore refused because the checkpoint does not know its tree: not a
/// failure of git, a fact about the record. The kernel tells it as a
/// notice, not an error (qal-j05's shape: a rewind that half-worked drawn
/// as a crash), and says when it stops.
#[derive(Debug)]
pub struct NoTree(pub String);

impl std::fmt::Display for NoTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NoTree {}

impl Checkpoint {
    /// Whether a `files: true` restore may reset and clean the tree: it
    /// knows the tree (a work commit), or knows there was nothing beyond
    /// HEAD. A checkpoint that knows its record is missing, or predates
    /// the record, must not delete on the strength of it.
    pub fn knows_tree(&self) -> bool {
        self.work.is_some() || self.clean
    }
}

/// Why a fresh checkpoint has no tree yet: its working-tree commit is
/// being made on the blocking pool. A `files: true` rewind that lands in
/// that window is refused with this rather than guessed.
pub const TREE_PENDING: &str =
    "the working tree for this turn is still being saved; try again in a moment";

/// Record where a turn starts: the plain HEAD mark `undo` uses, plus a
/// checkpoint of the working tree for `rewind`. Runs on the blocking pool.
/// The record and the tree, in one call; the turn itself uses the two
/// halves below so the record is on disk before the turn goes on.
pub fn snapshot_turn(cwd: &Path, agent_dir: &Path, agent: &str, line: u64) -> Result<()> {
    if let Some(cp) = snapshot_turn_record(cwd, agent_dir, agent, line)? {
        snapshot_turn_tree(cwd, agent_dir, agent, &cp)?;
    }
    Ok(())
}

/// The cheap half, done **before the turn goes on**: the undo mark
/// cleared and set to HEAD, and the checkpoint line appended with HEAD
/// and the turn's line, its tree marked pending. `rewind turn N`
/// resolves to the checkpoint on or before the Nth user line; when this
/// record was written on the blocking pool after the turn had started,
/// a rewind that landed first resolved to the *previous* turn's
/// checkpoint and cut one turn too many — down to an empty transcript
/// when the previous turn was the first (`standing_pass_e2e`, red under
/// load for days; a person pressing Rewind right after a turn on a busy
/// machine). None when the folder is not a git repository or has no
/// HEAD: nothing to rewind to.
pub fn snapshot_turn_record(
    cwd: &Path,
    agent_dir: &Path,
    agent: &str,
    line: u64,
) -> Result<Option<Checkpoint>> {
    snapshot(cwd)?;
    if !cwd.join(".git").exists() {
        return Ok(None);
    }
    let head = git_out(cwd, &["rev-parse", "HEAD"]).unwrap_or_default();
    if head.is_empty() {
        return Ok(None);
    }
    let cp = Checkpoint {
        line,
        ts: arbos_core::now_ms(),
        head,
        work: None,
        clean: false,
        work_error: Some(TREE_PENDING.to_string()),
    };
    let path = agent_dir.join("checkpoints.jsonl");
    let mut text = serde_json::to_string(&cp)?;
    text.push('\n');
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?
        .write_all(text.as_bytes())
        .with_context(|| format!("append {}", path.display()))?;
    let _ = agent;
    Ok(Some(cp))
}

/// The expensive half, on the blocking pool while the turn runs: the
/// working-tree commit (`git add -A` into a scratch index), the ref, the
/// undo mark with the tree, and the checkpoint line rewritten with what
/// was found. A line the meantime removed (a rewind cut it) is left
/// removed.
pub fn snapshot_turn_tree(
    cwd: &Path,
    agent_dir: &Path,
    agent: &str,
    cp: &Checkpoint,
) -> Result<()> {
    let line = cp.line;
    let head = &cp.head;
    let (work, clean, work_error) = match work_commit(cwd, head) {
        Ok(Some(w)) => (Some(w), false, None),
        Ok(None) => (None, true, None),
        Err(why) => {
            eprintln!("checkpoint {agent}:{line}: working tree not saved: {why}");
            (None, false, Some(why))
        }
    };
    if let Some(w) = &work {
        let safe: String = agent
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let _ = Command::new("git")
            .args(["update-ref", &format!("refs/arbos/cp/{safe}/{line}"), w])
            .current_dir(cwd)
            .status();
    }
    // The `undo` mark carries the same knowledge: HEAD, then the work
    // commit, `clean`, or `error:<why>`, then `line:<n>` — the turn it
    // was written for, which `undo` checks before it resets anything. It
    // is written whole or not at all, and a write that fails leaves *no*
    // mark rather than an older turn's: a stale mark sent `undo` to an
    // older HEAD, deleting a kept commit and its file (qal-j10).
    let mark = cwd.join(".arbos").join("runtime").join("checkpoint");
    let second = match (&work, clean, &work_error) {
        (Some(w), _, _) => w.clone(),
        (None, true, _) => "clean".to_string(),
        (None, false, why) => format!("error:{}", why.as_deref().unwrap_or("unknown")),
    };
    if let Err(e) = arbos_core::record::write_atomic(
        &mark,
        format!("{head}\n{second}\nline:{line}\n").as_bytes(),
    ) {
        let _ = std::fs::remove_file(&mark);
        return Err(e.context("the undo mark"));
    }
    let filled = Checkpoint {
        line,
        ts: cp.ts,
        head: head.clone(),
        work,
        clean,
        work_error,
    };
    // The filled record on its own first (`checkpoints.d/<line>.json`),
    // whole or not at all: a rewind that resolved the pending record and
    // cut the line meanwhile reads the tree from here once it lands.
    arbos_core::record::write_atomic(
        &tree_sidecar(agent_dir, line),
        serde_json::to_string(&filled)?.as_bytes(),
    )?;
    // Then this turn's line rewritten with the tree; a confirmed read,
    // whole or not at all (arbos_core::record).
    let path = agent_dir.join("checkpoints.jsonl");
    let Some(text) = arbos_core::record::read_text(&path).confirmed()? else {
        return Ok(());
    };
    let mut out = String::new();
    let mut seen = false;
    for l in text.lines() {
        match serde_json::from_str::<Checkpoint>(l) {
            Ok(existing) if existing.line == line && existing.head == filled.head => {
                out.push_str(&serde_json::to_string(&filled)?);
                seen = true;
            }
            _ => out.push_str(l),
        }
        out.push('\n');
    }
    if seen {
        arbos_core::record::write_atomic(&path, out.as_bytes())?;
    }
    Ok(())
}

/// Where a turn's filled checkpoint waits for a rewind that resolved it
/// while its tree was still being saved.
pub fn tree_sidecar(agent_dir: &Path, line: u64) -> std::path::PathBuf {
    agent_dir.join("checkpoints.d").join(format!("{line}.json"))
}

/// A checkpoint whose tree was pending when it was read: the filled
/// record, waited for up to `wait` (the save runs on the blocking pool
/// and takes what `git add -A` takes on the repository), or the pending
/// one back when it does not land — the caller refuses with
/// [`TREE_PENDING`] then. A checkpoint that already knows its tree, or
/// whose save failed for another reason, comes straight back.
pub fn settle_tree(agent_dir: &Path, cp: &Checkpoint, wait: std::time::Duration) -> Checkpoint {
    if cp.work_error.as_deref() != Some(TREE_PENDING) {
        return cp.clone();
    }
    let sidecar = tree_sidecar(agent_dir, cp.line);
    let deadline = std::time::Instant::now() + wait;
    loop {
        if let arbos_core::record::Read::Present(filled) =
            arbos_core::record::read_json::<Checkpoint>(&sidecar)
            && filled.head == cp.head
        {
            return filled;
        }
        if std::time::Instant::now() >= deadline {
            return cp.clone();
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// The identity an internal checkpoint commit is written under. It is
/// the kernel's own ref, never on a branch, never pushed; it needs no
/// real name, and must not depend on the user having set one — a fresh
/// machine has none, and that is exactly when rewind is reached for.
const CHECKPOINT_IDENTITY: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "arbos"),
    ("GIT_AUTHOR_EMAIL", "arbos@kernel"),
    ("GIT_COMMITTER_NAME", "arbos"),
    ("GIT_COMMITTER_EMAIL", "arbos@kernel"),
];

/// A commit whose tree is the working tree as it stands — tracked
/// changes and untracked files alike, ignored files and `.arbos/` left
/// out — parented on HEAD so `read-tree` can bring it all back. Built
/// through a scratch index copied from the real one (so the add is
/// incremental) and never touching the real index or the branch.
/// `Ok(None)` when the tree equals HEAD's; `Err(why)` when it could not
/// be made, which the checkpoint records rather than swallows.
fn work_commit(cwd: &Path, head: &str) -> Result<Option<String>, String> {
    let index = git_out(cwd, &["rev-parse", "--git-path", "index"])
        .ok_or_else(|| "git rev-parse --git-path index failed".to_string())?;
    let index = cwd.join(index);
    let scratch = cwd
        .join(".arbos")
        .join(format!("index-scratch-{}", std::process::id()));
    let _ = std::fs::create_dir_all(cwd.join(".arbos"));
    if index.exists() {
        std::fs::copy(&index, &scratch).map_err(|e| format!("copy the index: {e}"))?;
    }
    // `.arbos/` is kept out of the add by an excludes file of our own —
    // the user's global excludes plus `/.arbos/` — rather than a pathspec:
    // `:!.arbos` makes `add` fail when `.arbos` is *also* ignored by the
    // project ("paths are ignored by one of your .gitignore files"), and
    // no exclusion at all makes it fail when it is *not* ignored, since
    // the store is a git repository of its own that may have no commit
    // yet ("does not have a commit checked out; adding files failed").
    // The second sank every tree save in a fresh place whose project
    // repository did not ignore the store yet (found by the standing
    // pass under load).
    let excludes = cwd
        .join(".arbos")
        .join(format!("index-scratch-excludes-{}", std::process::id()));
    {
        let mut text = git_out(cwd, &["config", "--get", "core.excludesFile"])
            .filter(|p| !p.is_empty())
            .map(|p| std::path::PathBuf::from(shellexpand_home(&p)))
            .or_else(default_global_excludes)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        if !text.ends_with('\n') && !text.is_empty() {
            text.push('\n');
        }
        text.push_str("/.arbos/\n");
        std::fs::write(&excludes, text).map_err(|e| format!("write the excludes file: {e}"))?;
    }
    let run = |args: &[&str]| -> Result<String, String> {
        let out = Command::new("git")
            .arg("-c")
            .arg(format!("core.excludesFile={}", excludes.display()))
            .args(args)
            .env("GIT_INDEX_FILE", &scratch)
            .envs(CHECKPOINT_IDENTITY.iter().copied())
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("git {}: {e}", args.first().unwrap_or(&"")))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            Err(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    };
    let result = (|| {
        // `.arbos/` is excluded at the add: it is the agent's own state,
        // never part of the project's checkpoint — and it is a git
        // repository of its own (the F design's Phase 1), which `add -A`
        // refuses outright when it has no commit yet ("does not have a
        // commit checked out; adding files failed"). A place whose
        // project repository does not yet ignore the store (a fresh
        // place, `git init` run by the agent in its first turn) lost
        // every tree save to that until the next kernel start wrote the
        // exclude; found by the standing pass under load.
        run(&["add", "-A", "--", "."])?;
        // And dropped from the scratch index if an earlier plain `add`
        // had taken it.
        let _ = run(&[
            "rm",
            "-r",
            "-q",
            "--cached",
            "--ignore-unmatch",
            "--",
            ".arbos",
        ]);
        let tree = run(&["write-tree"])?;
        let head_tree = git_out(cwd, &["rev-parse", &format!("{head}^{{tree}}")])
            .ok_or_else(|| format!("git rev-parse {head}^{{tree}} failed"))?;
        if tree == head_tree {
            return Ok(None);
        }
        run(&["commit-tree", &tree, "-p", head, "-m", "arbos checkpoint"]).map(Some)
    })();
    let _ = std::fs::remove_file(&scratch);
    let _ = std::fs::remove_file(&excludes);
    result
}

/// `~/x` → `$HOME/x`, as git reads `core.excludesFile`.
fn shellexpand_home(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(home) => format!("{home}/{rest}"),
            Err(_) => p.to_string(),
        },
        None => p.to_string(),
    }
}

/// Where git looks for global excludes when `core.excludesFile` is unset:
/// `$XDG_CONFIG_HOME/git/ignore`, else `~/.config/git/ignore`.
fn default_global_excludes() -> Option<std::path::PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(std::path::PathBuf::from(xdg).join("git").join("ignore"));
    }
    std::env::var("HOME").ok().map(|h| {
        std::path::PathBuf::from(h)
            .join(".config")
            .join("git")
            .join("ignore")
    })
}

fn git_out(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Every checkpoint of an agent, oldest first.
pub fn checkpoints(agent_dir: &Path) -> Vec<Checkpoint> {
    std::fs::read_to_string(agent_dir.join("checkpoints.jsonl"))
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Put the working tree back to a checkpoint: HEAD to its commit, tracked
/// files to the saved tree (or to HEAD when the tree was clean), untracked
/// files from after it removed — never `.arbos/`.
pub fn restore(cwd: &Path, cp: &Checkpoint) -> Result<String> {
    // The agent's own state must not be part of what comes back: a
    // `.arbos/` tracked by the project repo would be reset to an old
    // transcript under a running kernel. Its own repo (the F design's
    // Phase 1) is the real fix; until then, refuse.
    if git_out(cwd, &["ls-files", "--", ".arbos"]).is_some_and(|l| !l.is_empty()) {
        anyhow::bail!(
            ".arbos/ is tracked by the project repository; run `git rm -r --cached .arbos` (and add .arbos to .gitignore) before rewinding files"
        );
    }
    // A restore deletes on the strength of the checkpoint's record of the
    // tree. A checkpoint that knows its record is missing — or predates
    // the record — gets no `reset --hard`, no `clean`: the transcript is
    // rewound, the files are left as they are, and the reason is said.
    if !cp.knows_tree() {
        return Err(anyhow::Error::new(NoTree(format!(
            "no checkpoint of the working tree for this turn ({}); files left as they are — the transcript is rewound",
            cp.work_error
                .as_deref()
                .unwrap_or("recorded before the kernel kept the tree, or whether it was clean")
        ))));
    }
    // Order matters: HEAD back first, then everything untracked that the
    // later turns added goes (never `.arbos/`), then the checkpoint's tree
    // — tracked changes and the untracked files of that moment — comes
    // back, and the index returns to HEAD so it all shows as it did.
    let st = Command::new("git")
        .args(["reset", "--hard", &cp.head])
        .current_dir(cwd)
        .status()?;
    if !st.success() {
        anyhow::bail!("git reset --hard {} failed", cp.head);
    }
    let _ = Command::new("git")
        .args(["clean", "-fd", "-e", ".arbos", "-e", ".arbos/**"])
        .current_dir(cwd)
        .status();
    if let Some(work) = &cp.work {
        let st = Command::new("git")
            .args(["read-tree", "-u", "--reset", work])
            .current_dir(cwd)
            .status()?;
        if !st.success() {
            anyhow::bail!("git read-tree {work} failed");
        }
        let _ = Command::new("git")
            .args(["reset", "-q"])
            .current_dir(cwd)
            .status();
    }
    Ok(match &cp.work {
        Some(w) => format!(
            "{} + working tree {}",
            &cp.head[..cp.head.len().min(12)],
            &w[..w.len().min(12)]
        ),
        None => cp.head[..cp.head.len().min(12)].to_string(),
    })
}

pub fn snapshot(cwd: &Path) -> Result<()> {
    if !cwd.join(".git").exists() {
        return Ok(());
    }
    // HEAD only. `git add -A` + stash on a large place blocked the first
    // token and staged thousands of files. Undo still uses this sha.
    // The old mark goes first: a write that then fails must leave no
    // mark, never a previous turn's (qal-j10). `snapshot_turn` rewrites
    // it whole, with the tree and the turn line, right after.
    let mark = cwd.join(".arbos").join("runtime").join("checkpoint");
    match std::fs::remove_file(&mark) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => anyhow::bail!("could not clear the undo mark {}: {e}", mark.display()),
    }
    if let Ok(out) = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(cwd)
        .output()
    {
        let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !sha.is_empty() {
            arbos_core::record::write_atomic(&mark, format!("{sha}\n").as_bytes())
                .map_err(|e| e.context("the undo mark"))?;
        }
    }
    Ok(())
}

pub fn changes(cwd: &Path, agent_dir: Option<&Path>) -> Result<ToolOut> {
    let out = Command::new("git")
        .args(["status", "--short"])
        .current_dir(cwd)
        .output()?;
    let status = String::from_utf8_lossy(&out.stdout).into_owned();
    // On a branch of its own, the first line says where the work stands
    // against the base: the moment an agent looks at its changes is the
    // moment to notice nothing is committed yet.
    let mut body = branch_line(cwd, &status).unwrap_or_default();
    body.push_str(&status);
    let diff = Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(cwd)
        .output()?;
    body.push_str(&String::from_utf8_lossy(&diff.stdout));
    if body.trim().is_empty() {
        body = "(no changes)\n".into();
    }
    if let Some(note) = test_files_note(&status) {
        body.push_str(&note);
    }
    if let Some(report) = agent_dir.and_then(crate::repro::rerun_report) {
        body.push_str(&report);
    }
    if let Some(line) = agent_dir.and_then(crate::mechanism::current_in) {
        body.push_str(&format!(
            "\nMechanism stated at the first edit: {line}\nBefore the final reply: does this line explain every symptom the request names? If one is not explained, the fix is not done.\n"
        ));
    }
    Ok(ToolOut::text(body))
}

/// A line naming the existing test files the working tree changes, so the
/// agent sees when it is editing the spec (SWE-bench: two of four losses
/// were a rewritten or loosened test). New test files are not the point.
fn test_files_note(status: &str) -> Option<String> {
    let touched: Vec<&str> = status
        .lines()
        .filter(|l| l.len() > 3 && !l.starts_with("??") && !l.starts_with('A'))
        .map(|l| l[3..].trim())
        .filter(|p| is_test_path(p))
        .collect();
    if touched.is_empty() {
        return None;
    }
    Some(format!(
        "\nNote: {} existing test file(s) changed: {}. Existing tests are read-only spec: put every changed assertion, tolerance, expected value, and fixture back as it was, and add new test functions instead. Only a request that names the test may change it.\n",
        touched.len(),
        touched.join(", ")
    ))
}

/// After an edit to a source file: which existing test files name the
/// functions the diff touches. SWE-bench cycle 1: five of eight losses
/// changed the layer the reporter saw the symptom in while the hidden tests
/// exercise the shared helper underneath; "no existing test names this
/// function" is the signal that the fix may sit at the wrong layer. The
/// report is per changed function or method: a broad class name (cycle 2:
/// `FigureCanvasBase`, `_print_Pow`'s printer) is named by every test that
/// imports it and said nothing. Test files and non-code files are skipped;
/// outside git there is nothing to say.
pub fn coverage_note(cwd: &Path, paths: &[String]) -> Option<String> {
    let mut lines = Vec::new();
    for path in paths {
        let rel = Path::new(path)
            .strip_prefix(cwd)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| path.clone());
        if is_test_path(&rel) || !is_code_path(&rel) {
            continue;
        }
        let diff = git_out(cwd, &["diff", "-U0", "HEAD", "--", &rel])?;
        let text = std::fs::read_to_string(cwd.join(&rel)).unwrap_or_default();
        let symbols = changed_symbols(&diff, &text);
        if symbols.functions.is_empty() && symbols.classes.is_empty() {
            continue;
        }
        let fns: Vec<&str> = symbols
            .functions
            .iter()
            .take(4)
            .map(String::as_str)
            .collect();
        let fn_refs = if symbols.functions.is_empty() {
            Vec::new()
        } else {
            test_files_naming(cwd, &symbols.functions)
        };
        if !fn_refs.is_empty() {
            lines.push(format!(
                "{rel}: {} named in {}",
                fns.join(", "),
                list_files(&fn_refs)
            ));
            continue;
        }
        let class_refs = if symbols.classes.is_empty() {
            Vec::new()
        } else {
            test_files_naming(cwd, &symbols.classes)
        };
        let advice = "If the wrong value comes from a helper this calls, the fix belongs in the helper that has tests (fix at the root); if this is the right level, add a test here.";
        if fns.is_empty() {
            lines.push(format!(
                "{rel}: class-level change ({}), no changed function to check{}. {advice}",
                symbols.classes.join(", "),
                if class_refs.is_empty() {
                    String::new()
                } else {
                    format!("; class named in {}", list_files(&class_refs))
                }
            ));
        } else if class_refs.is_empty() {
            lines.push(format!(
                "{rel}: no existing test names {}. {advice}",
                fns.join(", ")
            ));
        } else {
            lines.push(format!(
                "{rel}: class-level match only ({} named in {}), no test names {}. {advice}",
                symbols.classes.join(", "),
                list_files(&class_refs),
                fns.join(", ")
            ));
        }
    }
    if lines.is_empty() {
        return None;
    }
    Some(format!("Tests covering this edit — {}", lines.join(" | ")))
}

fn list_files(files: &[String]) -> String {
    let shown: Vec<&str> = files.iter().take(5).map(String::as_str).collect();
    if files.len() > 5 {
        format!("{} (+{} more)", shown.join(", "), files.len() - 5)
    } else {
        shown.join(", ")
    }
}

fn is_code_path(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "py" | "rs"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "go"
            | "java"
            | "rb"
            | "c"
            | "cc"
            | "cpp"
            | "h"
            | "hpp"
            | "cs"
            | "kt"
            | "swift"
            | "php"
            | "scala"
    )
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ChangedSymbols {
    /// Functions and methods that enclose a changed line, or are defined
    /// on one, in order of first appearance.
    functions: Vec<String>,
    /// The classes (or impl blocks) those lines sit in.
    classes: Vec<String>,
}

/// The functions and classes a `-U0` diff touches, resolved against the
/// file as it is now: for every changed line, the nearest `def`/`fn`/
/// `function` above it (a method's own name, not its class) and the
/// nearest `class`/`impl` above that.
fn changed_symbols(diff_u0: &str, text: &str) -> ChangedSymbols {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = ChangedSymbols::default();
    for header in diff_u0.lines().filter(|l| l.starts_with("@@")) {
        let Some(plus) = header.split_whitespace().nth(2) else {
            continue;
        };
        let plus = plus.trim_start_matches('+');
        let (start, count) = match plus.split_once(',') {
            Some((s, c)) => (
                s.parse::<usize>().unwrap_or(0),
                c.parse::<usize>().unwrap_or(0),
            ),
            None => (plus.parse::<usize>().unwrap_or(0), 1),
        };
        // A pure deletion reports the line before it; look from there.
        let first = start.max(1);
        let last = (start + count.max(1)).saturating_sub(1).max(first);
        for n in first..=last.min(lines.len()) {
            if let Some(name) = definition_name(lines[n - 1], false) {
                push_unique(&mut out.functions, name);
            }
        }
        let mut found_fn = false;
        for n in (1..=first.min(lines.len())).rev() {
            let line = lines[n - 1];
            if !found_fn {
                if let Some(name) = definition_name(line, false) {
                    push_unique(&mut out.functions, name);
                    found_fn = true;
                    continue;
                }
            }
            if let Some(name) = definition_name(line, true) {
                push_unique(&mut out.classes, name);
                break;
            }
        }
        if out.functions.len() + out.classes.len() >= 12 {
            break;
        }
    }
    out
}

/// `def name(`, `fn name<`, `function name(`, `func name(` (or, with
/// `class_like`, `class Name`, `impl Name`, `struct Name`) at the start of
/// a line, whatever the indentation.
fn definition_name(line: &str, class_like: bool) -> Option<String> {
    let mut words = line
        .trim_start()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| !w.is_empty());
    let mut kw = words.next()?;
    // `pub fn`, `async def`, `pub(crate) fn`, `export function`, `static def`.
    for _ in 0..3 {
        if matches!(
            kw,
            "pub" | "crate" | "async" | "export" | "static" | "unsafe" | "const" | "default"
        ) {
            kw = words.next()?;
        } else {
            break;
        }
    }
    let matches = if class_like {
        matches!(kw, "class" | "impl" | "struct" | "trait" | "enum")
    } else {
        matches!(kw, "def" | "fn" | "function" | "func")
    };
    if !matches {
        return None;
    }
    let name = words.next()?;
    (name.len() > 1 && !matches!(name, "self" | "cls")).then(|| name.to_string())
}

fn push_unique(v: &mut Vec<String>, name: String) {
    if !v.iter().any(|n| *n == name) {
        v.push(name);
    }
}

/// Existing test files that mention any of `names` as a whole word.
fn test_files_naming(cwd: &Path, names: &[String]) -> Vec<String> {
    let mut args: Vec<&str> = vec!["grep", "-l", "-w", "-I"];
    for n in names {
        args.push("-e");
        args.push(n);
    }
    args.push("--");
    args.push(":(glob)**/test*");
    args.push(":(glob)**/*test*");
    args.push(":(glob)**/tests/**");
    args.push(":(glob)**/testing/**");
    args.push(":(glob)**/spec/**");
    let Some(out) = git_out(cwd, &args) else {
        return Vec::new();
    };
    let mut files: Vec<String> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && is_test_path(l))
        .map(str::to_string)
        .collect();
    files.sort();
    files.dedup();
    files
}

pub(crate) fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    lower
        .split('/')
        .any(|seg| seg == "tests" || seg == "test" || seg == "__tests__" || seg == "spec")
        || name.starts_with("test_")
        || name.ends_with("_test.py")
        || name.ends_with("_test.go")
        || name.ends_with("_test.rs")
        || name.ends_with(".test.ts")
        || name.ends_with(".test.js")
        || name.ends_with(".test.tsx")
        || name.ends_with(".spec.ts")
        || name.ends_with(".spec.js")
        || name.ends_with("_spec.rb")
}

/// "branch `fix/x`: 2 uncommitted files, 0 commits ahead of main" when
/// `cwd` is on a branch other than the base. None on the base itself, on a
/// detached HEAD, or outside a repository.
fn branch_line(cwd: &Path, status: &str) -> Option<String> {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch.is_empty() || branch == "HEAD" {
        return None;
    }
    let base = base_branch(cwd, &git);
    if branch == base {
        return None;
    }
    let dirty = status.lines().filter(|l| !l.trim().is_empty()).count();
    let ahead = git(&["rev-list", "--count", &format!("{base}..HEAD")]).unwrap_or_default();
    let ahead_text = if ahead.is_empty() {
        format!("(no `{base}` branch to compare with)")
    } else {
        format!(
            "{ahead} commit{} ahead of {base}",
            if ahead == "1" { "" } else { "s" }
        )
    };
    Some(format!(
        "branch `{branch}`: {dirty} uncommitted file{}, {ahead_text}\n",
        if dirty == 1 { "" } else { "s" }
    ))
}

/// The branch work is measured against: `base` in `.arbos/git.toml` when the
/// place (found upward from `cwd`) configures one, else `main`, else
/// `master` when only that exists.
fn base_branch(cwd: &Path, git: &dyn Fn(&[&str]) -> Option<String>) -> String {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        if let Ok(text) = std::fs::read_to_string(d.join(".arbos").join("git.toml")) {
            let base = text
                .lines()
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| k.trim() == "base")
                .map(|(_, v)| v.trim().trim_matches('"').to_string())
                .unwrap_or_default();
            if !base.is_empty() {
                return base;
            }
            break;
        }
        dir = d.parent();
    }
    if git(&["rev-parse", "--verify", "--quiet", "refs/heads/main"]).is_some() {
        return "main".into();
    }
    if git(&["rev-parse", "--verify", "--quiet", "refs/heads/master"]).is_some() {
        return "master".into();
    }
    "main".into()
}

pub fn undo(cwd: &Path, turn_line: u64) -> Result<ToolOut> {
    let mark = cwd.join(".arbos").join("runtime").join("checkpoint");
    // A confirmed read: an unreadable mark is not a mark to reset to.
    let text = match arbos_core::record::read_text(&mark).confirmed() {
        Ok(Some(t)) => t,
        Ok(None) => {
            return Ok(ToolOut::text(
                "no checkpoint for this turn (its mark was never written, or its write failed and was said on the transcript); nothing reset",
            ));
        }
        Err(e) => anyhow::bail!("undo: {e}"),
    };
    {
        let mut lines = text.lines().map(str::trim);
        let sha = lines.next().unwrap_or("");
        // The second line, from `snapshot_turn`: the work commit, `clean`,
        // or `error:<why>`. A mark from before it carries HEAD alone.
        let tree = lines.next().unwrap_or("");
        // The third: the turn the mark was written for. A mark for another
        // turn is a stale mark — its writer failed after this one's start —
        // and resetting to it deletes kept work (qal-j10).
        let for_line = lines
            .next()
            .and_then(|l| l.strip_prefix("line:"))
            .and_then(|n| n.parse::<u64>().ok());
        match for_line {
            Some(l) if l == turn_line => {}
            Some(l) => {
                return Ok(ToolOut::text(format!(
                    "no checkpoint for this turn: the mark on disk is from the turn at line {l}, this turn started at line {turn_line} (its own mark was not written); nothing reset"
                )));
            }
            None if turn_line > 0 => {
                return Ok(ToolOut::text(
                    "no checkpoint for this turn: the mark on disk names no turn (written by an older kernel, or by a start that failed part way); nothing reset",
                ));
            }
            None => {}
        }
        if !sha.is_empty() {
            let knows_tree = tree == "clean" || (!tree.is_empty() && !tree.starts_with("error:"));
            if !knows_tree {
                // `reset --hard` puts tracked files back; `clean` would
                // delete every untracked file in the project on the
                // strength of a record this mark knows it lacks (qal-j08).
                let st = Command::new("git")
                    .args(["reset", "--hard", sha])
                    .current_dir(cwd)
                    .status()?;
                if st.success() {
                    let why = tree
                        .strip_prefix("error:")
                        .unwrap_or("the mark predates the record of the tree");
                    return Ok(ToolOut::text(format!(
                        "restored tracked files to {sha}; untracked files left as they are (no checkpoint of the working tree: {why})"
                    )));
                }
            } else {
                let st = Command::new("git")
                    .args(["reset", "--hard", sha])
                    .current_dir(cwd)
                    .status()?;
                if st.success() {
                    // `.arbos/` holds the agent's own state (transcripts,
                    // lock, kernel.json) and is often untracked; it is
                    // never the turn's work, so it must survive the clean.
                    let _ = Command::new("git")
                        .args(["clean", "-fd", "-e", ".arbos", "-e", ".arbos/**"])
                        .current_dir(cwd)
                        .status();
                    if tree != "clean" {
                        // The untracked files and tracked changes of the
                        // turn's start come back from the work commit.
                        let st = Command::new("git")
                            .args(["read-tree", "-u", "--reset", tree])
                            .current_dir(cwd)
                            .status()?;
                        if !st.success() {
                            anyhow::bail!("git read-tree {tree} failed after reset to {sha}");
                        }
                        let _ = Command::new("git")
                            .args(["reset", "-q"])
                            .current_dir(cwd)
                            .status();
                        return Ok(ToolOut::text(format!(
                            "restored {sha} + working tree {}",
                            &tree[..tree.len().min(12)]
                        )));
                    }
                    return Ok(ToolOut::text(format!("restored {sha}")));
                }
            }
        }
    }
    let st = Command::new("git")
        .args(["stash", "list"])
        .current_dir(cwd)
        .output()?;
    let list = String::from_utf8_lossy(&st.stdout);
    if let Some(line) = list.lines().find(|l| l.contains(TAG)) {
        let name = line.split(':').next().unwrap_or("stash@{0}");
        let _ = Command::new("git")
            .args(["stash", "pop", name])
            .current_dir(cwd)
            .status();
        return Ok(ToolOut::text("restored stash checkpoint"));
    }
    Ok(ToolOut::text("no checkpoint"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_symbols_resolve_each_line_to_its_method_and_class() {
        let text = "class Registry:\n    def register(self, cmap, *, name=None):\n        x = 1\n        return x\n\n    def unregister(self, name):\n        pass\n\ndef set_cmap(cmap):\n    rc('image', cmap=cmap.name)\n";
        // Line 3 changed (inside register), line 10 changed (inside set_cmap).
        let diff = "@@ -3 +3 @@\n-        x = 0\n+        x = 1\n@@ -10 +10 @@\n-    rc('image', cmap=name)\n+    rc('image', cmap=cmap.name)\n";
        let got = changed_symbols(diff, text);
        assert_eq!(
            got.functions,
            vec!["register".to_string(), "set_cmap".into()]
        );
        assert_eq!(got.classes, vec!["Registry".to_string()]);
    }

    #[test]
    fn coverage_note_says_which_tests_name_the_change_or_none() {
        let dir = std::env::temp_dir().join(format!("arbos-cov-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pkg")).unwrap();
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(
            dir.join("pkg/cm.py"),
            "def register_cmap(c):\n    return c\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("pkg/pyplot.py"),
            "def set_cmap(c):\n    return c\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("tests/test_cm.py"),
            "from pkg.cm import register_cmap\n\ndef test_it():\n    assert register_cmap(1) == 1\n",
        )
        .unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "init"]);
        std::fs::write(
            dir.join("pkg/pyplot.py"),
            "def set_cmap(c):\n    return c.name\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("pkg/cm.py"),
            "def register_cmap(c):\n    return c or 0\n",
        )
        .unwrap();
        let pyplot = dir.join("pkg/pyplot.py").display().to_string();
        let cm = dir.join("pkg/cm.py").display().to_string();
        let note = coverage_note(&dir, &[pyplot]).unwrap();
        assert!(note.contains("no existing test names set_cmap"), "{note}");
        let note = coverage_note(&dir, &[cm]).unwrap();
        assert!(
            note.contains("register_cmap named in tests/test_cm.py"),
            "{note}"
        );
        let test = dir.join("tests/test_cm.py").display().to_string();
        assert!(coverage_note(&dir, &[test]).is_none());
        // A method of a class the tests import but never call by name:
        // the class match alone must not read as coverage.
        std::fs::write(
            dir.join("pkg/base.py"),
            "class Base:\n    def run(self):\n        return 1\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("tests/test_base.py"),
            "from pkg.base import Base\n",
        )
        .unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "base"]);
        std::fs::write(
            dir.join("pkg/base.py"),
            "class Base:\n    def run(self):\n        return 2\n",
        )
        .unwrap();
        let base = dir.join("pkg/base.py").display().to_string();
        let note = coverage_note(&dir, &[base]).unwrap();
        assert!(
            note.contains(
                "class-level match only (Base named in tests/test_base.py), no test names run"
            ),
            "{note}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_note_names_changed_existing_tests_only() {
        let status = " M src/lib.rs\n M tests/test_csv.py\n?? tests/test_new.py\nA  tests/test_added.py\n M pkg/foo_test.go\n";
        let note = test_files_note(status).unwrap();
        assert!(note.contains("2 existing test file(s)"), "{note}");
        assert!(note.contains("tests/test_csv.py") && note.contains("pkg/foo_test.go"));
        assert!(!note.contains("test_new.py") && !note.contains("test_added.py"));
        assert!(test_files_note(" M src/lib.rs\n").is_none());
    }

    /// qal-j08. A repository with no identity to be found (the fresh-machine
    /// shape): the checkpoint's work commit is still made, because an
    /// internal ref needs no real name.
    fn identityless_repo(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "arbos-noid-{tag}-{}-{}",
            std::process::id(),
            arbos_core::now_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let st = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        git(&["add", "a.txt"]);
        git(&[
            "-c",
            "user.name=setup",
            "-c",
            "user.email=setup@t",
            "commit",
            "-q",
            "-m",
            "start",
        ]);
        // No name to be found: the repo's own config says empty, which
        // beats whatever the machine's global config holds (this box has
        // one), and git refuses an empty ident.
        git(&["config", "user.name", ""]);
        git(&["config", "user.email", ""]);
        git(&["config", "user.useConfigOnly", "true"]);
        // The control: git itself refuses a commit here for want of a name.
        let raw = Command::new("git")
            .args(["commit-tree", "HEAD^{tree}", "-m", "x"])
            .env_remove("GIT_AUTHOR_NAME")
            .env_remove("GIT_AUTHOR_EMAIL")
            .env_remove("GIT_COMMITTER_NAME")
            .env_remove("GIT_COMMITTER_EMAIL")
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            !raw.status.success(),
            "the repo must have no identity for this test to mean anything"
        );
        dir
    }

    #[test]
    fn a_checkpoint_needs_no_git_identity_and_rewind_brings_the_files_back() {
        let dir = identityless_repo("cp");
        std::fs::write(dir.join("f1.txt"), "first\n").unwrap();
        let head = git_out(&dir, &["rev-parse", "HEAD"]).unwrap();
        let work = work_commit(&dir, &head)
            .expect("the work commit is made without a user identity")
            .expect("the tree differs from HEAD");
        assert_eq!(work.len(), 40);
        // The checkpoint as snapshot_turn writes it, then a later file, then
        // a restore: the kept file is back, the later one gone.
        let agent_dir = dir.join(".arbos/agents/root");
        std::fs::create_dir_all(&agent_dir).unwrap();
        snapshot_turn(&dir, &agent_dir, "root", 7).unwrap();
        let cps = checkpoints(&agent_dir);
        assert_eq!(cps.len(), 1);
        assert!(
            cps[0].work.is_some() && cps[0].work_error.is_none(),
            "{:?}",
            cps[0]
        );
        assert!(cps[0].knows_tree());
        let mark = std::fs::read_to_string(dir.join(".arbos/runtime/checkpoint")).unwrap();
        assert_eq!(mark.lines().nth(1), cps[0].work.as_deref(), "{mark}");
        std::fs::write(dir.join("f2.txt"), "later\n").unwrap();
        let what = restore(&dir, &cps[0]).unwrap();
        assert!(what.contains("working tree"), "{what}");
        assert!(dir.join("f1.txt").exists(), "the kept turn's file is back");
        assert!(
            !dir.join("f2.txt").exists(),
            "the later turn's file is gone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_checkpoint_that_could_not_save_the_tree_says_so_and_restore_will_not_clean_on_it() {
        let dir = identityless_repo("nocp");
        std::fs::write(dir.join("f1.txt"), "first\n").unwrap();
        let head = git_out(&dir, &["rev-parse", "HEAD"]).unwrap();
        // As the old kernels recorded it, and as a failed save records it now.
        for cp in [
            Checkpoint {
                line: 1,
                ts: 0,
                head: head.clone(),
                work: None,
                clean: false,
                work_error: None,
            },
            Checkpoint {
                line: 2,
                ts: 0,
                head: head.clone(),
                work: None,
                clean: false,
                work_error: Some("git commit-tree failed: no name".into()),
            },
        ] {
            assert!(!cp.knows_tree());
            let err = restore(&dir, &cp).unwrap_err().to_string();
            assert!(err.contains("no checkpoint of the working tree"), "{err}");
            assert!(err.contains("files left as they are"), "{err}");
            assert!(dir.join("f1.txt").exists(), "nothing untracked was removed");
        }
        // A clean checkpoint knows its tree: HEAD alone is the whole truth.
        let clean = Checkpoint {
            line: 3,
            ts: 0,
            head,
            work: None,
            clean: true,
            work_error: None,
        };
        assert!(clean.knows_tree());
        restore(&dir, &clean).unwrap();
        assert!(
            !dir.join("f1.txt").exists(),
            "a clean tree restored is a clean tree"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_without_a_known_tree_resets_tracked_files_and_leaves_untracked_ones() {
        let dir = identityless_repo("undo");
        let head = git_out(&dir, &["rev-parse", "HEAD"]).unwrap();
        std::fs::create_dir_all(dir.join(".arbos/runtime")).unwrap();
        // A mark from before the tree was recorded: HEAD alone.
        std::fs::write(dir.join(".arbos/runtime/checkpoint"), format!("{head}\n")).unwrap();
        std::fs::write(dir.join("a.txt"), "changed\n").unwrap();
        std::fs::write(dir.join("mine.txt"), "the user's own untracked file\n").unwrap();
        let out = undo(&dir, 0).unwrap();
        assert!(
            out.body.contains("untracked files left as they are"),
            "{}",
            out.body
        );
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "a\n");
        assert!(
            dir.join("mine.txt").exists(),
            "undo must not delete what it never recorded"
        );
        // With the tree known (clean), the clean is right.
        std::fs::write(
            dir.join(".arbos/runtime/checkpoint"),
            format!("{head}\nclean\n"),
        )
        .unwrap();
        std::fs::write(dir.join("new.txt"), "this turn's\n").unwrap();
        let out = undo(&dir, 0).unwrap();
        assert!(out.body.starts_with("restored "), "{}", out.body);
        assert!(!dir.join("new.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The record lands before the tree: a rewind between the two finds
    /// this turn's checkpoint (pending, refused for files), never the
    /// previous turn's.
    #[test]
    fn the_checkpoint_record_is_on_disk_before_its_tree_and_is_filled_in_after() {
        let dir = identityless_repo("record-first");
        let agent_dir = dir.join(".arbos/agents/root");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(dir.join("f1.txt"), "first\n").unwrap();
        let cp = snapshot_turn_record(&dir, &agent_dir, "root", 9)
            .unwrap()
            .expect("a git repository with a HEAD");
        let on_disk = checkpoints(&agent_dir);
        assert_eq!(on_disk.len(), 1);
        assert_eq!(on_disk[0].line, 9);
        assert!(!on_disk[0].knows_tree(), "pending, not guessed");
        assert_eq!(on_disk[0].work_error.as_deref(), Some(TREE_PENDING));
        let err = restore(&dir, &on_disk[0]).unwrap_err();
        assert!(err.downcast_ref::<NoTree>().is_some(), "{err}");
        assert!(err.to_string().contains("still being saved"), "{err}");
        assert!(dir.join("f1.txt").exists());
        // The undo mark is HEAD alone until the tree lands: `undo` refuses.
        let mark = std::fs::read_to_string(dir.join(".arbos/runtime/checkpoint")).unwrap();
        assert_eq!(mark.lines().count(), 1, "{mark}");

        snapshot_turn_tree(&dir, &agent_dir, "root", &cp).unwrap();
        let on_disk = checkpoints(&agent_dir);
        assert_eq!(
            on_disk.len(),
            1,
            "filled in place, not appended: {on_disk:?}"
        );
        assert_eq!(on_disk[0].line, 9);
        assert!(on_disk[0].work.is_some() && on_disk[0].work_error.is_none());
        let mark = std::fs::read_to_string(dir.join(".arbos/runtime/checkpoint")).unwrap();
        assert_eq!(mark.lines().nth(2), Some("line:9"), "{mark}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
