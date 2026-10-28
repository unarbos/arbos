//! What the active keyboard layout types for a chord GPUI delivers without a
//! character ([feedback mention-hotkey-layout-2026-10-28], Scout, German Mac).
//!
//! GPUI's macOS layer leaves `Keystroke::key_char` empty whenever Control is
//! held (`gpui_macos/events.rs` `parse_keystroke`), and AppKit routes a Control
//! chord through `performKeyEquivalent:` only, so the chord never reaches
//! `insertText:` either. On layouts whose own tables define Control+Option
//! chords as characters — German and Austrian type `@` on ⌃⌥L as well as ⌥L —
//! the composer saw a bare `ctrl-alt-l` and typed nothing, so the `@` mention
//! picker never opened. `NSTextView` types `@` there.
//!
//! The fix asks the layout itself: [`typed_text`] finds the key the chord sits
//! on and reads the layout's output for the full modifier set (Carbon's
//! `UCKeyTranslate`, the same table AppKit consults), and the composer types
//! that when it is printable. Nothing is special-cased to `@` or to German —
//! US ⌃⌥L stays a control character and types nothing, exactly as in TextEdit.

use gpui::{Keystroke, Modifiers};

/// A keyboard layout: the text a physical key produces under a modifier set.
pub trait KeyboardLayout {
    /// What the key with this macOS virtual key code types under `modifiers`
    /// (dead keys resolved, no composition). `None` when the layout cannot be
    /// read; an empty string when the key types nothing.
    fn translate(&self, key_code: u16, modifiers: &Modifiers) -> Option<String>;
}

/// macOS virtual key codes run 0..=127.
const KEY_CODES: std::ops::Range<u16> = 0..128;

/// The text the layout types for a chord GPUI delivered with no `key_char`:
/// a Control+Option chord whose `key` is one character. `None` for every
/// chord that already carries its character, lacks Control or Option, carries
/// ⌘ or fn, or that the layout maps to nothing printable (US ⌃⌥L → `\x0c`).
///
/// Control+Option only: on the layouts that define it at all (German,
/// Austrian) the whole ⌃⌥ column repeats the ⌥ column — AltGr by another
/// name — and that is the class of chord GPUI loses. Plain Control chords stay
/// as they are (US ⌃2 reports "2" but is not typed).
pub fn typed_text(keystroke: &Keystroke, layout: &dyn KeyboardLayout) -> Option<String> {
    if keystroke.key_char.is_some() {
        return None;
    }
    let mods = &keystroke.modifiers;
    if !mods.control || !mods.alt || mods.platform || mods.function {
        return None;
    }
    if keystroke.key.chars().count() != 1 {
        return None;
    }
    let plain = Modifiers::none();
    let key_code = KEY_CODES
        .into_iter()
        .find(|code| layout.translate(*code, &plain).as_deref() == Some(keystroke.key.as_str()))?;
    let text = layout.translate(key_code, mods)?;
    (!text.is_empty() && text.chars().all(|c| !c.is_control())).then_some(text)
}

/// The layout the user has selected in System Settings (macOS); elsewhere a
/// layout that knows nothing — Linux's xkb already fills `key_char` for AltGr
/// chords and defines no Control+Alt characters.
#[derive(Default)]
pub struct SystemLayout;

#[cfg(not(target_os = "macos"))]
impl KeyboardLayout for SystemLayout {
    fn translate(&self, _key_code: u16, _modifiers: &Modifiers) -> Option<String> {
        None
    }
}

#[cfg(target_os = "macos")]
impl KeyboardLayout for SystemLayout {
    fn translate(&self, key_code: u16, modifiers: &Modifiers) -> Option<String> {
        carbon::translate(key_code, modifiers)
    }
}

/// Carbon's Text Input Services + `UCKeyTranslate`, the table AppKit itself
/// uses to turn a key event into `-[NSEvent characters]`.
#[cfg(target_os = "macos")]
mod carbon {
    use std::ffi::c_void;

    use gpui::Modifiers;

    type CFTypeRef = *const c_void;
    type CFDataRef = *const c_void;
    type CFStringRef = *const c_void;
    type TISInputSourceRef = *const c_void;

