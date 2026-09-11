//! Line anchors for `edit`.
//!
//! `read` prints `LINE:HASH|text`. The model names a line by that `LINE:HASH`.
//! The hash is the line body (whitespace collapsed), not the line number.
//! If the file shifted, we search nearby for the same hash.

use anyhow::{Result, bail};
use serde_json::Value;
use std::path::Path;

use super::{ToolOut, fs};

const HASH_LEN: usize = 3;
const SEARCH_RADIUS: usize = 15;

pub fn line_tag(line: &str) -> String {
    encode(line_hash(line))
}

fn line_hash(line: &str) -> u32 {
    let norm: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    fnv1a(norm.as_bytes())
}

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in bytes {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

fn encode(mut n: u32) -> String {
    let mut out = String::with_capacity(HASH_LEN);
    for _ in 0..HASH_LEN {
        out.push(char::from(b'a' + (n % 26) as u8));
        n /= 26;
    }
    out
}

#[derive(Debug, Clone)]
enum Marker {
    Line {
        line: usize,
        hash: String,
    },
    /// A tag with no line number (`kxm` or `kxm|text` pasted from read).
    /// Resolved by searching the whole file for that tag.
    Hash {
        hash: String,
    },
    Bof,
    Eof,
}

fn is_tag(s: &str) -> bool {
    s.len() == HASH_LEN && s.bytes().all(|b| b.is_ascii_lowercase())
}

impl Marker {
    fn parse(s: &str) -> Result<Self> {
        // `12:kxm|    def score(self):` — the model pasted the whole read
        // line. Everything after the bar is the text, not the anchor.
        let s = s.split('|').next().unwrap_or("").trim();
        if s.eq_ignore_ascii_case("eof") {
            return Ok(Self::Eof);
        }
        if s == "0" || s == "0:" || s.starts_with("0:") {
            return Ok(Self::Bof);
        }
        let Some((line_s, hash)) = s.split_once(':') else {
            if let Ok(line) = s.parse::<usize>() {
                return Ok(if line == 0 {
                    Self::Bof
                } else {
                    Self::Line {
                        line,
                        hash: String::new(),
                    }
                });
            }
            let tag = s.to_ascii_lowercase();
            if is_tag(&tag) {
                return Ok(Self::Hash { hash: tag });
            }
            bail!("bad anchor {s:?}; expected LINE:HASH from read (e.g. 12:kxm)");
        };
        let line: usize = match line_s.trim().parse() {
            Ok(n) => n,
            // `kxm:` or `kxm:kxm` — the tag landed on the left. Search for it.
            Err(_) if is_tag(&line_s.trim().to_ascii_lowercase()) => {
                return Ok(Self::Hash {
                    hash: line_s.trim().to_ascii_lowercase(),
                });
            }
            Err(_) => bail!("bad anchor line in {s:?}; expected LINE:HASH from read (e.g. 12:kxm)"),
        };
        if line == 0 {
            return Ok(Self::Bof);
        }
        let hash = hash
            .split(':')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        // Not a tag `read` printed (a column, a hex digest, nothing): the
        // model still named a line. Keep the line and let `resolve_point`
        // use it as-is, with a note that shows the real tag. Failing here
        // taught weaker models nothing they could act on.
        let hash = if is_tag(&hash) { hash } else { String::new() };
        Ok(Self::Line { line, hash })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpKind {
    Replace,
    InsertAfter,
    Write,
}

#[derive(Debug, Clone)]
struct Op {
    kind: OpKind,
    start: Marker,
    end: Option<Marker>,
    content: String,
}

/// Hashline when a real anchor, a non-empty edits list, or an op is given.
/// Empty strings and `[]` are what schema-filling models send beside a
/// classic old_string/new_string; they do not choose the mode.
pub fn looks_like_hashline(args: &Value) -> bool {
    let non_empty = |k: &str| {
        args.get(k)
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty())
    };
    non_empty("anchor")
        || args.get("edits").is_some_and(|e| match e {
            Value::Array(a) => !a.is_empty(),
            Value::Object(_) => true,
            Value::String(s) => !s.trim().is_empty(),
            _ => false,
        })
        || (non_empty("op")
            && !args
                .get("old_string")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty()))
}

pub fn edit(root: &Path, cwd: &Path, path: &str, args: &Value) -> Result<ToolOut> {
    let ops = parse_ops(args)?;
    if ops.is_empty() {
        bail!("edit: no operations");
    }
    let file = fs::confine(root, cwd, path)?;
    if ops.iter().any(|o| o.kind == OpKind::Write) {
        if ops.len() != 1 {
            bail!("write op must be the only edit");
        }
        let contents = &ops[0].content;
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if std::fs::read_to_string(&file).is_ok_and(|old| old == *contents) {
            return Err(fs::unchanged(&file));
        }
        std::fs::write(&file, contents)?;
        let mut body = format!("wrote {} ({} bytes)", file.display(), contents.len());
        if let Some(note) = fs::syntax_note(&file) {
            body.push('\n');
            body.push_str(&note);
        }
        return Ok(ToolOut::with_paths(body, vec![file.display().to_string()]));
    }

    let text = std::fs::read_to_string(&file)
        .map_err(|_| anyhow::anyhow!("file not found: {}", file.display()))?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let had_trailing_nl = text.ends_with('\n');

    let mut resolved = Vec::new();
    for op in &ops {
        resolved.push(resolve_op(op, &lines)?);
    }
    check_overlap(&resolved)?;
    let around = resolved
        .iter()
        .map(|(s, _, _, _, _)| (*s).max(1))
        .min()
        .unwrap_or(1);
    resolved.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    let mut notes = Vec::new();
    for (start, end, kind, content, note) in resolved {
        if let Some(n) = note {
            notes.push(n);
        }
        let new_lines = content_lines(&content);
        match kind {
            OpKind::Replace => {
                // start is 1-based inclusive; end is 1-based exclusive.
                let start_i = start.saturating_sub(1);
                let end_i = end.saturating_sub(1).min(lines.len());
                if start_i > end_i || start_i > lines.len() {
                    bail!("anchor out of range");
                }
                lines.splice(start_i..end_i, new_lines);
            }
            OpKind::InsertAfter => {
                // `end` is the 0-based splice index (after the anchored line).
                let at = end.min(lines.len());
                lines.splice(at..at, new_lines);
            }
            OpKind::Write => unreachable!(),
        }
    }

    let mut out = lines.join("\n");
    if had_trailing_nl && !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if out == text {
        return Err(fs::unchanged(&file));
    }
    std::fs::write(&file, &out)?;

    let snippet = snippet(&out, around);
    let mut body = format!("edited {}", file.display());
    if !notes.is_empty() {
        body.push('\n');
        body.push_str(&notes.join("\n"));
    }
    if let Some(note) = fs::syntax_note(&file) {
        body.push('\n');
        body.push_str(&note);
    }
    body.push('\n');
    body.push_str(&snippet);
    Ok(ToolOut::with_paths(body, vec![file.display().to_string()])
        .with_diff(super::editdiff::numbered_diff(&text, &out)))
}

fn parse_ops(args: &Value) -> Result<Vec<Op>> {
    // Models that fill every schema field send `edits: []` beside a real
    // top-level anchor. An empty list is not an instruction; fall through.
    if let Some(edits) = args.get("edits").filter(|e| !e.is_null()) {
        let arr = match edits {
            Value::Array(a) => a.clone(),
            Value::Object(_) => vec![edits.clone()],
            Value::String(s) if s.trim().is_empty() => Vec::new(),
            Value::String(s) => serde_json::from_str(s)?,
            _ => bail!("edits must be an array"),
        };
        if !arr.is_empty() {
            return arr.iter().map(op_from_value).collect();
        }
    }
    Ok(vec![op_from_value(args)?])
}

fn op_from_value(v: &Value) -> Result<Op> {
    let op = v
        .get("op")
        .and_then(|x| x.as_str())
        .unwrap_or("replace")
        .trim()
        .to_ascii_lowercase();
    let kind = match op.as_str() {
        "replace" | "" | "delete" | "remove" => OpKind::Replace,
        "insert_after" | "insert" | "append" => OpKind::InsertAfter,
        "write" | "create" | "overwrite" => OpKind::Write,
        other => bail!("unknown edit op {other:?}; use replace, insert_after, or write"),
    };
    let content = if matches!(op.as_str(), "delete" | "remove") {
        String::new()
    } else {
        // No `content` at all is not "delete": a dropped field must not
        // erase a line. An explicit empty string still deletes.
        v.get("content")
            .and_then(|x| x.as_str())
            .or_else(|| v.get("new_string").and_then(|x| x.as_str()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "edit needs content (use op=delete, or content \"\", to remove the anchored lines)"
                )
            })?
            .to_string()
    };
    if kind == OpKind::Write {
        return Ok(Op {
            kind,
            start: Marker::Bof,
            end: None,
            content,
        });
    }
    let anchor = v
        .get("anchor")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow::anyhow!("edit needs anchor (LINE:HASH from read)"))?;
    let end = v
        .get("end_anchor")
        .and_then(|x| x.as_str())
        .map(Marker::parse)
        .transpose()?;
    Ok(Op {
        kind,
        start: Marker::parse(anchor)?,
        end,
        content,
    })
}

