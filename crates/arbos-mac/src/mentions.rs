//! Lexical matching for normal agent handles. Collective tokens intentionally keep
//! stricter boundaries in sync::text_has_at_token; resolving IDs remains the caller's job.

pub(crate) fn text_has_at_token(lower: &str, token: &str) -> bool {
    let t = format!("@{token}");
    lower.match_indices(&t).any(|(i, _)| {
        let after = lower[i + t.len()..].chars().next();
        after.is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'))
    })
}
#[cfg(test)]
mod tests {
    use super::text_has_at_token;

    #[test]
    fn agent_boundaries_preserve_existing_addressing() {
        for (text, expected) in [
            ("", false),
            ("@agent", true),
            ("@agents", false),
            ("@agent-other", false),
            ("@agent_other", false),
            ("@agent.com", true),
            ("mail x@agent.com", true),
            ("@@agent", true),
            ("@agentx @agent", true),
        ] {
            assert_eq!(text_has_at_token(text, "agent"), expected, "{text}");
        }
    }

    #[test]
    fn unicode_word_characters_and_punctuation_keep_their_meaning() {
        assert!(!text_has_at_token("@agenté", "agent"));
        assert!(text_has_at_token("@agent🙂", "agent"));
        assert!(text_has_at_token("@équipe!", "équipe"));
    }

    #[test]
    fn tokens_are_literal_and_callers_supply_lower_case() {
        assert!(text_has_at_token("@a.b", "a.b"));
        assert!(!text_has_at_token("@axb", "a.b"));
        assert!(!text_has_at_token("@Agent", "agent"));
    }
}
