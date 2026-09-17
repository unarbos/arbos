//! What the app draws. Every view here reads
//! [`crate::model::workspace::Workspace`] and writes to it by name; none of
//! them owns app state.

pub mod chips;
pub mod component;
pub mod detail;
pub mod drawer;
#[cfg(target_os = "macos")]
mod fn_key;
pub mod menubar;
pub mod naming;
pub mod palette;
pub mod panel;
pub mod project_page;
pub mod root;
pub mod settings;
pub mod status_bar;
pub mod tabs;
pub mod terminal;

use bezel::{
    gpui::{App, KeyBinding},
    ui::input,
};

/// The chords a native field answers to, claimed on an extra context so a
/// composer or a rename box still gets them. `enter` / `up` / `down` stay
/// with the host — those are send, pick, and dismiss, not caret motion.
pub(crate) fn bind_field_editing(cx: &mut App, context: &'static str, multiline: bool) {
    cx.bind_keys(field_editing_bindings(context, multiline));
}

fn field_editing_bindings(context: &'static str, multiline: bool) -> Vec<KeyBinding> {
    let ctx = Some(context);
    let mut bindings = vec![
        KeyBinding::new("backspace", input::Backspace, ctx),
        KeyBinding::new("delete", input::Delete, ctx),
        KeyBinding::new("left", input::Left, ctx),
        KeyBinding::new("right", input::Right, ctx),
        KeyBinding::new("shift-left", input::SelectLeft, ctx),
        KeyBinding::new("shift-right", input::SelectRight, ctx),
        KeyBinding::new("home", input::Home, ctx),
        KeyBinding::new("end", input::End, ctx),
        KeyBinding::new("shift-home", input::SelectHome, ctx),
        KeyBinding::new("shift-end", input::SelectEnd, ctx),
    ];
    if multiline {
        bindings.extend([
            KeyBinding::new("shift-up", input::SelectUp, ctx),
            KeyBinding::new("shift-down", input::SelectDown, ctx),
        ]);
    } else {
        bindings.extend([
            KeyBinding::new("shift-up", input::SelectHome, ctx),
            KeyBinding::new("shift-down", input::SelectEnd, ctx),
        ]);
    }
    #[cfg(target_os = "macos")]
    bindings.extend([
        KeyBinding::new("cmd-a", input::SelectAll, ctx),
        KeyBinding::new("cmd-c", input::Copy, ctx),
        KeyBinding::new("cmd-x", input::Cut, ctx),
        KeyBinding::new("cmd-v", input::Paste, ctx),
        KeyBinding::new("cmd-z", input::Undo, ctx),
        KeyBinding::new("cmd-shift-z", input::Redo, ctx),
        KeyBinding::new("ctrl-cmd-space", input::ShowCharacterPalette, ctx),
        KeyBinding::new("cmd-left", input::Home, ctx),
        KeyBinding::new("cmd-right", input::End, ctx),
        KeyBinding::new("cmd-shift-left", input::SelectHome, ctx),
        KeyBinding::new("cmd-shift-right", input::SelectEnd, ctx),
        KeyBinding::new("alt-left", input::WordLeft, ctx),
        KeyBinding::new("alt-right", input::WordRight, ctx),
        KeyBinding::new("alt-shift-left", input::SelectWordLeft, ctx),
        KeyBinding::new("alt-shift-right", input::SelectWordRight, ctx),
        KeyBinding::new("cmd-backspace", input::DeleteToLineStart, ctx),
        KeyBinding::new("alt-backspace", input::DeleteWordLeft, ctx),
        KeyBinding::new("alt-delete", input::DeleteWordRight, ctx),
        KeyBinding::new("ctrl-a", input::Home, ctx),
        KeyBinding::new("ctrl-e", input::End, ctx),
        KeyBinding::new("ctrl-b", input::Left, ctx),
        KeyBinding::new("ctrl-f", input::Right, ctx),
        KeyBinding::new("ctrl-h", input::Backspace, ctx),
        KeyBinding::new("ctrl-d", input::Delete, ctx),
        KeyBinding::new("ctrl-k", input::DeleteToLineEnd, ctx),
    ]);
    #[cfg(not(target_os = "macos"))]
    bindings.extend([
        KeyBinding::new("ctrl-a", input::SelectAll, ctx),
        KeyBinding::new("ctrl-c", input::Copy, ctx),
        KeyBinding::new("ctrl-x", input::Cut, ctx),
        KeyBinding::new("ctrl-v", input::Paste, ctx),
        KeyBinding::new("ctrl-left", input::WordLeft, ctx),
        KeyBinding::new("ctrl-right", input::WordRight, ctx),
        KeyBinding::new("ctrl-shift-left", input::SelectWordLeft, ctx),
        KeyBinding::new("ctrl-shift-right", input::SelectWordRight, ctx),
        KeyBinding::new("ctrl-backspace", input::DeleteWordLeft, ctx),
        KeyBinding::new("ctrl-delete", input::DeleteWordRight, ctx),
        KeyBinding::new("ctrl-z", input::Undo, ctx),
        KeyBinding::new("ctrl-shift-z", input::Redo, ctx),
    ]);
    bindings
}

#[cfg(test)]
mod tests {
    use super::*;
    use bezel::gpui::{Action, Keystroke};

    fn assert_binding<A: Action>(bindings: &[KeyBinding], key: &str, action: A) {
        let key = Keystroke::parse(key).unwrap();
        let binding = bindings
            .iter()
            .find(|binding| binding.match_keystrokes(&[key.clone()]) == Some(false))
            .expect("editing key must be bound");
        assert_eq!(binding.action().name(), action.name());
        assert!(binding.predicate().is_some());
    }

    #[test]
    fn single_line_shift_arrows_select_to_name_boundaries() {
        let bindings = field_editing_bindings("ArbosSessionName", false);
        assert_binding(&bindings, "shift-up", input::SelectHome);
        assert_binding(&bindings, "shift-down", input::SelectEnd);
    }

    #[test]
    fn multiline_shift_arrows_keep_vertical_selection() {
        let bindings = field_editing_bindings("Composer", true);
        assert_binding(&bindings, "shift-up", input::SelectUp);
        assert_binding(&bindings, "shift-down", input::SelectDown);
    }

    #[test]
    fn editing_bindings_leave_host_navigation_unclaimed() {
        for multiline in [false, true] {
            let bindings = field_editing_bindings("Field", multiline);
            for key in ["up", "down", "enter", "escape"] {
                let key = Keystroke::parse(key).unwrap();
                assert!(
                    bindings
                        .iter()
                        .all(|binding| { binding.match_keystrokes(&[key.clone()]) != Some(false) })
                );
            }
        }
    }
}
