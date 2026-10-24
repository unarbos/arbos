//! The avatar picker sheet (D92 amendment, Jacob 2026-10-23): Grok Bot's avatar picker for an
//! agent's picture — a tab pill **Bot | Generate | Upload** with **Reset** at the right, a body
//! per tab, and a footer **Cancel | Set avatar**. Nothing changes until "Set avatar".
//!
//! - **Bot**: the sixteen D92 (v2) leaves as their 4 × 4 sheet, every leaf neutral and the
//!   selected one in its hue (pastel disc, darker leaf); the ten hues in two rows of five.
//!   The per-id automatic default stays: Reset goes back to it.
//! - **Generate**: a short prompt → `POST /projects/{id}/avatar/generate` (the relay's OpenAI
//!   key, one low-quality square, metered and capped) → the picture previews here; Set makes it
//!   the avatar.
//! - **Upload**: the image-only file panel (PNG, JPEG, HEIC, GIF, WebP), uploaded as a 512 px
//!   JPEG into the project's blob slot on Set.
//!
//! Groups and servers open the same sheet for their picture with the same three tabs (Bot = their
//! leaf, Generate through `/chats/{id}` / `/servers/{id}` `/avatar/generate`, Upload into their
//! own blob slot).
//!
//! Sizes are Grok's, measured on the 2x reference (612 × 794 px → 306 × 397 pt): the bar 44 pt
//! with a hairline, 32 pt icons on a 57 pt pitch (we keep the Create sheet's 33 / 40 / 56 so the
//! two sheets agree), 23 pt swatches, two equal 28 pt footer pills 8 pt apart.

use std::path::PathBuf;

use gpui::{
    div, prelude::*, px, Context, Div, ElementId, FontWeight, Hsla, MouseButton, SharedString,
};

use crate::app::AppState;
use crate::model::Project;
use crate::ui::avatars::{self, leaf, LeafPick};
use crate::ui::create_entity::{hex, palette};
use crate::ui::frost::{frosted, PANEL_BLUR};
use crate::ui::shell::Shell;
use crate::ui::theme::{Appearance, Theme};

pub const WIDTH: f32 = 306.0;
pub const HEIGHT: f32 = 397.0;
const RADIUS: f32 = 16.0;
const BAR_H: f32 = 44.0;
const BAR_INSET: f32 = 12.0;
const TAB_H: f32 = 28.0;
/// The 4 × 4 leaf sheet (v2) on a 48 pt pitch, then the ten hues on a 36 pt pitch; the
/// footer hairline stays at 340.5.
const LEAF_ROWS: [f32; 4] = [84.0, 132.0, 180.0, 228.0];
const LEAF_PITCH: f32 = 48.0;
const LEAF_DISC: f32 = 30.0;
const LEAF_RING: f32 = 36.0;
const SWATCH_ROWS: [f32; 2] = [282.0, 318.0];
const SWATCH_PITCH: f32 = 36.0;
const SWATCH: f32 = 21.0;
const SWATCH_RING: f32 = 29.0;
const FOOTER_H: f32 = 56.5;
const FOOTER_TOP: f32 = HEIGHT - FOOTER_H;
const PILL_H: f32 = 28.0;
const PILL_GAP: f32 = 8.0;
const PREVIEW: f32 = 114.0;
const FIELD_INSET: f32 = 16.5;
const FIELD_H: f32 = 35.0;

/// Which body is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Bot,
    Generate,
    Upload,
}

impl Tab {
    /// The tabs every target gets — agents, groups and servers alike (Jacob, 2026-10-24: the
    /// group / server picker had lost Generate); Reset sits beside them in the bar.
    pub fn for_target(_target: &Target) -> [Tab; 3] {
        [Tab::Bot, Tab::Generate, Tab::Upload]
    }

    fn label(self) -> &'static str {
        match self {
            Tab::Bot => "Bot",
            Tab::Generate => "Generate",
            Tab::Upload => "Upload",
        }
    }
}

