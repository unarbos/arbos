use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

use super::ToolOut;
use crate::access::Access;
use crate::tool::{BoxFuture, Plan, PlanCx, RunCx, Tool, blocking, simple_schema};

use arbos_core::wire::{FileChange, TurnChange};

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
        blocking(move || undo(&cx.cwd, cx.turn_line, cx.turn_ts))
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
    snapshot_turn_record_with_mark(cwd, agent_dir, agent, line).map(|r| r.map(|(cp, _)| cp))
}

/// `snapshot_turn_record`, and beside the record the reason the old undo
/// mark could not be cleared, when it could not — for the turn to say on
/// the transcript that this turn has no undo point.
pub fn snapshot_turn_record_with_mark(
    cwd: &Path,
    agent_dir: &Path,
    agent: &str,
    line: u64,
) -> Result<Option<(Checkpoint, Option<String>)>> {
    // The record first, in the agent's folder; the old mark's removal
    // after, and its failure is not this record's failure. Clearing the
    // mark needs write permission on `runtime/` — the resource whose
    // failure is the whole reason a stale mark would matter (qal-j22,
    // arm c: a read-only folder). With the record written, `undo`
    // refuses that stale mark by its time; with the record *not* written
    // because the mark could not be cleared, `undo` read the previous
    // record, matched the stale mark by line and time, and reset to it.
    // A recovery must not need the resource that failed.
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
    let mark_error = match snapshot(cwd) {
        Ok(()) => None,
        Err(e) => {
            // Said, not swallowed; the record stands and its time is what
            // `undo` checks the mark against.
            eprintln!(
                "checkpoint {agent}:{line}: {e:#}; undo for this turn is refused until its mark is written"
            );
            Some(format!("{e:#}"))
        }
    };
    Ok(Some((cp, mark_error)))
}

/// The ref that keeps a turn's tree commit alive: `refs/arbos/cp/<agent
/// with unsafe chars replaced>/<line>`.
pub fn checkpoint_ref(agent: &str, line: u64) -> String {
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
    format!("refs/arbos/cp/{safe}/{line}")
}

