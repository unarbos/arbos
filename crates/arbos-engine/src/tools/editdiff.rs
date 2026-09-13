//! Line-numbered display diff. Same rows as the Go kernel's `Details.diff`
//! and the web DiffCard: `+NN text` / `-NN text` / ` NN text`.

const MAX_CELLS: usize = 16_000_000;
const CONTEXT: usize = 3;

#[derive(Clone, Copy)]
enum Kind {
    Keep,
    Del,
    Add,
}

struct Op<'a> {
    kind: Kind,
    text: &'a str,
}

/// Numbered hunks for the transcript card. Empty when the files match
/// or the file is too large to diff in memory.
pub fn numbered_diff(old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len()
        .saturating_add(1)
        .saturating_mul(b.len().saturating_add(1))
        > MAX_CELLS
    {
        return String::new();
    }
    render(&line_ops(&a, &b), CONTEXT)
}

fn line_ops<'a>(a: &'a [&str], b: &'a [&str]) -> Vec<Op<'a>> {
    let n = a.len();
    let m = b.len();
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[at(i, j)] = if a[i] == b[j] {
                dp[at(i + 1, j + 1)] + 1
            } else {
                dp[at(i + 1, j)].max(dp[at(i, j + 1)])
            };
        }
    }
    let mut ops = Vec::new();
    let mut i = 0;
    let mut j = 0;
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push(Op {
                kind: Kind::Keep,
                text: a[i],
            });
            i += 1;
            j += 1;
        } else if dp[at(i + 1, j)] >= dp[at(i, j + 1)] {
            ops.push(Op {
                kind: Kind::Del,
                text: a[i],
            });
            i += 1;
        } else {
            ops.push(Op {
                kind: Kind::Add,
                text: b[j],
            });
            j += 1;
        }
    }
    while i < n {
        ops.push(Op {
            kind: Kind::Del,
            text: a[i],
        });
        i += 1;
    }
    while j < m {
        ops.push(Op {
            kind: Kind::Add,
            text: b[j],
        });
        j += 1;
    }
    ops
}

fn render(ops: &[Op<'_>], context: usize) -> String {
    let mut keep = vec![false; ops.len()];
    for (idx, op) in ops.iter().enumerate() {
        if matches!(op.kind, Kind::Keep) {
            continue;
        }
        let lo = idx.saturating_sub(context);
        let hi = (idx + context + 1).min(ops.len());
        for slot in keep.iter_mut().take(hi).skip(lo) {
            *slot = true;
        }
    }
    let last = ops.len().max(1);
    let width = last.to_string().len().max(1);
    let gap = format!("{}...", " ".repeat(width + 1));
    let mut rows = Vec::new();
    let mut skipping = false;
    let mut old_num = 0u32;
    let mut new_num = 0u32;
    for (idx, op) in ops.iter().enumerate() {
        match op.kind {
            Kind::Keep => {
                old_num += 1;
                new_num += 1;
            }
            Kind::Del => old_num += 1,
            Kind::Add => new_num += 1,
        }
        if !keep[idx] {
            if !skipping {
                rows.push(gap.clone());
                skipping = true;
            }
            continue;
        }
        skipping = false;
        let (sign, num) = match op.kind {
            Kind::Keep => (' ', old_num),
            Kind::Del => ('-', old_num),
            Kind::Add => ('+', new_num),
        };
        rows.push(format!("{sign}{num:>width$} {text}", text = op.text));
    }
    while rows.first().is_some_and(|row| row == &gap) {
        rows.remove(0);
    }
    while rows.last().is_some_and(|row| row == &gap) {
        rows.pop();
    }
    rows.join("\n")
}
