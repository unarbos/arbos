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
use arbos_core::hub::StoreAddress;

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
            "List a directory. A store address (arbos://<machine>/<project>/<path>, see .arbos/machines.md) lists a folder in another node's .arbos/.",
            &[(
                "path",
                "Directory path, relative to cwd, or a store address.",
                false,
            )],
        )
    }
    fn plan(&self, cx: &PlanCx, args: &Value) -> Result<Plan> {
        let path = opt_str(args, "path").unwrap_or(".");
        if StoreAddress::looks_like(path) {
            return Ok(Plan::access(Access::read_store(
                StoreAddress::parse(path)?.to_string(),
            )));
        }
        Ok(Plan::access(Access::read_path(&cx.resolve(path)?)))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        let path = opt_str(&args, "path").unwrap_or(".").to_string();
        if StoreAddress::looks_like(&path) {
            return Box::pin(async move { remote_ls(&cx, &path).await });
        }
        blocking(move || ls(cx.root(), &cx.cwd, &path))
    }
}

impl Tool for Read {
    fn name(&self) -> &'static str {
        "read"
    }
    fn schema(&self) -> Value {
        typed_schema(
            "read",
            "Read a file (text or image). Text lines are LINE:HASH|body. Pass LINE:HASH to edit. A store address (arbos://<machine>/<project>/<path>, see .arbos/machines.md) reads a file in another node's .arbos/ — the parent project's notes.md or docs/ from a remote worker.",
            &[
                ("path", "A path, or a store address.", true, "string"),
                ("offset", "Start line (1-based).", false, "integer"),
                ("limit", "Max lines.", false, "integer"),
            ],
        )
    }
    fn plan(&self, cx: &PlanCx, args: &Value) -> Result<Plan> {
        let path = req(args, "path")?;
        if StoreAddress::looks_like(path) {
            return Ok(Plan::access(Access::read_store(
                StoreAddress::parse(path)?.to_string(),
            )));
        }
        Ok(Plan::access(Access::read_path(&cx.resolve(path)?)))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        if let Some(addr) = opt_str(&args, "path").filter(|p| StoreAddress::looks_like(p)) {
            let addr = addr.to_string();
            let (offset, limit) = (opt_u64(&args, "offset"), opt_u64(&args, "limit"));
            return Box::pin(async move { remote_read(&cx, &addr, offset, limit).await });
        }
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
            &[
                ("pattern", "Glob, e.g. **/*.rs", true),
                (
                    "path",
                    "Search under this directory (relative to cwd). Default: cwd.",
                    false,
                ),
                (
                    "sort",
                    "name (default) or mtime (most recently changed first, Cursor's Glob order).",
                    false,
                ),
            ],
        )
    }
    fn plan(&self, cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::read_path(cx.cwd)))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || {
            let root = match opt_str(&args, "path").filter(|p| !p.trim().is_empty() && *p != ".") {
                Some(p) => confine(cx.root(), &cx.cwd, p)?,
                None => cx.cwd.clone(),
            };
            let by_mtime = opt_str(&args, "sort").is_some_and(|s| {
                matches!(
                    s.trim().to_ascii_lowercase().as_str(),
                    "mtime" | "modified" | "newest" | "time"
                )
            });
            find_sorted(&root, req(&args, "pattern")?, by_mtime)
        })
    }
}

impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }
    fn schema(&self) -> Value {
        simple_schema(
            "grep",
            "Search file contents (tgrep). scope=history searches every agent's transcript (what earlier workers did and found).",
            &[
                ("pattern", "Regex or literal.", true),
                (
                    "path",
                    "Only this file or directory (relative to cwd). Default: whole place.",
                    false,
                ),
                ("glob", "Optional file glob.", false),
                ("scope", "history: earlier agents' transcripts", false),
                ("ignore_case", "true: case-insensitive.", false),
                ("context", "N lines before and after each hit.", false),
                (
                    "mode",
                    "content (default), files (paths with a hit), or count (hits per file).",
                    false,
                ),
                ("limit", "Most hits shown (default 200).", false),
            ],
        )
    }
    fn plan(&self, cx: &PlanCx, _args: &Value) -> Result<Plan> {
        Ok(Plan::access(Access::read_path(cx.cwd)))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || {
            // `scope: history` is sugar for `path: .arbos/agents`: what every
            // earlier worker did, said, and found, cited by agent and line.
            let history =
                opt_str(&args, "scope").is_some_and(|s| s.trim().eq_ignore_ascii_case("history"));
            if history {
                // Transcripts change every turn and .arbos/ is usually
                // gitignored, so this is a fresh walk, not the index.
                // Finished workers move to the archive; their history
                // counts the same.
                let pattern = req(&args, "pattern")?;
                let arbos = cx.place.path().join(".arbos");
                let mut hits = history_walk(&arbos.join("agents"), pattern)?;
                hits.extend(history_walk(
                    &arbos.join("archive").join("agents"),
                    pattern,
                )?);
                return Ok(format_history_hits(&hits));
            }
            let pattern = req(&args, "pattern")?;
            let glob = opt_str(&args, "glob");
            let ignore_case = args
                .get("ignore_case")
                .is_some_and(|v| v.as_bool() == Some(true) || v.as_str() == Some("true"));
            // The index is case-sensitive; a case-insensitive search walks.
            let mut hits = if cx.grep.ready() && !ignore_case {
                cx.grep.search(pattern, glob)?
            } else {
                grep_walk_with(&cx.cwd, pattern, glob, ignore_case)?
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
            let limit = args
                .get("limit")
                .and_then(|v| {
                    v.as_u64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .map(|n| (n as usize).clamp(1, 2000))
                .unwrap_or(GREP_SHOWN);
            let context = args
                .get("context")
                .and_then(|v| {
                    v.as_u64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .map(|n| (n as usize).min(20))
                .unwrap_or(0);
            let mode = opt_str(&args, "mode")
                .unwrap_or("content")
                .trim()
                .to_ascii_lowercase();
            match mode.as_str() {
                "content" | "" => Ok(format_hits_with(&hits, limit, context, cx.root())),
                "files" | "files_with_matches" | "paths" => {
                    let mut paths: Vec<String> = Vec::new();
                    for h in &hits {
                        if !paths.contains(&h.path) {
                            paths.push(h.path.clone());
                        }
                    }
                    let shown: Vec<String> = paths.iter().take(limit).cloned().collect();
                    let mut body = shown.join("\n");
                    if paths.len() > shown.len() {
                        body.push_str(&format!("\n… {} more", paths.len() - shown.len()));
                    }
                    if body.is_empty() {
                        body = "(no matches)".into();
                    }
                    body.push('\n');
                    Ok(ToolOut::with_paths(body, shown))
                }
                "count" => {
                    let mut counts: Vec<(String, usize)> = Vec::new();
                    for h in &hits {
                        match counts.iter_mut().find(|(p, _)| *p == h.path) {
                            Some((_, n)) => *n += 1,
                            None => counts.push((h.path.clone(), 1)),
                        }
                    }
                    let body = if counts.is_empty() {
                        "(no matches)\n".to_string()
                    } else {
                        counts
                            .iter()
                            .take(limit)
                            .map(|(p, n)| format!("{p}:{n}\n"))
                            .collect()
                    };
                    Ok(ToolOut::with_paths(
                        body,
                        counts.iter().take(limit).map(|(p, _)| p.clone()).collect(),
                    ))
                }
                other => {
                    anyhow::bail!("grep: mode must be content, files, or count, not {other:?}")
                }
            }
        })
    }
}

/// `format_hits` with a cap and, when asked, `context` lines around each
/// hit read back from the file (`-` lines, the hit's own with `:`).
fn format_hits_with(hits: &[GrepHit], limit: usize, context: usize, root: &Path) -> ToolOut {
    if context == 0 && limit == GREP_SHOWN {
        return format_hits(hits);
    }
    let mut body = String::new();
    let mut paths = Vec::new();
    for h in hits.iter().take(limit) {
        if context > 0 {
            let file = if Path::new(&h.path).is_absolute() {
                PathBuf::from(&h.path)
            } else {
                root.join(&h.path)
            };
            if let Ok(text) = std::fs::read_to_string(&file) {
                let lines: Vec<&str> = text.lines().collect();
                let lo = h.line.saturating_sub(context + 1);
                let hi = (h.line + context).min(lines.len());
                for (i, l) in lines.iter().enumerate().take(hi).skip(lo) {
                    let n = i + 1;
                    let sep = if n == h.line { ':' } else { '-' };
                    body.push_str(&format!("{}{sep}{n}{sep}{l}\n", h.path));
                }
                body.push_str("--\n");
                paths.push(h.path.clone());
                continue;
            }
        }
        let text = if h.text.chars().count() > GREP_LINE_CHARS {
            let head: String = h.text.chars().take(GREP_LINE_CHARS).collect();
            format!("{head}… [line is {} chars]", h.text.chars().count())
        } else {
            h.text.clone()
        };
        body.push_str(&format!("{}:{}:{}\n", h.path, h.line, text));
        paths.push(h.path.clone());
    }
    if hits.len() > limit {
        body.push_str(&format!("… {} more\n", hits.len() - limit));
    }
    if body.is_empty() {
        body = "(no matches)\n".into();
    }
    ToolOut::with_paths(body, paths)
}

/// History hits read `agent · line N: text`, with the JSON of a transcript
/// line reduced to its kind and text so the model sees what was said, not
/// the record's shape.
fn history_walk(agents_dir: &Path, pattern: &str) -> Result<Vec<GrepHit>> {
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .or_else(|_| regex::Regex::new(&regex::escape(pattern)))?;
    let mut hits = Vec::new();
    let mut agents: Vec<_> = std::fs::read_dir(agents_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    agents.sort();
    for dir in agents {
        let Ok(text) = std::fs::read_to_string(dir.join("transcript.jsonl")) else {
            continue;
        };
        let name = dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        for (i, line) in text.lines().enumerate() {
            if re.is_match(line) {
                hits.push(GrepHit {
                    path: format!("{name}/transcript.jsonl"),
                    line: i + 1,
                    text: line.to_string(),
                });
            }
        }
    }
    Ok(hits)
}

fn format_history_hits(hits: &[GrepHit]) -> ToolOut {
    if hits.is_empty() {
        return ToolOut::text("(no matches in any agent's history)");
    }
    let mut out = String::new();
    for h in hits.iter().take(GREP_SHOWN) {
        let agent = h.path.split('/').next().unwrap_or("?");
        let shown = match serde_json::from_str::<Value>(&h.text) {
            Ok(v) => {
                let kind = v["kind"].as_str().unwrap_or("?");
                let text = v["text"]
                    .as_str()
                    .or_else(|| v["body"].as_str())
                    .or_else(|| v["name"].as_str())
                    .unwrap_or("");
                format!(
                    "{kind}: {}",
                    arbos_core::text::clip(text.trim(), GREP_LINE_CHARS)
                )
            }
            Err(_) => arbos_core::text::clip(h.text.trim(), GREP_LINE_CHARS),
        };
        out.push_str(&format!("{agent} · line {}: {shown}\n", h.line));
    }
    if hits.len() > GREP_SHOWN {
        out.push_str(&format!("({} more not shown)\n", hits.len() - GREP_SHOWN));
    }
    ToolOut::text(out.trim_end())
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

pub struct Delete;

impl Tool for Delete {
    fn name(&self) -> &'static str {
        "delete"
    }
    fn schema(&self) -> Value {
        simple_schema(
            "delete",
            "Delete one file (not a directory) under the place.",
            &[("path", "The file.", true)],
        )
    }
    fn plan(&self, cx: &PlanCx, args: &Value) -> Result<Plan> {
        let file = cx.resolve_write(req(args, "path")?)?;
        Ok(Plan::access(Access::write_path(&file)))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || {
            let path = req(&args, "path")?;
            let file = confine(cx.root(), &cx.cwd, path)?;
            if file.is_dir() {
                bail!("{} is a directory; delete names one file", file.display());
            }
            if !file.exists() {
                bail!("{} does not exist", file.display());
            }
            std::fs::remove_file(&file)?;
            Ok(ToolOut::with_paths(
                format!("deleted {}", file.display()),
                vec![file.display().to_string()],
            ))
        })
    }
}

impl Tool for Write {
    fn name(&self) -> &'static str {
        "write"
    }
    fn schema(&self) -> Value {
        let mut schema = simple_schema(
            "write",
            "Create or overwrite a file. A store address (arbos://<machine>/<project>/docs/…) writes into another node's .arbos/ — how a remote worker delivers into its parent's project store; that node applies its own rules (its notes.md stays its root's).",
            &[
                ("path", "A path, or a store address.", true),
                ("contents", "", true),
            ],
        );
        if let Some(props) = schema
            .pointer_mut("/function/parameters/properties")
            .and_then(Value::as_object_mut)
        {
            props.insert("mechanism".into(), crate::mechanism::schema_property());
        }
        schema
    }
    fn plan(&self, cx: &PlanCx, args: &Value) -> Result<Plan> {
        let path = req(args, "path")?;
        if StoreAddress::looks_like(path) {
            return Ok(Plan::access(Access::write_store(
                StoreAddress::parse(path)?.to_string(),
            )));
        }
        Ok(Plan::access(Access::write_path(&cx.resolve_write(path)?)))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        if let Some(addr) = opt_str(&args, "path").filter(|p| StoreAddress::looks_like(p)) {
            let addr = addr.to_string();
            return Box::pin(async move {
                let contents = req(&args, "contents").or_else(|_| req(&args, "content"))?;
                remote_write(&cx, &addr, contents, None).await
            });
        }
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
                "description": "Edit one file by hashline: anchor from read (LINE:HASH) + content (empty deletes; end_anchor for a range; op insert_after with anchor 0: or EOF; op write for the whole file). Classic unique old_string/new_string also works.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "anchor": {"type": "string", "description": "LINE:HASH from read, or 0: / EOF."},
                        "end_anchor": {"type": "string", "description": "Inclusive range end."},
                        "content": {"type": "string", "description": "Empty deletes."},
                        "op": {"type": "string", "enum": ["replace", "insert_after", "write"], "description": "Default replace."},
                        "edits": {"type": "array", "items": {"type": "object"}, "description": "Several {op, anchor, end_anchor, content} on this file, applied bottom-up."},
                        "old_string": {"type": "string", "description": "Unique text to find (classic)."},
                        "replace_all": {"type": "boolean", "description": "classic: replace every occurrence of old_string, not one unique one."},
                        "new_string": {"type": "string", "description": "Replacement (classic)."},
                        "mechanism": crate::mechanism::schema_property()
                    },
                    "required": ["path"]
                }
            }
        })
    }
    fn plan(&self, cx: &PlanCx, args: &Value) -> Result<Plan> {
        let path = req(args, "path")?;
        if StoreAddress::looks_like(path) {
            return Ok(Plan::access(Access::write_store(
                StoreAddress::parse(path)?.to_string(),
            )));
        }
        Ok(Plan::access(Access::write_path(&cx.resolve_write(path)?)))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        if let Some(addr) = opt_str(&args, "path").filter(|p| StoreAddress::looks_like(p)) {
            let addr = addr.to_string();
            return Box::pin(async move { remote_edit(&cx, &addr, &args).await });
        }
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
            } else if args
                .get("replace_all")
                .is_some_and(|v| v.as_bool() == Some(true) || v.as_str() == Some("true"))
            {
                edit_all(
                    root,
                    &cx.cwd,
                    path,
                    req(&args, "old_string")?,
                    req(&args, "new_string")?,
                )
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

// ---- Another node's store, by address --------------------------------------

/// `ls` of a folder in another node's store. Failure is the host's error,
/// with the address in it; never an empty listing for a peer that is gone.
async fn remote_ls(cx: &RunCx, address: &str) -> Result<ToolOut> {
    let addr = StoreAddress::parse(address)?;
    let entries = cx.hooks.store_list(&addr.to_string()).await?;
    let names: Vec<String> = entries
        .iter()
        .map(|e| {
            if e.dir {
                format!("{}/", e.name)
            } else {
                e.name.clone()
            }
        })
        .collect();
    Ok(ToolOut::with_paths(
        names.join("\n"),
        vec![addr.to_string()],
    ))
}

/// `read` of a file in another node's store, rendered like a local read
/// (LINE:HASH|body) so `edit` by address takes the same anchors.
async fn remote_read(
    cx: &RunCx,
    address: &str,
    offset: Option<u64>,
    limit: Option<u64>,
) -> Result<ToolOut> {
    let addr = StoreAddress::parse(address)?;
    if image::is_image_path(Path::new(&addr.path)) {
        bail!(
            "{addr}: an image in another node's store is not read by address yet; ask its agent for the file, or read a text file"
        );
    }
    let file = cx.hooks.store_read(&addr.to_string()).await?;
    let lines: Vec<&str> = file.text.lines().collect();
    let start = offset.unwrap_or(1).saturating_sub(1) as usize;
    let take = limit.unwrap_or(lines.len() as u64) as usize;
    let mut body = String::new();
    for (i, line) in lines.iter().skip(start).take(take).enumerate() {
        let n = start + i + 1;
        body.push_str(&format!(
            "{n:>6}:{h}|{line}\n",
            h = super::hashline::line_tag(line)
        ));
    }
    if file.truncated {
        body.push_str(&format!(
            "[{addr} is {} bytes; only the first {} were read]\n",
            file.size,
            file.text.len()
        ));
    }
    Ok(ToolOut::with_paths(body, vec![addr.to_string()]))
}

/// `write` into another node's store. `base_hash` carries the hash the
/// caller read, for an edit; a plain write passes none.
async fn remote_write(
    cx: &RunCx,
    address: &str,
    contents: &str,
    base_hash: Option<String>,
) -> Result<ToolOut> {
    let addr = StoreAddress::parse(address)?;
    let written = cx
        .hooks
        .store_write(&addr.to_string(), contents.to_string(), base_hash)
        .await?;
    Ok(ToolOut::with_paths(
        format!(
            "wrote {addr} ({} bytes) on {}; that node's own rules applied",
            written.size, addr.machine
        ),
        vec![addr.to_string()],
    ))
}

/// `edit` of a file in another node's store: fetch it, apply the same
/// edit the local tool would (hashline anchors, classic old/new, whole
/// file) to a copy, and put the result back with the hash that was read,
/// so a change on the far side in between is a conflict, not a lost update.
async fn remote_edit(cx: &RunCx, address: &str, args: &Value) -> Result<ToolOut> {
    let addr = StoreAddress::parse(address)?;
    let whole = (opt_str(args, "old_string"), opt_str(args, "content"));
    if let (None, Some(content)) = whole
        && !hashline::looks_like_hashline(args)
    {
        return remote_write(cx, address, content, None).await;
    }
    let file = cx.hooks.store_read(&addr.to_string()).await?;
    if file.truncated {
        bail!(
            "{addr} is {} bytes, over what one read carries; edit it on its own node",
            file.size
        );
    }
    // A scratch copy the local edit code works on. Its folder is the
    // confinement root, so the edit sees one file and nothing else.
    let scratch = std::env::temp_dir().join(format!(
        "arbos-remote-edit-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    std::fs::create_dir_all(&scratch)?;
    let name = Path::new(&addr.path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    std::fs::write(scratch.join(&name), &file.text)?;
    let args_local = args.clone();
    let (scratch2, name2) = (scratch.clone(), name.clone());
    let out = tokio::task::spawn_blocking(move || -> Result<String> {
        let root = scratch2.as_path();
        let result = if hashline::looks_like_hashline(&args_local) {
            hashline::edit(root, root, &name2, &args_local)
        } else if args_local
            .get("replace_all")
            .is_some_and(|v| v.as_bool() == Some(true) || v.as_str() == Some("true"))
        {
            edit_all(
                root,
                root,
                &name2,
                req(&args_local, "old_string")?,
                req(&args_local, "new_string")?,
            )
        } else {
            edit(
                root,
                root,
                &name2,
                req(&args_local, "old_string")?,
                req(&args_local, "new_string")?,
            )
        };
        result?;
        Ok(std::fs::read_to_string(root.join(&name2))?)
    })
    .await
    .map_err(|e| anyhow::anyhow!("edit task: {e}"))?;
    let _ = std::fs::remove_dir_all(&scratch);
    let new_text = out?;
    let written = cx
        .hooks
        .store_write(&addr.to_string(), new_text, Some(file.hash))
        .await?;
    Ok(ToolOut::with_paths(
        format!("edited {addr} ({} bytes) on {}", written.size, addr.machine),
        vec![addr.to_string()],
    ))
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
    let path = store_alias(root, cwd, path);
    let candidate = normalize(&resolve(cwd, &path));
    let root_real = realize(&normalize(root));
    let real = realize(&candidate);
    if !real.starts_with(&root_real) && !in_shared_store(root, &real) {
        bail!(
            "{} is outside the workspace {}; file tools reach files under it and the project store .arbos/",
            path,
            root.display()
        );
    }
    Ok(candidate)
}

/// The place's `.arbos/` for a confinement root: the root's own when the
/// root is the place, the place's when the root is a worktree under
/// `<place>/.arbos/worktrees/<id>` (a worktree has no `.arbos` of its own;
/// the store is excluded from git).
pub fn store_dir(root: &Path) -> PathBuf {
    if let Some(store) = worktree_store(root) {
        return store;
    }
    root.join(".arbos")
}

/// `<place>/.arbos` when `root` is `<place>/.arbos/worktrees/<id>`.
fn worktree_store(root: &Path) -> Option<PathBuf> {
    let worktrees = root.parent()?;
    let store = worktrees.parent()?;
    (worktrees.file_name()? == "worktrees" && store.file_name()? == ".arbos")
        .then(|| store.to_path_buf())
}

/// A worktree child reaches the project store (`docs/`, `notes.md`,
/// `internal/`, `media/`, its own `agents/<id>/`) like any worker of the
/// project — the brief points there and the output rules name it (qa-035).
/// Not the other workers' worktrees under `.arbos/worktrees/`: those are
/// checkouts it may not touch. `real` is the canonicalised candidate.
fn in_shared_store(root: &Path, real: &Path) -> bool {
    let Some(store) = worktree_store(root) else {
        return false;
    };
    let store_real = realize(&normalize(&store));
    real.starts_with(&store_real) && !real.starts_with(store_real.join("worktrees"))
}

/// The project store is spoken of as `docs/`, `internal/`, `media/`,
/// `archived.md`, and `project-context.md` (the contract, the briefs, the
/// refusal texts all say so), while it lives under `.arbos/`. A relative
/// path that starts with one of those names, when nothing by that name
/// exists where the agent stands but the store has it, means the store.
/// So `write docs/research.md` lands in `.arbos/docs/`, and a coordinator's
/// `edit docs/project-context.md` is the file the contract named, not a
/// refused write outside the store.
fn store_alias(root: &Path, cwd: &Path, path: &str) -> String {
    let trimmed = path.trim().trim_start_matches("./");
    if trimmed.is_empty() || Path::new(trimmed).is_absolute() {
        return path.to_string();
    }
    let store = store_dir(root);
    // A finished worker's folder has moved to the archive; the path the
    // done message named (and the one the parent remembers) still reads.
    if let Some(rest) = trimmed.strip_prefix(".arbos/agents/")
        && !store.join("agents").join(rest).exists()
    {
        let archived = store.join("archive/agents").join(rest);
        if archived.exists() {
            return archived.display().to_string();
        }
    }
    if let Some(rest) = trimmed
        .strip_prefix(".arbos/")
        .or_else(|| (trimmed == ".arbos").then_some(""))
    {
        // From a worktree, `.arbos/…` means the place's store, which the
        // worktree does not contain.
        if worktree_store(root).is_some() {
            return store.join(rest).display().to_string();
        }
        return path.to_string();
    }
    let mut parts = trimmed.splitn(2, '/');
    let head = parts.next().unwrap_or("");
    let rest = parts.next();
    let store_rel = match (head, rest) {
        ("docs" | "internal" | "media", _) => trimmed.to_string(),
        ("archived.md", None) => trimmed.to_string(),
        ("project-context.md", None) => format!("docs/{trimmed}"),
        _ => return path.to_string(),
    };
    if cwd.join(head).exists() {
        return path.to_string();
    }
    let target = store.join(&store_rel);
    let dir_exists = target.parent().is_some_and(|d| d.exists());
    if target.exists() || dir_exists {
        return target.display().to_string();
    }
    path.to_string()
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
    find_sorted(cwd, pattern, false)
}

/// `find`, by name or by modification time (newest first).
pub fn find_sorted(cwd: &Path, pattern: &str, by_mtime: bool) -> Result<ToolOut> {
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
    if by_mtime {
        let mut stamped: Vec<(std::time::SystemTime, String)> = paths
            .into_iter()
            .map(|p| {
                let t = std::fs::metadata(cwd.join(&p))
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                (t, p)
            })
            .collect();
        stamped.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        paths = stamped.into_iter().map(|(_, p)| p).collect();
    } else {
        paths.sort();
    }
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
    let old = std::fs::read_to_string(&file).ok();
    if old.as_deref() == Some(contents) {
        return Err(unchanged(&file));
    }
    // The project page keeps its head whoever rewrites it: a coordinator
    // that `write`s the whole page with its own title dropped the front
    // matter and the context link the page algorithm and `check` expect
    // (kickoff item 11). The body is the model's; the head is the page's.
    let page = store_dir(root).join(arbos_core::store::NOTES);
    let is_page = realize(&normalize(&file)) == realize(&normalize(&page));
    let kept =
        is_page.then(|| arbos_core::notes::keep_page_head(old.as_deref().unwrap_or(""), contents));
    let written = kept.as_deref().unwrap_or(contents);
    std::fs::write(&file, written)?;
    let mut body = format!("wrote {}", file.display());
    if kept.is_some_and(|k| k != contents) {
        body.push_str(
            "\n(the page's head — front matter and the link to docs/project-context.md — was kept above your text; plan set/add/check keep the page's shape for you)",
        );
    }
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

/// `edit` with `replace_all`: every occurrence of `old` becomes `new`
/// (Cursor's StrReplace `replace_all`; a rename across one file).
pub fn edit_all(root: &Path, cwd: &Path, path: &str, old: &str, new: &str) -> Result<ToolOut> {
    let file = confine(root, cwd, path)?;
    if old.is_empty() {
        bail!("old_string must not be empty");
    }
    let text = std::fs::read_to_string(&file)?;
    let count = text.matches(old).count();
    if count == 0 {
        bail!("old_string not found in {}", file.display());
    }
    let next = text.replace(old, new);
    if next == text {
        return Err(unchanged(&file));
    }
    std::fs::write(&file, &next)?;
    let mut body = format!("edited {} ({count} occurrence(s) replaced)", file.display());
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
    grep_walk_with(cwd, pattern, glob, false)
}

/// `grep_walk`, case-insensitive when asked (the literal fallback too).
pub fn grep_walk_with(
    cwd: &Path,
    pattern: &str,
    glob: Option<&str>,
    ignore_case: bool,
) -> Result<Vec<GrepHit>> {
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build()
        .or_else(|_| {
            regex::RegexBuilder::new(&regex::escape(pattern))
                .case_insensitive(ignore_case)
                .build()
        })?;
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

#[cfg(test)]
mod store_alias_tests {
    use super::*;

    fn place() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".arbos/docs")).unwrap();
        std::fs::create_dir_all(dir.path().join(".arbos/internal")).unwrap();
        std::fs::write(dir.path().join(".arbos/docs/project-context.md"), "# ctx").unwrap();
        dir
    }

    #[test]
    fn docs_and_context_paths_mean_the_store_when_nothing_else_has_them() {
        let d = place();
        let root = d.path();
        let store = root.join(".arbos");
        assert_eq!(
            confine(root, root, "docs/research.md").unwrap(),
            store.join("docs/research.md")
        );
        assert_eq!(
            confine(root, root, "./docs/project-context.md").unwrap(),
            store.join("docs/project-context.md")
        );
        assert_eq!(
            confine(root, root, "project-context.md").unwrap(),
            store.join("docs/project-context.md")
        );
        assert_eq!(
            confine(root, root, "internal/notes-for-qa.md").unwrap(),
            store.join("internal/notes-for-qa.md")
        );
        // Already spelled with the store: unchanged.
        assert_eq!(
            confine(root, root, ".arbos/docs/x.md").unwrap(),
            store.join("docs/x.md")
        );
    }

    #[test]
    fn an_archived_workers_folder_still_reads_at_its_old_path() {
        let d = place();
        let root = d.path();
        let store = root.join(".arbos");
        std::fs::create_dir_all(store.join("archive/agents/w1")).unwrap();
        std::fs::write(store.join("archive/agents/w1/transcript.jsonl"), "{}\n").unwrap();
        std::fs::create_dir_all(store.join("agents/live")).unwrap();
        assert_eq!(
            confine(root, root, ".arbos/agents/w1/transcript.jsonl").unwrap(),
            store.join("archive/agents/w1/transcript.jsonl")
        );
        // A live folder is itself; an unknown one stays where it was asked.
        assert_eq!(
            confine(root, root, ".arbos/agents/live/notes.md").unwrap(),
            store.join("agents/live/notes.md")
        );
        assert_eq!(
            confine(root, root, ".arbos/agents/nobody/notes.md").unwrap(),
            store.join("agents/nobody/notes.md")
        );
    }

    #[test]
    fn a_real_docs_folder_at_the_place_wins() {
        let d = place();
        let root = d.path();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        assert_eq!(
            confine(root, root, "docs/readme.md").unwrap(),
            root.join("docs/readme.md")
        );
        // Other names never alias.
        assert_eq!(
            confine(root, root, "src/main.rs").unwrap(),
            root.join("src/main.rs")
        );
        // No media/ in the store yet and none at the place: no alias.
        assert_eq!(
            confine(root, root, "media/x.png").unwrap(),
            root.join("media/x.png")
        );
    }

    /// qa-035: a worktree child is confined to its worktree for the
    /// checkout, but the project store is every worker's — by absolute
    /// path, by `.arbos/…`, and by the spoken aliases. Other workers'
    /// worktrees are not.
    #[test]
    fn a_worktree_root_reaches_the_places_store_but_not_other_worktrees() {
        let d = place();
        let place_root = d.path();
        let store = place_root.join(".arbos");
        let wt = store.join("worktrees/w1");
        let other = store.join("worktrees/w2");
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::create_dir_all(store.join("agents/w1")).unwrap();
        std::fs::write(other.join("secret.rs"), "x").unwrap();
        let root = wt.as_path();
        assert_eq!(store_dir(root), store);
        // Its own checkout, as before.
        assert_eq!(
            confine(root, root, "src/main.rs").unwrap(),
            wt.join("src/main.rs")
        );
        // The store, three spellings.
        let ctx = store.join("docs/project-context.md");
        assert_eq!(
            confine(root, root, &ctx.display().to_string()).unwrap(),
            ctx
        );
        assert_eq!(
            confine(root, root, ".arbos/docs/project-context.md").unwrap(),
            ctx
        );
        assert_eq!(confine(root, root, "project-context.md").unwrap(), ctx);
        assert_eq!(
            confine(root, root, "docs/new-report.md").unwrap(),
            store.join("docs/new-report.md")
        );
        assert_eq!(
            confine(root, root, ".arbos/notes.md").unwrap(),
            store.join("notes.md")
        );
        // Its own folder.
        assert_eq!(
            confine(root, root, ".arbos/agents/w1/notes.md").unwrap(),
            store.join("agents/w1/notes.md")
        );
        // Not the parent's checkout, not a sibling's worktree.
        assert!(
            confine(
                root,
                root,
                &place_root.join("main.py").display().to_string()
            )
            .is_err()
        );
        assert!(confine(root, root, &other.join("secret.rs").display().to_string()).is_err());
        assert!(confine(root, root, ".arbos/worktrees/w2/secret.rs").is_err());
        assert!(confine(root, root, "../w2/secret.rs").is_err());
    }
}

#[cfg(test)]
mod store_address_tests {
    use super::*;
    use crate::tool::WebCfg;
    use crate::tools::{Hooks, StoreFile, StoreWritten};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// A peer store in memory: what the kernel would answer through the
    /// hub, without a hub. `down` makes every call fail like an
    /// unreachable machine.
    #[derive(Default)]
    struct FakeMesh {
        files: Mutex<HashMap<String, String>>,
        down: bool,
        writes: Mutex<Vec<(String, Option<String>)>>,
    }

    impl Hooks for FakeMesh {
        fn approve(
            &self,
            _agent: &arbos_core::AgentId,
            _tool: &str,
            _command: &str,
        ) -> BoxFuture<'static, Result<bool>> {
            Box::pin(async { Ok(true) })
        }
        fn store_read(&self, address: &str) -> BoxFuture<'static, Result<StoreFile>> {
            let a = address.to_string();
            if self.down {
                return Box::pin(async move {
                    bail!("{a}: arboslife is not reachable through the hub; nothing was read")
                });
            }
            let got = self.files.lock().unwrap().get(&a).cloned();
            Box::pin(async move {
                let text = got.with_context(|| format!("{a}: no such file"))?;
                Ok(StoreFile {
                    hash: arbos_core::hub::content_hash(text.as_bytes()),
                    size: text.len() as u64,
                    text,
                    truncated: false,
                })
            })
        }
        fn store_list(
            &self,
            address: &str,
        ) -> BoxFuture<'static, Result<Vec<arbos_core::wire::Entry>>> {
            let prefix = address.trim_end_matches('/').to_string() + "/";
            let names: Vec<String> = self
                .files
                .lock()
                .unwrap()
                .keys()
                .filter_map(|k| k.strip_prefix(&prefix).map(str::to_string))
                .collect();
            Box::pin(async move {
                Ok(names
                    .into_iter()
                    .map(|n| arbos_core::wire::Entry {
                        dir: n.contains('/'),
                        name: n.split('/').next().unwrap_or("").to_string(),
                        size: 0,
                        modified: None,
                    })
                    .collect())
            })
        }
        fn store_write(
            &self,
            address: &str,
            text: String,
            base_hash: Option<String>,
        ) -> BoxFuture<'static, Result<StoreWritten>> {
            let a = address.to_string();
            self.writes
                .lock()
                .unwrap()
                .push((a.clone(), base_hash.clone()));
            let mut files = self.files.lock().unwrap();
            let current = files
                .get(&a)
                .map(|t| arbos_core::hub::content_hash(t.as_bytes()))
                .unwrap_or_default();
            if let Some(base) = base_hash
                && base != current
            {
                return Box::pin(async move {
                    bail!("{a}: conflict — the file changed since you read it")
                });
            }
            if a.ends_with("notes.md") {
                return Box::pin(async move { bail!("{a}: {}", arbos_core::store::REFUSAL) });
            }
            let size = text.len() as u64;
            let hash = arbos_core::hub::content_hash(text.as_bytes());
            files.insert(a, text);
            Box::pin(async move { Ok(StoreWritten { size, hash }) })
        }
    }

    struct NoGrep;
    impl crate::tools::Grep for NoGrep {
        fn search(&self, _p: &str, _g: Option<&str>) -> Result<Vec<GrepHit>> {
            Ok(vec![])
        }
    }

    fn cx(mesh: Arc<FakeMesh>, dir: &Path) -> RunCx {
        RunCx {
            place: arbos_core::Place::new(dir.to_path_buf()),
            agent: arbos_core::Agent::root("root"),
            cwd: dir.to_path_buf(),
            call_id: String::new(),
            cancel: tokio_util::sync::CancellationToken::new(),
            grep: Arc::new(NoGrep),
            hooks: mesh,
            bash_wait_ms: 0,
            hops: 0,
            web: Arc::new(WebCfg::default()),
        }
    }

    fn run(tool: &dyn Tool, cx: RunCx, args: Value) -> Result<ToolOut> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(tool.run(cx, args))
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("arbos-mesh-fs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".arbos")).unwrap();
        dir
    }

    const NOTES: &str = "arbos://cloud/demo/notes.md";
    const DOC: &str = "arbos://cloud/demo/docs/plan.md";

    fn mesh() -> Arc<FakeMesh> {
        let m = FakeMesh::default();
        m.files.lock().unwrap().insert(
            NOTES.into(),
            "# Notes\n\n- [ ] [Voice](docs/voice.md) — landing\n".into(),
        );
        m.files
            .lock()
            .unwrap()
            .insert(DOC.into(), "one\ntwo\nthree\n".into());
        Arc::new(m)
    }

    /// A remote worker reads its parent's notes by address and gets the
    /// same LINE:HASH lines a local read gives; `ls` lists the store.
    #[test]
    fn read_and_ls_by_address_go_through_the_mesh() {
        let dir = scratch("read");
        let m = mesh();
        let plan = Read
            .plan(
                &PlanCx {
                    root: &dir,
                    cwd: &dir,
                    agent: &arbos_core::Agent::root("root"),
                },
                &json!({"path": NOTES}),
            )
            .unwrap();
        assert!(plan.access.is_readonly());
        assert_eq!(
            plan.access.reads,
            vec![crate::access::Resource::Store(NOTES.into())]
        );
        let out = run(&Read, cx(m.clone(), &dir), json!({"path": NOTES})).unwrap();
        assert!(out.body.starts_with("     1:"), "{}", out.body);
        assert!(out.body.contains("|# Notes"), "{}", out.body);
        assert_eq!(out.paths, vec![NOTES.to_string()]);
        let page = run(
            &Read,
            cx(m.clone(), &dir),
            json!({"path": DOC, "offset": 2, "limit": 1}),
        )
        .unwrap();
        assert!(
            page.body.contains("|two") && !page.body.contains("|one"),
            "{}",
            page.body
        );
        let ls = run(
            &Ls,
            cx(m.clone(), &dir),
            json!({"path": "arbos://cloud/demo/"}),
        )
        .unwrap();
        assert!(
            ls.body.contains("notes.md") && ls.body.contains("docs/"),
            "{}",
            ls.body
        );
        // A bad address is refused at plan time, before anything runs.
        assert!(
            Read.plan(
                &PlanCx {
                    root: &dir,
                    cwd: &dir,
                    agent: &arbos_core::Agent::root("root"),
                },
                &json!({"path": "arbos://cloud"}),
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An unreachable peer is an error naming the address, never empty
    /// text an agent could take for an empty file.
    #[test]
    fn an_unreachable_peer_fails_loudly() {
        let dir = scratch("down");
        let m = Arc::new(FakeMesh {
            down: true,
            ..FakeMesh::default()
        });
        let err = run(&Read, cx(m, &dir), json!({"path": NOTES})).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(NOTES) && msg.contains("not reachable"),
            "{msg}"
        );
        // A host with no hub says so too.
        struct NoHub;
        impl Hooks for NoHub {
            fn approve(
                &self,
                _a: &arbos_core::AgentId,
                _t: &str,
                _c: &str,
            ) -> BoxFuture<'static, Result<bool>> {
                Box::pin(async { Ok(true) })
            }
        }
        let cx = RunCx {
            hooks: Arc::new(NoHub),
            ..cx(Arc::new(FakeMesh::default()), &dir)
        };
        let err = run(&Read, cx, json!({"path": NOTES})).unwrap_err();
        assert!(
            err.to_string()
                .contains("needs a kernel registered on a hub"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `write` by address is a write (plan mode and readonly agents see
    /// it); the far node's rules answer: the parent's notes.md is refused.
    #[test]
    fn write_by_address_is_a_write_and_the_far_node_rules() {
        let dir = scratch("write");
        let m = mesh();
        let plan = Write
            .plan(
                &PlanCx {
                    root: &dir,
                    cwd: &dir,
                    agent: &arbos_core::Agent::root("root"),
                },
                &json!({"path": DOC, "contents": "x"}),
            )
            .unwrap();
        assert!(!plan.access.is_readonly());
        let out = run(
            &Write,
            cx(m.clone(), &dir),
            json!({"path": "arbos://cloud/demo/docs/new.md", "contents": "# New\n"}),
        )
        .unwrap();
        assert!(
            out.body.contains("wrote arbos://cloud/demo/docs/new.md"),
            "{}",
            out.body
        );
        assert_eq!(
            m.files
                .lock()
                .unwrap()
                .get("arbos://cloud/demo/docs/new.md")
                .map(String::as_str),
            Some("# New\n")
        );
        let err = run(
            &Write,
            cx(m.clone(), &dir),
            json!({"path": NOTES, "contents": "mine now"}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("owned by the main chat"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `edit` by address fetches, edits the copy with the local rules
    /// (classic, hashline, whole), and puts it back with the hash it
    /// read; a change in between is a conflict, not a lost update.
    #[test]
    fn edit_by_address_is_a_compare_and_swap() {
        let dir = scratch("edit");
        let m = mesh();
        let out = run(
            &Edit,
            cx(m.clone(), &dir),
            json!({"path": DOC, "old_string": "two", "new_string": "deux"}),
        )
        .unwrap();
        assert!(
            out.body.contains("edited arbos://cloud/demo/docs/plan.md"),
            "{}",
            out.body
        );
        assert_eq!(
            m.files.lock().unwrap().get(DOC).map(String::as_str),
            Some("one\ndeux\nthree\n")
        );
        let (_, base) = m.writes.lock().unwrap().last().cloned().unwrap();
        assert_eq!(
            base.as_deref(),
            Some(arbos_core::hub::content_hash(b"one\ntwo\nthree\n").as_str()),
            "the put carries the hash that was read"
        );
        // Hashline anchors from a read by address work too.
        let read = run(&Read, cx(m.clone(), &dir), json!({"path": DOC})).unwrap();
        let anchor = read
            .body
            .lines()
            .nth(2)
            .unwrap()
            .split('|')
            .next()
            .unwrap()
            .trim()
            .to_string();
        assert!(anchor.starts_with("3:"), "{anchor}");
        run(
            &Edit,
            cx(m.clone(), &dir),
            json!({"path": DOC, "anchor": anchor, "content": "trois"}),
        )
        .unwrap();
        assert_eq!(
            m.files.lock().unwrap().get(DOC).map(String::as_str),
            Some("one\ndeux\ntrois\n")
        );
        // Whole-file: content with no anchor and no old_string.
        run(
            &Edit,
            cx(m.clone(), &dir),
            json!({"path": DOC, "content": "whole\n"}),
        )
        .unwrap();
        assert_eq!(
            m.files.lock().unwrap().get(DOC).map(String::as_str),
            Some("whole\n")
        );
        // A conflict: the store changes between the read and the put.
        struct Racing(Arc<FakeMesh>);
        impl Hooks for Racing {
            fn approve(
                &self,
                a: &arbos_core::AgentId,
                t: &str,
                c: &str,
            ) -> BoxFuture<'static, Result<bool>> {
                self.0.approve(a, t, c)
            }
            fn store_read(&self, address: &str) -> BoxFuture<'static, Result<StoreFile>> {
                let f = self.0.store_read(address);
                self.0
                    .files
                    .lock()
                    .unwrap()
                    .insert(address.to_string(), "someone else\n".into());
                f
            }
            fn store_write(
                &self,
                address: &str,
                text: String,
                base_hash: Option<String>,
            ) -> BoxFuture<'static, Result<StoreWritten>> {
                self.0.store_write(address, text, base_hash)
            }
        }
        let cx_racing = RunCx {
            hooks: Arc::new(Racing(m.clone())),
            ..cx(m.clone(), &dir)
        };
        let err = run(
            &Edit,
            cx_racing,
            json!({"path": DOC, "old_string": "whole", "new_string": "lost"}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("conflict"), "{err}");
        assert_eq!(
            m.files.lock().unwrap().get(DOC).map(String::as_str),
            Some("someone else\n"),
            "the other writer's text stands"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
