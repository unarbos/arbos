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
//! - **Upload**: the image-only file panel (PNG, JPEG, HEIC, GIF, WebP), uploaded as the
//!   picture master (≤ 1024 px; PNG for a flat mark, JPEG q92 for a photo) into the project's
//!   blob slot on Set.
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
//! The one line in a Generate / Upload body whose height is its text's — the Upload hint, the
//! Generate status — is in the card's flow between the body's box and the footer, never in an
//! absolute slot inside the box ([`hint_line`], Affinity 2026-10-31).
//!
//! Sizes are Grok's, measured on the 2x reference (612 × 794 px → 306 × 397 pt): the bar 44 pt
//! with a hairline, 32 pt icons on a 57 pt pitch (we keep the Create sheet's 33 / 40 / 56 so the
//! two sheets agree), 23 pt swatches, two equal 28 pt footer pills 8 pt apart.

use std::path::PathBuf;

use gpui::{
    div, prelude::*, px, Context, Div, ElementId, FontWeight, Hsla, MouseButton, SharedString,
    Stateful,
};

use crate::app::AppState;
use crate::model::Project;
use crate::sync::user_error::UserError;
use crate::ui::avatars::{self, leaf, LeafPick};
use crate::ui::create_entity::{hex, palette, Palette};
use crate::ui::error_note;
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
/// The body box between the bar and the footer (the tab bodies' absolute slots live in it).
const BODY_H: f32 = HEIGHT - BAR_H - FOOTER_H;
/// A Generate / Upload body's box: the body less [`HINT_RESERVED`], because the line under it
/// is in the card's flow, not in the box (see [`hint_line`]).
const TAB_BODY_H: f32 = BODY_H - HINT_RESERVED;
const PILL_H: f32 = 28.0;
const PILL_GAP: f32 = 8.0;
const PREVIEW: f32 = 114.0;
const FIELD_INSET: f32 = 16.5;
const FIELD_H: f32 = 35.0;
/// One row of the hint line, at 12 / 16.
const HINT_LINE_H: f32 = 16.0;
/// The least space between the hint line's last row and the footer's hairline.
const HINT_GAP: f32 = 14.0;
/// The band this sheet keeps between the body's box and the hairline for the hint line: **two**
/// rows and the gap, because the Upload tab's "PNG, JPEG, HEIC, GIF or WebP; kept at up to 1024
/// px." takes two at 306 pt. It is a floor, not a cap — a line that needs a third row grows the
/// band, the card and the footer with it — and a floor of two keeps the card at Grok's 397 and
/// the footer still while the tab changes.
const HINT_RESERVED: f32 = 2.0 * HINT_LINE_H + HINT_GAP;

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
    /// When the running generation started (the status line counts the seconds).
    pub started_at: Option<std::time::Instant>,
    /// The last failure, drawn by the shared error element at the top of the body (Jacob,
    /// 2026-10-26, [feedback error-messages-mac-2026-10-26]): one sentence, Try again, Details.
    pub error: Option<error_note::Slot>,
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
            started_at: None,
            error: None,
        }
    }

    /// The generation failed: the note goes up, folded.
    pub fn fail(&mut self, error: UserError) {
        self.error = Some(error_note::Slot::new(error));
    }

    /// Seconds since Generate was pressed, while it runs.
    pub fn generating_for_s(&self) -> Option<u64> {
        self.generating
            .then(|| self.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0))
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
        // The chosen file goes through the avatar loader as `file://…` (decoded off the UI
        // thread into the ≤ 1024 px master the upload will send), then through `picture_fit`
        // like every other avatar. Handing GPUI the raw path drew a 4000 px texture into a
        // 114 pt disc with no mipmaps — Jacob's stair-stepped edges (2026-10-26). Until the
        // master is ready the frame is empty for a moment, never the aliased original.
        let image = state.avatar(&file_url(path));
        return Some(frame().children(image.map(|image| {
            gpui::img(crate::ui::picture_fit::fit(&image, size))
                .id("avatar-upload-preview")
                .debug_selector(|| "avatar-upload-preview".into())
                .size(px(size))
                .rounded_full()
                .object_fit(gpui::ObjectFit::Cover)
        })));
    }
    if let Some((_, url)) = &picker.generated {
        if let Some(image) = state.avatar(url) {
            return Some(
                frame().child(
                    gpui::img(crate::ui::picture_fit::fit(&image, size))
                        .size(px(size))
                        .rounded_full()
                        .object_fit(gpui::ObjectFit::Cover),
                ),
            );
        }
    }
    None
}

/// The avatar-cache URL of a picked file: `file://<path>`; the sync client reads and masters it
/// (`sync::avatar_master`) and answers with `SyncEvent::Avatar` like a fetched picture.
pub(crate) fn file_url(path: &std::path::Path) -> String {
    format!("file://{}", path.display())
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
                // hover-fill: exception choice
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

/// Which end of its band the hint line sits at — the anchor each sheet already had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HintAnchor {
    /// The picker sheet: the line is placed from the footer up (it was `BODY_H - 30`), so its
    /// last row stays `gap` over the hairline whatever the row count.
    Footer,
    /// The Create sheets: the line is placed from the body down (12 under the "Choose a photo…"
    /// pill), and the band's spare room is under it.
    Body,
}

