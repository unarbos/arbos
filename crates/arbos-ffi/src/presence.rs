//! The shared typing line for Swift (`arbos_core::presence`): the iPhone's
//! presence row says who is typing in the exact words the Mac says it in
//! ([feedback group-typing-names-2026-10-30], Scout's wording, Jacob's call on
//! 2026-10-30).
//!
//! The sentence used to be written twice — once in Rust for the Mac and once in
//! Swift in `PresenceLine` — and the two drifted apart. There is one copy now,
//! in `arbos-core`, and this is how Swift reaches it, the same way every iPhone
//! search box reaches the shared matcher through [`crate::search`].

use arbos_core::presence;

/// The typing line for `names` (display names, in the order to read them), or
/// `nil` when nobody is typing.
///
/// One person keeps the whole display name; two shorten to first names; from
/// three on it counts them instead of listing them, so the line never grows
/// with the room. The caller's order is the reading order: this never sorts.
#[uniffi::export]
pub fn presence_typing_line(names: Vec<String>) -> Option<String> {
    presence::typing_line(&names)
}

/// "Sam Okafor" → "Sam"; "@sam" and one-word names stay as they are.
/// Idempotent, so a caller that already shortened a name may pass it through.
#[uniffi::export]
pub fn presence_first_name(name: String) -> String {
    presence::first_name(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(names: &[&str]) -> Option<String> {
        presence_typing_line(names.iter().map(|s| (*s).to_owned()).collect())
    }

    /// Scout's three forms, over the FFI, in the words the Mac draws.
    #[test]
    fn one_name_two_names_then_a_count_through_the_ffi() {
        assert_eq!(line(&[]), None);
        assert_eq!(
            line(&["Sam Okafor"]).as_deref(),
            Some("Sam Okafor is typing")
        );
        assert_eq!(
            line(&["Mira Kaplan", "Sam Okafor"]).as_deref(),
            Some("Mira and Sam are typing")
        );
        assert_eq!(
            line(&["Sam", "Mira", "Kai", "Ava"]).as_deref(),
            Some("4 people are typing")
        );
        assert_eq!(presence_first_name("Sam Okafor".into()), "Sam");
        assert_eq!(presence_first_name("@sam".into()), "@sam");
    }
}
