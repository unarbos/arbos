//! Telegram-style initials avatars (D79): a disc with one of seven vertical
//! gradients, picked from the stable id (agents: project id; people: user id)
//! unless the agent's owner chose one (`avatar_color_index`), with white
//! semibold initials (one or two letters) at 44.5 % of the diameter. A photo
//! (GitHub avatar) always wins over initials — see [`crate::ui::people::avatar`].
//! The brand mark is the only image left in the app; no character art.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use gpui::{
    div, linear_color_stop, linear_gradient, point, prelude::*, px, radians, svg, Animation,
    AnimationExt, AnyElement, Div, ElementId, FontWeight, Hsla, SharedString, Svg, Transformation,
};

use crate::app::AppState;
use crate::model::{AgentMember, Person, Project, Sender};
use crate::ui::theme::{Appearance, Theme};

/// What the agent is doing (from its status). Kept for callers that pulse
/// while an agent works; the disc itself does not animate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AvatarState {
    Idle,
    Working,
    Reply,
}

pub use arbos_core::avatar::leaf::{self, LeafPick};

use crate::ui::lottie;
/// The palette, the index rule and the sender accent live in `arbos-core`
/// (`avatar.rs`) so the iPhone draws the same colours through arbos-ffi.
pub use arbos_core::avatar::{color_index, index_for, GRADIENTS, INITIALS_SCALE};
use arbos_core::avatar::{Accent, Rgb};

// ----- D92 leaf avatars ---------------------------------------------------------

/// Remote flag `features.leaf_avatars` from `GET /me` (default on; the relay's
/// `ARBOS_FEATURE_LEAF_AVATARS=0` is the kill switch). Process-wide so every
/// avatar call site reads it without threading state through the views.
static LEAF_AVATARS: AtomicBool = AtomicBool::new(true);

pub fn set_leaf_avatars(on: bool) {
    LEAF_AVATARS.store(on, Ordering::Relaxed);
}

pub fn leaf_avatars_on() -> bool {
    LEAF_AVATARS.load(Ordering::Relaxed)
}

/// Remote flag `features.leaf_lottie` (default on): the leaves play their
/// committed Lottie idle / gust files; off = the native sway and ruffle.
static LEAF_LOTTIE: AtomicBool = AtomicBool::new(true);

pub fn set_leaf_lottie(on: bool) {
    LEAF_LOTTIE.store(on, Ordering::Relaxed);
}

pub fn leaf_lottie_on() -> bool {
    LEAF_LOTTIE.load(Ordering::Relaxed)
}

/// The leaf and disc colour an agent shows: the owner's picks, else the id's cell.
pub fn leaf_pick(seed: &str, leaf_override: Option<u8>, color_override: Option<u8>) -> LeafPick {
    leaf::pick_for(seed, leaf_override, color_override)
}

// ----- Neutral default (Jacob, 2026-10-25) -------------------------------------------

/// The pair the Create sheets store when nothing was uploaded or generated: "no leaf".
pub fn neutral_pick() -> LeafPick {
    LeafPick {
        leaf: leaf::NEUTRAL,
        color: leaf::NEUTRAL,
    }
}

/// Neutral disc fill and letter ink for the appearance.
pub fn neutral_colors(theme: &Theme) -> (Hsla, Hsla) {
    let (disc, ink) = match theme.appearance {
        Appearance::Dark => leaf::NEUTRAL_DARK,
        Appearance::Light => leaf::NEUTRAL_LIGHT,
    };
    (rgb([disc.0, disc.1, disc.2]), rgb([ink.0, ink.1, ink.2]))
}

/// The neutral avatar: the name's first letter, semibold, on a plain grey disc (no gradient,
/// no leaf). `name` empty = an empty disc (the Create sheet before a name is typed).
pub fn neutral_element(name: &str, size: f32, theme: &Theme) -> Div {
    let (disc, ink) = neutral_colors(theme);
    let font = size * INITIALS_SCALE;
    let letter = if name.trim().is_empty() {
        String::new()
    } else {
        leaf::initial(name)
    };
    div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(disc)
        .text_size(px(font))
        .line_height(px(font))
        .pb(px(size * 0.02))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ink)
        .child(letter)
}

/// The leaf ink on disc colour `index`: the darker same hue (v2 rule).
pub fn leaf_ink(index: u8) -> Hsla {
    let Rgb(r, g, b) = leaf::COLORS[(index % leaf::COLOR_COUNT) as usize].ink;
    rgb([r, g, b])
}