/// The band a sheet keeps for its hint line between the body's box and the footer's hairline
/// (see [`hint_line`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct HintSlot {
    /// The band's floor: what the sheet already kept there, so a line that fits leaves the
    /// sheet's measured height exactly as it was.
    pub reserved: f32,
    /// The least space under the line's last row.
    pub gap: f32,
    /// Which end of the band the line sits at.
    pub anchor: HintAnchor,
}

impl HintSlot {
    /// The picker sheet's band: [`HINT_RESERVED`], the line hanging off the footer.
    const SHEET: HintSlot = HintSlot {
        reserved: HINT_RESERVED,
        gap: HINT_GAP,
        anchor: HintAnchor::Footer,
    };
}

/// The line under a Generate / Upload body — the Generate status, the Upload hint — **in the
/// card's flow**, between the body's box and the footer, never in an absolute slot inside the
/// box. It is the one element in these sheets whose height is its text's: "PNG, JPEG, HEIC, GIF
/// or WebP; kept at up to 1024 px." takes two rows at the picker's width, and from a fixed box
/// the second row drew over the footer's hairline ([feedback
/// avatar-upload-hint-divider-mac-2026-10-31], Affinity: "1024 px." sitting on the divider). In
/// flow the band holds it and a longer line grows the card, the way `error_slot`'s note already
/// grows it — the sheet rule is that content above the footer takes space and is never drawn
/// over it.
///
/// `slot.reserved` is a floor, not a height: while the line fits in the band the sheet measures
/// exactly as it did before, and `slot.gap` keeps the last row off the hairline either way.
pub(crate) fn hint_line(
    id: &'static str,
    text: SharedString,
    slot: HintSlot,
    p: &Palette,
) -> Stateful<Div> {
    let line = format!("{id}-line");
    let selector = line.clone();
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .flex_none()
        .w_full()
        .min_h(px(slot.reserved))
        .px(px(FIELD_INSET))
        .pb(px(slot.gap))
        // A flex column — `div()` is a block, which would ignore the alignment below — so the
        // band's spare room goes on the vertical: the line keeps its own rows at the end the
        // sheet anchors it to, and so stays where it has always been.
        .flex()
        .flex_col()
        .map(|el| match slot.anchor {
            HintAnchor::Footer => el.justify_end(),
            HintAnchor::Body => el.justify_start(),
        })
        .child(
            div()
                .id(SharedString::from(line))
                .debug_selector(move || selector.clone())
                .w_full()
                .text_size(px(12.0))
                .line_height(px(HINT_LINE_H))
                .text_center()
                .text_color(p.placeholder)
                .child(text),
        )
}

/// The Generate tab's body, absolutely placed in a sheet: the prompt field at `top`, the
/// Generate pill under it, and `preview` (when given) under that. Shared by the picker sheet
/// and the Create sheets; the status line is [`generate_status`] in the sheet's [`hint_line`].
pub(crate) fn generate_body(
    shell: &Shell,
    picker: &Picker,
    top: f32,
    preview: Option<Div>,
    p: &Palette,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<gpui::AnyElement> {
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
    // The preview slot: the picture, dimmed under a spinner while painting. A failure is the
    // shared note at the top of the body (`error_slot`), never anything near the footer.
    let slot_top = pill_top + PILL_H + 16.0;
    if let Some(preview) = preview {
        let painting = picker.generating;
        body.push(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(slot_top))
                .flex()
                .justify_center()
                .child(
                    div()
                        .relative()
                        .size(px(PREVIEW))
                        .child(
                            div()
                                .size(px(PREVIEW))
                                .when(painting, |d| d.opacity(0.4))
                                .child(preview),
                        )
                        .when(painting, |d| {
                            d.child(
                                div()
                                    .id("avatar-generate-spinner")
                                    .debug_selector(|| "avatar-generate-spinner".into())
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(crate::ui::cards::activity_spinner(28.0, theme, cx)),
                            )
                        }),
                )
                .into_any_element(),
        );
    }
    body
}

/// What the Generate tab's line says — progress, the keep hint, or the prompt hint. Never a
/// failure: that is the note at the top of the body (`error_slot`).
pub(crate) fn generate_status(picker: &Picker, cx: &mut Context<Shell>) -> SharedString {
    if picker.generating {
        // The seconds tick while it paints.
        crate::ui::motion::PulseClock::request(cx);
    }
    let keeps = match picker.target {
        Target::Draft => "Create keeps this one.",
        Target::Agent(_) | Target::Group(_) | Target::Server(_) | Target::Me(_) => {
            "Set avatar keeps this one."
        }
    };
    if let Some(s) = picker.generating_for_s() {
        format!("Painting a picture — {s} s").into()
    } else if picker.generated.is_some() {
        keeps.into()
    } else {
        "A few words about the picture you want.".into()
    }
}

