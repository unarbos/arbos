//! The colours the chat is painted in: Cursor's, measured from its window.
//!
//! bezel's own dark palette is near-black with a translucent tint over the
//! desktop; Cursor is an opaque warm charcoal. Every value here was sampled
//! from a screenshot of Cursor at 2x (dominant colour of a region), so the
//! two windows side by side read as the same material. Light keeps bezel's
//! palette: Cursor's light theme was not sampled.

use bezel::{
    gpui::{App, Hsla, hsla, rgb},
    theme::{Appearance, Material, SurfaceStyle, SyntaxPalette, Theme, set_palette},
};

/// Register the palette. Call before `appearance::init`, which installs the
/// first theme.
pub fn install(cx: &mut App) {
    set_palette(build, cx);
}

fn build(appearance: Appearance) -> Theme {
    let mut theme = Theme::for_appearance(appearance);
    if appearance == Appearance::Dark {
        cursor_dark(&mut theme);
    }
    // The app's own face on every platform (`crate::fonts`): the same
    // glyphs, metrics and weights on a Mac, on Linux, and on the rig that
    // measures them — Cursor ships its font for the same reason.
    theme.font_sans = crate::fonts::FAMILY.into();
    theme
}

fn grey(value: u32) -> Hsla {
    rgb(value).into()
}

/// Cursor's dark palette. Comments give the sampled sRGB value.
fn cursor_dark(t: &mut Theme) {
    // Panels.
    t.bg = grey(0x161514); // chat column and the transcript behind it
    t.surface = grey(0x1a1a16); // sidebar
    t.surface_card = grey(0x212121); // the user's message card
    t.surface_raised = grey(0x212121); // chips, composer, hover plates
    t.surface_raised_hover = grey(0x2a2a2a);
    t.surface_dialog = grey(0x1e1e1e);
    t.surface_overlay = grey(0x242424);
    t.input_bg = grey(0x212121); // the composer pill
    // Lines: a hair lighter than the plate they sit on.
    t.border = hsla(0.0, 0.0, 1.0, 0.11); // #353535 on the composer
    t.border_strong = hsla(0.0, 0.0, 1.0, 0.18);
    // Ink.
    t.text = grey(0xf0f0f0); // body and headings
    t.text_muted = grey(0xbbbbbb); // the model name in the composer
    t.text_faint = grey(0x999898); // "Worked for 15m 20s"
    t.text_dim = grey(0x6b6b6b); // the composer placeholder
    // Inline code: a pill a step above the panel, not a wash.
    t.code_wash = grey(0x272625);
    t.code_text = grey(0xf0f0f0);
    // Cursor's one colour: links and "asking" marks in its blue. The send
    // plate stays white ink.
    t.accent = grey(0x86aee4);
    t.caret = grey(0x86aee4);
    // Selected text: the same blue as a wash, not bezel's violet.
    t.selection = hsla(0.6, 0.6, 0.7, 0.33);
    t.solid = grey(0xf0f0f0);
    // Sidebar hover and selection: Cursor's selected row is #262623 on
    // #1a1a16, a lift of twelve — white at 5%; hover a step under it.
    t.element_hover = hsla(0.0, 0.0, 1.0, 0.035);
    t.element_active = hsla(0.0, 0.0, 1.0, 0.05);
    // Menus and pickers: Cursor's are opaque dark panels with a hairline,
    // not frost. The thickest material is the nearest bezel has to opaque.
    t.popover_surface = SurfaceStyle::Material(Material::UltraThick);
    // Code: Cursor Dark is VS Code's Dark+ — orange strings, mauve
    // keywords, sky-blue names, sage numbers, green comments.
    t.syntax = SyntaxPalette {
        comment: grey(0x6a9955),
        keyword: grey(0xc586c0),
        string: grey(0xce9178),
        string_special: grey(0xd7ba7d),
        escape: grey(0xd7ba7d),
        number: grey(0xb5cea8),
        boolean: grey(0x569cd6),
        type_name: grey(0x4ec9b0),
        type_builtin: grey(0x4ec9b0),
        constructor: grey(0x4ec9b0),
        function: grey(0xdcdcaa),
        function_builtin: grey(0xdcdcaa),
        macro_name: grey(0xdcdcaa),
        property: grey(0x9cdcfe),
        constant: grey(0x4fc1ff),
        variable: grey(0x9cdcfe),
        variable_special: grey(0x569cd6),
        parameter: grey(0x9cdcfe),
        operator: grey(0xd4d4d4),
        punctuation: grey(0xd4d4d4),
        tag: grey(0x569cd6),
        attribute: grey(0x9cdcfe),
        label: grey(0xc8c8c8),
        invalid: grey(0xf44747),
    };
}
