//! Per-result eviction: what a fresh tool result looks like to the model.
//!
//! The full body is on disk. The model sees a bounded slice plus a cite.
//! Which end survives depends on the tool: a file read or a search wants
//! its head (the model asked for the start); a shell command wants its tail
//! (the exit and the last error are at the end). Older results shrink
//! further at compaction time; see `compact`.

/// Shell output and the like. Was 64 lines / 8 KB: a `cat` or a test run
/// lost its head, and models that read files through the shell spent
/// turns probing whether stdout worked. The per-step cap in `project`
/// still bounds what a batch of commands can add.
pub const EVICT_BYTES: usize = 24 * 1024;
pub const EVICT_LINES: usize = 200;
/// File reads and searches: the model asked to see this. At 64 lines a
/// 440-line test file took seven round trips to read; every one of them
/// was a model call. A whole source file in one result is cheaper than
/// paging, so the head budget is sized for files, not for logs.
pub const READ_BYTES: usize = 48 * 1024;
pub const READ_LINES: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    Head,
    Tail,
}

/// Which end of a tool's output matters.
pub fn keep_for(tool: &str) -> Keep {
    match tool {
        "bash" | "await" | "jobs" | "changes" | "undo" => Keep::Tail,
        _ => Keep::Head,
    }
}

/// Bounded view of a tool body: the head or tail, whole lines, plus a cite
/// to the full text.
pub fn evict_tool_body(tool: &str, full: &str, cite: &str) -> String {
    match keep_for(tool) {
        Keep::Head => evict_head_to(full, cite, READ_BYTES, READ_LINES),
        Keep::Tail => evict_body(full, cite),
    }
}

/// Persist the full body; the model sees a short tail plus a cite.
pub fn evict_body(full: &str, cite: &str) -> String {
    evict_tail_to(full, cite, EVICT_BYTES, EVICT_LINES)
}

pub fn evict_tail_to(full: &str, cite: &str, max_bytes: usize, max_lines: usize) -> String {
    if fits(full, max_bytes, max_lines) {
        return full.to_string();
    }
    let lines: Vec<&str> = full.lines().collect();
    let mut tail = lines[lines.len().saturating_sub(max_lines)..].join("\n");
    if tail.len() > max_bytes {
        let cut = ceil_char_boundary(&tail, tail.len() - max_bytes);
        tail = tail[cut..].to_string();
    }
    format!(
        "…evicted ({cite}; {} bytes, {} lines)\n{tail}",
        full.len(),
        lines.len()
    )
}

/// Persist the full body; the model sees a short head plus a cite.
pub fn evict_head(full: &str, cite: &str) -> String {
    evict_head_to(full, cite, EVICT_BYTES, EVICT_LINES)
}

pub fn evict_head_to(full: &str, cite: &str, max_bytes: usize, max_lines: usize) -> String {
    if fits(full, max_bytes, max_lines) {
        return full.to_string();
    }
    let lines: Vec<&str> = full.lines().collect();
    let mut head = lines[..lines.len().min(max_lines)].join("\n");
    head.truncate(floor_char_boundary(&head, max_bytes));
    let shown = head.lines().count();
    format!(
        "{head}\n…evicted ({cite}; {} bytes, {} lines; first {shown} shown; read with offset to continue)",
        full.len(),
        lines.len()
    )
}

fn fits(full: &str, max_bytes: usize, max_lines: usize) -> bool {
    full.len() <= max_bytes && full.lines().count() <= max_lines
}

/// Largest char boundary `<= i` (std's is unstable).
pub fn floor_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Smallest char boundary `>= i`.
pub fn ceil_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

pub fn estimate_tokens(text: &str) -> u64 {
    (text.len() as u64 / 4).max(1)
}