/// The picker's note (the shared error element) for the top of a sheet's body: inset like the
/// field, `GAP` above and below. Try again re-sends the prompt; Details unfolds in place. The
/// sheet grows by it — the body's absolute slots are unchanged, so nothing can meet the footer.
pub(crate) fn error_slot(picker: &Picker, theme: &Theme, cx: &mut Context<Shell>) -> Option<Div> {
    let slot = picker.error.as_ref()?;
    let note = error_note::render(
        "avatar-generate-error",
        &slot.error,
        slot.details_open,
        error_note::Style::Inline,
        theme,
        error_note::Handlers::new()
            .try_again(cx.listener(|this, _, _, cx| this.generate_avatar(cx)))
            .toggle_details(cx.listener(|this, _, _, cx| {
                if let Some(slot) = this.pending_picker_mut().and_then(|pk| pk.error.as_mut()) {
                    slot.toggle_details();
                }
                cx.notify();
            })),
    );
    Some(
        div()
            .px(px(FIELD_INSET))
            .pt(px(error_note::GAP))
            .flex_none()
            .child(note),
    )
}

/// The Upload tab's body: `preview` (when given) at `top` with the "Choose a photo…" pill under
/// it — else the pill at `top`. Shared by the picker sheet and the Create sheets; the hint is
/// [`upload_hint`] in the sheet's [`hint_line`].
pub(crate) fn upload_body(
    picker: &Picker,
    top: f32,
    preview: Option<Div>,
    p: &Palette,
    cx: &mut Context<Shell>,
) -> Vec<gpui::AnyElement> {
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
    body
}

/// What the Upload tab's line says: the chosen file's name, else the formats and the cap.
pub(crate) fn upload_hint(picker: &Picker) -> SharedString {
    match &picker.upload {
        Some(path) => path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("picture")
            .to_owned()
            .into(),
        None => "PNG, JPEG, HEIC, GIF or WebP; kept at up to 1024 px.".into(),
    }
}