/// Whose picture the sheet edits (the sync layer's `PictureTarget`).
pub type Target = crate::sync::PictureTarget;

/// The sheet's pending choice; applied by "Set avatar", dropped by Cancel.
#[derive(Clone, Debug)]
pub struct Picker {
    pub target: Target,
    pub tab: Tab,
    /// Bot tab: the leaf / colour clicked here (`None` = as the entity has it).
    pub leaf: Option<u8>,
    pub color: Option<u8>,
    /// Reset pressed: back to the automatic leaf, no picture.
    pub reset: bool,
    /// Upload tab: the file chosen in the panel.
    pub upload: Option<PathBuf>,
    /// Generate tab: the relay's picture, `(image_key, url)`.
    pub generated: Option<(String, String)>,
    pub generating: bool,
    pub error: Option<String>,
}

impl Picker {
    pub fn new(target: Target) -> Self {
        Picker {
            target,
            tab: Tab::Bot,
            leaf: None,
            color: None,
            reset: false,
            upload: None,
            generated: None,
            generating: false,
            error: None,
        }
    }

    /// Is there anything for "Set avatar" to apply?
    pub fn dirty(&self) -> bool {
        self.reset
            || match self.tab {
                Tab::Bot => self.leaf.is_some() || self.color.is_some(),
                Tab::Generate => self.generated.is_some(),
                Tab::Upload => self.upload.is_some(),
            }
    }

    /// The leaf + colour the Bot tab shows: the sheet's clicks over the entity's current pair.
    fn pick(&self, project: &Project) -> LeafPick {
        let current = if self.reset {
            leaf::pick_for(&project.id, None, None)
        } else {
            avatars::project_leaf(project)
        };
        LeafPick {
            leaf: self.leaf.unwrap_or(current.leaf),
            color: self.color.unwrap_or(current.color),
        }
    }
}

/// Unselected leaves in the grid: a neutral disc and a neutral leaf (Grok tints only the pick).
fn muted_disc(theme: &Theme) -> Hsla {
    match theme.appearance {
        Appearance::Dark => hex(0x3A3A3A),
        Appearance::Light => theme.field,
    }
}

fn muted_ink(theme: &Theme) -> Hsla {
    match theme.appearance {
        Appearance::Dark => hex(0x8A8A8A),
        Appearance::Light => theme.text_muted,
    }
}

fn ring(size: f32, selected: bool, colour: Hsla) -> Div {
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(if selected {
            colour
        } else {
            gpui::transparent_black()
        })
        .cursor_pointer()
}

/// A leaf on a disc, both in any colour (the Bot grid tints only the selected one).
fn leaf_on(leaf_index: u8, size: f32, disc: Hsla, ink: Hsla) -> Div {
    div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(disc)
        .child(avatars::leaf_glyph_inked(leaf_index, size, 0.0, ink))
}

/// The round picture the Generate / Upload tabs preview: the pending choice, else the entity's
/// picture as it is now.
fn preview(picker: &Picker, project: &Project, state: &AppState, theme: &Theme) -> Div {
    let frame = || {
        div()
            .size(px(PREVIEW))
            .flex_shrink_0()
            .rounded_full()
            .overflow_hidden()
    };
    if let Some(path) = &picker.upload {
        return frame().child(
            gpui::img(path.clone())
                .size(px(PREVIEW))
                .rounded_full()
                .object_fit(gpui::ObjectFit::Cover),
        );
    }
    if let Some((_, url)) = &picker.generated {
        if let Some(image) = state.avatar(url) {
            return frame().child(
                gpui::img(image)
                    .size(px(PREVIEW))
                    .rounded_full()
                    .object_fit(gpui::ObjectFit::Cover),
            );
        }
    }
    if picker.reset {
        return avatars::leaf_element(leaf::pick_for(&project.id, None, None), PREVIEW);
    }
    crate::ui::groups::picture(project, state, PREVIEW, theme)
}

