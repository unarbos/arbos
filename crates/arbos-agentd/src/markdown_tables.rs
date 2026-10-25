//! Outgoing bubble text is plain GitHub-flavoured markdown: the Mac and iPhone renderers draw a
//! table only when it is a header row, a `| --- |` separator row and body rows, every row
//! starting and ending with `|`, at column one, outside any code fence. `prompts/router.md`
//! says so; this is the backstop for the turns where the model still wraps a table in a fence,
//! indents it, or forgets the separator row ([feedback markdown-tables-agentd-2026-10-25]:
//! Jacob's Lima bookstore table). Prose, lists and real code fences pass through byte for byte.

/// Normalise the tables in one `send_message` text (see the module doc). Idempotent.
pub fn normalize_tables(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 2);
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(lang) = fence_open(line) {
            // A fence whose whole body is a table is the table (the fence is dropped); any
            // other fence is code and stays as it is, including an unterminated one.
            let close = (i + 1..lines.len()).find(|&j| is_fence(lines[j]));
            if let Some(close) = close {
                let body = &lines[i + 1..close];
                if fence_hides_a_table(lang, body) {
                    push_table(body, &mut out);
                    i = close + 1;
                    continue;
                }
            }
            let end = close.unwrap_or(lines.len() - 1);
            for l in &lines[i..=end] {
                out.push((*l).to_owned());
            }
            i = end + 1;
            continue;
        }
        if is_pipe_row(line) {
            let end = (i..lines.len())
                .take_while(|&j| is_pipe_row(lines[j]))
                .last()
                .unwrap_or(i);
            let run = &lines[i..=end];
            if run.len() >= 2 {
                push_table(run, &mut out);
            } else {
                out.push(line.to_owned());
            }
            i = end + 1;
            continue;
        }
        out.push(line.to_owned());
        i += 1;
    }
    let mut joined = out.join("\n");
    joined = unwrap_angle_links(&joined);
    joined
}

/// The rows of one table, trimmed to column one, with a separator row after the header when
/// the model left it out.
fn push_table(rows: &[&str], out: &mut Vec<String>) {
    let rows: Vec<&str> = rows
        .iter()
        .map(|r| r.trim())
        .filter(|r| !r.is_empty())
        .collect();
    if rows.is_empty() {
        return;
    }
    out.push(rows[0].to_owned());
    if rows.len() == 1 || !is_separator_row(rows[1]) {
        let cols = cell_count(rows[0]).max(1);
        out.push(format!("|{}", " --- |".repeat(cols)));
    }
    for r in &rows[1..] {
        out.push((*r).to_owned());
    }
}

fn fence_open(line: &str) -> Option<&str> {
    line.trim_start()
        .strip_prefix("```")
        .map(|rest| rest.trim())
}