/// Disc colour `index`: the pastel of that hue.
pub fn leaf_disc(index: u8) -> Hsla {
    let Rgb(r, g, b) = leaf::COLORS[(index % leaf::COLOR_COUNT) as usize].rgb;
    rgb([r, g, b])
}

/// Asset path of leaf shape `index` (served by `icons::Assets`).
pub fn leaf_asset(index: u8) -> SharedString {
    SharedString::from(format!(
        "leaves/{}.svg",
        leaf::LEAVES[(index % leaf::LEAF_COUNT) as usize].slug
    ))
}

/// The SVG bytes behind a `leaves/<slug>.svg` asset path, for the asset source.
pub fn leaf_asset_bytes(path: &str) -> Option<&'static [u8]> {
    let slug = path.strip_prefix("leaves/")?.strip_suffix(".svg")?;
    leaf::LEAVES
        .iter()
        .find(|l| l.slug == slug)
        .map(|l| l.svg.as_bytes())
}

/// The leaf glyph alone (the hue's ink, `LEAF_SCALE` of `size`), optionally swayed:
/// `angle` in radians about the stem, i.e. the bottom centre of the glyph.
pub fn leaf_glyph(pick: LeafPick, size: f32, angle: f32) -> Svg {
    leaf_glyph_inked(pick.leaf, size, angle, leaf_ink(pick.color))
}

/// The leaf glyph in any ink (the picker draws unselected leaves in a neutral grey).
pub fn leaf_glyph_inked(leaf_index: u8, size: f32, angle: f32, ink: Hsla) -> Svg {
    let side = size * leaf::LEAF_SCALE;
    svg()
        .path(leaf_asset(leaf_index))
        .size(px(side))
        .flex_shrink_0()
        .text_color(ink)
        .with_transformation(sway(leaf_index, side, angle))
}

/// Reduce Motion (macOS Accessibility), read once: no sway, no ruffle.
fn reduce_motion() -> bool {
    static REDUCE: OnceLock<bool> = OnceLock::new();
    *REDUCE.get_or_init(crate::ui::motion::system_reduce_motion)
}

/// Hover state per leaf disc (`key` = who + where): hovered now, and a tick
/// that advances on every hover start so the one-shot ruffle replays. One map
/// for the reader and the writer (two function-local statics were two maps,
/// so the ruffle never showed on the real build).
static HOVER: OnceLock<Mutex<HashMap<u64, (bool, u64)>>> = OnceLock::new();

fn hover_state(key: u64) -> (bool, u64) {
    HOVER
        .get_or_init(Default::default)
        .lock()
        .ok()
        .and_then(|m| m.get(&key).copied())
        .unwrap_or((false, 0))
}

fn set_hover_state(key: u64, hovered: bool) {
    if let Ok(mut m) = HOVER.get_or_init(Default::default).lock() {
        let e = m.entry(key).or_insert((false, 0));
        if hovered && !e.0 {
            e.1 += 1;
        }
        e.0 = hovered;
    }
}

/// Rig: `ARBOS_LEAF_GUST=<project id or name>` holds that avatar hovered and
/// loops its gust, as if the pointer kept arriving — the screenshot rigs cannot
/// move the real pointer. Off unless the variable is set.
fn rig_gust(seed: &str, name: &str, key: u64) {
    static TARGET: OnceLock<Option<String>> = OnceLock::new();
    let target = TARGET.get_or_init(|| std::env::var("ARBOS_LEAF_GUST").ok());
    if target.as_deref() == Some(seed) || target.as_deref() == Some(name) {
        RIG_GUST.store(true, Ordering::Relaxed);
        set_hover_state(key, true);
    }
}

/// Set once `rig_gust` matched: the gust animation repeats instead of playing once.
static RIG_GUST: AtomicBool = AtomicBool::new(false);

fn gust_animation(secs: f32) -> Animation {
    let a = Animation::new(Duration::from_secs_f32(secs));
    if RIG_GUST.load(Ordering::Relaxed) {
        a.repeat()
    } else {
        a
    }
}

pub fn leaf_hover_key(seed: &str, size: f32) -> u64 {
    hover_key(seed, size)
}

fn hover_key(seed: &str, size: f32) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut h);
    size.to_bits().hash(&mut h);
    h.finish()
}

