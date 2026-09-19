//! Codex apply-patch format.
//!
//! One tool call can add, delete, or update several files. Update hunks are
//! checked: the `-` lines must still exist in the file. We do not write until
//! every hunk matches.

use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{ToolOut, fs};
use crate::access::Access;
use crate::tool::{BoxFuture, Plan as ToolPlan, PlanCx, RunCx, Tool, blocking, req};

pub struct ApplyPatch;

impl Tool for ApplyPatch {
    fn name(&self) -> &'static str {
        "apply_patch"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "apply_patch",
                "description": "Apply a Codex multi-file patch. One call can add, delete, or update several files. Hunks are checked: the '-' lines must still be in the file.\n\n*** Begin Patch\n*** Add File: path\n+line\n*** Delete File: path\n*** Update File: path\n@@ optional context\n-old\n+new\n*** End Patch",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "patch": {
                            "type": "string",
                            "description": "Full patch text, including *** Begin Patch and *** End Patch."
                        },
                        "mechanism": crate::mechanism::schema_property()
                    },
                    "required": ["patch"]
                }
            }
        })
    }
    fn plan(&self, cx: &PlanCx, args: &Value) -> Result<ToolPlan> {
        let paths = paths_in(req(args, "patch")?);
        if paths.is_empty() {
            // Malformed patch: run alone so the error surfaces in order.
            return Ok(ToolPlan::access(Access::exclusive()));
        }
        let resolved = paths
            .iter()
            .map(|p| cx.resolve_write(p))
            .collect::<Result<Vec<_>>>()?;
        Ok(ToolPlan::access(Access::writes(resolved)))
    }
    fn run(&self, cx: RunCx, args: Value) -> BoxFuture<'static, Result<ToolOut>> {
        blocking(move || apply(cx.root(), &cx.cwd, req(&args, "patch")?))
    }
}

/// Every path a patch names, from its headers only. No hunk matching.
pub fn paths_in(patch: &str) -> Vec<String> {
    patch
        .lines()
        .map(str::trim)
        .filter_map(|l| {
            l.strip_prefix(ADD)
                .or_else(|| l.strip_prefix(DELETE))
                .or_else(|| l.strip_prefix(UPDATE))
                .or_else(|| l.strip_prefix(MOVE))
        })
        .map(str::to_string)
        .collect()
}

const BEGIN: &str = "*** Begin Patch";
const END: &str = "*** End Patch";
const ADD: &str = "*** Add File: ";
const DELETE: &str = "*** Delete File: ";
const UPDATE: &str = "*** Update File: ";
const MOVE: &str = "*** Move to: ";
const EOF: &str = "*** End of File";

#[derive(Debug)]
enum Hunk {
    Add {
        path: PathBuf,
        contents: String,
    },
    Delete {
        path: PathBuf,
    },
    Update {
        path: PathBuf,
        move_to: Option<PathBuf>,
        chunks: Vec<Chunk>,
    },
}

#[derive(Debug)]
struct Chunk {
    context: Option<String>,
    old: Vec<String>,
    new: Vec<String>,
    eof: bool,
}

enum Plan {
    Create(PathBuf, String),
    Replace(PathBuf, String),
    Remove(PathBuf),
    Relocate {
        from: PathBuf,
        to: PathBuf,
        contents: String,
    },
}

pub fn apply(root: &Path, cwd: &Path, patch: &str) -> Result<ToolOut> {
    let hunks = parse_patch(patch)?;
    if hunks.is_empty() {
        bail!("empty patch");
    }
    let mut pending: HashMap<PathBuf, String> = HashMap::new();
    let mut plans = Vec::new();
    for h in &hunks {
        plans.push(plan_hunk(root, cwd, h, &mut pending)?);
    }
    let mut paths = Vec::new();
    let mut body = String::new();
    // Every hunk was planned before this; the writes are still one file
    // at a time, and a write that fails midway has applied the files
    // before it. The error says which, so "patch failed" never hides a
    // half-applied patch from the model or the diff view.
    let total = plans.len();
    if let Err(e) = write_plans(plans, &mut body, &mut paths) {
        let done = body.trim_end();
        anyhow::bail!(
            "{e}\n{} of {total} file step(s) were already applied before this failed{}{}",
            paths.len(),
            if done.is_empty() { "" } else { ":\n" },
            done
        );
    }
    Ok(ToolOut::with_paths(body, paths))
}

