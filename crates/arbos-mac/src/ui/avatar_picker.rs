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
//! The Create sheets (`create_entity`, Jacob 2026-10-25) draw the **same Generate and Upload
//! bodies** — [`tab_pill`], [`generate_body`], [`upload_body`], [`pending_preview`] — on a
//! [`Picker`] whose target is [`Target::Draft`]: no Bot tab, no Reset, and the picture is
//! generated into my own slot (`POST /me/avatar/generate`) until the entity exists.
//!
//! My own profile picture (Jacob, 2026-10-25) opens this sheet too — [`Target::Me`]: **Generate |
//! Upload** with Reset (people are D79 coloured initials, so no Bot tab); Generate goes through
//! the same `/me/avatar/generate`, Set sends `PUT /me/avatar {image_key}` or the file, Reset is
//! `DELETE /me/avatar`.
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
use crate::ui::create_entity::{hex, palette, Palette};
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
    /// The tabs a target gets: agents, groups and servers alike have all three (Jacob,
    /// 2026-10-24: the group / server picker had lost Generate), Reset beside them in the bar.
    /// A Create sheet's draft (Jacob, 2026-10-25) has **Upload and Generate only** — no leaf
    /// grid, no swatches, no Reset. My own picture (Jacob, 2026-10-25) has no leaf either —
    /// people are D79 coloured initials — so it gets **Generate | Upload**, Reset in the bar.
    pub fn for_target(target: &Target) -> Vec<Tab> {
        match target {
            Target::Agent(_) | Target::Group(_) | Target::Server(_) => {
                vec![Tab::Bot, Tab::Generate, Tab::Upload]
            }
            Target::Draft => vec![Tab::Upload, Tab::Generate],
            Target::Me(_) => vec![Tab::Generate, Tab::Upload],
        }
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
        let tab = Tab::for_target(&target)[0];
        Picker {
            target,
            tab,
            leaf: None,
            color: None,
            reset: false,
            upload: None,
            generated: None,
            generating: false,
            error: None,
        }
    }

    /// The Create sheet's pending picture for the entity about to exist: the file, or the
    /// picture generated into my slot. `None` = nothing picked → the entity is stored neutral.
    pub fn draft_picture(&self) -> Option<crate::sync::DraftPicture> {
        match self.tab {
            Tab::Upload => self.upload.clone().map(crate::sync::DraftPicture::File),
            Tab::Generate => self
                .generated
                .as_ref()
                .map(|(key, _)| crate::sync::DraftPicture::Generated { key: key.clone() }),
            Tab::Bot => None,
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

/// The pending choice as a round picture of `size`: the chosen file, or the generated picture
/// once the loader has it; `None` while there is neither (the caller draws what the entity has
/// now — or, on a Create sheet, the neutral initial).
pub(crate) fn pending_preview(picker: &Picker, state: &AppState, size: f32) -> Option<Div> {
    let frame = || {
        div()
            .size(px(size))
            .flex_shrink_0()
            .rounded_full()
            .overflow_hidden()
    };
    if let Some(path) = &picker.upload {
        return Some(
            frame().child(
                gpui::img(path.clone())
                    .size(px(size))
                    .rounded_full()
                    .object_fit(gpui::ObjectFit::Cover),
            ),
        );
    }
    if let Some((_, url)) = &picker.generated {
        if let Some(image) = state.avatar(url) {
            return Some(
                frame().child(
                    gpui::img(image)
                        .size(px(size))
                        .rounded_full()
                        .object_fit(gpui::ObjectFit::Cover),
                ),
            );
        }
    }
    None
}

/// The round picture the Generate / Upload tabs preview: the pending choice, else the entity's
/// picture as it is now.
fn preview(picker: &Picker, project: &Project, state: &AppState, theme: &Theme) -> Div {
    if let Some(pending) = pending_preview(picker, state, PREVIEW) {
        return pending;
    }
    if let Target::Me(_) = &picker.target {
        // My own picture: the account's (GitHub or uploaded), or the D79 initials after Reset.
        let person = me_person(state, picker.reset);
        let image = state.avatar(&person.avatar_url);
        return div()
            .size(px(PREVIEW))
            .flex_shrink_0()
            .rounded_full()
            .overflow_hidden()
            .child(crate::ui::people::avatar(image, &person, PREVIEW, theme));
    }
    if picker.reset {
        return avatars::leaf_element(leaf::pick_for(&project.id, None, None), PREVIEW);
    }
    crate::ui::groups::picture(project, state, PREVIEW, theme)
}

/// Me as a `Person` for the people avatar (initials when `plain` or without a picture).
fn me_person(state: &AppState, plain: bool) -> crate::model::Person {
    let account = state.account.as_ref();
    crate::model::Person {
        id: account.map(|a| a.id.clone()).unwrap_or_default(),
        handle: account.map(|a| a.login.clone()).unwrap_or_default(),
        name: account.map(|a| a.name.clone()),
        avatar_url: if plain {
            String::new()
        } else {
            account.map(|a| a.avatar_url.clone()).unwrap_or_default()
        },
        status: String::new(),
        role: String::new(),
        last_seen_at: None,
        online: false,
        avatar_color_index: None,
    }
}

/// The tab capsules (`Bot | Generate | Upload`, or the draft's `Upload | Generate`), 28 pt,
/// the selected one filled. A click switches the open picker's tab (the sheet's or the Create
/// sheet's — whichever is up, `Shell::pending_picker_mut`).
pub(crate) fn tab_pill(
    tabs: &[Tab],
    selected: Tab,
    p: &Palette,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Div {
    let label_muted = match theme.appearance {
        Appearance::Dark => hex(0x9A9A9A),
        Appearance::Light => theme.text_muted,
    };
    let mut row = div().h(px(TAB_H)).flex().items_center().gap(px(2.0));
    for &tab in tabs {
        let is_selected = selected == tab;
        row = row.child(
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "avatar-tab-{}",
                    tab.label().to_lowercase()
                ))))
                .debug_selector(move || format!("avatar-tab-{}", tab.label().to_lowercase()))
                .h(px(TAB_H))
                .px(px(11.0))
                .rounded_full()
                .flex()
                .items_center()
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .when(is_selected, |d| d.bg(p.cancel_bg).text_color(p.title))
                .when(!is_selected, |d| d.text_color(label_muted))
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .child(tab.label())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(pk) = this.pending_picker_mut() {
                        pk.tab = tab;
                        pk.error = None;
                    }
                    cx.notify();
                })),
        );
    }
    row
}