fn is_fence(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

/// A fence that only holds pipe rows is a table the model fenced by mistake. `json`, `bash`
/// and the other real languages keep their fence even then.
fn fence_hides_a_table(lang: &str, body: &[&str]) -> bool {
    let lang = lang.to_ascii_lowercase();
    let table_ish = lang.is_empty()
        || matches!(
            lang.as_str(),
            "markdown" | "md" | "text" | "txt" | "plain" | "plaintext" | "table" | "gfm"
        );
    let rows: Vec<&&str> = body.iter().filter(|l| !l.trim().is_empty()).collect();
    table_ish && rows.len() >= 2 && rows.iter().all(|l| is_pipe_row(l))
}

fn is_pipe_row(line: &str) -> bool {
    let t = line.trim();
    t.len() > 2 && t.starts_with('|') && t.ends_with('|')
}

fn is_separator_row(line: &str) -> bool {
    let t = line.trim();
    is_pipe_row(t)
        && t.contains('-')
        && t.chars()
            .all(|c| c == '|' || c == '-' || c == ':' || c == ' ')
}

fn cell_count(row: &str) -> usize {
    let t = row.trim();
    let inner = &t[1..t.len() - 1];
    // A `\|` is an escaped pipe inside a cell, not a cell boundary.
    inner.replace("\\|", "").split('|').count()
}

/// `<https://…>` autolinks become the bare URL (the apps link bare URLs; the angle form shows
/// its brackets).
fn unwrap_angle_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<http") {
        let after = &rest[start..];
        match after.find('>') {
            Some(end) if !after[..end].contains(char::is_whitespace) => {
                out.push_str(&rest[..start]);
                out.push_str(&after[1..end]);
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str(&rest[..start + 1]);
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table the model wrote for "Find me a book store in lima" (Bubble, 2026-10-25
    /// 21:31 UTC): already plain GFM, so it must come out byte for byte.
    const LIMA: &str = "| Store | Area | Sunday | What they sell |\n| --- | --- | --- | --- |\n| [Librería El Virrey](https://www.elvirrey.com/) | Miraflores, Calle Bolognesi 510 | 11am–7pm | Spanish literature, essays, history, art; some English at a higher price |\n| [Communitas](https://www.communitas.pe/) | San Isidro, Av. 2 de Mayo 1690 | 10:30am–7:30pm | Humanities, literature, law, history, science, in Spanish |\n| [Vallejo Librería Café](https://www.tangol.com/blog/en/the-3-best-bookstores-in-lima/n-15210) | San Isidro, Av. Camino Real 1119 | not listed | Books plus coffee or tea |\n| [Assouline Lima](https://www.assouline.com/pages/store/lima) | Miraflores, street not listed | not listed | Luxury and design books |";

    #[test]
    fn a_plain_gfm_table_is_untouched() {
        assert_eq!(normalize_tables(LIMA), LIMA);
        let with_prose = format!("Top pick first.\n\n{LIMA}\n\nHours were not checked twice.");
        assert_eq!(normalize_tables(&with_prose), with_prose);
    }

    #[test]
    fn a_fenced_table_loses_its_fence() {
        for lang in ["", "markdown", "md", "text"] {
            let fenced = format!("Here:\n\n```{lang}\n{LIMA}\n```\n\nMore.");
            assert_eq!(
                normalize_tables(&fenced),
                format!("Here:\n\n{LIMA}\n\nMore."),
                "lang {lang:?}"
            );
        }
    }

    #[test]
    fn an_indented_table_moves_to_column_one() {
        let indented = LIMA
            .lines()
            .map(|l| format!("    {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(normalize_tables(&indented), LIMA);
        let two_spaces = LIMA
            .lines()
            .map(|l| format!("  {l}  "))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(normalize_tables(&two_spaces), LIMA);
    }

    #[test]
    fn a_table_without_a_separator_row_gets_one() {
        let mut lines: Vec<&str> = LIMA.lines().collect();
        lines.remove(1);
        let headless = lines.join("\n");
        assert_eq!(normalize_tables(&headless), LIMA);
    }

    #[test]
    fn a_fenced_table_without_a_separator_row_is_both_fixed() {
        let mut lines: Vec<&str> = LIMA.lines().collect();
        lines.remove(1);
        let fenced = format!("```\n{}\n```", lines.join("\n"));
        assert_eq!(normalize_tables(&fenced), LIMA);
    }

    #[test]
    fn code_fences_lists_and_prose_pass_through() {
        let code = "Run it:\n\n```bash\necho '| not | a table |'\necho '| --- | --- |'\n```\n\n- a | b\n- c\n\nA line with | one pipe in prose.";
        assert_eq!(normalize_tables(code), code);
        let json = "```json\n{\"a\": 1}\n```";
        assert_eq!(normalize_tables(json), json);
        let unterminated = "```\n| a | b |\n| c | d |";
        assert_eq!(normalize_tables(unterminated), unterminated);
        let one_row = "| just | one |\n\nthen prose";
        assert_eq!(normalize_tables(one_row), one_row);
    }

    #[test]
    fn angle_autolinks_become_bare_urls() {
        assert_eq!(
            normalize_tables("See <https://www.elvirrey.com/> today, and a < b > c."),
            "See https://www.elvirrey.com/ today, and a < b > c."
        );
        assert_eq!(normalize_tables("<http://x.y/a b>"), "<http://x.y/a b>");
    }

    #[test]
    fn escaped_pipes_do_not_count_as_columns() {
        let t = "| a \\| b | c |\n| 1 | 2 |";
        assert_eq!(
            normalize_tables(t),
            "| a \\| b | c |\n| --- | --- |\n| 1 | 2 |"
        );
    }

    #[test]
    fn normalising_twice_changes_nothing() {
        let fenced = format!("```\n  {}\n```", LIMA.replace('\n', "\n  "));
        let once = normalize_tables(&fenced);
        assert_eq!(normalize_tables(&once), once);
    }
}
