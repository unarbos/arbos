use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use super::{GrepHit, ToolOut, hashline};
use crate::access::Access;
use crate::image;
use crate::tool::{
    BoxFuture, Plan, PlanCx, RunCx, Tool, blocking, opt_str, opt_u64, req, simple_schema,
    typed_schema,
};

// ---- Tool impls -----------------------------------------------------------

pub struct Ls;
pub struct Read;
pub struct Find;
pub struct GrepTool;
pub struct Write;
pub struct Edit;

impl Tool for Ls {
    fn name(&self) -> &'static str {
        "ls"
    }
    fn schema(&self) -> Value {
        simple_schema(
            "ls",
            "List a directory.",
            &[("path", "Directory path, relative to cwd.", false)],
        )
    }
    fn plan(&self, cx: &PlanCx, args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::read_path(
            &cx.resolve(opt_str(args, "path").unwrap_or("."))?,
        )))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || ls(cx.root(), &cx.cwd, opt_str(&args, "path").unwrap_or(".")))
    }
}

impl Tool for Read {
    fn name(&self) -> &'static str {
        "read"
    }
    fn schema(&self) -> Value {
        typed_schema(
            "read",
            "Read a file (text or image). Text lines are LINE:HASH|body. Pass LINE:HASH to edit.",
            &[
                ("path", "File path.", true, "string"),
                ("offset", "Start line (1-based).", false, "integer"),
                ("limit", "Max lines.", false, "integer"),
            ],
        )
    }
    fn plan(&self, cx: &PlanCx, args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::read_path(
            &cx.resolve(req(args, "path")?)?,
        )))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || {
            read(
                cx.root(),
                &cx.cwd,
                req(&args, "path")?,
                opt_u64(&args, "offset"),
                opt_u64(&args, "limit"),
            )
        })
    }
}

impl Tool for Find {
    fn name(&self) -> &'static str {
        "find"
    }
    fn schema(&self) -> Value {
        simple_schema(
            "find",
            "Find files by glob name.",
            &[("pattern", "Glob, e.g. **/*.rs", true)],
        )
    }
    fn plan(&self, cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::read_path(cx.cwd)))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || find(&cx.cwd, req(&args, "pattern")?))
    }
}

impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }
    fn schema(&self) -> Value {
        simple_schema(
            "grep",
            "Search file contents (tgrep).",
            &[
                ("pattern", "Regex or literal.", true),
                (
                    "path",
                    "Only this file or directory (relative to cwd). Default: whole place.",
                    false,
                ),
                ("glob", "Optional file glob.", false),
            ],
        )
    }
    fn plan(&self, cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::read_path(cx.cwd)))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || {
            let pattern = req(&args, "pattern")?;
            let glob = opt_str(&args, "glob");
            let mut hits = if cx.grep.ready() {
                cx.grep.search(pattern, glob)?
            } else {
                grep_walk(&cx.cwd, pattern, glob)?
            };
            // The index covers the whole place. A child in its own
            // worktree sees only its worktree, with paths relative to it:
            // the same file would otherwise show up twice, and the copy in
            // the parent's checkout is one it may not touch.
            if let Ok(prefix) = cx.root().strip_prefix(cx.place.path())
                && !prefix.as_os_str().is_empty()
            {
                let prefix = format!("{}/", prefix.to_string_lossy());
                let root_s = format!("{}/", cx.root().to_string_lossy());
                hits.retain_mut(|h| {
                    let p = h.path.trim_start_matches("./").to_string();
                    if let Some(rest) = p.strip_prefix(&prefix) {
                        h.path = rest.to_string();
                        true
                    } else if let Some(rest) = h.path.strip_prefix(&root_s) {
                        h.path = rest.to_string();
                        true
                    } else {
                        false
                    }
                });
            } else {
                // And the parent does not see its children's worktrees:
                // each is a copy of files it already has.
                let wt = format!(
                    "{}/",
                    cx.place
                        .worktrees_dir()
                        .strip_prefix(cx.place.path())
                        .unwrap_or(&cx.place.worktrees_dir())
                        .to_string_lossy()
                );
                let wt_abs = format!("{}/", cx.place.worktrees_dir().to_string_lossy());
                hits.retain(|h| {
                    let p = h.path.trim_start_matches("./");
                    !p.starts_with(&wt) && !h.path.starts_with(&wt_abs)
                });
            }
            // The kernel's own state is not the project. A transcript holds
            // every earlier tool result, so a grep that reaches into
            // `.arbos/agents/*/transcript.jsonl` finds its own past greps
            // and grows without bound. Asking for `.arbos/…` explicitly
            // still works (the CONTRACT says "grep it" for prior work).
            let scope_in_arbos = opt_str(&args, "path")
                .map(|p| p.trim_start_matches("./").starts_with(".arbos"))
                .unwrap_or(false);
            if !scope_in_arbos {
                hits.retain(|h| !inside_arbos(&h.path));
            }
            // Every other agent's grep takes a path; models send one. Hits
            // outside it are noise that sends the model to the wrong file.
            if let Some(scope) = opt_str(&args, "path").filter(|p| *p != ".") {
                let root = confine(cx.root(), &cx.cwd, scope)?;
                let root_s = root.to_string_lossy().into_owned();
                let rel = scope.trim_start_matches("./").trim_end_matches('/');
                hits.retain(|h| {
                    let p = h.path.trim_start_matches("./");
                    p == rel
                        || p.starts_with(&format!("{rel}/"))
                        || h.path == root_s
                        || h.path.starts_with(&format!("{root_s}/"))
                });
                if hits.is_empty() && !root.exists() {
                    anyhow::bail!("{} does not exist", root.display());
                }
            }
            Ok(format_hits(&hits))
        })
    }
}

