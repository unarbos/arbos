//! Small text helpers shared by the kernel's records.

/// How much of a command's output rides into a one-line outcome.
pub const TAIL_LIMIT: usize = 1_200;

/// The first line of `s`, at most `n` chars, with an ellipsis when cut.
pub fn clip(s: &str, n: usize) -> String {
    let first = s.lines().next().unwrap_or("");
    let more = s.lines().nth(1).is_some();
    let mut out: String = first.chars().take(n).collect();
    if first.chars().count() > n || more {
        out.push('…');
    }
    out
}

/// The tail of a command's output that rides into an outcome.
pub fn tail(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= TAIL_LIMIT {
        return s.to_string();
    }
    let skip = s.chars().count() - TAIL_LIMIT;
    format!("…{}", s.chars().skip(skip).collect::<String>())
}