/// (start_line 1-based, end_exclusive 1-based, kind, content, optional note)
fn resolve_op(op: &Op, lines: &[String]) -> Result<(usize, usize, OpKind, String, Option<String>)> {
    match op.kind {
        OpKind::Write => Ok((1, lines.len() + 1, op.kind, op.content.clone(), None)),
        OpKind::InsertAfter => {
            let (at, note) = resolve_point(&op.start, lines)?;
            Ok((at, at, op.kind, op.content.clone(), note))
        }
        OpKind::Replace => {
            let (start, n1) = resolve_point(&op.start, lines)?;
            let (end, n2) = match &op.end {
                Some(m) => resolve_point(m, lines)?,
                None => (start, None),
            };
            // Reversed range: the model meant the span between them.
            let (start, end, swapped) = if end < start {
                (end, start, true)
            } else {
                (start, end, false)
            };
            let note = n1
                .into_iter()
                .chain(n2)
                .chain(swapped.then(|| {
                    "anchor and end_anchor were reversed; used the span between them".to_string()
                }))
                .next();
            Ok((start, end + 1, op.kind, op.content.clone(), note))
        }
    }
}

fn resolve_point(m: &Marker, lines: &[String]) -> Result<(usize, Option<String>)> {
    match m {
        Marker::Bof => Ok((0, None)),
        Marker::Eof => Ok((lines.len(), None)),
        Marker::Hash { hash } => {
            let hits: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| line_tag(l) == *hash)
                .map(|(i, _)| i + 1)
                .collect();
            match hits.as_slice() {
                [one] => Ok((*one, Some(format!("anchor {hash} resolved to line {one}")))),
                [] => bail!("anchor {hash} not found in the file; re-read it and use LINE:HASH"),
                many => bail!(
                    "anchor {hash} matches lines {}; use LINE:HASH to pick one",
                    many.iter()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }
        }
        Marker::Line { line, hash } => {
            let idx = line.saturating_sub(1);
            if hash.is_empty() {
                if idx < lines.len() {
                    return Ok((
                        *line,
                        Some(format!(
                            "anchor {line} had no valid hash; used line {line}:{}|{}",
                            line_tag(&lines[idx]),
                            lines[idx]
                        )),
                    ));
                }
                bail!(
                    "anchor line {line} is out of range (file has {} lines)",
                    lines.len()
                );
            }
            if idx < lines.len() && line_tag(&lines[idx]) == *hash {
                return Ok((*line, None));
            }
            let found = find_shifted(hash, *line, lines);
            match found {
                Shift::One(new_line) => Ok((
                    new_line,
                    Some(format!(
                        "anchor {line}:{hash} moved to {new_line}:{}",
                        line_tag(&lines[new_line - 1])
                    )),
                )),
                Shift::Many(cands) => {
                    bail!(
                        "anchor {line}:{hash} is stale and matches lines {}. Current text near line {line} (use these LINE:HASH):\n{}",
                        cands
                            .iter()
                            .map(|n| format!("{n}:{}", line_tag(&lines[n - 1])))
                            .collect::<Vec<_>>()
                            .join(", "),
                        around(lines, *line)
                    )
                }
                Shift::None => {
                    let here = if idx < lines.len() {
                        format!(
                            "line {line} is now {line}:{}|{}",
                            line_tag(&lines[idx]),
                            lines[idx]
                        )
                    } else {
                        format!(
                            "line {line} is out of range (file has {} lines)",
                            lines.len()
                        )
                    };
                    bail!(
                        "anchor {line}:{hash} not found. {here}. Current text near there (use these LINE:HASH):\n{}",
                        around(lines, *line)
                    )
                }
            }
        }
    }
}