const GREP_SHOWN: usize = 200;
/// Longest matched line shown. A JSONL line can be megabytes; the match
/// is what matters, and `read` has the rest.
const GREP_LINE_CHARS: usize = 400;

/// `.arbos/…` at any depth of a relative or absolute hit path.
fn inside_arbos(path: &str) -> bool {
    path.split('/').any(|seg| seg == ".arbos")
}

fn format_hits(hits: &[GrepHit]) -> ToolOut {
    let mut body = String::new();
    let mut paths = Vec::new();
    for h in hits.iter().take(GREP_SHOWN) {
        let text = if h.text.chars().count() > GREP_LINE_CHARS {
            let head: String = h.text.chars().take(GREP_LINE_CHARS).collect();
            format!("{head}… [line is {} chars]", h.text.chars().count())
        } else {
            h.text.clone()
        };
        body.push_str(&format!("{}:{}:{}\n", h.path, h.line, text));
        paths.push(h.path.clone());
    }
    if hits.len() > GREP_SHOWN {
        body.push_str(&format!("… {} more\n", hits.len() - GREP_SHOWN));
    }
    if body.is_empty() {
        body = "(no matches)\n".into();
    }
    ToolOut::with_paths(body, paths)
}

impl Tool for Write {
    fn name(&self) -> &'static str {
        "write"
    }
    fn schema(&self) -> Value {
        simple_schema(
            "write",
            "Create or overwrite a file.",
            &[
                ("path", "File path.", true),
                ("contents", "Full contents.", true),
            ],
        )
    }
    fn plan(&self, cx: &PlanCx, args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::write_path(
            &cx.resolve_write(req(args, "path")?)?,
        )))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || {
            let path = req(&args, "path")?;
            // `content` for `contents` is the usual slip.
            let contents = req(&args, "contents").or_else(|_| req(&args, "content"))?;
            note_inferred_path(write(cx.root(), &cx.cwd, path, contents), &args, path)
        })
    }
}