/// Drop the refs of checkpoints no rewind can reach any more: turns a
/// rewind cut, or lines a roll archived. Their tree commits become
/// unreachable and git's own gc reclaims them in time. Without this a
/// long project's repository kept every turn's working tree for ever,
/// and `git log --all` in a person's own tools showed thousands of
/// `arbos-checkpoint` commits. Best effort; a ref already gone is fine.
pub fn drop_checkpoint_refs(cwd: &Path, agent: &str, lines: impl IntoIterator<Item = u64>) {
    for line in lines {
        let _ = Command::new("git")
            .args(["update-ref", "-d", &checkpoint_ref(agent, line)])
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// Every checkpoint ref of `agent`: for a worker being archived, whose
/// checkpoints leave the live folder with it.
pub fn drop_agent_checkpoint_refs(cwd: &Path, agent: &str) {
    let prefix = checkpoint_ref(agent, 0);
    let prefix = prefix.trim_end_matches("/0");
    let Some(list) = git_out(cwd, &["for-each-ref", "--format=%(refname)", prefix]) else {
        return;
    };
    for r in list.lines().map(str::trim).filter(|r| !r.is_empty()) {
        let _ = Command::new("git")
            .args(["update-ref", "-d", r])
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
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
    snapshot_turn_tree_unless(cwd, agent_dir, agent, cp, None)
}

/// What the record says when the tree was abandoned: the turn went on
/// before the snapshot finished, so a tree taken now might hold the
/// turn's own changes and is not kept (qal-j17's wrong-checkpoint shape).
pub const TREE_TOO_SLOW: &str = "tree not saved: the working tree took too long to snapshot and the turn went on without it — a rewind of files to this turn is refused; the transcript rewind still works";

/// `snapshot_turn_tree`, with a flag the turn raises when it stopped
/// waiting for the tree: a tree finished after that is dropped and the
/// record says why, rather than filled in from a state the turn may
/// already have touched.
pub fn snapshot_turn_tree_unless(
    cwd: &Path,
    agent_dir: &Path,
    agent: &str,
    cp: &Checkpoint,
    abandoned: Option<&std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let line = cp.line;
    let head = &cp.head;
    // Test knob: a slow `add -A`, as a large repository has.
    if let Some(ms) = std::env::var("ARBOS_TEST_TREE_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    let (work, clean, work_error) = match work_commit(cwd, head) {
        _ if abandoned.is_some_and(|a| a.load(std::sync::atomic::Ordering::SeqCst)) => {
            eprintln!("checkpoint {agent}:{line}: {TREE_TOO_SLOW}");
            (None, false, Some(TREE_TOO_SLOW.to_string()))
        }
        Ok(Some(w)) => (Some(w), false, None),
        Ok(None) => (None, true, None),
        Err(why) => {
            eprintln!("checkpoint {agent}:{line}: working tree not saved: {why}");
            (None, false, Some(why))
        }
    };
    if let Some(w) = &work {
        let _ = Command::new("git")
            .args(["update-ref", &checkpoint_ref(agent, line), w])
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
    // A mark that could not be written is removed rather than left
    // stale: `undo` on a mark from an earlier turn would reset HEAD to
    // that turn's commit. No mark → `undo` refuses and says so (#444).
    // Written atomically, with the line the turn began at (#392).
    // And with the record's own time: a line recurs after a rewind, and a
    // stale mark that could not be removed (a read-only folder, qal-j22)
    // would match a new turn at the same line by line alone.
    if let Err(e) = arbos_core::record::write_atomic(
        &mark,
        format!("{head}\n{second}\nline:{line}\nts:{}\n", cp.ts).as_bytes(),
    ) {
        let _ = std::fs::remove_file(&mark);
        return Err(e.context("the undo mark could not be written; undo is refused for this turn"));
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
    atomic_write(
        &tree_sidecar(agent_dir, line),
        serde_json::to_string(&filled)?.as_bytes(),
    )?;
    // Then this turn's line rewritten with the tree; a confirmed read,
    // whole or not at all.
    let path = agent_dir.join("checkpoints.jsonl");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
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
        atomic_write(&path, out.as_bytes())?;
    }
    Ok(())
}

/// Write whole or not at all: a sibling temp file renamed over `path`.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("file has no parent folder")?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))
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
        // The sidecar is *this* record's when its `ts` is this record's:
        // the fact that identifies it. Line and HEAD alone took a cut
        // turn's sidecar for the new turn at the same line (qal-j20: HEAD
        // had not moved all session, the common case) and restored the
        // tree the person had rewound away, saying restored.
        if let Ok(text) = std::fs::read_to_string(&sidecar)
            && let Ok(filled) = serde_json::from_str::<Checkpoint>(&text)
            && filled.ts == cp.ts
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
    let tree = work_tree(cwd)?;
    let head_tree = git_out(cwd, &["rev-parse", &format!("{head}^{{tree}}")]).ok_or_else(|| {
        // The head the record took a moment ago is not a commit here now:
        // the one CI flake this path has (#527's test, ~once a day) and
        // nothing reproduces it. What the next red needs to say: what
        // HEAD is now, whether the object file is on disk, and which
        // repository this is.
        let now = git_out(cwd, &["rev-parse", "HEAD"]).unwrap_or_else(|| "(no HEAD)".into());
        let loose = cwd
            .join(".git")
            .join("objects")
            .join(head.get(..2).unwrap_or(""))
            .join(head.get(2..).unwrap_or(""));
        let git_dir = git_out(cwd, &["rev-parse", "--git-dir"]).unwrap_or_else(|| "(none)".into());
        format!(
            "git rev-parse {head}^{{tree}} failed (HEAD is now {now}; loose object {}: {}; git dir {git_dir}; cwd {})",
            loose.display(),
            if loose.exists() { "present" } else { "absent" },
            cwd.display()
        )
    })?;
    if tree == head_tree {
        return Ok(None);
    }
    let out = Command::new("git")
        .args(["commit-tree", &tree, "-p", head, "-m", "arbos checkpoint"])
        .envs(CHECKPOINT_IDENTITY.iter().copied())
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("git commit-tree: {e}"))?;
    if out.status.success() {
        Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ))
    } else {
        Err(format!(
            "git commit-tree failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Each scratch index gets a name of its own: a checkpoint on the
/// blocking pool and a `turn_changes` read may build one at the same
/// moment in the same process.
static SCRATCH_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The working tree as a git tree object — every file `add -A` would
/// take, `.arbos/` excluded — written through a scratch copy of the
/// index so the real one is not touched. The tree is in the object store
/// unreferenced; a checkpoint hangs a commit on it, a diff reads it.
pub fn work_tree(cwd: &Path) -> Result<String, String> {
    let index = git_out(cwd, &["rev-parse", "--git-path", "index"])
        .ok_or_else(|| "git rev-parse --git-path index failed".to_string())?;
    let index = cwd.join(index);
    let n = SCRATCH_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = cwd
        .join(".arbos")
        .join(format!("index-scratch-{}-{n}", std::process::id()));
    let _ = std::fs::create_dir_all(cwd.join(".arbos"));
    // Another git in the same repository (a person's `git commit` in a
    // terminal) renames `index.lock` over `index` while this runs: for an
    // instant the index is not a regular file, and the copy failed with
    // "the source path is neither a regular file nor a symlink" (seen
    // once by QA, beside their harness's own commit). A few tries over a
    // quarter of a second; no index at all is an empty one.
    let mut tries = 0;
    loop {
        if !index.exists() {
            break;
        }
        match std::fs::copy(&index, &scratch) {
            Ok(_) => break,
            Err(e) if tries < 10 => {
                tries += 1;
                let _ = e;
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) => return Err(format!("copy the index: {e}")),
        }
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
        .join(format!("index-scratch-excludes-{}-{n}", std::process::id()));
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
        run(&["write-tree"])
    })();
    let _ = std::fs::remove_file(&scratch);
    let _ = std::fs::remove_file(&excludes);
    result
}

/// The tree a checkpoint stands for: its work commit, or HEAD's tree when
/// the working tree equalled it. None when the record does not know.
pub fn checkpoint_tree(cp: &Checkpoint) -> Option<String> {
    match (&cp.work, cp.clean) {
        (Some(w), _) => Some(w.clone()),
        (None, true) => Some(cp.head.clone()),
        (None, false) => None,
    }
}

/// The files that differ between two trees (commits or tree objects), as
/// `git diff --numstat` and `--name-status` count them. Renames detected.
pub fn diff_trees(cwd: &Path, from: &str, to: &str) -> Result<Vec<FileChange>, String> {
    let run = |args: &[&str]| -> Result<String, String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("git diff: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    };
    // `-z`: paths as they are, no quoting; a rename carries two.
    let status = run(&["diff", "-M", "--name-status", "-z", from, to])?;
    let numstat = run(&["diff", "-M", "--numstat", "-z", from, to])?;
    let mut counts: std::collections::HashMap<String, (u64, u64, bool)> =
        std::collections::HashMap::new();
    let mut fields = numstat.split('\0').filter(|s| !s.is_empty()).peekable();
    while let Some(head) = fields.next() {
        // `added\tremoved\tpath`, or `added\tremoved\t` then `from`, `to`
        // for a rename; `-\t-` for binary.
        let mut parts = head.splitn(3, '\t');
        let (Some(a), Some(r)) = (parts.next(), parts.next()) else {
            continue;
        };
        let binary = a == "-";
        let (added, removed) = if binary {
            (0, 0)
        } else {
            (a.parse().unwrap_or(0), r.parse().unwrap_or(0))
        };
        let path = match parts.next().filter(|p| !p.is_empty()) {
            Some(p) => p.to_string(),
            None => {
                let _from = fields.next();
                fields.next().unwrap_or_default().to_string()
            }
        };
        counts.insert(path, (added, removed, binary));
    }
    let mut out = Vec::new();
    let mut fields = status.split('\0').filter(|s| !s.is_empty());
    while let Some(code) = fields.next() {
        let Some(first) = fields.next() else { break };
        let (kind, from, path) = match code.chars().next() {
            Some('A') => ("added", String::new(), first.to_string()),
            Some('D') => ("deleted", String::new(), first.to_string()),
            Some('M') => ("modified", String::new(), first.to_string()),
            Some('T') => ("typechange", String::new(), first.to_string()),
            Some('R') | Some('C') => {
                let to = fields.next().unwrap_or_default().to_string();
                ("renamed", first.to_string(), to)
            }
            _ => ("modified", String::new(), first.to_string()),
        };
        let (added, removed, binary) = counts.get(&path).copied().unwrap_or((0, 0, false));
        out.push(FileChange {
            path,
            kind: kind.to_string(),
            from,
            added,
            removed,
            binary,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// What each of the newest `limit` turns did to the working tree, oldest
/// first: turn N's files are the difference between the tree saved at its
/// start and the one saved at the next turn's start; the newest turn is
/// measured against the tree as it stands now (its row may still grow).
/// `touched` comes from the transcript: the paths each tool call of the
/// turn reported. Outside git, or with the tree unsaved, the row says so
/// in `unmeasured` instead of guessing.
pub fn turn_changes(
    cwd: &Path,
    agent_dir: &Path,
    events: &[arbos_core::Event],
    limit: usize,
) -> Vec<TurnChange> {
    let cps = checkpoints(agent_dir);
    if cps.is_empty() {
        return Vec::new();
    }
    let start = cps.len().saturating_sub(limit.max(1));
    // A turn that just began has its tree on the way; a moment's wait
    // reads it filled rather than reporting it unsaved.
    let cps: Vec<Checkpoint> = cps
        .iter()
        .enumerate()
        .map(|(i, cp)| {
            if i + 1 >= start {
                settle_tree(agent_dir, cp, std::time::Duration::from_secs(2))
            } else {
                cp.clone()
            }
        })
        .collect();
    let in_git = git_out(cwd, &["rev-parse", "--git-dir"]).is_some();
    // One `add -A` for the newest turn, made once whatever `limit` is.
    let now_tree = if in_git { Some(work_tree(cwd)) } else { None };
    let mut out = Vec::with_capacity(cps.len() - start);
    for (i, cp) in cps.iter().enumerate().skip(start) {
        let next = cps.get(i + 1);
        let hi = next.map(|n| n.line).unwrap_or(u64::MAX);
        let mut touched: Vec<String> = events
            .iter()
            .filter(|e| e.seq > cp.line && e.seq <= hi)
            .filter_map(|e| match &e.kind {
                arbos_core::EventKind::Tool(rec) => Some(rec.paths.iter()),
                _ => None,
            })
            .flatten()
            // Tools record absolute paths; the row speaks as git does,
            // relative to the place.
            .map(|p| {
                Path::new(p)
                    .strip_prefix(cwd)
                    .map(|r| r.display().to_string())
                    .unwrap_or_else(|_| p.clone())
            })
            .collect();
        touched.sort();
        touched.dedup();
        let mut row = TurnChange {
            line: cp.line,
            ts: cp.ts,
            ended: next.is_some(),
            touched,
            ..Default::default()
        };
        if !in_git {
            row.unmeasured = "not a git repository: files are not measured".into();
            out.push(row);
            continue;
        }
        let from = match checkpoint_tree(cp) {
            Some(t) => t,
            None => {
                row.unmeasured = match &cp.work_error {
                    Some(why) => format!("the tree at this turn's start was not saved: {why}"),
                    None => "the tree at this turn's start is not on record".into(),
                };
                out.push(row);
                continue;
            }
        };
        let to = match next {
            Some(n) => match checkpoint_tree(n) {
                Some(t) => Ok(t),
                None => Err(match &n.work_error {
                    Some(why) => format!("the tree at the next turn's start was not saved: {why}"),
                    None => "the tree at the next turn's start is not on record".into(),
                }),
            },
            None => now_tree.clone().unwrap_or_else(|| Err("no tree".into())),
        };
        match to.and_then(|to| diff_trees(cwd, &from, &to)) {
            Ok(files) => row.files = files,
            Err(why) => row.unmeasured = why,
        }
        out.push(row);
    }
    out
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
    // Nothing destructive until everything the restore needs is known to
    // be there (qal-j16: a `reset --hard` ran, then `read-tree` failed on
    // a missing object, and the person's own later commit was gone from
    // the tree while the message said "files not restored"). The
    // checkpoint's commit and its tree are checked first; then the tree
    // as it stands now is committed the same way a checkpoint's is, so
    // a failure after this point can put everything back.
    for (what, obj) in [("HEAD", format!("{}^{{commit}}", cp.head))]
        .into_iter()
        .chain(
            cp.work
                .iter()
                .map(|w| ("working tree", format!("{w}^{{tree}}"))),
        )
    {
        let ok = Command::new("git")
            .args(["cat-file", "-e", &obj])
            .current_dir(cwd)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            anyhow::bail!(
                "the checkpoint's {what} ({}) is not in the repository (a missing or corrupt object); files left as they are",
                &obj[..obj.len().min(12)]
            );
        }
    }
    let before_head = git_out(cwd, &["rev-parse", "HEAD"])
        .context("git rev-parse HEAD failed; files left as they are")?;
    let before_work = work_commit(cwd, &before_head).map_err(|e| {
        anyhow::anyhow!(
            "could not keep a copy of the tree as it stands ({e}); files left as they are"
        )
    })?;
    let put_back = |why: String| -> anyhow::Error {
        let mut steps = vec![
            Command::new("git")
                .args(["reset", "--hard", &before_head])
                .current_dir(cwd)
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
        ];
        if let Some(w) = &before_work {
            steps.push(
                Command::new("git")
                    .args(["read-tree", "-u", "--reset", w])
                    .current_dir(cwd)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false),
            );
            let _ = Command::new("git")
                .args(["reset", "-q"])
                .current_dir(cwd)
                .status();
        }
        // Judged by the tree's state, not the last command's exit code:
        // `read-tree -u` returned non-zero on an unwritable folder after
        // the tree was already right, and the person with a good tree
        // was told it was broken and handed commands to run (QA's
        // rw-08c). HEAD is compared, and the working tree is committed
        // the same way once more and its tree id compared.
        let head_now = git_out(cwd, &["rev-parse", "HEAD"]);
        let tree_of = |commit: Option<&str>| -> Option<String> {
            let c = commit.unwrap_or(&before_head);
            git_out(cwd, &["rev-parse", &format!("{c}^{{tree}}")])
        };
        let tree_before = tree_of(before_work.as_deref());
        let tree_now = match work_commit(cwd, &before_head) {
            Ok(now) => tree_of(now.as_deref()),
            Err(_) => None,
        };
        let same = head_now.as_deref() == Some(before_head.as_str())
            && tree_before.is_some()
            && tree_now == tree_before;
        let _ = steps;
        if same {
            anyhow::anyhow!(
                "{why}; the tree was put back as it was (HEAD {}{})",
                &before_head[..before_head.len().min(12)],
                before_work
                    .as_deref()
                    .map(|w| format!(", your files from {}", &w[..w.len().min(12)]))
                    .unwrap_or_default()
            )
        } else {
            let what = if head_now.as_deref() != Some(before_head.as_str()) {
                format!(
                    "HEAD is at {} instead of {}",
                    head_now
                        .as_deref()
                        .map(|h| &h[..h.len().min(12)])
                        .unwrap_or("?"),
                    &before_head[..before_head.len().min(12)]
                )
            } else {
                "the working tree differs from what it was".to_string()
            };
            anyhow::anyhow!(
                "{why}; and the tree could not be put back ({what}) — recover by hand: git reset --hard {}{}",
                before_head,
                before_work
                    .as_deref()
                    .map(|w| format!(" && git read-tree -u --reset {w} && git reset -q"))
                    .unwrap_or_default()
            )
        }
    };
    // Order: HEAD back, then the checkpoint's tree — tracked changes and
    // the untracked files of that moment — into the index and working
    // tree, then everything the checkpoint did not have goes (never
    // `.arbos/`), and the index returns to HEAD so it all shows as it
    // did. The tree comes back *before* anything untracked is removed:
    // with the old order (clean, then read-tree) a read-tree that failed
    // left the person with neither the later files nor the checkpoint's.
    let st = Command::new("git")
        .args(["reset", "--hard", &cp.head])
        .current_dir(cwd)
        .status()?;
    if !st.success() {
        return Err(put_back(format!("git reset --hard {} failed", cp.head)));
    }
    if let Some(work) = &cp.work {
        let st = Command::new("git")
            .args(["read-tree", "-u", "--reset", work])
            .current_dir(cwd)
            .status()?;
        if !st.success() {
            return Err(put_back(format!("git read-tree {work} failed")));
        }
    }
    // What `clean` did is part of what was restored: a failure here is
    // not "restored" with leftovers unmentioned.
    let clean = Command::new("git")
        .args(["clean", "-fd", "-e", ".arbos", "-e", ".arbos/**"])
        .current_dir(cwd)
        .output();
    let clean_note = match &clean {
        Ok(o) if o.status.success() => None,
        Ok(o) => Some(format!(
            "untracked files from later turns may remain (git clean: {})",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Some(format!(
            "untracked files from later turns may remain (git clean: {e})"
        )),
    };
    if cp.work.is_some() {
        let _ = Command::new("git")
            .args(["reset", "-q"])
            .current_dir(cwd)
            .status();
    }
    let mut what = match &cp.work {
        Some(w) => format!(
            "{} + working tree {}",
            &cp.head[..cp.head.len().min(12)],
            &w[..w.len().min(12)]
        ),
        None => cp.head[..cp.head.len().min(12)].to_string(),
    };
    if let Some(note) = clean_note {
        what.push_str("; ");
        what.push_str(&note);
    }
    Ok(what)
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

pub fn undo(cwd: &Path, turn_line: u64, turn_ts: Option<i64>) -> Result<ToolOut> {
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
        // The fourth: the record's time. The one fact that survives a
        // rewind reusing the line and a folder that refused the removal
        // of a stale mark (qal-j22, arm c): a mark this turn did not write
        // cannot carry this turn's time.
        let for_ts = lines
            .next()
            .and_then(|l| l.strip_prefix("ts:"))
            .and_then(|n| n.parse::<i64>().ok());
        match (turn_ts, for_ts) {
            (Some(want), got) if got != Some(want) => {
                return Ok(ToolOut::text(format!(
                    "no checkpoint for this turn: the mark on disk is another turn's (its time {} is not this turn's {want}; this turn's own mark was not written); nothing reset",
                    got.map(|t| t.to_string())
                        .unwrap_or_else(|| "unrecorded".into())
                )));
            }
            // This turn has no record, and the mark names a turn that had
            // one: not this turn's.
            (None, Some(_)) if turn_line > 0 => {
                return Ok(ToolOut::text(
                    "no checkpoint for this turn: it has no record, and the mark on disk belongs to a turn that had one; nothing reset",
                ));
            }
            _ => {}
        }
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

    /// `diff_trees` reads what `git diff` says between two trees: an add,
    /// a change with its line counts, a delete, a rename (with where it
    /// came from), a binary file (no counts). The working tree's own
    /// tree object (`work_tree`) diffs like any commit.
    #[test]
    fn diff_trees_names_each_kind_of_change_with_its_counts() {
        let dir = std::env::temp_dir().join(format!("arbos-difftrees-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| -> String {
            let out = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("keep.txt"), "a\nb\nc\n").unwrap();
        std::fs::write(dir.join("gone.txt"), "x\n").unwrap();
        std::fs::write(
            dir.join("old-name.txt"),
            "same content\nline two\nline three\n",
        )
        .unwrap();
        std::fs::write(dir.join("pic.bin"), [0u8, 159, 146, 150, 0, 1]).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "one"]);
        let one = git(&["rev-parse", "HEAD"]);
        std::fs::write(dir.join("keep.txt"), "a\nB\nc\nd\n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::rename(dir.join("old-name.txt"), dir.join("new-name.txt")).unwrap();
        std::fs::write(dir.join("pic.bin"), [0u8, 159, 146, 150, 0, 2, 3]).unwrap();
        std::fs::write(dir.join("fresh.txt"), "1\n2\n").unwrap();
        let now = work_tree(&dir).unwrap();
        let files = diff_trees(&dir, &one, &now).unwrap();
        let by = |p: &str| {
            files
                .iter()
                .find(|f| f.path == p)
                .unwrap_or_else(|| panic!("{p}: {files:?}"))
        };
        assert_eq!(files.len(), 5, "{files:?}");
        let keep = by("keep.txt");
        assert_eq!(
            (keep.kind.as_str(), keep.added, keep.removed),
            ("modified", 2, 1)
        );
        assert_eq!(by("gone.txt").kind, "deleted");
        let fresh = by("fresh.txt");
        assert_eq!((fresh.kind.as_str(), fresh.added), ("added", 2));
        let renamed = by("new-name.txt");
        assert_eq!(
            (renamed.kind.as_str(), renamed.from.as_str()),
            ("renamed", "old-name.txt")
        );
        let pic = by("pic.bin");
        assert!(pic.binary && pic.added == 0 && pic.removed == 0, "{pic:?}");
        // The scratch index left nothing behind and touched nothing real.
        assert!(
            !dir.join(".arbos").exists()
                || std::fs::read_dir(dir.join(".arbos"))
                    .unwrap()
                    .next()
                    .is_none()
        );
        assert_eq!(
            git(&["diff", "--cached", "--name-only"]),
            "",
            "the real index is untouched"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
        // And the kept file changed and removed since: both come back.
        std::fs::remove_file(dir.join("f1.txt")).unwrap();
        let what = restore(&dir, &cps[0]).unwrap();
        assert!(what.contains("working tree"), "{what}");
        assert!(
            !what.contains("may remain"),
            "a clean that worked is not reported as doubtful: {what}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("f1.txt")).unwrap(),
            "first\n",
            "the kept turn's file is back with its content"
        );
        assert!(
            !dir.join("f2.txt").exists(),
            "the later turn's file is gone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The loose object of `sha` removed from an uncompressed repository:
    /// what a corrupt store looks like to git.
    fn drop_object(dir: &Path, sha: &str) {
        let p = dir.join(".git/objects").join(&sha[..2]).join(&sha[2..]);
        std::fs::remove_file(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    }

    fn git_ok(dir: &Path, args: &[&str]) {
        let st = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?}");
    }

    /// The person's tree after the checkpoint: their own commit of `f2`,
    /// `f1` edited by hand, a note of their own. Returns HEAD.
    fn later_work(dir: &Path) -> String {
        git_ok(dir, &["add", "f2.txt"]);
        git_ok(
            dir,
            &[
                "-c",
                "user.name=me",
                "-c",
                "user.email=me@x",
                "commit",
                "-q",
                "-m",
                "mine",
            ],
        );
        std::fs::write(dir.join("f1.txt"), "edited by hand\n").unwrap();
        std::fs::write(dir.join("my-notes.txt"), "keep\n").unwrap();
        git_out(dir, &["rev-parse", "HEAD"]).unwrap()
    }

    /// QA's general property for every rewind that reports an error:
    /// HEAD, the index and the working tree (every file and its bytes,
    /// `.git` and `.arbos` aside) are what they were before.
    fn state(dir: &Path) -> (String, String, Vec<(String, Vec<u8>)>) {
        fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                if p.is_dir() {
                    if name != ".git" && name != ".arbos" {
                        walk(root, &p, out);
                    }
                } else {
                    out.push((
                        p.strip_prefix(root).unwrap().display().to_string(),
                        std::fs::read(&p).unwrap(),
                    ));
                }
            }
        }
        let mut files = Vec::new();
        walk(dir, dir, &mut files);
        files.sort();
        (
            git_out(dir, &["rev-parse", "HEAD"]).unwrap(),
            git_out(dir, &["ls-files", "-s"]).unwrap(),
            files,
        )
    }

    fn tree_as_it_was(dir: &Path, head: &str) {
        assert_eq!(
            git_out(dir, &["rev-parse", "HEAD"]).unwrap(),
            head,
            "HEAD moved"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("f1.txt")).unwrap(),
            "edited by hand\n"
        );
        assert!(
            dir.join("f2.txt").exists(),
            "the person's committed file is gone"
        );
        assert!(dir.join("f3.txt").exists());
        assert!(dir.join("my-notes.txt").exists());
    }

    /// qal-j16: `reset --hard` ran before `read-tree`, so a restore that
    /// then failed on a missing object had already moved HEAD and taken
    /// the person's own later commit out of the tree — while the message
    /// said "files not restored". A checkpoint whose objects are missing
    /// is refused before anything moves.
    #[test]
    fn a_restore_whose_checkpoint_is_corrupt_is_refused_with_the_tree_untouched() {
        let dir = identityless_repo("corrupt-cp");
        for f in ["f1", "f2", "f3"] {
            std::fs::write(dir.join(format!("{f}.txt")), format!("{f}\n")).unwrap();
        }
        let head = git_out(&dir, &["rev-parse", "HEAD"]).unwrap();
        let work = work_commit(&dir, &head).unwrap().unwrap();
        let cp = Checkpoint {
            line: 9,
            ts: 0,
            head: head.clone(),
            work: Some(work.clone()),
            clean: false,
            work_error: None,
        };
        let mine = later_work(&dir);
        drop_object(&dir, &work);
        let before = state(&dir);
        let err = restore(&dir, &cp).unwrap_err().to_string();
        assert!(err.contains("not in the repository"), "{err}");
        assert!(err.contains("files left as they are"), "{err}");
        tree_as_it_was(&dir, &mine);
        assert_eq!(
            state(&dir),
            before,
            "a restore that reports an error changed the tree"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// QA's rw-08c: the put-back's own `read-tree` returns non-zero on a
    /// folder it cannot write, *after* the tree is already right — and the
    /// message judged by that exit code told a person with a good tree
    /// that it was broken. The put-back is judged by the tree's state.
    #[cfg(unix)]
    #[test]
    fn a_put_back_that_leaves_the_tree_right_is_reported_right_whatever_git_returned() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = identityless_repo("putback-ro");
        for f in ["f1", "f2", "f3"] {
            std::fs::write(dir.join(format!("{f}.txt")), format!("{f}\n")).unwrap();
        }
        let head = git_out(&dir, &["rev-parse", "HEAD"]).unwrap();
        let work = work_commit(&dir, &head).unwrap().unwrap();
        let cp = Checkpoint {
            line: 9,
            ts: 0,
            head: head.clone(),
            work: Some(work.clone()),
            clean: false,
            work_error: None,
        };
        let mine = later_work(&dir);
        // A folder git cannot write into, holding a file of the person's.
        std::fs::create_dir(dir.join("later-dir")).unwrap();
        std::fs::write(dir.join("later-dir/keep.txt"), "keep\n").unwrap();
        std::fs::set_permissions(
            dir.join("later-dir"),
            std::fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        std::fs::write(dir.join("f3.txt"), "f3 changed\n").unwrap();
        let blob = git_out(&dir, &["rev-parse", &format!("{work}:f3.txt")]).unwrap();
        drop_object(&dir, &blob);
        let before = state(&dir);
        let err = restore(&dir, &cp).unwrap_err().to_string();
        let after = state(&dir);
        std::fs::set_permissions(
            dir.join("later-dir"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert_eq!(
            after, before,
            "a restore that reports an error changed the tree"
        );
        assert!(
            err.contains("the tree was put back as it was"),
            "a good tree is not called broken: {err}"
        );
        assert!(!err.contains("recover by hand"), "{err}");
        tree_as_it_was(&dir, &mine);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The checks pass (the tree object is there) and `read-tree` still
    /// fails, on a blob the tree names that is gone: the tree is put back
    /// as it was, and the message says so.
    #[test]
    fn a_restore_that_fails_after_the_checks_puts_the_tree_back() {
        let dir = identityless_repo("putback");
        for f in ["f1", "f2", "f3"] {
            std::fs::write(dir.join(format!("{f}.txt")), format!("{f}\n")).unwrap();
        }
        let head = git_out(&dir, &["rev-parse", "HEAD"]).unwrap();
        let work = work_commit(&dir, &head).unwrap().unwrap();
        let cp = Checkpoint {
            line: 9,
            ts: 0,
            head: head.clone(),
            work: Some(work.clone()),
            clean: false,
            work_error: None,
        };
        let mine = later_work(&dir);
        // f3's blob, as the checkpoint's tree names it — and f3 changed
        // since, so the copy of the current tree does not write that
        // blob back (the same content would; git stores by content).
        std::fs::write(dir.join("f3.txt"), "f3 changed\n").unwrap();
        let blob = git_out(&dir, &["rev-parse", &format!("{work}:f3.txt")]).unwrap();
        drop_object(&dir, &blob);
        let before = state(&dir);
        let err = restore(&dir, &cp).unwrap_err().to_string();
        assert!(err.contains("read-tree") && err.contains("failed"), "{err}");
        assert!(err.contains("the tree was put back as it was"), "{err}");
        tree_as_it_was(&dir, &mine);
        assert_eq!(
            state(&dir),
            before,
            "a restore that reports an error changed the tree"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("f3.txt")).unwrap(),
            "f3 changed\n"
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
        let out = undo(&dir, 0, None).unwrap();
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
        let out = undo(&dir, 0, None).unwrap();
        assert!(out.body.starts_with("restored "), "{}", out.body);
        assert!(!dir.join("new.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// qal-j22, arm (c): the runtime folder went read-only mid-session (an
    /// ext4 remount on one I/O error), so the new turn's mark could not be
    /// written *and* the stale one could not be removed — the recovery
    /// needed the resource that failed. `undo` must not believe a mark it
    /// cannot prove is this turn's: the line stamp catches a different
    /// line, and the time stamp catches the same line reused after a
    /// rewind. Both staged.
    #[cfg(unix)]
    #[test]
    fn a_stale_mark_that_could_not_be_removed_is_not_believed_by_line_or_by_time() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = identityless_repo("undo-ro-folder");
        let agent_dir = dir.join(".arbos/agents/root");
        std::fs::create_dir_all(&agent_dir).unwrap();
        // Turn one: a record and its mark; the person then commits.
        std::fs::write(dir.join("a.txt"), "turn one\n").unwrap();
        let old = snapshot_turn_record(&dir, &agent_dir, "root", 7)
            .unwrap()
            .unwrap();
        snapshot_turn_tree(&dir, &agent_dir, "root", &old).unwrap();
        let old_head = git_out(&dir, &["rev-parse", "HEAD"]).unwrap();
        for args in [
            vec!["add", "a.txt"],
            vec![
                "-c",
                "user.name=me",
                "-c",
                "user.email=me@x",
                "commit",
                "-q",
                "-m",
                "mine",
            ],
        ] {
            assert!(
                Command::new("git")
                    .args(&args)
                    .current_dir(&dir)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let mine = git_out(&dir, &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(mine, old_head);
        // The folder goes read-only. A new turn — at the SAME line, as
        // after a rewind — cannot write its mark, and cannot remove the
        // old one either.
        let runtime = dir.join(".arbos/runtime");
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o555)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let fresh = snapshot_turn_record(&dir, &agent_dir, "root", 7)
            .unwrap()
            .unwrap();
        let tree = snapshot_turn_tree(&dir, &agent_dir, "root", &fresh);
        assert!(
            tree.is_err(),
            "the mark's write fails on the read-only folder"
        );
        let stale = std::fs::read_to_string(runtime.join("checkpoint")).unwrap();
        assert!(
            stale.contains(&format!("ts:{}", old.ts)),
            "the stale mark survived: {stale}"
        );
        // The record was written although the mark could not be cleared:
        // the kernel's undo (last record's line and time) sees the new
        // turn, not the old one.
        let last = checkpoints(&agent_dir).last().cloned().expect("a record");
        assert_eq!(
            (last.line, last.ts),
            (fresh.line, fresh.ts),
            "the new turn's record stands"
        );
        // undo for the new turn: same line, different time → refused.
        let out = undo(&dir, last.line, Some(last.ts)).unwrap();
        assert!(
            out.body.contains("no checkpoint for this turn"),
            "{}",
            out.body
        );
        assert!(out.body.contains("another turn's"), "{}", out.body);
        assert_eq!(
            git_out(&dir, &["rev-parse", "HEAD"]).unwrap(),
            mine,
            "HEAD did not move"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "turn one\n"
        );
        // And by line, as #392 does, for a turn at another line.
        let out = undo(&dir, 12, Some(fresh.ts)).unwrap();
        assert!(
            out.body.contains("no checkpoint for this turn"),
            "{}",
            out.body
        );
        assert_eq!(git_out(&dir, &["rev-parse", "HEAD"]).unwrap(), mine);
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A turn's tree commit is kept alive by its ref; a cut, a roll or an
    /// archive drops the ref, and git may reclaim the tree. Without the
    /// drop a project carried every turn's working tree for ever.
    #[test]
    fn checkpoint_refs_are_dropped_by_line_and_by_agent() {
        let dir = identityless_repo("cp-refs");
        let agent_dir = dir.join(".arbos/agents/root");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let refs = || {
            git_out(
                &dir,
                &["for-each-ref", "--format=%(refname)", "refs/arbos/cp/"],
            )
            .unwrap_or_default()
        };
        for (line, name) in [(3u64, "a.txt"), (7, "b.txt")] {
            std::fs::write(dir.join(name), format!("{line}\n")).unwrap();
            let cp = snapshot_turn_record(&dir, &agent_dir, "root", line)
                .unwrap()
                .unwrap();
            snapshot_turn_tree(&dir, &agent_dir, "root", &cp).unwrap();
        }
        assert!(refs().contains("refs/arbos/cp/root/3"), "{}", refs());
        assert!(refs().contains("refs/arbos/cp/root/7"), "{}", refs());
        drop_checkpoint_refs(&dir, "root", [3]);
        assert!(!refs().contains("refs/arbos/cp/root/3"), "{}", refs());
        assert!(
            refs().contains("refs/arbos/cp/root/7"),
            "the kept turn's ref stays"
        );
        // A ref already gone is fine.
        drop_checkpoint_refs(&dir, "root", [3, 99]);
        // A worker's whole set, by agent; another agent's untouched.
        std::fs::write(dir.join("c.txt"), "c\n").unwrap();
        let w_dir = dir.join(".arbos/agents/w1");
        std::fs::create_dir_all(&w_dir).unwrap();
        let cp = snapshot_turn_record(&dir, &w_dir, "w1", 2)
            .unwrap()
            .unwrap();
        snapshot_turn_tree(&dir, &w_dir, "w1", &cp).unwrap();
        assert!(refs().contains("refs/arbos/cp/w1/2"), "{}", refs());
        drop_agent_checkpoint_refs(&dir, "w1");
        assert!(!refs().contains("refs/arbos/cp/w1/"), "{}", refs());
        assert!(refs().contains("refs/arbos/cp/root/7"), "{}", refs());
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
        assert!(
            err.to_string()
                .contains("no checkpoint of the working tree"),
            "{err}"
        );
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
        // The undo mark carries the tree (HEAD, the work commit) and the
        // turn line it belongs to, so a stale mark is refused.
        let mark = std::fs::read_to_string(dir.join(".arbos/runtime/checkpoint")).unwrap();
        assert_eq!(mark.lines().count(), 4, "{mark}");
        assert_eq!(mark.lines().nth(1), on_disk[0].work.as_deref(), "{mark}");
        assert_eq!(
            mark.lines().nth(3),
            Some(format!("ts:{}", cp.ts).as_str()),
            "{mark}"
        );
        assert_eq!(mark.lines().nth(2), Some("line:9"), "{mark}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// qal-j20 (QA's `fm-01`): a rewind cut turns whose sidecars stayed
    /// in `checkpoints.d/`; a new turn at the same line, tree pending,
    /// was settled from the cut turn's sidecar because line and HEAD
    /// matched (HEAD had not moved all session), and a rewind restored
    /// the tree the person had rewound away, saying restored. The
    /// sidecar is this record's only when its `ts` is this record's.
    #[test]
    fn a_cut_turns_sidecar_at_the_same_line_is_not_taken_for_the_new_turn() {
        let dir = identityless_repo("stale-sidecar");
        let agent_dir = dir.join(".arbos/agents/root");
        std::fs::create_dir_all(&agent_dir).unwrap();
        // The cut turn's tree, at line 20: it had f3.
        std::fs::write(dir.join("f3.txt"), "bad turn's file\n").unwrap();
        let old = snapshot_turn_record(&dir, &agent_dir, "root", 20)
            .unwrap()
            .unwrap();
        snapshot_turn_tree(&dir, &agent_dir, "root", &old).unwrap();
        let stale_sidecar = std::fs::read_to_string(tree_sidecar(&agent_dir, 20)).unwrap();
        let stale: Checkpoint = serde_json::from_str(&stale_sidecar).unwrap();
        assert!(stale.work.is_some());
        // The rewind removed f3 and the record — and (before the fix) left
        // the sidecar. Stage exactly that.
        std::fs::remove_file(dir.join("f3.txt")).unwrap();
        std::fs::write(agent_dir.join("checkpoints.jsonl"), "").unwrap();
        std::fs::write(dir.join("g3.txt"), "new turn's file\n").unwrap();
        // The new turn at the same line, its tree still pending.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let fresh = snapshot_turn_record(&dir, &agent_dir, "root", 20)
            .unwrap()
            .unwrap();
        assert_ne!(fresh.ts, stale.ts);
        assert_eq!(
            fresh.head, stale.head,
            "HEAD has not moved: the weak fact matches"
        );
        // A rewind arrives while the tree is pending: settle must not take
        // the stale sidecar for it.
        let settled = settle_tree(&agent_dir, &fresh, std::time::Duration::from_millis(300));
        assert_eq!(
            settled.work_error.as_deref(),
            Some(TREE_PENDING),
            "the cut turn's sidecar was taken for the new turn: {settled:?}"
        );
        assert!(settled.work.is_none());
        // When the new turn's own tree lands, it settles to that one: g3, not f3.
        snapshot_turn_tree(&dir, &agent_dir, "root", &fresh).unwrap();
        let settled = settle_tree(&agent_dir, &fresh, std::time::Duration::from_millis(300));
        let work = settled.work.expect("the new turn's tree");
        assert_ne!(Some(work.as_str()), stale.work.as_deref());
        let names = git_out(&dir, &["ls-tree", "--name-only", &work]).unwrap();
        assert!(
            names.contains("g3.txt") && !names.contains("f3.txt"),
            "{names}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