/// A rotation by `angle` radians about the leaf's stem pivot (from the
/// manifest) for a glyph of `side` points. GPUI rotates about the glyph
/// centre, so the pivot is restored with a translation of `p − R·p`.
fn sway(leaf_index: u8, side: f32, angle: f32) -> Transformation {
    let (pvx, pvy) = leaf::LEAVES[(leaf_index % leaf::LEAF_COUNT) as usize].pivot;
    let (x, y) = ((pvx - 50.0) / 100.0 * side, (pvy - 50.0) / 100.0 * side);
    let (sin, cos) = angle.sin_cos();
    Transformation::rotate(radians(angle)).with_translation(point(
        px(x - (x * cos - y * sin)),
        px(y - (x * sin + y * cos)),
    ))
}

fn disc(pick: LeafPick, size: f32) -> Div {
    div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(leaf_disc(pick.color))
}

/// The leaf avatar disc at `size` points: a cream leaf on its autumn colour. Still.
pub fn leaf_element(pick: LeafPick, size: f32) -> Div {
    disc(pick, size).child(leaf_glyph(pick, size, 0.0))
}

/// One-shot hover gust: the leaf's committed gust Lottie while the flag is
/// on (one play, settles on the rest pose), else the native spring (≈9° peak,
/// settled in 1 s).
fn ruffle_glyph(pick: LeafPick, size: f32, tick: u64, ink: Hsla) -> AnyElement {
    if leaf_lottie_on() {
        if let Some(comp) = lottie::composition(pick.leaf, lottie::Clip::Gust) {
            let secs = lottie::duration_secs(&comp);
            return div()
                .size(px(size * leaf::LEAF_SCALE))
                .flex_shrink_0()
                .with_animation(
                    ElementId::NamedInteger(
                        SharedString::from(format!("leaf-lottie-gust-{}", pick.leaf)),
                        tick,
                    ),
                    gust_animation(secs),
                    move |el, t| {
                        el.child(lottie::frame_element(
                            comp.clone(),
                            t,
                            size * leaf::LEAF_SCALE,
                            ink,
                        ))
                    },
                )
                .into_any_element();
        }
    }
    let side = size * leaf::LEAF_SCALE;
    leaf_glyph_inked(pick.leaf, size, 0.0, ink)
        .with_animation(
            ElementId::NamedInteger(
                SharedString::from(format!("leaf-gust-{}-{}", pick.leaf, pick.color)),
                tick,
            ),
            gust_animation(leaf::GUST_SECS),
            move |el, t| {
                el.with_transformation(sway(pick.leaf, side, leaf::gust_angle(t * leaf::GUST_SECS)))
            },
        )
        .into_any_element()
}

/// Idle sway, profile hero only: the leaf's committed idle Lottie, looping
/// (each leaf its own 2.3–2.8 s length), while the flag is on; else the
/// native sway of a few degrees about the stem.
fn sway_glyph(pick: LeafPick, size: f32, ink: Hsla) -> AnyElement {
    if leaf_lottie_on() {
        if let Some(comp) = lottie::composition(pick.leaf, lottie::Clip::Idle) {
            let secs = lottie::duration_secs(&comp);
            return div()
                .size(px(size * leaf::LEAF_SCALE))
                .flex_shrink_0()
                .with_animation(
                    ElementId::Name(SharedString::from(format!(
                        "leaf-lottie-idle-{}",
                        pick.leaf
                    ))),
                    Animation::new(Duration::from_secs_f32(secs)).repeat(),
                    move |el, t| {
                        el.child(lottie::frame_element(
                            comp.clone(),
                            t,
                            size * leaf::LEAF_SCALE,
                            ink,
                        ))
                    },
                )
                .into_any_element();
        }
    }
    let side = size * leaf::LEAF_SCALE;
    let period = leaf::LEAVES[(pick.leaf % leaf::LEAF_COUNT) as usize].idle_secs;
    leaf_glyph_inked(pick.leaf, size, 0.0, ink)
        .with_animation(
            ElementId::Name(SharedString::from(format!(
                "leaf-sway-{}-{}",
                pick.leaf, pick.color
            ))),
            Animation::new(Duration::from_secs_f32(period)).repeat(),
            move |el, t| {
                el.with_transformation(sway(
                    pick.leaf,
                    side,
                    leaf::idle_angle(pick.leaf, t * period),
                ))
            },
        )
        .into_any_element()
}