/// The target's picture as it is now — `(name, url)` — what the bar's View opens; `None` for a
/// leaf / letter avatar (nothing to view) or an unknown target.
pub(crate) fn viewable_picture(picker: &Picker, state: &AppState) -> Option<(String, String)> {
    let (name, url) = match &picker.target {
        Target::Agent(id) | Target::Group(id) => {
            let p = state.projects.iter().find(|x| &x.id == id)?;
            (p.name.clone(), p.image_url.clone()?)
        }
        Target::Server(id) => {
            let s = state.servers.iter().find(|s| &s.id == id)?;
            (s.name.clone(), s.image_url.clone()?)
        }
        Target::Me(_) => {
            let a = state.account.as_ref()?;
            (a.name.clone(), a.avatar_url.clone())
        }
        Target::Draft => return None,
    };
    (!url.is_empty()).then_some((name, url))
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
    let view_picture = viewable_picture(picker, state);
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
        .relative()
        .flex_none()
        .w_full()
        .h(px(BAR_H))
        .border_b_1()
        .border_color(p.line)
        .child(tabs)
        .children(view)
        .child(reset);
    // The note (a failed generation) sits between the bar and the body; the card grows by it.
    let note = error_slot(picker, &theme, cx);

    // ----- body ---------------------------------------------------------------------------------
    let mut body: Vec<gpui::AnyElement> = Vec::new();
    // The Generate / Upload line, in the card's flow under the body's box (see `hint_line`);
    // the Bot tab has none, so its box keeps the whole body.
    let mut hint: Option<Stateful<Div>> = None;
    match picker.tab {
        Tab::Bot => {
            let pick = picker.pick(&project);
            for (r, top) in LEAF_ROWS.iter().enumerate() {
                let leaves: Vec<u8> = (r as u8 * 4..r as u8 * 4 + 4).collect();
                let mut row = div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top - BAR_H - LEAF_RING / 2.0))
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
                    .top(px(top - BAR_H - SWATCH_RING / 2.0))
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
                16.0,
                Some(tab_preview),
                &p,
                &theme,
                cx,
            ));
            hint = Some(hint_line(
                "avatar-generate-status",
                generate_status(picker, cx),
                HintSlot::SHEET,
                &p,
            ));
        }
        Tab::Upload => {
            body.extend(upload_body(picker, 24.0, Some(tab_preview), &p, cx));
            hint = Some(hint_line(
                "avatar-upload-hint",
                upload_hint(picker),
                HintSlot::SHEET,
                &p,
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
        .relative()
        .flex_none()
        .w_full()
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

    // A column: bar · note (when a generation failed) · the body's fixed box · the hint line's
    // band (Generate / Upload) · footer. Without a note, and with a line the band holds, the
    // card is exactly `HEIGHT`; a note or a longer line grows it, never the other way.
    let card = div()
        .id("avatar-card")
        .debug_selector(|| "avatar-card".into())
        .relative()
        .w(px(WIDTH))
        .max_w_full()
        .max_h_full()
        .flex()
        .flex_col()
        .rounded(px(RADIUS))
        .bg(p.bg)
        .border_1()
        .border_color(p.line)
        .shadow_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(bar)
        .children(note)
        .child(
            div()
                .id("avatar-body")
                .debug_selector(|| "avatar-body".into())
                .relative()
                .flex_none()
                .w_full()
                .h(px(if hint.is_some() { TAB_BODY_H } else { BODY_H }))
                .children(body),
        )
        .children(hint)
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

    /// Element ids built at run time, for `debug_bounds` (which wants `'static`).
    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

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

    /// [feedback error-messages-mac-2026-10-26]: a `Failure` event with nobody to claim it is an
    /// error toast — the shared element top-right under the header: the sentence, Try again
    /// (when the event carries a command), Details that unfold the mono block + Copy, a × that
    /// dismisses — never a plain card with the raw words. An info `Notice` stays the plain card.
    #[gpui::test]
    fn a_failure_event_is_an_error_toast_with_details_try_again_and_close(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx, "");
        let mut commands = state.update(vcx, |s, _| s.take_command_channel());
        settle(&shell, vcx);
        let raw = anyhow::anyhow!(
            "error sending request for url (https://relay.arbos.life/blobs/x): operation timed out"
        )
        .context("GET /blobs/01M5N1WSR91G8CJKNYXKJESK7X/report-q3.pdf");
        let error = UserError::classify("download report-q3.pdf", &raw);
        state.update(vcx, |s, cx| {
            s.apply(SyncEvent::Notice("Forwarded to Studio".into()), cx);
            s.apply(
                SyncEvent::Failure {
                    error,
                    retry: Some(Box::new(ClientCommand::DevicesRefresh)),
                },
                cx,
            );
        });
        settle(&shell, vcx);
        let at = state.read_with(&*vcx, |s, _| {
            s.live_notices()
                .iter()
                .find(|n| n.error.is_some())
                .map(|n| n.id)
                .expect("the error toast is live")
        });
        let id: &'static str = leak(format!("notice-{at}"));
        let toast = vcx
            .debug_bounds(id)
            .expect("the toast is the shared element");
        let sentence = vcx
            .debug_bounds(leak(format!("{id}-sentence")))
            .expect("sentence");
        // The one slot (Jacob 2026-10-27): just below the header pill, centred on the
        // chat column — not down the right edge.
        let slot = vcx.debug_bounds("toast-slot").expect("the slot");
        assert!(
            (f32::from(slot.top()) - crate::ui::header::metrics::TOAST_SLOT_TOP).abs() < 1.0,
            "under the pill: {slot:?}"
        );
        assert!(
            toast.top() > slot.top(),
            "the info card above, the failure under it"
        );
        let width = f32::from(vcx.update(|window, _| window.viewport_size().width));
        let sidebar = state.read_with(&*vcx, |s, _| s.sidebar_width);
        assert!(
            f32::from(toast.left()) > sidebar + 40.0 && f32::from(toast.right()) < width - 40.0,
            "centred on the chat column, not pinned to the right edge: {toast:?} in {width}"
        );
        assert!(
            (f32::from(toast.size.width) - error_note::TOAST_WIDTH).abs() < 1.0,
            "toast width {toast:?}"
        );
        let _ = sentence;
        assert!(
            vcx.debug_bounds(leak(format!("{id}-try-again"))).is_some(),
            "Try again"
        );
        assert!(
            vcx.debug_bounds(leak(format!("{id}-close"))).is_some(),
            "a ×"
        );
        assert!(vcx
            .debug_bounds(leak(format!("{id}-details-block")))
            .is_none());

        // Details unfolds the mono block with Copy; the toast is held while open.
        let details = leak(format!("{id}-details"));
        click(vcx, &shell, details);
        assert!(vcx
            .debug_bounds(leak(format!("{id}-details-block")))
            .is_some());
        assert!(
            vcx.debug_bounds(leak(format!("{id}-copy"))).is_some(),
            "Copy"
        );
        assert!(state.read_with(&*vcx, |s, _| s
            .live_notices()
            .iter()
            .any(|n| n.id == at && n.held && n.details_open)));

        // Try again re-issues the command and drops the toast.
        let try_again = leak(format!("{id}-try-again"));
        click(vcx, &shell, try_again);
        assert!(matches!(
            commands.try_recv(),
            Ok(ClientCommand::DevicesRefresh)
        ));
        assert!(vcx.debug_bounds(id).is_none(), "the toast is gone");

        // The info notice is still the plain card and never a red block.
        assert!(state.read_with(&*vcx, |s, _| s
            .live_notices()
            .iter()
            .all(|n| n.error.is_none() && n.text == "Forwarded to Studio")));

        // × on a fresh error toast dismisses it.
        state.update(vcx, |s, cx| {
            s.apply(
                SyncEvent::Failure {
                    error: UserError::from_text("save the secret", "HTTP 502"),
                    retry: None,
                },
                cx,
            );
        });
        settle(&shell, vcx);
        let at2 = state.read_with(&*vcx, |s, _| {
            s.live_notices()
                .iter()
                .find(|n| n.error.is_some())
                .map(|n| n.id)
                .expect("second toast")
        });
        // No command to re-issue → no Try again (the action's own button is the retry).
        assert!(vcx
            .debug_bounds(leak(format!("notice-{at2}-try-again")))
            .is_none());
        assert!(vcx
            .debug_bounds(leak(format!("notice-{at2}-details")))
            .is_some());
        let close = leak(format!("notice-{at2}-close"));
        click(vcx, &shell, close);
        assert!(vcx.debug_bounds(leak(format!("notice-{at2}"))).is_none());
    }

    /// [feedback toast-stack-mac-2026-10-27] (Jacob: eight identical "Couldn't load your
    /// secrets. You're offline." toasts down the right side — "this must never happen"): the
    /// same failure again while its toast stands is one toast with a ×N; distinct failures
    /// beyond three fold into one "N more" line whose × clears the slot; a held toast is let
    /// go after the hold cap.
    #[gpui::test]
    fn the_same_failure_eight_times_is_one_toast_and_the_slot_shows_at_most_three(
        cx: &mut TestAppContext,
    ) {
        let (_rt, state, shell, vcx) = mount(cx, "");
        settle(&shell, vcx);
        let offline = || {
            UserError::classify(
                "load your secrets",
                &anyhow::anyhow!(
                    "error sending request for url (https://relay.arbos.life/secrets): dns error: failed to lookup address information"
                )
                .context("GET /secrets"),
            )
        };
        state.update(vcx, |s, cx| {
            for _ in 0..8 {
                s.apply(
                    SyncEvent::Failure {
                        error: offline(),
                        retry: None,
                    },
                    cx,
                );
            }
        });
        settle(&shell, vcx);
        let live = state.read_with(&*vcx, |s, _| s.live_notices());
        assert_eq!(live.len(), 1, "one toast, not eight: {live:?}");
        assert_eq!(live[0].count, 8);
        assert_eq!(live[0].text, "Couldn't load your secrets. You're offline.");
        let id: &'static str = leak(format!("notice-{}", live[0].id));
        assert!(vcx.debug_bounds(id).is_some());
        assert!(
            vcx.debug_bounds(leak(format!("{id}-count"))).is_some(),
            "the ×8 badge"
        );

        // Four more distinct failures: three toasts visible (the newest), the rest folded.
        state.update(vcx, |s, cx| {
            for verb in [
                "save the secret",
                "load the voices",
                "download report.pdf",
                "join the call",
            ] {
                s.apply(
                    SyncEvent::Failure {
                        error: UserError::from_text(verb, "HTTP 502"),
                        retry: None,
                    },
                    cx,
                );
            }
        });
        settle(&shell, vcx);
        let stack = state.read_with(&*vcx, |s, _| s.notice_stack());
        assert_eq!(stack.visible.len(), crate::app::NOTICE_VISIBLE_MAX);
        assert_eq!(stack.folded, 2);
        assert_eq!(
            stack.visible.last().map(|n| n.text.as_str()),
            Some("Couldn't join the call. The server had a problem."),
            "newest at the bottom"
        );
        let folded = vcx.debug_bounds("toast-folded").expect("the N more line");
        let newest = vcx
            .debug_bounds(leak(format!("notice-{}", stack.visible[2].id)))
            .expect("newest toast");
        assert!(
            folded.top() < newest.top(),
            "the fold line sits above the stack"
        );
        assert!(
            vcx.debug_bounds(id).is_none(),
            "the oldest (secrets) toast is folded away"
        );
        // Three toasts and the fold line fit under the pill without reaching the composer.
        assert!(
            newest.bottom() < vcx.update(|window, _| window.viewport_size().height) - px(120.0),
            "the stack stays out of the composer's way: {newest:?}"
        );

        // × on the fold line clears the slot.
        click(vcx, &shell, "toast-folded-close");
        assert!(state.read_with(&*vcx, |s, _| s.live_notices().is_empty()));
        assert!(vcx.debug_bounds("toast-slot").is_none());
    }

    #[test]
    fn a_held_toast_is_let_go_after_the_hold_cap_unless_details_are_open() {
        let mut n = crate::app::Notice {
            id: 1,
            text: "x".into(),
            at: 1_000,
            error: Some(UserError::from_text("load your secrets", "HTTP 502")),
            details_open: false,
            held: true,
            held_since: 1_000,
            retry: None,
            count: 1,
        };
        assert!(
            n.live_at(1_000 + crate::app::ERROR_NOTICE_TTL_MS + 1),
            "held: past the TTL"
        );
        assert!(
            !n.live_at(1_000 + crate::app::NOTICE_HOLD_MAX_MS + 1),
            "a hold without Details ends at the cap"
        );
        n.details_open = true;
        assert!(
            n.live_at(1_000 + crate::app::NOTICE_HOLD_MAX_MS + 1),
            "Details open: stays"
        );
        n.held = false;
        assert!(!n.live_at(1_000 + crate::app::ERROR_NOTICE_TTL_MS + 1));
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

    /// [feedback avatar-upload-hint-divider-mac-2026-10-31] (Affinity, on the Mac): the Upload
    /// tab's "PNG, JPEG, HEIC, GIF or WebP; kept at up to 1024 px." wrapped, and its second row
    /// — "1024 px." — was drawn over the footer's hairline. The line sits in the card's flow
    /// now, in a band the sheet keeps for it: the two rows fit with the gap clear and the sheet
    /// keeps Grok's height, a longer line grows the card instead of crossing the hairline, and
    /// the Generate tab's status is the same element in the same band, one row, unmoved.
    #[gpui::test]
    fn a_wrapped_hint_grows_the_sheet_instead_of_crossing_the_footer(cx: &mut TestAppContext) {
        let (_rt, _state, shell, vcx) = mount(cx, "");
        settle(&shell, vcx);
        click(vcx, &shell, "me-avatar");
        // `line` is the text's own box inside the band; the hairline is the footer's top edge.
        let measure = |vcx: &mut VisualTestContext, which: &'static str| {
            let card = vcx.debug_bounds("avatar-card").expect("card");
            let line = vcx
                .debug_bounds(which)
                .unwrap_or_else(|| panic!("{which} is on screen"));
            let hairline = card.bottom() - px(FOOTER_H);
            (
                f32::from(card.size.height),
                f32::from(line.size.height),
                f32::from(hairline - line.bottom()),
            )
        };
        // Grok's 397, plus the card's own 1 pt border top and bottom. How many rows a string
        // takes is the platform's font metrics, so the sheet's height is asserted against the
        // band instead: exactly `want` while the rows fit it, `want` plus the overflow when
        // they do not. That holds for any string on any metrics.
        let want = HEIGHT + 2.0;
        let band = |rows: f32| want + (rows + HINT_GAP - HINT_RESERVED).max(0.0);
        for (tab, line) in [
            // The reported line: two rows at this width, inside the band — the second row no
            // longer reaches the hairline under it.
            ("avatar-tab-upload", "avatar-upload-hint-line"),
            ("avatar-tab-generate", "avatar-generate-status-line"),
        ] {
            click(vcx, &shell, tab);
            let (height, rows, clear) = measure(vcx, line);
            assert!(
                rows >= HINT_LINE_H - 1.0,
                "{tab}: the line is drawn with its own rows: {rows}"
            );
            assert!(
                (height - band(rows)).abs() < 1.5,
                "{tab}: {rows} of line leaves the sheet at {}, measured {height}",
                band(rows)
            );
            assert!(
                clear >= HINT_GAP - 1.0,
                "{tab}: the line is {clear} over the hairline, not {HINT_GAP}"
            );
        }

        // A line of one row hangs off the footer, where the old absolute slot put it: the band
        // is two rows deep so the sheet does not jump between tabs, and its spare row is over
        // the line, not under it (`HintAnchor::Footer`). Without the band's `flex()` the
        // alignment is dropped — `div()` is a block — and the row drifts up by the spare row.
        click(vcx, &shell, "avatar-tab-upload");
        shell.update(vcx, |sh, _| {
            if let Some(pk) = sh.avatar_picker.as_mut() {
                pk.upload = Some(PathBuf::from("/tmp/a.png"));
            }
        });
        settle(&shell, vcx);
        let (height, rows, clear) = measure(vcx, "avatar-upload-hint-line");
        assert!(
            (height - want).abs() < 1.5 && (rows - HINT_LINE_H).abs() < 1.5,
            "one row of line leaves the sheet at {want}: measured {height} for {rows} of line"
        );
        assert!(
            (clear - HINT_GAP).abs() < 1.5,
            "one row of line sits {HINT_GAP} over the hairline, not {clear}"
        );

        // A chosen file's name is the same line, and a name too long for the band is what the
        // next format or translation would be: the card grows by the rows the band cannot
        // hold, the footer goes down with it, and the gap over the hairline holds.
        click(vcx, &shell, "avatar-tab-upload");
        shell.update(vcx, |sh, _| {
            if let Some(pk) = sh.avatar_picker.as_mut() {
                pk.upload = Some(PathBuf::from(
                    "/tmp/the whole team at the offsite in lisbon on a long evening in late \
                     autumn with the sea behind us and nobody looking at the camera 2026.heic",
                ));
            }
        });
        settle(&shell, vcx);
        let (grown, rows, clear) = measure(vcx, "avatar-upload-hint-line");
        assert!(
            rows > HINT_RESERVED - HINT_GAP + 1.0,
            "the long file name takes more rows than the band holds: {rows}"
        );
        assert!(
            (grown - band(rows)).abs() < 1.5,
            "the card grew by those rows only: {} expected, measured {grown}",
            band(rows)
        );
        assert!(
            grown > want + 1.0,
            "and it did grow: {want} → {grown} for {rows} of line"
        );
        assert!(
            clear >= HINT_GAP - 1.0,
            "the wrapped line is {clear} over the hairline, not {HINT_GAP}"
        );
        let cancel = vcx.debug_bounds("avatar-cancel").expect("Cancel");
        let set = vcx.debug_bounds("avatar-set").expect("Set avatar");
        let line = vcx
            .debug_bounds("avatar-upload-hint-line")
            .expect("the line");
        assert!(
            line.bottom() <= cancel.top() && line.bottom() <= set.top(),
            "nothing reaches the footer's pills: {line:?} {cancel:?}"
        );
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
        // Some state in the picker: a typed prompt and a generated preview.
        shell.update(vcx, |sh, cx| {
            sh.avatar_prompt_input
                .update(cx, |input, cx| input.set_text("a fox".into(), cx));
            if let Some(pk) = sh.avatar_picker.as_mut() {
                pk.generated = Some(("avatar-gen-9.jpg".into(), "fixture://avatar/me-9".into()));
            }
        });
        settle(&shell, vcx);
        click(vcx, &shell, "avatar-view");
        // Jacob, 2026-10-26: the picker stays underneath — the viewer is on top of it, and
        // closing the viewer returns to the picker with its state intact.
        assert!(
            shell.read_with(&*vcx, |sh, _| sh.avatar_picker.is_some()),
            "View keeps the picker up"
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
        let viewer_b = vcx.debug_bounds("media-viewer").expect("viewer root");
        let picker_b = vcx.debug_bounds("avatar-set").expect("picker footer");
        assert!(
            viewer_b.contains(&centre(picker_b)),
            "the viewer covers the picker: {viewer_b:?} over {picker_b:?}"
        );
        // The viewer's × closes the viewer only.
        click(vcx, &shell, "media-viewer-close");
        assert!(state.read_with(&*vcx, |s, _| s.media_viewer.is_none()));
        let picker = shell.read_with(&*vcx, |sh, _| sh.avatar_picker.clone());
        let picker = picker.expect("the picker is still up after the viewer's ×");
        assert_eq!(picker.tab, Tab::Generate);
        assert_eq!(
            picker.generated.as_ref().map(|(k, _)| k.as_str()),
            Some("avatar-gen-9.jpg"),
            "the preview survived"
        );
        assert_eq!(
            shell.read_with(&*vcx, |sh, cx| sh
                .avatar_prompt_input
                .read(cx)
                .text()
                .to_owned()),
            "a fox",
            "the typed prompt survived"
        );
        assert!(
            vcx.debug_bounds("avatar-set").is_some(),
            "the sheet is drawn again"
        );
        // Esc on the viewer: the same — viewer gone, picker stays; a second Esc is not sent.
        click(vcx, &shell, "avatar-view");
        assert!(state.read_with(&*vcx, |s, _| s.media_viewer.is_some()));
        vcx.dispatch_action(crate::ui::shell::Escape);
        settle(&shell, vcx);
        assert!(state.read_with(&*vcx, |s, _| s.media_viewer.is_none()));
        assert!(shell.read_with(&*vcx, |sh, _| sh.avatar_picker.is_some()));
        // The Close pill too; then Cancel closes the picker as always.
        click(vcx, &shell, "avatar-view");
        click(vcx, &shell, "media-viewer-close-pill");
        assert!(state.read_with(&*vcx, |s, _| s.media_viewer.is_none()));
        assert!(shell.read_with(&*vcx, |sh, _| sh.avatar_picker.is_some()));
        click(vcx, &shell, "avatar-cancel");
        assert!(shell.read_with(&*vcx, |sh, _| sh.avatar_picker.is_none()));

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
        // The pane underneath is untouched by the viewer's × (Jacob, 2026-10-26).
        assert_eq!(
            state.read_with(&*vcx, |s, _| s.profile.clone()),
            Some(ProfilePage::User {
                handle: "sam".into(),
                space_id: None,
            })
        );
        assert!(vcx.debug_bounds("profile-avatar").is_some());

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

    /// Jacob, 2026-10-26 ("nuke vampire"): a generation that times out must end in a clean
    /// state — the shared note at the top of the body (one sentence, Try again, Details with
    /// the raw words behind it), the sheet grown by the note, the field, preview and footer
    /// below it untouched — and while it runs the sheet shows progress (spinner over the
    /// preview, "Painting a picture — N s"). Nothing may reach the footer.
    #[gpui::test]
    fn a_timed_out_generation_shows_the_shared_note_above_the_body(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx, "");
        let mut commands = state.update(vcx, |s, _| s.take_command_channel());
        settle(&shell, vcx);
        click(vcx, &shell, "me-avatar");
        shell.update(vcx, |sh, cx| {
            sh.avatar_prompt_input
                .update(cx, |input, cx| input.set_text("nuke vampire".into(), cx));
        });
        let card_before = vcx.debug_bounds("avatar-card").expect("card");
        click(vcx, &shell, "avatar-generate-go");
        assert!(matches!(
            commands.try_recv(),
            Ok(ClientCommand::GenerateImage { .. })
        ));

        // Progress: the spinner sits on the preview and the status counts seconds.
        assert!(
            vcx.debug_bounds("avatar-generate-spinner").is_some(),
            "spinner while painting"
        );
        let picker = shell
            .read_with(&*vcx, |sh, _| sh.avatar_picker.clone())
            .unwrap();
        assert!(picker.generating && picker.started_at.is_some());
        assert_eq!(picker.generating_for_s(), Some(0));
        assert!(vcx.debug_bounds("avatar-generate-error").is_none());

        // The relay's clean 504 (or the client's own timeout) arrives.
        let raw = "HTTP 504 avatar_timeout — gpt-image-1-mini: operation timed out after 40 s (attempt 2 of 2); relay budget 75 s";
        state.update(vcx, |s, cx| {
            s.apply(
                SyncEvent::ImageGenerated {
                    target: Target::Me("gh_1".into()),
                    image_key: None,
                    url: None,
                    error: Some(UserError::of(
                        "generate the image",
                        crate::sync::user_error::ErrorClass::Timeout,
                        raw,
                    )),
                },
                cx,
            )
        });
        settle(&shell, vcx);
        let picker = shell
            .read_with(&*vcx, |sh, _| sh.avatar_picker.clone())
            .unwrap();
        assert!(!picker.generating);
        let slot = picker.error.as_ref().expect("the note is up");
        assert_eq!(
            slot.error.sentence(),
            "Couldn't generate the image. The server took too long."
        );
        assert!(!slot.details_open, "details start closed");
        assert!(vcx.debug_bounds("avatar-generate-spinner").is_none());
        assert!(vcx
            .debug_bounds("avatar-generate-error-details-block")
            .is_none());

        // Geometry: bar · note · body · footer. The note sits under the bar and above the
        // field; the card grew by exactly the note (+ its gap); the footer is still last.
        let note = vcx.debug_bounds("avatar-generate-error").expect("the note");
        let body = vcx.debug_bounds("avatar-body").expect("body box");
        let cancel = vcx.debug_bounds("avatar-cancel").expect("Cancel");
        let set = vcx.debug_bounds("avatar-set").expect("Set avatar");
        let card = vcx.debug_bounds("avatar-card").expect("card");
        let status = vcx
            .debug_bounds("avatar-generate-status")
            .expect("status line");
        assert!(
            note.top() >= card.top() + px(BAR_H) && note.bottom() <= body.top(),
            "note {note:?} between the bar and the body {body:?}"
        );
        assert!(
            f32::from(note.left() - card.left()) > FIELD_INSET - 1.0,
            "note inset like the field: {note:?} in {card:?}"
        );
        let grew = f32::from(card.size.height - card_before.size.height);
        let expected = f32::from(note.size.height) + error_note::GAP;
        assert!(
            (grew - expected).abs() < 1.5,
            "the card grew by the note: {grew} vs {expected}"
        );
        assert!(
            status.bottom() <= cancel.top() && status.bottom() <= set.top(),
            "nothing reaches the footer: status {status:?} cancel {cancel:?}"
        );
        assert!(
            note.bottom() <= cancel.top(),
            "the note is far from the footer: {note:?} {cancel:?}"
        );
        assert!(
            vcx.debug_bounds("avatar-generate-error-try-again")
                .is_some(),
            "Try again on a timeout"
        );

        // Details: the mono block unfolds inside the note; the card grows again; then folds.
        click(vcx, &shell, "avatar-generate-error-details");
        let block = vcx
            .debug_bounds("avatar-generate-error-details-block")
            .expect("the details block");
        let note_open = vcx.debug_bounds("avatar-generate-error").expect("note");
        assert!(
            block.top() >= note_open.top() && block.bottom() <= note_open.bottom(),
            "block {block:?} inside the note {note_open:?}"
        );
        assert!(shell.read_with(&*vcx, |sh, _| sh
            .avatar_picker
            .as_ref()
            .and_then(|pk| pk.error.as_ref())
            .is_some_and(|slot| slot.details_open)));
        assert!(
            vcx.debug_bounds("avatar-generate-error-copy").is_some(),
            "Copy"
        );
        click(vcx, &shell, "avatar-generate-error-details");
        assert!(vcx
            .debug_bounds("avatar-generate-error-details-block")
            .is_none());

        // Try again re-sends the same prompt and clears the note.
        click(vcx, &shell, "avatar-generate-error-try-again");
        match commands.try_recv() {
            Ok(ClientCommand::GenerateImage { prompt, .. }) => assert_eq!(prompt, "nuke vampire"),
            other => panic!("expected GenerateImage, got {other:?}"),
        }
        let picker = shell
            .read_with(&*vcx, |sh, _| sh.avatar_picker.clone())
            .unwrap();
        assert!(picker.generating && picker.error.is_none());
        assert!(vcx.debug_bounds("avatar-generate-error").is_none());
        assert!(vcx.debug_bounds("avatar-generate-spinner").is_some());
        let card_after = vcx.debug_bounds("avatar-card").expect("card");
        assert!(
            (f32::from(card_after.size.height - card_before.size.height)).abs() < 1.0,
            "back to the plain height"
        );
        // Set avatar stays disabled through all of it (nothing to set).
        assert!(!picker.dirty());
    }
}