impl Tool for Edit {
    fn name(&self) -> &'static str {
        "edit"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "edit",
                "description": "Edit one file. Prefer hashline: path + anchor from read (LINE:HASH) + content. Empty content deletes. end_anchor replaces a range. op=insert_after uses anchor 0: or EOF. op=write replaces the whole file. edits is a list of {op,anchor,content}. Classic unique old_string/new_string still works.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "File path."},
                        "anchor": {"type": "string", "description": "LINE:HASH from read, or 0: / EOF."},
                        "end_anchor": {"type": "string", "description": "Inclusive end LINE:HASH for a range replace."},
                        "content": {"type": "string", "description": "Replacement or insert text. Empty deletes."},
                        "op": {"type": "string", "enum": ["replace", "insert_after", "write"], "description": "Default replace."},
                        "edits": {
                            "type": "array",
                            "description": "Several hashline ops on this file, applied bottom-up.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "op": {"type": "string", "enum": ["replace", "insert_after", "write"]},
                                    "anchor": {"type": "string"},
                                    "end_anchor": {"type": "string"},
                                    "content": {"type": "string"}
                                }
                            }
                        },
                        "old_string": {"type": "string", "description": "Unique text to find (classic)."},
                        "new_string": {"type": "string", "description": "Replacement (classic)."}
                    },
                    "required": ["path"]
                }
            }
        })
    }
    fn plan(&self, cx: &PlanCx, args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::write_path(
            &cx.resolve_write(req(args, "path")?)?,
        )))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || {
            let path = req(&args, "path")
                .map_err(|e| anyhow::anyhow!("{e}. edit always needs path — the file you read"))?;
            let root = cx.root();
            let out = if hashline::looks_like_hashline(&args) {
                hashline::edit(root, &cx.cwd, path, &args)
            } else if let (None, Some(content)) =
                (opt_str(&args, "old_string"), opt_str(&args, "content"))
            {
                // path + content and nothing to anchor on: the model means
                // the whole file ("here is the corrected implementation").
                write(root, &cx.cwd, path, content)
            } else {
                edit(
                    root,
                    &cx.cwd,
                    path,
                    req(&args, "old_string")?,
                    req(&args, "new_string")?,
                )
            };
            note_inferred_path(out, &args, path)
        })
    }
}

/// Prefix the result when `path` was not in the call but guessed from the
/// last file touched, so a wrong guess is caught at once.
fn note_inferred_path(out: Result<ToolOut>, args: &Value, path: &str) -> Result<ToolOut> {
    if args.get(super::INFERRED_PATH).is_none() {
        return out;
    }
    let note = format!("(no path in the call; used {path}, the last file you touched)");
    match out {
        Ok(mut o) => {
            o.body = format!("{note}\n{}", o.body);
            Ok(o)
        }
        Err(e) => Err(anyhow::anyhow!("{note} {e}")),
    }
}

// ---- Bodies ---------------------------------------------------------------

pub fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() { p } else { cwd.join(p) }
}

/// Resolve `path` against `cwd` and refuse it unless it stays inside `root`
/// (the place). `..` is folded lexically and the deepest existing ancestor is
/// canonicalised, so neither `../x`, an absolute path, nor a symlink out of
/// the place gets through. The returned path is the lexical one, so error
/// messages and cites keep the spelling the model used.
pub fn confine(root: &Path, cwd: &Path, path: &str) -> Result<PathBuf> {
    let candidate = normalize(&resolve(cwd, path));
    let root_real = realize(&normalize(root));
    let real = realize(&candidate);
    if !real.starts_with(&root_real) {
        bail!(
            "{} is outside the workspace {}; file tools only reach files under it",
            path,
            root.display()
        );
    }
    Ok(candidate)
}

/// Fold `.` and `..` without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Canonicalise the longest existing prefix of `path` (so symlinks resolve)
/// and re-append whatever does not exist yet.
fn realize(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut out = std::fs::canonicalize(&existing).unwrap_or(existing);
    for part in rest.iter().rev() {
        out.push(part);
    }
    out
}

pub fn ls(root: &Path, cwd: &Path, path: &str) -> Result<ToolOut> {
    let dir = confine(root, cwd, path)?;
    let mut names = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .with_context(|| format!("ls {}", dir.display()))?
        .flatten()
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let mut name = e.file_name().to_string_lossy().into_owned();
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            name.push('/');
        }
        names.push(name);
    }
    Ok(ToolOut::with_paths(
        names.join("\n"),
        vec![dir.display().to_string()],
    ))
}