/// The leaf disc with the Mac hover ruffle (lists, chips, header): still until
/// the pointer enters, one wobble, still again. Nothing moves under Reduce
/// Motion or below 20 pt, where a wobble is noise.
pub fn leaf_hover(pick: LeafPick, size: f32, key: u64) -> Div {
    if reduce_motion() || size < 20.0 {
        return leaf_element(pick, size);
    }
    let (hovered, tick) = hover_state(key);
    let glyph = if hovered {
        ruffle_glyph(pick, size, tick, leaf_ink(pick.color))
    } else {
        leaf_glyph(pick, size, 0.0).into_any_element()
    };
    disc(pick, size).child(
        div()
            .id(ElementId::NamedInteger("leaf-hover".into(), key))
            .debug_selector(|| "leaf-hover".into())
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .on_hover(move |is_hovered, window, _| {
                set_hover_state(key, *is_hovered);
                window.refresh();
            })
            .child(glyph),
    )
}

/// The profile hero with no disc behind the leaf (Jacob, [feedback
/// info-pane-chrome-2026-10-24]: "drop the pale disc behind the leaf", as
/// Grok Bot's plain bot mark): the leaf alone, the same sway and ruffle. The
/// ink is the hue's pastel on a dark pane and its darker ink on a light one,
/// so the bare leaf reads on either (the dark ink on the dark pane did not).
pub fn leaf_hero_bare(pick: LeafPick, size: f32, key: u64, theme: &Theme) -> Div {
    let ink = if theme.is_light() {
        leaf_ink(pick.color)
    } else {
        leaf_disc(pick.color)
    };
    let slot = div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center();
    leaf_hero_in(slot, pick, size, key, ink)
}

fn leaf_hero_in(container: Div, pick: LeafPick, size: f32, key: u64, ink: Hsla) -> Div {
    if reduce_motion() {
        return container.child(leaf_glyph_inked(pick.leaf, size, 0.0, ink));
    }
    let (hovered, tick) = hover_state(key);
    let glyph = if hovered {
        ruffle_glyph(pick, size, tick, ink)
    } else {
        sway_glyph(pick, size, ink)
    };
    container.child(
        div()
            .id(ElementId::NamedInteger("leaf-hero".into(), key))
            .debug_selector(|| "leaf-hero".into())
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .on_hover(move |is_hovered, window, _| {
                set_hover_state(key, *is_hovered);
                window.refresh();
            })
            .child(glyph),
    )
}

/// An agent's avatar: the leaf (D92) while the flag is on, else the D79 disc — or the neutral
/// initial when the agent was created with no leaf (Jacob, 2026-10-25).
pub fn agent(
    seed: &str,
    name: &str,
    color_index: Option<u8>,
    leaf_override: Option<u8>,
    leaf_color_override: Option<u8>,
    size: f32,
    theme: &Theme,
) -> Div {
    if leaf::is_neutral(leaf_override) {
        return neutral_element(name, size, theme);
    }
    if leaf_avatars_on() {
        let key = hover_key(seed, size);
        rig_gust(seed, name, key);
        leaf_hover(
            leaf_pick(seed, leaf_override, leaf_color_override),
            size,
            key,
        )
    } else {
        element(seed, name, color_index, size, theme)
    }
}

/// The agent pane's hero (Grok Bot's plain mark, [feedback
/// info-pane-chrome-2026-10-24]): the swaying leaf with no disc behind it;
/// the D79 initials disc stays when the leaf flag is off (initials need a fill).
pub fn project_hero_bare(p: &Project, size: f32, theme: &Theme) -> Div {
    if leaf::is_neutral(p.avatar_leaf) {
        return neutral_element(&p.name, size, theme);
    }
    if leaf_avatars_on() && !p.is_people() {
        leaf_hero_bare(project_leaf(p), size, hover_key(&p.id, size), theme)
    } else {
        element(&p.id, &p.name, p.avatar_color_index, size, theme)
    }
}

/// The bubble-bar accent of an agent: its leaf disc colour (D92), else D79; a mid grey for a
/// neutral agent.
pub fn agent_accent(
    seed: &str,
    color_index: Option<u8>,
    leaf_override: Option<u8>,
    leaf_color_override: Option<u8>,
    theme: &Theme,
) -> Hsla {
    if leaf::is_neutral(leaf_override) {
        return accent_color(leaf::neutral_accent(), theme);
    }
    if leaf_avatars_on() {
        accent_color(
            leaf::accent(leaf_pick(seed, leaf_override, leaf_color_override)),
            theme,
        )
    } else {
        accent_fallback(seed, color_index, theme)
    }
}