pub fn sheet(shell: &Shell, picker: &Picker, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();
    let p = palette(&theme);
    let state = shell.state.read(cx);
    // Agents and groups are `Project`s; a server is a `ServerInfo` dressed as one for the preview
    // (id, name, leaf pick and picture URL are all the sheet reads).
    let project: Project = match &picker.target {
        Target::Agent(id) | Target::Group(id) => {
            match state.projects.iter().find(|x| &x.id == id) {
                Some(p) => p.clone(),
                None => return div(),
            }
        }
        Target::Server(id) => match state.servers.iter().find(|s| &s.id == id) {
            Some(s) => Project {
                id: s.id.clone(),
                name: s.name.clone(),
                avatar_leaf: s.avatar_leaf,
                avatar_leaf_color: s.avatar_leaf_color,
                image_url: s.image_url.clone(),
                ..Project::default()
            },
            None => return div(),
        },
    };
    let label_muted = match theme.appearance {
        Appearance::Dark => hex(0x9A9A9A),
        Appearance::Light => theme.text_muted,
    };
    let can_reset = project.avatar_leaf.is_some()
        || project.avatar_leaf_color.is_some()
        || project.image_url.is_some();

    // ----- bar: Bot | Generate | Upload …… Reset ---------------------------------------------
    let mut tabs = div()
        .absolute()
        .left(px(BAR_INSET))
        .top(px((BAR_H - TAB_H) / 2.0))
        .h(px(TAB_H))
        .flex()
        .items_center()
        .gap(px(2.0));
    for tab in Tab::for_target(&picker.target) {
        let selected = picker.tab == tab;
        tabs = tabs.child(
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "avatar-tab-{}",
                    tab.label().to_lowercase()
                ))))
                .h(px(TAB_H))
                .px(px(11.0))
                .rounded_full()
                .flex()
                .items_center()
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .when(selected, |d| d.bg(p.cancel_bg).text_color(p.title))
                .when(!selected, |d| d.text_color(label_muted))
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .child(tab.label())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(pk) = this.avatar_picker.as_mut() {
                        pk.tab = tab;
                        pk.error = None;
                    }
                    cx.notify();
                })),
        );
    }
    let reset = div()
        .id("avatar-reset")
        .absolute()
        .right(px(BAR_INSET))
        .top(px((BAR_H - TAB_H) / 2.0))
        .h(px(TAB_H))
        .px(px(6.0))
        .flex()
        .items_center()
        .text_size(px(13.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if picker.reset {
            p.title
        } else if can_reset {
            label_muted
        } else {
            p.create_text
        })
        .child("Reset")
        .when(can_reset, |d| {
            d.cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(pk) = this.avatar_picker.as_mut() {
                        pk.reset = true;
                        pk.leaf = None;
                        pk.color = None;
                        pk.upload = None;
                        pk.generated = None;
                        pk.error = None;
                    }
                    cx.notify();
                }))
        });
    let bar = div()
        .absolute()
        .left_0()
        .right_0()
        .top_0()
        .h(px(BAR_H))
        .border_b_1()
        .border_color(p.line)
        .child(tabs)
        .child(reset);

    // ----- body ---------------------------------------------------------------------------------
    let mut body: Vec<gpui::AnyElement> = Vec::new();
    match picker.tab {
        Tab::Bot => {
            let pick = picker.pick(&project);
            for (r, top) in LEAF_ROWS.iter().enumerate() {
                let leaves: Vec<u8> = (r as u8 * 4..r as u8 * 4 + 4).collect();
                let mut row = div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top - LEAF_RING / 2.0))
                    .flex()
                    .justify_center();
                for index in leaves {
                    let selected = pick.leaf == index;
                    let (disc, ink) = if selected {
                        (
                            avatars::leaf_disc(pick.color),
                            avatars::leaf_ink(pick.color),
                        )
                    } else {
                        (muted_disc(&theme), muted_ink(&theme))
                    };
                    row = row.child(
                        ring(LEAF_RING, selected, p.ring)
                            .id(ElementId::Name(SharedString::from(format!(
                                "avatar-leaf-{index}"
                            ))))
                            .mx(px((LEAF_PITCH - LEAF_RING) / 2.0))
                            .child(leaf_on(index, LEAF_DISC, disc, ink))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(pk) = this.avatar_picker.as_mut() {
                                    pk.leaf = Some(index);
                                    pk.reset = false;
                                }
                                cx.notify();
                            })),
                    );
                }
                body.push(row.into_any_element());
            }
            for (r, top) in SWATCH_ROWS.iter().enumerate() {
                let mut row = div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top - SWATCH_RING / 2.0))
                    .flex()
                    .justify_center();
                for j in 0..5u8 {
                    let index = r as u8 * 5 + j;
                    row = row.child(
                        ring(SWATCH_RING, pick.color == index, p.ring)
                            .id(ElementId::Name(SharedString::from(format!(
                                "avatar-colour-{index}"
                            ))))
                            .mx(px((SWATCH_PITCH - SWATCH_RING) / 2.0))
                            .child(
                                div()
                                    .size(px(SWATCH))
                                    .rounded_full()
                                    .bg(avatars::leaf_disc(index)),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(pk) = this.avatar_picker.as_mut() {
                                    pk.color = Some(index);
                                    pk.reset = false;
                                }
                                cx.notify();
                            })),
                    );
                }
                body.push(row.into_any_element());
            }
        }
        Tab::Generate => {
            let input = shell.avatar_prompt_input.clone();
            let typed = input.read(cx).text().trim().to_string();
            let can_generate = !typed.is_empty() && !picker.generating;
            body.push(
                div()
                    .absolute()
                    .left(px(FIELD_INSET))
                    .right(px(FIELD_INSET))
                    .top(px(BAR_H + 16.0))
                    .h(px(FIELD_H))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(p.line)
                    .flex()
                    .items_center()
                    .px(px(12.0))
                    .text_size(px(14.0))
                    .text_color(if typed.is_empty() {
                        p.placeholder
                    } else {
                        p.title
                    })
                    .child(input)
                    .into_any_element(),
            );
            let go = div()
                .id("avatar-generate-go")
                .absolute()
                .left_0()
                .right_0()
                .top(px(BAR_H + 16.0 + FIELD_H + 12.0))
                .flex()
                .justify_center()
                .child(
                    div()
                        .h(px(PILL_H))
                        .px(px(18.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .when(can_generate, |d| {
                            d.bg(p.cancel_bg).text_color(p.title).cursor_pointer()
                        })
                        .when(!can_generate, |d| {
                            d.bg(p.create_bg).text_color(p.create_text)
                        })
                        .child(if picker.generating {
                            "Generating…"
                        } else if picker.generated.is_some() {
                            "Generate again"
                        } else {
                            "Generate"
                        }),
                )
                .when(can_generate, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| this.generate_avatar(cx)))
                });
            body.push(go.into_any_element());
            body.push(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(BAR_H + 16.0 + FIELD_H + 12.0 + PILL_H + 16.0))
                    .flex()
                    .justify_center()
                    .child(preview(picker, &project, state, &theme))
                    .into_any_element(),
            );
            let status: SharedString = if let Some(e) = &picker.error {
                e.clone().into()
            } else if picker.generating {
                "Painting a picture — a few seconds.".into()
            } else if picker.generated.is_some() {
                "Set avatar keeps this one.".into()
            } else {
                "A few words about the picture you want.".into()
            };
            body.push(
                div()
                    .absolute()
                    .left(px(FIELD_INSET))
                    .right(px(FIELD_INSET))
                    .top(px(FOOTER_TOP - 30.0))
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_center()
                    .text_color(if picker.error.is_some() {
                        theme.danger
                    } else {
                        p.placeholder
                    })
                    .child(status)
                    .into_any_element(),
            );
        }
        Tab::Upload => {
            body.push(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(BAR_H + 24.0))
                    .flex()
                    .justify_center()
                    .child(preview(picker, &project, state, &theme))
                    .into_any_element(),
            );
            body.push(
                div()
                    .id("avatar-upload-choose")
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(BAR_H + 24.0 + PREVIEW + 16.0))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .h(px(PILL_H))
                            .px(px(18.0))
                            .rounded_full()
                            .bg(p.cancel_bg)
                            .flex()
                            .items_center()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.title)
                            .cursor_pointer()
                            .child(if picker.upload.is_some() {
                                "Choose another…"
                            } else {
                                "Choose a photo…"
                            }),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.choose_avatar_file(cx)))
                    .into_any_element(),
            );
            let hint: SharedString = match &picker.upload {
                Some(path) => path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("picture")
                    .to_owned()
                    .into(),
                None => "PNG, JPEG, HEIC, GIF or WebP; kept at 512 px.".into(),
            };
            body.push(
                div()
                    .absolute()
                    .left(px(FIELD_INSET))
                    .right(px(FIELD_INSET))
                    .top(px(FOOTER_TOP - 30.0))
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_center()
                    .text_color(p.placeholder)
                    .child(hint)
                    .into_any_element(),
            );
        }
    }

    // ----- footer: Cancel | Set avatar -----------------------------------------------------
    let pill_w = (WIDTH - 2.0 * BAR_INSET - PILL_GAP) / 2.0;
    let pill = |id: &'static str, label: &'static str, bg: Hsla, fg: Hsla| {
        div()
            .id(id)
            .absolute()
            .top(px((FOOTER_H - PILL_H) / 2.0))
            .w(px(pill_w))
            .h(px(PILL_H))
            .rounded_full()
            .bg(bg)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(13.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(fg)
            .child(label)
    };
    let can_set = picker.dirty() && !picker.generating;
    let (set_bg, set_fg) = match theme.appearance {
        Appearance::Dark => (hex(0xE5E5E5), hex(0x1F1F1F)),
        Appearance::Light => (theme.primary, theme.on_primary),
    };
    let footer = div()
        .absolute()
        .left_0()
        .right_0()
        .top(px(FOOTER_TOP))
        .h(px(FOOTER_H))
        .border_t_1()
        .border_color(p.line)
        .child(
            pill("avatar-cancel", "Cancel", p.cancel_bg, p.title)
                .left(px(BAR_INSET))
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .on_click(cx.listener(|this, _, _, cx| this.close_avatar_picker(cx))),
        )
        .child(if can_set {
            pill("avatar-set", "Set avatar", set_bg, set_fg)
                .right(px(BAR_INSET))
                .cursor_pointer()
                .hover(|s| s.opacity(0.88))
                .on_click(cx.listener(|this, _, _, cx| this.set_avatar(cx)))
        } else {
            pill("avatar-set", "Set avatar", p.create_bg, p.create_text).right(px(BAR_INSET))
        });

    let card = div()
        .relative()
        .w(px(WIDTH))
        .h(px(HEIGHT))
        .max_w_full()
        .max_h_full()
        .rounded(px(RADIUS))
        .bg(p.bg)
        .border_1()
        .border_color(p.line)
        .shadow_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(bar)
        .children(body)
        .child(footer);
    div()
        .absolute()
        .inset_0()
        .occlude()
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .flex()
        .items_center()
        .justify_center()
        .p(px(24.0))
        .bg(Theme::with_alpha(theme.bg, 0.55))
        .child(frosted(RADIUS, PANEL_BLUR, card))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| this.close_avatar_picker(cx)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_target_gets_bot_generate_upload_and_reset() {
        for target in [
            Target::Agent("a".into()),
            Target::Group("g".into()),
            Target::Server("s".into()),
        ] {
            assert_eq!(
                Tab::for_target(&target),
                [Tab::Bot, Tab::Generate, Tab::Upload],
                "{target:?}"
            );
            // Reset is live for every target once there is something to reset.
            let mut pk = Picker::new(target);
            pk.reset = true;
            assert!(pk.dirty());
        }
    }
}
