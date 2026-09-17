//! What a brief asks a worker to do to the world. A read-only worker
//! (`kind: explore`, `readonly: true`) cannot write, so a brief that
//! names files to produce or work that changes code is the wrong pairing
//! — and the guard that only looked at the `Output:` line taught the
//! model to drop the line and keep the kind (symmetry cycle 14, F-56:
//! seven read-only workers, zero files).

/// Verbs, at the head of a task or step line, that change something.
const WRITE_VERBS: &[&str] = &[
    "create",
    "write",
    "implement",
    "add",
    "rename",
    "fix",
    "refactor",
    "edit",
    "update",
    "change",
    "build",
    "install",
    "remove",
    "delete",
    "move",
    "migrate",
    "convert",
    "generate",
    "scaffold",
    "set up",
    "setup",
    "configure",
    "wire",
    "commit",
    "push",
    "open a pr",
    "open a pull request",
    "make",
    "introduce",
    "replace",
    "extract",
    "split",
    "merge",
    "bump",
    "upgrade",
    "record",
    "capture",
    "screenshot",
    "save",
];

/// The reasons a brief needs a worker that can write, in words for a
/// refusal or a note. Empty for a read-only ask ("what does main.py do",
/// "which tests exist", "summarise the design").
pub fn write_reasons(brief: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let outputs = crate::brief_output_paths(brief);
    if !outputs.is_empty() {
        out.push(format!("Output: {}", outputs.join(", ")));
    }
    if brief.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("Show:") || t.starts_with("Show ")
    }) {
        out.push("a Show line (an image must be written)".into());
    }
    // Task / Do lines, and numbered steps: the first words say what.
    let mut verbs: Vec<String> = Vec::new();
    let mut commits = false;
    for line in brief.lines() {
        let t = line.trim();
        let body = t
            .strip_prefix("Task:")
            .or_else(|| t.strip_prefix("Do:"))
            .or_else(|| {
                // "1. Create …", "- Write …", "2) Fix …"
                let mut chars = t.chars();
                let head: String = chars.by_ref().take(3).collect();
                let stripped = head.trim_start_matches(|c: char| {
                    c.is_ascii_digit() || matches!(c, '.' | ')' | '-' | '*')
                });
                if stripped.len() < head.len() {
                    Some(&t[head.len() - stripped.len()..])
                } else {
                    None
                }
            })
            .map(str::trim)
            .filter(|b| !b.is_empty());
        let Some(body) = body else { continue };
        let lower = body.to_lowercase();
        // Only the task's own lines: the template's Rules line mentions
        // commits and pull requests on every brief.
        if lower.contains("commit") || lower.contains("pull request") || lower.contains("pr create")
        {
            commits = true;
        }
        for v in WRITE_VERBS {
            if lower.starts_with(v)
                && lower[v.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric())
            {
                let shown: String = body
                    .split_whitespace()
                    .take(5)
                    .collect::<Vec<_>>()
                    .join(" ");
                verbs.push(format!("\"{shown}\""));
                break;
            }
        }
    }
    verbs.dedup();
    if !verbs.is_empty() {
        verbs.truncate(3);
        out.push(format!("steps that change files ({})", verbs.join(", ")));
    }
    if commits {
        out.push("a commit or a pull request".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::write_reasons;

    #[test]
    fn briefs_that_change_things_are_seen_and_questions_are_not() {
        let writes = "Task: Create a CLI that tallies lines per file\nDo:\n1. Write tally/cli.py with argparse\n2. Add a test\n3. Commit on a branch and open a pull request\nRules: a fix goes on its own branch and a PR via pr create\nOutput: .arbos/docs/tally.md\nReport: paths";
        let reasons = write_reasons(writes);
        assert!(
            reasons
                .iter()
                .any(|r| r.starts_with("Output: .arbos/docs/tally.md")),
            "{reasons:?}"
        );
        assert!(
            reasons
                .iter()
                .any(|r| r.contains("\"Create a CLI that tallies")),
            "{reasons:?}"
        );
        assert!(
            reasons
                .iter()
                .any(|r| r.contains("a commit or a pull request")),
            "{reasons:?}"
        );
        // The dropped-Output shape from cycle 14 is still a writer.
        let no_output = "Task: Add type hints to every function in tally/\nDo: as the task says\nReport: what changed";
        let reasons = write_reasons(no_output);
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert!(
            reasons[0].contains("\"Add type hints to every"),
            "{reasons:?}"
        );
        // The template's Rules line is on every brief; alone it is nothing.
        let templated = "Task: Which tests exist?\nRead first: .arbos/docs/project-context.md\nDo: as the task says\nRules: a code fix goes on its own branch, committed and pushed there, and comes back as a draft pull request (`pr create`); never merge.\nOutput: Deliverables under .arbos/docs/, captures under .arbos/media/<topic>/.\nReport: a list";
        assert!(
            write_reasons(templated).is_empty(),
            "{:?}",
            write_reasons(templated)
        );
        for q in [
            "Task: What does main.py do? Summarise in five lines.\nReport: the summary",
            "Task: Which tests exist and what do they cover?\nDo: read tests/, list them\nReport: a list",
            "Task: Explain the additive model in docs/design.md",
        ] {
            assert!(write_reasons(q).is_empty(), "{q}: {:?}", write_reasons(q));
        }
        // "Add" inside a sentence is not the verb of the step.
        assert!(
            write_reasons("Task: Say whether we should add a cache\nReport: yes or no").is_empty()
        );
    }
}