enum Shift {
    One(usize),
    Many(Vec<usize>),
    None,
}

/// Twelve lines around `line` with current tags, for an anchor error.
fn around(lines: &[String], line: usize) -> String {
    if lines.is_empty() {
        return "(empty file)".into();
    }
    let lo = line.saturating_sub(6).max(1).min(lines.len());
    let hi = (line + 6).min(lines.len());
    (lo..=hi)
        .map(|n| format!("{n:>6}:{}|{}", line_tag(&lines[n - 1]), lines[n - 1]))
        .collect::<Vec<_>>()
        .join("\n")
}

fn find_shifted(hash: &str, line: usize, lines: &[String]) -> Shift {
    let lo = line.saturating_sub(SEARCH_RADIUS).max(1);
    let hi = (line + SEARCH_RADIUS).min(lines.len());
    let mut hits = Vec::new();
    for n in lo..=hi {
        if n == line {
            continue;
        }
        if line_tag(&lines[n - 1]) == hash {
            hits.push(n);
        }
    }
    match hits.len() {
        0 => {
            // Not near where the model pointed. A line whose text appears
            // exactly once in the file is still unambiguous wherever it
            // moved to — the usual case after the model's own earlier edit
            // pushed everything down.
            let all: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| line_tag(l) == hash)
                .map(|(i, _)| i + 1)
                .collect();
            match all.as_slice() {
                [one] => Shift::One(*one),
                _ => Shift::None,
            }
        }
        1 => Shift::One(hits[0]),
        _ => {
            // `}` and blank lines share a tag with many neighbours. When
            // exactly one hit sits within two lines of where the model
            // pointed, that is the line it meant; the edit result names it.
            let near: Vec<usize> = hits
                .iter()
                .copied()
                .filter(|n| n.abs_diff(line) <= 2)
                .collect();
            if near.len() == 1 {
                Shift::One(near[0])
            } else {
                Shift::Many(hits)
            }
        }
    }
}