pub fn read(
    root: &Path,
    cwd: &Path,
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
) -> Result<ToolOut> {
    let file = confine(root, cwd, path)?;
    if image::is_image_path(&file) {
        return read_image(&file);
    }
    if crate::pdf::is_pdf_path(&file) && file.is_file() {
        return read_pdf(&file, offset, limit);
    }
    // Models `read` a folder to see what is in it. Answer the question.
    if file.is_dir() {
        return ls(root, cwd, path);
    }
    if !file.exists() {
        // A guessed name (`test_foo.py` for `foo_test.py`) is the common
        // case; name what is actually there so the next call is right.
        let dir = file.parent().filter(|d| d.is_dir()).unwrap_or(cwd);
        let want = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut near: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.file_name().to_str().map(str::to_string))
                    .filter(|n| !n.starts_with('.'))
                    .filter(|n| similar_name(&n.to_ascii_lowercase(), &want))
                    .collect()
            })
            .unwrap_or_default();
        near.sort();
        let hint = if near.is_empty() {
            format!(" (ls {} to see what is there)", dir.display())
        } else {
            format!(
                "; did you mean {}?",
                near.iter()
                    .take(4)
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        bail!("{} does not exist{hint}", file.display());
    }
    let text =
        std::fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    let start = offset.unwrap_or(1).saturating_sub(1) as usize;
    let take = limit.unwrap_or(lines.len() as u64) as usize;
    let slice = lines.iter().skip(start).take(take);
    let mut body = String::new();
    for (i, line) in slice.enumerate() {
        let n = start + i + 1;
        body.push_str(&format!(
            "{n:>6}:{h}|{line}\n",
            h = super::hashline::line_tag(line)
        ));
    }
    Ok(ToolOut::with_paths(body, vec![file.display().to_string()]))
}

/// Same stem in a different arrangement (`test_x.py` vs `x_test.py`), or
/// one contains the other.
fn similar_name(have: &str, want: &str) -> bool {
    if want.is_empty() {
        return false;
    }
    let stem = |s: &str| {
        s.rsplit_once('.')
            .map(|(a, _)| a.to_string())
            .unwrap_or_else(|| s.to_string())
    };
    let (hs, ws) = (stem(have), stem(want));
    fn parts(s: &str) -> Vec<String> {
        let mut v: Vec<String> = s
            .split(['_', '-', '.'])
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        v.sort_unstable();
        v
    }
    hs.contains(&ws) || ws.contains(&hs) || parts(&hs) == parts(&ws)
}

/// A PDF reads as its text with page markers, numbered lines like any
/// file (no LINE:HASH: there is nothing to edit). `offset`/`limit` page
/// through a long one.
fn read_pdf(file: &Path, offset: Option<u64>, limit: Option<u64>) -> Result<ToolOut> {
    let (text, pages) = crate::pdf::text(file)?;
    let lines: Vec<&str> = text.lines().collect();
    let start = offset.unwrap_or(1).saturating_sub(1) as usize;
    let take = limit.unwrap_or(lines.len() as u64) as usize;
    let mut body = format!(
        "pdf {} — {pages} page(s), {} lines of text{}\n",
        file.display(),
        lines.len(),
        if start > 0 || take < lines.len() {
            format!(" (lines {}–{})", start + 1, (start + take).min(lines.len()))
        } else {
            String::new()
        }
    );
    for (i, line) in lines.iter().skip(start).take(take).enumerate() {
        body.push_str(&format!("{:>6}|{line}\n", start + i + 1));
    }
    Ok(ToolOut::with_paths(body, vec![file.display().to_string()]))
}