fn rgb(c: [u8; 3]) -> Hsla {
    gpui::Rgba {
        r: c[0] as f32 / 255.0,
        g: c[1] as f32 / 255.0,
        b: c[2] as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

/// Top and bottom colours of gradient `index` for the current appearance.
pub fn gradient(index: u8, theme: &Theme) -> (Hsla, Hsla) {
    let g = GRADIENTS[(index as usize) % 7];
    match theme.appearance {
        Appearance::Light => (rgb(g.top), rgb(g.bottom)),
        Appearance::Dark => (rgb(g.dark_top), rgb(g.dark_bottom)),
    }
}

/// An `arbos-core` accent in the current appearance.
pub fn accent_color(accent: Accent, theme: &Theme) -> Hsla {
    let Rgb(r, g, b) = accent.for_appearance(matches!(theme.appearance, Appearance::Dark));
    rgb([r, g, b])
}

/// The bubble bar colour for a sender without a usable picture: the midpoint
/// of their D79 gradient, i.e. the disc's middle row.
pub fn accent_fallback(seed: &str, override_index: Option<u8>, theme: &Theme) -> Hsla {
    accent_color(
        arbos_core::avatar::fallback_accent(seed, override_index),
        theme,
    )
}

/// The seed that colours a person's disc: their id, the handle when it is empty.
pub fn person_seed(p: &Person) -> &str {
    if p.id.is_empty() {
        &p.handle
    } else {
        &p.id
    }
}

/// One or two letters: the first letter of the first two words ("Growth Lab"
/// → "GL"), one for a single word ("Arbos" → "A"). Leading `@` and
/// punctuation are skipped; an empty name gives "?".
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || c == '-' || c == '_' || c == '·' || c == '/')
        .map(|w| w.trim_start_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();
    let first = |w: &str| {
        w.chars()
            .next()
            .map(|c| c.to_uppercase().collect::<String>())
            .unwrap_or_default()
    };
    match words.as_slice() {
        [] => "?".into(),
        [one] => first(one),
        [a, b, ..] => format!("{}{}", first(a), first(b)),
    }
}

/// The avatar disc: gradient `index` (or the seed's) with the name's initials.
pub fn element(seed: &str, name: &str, index: Option<u8>, size: f32, theme: &Theme) -> Div {
    let (top, bottom) = gradient(index_for(seed, index), theme);
    let text = initials(name);
    let font = size * INITIALS_SCALE;
    div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(linear_gradient(
            180.0,
            linear_color_stop(top, 0.0),
            linear_color_stop(bottom, 1.0),
        ))
        .text_size(px(font))
        // A tight line box: with the 1.1× box GPUI centred, SF's ascent pushed
        // the caps ~0.75 pt below the disc centre (parity pass, Telegram ≈ 0.17 pt).
        .line_height(px(font))
        .pb(px(size * 0.02))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(gpui::white())
        .child(text)
}

/// A project's picture as every surface should draw it: the uploaded / generated picture of
/// the newest copy the store holds (`sync::agent_avatar::newest`, decoded in `state.avatars`),
/// else `project()` (the leaf / initials). The Secrets agent picker, member chips and the
/// subagent glyph drew `project()` alone and showed the leaf next to a header that had the
/// picture (Jacob, 2026-10-26, Mac v0.6.268).
pub fn project_in(state: &AppState, p: &Project, size: f32, theme: &Theme) -> Div {
    project_with(project_image(state, p), p, size, theme)
}

/// The decoded picture a project draws (the newest copy's URL through the avatar cache).
pub fn project_image(state: &AppState, p: &Project) -> Option<std::sync::Arc<gpui::RenderImage>> {
    crate::sync::agent_avatar::newest(&state.projects, &p.id)
        .and_then(|f| f.image_url)
        .or_else(|| p.image_url.clone())
        .and_then(|u| state.avatar(&u))
}

/// `project_image` for every project at once — for builders that hold `cx` mutably while
/// they loop (listeners) and cannot keep the state borrowed.
pub fn project_images(
    state: &AppState,
) -> std::collections::HashMap<String, std::sync::Arc<gpui::RenderImage>> {
    state
        .projects
        .iter()
        .filter_map(|p| project_image(state, p).map(|i| (p.id.clone(), i)))
        .collect()
}

/// `project()` with the picture when the caller has it.
pub fn project_with(
    image: Option<std::sync::Arc<gpui::RenderImage>>,
    p: &Project,
    size: f32,
    theme: &Theme,
) -> Div {
    if let Some(image) = image {
        return div()
            .size(px(size))
            .flex_shrink_0()
            .rounded_full()
            .overflow_hidden()
            .child(
                gpui::img(image)
                    .size(px(size))
                    .rounded_full()
                    .object_fit(gpui::ObjectFit::Cover),
            );
    }
    project(p, size, theme)
}

/// An agent (project): its id seeds the leaf and colour, the owner's picks override.
/// A people space (DM / group) rides the same `Project` type: a group shows the
/// leaf picked on its Create screen (D92 amendment) and otherwise, like a DM,
/// its D79 initials — never an id-derived leaf.
pub fn project(p: &Project, size: f32, theme: &Theme) -> Div {
    if p.is_people() {
        if leaf::is_neutral(p.avatar_leaf) {
            return neutral_element(&p.name, size, theme);
        }
        if let (true, Some(leaf), Some(color)) =
            (leaf_avatars_on(), p.avatar_leaf, p.avatar_leaf_color)
        {
            return leaf_hover(LeafPick { leaf, color }, size, hover_key(&p.id, size));
        }
        return element(&p.id, &p.name, p.avatar_color_index, size, theme);
    }
    agent(
        &p.id,
        &p.name,
        p.avatar_color_index,
        p.avatar_leaf,
        p.avatar_leaf_color,
        size,
        theme,
    )
}

/// The agent's leaf pick (D92) for callers that animate it.
pub fn project_leaf(p: &Project) -> LeafPick {
    leaf_pick(&p.id, p.avatar_leaf, p.avatar_leaf_color)
}

// ----- One identity for every surface (#1592) --------------------------------------

pub use arbos_core::avatar::identity::{AgentCard, AgentIdentity, AgentQuery};

/// A roster row as a card. On the Mac the card's `image_key` slot carries the picture's
/// fetch URL (the relay client already resolved the key), so the identity hands back what
/// `AppState::avatar` wants.
fn member_card(a: &AgentMember) -> AgentCard {
    AgentCard {
        id: a.project_id.clone(),
        name: a.name.clone(),
        handle: a.handle.clone(),
        leaf: a.avatar_leaf,
        leaf_color: a.avatar_leaf_color,
        color_index: a.avatar_color_index,
        image_key: a.image_url.clone(),
        avatar_version: a.avatar_version,
    }
}

fn project_card(p: &Project) -> AgentCard {
    AgentCard {
        id: p.id.clone(),
        name: p.name.clone(),
        handle: Project::mention_slug(&p.name),
        leaf: p.avatar_leaf,
        leaf_color: p.avatar_leaf_color,
        color_index: p.avatar_color_index,
        image_key: p.image_url.clone(),
        avatar_version: p.avatar_version,
    }
}

/// The cards an agent row is resolved against, in the rule's order: the open chat's
/// roster (the relay's current truth for that chat), my projects, every other roster.
pub fn agent_cards(chat: Option<&Project>, projects: &[Project]) -> Vec<AgentCard> {
    let mut cards: Vec<AgentCard> = Vec::new();
    if let Some(chat) = chat {
        cards.extend(chat.agents.iter().map(member_card));
    }
    cards.extend(projects.iter().filter(|p| !p.is_people()).map(project_card));
    cards.extend(
        projects
            .iter()
            .filter(|p| p.is_people() && chat.map(|c| c.id != p.id).unwrap_or(true))
            .flat_map(|p| p.agents.iter().map(member_card)),
    );
    cards
}

/// One agent, one answer: the bubble disc and caption, the presence row, the member
/// lists, the chips and the call tiles all ask here with the row's `sender`,
/// `senderHandle` and `senderName` (`arbos_core::avatar::identity::resolve`).
pub fn agent_identity(
    chat: Option<&Project>,
    projects: &[Project],
    sender: &str,
    handle: Option<&str>,
    name: Option<&str>,
) -> AgentIdentity {
    let cards = agent_cards(chat, projects);
    let resolved = arbos_core::avatar::identity::resolve(
        &AgentQuery {
            sender,
            handle,
            name,
        },
        &cards,
    );
    if resolved.known {
        return resolved;
    }
    // A live row the box has not attributed yet (`sender: "agent"`, no handle, no name —
    // the box's people-chat info was not loaded when the turn started, e.g. right after an
    // image roll) in a people chat with one agent: it is that agent. Jacob, 2026-10-26: Doug
    // agent's "Reading 500" row drew a leaf seeded by the word "agent" while his finished
    // bubbles (attributed) had the photo. With several agents on the roster nothing is safe
    // to assume, and the seeded leaf stays until the attribution lands.
    let bare = arbos_core::avatar::identity::sender_id(sender).is_empty()
        || sender == "agent"
        || sender == "assistant";
    if bare && handle.map(str::trim).unwrap_or("").is_empty() {
        if let Some(chat) = chat.filter(|c| c.is_people() && c.agents.len() == 1) {
            let only = &chat.agents[0];
            return arbos_core::avatar::identity::resolve(
                &AgentQuery {
                    sender: &only.project_id,
                    handle: Some(&only.handle),
                    name: Some(&only.name),
                },
                &cards,
            );
        }
    }
    resolved
}

/// The identity of a roster member, drawn from the same rule (so its picture is the
/// one its bubbles get, uploaded picture included).
pub fn member_identity(
    chat: Option<&Project>,
    projects: &[Project],
    a: &AgentMember,
) -> AgentIdentity {
    agent_identity(
        chat,
        projects,
        &a.project_id,
        Some(&a.handle),
        Some(&a.name),
    )
}

/// A roster member's picture through the resolver (identity → newest copy → decoded image).
pub fn member_picture(
    state: &AppState,
    chat: Option<&Project>,
    a: &AgentMember,
    size: f32,
    theme: &Theme,
) -> Div {
    let id = member_identity(chat, &state.projects, a);
    let image = id.image_key.as_deref().and_then(|u| state.avatar(u));
    agent_picture(&id, image, size, theme)
}

/// The picture for an identity: the uploaded / generated image when the loader has it,
/// else the leaf (D92) or the initials disc (D79) from the identity's own picks.
pub fn agent_picture(
    a: &AgentIdentity,
    image: Option<std::sync::Arc<gpui::RenderImage>>,
    size: f32,
    theme: &Theme,
) -> Div {
    if let Some(image) = image {
        return div()
            .size(px(size))
            .flex_shrink_0()
            .rounded_full()
            .overflow_hidden()
            .child(
                gpui::img(crate::ui::picture_fit::fit(&image, size))
                    .size(px(size))
                    .rounded_full()
                    .object_fit(gpui::ObjectFit::Cover),
            );
    }
    if a.neutral {
        return neutral_element(&a.name, size, theme);
    }
    agent(
        &a.id,
        &a.name,
        Some(a.color_index),
        Some(a.leaf.leaf),
        Some(a.leaf.color),
        size,
        theme,
    )
}

/// Who started a call / sits in it (users and agents).
pub fn sender(s: &Sender, size: f32, theme: &Theme) -> Div {
    if s.kind == "agent" {
        agent(
            &s.id,
            &s.display_name(),
            s.avatar_color_index,
            s.avatar_leaf,
            s.avatar_leaf_color,
            size,
            theme,
        )
    } else {
        element(&s.id, &s.display_name(), s.avatar_color_index, size, theme)
    }
}

/// A person without a photo.
pub fn person(p: &Person, size: f32, theme: &Theme) -> Div {
    element(person_seed(p), &p.display_name(), None, size, theme)
}

// ----- notification attachment --------------------------------------------------

fn fonts() -> &'static Mutex<(cosmic_text::FontSystem, cosmic_text::SwashCache)> {
    static FONTS: OnceLock<Mutex<(cosmic_text::FontSystem, cosmic_text::SwashCache)>> =
        OnceLock::new();
    FONTS.get_or_init(|| {
        Mutex::new((
            cosmic_text::FontSystem::new(),
            cosmic_text::SwashCache::new(),
        ))
    })
}