/// Where a tab body sits in its sheet (points from the card's top): the body's first element
/// at `top`, the status / hint line at `status_top`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Slots {
    pub top: f32,
    pub status_top: f32,
}

/// The Generate tab's body, absolutely placed in a sheet: the prompt field at `slots.top`, the
/// Generate pill under it, `preview` (when given) under that, and the status line at
/// `slots.status_top`. Shared by the picker sheet and the Create sheets.
pub(crate) fn generate_body(
    shell: &Shell,
    picker: &Picker,
    slots: Slots,
    preview: Option<Div>,
    p: &Palette,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<gpui::AnyElement> {
    let Slots { top, status_top } = slots;
    let mut body: Vec<gpui::AnyElement> = Vec::new();
    let input = shell.avatar_prompt_input.clone();
    let typed = input.read(cx).text().trim().to_string();
    let can_generate = !typed.is_empty() && !picker.generating;
    body.push(
        div()
            .absolute()
            .left(px(FIELD_INSET))
            .right(px(FIELD_INSET))
            .top(px(top))
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
    let pill_top = top + FIELD_H + 12.0;
    let go = div()
        .id("avatar-generate-go")
        .debug_selector(|| "avatar-generate-go".into())
        .absolute()
        .left_0()
        .right_0()
        .top(px(pill_top))
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
    if let Some(preview) = preview {
        body.push(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(pill_top + PILL_H + 16.0))
                .flex()
                .justify_center()
                .child(preview)
                .into_any_element(),
        );
    }
    let keeps = match picker.target {
        Target::Draft => "Create keeps this one.",
        Target::Agent(_) | Target::Group(_) | Target::Server(_) | Target::Me(_) => {
            "Set avatar keeps this one."
        }
    };
    let status: SharedString = if let Some(e) = &picker.error {
        e.clone().into()
    } else if picker.generating {
        "Painting a picture — a few seconds.".into()
    } else if picker.generated.is_some() {
        keeps.into()
    } else {
        "A few words about the picture you want.".into()
    };
    body.push(
        div()
            .id("avatar-generate-status")
            .debug_selector(|| "avatar-generate-status".into())
            .absolute()
            .left(px(FIELD_INSET))
            .right(px(FIELD_INSET))
            .top(px(status_top))
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
    body
}

/// The Upload tab's body: `preview` (when given) at `slots.top` with the "Choose a photo…" pill
/// under it — else the pill at `slots.top` — and the hint at `slots.status_top`. Shared by the
/// picker sheet and the Create sheets.
pub(crate) fn upload_body(
    picker: &Picker,
    slots: Slots,
    preview: Option<Div>,
    p: &Palette,
    cx: &mut Context<Shell>,
) -> Vec<gpui::AnyElement> {
    let Slots { top, status_top } = slots;
    let mut body: Vec<gpui::AnyElement> = Vec::new();
    let mut pill_top = top;
    if let Some(preview) = preview {
        body.push(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(top))
                .flex()
                .justify_center()
                .child(preview)
                .into_any_element(),
        );
        pill_top = top + PREVIEW + 16.0;
    }
    body.push(
        div()
            .id("avatar-upload-choose")
            .debug_selector(|| "avatar-upload-choose".into())
            .absolute()
            .left_0()
            .right_0()
            .top(px(pill_top))
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
            .id("avatar-upload-hint")
            .debug_selector(|| "avatar-upload-hint".into())
            .absolute()
            .left(px(FIELD_INSET))
            .right(px(FIELD_INSET))
            .top(px(status_top))
            .text_size(px(12.0))
            .line_height(px(16.0))
            .text_center()
            .text_color(p.placeholder)
            .child(hint)
            .into_any_element(),
    );
    body
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
                avatar_version: 0,
                ..Project::default()
            },
            None => return div(),
        },
        // The draft lives on the Create sheet, never in this sheet.
        Target::Draft => return div(),
        // Me: the account dressed the same way (only the preview reads it, see `preview`).
        Target::Me(_) => match state.account.as_ref() {
            Some(a) => Project {
                id: a.id.clone(),
                name: a.name.clone(),
                image_url: (!a.avatar_url.is_empty()).then(|| a.avatar_url.clone()),
                avatar_version: 0,
                ..Project::default()
            },
            None => return div(),
        },
    };
    let label_muted = match theme.appearance {
        Appearance::Dark => hex(0x9A9A9A),
        Appearance::Light => theme.text_muted,
    };
    // Reset has something to undo: a leaf / colour / picture on an entity; for me, only an
    // Arbos-uploaded picture (the GitHub one is already the default).
    let can_reset = match &picker.target {
        Target::Me(_) => state.has_custom_avatar(),
        Target::Agent(_) | Target::Group(_) | Target::Server(_) | Target::Draft => {
            project.avatar_leaf.is_some()
                || project.avatar_leaf_color.is_some()
                || project.image_url.is_some()
        }
    };
    // View (Jacob, 2026-10-25): the picture as it is now, full-size in the media viewer — only
    // when there is a picture (a leaf / letter avatar has nothing to view).
    let view_picture: Option<(String, String)> = project
        .image_url
        .clone()
        .filter(|u| !u.is_empty())
        .map(|url| (project.name.clone(), url));
    // The Generate / Upload preview, built while `state` is borrowed (the bodies need `cx`).
    let tab_preview = preview(picker, &project, state, &theme);

    // ----- bar: Bot | Generate | Upload …… View · Reset ------------------------------------
    let tabs = tab_pill(&Tab::for_target(&picker.target), picker.tab, &p, &theme, cx)
        .absolute()
        .left(px(BAR_INSET))
        .top(px((BAR_H - TAB_H) / 2.0));
    let reset = div()
        .id("avatar-reset")
        .debug_selector(|| "avatar-reset".into())
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
    let view = view_picture.map(|(name, url)| {
        div()
            .id("avatar-view")
            .debug_selector(|| "avatar-view".into())
            .absolute()
            .right(px(BAR_INSET + 50.0))
            .top(px((BAR_H - TAB_H) / 2.0))
            .h(px(TAB_H))
            .px(px(6.0))
            .flex()
            .items_center()
            .text_size(px(13.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(label_muted)
            .cursor_pointer()
            .hover(|s| s.opacity(0.85))
            .child("View")
            .on_click(cx.listener(move |this, _, _, cx| this.view_picture(&name, &url, cx)))
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
        .children(view)
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
            body.extend(generate_body(
                shell,
                picker,
                Slots {
                    top: BAR_H + 16.0,
                    status_top: FOOTER_TOP - 30.0,
                },
                Some(tab_preview),
                &p,
                &theme,
                cx,
            ));
        }
        Tab::Upload => {
            body.extend(upload_body(
                picker,
                Slots {
                    top: BAR_H + 24.0,
                    status_top: FOOTER_TOP - 30.0,
                },
                Some(tab_preview),
                &p,
                cx,
            ));
        }
    }

    // ----- footer: Cancel | Set avatar -----------------------------------------------------
    let pill_w = (WIDTH - 2.0 * BAR_INSET - PILL_GAP) / 2.0;
    let pill = |id: &'static str, label: &'static str, bg: Hsla, fg: Hsla| {
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
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
            assert_eq!(pk.tab, Tab::Bot);
            pk.reset = true;
            assert!(pk.dirty());
        }
    }

    /// Jacob, 2026-10-25: a Create sheet's draft offers Upload and Generate only — no Bot
    /// tab (leaf grid + swatches), Upload first — and hands the create exactly what was
    /// picked: nothing (→ the neutral initial), the file, or the generated key.
    #[test]
    fn the_draft_offers_upload_and_generate_only_and_hands_over_its_pick() {
        assert_eq!(
            Tab::for_target(&Target::Draft),
            [Tab::Upload, Tab::Generate]
        );
        let mut pk = Picker::new(Target::Draft);
        assert_eq!(pk.tab, Tab::Upload);
        assert_eq!(pk.draft_picture(), None);
        pk.upload = Some(PathBuf::from("/tmp/me.png"));
        assert_eq!(
            pk.draft_picture(),
            Some(crate::sync::DraftPicture::File(PathBuf::from(
                "/tmp/me.png"
            )))
        );
        pk.tab = Tab::Generate;
        assert_eq!(
            pk.draft_picture(),
            None,
            "the other tab's choice is not sent"
        );
        pk.generated = Some(("avatar-gen-1.jpg".into(), "http://x/1".into()));
        assert_eq!(
            pk.draft_picture(),
            Some(crate::sync::DraftPicture::Generated {
                key: "avatar-gen-1.jpg".into()
            })
        );
    }

    /// My own picture (Jacob, 2026-10-25): the same sheet without the leaf tab — Generate is
    /// the landing tab, Upload beside it, Reset in the bar; Set applies a generated key or a
    /// file, Reset means back to the GitHub picture.
    #[test]
    fn my_profile_gets_generate_upload_and_reset_without_a_leaf_tab() {
        let target = Target::Me("gh_1".into());
        assert_eq!(Tab::for_target(&target), [Tab::Generate, Tab::Upload]);
        let mut pk = Picker::new(target);
        assert_eq!(pk.tab, Tab::Generate);
        assert!(!pk.dirty(), "nothing to set until a picture exists");
        pk.generated = Some(("avatar-gen-1.jpg".into(), "fixture://avatar/me-x".into()));
        assert!(pk.dirty());
        pk.tab = Tab::Upload;
        assert!(!pk.dirty(), "the Upload tab applies only a chosen file");
        pk.upload = Some(PathBuf::from("/tmp/me.jpg"));
        assert!(pk.dirty());
        pk.reset = true;
        assert!(pk.dirty());
    }
}

