//! Telegram-style initials avatars (D79): a disc with one of seven vertical
//! gradients, picked from the stable id (agents: project id; people: user id)
//! unless the agent's owner chose one (`avatar_color_index`), with white
//! semibold initials (one or two letters) at 44.5 % of the diameter. A photo
//! (GitHub avatar) always wins over initials — see [`crate::ui::people::avatar`].
//! An agent without a photo draws a plain solid-colour circle instead (D92): see [`disc`].
//! The brand mark is the only image left in the app; no character art.

use std::hash::Hash;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use gpui::{div, linear_color_stop, linear_gradient, prelude::*, px, Div, FontWeight, Hsla};

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

pub use arbos_core::avatar::disc;

/// The palette, the index rule and the sender accent live in `arbos-core`
/// (`avatar.rs`) so the iPhone draws the same colours through arbos-ffi.
#[cfg(test)]
use arbos_core::avatar::color_index;
pub use arbos_core::avatar::{index_for, GRADIENTS, INITIALS_SCALE};
use arbos_core::avatar::{Accent, Rgb};

// ----- D92 solid-colour discs ---------------------------------------------------

/// The disc colour an agent shows: the owner's pick, else the id's colour.
pub fn disc_color(seed: &str, color_override: Option<u8>) -> u8 {
    disc::color_for(seed, color_override)
}

/// Disc colour `index` as a fill.
pub fn disc_fill(index: u8) -> Hsla {
    let Rgb(r, g, b) = disc::fill(index);
    rgb([r, g, b])
}

/// An agent's default picture: one plain filled circle, nothing on it.
pub fn disc_element(color: u8, size: f32) -> Div {
    div()
        .debug_selector(|| "agent-disc".into())
        .size(px(size))
        .flex_shrink_0()
        .flex_none()
        .rounded_full()
        .bg(disc_fill(color))
}

// ----- Create-sheet chrome ------------------------------------------------------------
//
// No *default avatar* is grey any more (Jacob, 2026-11-06). The grey disc survives only as
// Create-sheet chrome: that sheet previews an entity that has no id yet, so there is no seed
// to derive a colour from. The relay assigns the id, and the colour follows from it.

/// Neutral disc fill and letter ink for the appearance.
pub fn neutral_colors(theme: &Theme) -> (Hsla, Hsla) {
    let (disc, ink) = match theme.appearance {
        Appearance::Dark => disc::NEUTRAL_DARK,
        Appearance::Light => disc::NEUTRAL_LIGHT,
    };
    (rgb([disc.0, disc.1, disc.2]), rgb([ink.0, ink.1, ink.2]))
}