fn write_plans(plans: Vec<Plan>, body: &mut String, paths: &mut Vec<String>) -> Result<()> {
    for p in plans {
        match p {
            Plan::Create(path, contents) => {
                if path.exists() {
                    bail!("add failed; already exists: {}", path.display());
                }
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, contents)?;
                body.push_str(&format!("added {}\n", path.display()));
                paths.push(path.display().to_string());
            }
            Plan::Replace(path, contents) => {
                arbos_core::record::replace_file(&path, contents.as_bytes())?;
                body.push_str(&format!("updated {}\n", path.display()));
                paths.push(path.display().to_string());
            }
            Plan::Remove(path) => {
                std::fs::remove_file(&path)?;
                body.push_str(&format!("deleted {}\n", path.display()));
                paths.push(path.display().to_string());
            }
            Plan::Relocate { from, to, contents } => {
                if to.exists() {
                    bail!("move failed; already exists: {}", to.display());
                }
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&to, contents)?;
                std::fs::remove_file(&from)?;
                body.push_str(&format!("moved {} → {}\n", from.display(), to.display()));
                paths.push(to.display().to_string());
            }
        }
    }
    Ok(())
}

fn plan_hunk(
    root: &Path,
    cwd: &Path,
    hunk: &Hunk,
    pending: &mut HashMap<PathBuf, String>,
) -> Result<Plan> {
    match hunk {
        Hunk::Add { path, contents } => {
            let file = fs::confine(root, cwd, &path_str(path))?;
            if file.exists() || pending.contains_key(&file) {
                bail!("add failed; already exists: {}", file.display());
            }
            pending.insert(file.clone(), contents.clone());
            Ok(Plan::Create(file, contents.clone()))
        }
        Hunk::Delete { path } => {
            let file = fs::confine(root, cwd, &path_str(path))?;
            if !file.exists() && !pending.contains_key(&file) {
                bail!("delete failed; missing {}", file.display());
            }
            pending.remove(&file);
            Ok(Plan::Remove(file))
        }
        Hunk::Update {
            path,
            move_to,
            chunks,
        } => {
            let file = fs::confine(root, cwd, &path_str(path))?;
            let original = if let Some(text) = pending.get(&file) {
                text.clone()
            } else {
                if !file.exists() {
                    bail!("update failed; missing {}", file.display());
                }
                // Not UTF-8 is refused with what it is, not "missing".
                fs::text_for_edit(&file)?
            };
            let next = derive_new_contents(&original, &file, chunks)?;
            if let Some(dest) = move_to {
                let to = fs::confine(root, cwd, &path_str(dest))?;
                if (to.exists() || pending.contains_key(&to)) && to != file {
                    bail!("move failed; already exists: {}", to.display());
                }
                pending.remove(&file);
                pending.insert(to.clone(), next.clone());
                Ok(Plan::Relocate {
                    from: file,
                    to,
                    contents: next,
                })
            } else {
                pending.insert(file.clone(), next.clone());
                Ok(Plan::Replace(file, next))
            }
        }
    }
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn parse_patch(patch: &str) -> Result<Vec<Hunk>> {
    let trimmed = patch.trim();
    let mut lines: Vec<&str> = trimmed.lines().collect();
    if matches!(
        lines.first().copied(),
        Some("<<EOF" | "<<'EOF'" | "<<\"EOF\"")
    ) && lines.len() >= 4
        && lines.last().is_some_and(|l| l.ends_with("EOF"))
    {
        lines = lines[1..lines.len() - 1].to_vec();
    }
    let first = lines.first().map(|l| l.trim());
    let last = lines.last().map(|l| l.trim());
    if first != Some(BEGIN) {
        bail!("patch must start with '{BEGIN}'");
    }
    // A missing end marker is the commonest slip and costs nothing to
    // forgive: the body is either well-formed or the hunk parser says why.
    let body_end = if last == Some(END) {
        lines.len() - 1
    } else {
        lines.len()
    };
    if lines.len() < 2 {
        return Ok(Vec::new());
    }
    let mut rest = &lines[1..body_end];
    if !rest.iter().any(|l| l.starts_with("*** ")) {
        bail!(
            "patch has no file header: after '{BEGIN}' each file needs '{UPDATE}path', '{ADD}path' or '{DELETE}path' before its +/- lines"
        );
    }
    let mut hunks = Vec::new();
    let mut lineno = 2;
    while !rest.is_empty() {
        if rest[0].trim().is_empty() {
            lineno += 1;
            rest = &rest[1..];
            continue;
        }
        let (hunk, n) = parse_hunk(rest, lineno)?;
        hunks.push(hunk);
        lineno += n;
        rest = &rest[n..];
    }
    Ok(hunks)
}

fn parse_hunk(lines: &[&str], lineno: usize) -> Result<(Hunk, usize)> {
    let first = lines[0].trim();
    if let Some(path) = first.strip_prefix(ADD) {
        let mut contents = String::new();
        let mut n = 1;
        for line in &lines[1..] {
            if let Some(body) = line.strip_prefix('+') {
                contents.push_str(body);
                contents.push('\n');
                n += 1;
            } else {
                break;
            }
        }
        return Ok((
            Hunk::Add {
                path: PathBuf::from(path),
                contents,
            },
            n,
        ));
    }
    if let Some(path) = first.strip_prefix(DELETE) {
        return Ok((
            Hunk::Delete {
                path: PathBuf::from(path),
            },
            1,
        ));
    }
    if let Some(path) = first.strip_prefix(UPDATE) {
        let mut rest = &lines[1..];
        let mut n = 1;
        let move_to = rest
            .first()
            .and_then(|l| l.strip_prefix(MOVE))
            .map(PathBuf::from);
        if move_to.is_some() {
            rest = &rest[1..];
            n += 1;
        }
        let mut chunks = Vec::new();
        while !rest.is_empty() {
            if rest[0].trim().is_empty() {
                n += 1;
                rest = &rest[1..];
                continue;
            }
            if rest[0].starts_with("***") {
                break;
            }
            let (chunk, used) = parse_chunk(rest, lineno + n, chunks.is_empty())?;
            chunks.push(chunk);
            n += used;
            rest = &rest[used..];
        }
        if chunks.is_empty() {
            bail!("empty update hunk for {path} (line {lineno})");
        }
        return Ok((
            Hunk::Update {
                path: PathBuf::from(path),
                move_to,
                chunks,
            },
            n,
        ));
    }
    bail!(
        "line {lineno}: expected '{ADD}{{path}}', '{DELETE}{{path}}', or '{UPDATE}{{path}}'; got {first:?}"
    );
}

/// `-1,10 +1,54 @@` or `-1 +1 @@`, with or without the trailing `@@`.
fn is_unified_range(s: &str) -> bool {
    let s = s.trim().trim_end_matches("@@").trim();
    let mut parts = s.split_whitespace();
    let range = |p: Option<&str>, sign: char| {
        p.is_some_and(|p| {
            p.strip_prefix(sign).is_some_and(|r| {
                r.split(',')
                    .all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            })
        })
    };
    range(parts.next(), '-') && range(parts.next(), '+') && parts.next().is_none()
}

fn parse_chunk(lines: &[&str], lineno: usize, allow_bare: bool) -> Result<(Chunk, usize)> {
    let (context, start) = if lines[0] == "@@" {
        (None, 1)
    } else if let Some(ctx) = lines[0].strip_prefix("@@ ") {
        // `@@ -1,10 +1,54 @@` is a unified-diff range, not context text.
        // Models trained on git output write it by habit; the numbers say
        // nothing this parser can use, so the chunk is anchored by its
        // `-` lines alone.
        if is_unified_range(ctx) {
            (None, 1)
        } else {
            (Some(ctx.trim_end_matches(" @@").to_string()), 1)
        }
    } else if allow_bare {
        (None, 0)
    } else {
        bail!("line {lineno}: update chunk must start with @@");
    };
    if start >= lines.len() {
        bail!("line {}: empty update chunk", lineno + 1);
    }
    let mut chunk = Chunk {
        context,
        old: Vec::new(),
        new: Vec::new(),
        eof: false,
    };
    let mut used = 0;
    for line in &lines[start..] {
        if *line == EOF {
            if used == 0 {
                bail!("line {}: empty update chunk", lineno + 1);
            }
            chunk.eof = true;
            used += 1;
            break;
        }
        match line.chars().next() {
            None => {
                chunk.old.push(String::new());
                chunk.new.push(String::new());
            }
            Some(' ') => {
                chunk.old.push(line[1..].to_string());
                chunk.new.push(line[1..].to_string());
            }
            Some('+') => chunk.new.push(line[1..].to_string()),
            Some('-') => chunk.old.push(line[1..].to_string()),
            _ if used == 0 => {
                bail!(
                    "line {}: hunk line must start with ' ', '+', or '-'",
                    lineno + 1
                );
            }
            _ => break,
        }
        used += 1;
    }
    Ok((chunk, used + start))
}

fn derive_new_contents(original: &str, path: &Path, chunks: &[Chunk]) -> Result<String> {
    // The file's own line ending, kept (hashline's rule, #736). Lines are
    // matched and stored without their `\r`: with it, a CRLF file's
    // context matched only through the trimmed pass and came back from
    // the patch as LF — three lines, two flipped, a mixed file whose diff
    // showed the context as changed.
    let eol = super::hashline::line_ending(original);
    let mut lines: Vec<String> = original
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let mut reps = Vec::new();
    let mut idx = 0;
    for chunk in chunks {
        if let Some(ctx) = &chunk.context {
            if let Some(found) = seek_sequence(&lines, std::slice::from_ref(ctx), idx, false) {
                idx = found + 1;
            } else {
                bail!("context {:?} not found in {}", ctx, path.display());
            }
        }
        if chunk.old.is_empty() {
            let at = lines.len();
            reps.push((at, 0, chunk.new.clone()));
            continue;
        }
        let mut pattern: &[String] = &chunk.old;
        let mut new_slice: &[String] = &chunk.new;
        let mut found = seek_sequence(&lines, pattern, idx, chunk.eof);
        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_sequence(&lines, pattern, idx, chunk.eof);
        }
        if let Some(start) = found {
            reps.push((start, pattern.len(), new_slice.to_vec()));
            idx = start + pattern.len();
        } else {
            bail!(
                "hunk not found in {}:\n{}",
                path.display(),
                chunk.old.join("\n")
            );
        }
    }
    reps.sort_by_key(|(i, _, _)| *i);
    for (start, old_len, new) in reps.iter().rev() {
        let start = *start;
        for _ in 0..*old_len {
            if start < lines.len() {
                lines.remove(start);
            }
        }
        for (off, line) in new.iter().enumerate() {
            lines.insert(start + off, line.clone());
        }
    }
    if !lines.last().is_some_and(String::is_empty) {
        lines.push(String::new());
    }
    Ok(lines.join(eol))
}