#[cfg(test)]
mod me_picker_tests {
    //! The real `Shell` headless over a real `AppState` with a signed-in account: the my-profile
    //! pane's picture opens the avatar picker with Generate | Upload and Reset (no leaf tab);
    //! Generate sends `GenerateImage { Me }`; the relay's answer previews; Set sends
    //! `SetMyAvatarKey`; Reset + Set sends `RemoveMyAvatar`.
    use futures::future::BoxFuture;
    use gpui::{point, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext};
    use tokio::sync::mpsc;

    use super::*;
    use crate::app::ProfilePage;
    use crate::model::AccountView;
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;

    struct Idle;

    impl SyncSource for Idle {
        fn name(&self) -> &'static str {
            "idle"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            _commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            Box::pin(std::future::pending())
        }
    }

    fn account(avatar_url: &str) -> AccountView {
        AccountView {
            id: "gh_1".into(),
            login: "unarbos".into(),
            name: "Arbos".into(),
            avatar_url: avatar_url.into(),
            device_name: "Test".into(),
            balance_usd: 1.0,
            spent_today_usd: 0.0,
            daily_cap_usd: None,
        }
    }

    fn mount<'a>(
        cx: &'a mut TestAppContext,
        avatar_url: &str,
    ) -> (
        tokio::runtime::Runtime,
        Entity<AppState>,
        Entity<Shell>,
        &'a mut VisualTestContext,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let me = account(avatar_url);
        state.update(cx, |s, cx| {
            s.apply(SyncEvent::Account(Some(me)), cx);
            s.open_profile(ProfilePage::Me, cx);
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.run_until_parked();
        (rt, state, shell, vcx)
    }

    fn settle(shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        for _ in 0..3 {
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    fn centre(b: Bounds<Pixels>) -> gpui::Point<Pixels> {
        point(b.left() + b.size.width / 2.0, b.top() + b.size.height / 2.0)
    }

    fn click(vcx: &mut VisualTestContext, shell: &Entity<Shell>, id: &'static str) {
        let b = vcx
            .debug_bounds(id)
            .unwrap_or_else(|| panic!("{id} is on screen"));
        vcx.simulate_click(centre(b), Modifiers::default());
        settle(shell, vcx);
    }

    #[gpui::test]
    fn my_picture_opens_the_picker_and_generate_then_set_sends_the_key(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx, "");
        let mut commands = state.update(vcx, |s, _| s.take_command_channel());
        settle(&shell, vcx);
        assert!(
            vcx.debug_bounds("avatar-set").is_none(),
            "no sheet before the click"
        );

        // The hero picture on my pane → the picker, landing on Generate, no Bot tab.
        click(vcx, &shell, "me-avatar");
        let picker = shell.read_with(&*vcx, |sh, _| sh.avatar_picker.clone());
        let picker = picker.expect("the picker opened");
        assert_eq!(picker.target, Target::Me("gh_1".into()));
        assert_eq!(picker.tab, Tab::Generate);
        assert!(vcx.debug_bounds("avatar-tab-generate").is_some());
        assert!(vcx.debug_bounds("avatar-tab-upload").is_some());
        assert!(
            vcx.debug_bounds("avatar-tab-bot").is_none(),
            "people have no leaf"
        );
        assert!(vcx.debug_bounds("avatar-reset").is_some());
        assert!(vcx.debug_bounds("avatar-generate-go").is_some());
        assert!(vcx.debug_bounds("avatar-cancel").is_some());
        let sheet_w = vcx.debug_bounds("avatar-set").expect("footer").size.width;
        assert!(f32::from(sheet_w) > 100.0);

        // A prompt, Generate → `GenerateImage { Me, prompt }`; the sheet says it is painting.
        shell.update(vcx, |sh, cx| {
            sh.avatar_prompt_input
                .update(cx, |input, cx| input.set_text("a fox reading".into(), cx));
        });
        click(vcx, &shell, "avatar-generate-go");
        match commands.try_recv() {
            Ok(ClientCommand::GenerateImage { target, prompt }) => {
                assert_eq!(target, Target::Me("gh_1".into()));
                assert_eq!(prompt, "a fox reading");
            }
            other => panic!("expected GenerateImage, got {other:?}"),
        }
        assert!(shell.read_with(&*vcx, |sh, _| sh
            .avatar_picker
            .as_ref()
            .is_some_and(|pk| pk.generating)));

        // The relay's answer lands in the preview; Set → `SetMyAvatarKey { key }`, sheet gone.
        state.update(vcx, |s, cx| {
            s.apply(
                SyncEvent::ImageGenerated {
                    target: Target::Me("gh_1".into()),
                    image_key: Some("avatar-gen-01.jpg".into()),
                    url: Some("https://relay.test/blob/users:gh_1/avatar-gen-01.jpg".into()),
                    error: None,
                },
                cx,
            )
        });
        settle(&shell, vcx);
        let picker = shell.read_with(&*vcx, |sh, _| sh.avatar_picker.clone());
        let picker = picker.expect("still open");
        assert!(!picker.generating);
        assert_eq!(
            picker.generated.as_ref().map(|(k, _)| k.as_str()),
            Some("avatar-gen-01.jpg")
        );
        assert!(picker.dirty());
        click(vcx, &shell, "avatar-set");
        // The preview's own `FetchAvatar` precedes Set on the channel.
        let set = std::iter::from_fn(|| commands.try_recv().ok())
            .find(|c| !matches!(c, ClientCommand::FetchAvatar { .. }));
        match set {
            Some(ClientCommand::SetMyAvatarKey { key }) => assert_eq!(key, "avatar-gen-01.jpg"),
            other => panic!("expected SetMyAvatarKey, got {other:?}"),
        }
        assert!(
            shell.read_with(&*vcx, |sh, _| sh.avatar_picker.is_none()),
            "Set closes the sheet"
        );
        assert!(vcx.debug_bounds("avatar-set").is_none());
    }

    #[gpui::test]
    fn reset_on_an_uploaded_picture_goes_back_to_github(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx, "https://relay.test/avatars/gh_1.jpg?v=1");
        let mut commands = state.update(vcx, |s, _| s.take_command_channel());
        settle(&shell, vcx);
        click(vcx, &shell, "me-avatar");
        assert!(shell.read_with(&*vcx, |sh, _| sh.avatar_picker.is_some()));
        // Reset is live (an Arbos picture is set), then Set → `RemoveMyAvatar`.
        click(vcx, &shell, "avatar-reset");
        assert!(shell.read_with(&*vcx, |sh, _| sh
            .avatar_picker
            .as_ref()
            .is_some_and(|pk| pk.reset && pk.dirty())));
        click(vcx, &shell, "avatar-set");
        match commands.try_recv() {
            Ok(ClientCommand::RemoveMyAvatar) => {}
            other => panic!("expected RemoveMyAvatar, got {other:?}"),
        }
        assert!(shell.read_with(&*vcx, |sh, _| sh.avatar_picker.is_none()));

        // Cancel closes without a command.
        click(vcx, &shell, "me-avatar");
        click(vcx, &shell, "avatar-cancel");
        assert!(shell.read_with(&*vcx, |sh, _| sh.avatar_picker.is_none()));
        assert!(commands.try_recv().is_err(), "Cancel sends nothing");
    }

    /// Jacob, 2026-10-25: on my own picture the picker keeps its place, and gains **View**
    /// beside Reset when there is a picture — it opens the same media viewer chat photos use
    /// (Close only: no Download / Forward for a profile picture). Initials offer no View.
    #[gpui::test]
    fn view_in_the_picker_opens_my_picture_in_the_media_viewer(cx: &mut TestAppContext) {
        let url = "https://relay.test/avatars/gh_1.jpg?v=1";
        let (_rt, state, shell, vcx) = mount(cx, url);
        settle(&shell, vcx);
        click(vcx, &shell, "me-avatar");
        assert!(
            vcx.debug_bounds("avatar-view").is_some(),
            "a picture → View"
        );
        click(vcx, &shell, "avatar-view");
        assert!(
            shell.read_with(&*vcx, |sh, _| sh.avatar_picker.is_none()),
            "View closes the picker"
        );
        let viewer = state.read_with(&*vcx, |s, _| s.media_viewer.clone());
        let viewer = viewer.expect("the media viewer is up");
        assert_eq!(viewer.items.len(), 1);
        assert_eq!(viewer.items[0].picture_url.as_deref(), Some(url));
        assert_eq!(viewer.items[0].by, "Arbos");
        assert!(vcx.debug_bounds("media-viewer-close-pill").is_some());
        assert!(
            vcx.debug_bounds("media-viewer-download").is_none()
                && vcx.debug_bounds("media-viewer-forward").is_none(),
            "a profile picture has no Download / Forward"
        );
        click(vcx, &shell, "media-viewer-close-pill");
        assert!(state.read_with(&*vcx, |s, _| s.media_viewer.is_none()));

        // Initials only: no View in the bar.
        state.update(vcx, |s, cx| {
            s.apply(SyncEvent::Account(Some(account(""))), cx)
        });
        settle(&shell, vcx);
        click(vcx, &shell, "me-avatar");
        assert!(shell.read_with(&*vcx, |sh, _| sh.avatar_picker.is_some()));
        assert!(vcx.debug_bounds("avatar-view").is_none(), "nothing to view");
    }

    /// Jacob, 2026-10-25: on someone else's pane a click on their picture opens it full-size in
    /// the media viewer; a person with initials only has nothing to open.
    #[gpui::test]
    fn another_persons_picture_opens_the_viewer_on_a_click(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx, "");
        let person = |handle: &str, url: &str| crate::model::Person {
            id: format!("gh_{handle}"),
            handle: handle.into(),
            name: Some("Sam Rivera".into()),
            avatar_url: url.into(),
            status: "member".into(),
            role: String::new(),
            last_seen_at: None,
            online: false,
            avatar_color_index: None,
        };
        state.update(vcx, |s, cx| {
            s.people_results = vec![
                person("sam", "https://relay.test/avatars/gh_sam.jpg?v=2"),
                person("kim", ""),
            ];
            s.open_profile(
                ProfilePage::User {
                    handle: "sam".into(),
                    space_id: None,
                },
                cx,
            );
        });
        settle(&shell, vcx);
        click(vcx, &shell, "profile-avatar");
        let viewer = state.read_with(&*vcx, |s, _| s.media_viewer.clone());
        let viewer = viewer.expect("Sam's picture opens the viewer");
        assert_eq!(
            viewer.items[0].picture_url.as_deref(),
            Some("https://relay.test/avatars/gh_sam.jpg?v=2")
        );
        assert_eq!(viewer.items[0].by, "Sam Rivera");
        assert!(
            vcx.debug_bounds("media-viewer-close").is_some(),
            "the × is up"
        );
        click(vcx, &shell, "media-viewer-close");
        assert!(state.read_with(&*vcx, |s, _| s.media_viewer.is_none()));

        // Kim has initials only: the hero is not a button.
        state.update(vcx, |s, cx| {
            s.open_profile(
                ProfilePage::User {
                    handle: "kim".into(),
                    space_id: None,
                },
                cx,
            )
        });
        settle(&shell, vcx);
        click(vcx, &shell, "profile-avatar");
        assert!(
            state.read_with(&*vcx, |s, _| s.media_viewer.is_none()),
            "initials open nothing"
        );
    }
}