    #[repr(C)]
    struct UCKeyboardLayout {
        _opaque: [u8; 0],
    }

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
        fn TISCopyCurrentKeyboardLayoutInputSource() -> TISInputSourceRef;
        fn TISGetInputSourceProperty(source: TISInputSourceRef, key: CFStringRef) -> *const c_void;
        fn LMGetKbdType() -> u8;
        fn UCKeyTranslate(
            layout: *const UCKeyboardLayout,
            virtual_key_code: u16,
            key_action: u16,
            modifier_key_state: u32,
            keyboard_type: u32,
            key_translate_options: u32,
            dead_key_state: *mut u32,
            max_string_length: usize,
            actual_string_length: *mut usize,
            unicode_string: *mut u16,
        ) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
        fn CFRelease(cf: CFTypeRef);
    }

    // Carbon `Events.h` modifier bits, shifted right by 8 for `UCKeyTranslate`.
    const SHIFT: u32 = 1 << 1;
    const OPTION: u32 = 1 << 3;
    const CONTROL: u32 = 1 << 4;
    const KEY_ACTION_DOWN: u16 = 0;
    const NO_DEAD_KEYS: u32 = 0;

    fn modifier_state(modifiers: &Modifiers) -> u32 {
        let mut state = 0;
        if modifiers.shift {
            state |= SHIFT;
        }
        if modifiers.alt {
            state |= OPTION;
        }
        if modifiers.control {
            state |= CONTROL;
        }
        state
    }

    pub(super) fn translate(key_code: u16, modifiers: &Modifiers) -> Option<String> {
        // SAFETY: Carbon's documented contract — the copied input source is
        // released once, the layout data pointer stays valid while the source
        // is alive, and `UCKeyTranslate` writes at most `max_string_length`
        // UTF-16 units into the buffer we hand it.
        unsafe {
            let source = TISCopyCurrentKeyboardLayoutInputSource();
            if source.is_null() {
                return None;
            }
            let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
            let text = if data.is_null() {
                None
            } else {
                let layout = CFDataGetBytePtr(data) as *const UCKeyboardLayout;
                let mut dead_key_state = 0u32;
                let mut buffer = [0u16; 8];
                let mut len = 0usize;
                let status = UCKeyTranslate(
                    layout,
                    key_code,
                    KEY_ACTION_DOWN,
                    modifier_state(modifiers),
                    LMGetKbdType() as u32,
                    NO_DEAD_KEYS,
                    &mut dead_key_state,
                    buffer.len(),
                    &mut len,
                    buffer.as_mut_ptr(),
                );
                (status == 0).then(|| String::from_utf16_lossy(&buffer[..len.min(buffer.len())]))
            };
            CFRelease(source);
            text
        }
    }
}

#[cfg(test)]
pub mod test_layouts {
    //! Layout tables read off real macOS layouts with `UCKeyTranslate`
    //! (gate Mac, macOS 26.6, 2026-10-28): the key codes and outputs below are
    //! what the system reports, not approximations.
    use gpui::Modifiers;

    use super::KeyboardLayout;

    /// `(key code, plain, ⇧, ⌥, ⌥⇧, ⌃, ⌃⇧, ⌃⌥, ⌃⌥⇧)`.
    type Row = (
        u16,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    );

    pub struct Table(&'static [Row]);

    impl KeyboardLayout for Table {
        fn translate(&self, key_code: u16, m: &Modifiers) -> Option<String> {
            let row = self.0.iter().find(|r| r.0 == key_code)?;
            let text = match (m.control, m.alt, m.shift) {
                (false, false, false) => row.1,
                (false, false, true) => row.2,
                (false, true, false) => row.3,
                (false, true, true) => row.4,
                (true, false, false) => row.5,
                (true, false, true) => row.6,
                (true, true, false) => row.7,
                (true, true, true) => row.8,
            };
            Some(text.to_string())
        }
    }

    /// `com.apple.keylayout.German`: `@` sits on L under ⌥ **and** ⌃⌥ — the
    /// whole ⌃⌥ column repeats the ⌥ column (Q `«`, F `ƒ`, Y `¥`); the plain
    /// ⌃ column is control characters.
    pub const GERMAN: Table = Table(&[
        (37, "l", "L", "@", "ﬂ", "\x0c", "\x0c", "@", "ﬂ"),
        (12, "q", "Q", "«", "»", "\x11", "\x11", "«", "»"),
        (3, "f", "F", "ƒ", "Ï", "\x06", "\x06", "ƒ", "Ï"),
        (6, "y", "Y", "¥", "‡", "\x19", "\x19", "¥", "‡"),
        (19, "2", "\"", "“", "”", "2", "\"", "“", "”"),
    ]);

    /// `com.apple.keylayout.US`: `@` is ⇧2; ⌥L is `¬`; every ⌃ / ⌃⌥ letter
    /// chord is a control character, so ⌃⌥L types nothing; ⌃2 reports "2".
    pub const US: Table = Table(&[
        (37, "l", "L", "¬", "Ò", "\x0c", "\x0c", "\x0c", "\x0c"),
        (12, "q", "Q", "œ", "Œ", "\x11", "\x11", "\x11", "\x11"),
        (19, "2", "@", "™", "€", "2", "2", "2", "2"),
    ]);

    /// `com.apple.keylayout.French` (AZERTY): `@` is a plain key (code 10,
    /// left of 1); ⌃⌥L is a control character.
    pub const FRENCH: Table = Table(&[
        (37, "l", "L", "¬", "|", "\x0c", "\x0c", "\x0c", "\x0c"),
        (10, "@", "#", "•", "Ÿ", "0", "0", "0", "0"),
    ]);

    /// `com.apple.keylayout.Spanish`: `@` is ⌥G; ⌃⌥L and ⌃⌥G are control
    /// characters.
    pub const SPANISH: Table = Table(&[
        (37, "l", "L", "¬", "ˆ", "\x0c", "\x0c", "\x0c", "\x0c"),
        (5, "g", "G", "@", "‚", "\x07", "\x07", "\x07", "\x07"),
    ]);
}

#[cfg(test)]
mod tests {
    use gpui::Keystroke;

