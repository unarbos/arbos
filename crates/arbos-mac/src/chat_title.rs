pub(crate) fn title_from(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(34).collect();
    if line.chars().count() > 34 {
        out.push('…');
    }
    if out.is_empty() {
        "New chat".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::title_from;

    #[test]
    fn empty_first_line_keeps_the_default_title() {
        for text in [
            "",
            " ",
            "\nLater text",
            "\r\nLater text",
            "\u{2003}\t\nLater text",
        ] {
            assert_eq!(title_from(text), "New chat", "{text:?}");
        }
    }

    #[test]
    fn trims_only_the_first_line() {
        assert_eq!(
            title_from("\t  First title  \r\nSecond line"),
            "First title"
        );
        assert_eq!(title_from("\u{2003}Title\u{2003}"), "Title");
    }

    #[test]
    fn preserves_interior_whitespace() {
        assert_eq!(title_from("one   two\tthree"), "one   two\tthree");
    }

    #[test]
    fn thirty_four_characters_fit_without_an_ellipsis() {
        assert_eq!(title_from(&"a".repeat(34)), "a".repeat(34));
    }

    #[test]
    fn thirty_five_characters_are_truncated_with_an_ellipsis() {
        assert_eq!(title_from(&"a".repeat(35)), format!("{}…", "a".repeat(34)));
    }

    #[test]
    fn truncation_counts_unicode_characters_instead_of_bytes() {
        assert_eq!(title_from(&"🌳".repeat(34)), "🌳".repeat(34));
        assert_eq!(
            title_from(&"🌳".repeat(35)),
            format!("{}…", "🌳".repeat(34))
        );
    }

    #[test]
    fn combining_marks_count_separately() {
        assert_eq!(
            title_from(&"e\u{301}".repeat(18)),
            format!("{}…", "e\u{301}".repeat(17))
        );
    }

    #[test]
    fn a_bare_carriage_return_does_not_start_a_new_line() {
        assert_eq!(title_from("First\rSecond"), "First\rSecond");
    }
}