/// The neutral avatar: the name's first letter, semibold, on a plain grey disc (no gradient).
/// `name` empty = an empty disc (the Create sheet before a name is typed).
pub fn neutral_element(name: &str, size: f32, theme: &Theme) -> Div {
    let (disc, ink) = neutral_colors(theme);
    let font = size * INITIALS_SCALE;
    let letter = if name.trim().is_empty() {
        String::new()
    } else {
        disc::initial(name)
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

/// An agent's avatar: its solid disc (D92).
pub fn agent(seed: &str, disc_override: Option<u8>, size: f32) -> Div {
    disc_element(disc_color(seed, disc_override), size)
}

/// The disc for an id whose `Project` record is not loaded yet — a workspace row the registry
/// has not caught up with. The id is all the colour needs.
pub fn disc_for_id(id: &str, size: f32) -> Div {
    disc_element(disc_color(id, None), size)
}

/// The agent pane's hero: the same disc at hero size.
pub fn project_hero(p: &Project, size: f32, theme: &Theme) -> Div {
    if p.is_people() {
        return element(&p.id, &p.name, p.avatar_color_index, size, theme);
    }
    disc_element(disc_color(&p.id, p.avatar_disc_color), size)
}

/// The bubble-bar accent of an agent: its disc colour (D92).
pub fn agent_accent(seed: &str, disc_override: Option<u8>, theme: &Theme) -> Hsla {
    accent_color(disc::accent(disc_color(seed, disc_override)), theme)
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
/// else `project()` (the disc / initials). The Secrets agent picker, member chips and the
/// subagent glyph drew `project()` alone and showed the disc next to a header that had the
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

/// An agent (project): its id gives the disc colour, the owner's pick overrides.
/// A people space (DM / group) rides the same `Project` type: a group shows the
/// colour picked on its Create screen (D92 amendment) and otherwise, like a DM,
/// its D79 initials — never an id-derived colour.
pub fn project(p: &Project, size: f32, theme: &Theme) -> Div {
    if p.is_people() {
        // A pick (including an older client's `255`) resolves through `color_for`, so a
        // group that was stored "no picture" draws its id's colour rather than a grey disc.
        if p.avatar_disc_color.is_some() {
            return disc_element(disc_color(&p.id, p.avatar_disc_color), size);
        }
        return element(&p.id, &p.name, p.avatar_color_index, size, theme);
    }
    agent(&p.id, p.avatar_disc_color, size)
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
        disc_color: a.avatar_disc_color,
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
        disc_color: p.avatar_disc_color,
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
    // agent's "Reading 500" row drew a disc seeded by the word "agent" while his finished
    // bubbles (attributed) had the photo. With several agents on the roster nothing is safe
    // to assume, and the seeded disc stays until the attribution lands.
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
pub fn member_picture(state: &AppState, chat: Option<&Project>, a: &AgentMember, size: f32) -> Div {
    let id = member_identity(chat, &state.projects, a);
    let image = id.image_key.as_deref().and_then(|u| state.avatar(u));
    agent_picture(&id, image, size)
}

/// The picture for an identity: the uploaded / generated image when the loader has it,
/// else the solid disc (D92) from the identity's own pick.
pub fn agent_picture(
    a: &AgentIdentity,
    image: Option<std::sync::Arc<gpui::RenderImage>>,
    size: f32,
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
    agent(&a.id, Some(a.disc_color), size)
}

/// Who started a call / sits in it (users and agents).
pub fn sender(s: &Sender, size: f32, theme: &Theme) -> Div {
    if s.kind == "agent" {
        agent(&s.id, s.avatar_disc_color, size)
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
        // Bundled Inter SemiBold (same subset as chart PNGs: Latin letters +
        // digits). cosmic-text's default sans is "Open Sans", which macOS does
        // not ship — shaping then panics with "no default font found" when
        // building the notification attachment
        // (feedback app-01M61N05XSVV6PFWWAW0R2BJYP).
        const INTER_SEMIBOLD: &[u8] =
            include_bytes!("../../../arbos-core/assets/Inter-SemiBold-subset.ttf");
        let mut font_system = cosmic_text::FontSystem::new();
        {
            let db = font_system.db_mut();
            db.load_font_data(INTER_SEMIBOLD.to_vec());
            db.set_sans_serif_family("Inter");
        }
        Mutex::new((font_system, cosmic_text::SwashCache::new()))
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
/// bundled Inter sans (loaded in [`fonts`]).
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

    /// Jacob, 2026-11-06: no grey default survives. An agent stored with the old `255`
    /// sentinel resolves to its id's palette colour like any other.
    #[test]
    fn an_agent_stored_neutral_now_resolves_to_its_ids_colour() {
        let card = AgentCard {
            id: "01N".into(),
            name: "Field notes".into(),
            handle: "field-notes".into(),
            disc_color: Some(disc::NEUTRAL),
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
        assert_eq!(who.disc_color, disc::default_color("01N"));
        assert!(who.disc_color < disc::COLOR_COUNT);
    }

    /// The Mac leg of the cross-surface vector. The relay asserts the same id and index in
    /// `arbos-relay/src/avatar.rs` and the iPhone asserts it through the FFI in
    /// `AvatarPickerTabsTests.swift`. All three call `disc::color_for`, so a divergence in the
    /// hash, the palette size or the shape of the seed fails here too.
    #[test]
    fn a_stored_neutral_resolves_to_the_same_vivid_disc_as_the_other_surfaces() {
        assert_eq!(disc_color("01M5AGENT", Some(disc::NEUTRAL)), 7);
        assert_eq!(disc_color("01M5AGENT", None), 7);
        // The seed is the bare id. A prefixed sender would hash to something else, which is
        // how the three surfaces would silently drift apart.
        assert_ne!(disc_color("agent:01M5AGENT", None), 7);
    }

    /// Every agent without a picture gets one of the ten colours, stable for its id.
    #[test]
    fn a_disc_colour_is_stable_and_in_range() {
        for i in 0..200 {
            let id = format!("01M5AGENT{i:04}");
            let c = disc_color(&id, None);
            assert!(c < disc::COLOR_COUNT);
            assert_eq!(c, disc_color(&id, None));
        }
        // A pinned colour wins; an out-of-range pin falls back to the id's colour.
        assert_eq!(disc_color("01M5", Some(3)), 3);
        assert_eq!(disc_color("01M5", Some(99)), disc_color("01M5", None));
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