/// The same disc as a 128 px PNG for notification banners (cached by
/// gradient + initials under the notify image dir). `dark` picks the dark
/// pair: the system draws the banner, so callers pass macOS's own appearance
/// (`theme::system_is_dark()`), not the in-app theme (D81).
pub fn notification_png(seed: &str, name: &str, index: Option<u8>, dark: bool) -> Option<PathBuf> {
    let idx = index_for(seed, index);
    let text = initials(name);
    let variant = if dark { "-dark" } else { "" };
    let key = format!("initials-{idx}-{}{variant}", text.to_lowercase());
    let size = 128u32;
    let g = GRADIENTS[idx as usize];
    let (top, bottom) = if dark {
        (g.dark_top, g.dark_bottom)
    } else {
        (g.top, g.bottom)
    };
    let mut out = image::RgbaImage::new(size, size);
    let c = (size as f32 - 1.0) / 2.0;
    let radius = size as f32 / 2.0;
    for (x, y, px) in out.enumerate_pixels_mut() {
        let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
        let cover = (radius - d + 0.5).clamp(0.0, 1.0);
        if cover <= 0.0 {
            continue;
        }
        let t = y as f32 / (size as f32 - 1.0);
        let mix = |i: usize| (top[i] as f32 * (1.0 - t) + bottom[i] as f32 * t) as u8;
        *px = image::Rgba([mix(0), mix(1), mix(2), (cover * 255.0) as u8]);
    }
    draw_initials(&mut out, &text);
    crate::ui::notify::image_file(&key, size, size, out.as_raw())
}

