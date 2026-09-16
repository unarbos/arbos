//! Saying the same thing twice. The first guard (#236) compared bytes;
//! the model reworded — "may still be active but currently unresponsive"
//! against "may still be active but is currently unresponsive", "if
//! needed" against "if necessary" — and the reader saw the same paragraph
//! twice (mobile cycle 5). Now two replies are the same when their words,
//! folded and stripped of punctuation, overlap almost entirely.

/// Replies shorter than this are never a repeat: a status word, a short
/// answer, "done" said twice.
pub const MIN_CHARS: usize = 40;
/// The word overlap (Dice coefficient over word multisets) at and above
/// which two replies are one reply said twice.
pub const THRESHOLD: f64 = 0.9;

/// The comparison form: lower-case words with punctuation removed, in
/// order. None for a reply too short to count.
pub fn words(text: &str) -> Option<Vec<String>> {
    let folded: String = text
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                ' '
            }
        })
        .collect();
    let out: Vec<String> = folded.split_whitespace().map(str::to_string).collect();
    (out.iter().map(|w| w.len() + 1).sum::<usize>() >= MIN_CHARS).then_some(out)
}

/// How much of two word lists is shared: 2·|A∩B| / (|A|+|B|) over
/// multisets. 1.0 for the same words in any order; 0.0 for none shared.
pub fn overlap(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut counts: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    for w in a {
        *counts.entry(w.as_str()).or_insert(0) += 1;
    }
    let mut shared = 0i64;
    for w in b {
        if let Some(n) = counts.get_mut(w.as_str())
            && *n > 0
        {
            *n -= 1;
            shared += 1;
        }
    }
    2.0 * shared as f64 / (a.len() + b.len()) as f64
}

/// Whether `text` says what `earlier` said: long enough to count, and
/// the words overlap at `THRESHOLD` or above.
pub fn near_duplicate(earlier: &[String], text: &str) -> bool {
    match words(text) {
        Some(now) => overlap(earlier, &now) >= THRESHOLD,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(a: &str, b: &str) -> bool {
        near_duplicate(&words(a).unwrap(), b)
    }

    /// The pair from `media/mobile/cycle-5/03-link-cut-pending-card.png`.
    #[test]
    fn a_reworded_paragraph_is_the_same_paragraph() {
        let a = "The connection to the sub-agent \"sayonesentenceaboutsprin\" on arboslife has been lost, and the remote kernel may still be active but currently unresponsive here. To continue, you may need to restart this kernel to re-attach or respawn the sub-agent if needed. Let me know how you'd like to proceed.";
        let b = "The connection to the sub-agent \"sayonesentenceaboutsprin\" on arboslife has been lost, and the remote kernel may still be active but is currently unresponsive here. To continue, you may need to restart this kernel to re-attach or respawn the sub-agent if necessary. Let me know how you'd like to proceed.";
        assert!(same(a, b));
        assert!(same(a, a));
        // Case and punctuation do not matter.
        assert!(same(a, &a.to_uppercase().replace(',', " —")));
    }

    #[test]
    fn different_content_and_short_phrases_are_not_repeats() {
        let a = "Please provide the sentences from the sub-agents so I can combine them once both are available.";
        let b = "Both sentences are in: the first says the sky is blue, the second that the grass is green. Combined below.";
        assert!(!same(a, b));
        // A short phrase said twice is not the loop this catches.
        assert!(words("Working on it.").is_none());
        assert!(words("Done: two tests fixed.").is_none());
        // Same opening, different second half: a progress report, not a repeat.
        let c = "The connection to the sub-agent on arboslife has been lost. I have restarted the kernel and re-attached; the worker is back and its report is below.";
        assert!(!same(a, c));
        let d = "The connection to the sub-agent \"sayonesentenceaboutsprin\" on arboslife has been lost, and the remote kernel may still be active but currently unresponsive here. To continue, you may need to restart this kernel to re-attach or respawn the sub-agent if needed. Let me know how you'd like to proceed.";
        assert!(
            !same(d, c),
            "{}",
            overlap(&words(d).unwrap(), &words(c).unwrap())
        );
    }
}