fn seek_sequence(lines: &[String], pattern: &[String], start: usize, eof: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    if pattern.len() > lines.len() {
        return None;
    }
    let search_start = if eof && lines.len() >= pattern.len() {
        lines.len() - pattern.len()
    } else {
        start
    };
    let last = lines.len().saturating_sub(pattern.len());
    if search_start > last {
        return None;
    }
    for i in search_start..=last {
        if lines[i..i + pattern.len()] == *pattern {
            return Some(i);
        }
    }
    for i in search_start..=last {
        if pattern
            .iter()
            .enumerate()
            .all(|(k, p)| lines[i + k].trim_end() == p.trim_end())
        {
            return Some(i);
        }
    }
    for i in search_start..=last {
        if pattern
            .iter()
            .enumerate()
            .all(|(k, p)| lines[i + k].trim() == p.trim())
        {
            return Some(i);
        }
    }
    (search_start..=last).find(|&i| {
        pattern
            .iter()
            .enumerate()
            .all(|(k, p)| ascii_punct(&lines[i + k]) == ascii_punct(p))
    })
}

fn ascii_punct(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "arbos-patch-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn wrap(body: &str) -> String {
        format!("*** Begin Patch\n{body}\n*** End Patch")
    }

    /// Control on main cc369869: a three-line CRLF file, one hunk with one
    /// context line and one replaced line, came back "foo\nqux\nbaz\r\n" —
    /// two of three endings flipped, the context line in the diff. Now
    /// every line keeps CRLF, the new one included; a Latin-1 file is
    /// refused with what it is.
    #[test]
    fn a_crlf_file_keeps_its_endings_through_a_patch() {
        let dir = tmp();
        fs::write(dir.join("a.txt"), "foo\r\nbar\r\nbaz\r\n").unwrap();
        apply(
            &dir,
            &dir,
            &wrap("*** Update File: a.txt\n@@\n foo\n-bar\n+qux\n+quux\n"),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("a.txt")).unwrap(),
            "foo\r\nqux\r\nquux\r\nbaz\r\n"
        );
        fs::write(dir.join("old.c"), b"/* caf\xe9 */\nint x = 1;\n").unwrap();
        let err = apply(
            &dir,
            &dir,
            &wrap("*** Update File: old.c\n@@\n-int x = 1;\n+int x = 2;\n"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("is not valid UTF-8"), "{err:#}");
        assert_eq!(
            fs::read(dir.join("old.c")).unwrap(),
            b"/* caf\xe9 */\nint x = 1;\n"
        );
    }

    #[test]
    fn update_and_add() {
        let dir = tmp();
        fs::write(dir.join("a.txt"), "foo\nbar\n").unwrap();
        apply(
            &dir,
            &dir,
            &wrap("*** Update File: a.txt\n@@\n foo\n-bar\n+baz\n*** Add File: b.txt\n+hello\n"),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "foo\nbaz\n");
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "hello\n");
    }

    #[test]
    fn missing_hunk_writes_nothing() {
        let dir = tmp();
        fs::write(dir.join("a.txt"), "foo\nbar\n").unwrap();
        let err = apply(
            &dir,
            &dir,
            &wrap("*** Update File: a.txt\n@@\n-nope\n+yes\n*** Add File: b.txt\n+x\n"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("not found"));
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "foo\nbar\n");
        assert!(!dir.join("b.txt").exists());
    }

    /// Garbage, truncated, and escaping patches must fail without writing.
    #[test]
    fn malformed_patches_write_nothing() {
        let dir = tmp();
        fs::write(dir.join("a.txt"), "foo\nbar\n").unwrap();
        let bad = [
            String::new(),
            "not a patch at all".to_string(),
            "*** Begin Patch\n".to_string(),
            wrap(""),
            wrap("*** Frobnicate File: a.txt\n+x"),
            wrap("*** Update File: missing.txt\n@@\n-foo\n+bar"),
            wrap("*** Add File: ../escape.txt\n+pwned"),
            wrap("*** Delete File: ../../etc/hosts"),
        ];
        for patch in &bad {
            let result = apply(&dir, &dir, patch);
            assert!(result.is_err(), "expected an error for {patch:?}");
            assert_eq!(
                fs::read_to_string(dir.join("a.txt")).unwrap(),
                "foo\nbar\n",
                "a.txt changed by {patch:?}"
            );
        }
        // F-040: today this writes ../escape.txt outside the working directory.
        let escaped = dir.parent().unwrap().join("escape.txt");
        let leaked = escaped.exists();
        let _ = fs::remove_file(&escaped);
        assert!(!leaked, "Add File escaped the workspace (F-040)");
    }

    #[test]
    fn unicode_dash() {
        let dir = tmp();
        fs::write(dir.join("a.py"), "import x # local \u{2013} dep\n").unwrap();
        apply(
            &dir,
            &dir,
            &wrap("*** Update File: a.py\n@@\n-import x # local - dep\n+import x # ok\n"),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("a.py")).unwrap(),
            "import x # ok\n"
        );
    }
}