/// The body is one caption line; the pixels go out as an image part when
/// the projection runs. A file that is not really an image (wrong
/// extension, too big) is reported as text so the model can react.
fn read_image(file: &Path) -> Result<ToolOut> {
    let meta = std::fs::metadata(file).with_context(|| format!("read {}", file.display()))?;
    if meta.len() > image::MAX_IMAGE_BYTES {
        bail!(
            "{} is {} MB; images over {} MB cannot be shown. Downscale it with bash (sips/convert) first.",
            file.display(),
            meta.len() / (1024 * 1024),
            image::MAX_IMAGE_BYTES / (1024 * 1024)
        );
    }
    let mut head = [0u8; 32];
    let n = {
        use std::io::Read;
        let mut f = std::fs::File::open(file)?;
        f.read(&mut head)?
    };
    let Some(mime) = image::sniff_mime(&head[..n]) else {
        bail!(
            "{} has an image extension but is not a png/jpeg/gif/webp",
            file.display()
        );
    };
    let dims = image::dimensions(&head[..n])
        .map(|(w, h)| format!(", {w}x{h}"))
        .unwrap_or_default();
    let shown = file.display().to_string();
    Ok(ToolOut {
        body: format!(
            "image {shown} ({mime}{dims}, {} KB) — attached below",
            meta.len().div_ceil(1024)
        ),
        paths: vec![shown.clone()],
        child: None,
        images: vec![shown],
        diff: None,
        park: None,
    })
}

pub fn find(cwd: &Path, pattern: &str) -> Result<ToolOut> {
    let glob = glob::Pattern::new(pattern).unwrap_or_else(|_| glob::Pattern::new("**/*").unwrap());
    // The kernel's own folder is not the project (see grep). A pattern that
    // names `.arbos` still reaches it.
    let want_arbos = pattern.contains(".arbos");
    let mut paths = Vec::new();
    let walker = ignore::WalkBuilder::new(cwd)
        .hidden(false)
        .git_ignore(true)
        .filter_entry(move |e| want_arbos || e.file_name() != ".arbos")
        .build();
    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let rel = path.strip_prefix(cwd).unwrap_or(path);
        let s = rel.to_string_lossy();
        if glob.matches(&s)
            || rel
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| glob.matches(n))
        {
            paths.push(s.into_owned());
        }
    }
    paths.sort();
    let body = if paths.is_empty() {
        "(no files)\n".into()
    } else {
        paths.join("\n")
    };
    Ok(ToolOut::with_paths(body, paths))
}

/// The error for a write that changes nothing. A model that keeps
/// re-sending the same file after every failed test run needs to hear that
/// the file is not what is wrong.
pub fn unchanged(file: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "no change: {} already has exactly this content. Re-writing it will not change the result; look elsewhere.",
        file.display()
    )
}

pub fn write(root: &Path, cwd: &Path, path: &str, contents: &str) -> Result<ToolOut> {
    let file = confine(root, cwd, path)?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::read_to_string(&file).is_ok_and(|old| old == contents) {
        return Err(unchanged(&file));
    }
    std::fs::write(&file, contents)?;
    let mut body = format!("wrote {}", file.display());
    if let Some(note) = syntax_note(&file) {
        body.push('\n');
        body.push_str(&note);
    }
    Ok(ToolOut::with_paths(body, vec![file.display().to_string()]))
}