fn check_overlap(ops: &[(usize, usize, OpKind, String, Option<String>)]) -> Result<()> {
    let mut spans: Vec<(usize, usize)> = ops
        .iter()
        .filter(|(_, _, k, _, _)| *k != OpKind::InsertAfter)
        .map(|(s, e, _, _, _)| (*s, *e))
        .collect();
    spans.sort_by_key(|s| s.0);
    for w in spans.windows(2) {
        if w[0].1 > w[1].0 {
            bail!(
                "overlapping edits at lines {}-{} and {}-{}",
                w[0].0,
                w[0].1,
                w[1].0,
                w[1].1
            );
        }
    }
    Ok(())
}

fn content_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        return Vec::new();
    }
    content.lines().map(str::to_string).collect()
}

/// Lines at or under which the whole file comes back after an edit. Every
/// line below an edit gets a new number, so a model working from its last
/// read sends stale anchors; a fresh view of a small file costs less than
/// the failed edit and the re-read it would otherwise take.
const WHOLE_FILE_LINES: usize = 200;
/// Lines shown on each side of the edit in a bigger file.
const CONTEXT_LINES: usize = 25;

fn snippet(text: &str, around: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return "(empty)\n".into();
    }
    let (start, end) = if lines.len() <= WHOLE_FILE_LINES {
        (1, lines.len())
    } else {
        (
            around.saturating_sub(CONTEXT_LINES).max(1),
            (around + CONTEXT_LINES).min(lines.len()),
        )
    };
    let mut out = String::new();
    for n in start..=end {
        let line = lines[n - 1];
        out.push_str(&format!("{n:>6}:{}|{line}\n", line_tag(line)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn tmp() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "arbos-hashline-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn replace_by_hash() {
        let dir = tmp();
        fs::write(dir.join("a.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
        let h = line_tag("fn b() {}");
        let out = edit(
            &dir,
            &dir,
            "a.rs",
            &json!({"anchor": format!("2:{h}"), "content": "fn b() { 1 }"}),
        )
        .unwrap();
        assert!(out.body.contains("edited"));
        let text = fs::read_to_string(dir.join("a.rs")).unwrap();
        assert_eq!(text, "fn a() {}\nfn b() { 1 }\nfn c() {}\n");
    }

    #[test]
    fn stale_hash_shifts() {
        let dir = tmp();
        fs::write(dir.join("a.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
        let h = line_tag("fn b() {}");
        fs::write(
            dir.join("a.rs"),
            "fn z() {}\nfn a() {}\nfn b() {}\nfn c() {}\n",
        )
        .unwrap();
        let out = edit(
            &dir,
            &dir,
            "a.rs",
            &json!({"anchor": format!("2:{h}"), "content": "fn b() { 1 }"}),
        )
        .unwrap();
        assert!(out.body.contains("moved"));
        let text = fs::read_to_string(dir.join("a.rs")).unwrap();
        assert_eq!(text, "fn z() {}\nfn a() {}\nfn b() { 1 }\nfn c() {}\n");
    }

    #[test]
    fn stale_wrong_hash_fails() {
        let dir = tmp();
        fs::write(dir.join("a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        let err = edit(
            &dir,
            &dir,
            "a.rs",
            &json!({"anchor": "1:zzz", "content": "x"}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("not found"));
        assert_eq!(
            fs::read_to_string(dir.join("a.rs")).unwrap(),
            "fn a() {}\nfn b() {}\n"
        );
    }

    #[test]
    fn insert_and_range() {
        let dir = tmp();
        fs::write(dir.join("a.rs"), "a\nb\nc\n").unwrap();
        let ha = line_tag("a");
        let hc = line_tag("c");
        edit(
            &dir,
            &dir,
            "a.rs",
            &json!({
                "edits": [
                    {"op": "insert_after", "anchor": format!("1:{ha}"), "content": "mid"},
                    {"op": "replace", "anchor": format!("3:{hc}"), "content": ""}
                ]
            }),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.rs")).unwrap(), "a\nmid\nb\n");
    }

    /// Malformed arguments must error without touching the file.
    #[test]
    fn malformed_args_leave_the_file_alone() {
        let dir = tmp();
        let original = "a\nb\nc\n";
        fs::write(dir.join("a.rs"), original).unwrap();
        let ha = line_tag("a");
        let bad: Vec<serde_json::Value> = vec![
            json!({}),
            json!({"anchor": "", "content": "x"}),
            json!({"anchor": "notanumber:abc", "content": "x"}),
            json!({"anchor": "999:zzz", "content": "x"}),
            json!({"edits": "not a list"}),
            json!({"edits": [{"op": "explode", "anchor": format!("1:{ha}"), "content": "x"}]}),
        ];
        for args in &bad {
            let result = edit(&dir, &dir, "a.rs", args);
            assert!(result.is_err(), "expected an error for {args}");
            assert_eq!(
                fs::read_to_string(dir.join("a.rs")).unwrap(),
                original,
                "file changed by {args}"
            );
        }
        // F-041: an edit with no `content` must not be read as "delete this
        // line"; today the `edits` list form defaults content to "".
        for args in [
            json!({"anchor": format!("1:{ha}")}),
            json!({"edits": [{"anchor": format!("1:{ha}")}]}),
        ] {
            let _ = edit(&dir, &dir, "a.rs", &args);
            let now = fs::read_to_string(dir.join("a.rs")).unwrap();
            fs::write(dir.join("a.rs"), original).unwrap();
            assert_eq!(
                now, original,
                "a missing `content` changed the file for {args} (F-041)"
            );
        }
        assert!(
            edit(
                &dir,
                &dir,
                "missing.rs",
                &json!({"anchor": "1:abc", "content": "x"})
            )
            .is_err()
        );
        assert!(
            edit(
                &dir,
                &dir,
                "../outside.rs",
                &json!({"anchor": "1:abc", "content": "x"})
            )
            .is_err(),
            "paths must not escape the working directory"
        );
    }
}
