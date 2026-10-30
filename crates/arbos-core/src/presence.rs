//! The words every surface uses for "who is typing": the chat-list row and the
//! row over the composer, on the Mac and on the iPhone.
//!
//! Scout reported that a group chat's row said only "typing", naming nobody,
//! and Jacob's call on 2026-10-30 was Scout's own wording on both platforms
//! ([feedback group-typing-names-2026-10-30]):
//!
//! | Typing | The line |
//! |---|---|
//! | 1 | `Sam Okafor is typing` |
//! | 2 | `Mira and Sam are typing` |
//! | 3 or more | `3 people are typing` |
//!
//! It lives here because the sentence was built twice before — once in Rust for
//! the Mac and once in Swift for the iPhone — and the two drifted. The iPhone
//! reaches it through `arbos-ffi` (`presence_typing_line`), the same way every
//! iPhone search box reaches the shared matcher in [`crate::search`].
//!
//! Two things this deliberately does **not** decide, because they belong to the
//! surface and not to the sentence:
//!
//! * **Order.** The caller passes the names in the order it wants them read.
//!   The chat-list row sorts them, so a typist's 3-second ping cannot swap the
//!   words; the row over the composer keeps its newest-first order, because its
//!   words sit beside a stack of faces in that same order and the two must
//!   agree.
//! * **Who else is on the line.** A row that also has agents working joins this
//!   clause to its own with " · ".

/// "Sam Okafor" → "Sam"; "@sam" and one-word names stay as they are.
///
/// Idempotent, so a caller that has already shortened a name for its own
/// reasons can pass it straight through.
pub fn first_name(name: &str) -> String {
    name.split_whitespace().next().unwrap_or(name).to_string()
}

/// The typing line for `names` (display names, in the order to read them), or
/// `None` when nobody is typing.
///
/// One person keeps the whole display name. Two shorten to first names, so the
/// line still fits on one line. From three on it counts them instead of listing
/// them, so the line never grows with the room.
pub fn typing_line(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [one] => Some(format!("{one} is typing")),
        [a, b] => Some(format!(
            "{} and {} are typing",
            first_name(a),
            first_name(b)
        )),
        many => Some(format!("{} people are typing", many.len())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(names: &[&str]) -> Option<String> {
        typing_line(&names.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    }

    /// Scout's three forms, which both platforms now read from here.
    #[test]
    fn one_name_two_names_then_a_count() {
        assert_eq!(line(&[]), None, "nobody typing: the surface says nothing");
        assert_eq!(
            line(&["Sam Okafor"]).as_deref(),
            Some("Sam Okafor is typing")
        );
        assert_eq!(
            line(&["Mira Kaplan", "Sam Okafor"]).as_deref(),
            Some("Mira and Sam are typing")
        );
        assert_eq!(
            line(&["Sam", "Mira", "Kai"]).as_deref(),
            Some("3 people are typing")
        );
        assert_eq!(
            line(&["Sam", "Mira", "Kai", "Ava"]).as_deref(),
            Some("4 people are typing")
        );
    }

    /// No ellipsis and no trailing dots: Scout's wording is a plain sentence,
    /// and from three on it names nobody, so the line's length is bounded
    /// however big the room is.
    #[test]
    fn the_line_is_plain_and_bounded() {
        for names in [
            vec!["Sam Okafor"],
            vec!["Mira Kaplan", "Sam Okafor"],
            vec!["Sam", "Mira", "Kai"],
        ] {
            let text = line(&names).expect("a line");
            assert!(!text.contains('…'), "no ellipsis character: {text}");
            assert!(!text.ends_with('.'), "no trailing dots: {text}");
        }
        let crowd: Vec<&str> = std::iter::repeat_n("Somebody With A Long Name", 40).collect();
        assert_eq!(line(&crowd).as_deref(), Some("40 people are typing"));
    }

    /// The caller's order is the reading order: this never sorts.
    #[test]
    fn the_callers_order_is_kept() {
        assert_eq!(
            line(&["Sam Okafor", "Mira Kaplan"]).as_deref(),
            Some("Sam and Mira are typing")
        );
        assert_eq!(
            line(&["Mira Kaplan", "Sam Okafor"]).as_deref(),
            Some("Mira and Sam are typing")
        );
    }

    #[test]
    fn first_name_shortens_once_and_leaves_handles_alone() {
        assert_eq!(first_name("Sam Okafor"), "Sam");
        assert_eq!(first_name("Sam"), "Sam");
        assert_eq!(first_name("@sam"), "@sam");
        assert_eq!(first_name(""), "");
        assert_eq!(
            first_name(&first_name("Sam Okafor")),
            "Sam",
            "idempotent: a caller may shorten first"
        );
    }
}