/// White semibold initials centred on the disc, through cosmic-text and the
/// system's sans-serif.
fn draw_initials(out: &mut image::RgbaImage, text: &str) {
    use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping, Weight};
    let Ok(mut guard) = fonts().lock() else {
        return;
    };
    let (font_system, cache) = &mut *guard;
    let size = out.width() as f32;
    let font_px = size * INITIALS_SCALE;
    let metrics = Metrics::new(font_px, font_px * 1.2);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_size(Some(size), Some(size));
    buffer.set_text(
        text,
        &Attrs::new()
            .family(Family::SansSerif)
            .weight(Weight::SEMIBOLD),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    // Centre: measure the laid-out run, then offset while drawing.
    let (mut width, mut ascent, mut descent) = (0.0f32, 0.0f32, 0.0f32);
    for run in buffer.layout_runs() {
        width = width.max(run.line_w);
        ascent = ascent.max(run.line_y - run.line_top);
        descent = descent.max(run.line_height - (run.line_y - run.line_top));
    }
    let text_h = ascent + descent;
    let dx = ((size - width) / 2.0).round() as i32;
    let dy = ((size - text_h) / 2.0).round() as i32;
    let white = cosmic_text::Color::rgb(255, 255, 255);
    let (w, h) = (out.width() as i32, out.height() as i32);
    buffer.draw(font_system, cache, white, |x, y, bw, bh, color| {
        let a = color.a() as f32 / 255.0;
        if a <= 0.0 {
            return;
        }
        for yy in y..y + bh as i32 {
            for xx in x..x + bw as i32 {
                let (px_x, px_y) = (xx + dx, yy + dy);
                if px_x < 0 || px_y < 0 || px_x >= w || px_y >= h {
                    continue;
                }
                let p = out.get_pixel_mut(px_x as u32, px_y as u32);
                let blend = |c: u8| (c as f32 * (1.0 - a) + 255.0 * a) as u8;
                *p = image::Rgba([
                    blend(p[0]),
                    blend(p[1]),
                    blend(p[2]),
                    p[3].max((a * 255.0) as u8),
                ]);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Jacob, 2026-10-25: the stored neutral pair is told apart from the automatic leaf, and a
    /// neutral identity never draws a leaf.
    #[test]
    fn neutral_pick_is_the_core_value_and_identity_carries_it() {
        let n = neutral_pick();
        assert!(leaf::is_neutral(Some(n.leaf)));
        assert!(!leaf::is_neutral(None));
        let card = AgentCard {
            id: "01N".into(),
            name: "Field notes".into(),
            handle: "field-notes".into(),
            leaf: Some(leaf::NEUTRAL),
            leaf_color: Some(leaf::NEUTRAL),
            color_index: None,
            image_key: None,
            avatar_version: 0,
        };
        let who = arbos_core::avatar::identity::resolve(
            &AgentQuery {
                sender: "agent:01N",
                handle: None,
                name: None,
            },
            &[card],
        );
        assert!(who.neutral);
        assert_eq!(leaf::initial(&who.name), "F");
    }

    #[test]
    fn initials_follow_the_rule() {
        assert_eq!(initials("Growth Lab"), "GL");
        assert_eq!(initials("Arbos"), "A");
        assert_eq!(initials("@alpha-lab"), "AL");
        assert_eq!(initials("sam rivera jones"), "SR");
        assert_eq!(initials(""), "?");
    }

    #[test]
    fn colour_index_is_byte_sum_mod_seven() {
        assert_eq!(color_index(""), 0);
        assert_eq!(color_index("a"), (97 % 7) as u8);
        // Reference values from the relay's spec.
        assert_eq!(color_index("gh_1"), 1);
        assert_eq!(color_index("u_mac"), 6);
        assert_eq!(
            color_index("01M50W7HPM0KCFVMQ32HXNTN5S"),
            color_index("01M50W7HPM0KCFVMQ32HXNTN5S")
        );
        assert!(color_index("01M50W7HPM0KCFVMQ32HXNTN5S") < 7);
        assert_eq!(index_for("x", Some(3)), 3);
        assert_eq!(index_for("x", Some(9)), color_index("x"));
    }
}