    use super::test_layouts::{FRENCH, GERMAN, SPANISH, US};
    use super::typed_text;

    fn chord(keys: &str) -> Keystroke {
        Keystroke::parse(keys).expect("keystroke")
    }

    #[test]
    fn german_control_option_l_types_the_at_sign() {
        assert_eq!(
            typed_text(&chord("ctrl-alt-l"), &GERMAN).as_deref(),
            Some("@")
        );
    }

    #[test]
    fn german_control_option_column_types_what_the_layout_says() {
        // The whole ⌃⌥ column is the ⌥ column on German, not just L.
        assert_eq!(
            typed_text(&chord("ctrl-alt-shift-l"), &GERMAN).as_deref(),
            Some("ﬂ")
        );
        assert_eq!(
            typed_text(&chord("ctrl-alt-q"), &GERMAN).as_deref(),
            Some("«")
        );
        assert_eq!(
            typed_text(&chord("ctrl-alt-y"), &GERMAN).as_deref(),
            Some("¥")
        );
        // ⌃⌥F too — in the app the `ctrl-alt-f` Send Feedback binding runs
        // first and the chord never reaches the composer's fallback.
        assert_eq!(
            typed_text(&chord("ctrl-alt-f"), &GERMAN).as_deref(),
            Some("ƒ")
        );
    }

    #[test]
    fn control_characters_type_nothing() {
        // ⌃⌥L is form-feed on US, French and Spanish; ⌃⌥G is BEL on Spanish.
        assert_eq!(typed_text(&chord("ctrl-alt-l"), &US), None);
        assert_eq!(typed_text(&chord("ctrl-alt-l"), &FRENCH), None);
        assert_eq!(typed_text(&chord("ctrl-alt-l"), &SPANISH), None);
        assert_eq!(typed_text(&chord("ctrl-alt-g"), &SPANISH), None);
    }

    #[test]
    fn plain_control_chords_are_not_typed_even_when_the_layout_reports_a_digit() {
        // US / German ⌃2 report "2"; Control without Option is not the lost class.
        assert_eq!(typed_text(&chord("ctrl-2"), &US), None);
        assert_eq!(typed_text(&chord("ctrl-2"), &GERMAN), None);
        assert_eq!(typed_text(&chord("ctrl-l"), &GERMAN), None);
    }

    #[test]
    fn chords_that_already_carry_a_character_are_left_to_text_input() {
        // ⌥L on German arrives as `alt-l` with `key_char` "@" and is typed by
        // the platform; the fallback must not type it a second time.
        assert_eq!(typed_text(&chord("alt-l->@"), &GERMAN), None);
        assert_eq!(typed_text(&chord("l->l"), &GERMAN), None);
    }

    #[test]
    fn command_fn_and_plain_chords_are_never_typed() {
        assert_eq!(typed_text(&chord("cmd-ctrl-alt-l"), &GERMAN), None);
        assert_eq!(typed_text(&chord("fn-ctrl-alt-l"), &GERMAN), None);
        assert_eq!(typed_text(&chord("alt-l"), &GERMAN), None);
        assert_eq!(typed_text(&chord("l"), &GERMAN), None);
        assert_eq!(typed_text(&chord("ctrl-alt-enter"), &GERMAN), None);
    }

    #[test]
    fn a_key_the_layout_does_not_have_types_nothing() {
        assert_eq!(typed_text(&chord("ctrl-alt-z"), &GERMAN), None);
    }
}