pub fn edit(root: &Path, cwd: &Path, path: &str, old: &str, new: &str) -> Result<ToolOut> {
    let file = confine(root, cwd, path)?;
    let text = std::fs::read_to_string(&file)?;
    let count = text.matches(old).count();
    if count == 0 {
        // The first line of what the model thinks is there, wherever it is
        // now, with anchors — so the retry can use LINE:HASH instead.
        let probe = old
            .lines()
            .map(str::trim)
            .find(|l| l.len() > 3)
            .unwrap_or("");
        let lines: Vec<&str> = text.lines().collect();
        let hits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| !probe.is_empty() && l.contains(probe))
            .map(|(i, _)| i)
            .take(3)
            .collect();
        let mut msg = format!("old_string not found in {}", file.display());
        if hits.is_empty() {
            msg.push_str(". Nothing in the file contains its first line; read the file and use LINE:HASH anchors.");
        } else {
            msg.push_str(". Its first line appears near:");
            for i in hits {
                for n in i.saturating_sub(2)..(i + 3).min(lines.len()) {
                    msg.push_str(&format!(
                        "\n{:>6}:{}|{}",
                        n + 1,
                        hashline::line_tag(lines[n]),
                        lines[n]
                    ));
                }
                msg.push('\n');
            }
            msg.push_str("The rest of old_string differs from the file (whitespace or a changed line). Use an anchor from above.");
        }
        bail!("{msg}");
    }
    if count > 1 {
        bail!("old_string matches {count} times; make it unique");
    }
    let next = text.replacen(old, new, 1);
    if next == text {
        return Err(unchanged(&file));
    }
    std::fs::write(&file, &next)?;
    let mut body = format!("edited {}", file.display());
    if let Some(note) = syntax_note(&file) {
        body.push('\n');
        body.push_str(&note);
    }
    Ok(ToolOut::with_paths(body, vec![file.display().to_string()])
        .with_diff(super::editdiff::numbered_diff(&text, &next)))
}

/// A one-line parse check after a write, for languages with a cheap
/// checker on the machine. A model that broke the file learns it from the
/// tool result instead of from a test run three steps later. Best effort:
/// no checker, or a slow one, means no note.
pub fn syntax_note(file: &Path) -> Option<String> {
    let ext = file.extension()?.to_str()?;
    let f = file.to_str()?;
    let (cmd, args): (&str, Vec<&str>) = match ext {
        "py" => (
            "python3",
            vec![
                "-c",
                "import ast,sys; ast.parse(open(sys.argv[1]).read(), sys.argv[1])",
                f,
            ],
        ),
        "rs" => (
            "rustfmt",
            vec!["--check", "--edition", "2021", "--color", "never", f],
        ),
        "js" | "mjs" | "cjs" => ("node", vec!["--check", f]),
        "json" => {
            let text = std::fs::read_to_string(file).ok()?;
            return serde_json::from_str::<serde_json::Value>(&text)
                .err()
                .map(|e| format!("syntax check: invalid JSON — {e}"));
        }
        _ => return None,
    };
    let out = std::process::Command::new(cmd)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if out.status.success() {
        return None;
    }
    let err = String::from_utf8_lossy(&out.stderr);
    // rustfmt --check exits 1 for formatting differences too; only a parse
    // error is worth the model's attention.
    if ext == "rs" && !err.contains("error") {
        return None;
    }
    let all: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
    let lines: Vec<&str> = match ext {
        // Python prints the checker's own traceback first; the file, the
        // caret and the SyntaxError line are the last three or four.
        "py" => all[all.len().saturating_sub(4)..].to_vec(),
        "rs" => all
            .iter()
            .copied()
            .filter(|l| l.contains("error") || l.contains("-->"))
            .take(6)
            .collect(),
        _ => all.iter().copied().take(6).collect(),
    };
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "syntax check FAILED — fix before running tests:\n{}",
        lines.join("\n")
    ))
}

pub fn grep_walk(cwd: &Path, pattern: &str, glob: Option<&str>) -> Result<Vec<GrepHit>> {
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(false)
        .build()
        .or_else(|_| regex::Regex::new(&regex::escape(pattern)))?;
    let file_glob = glob.and_then(|g| glob::Pattern::new(g).ok());
    let mut hits = Vec::new();
    let walker = ignore::WalkBuilder::new(cwd)
        .hidden(false)
        .git_ignore(true)
        .build();
    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let rel = path.strip_prefix(cwd).unwrap_or(path);
        if let Some(g) = &file_glob {
            if !g.matches(&rel.to_string_lossy()) {
                continue;
            }
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if re.is_match(line) {
                hits.push(GrepHit {
                    path: rel.to_string_lossy().into_owned(),
                    line: i + 1,
                    text: line.to_string(),
                });
                if hits.len() >= 500 {
                    return Ok(hits);
                }
            }
        }
    }
    Ok(hits)
}
