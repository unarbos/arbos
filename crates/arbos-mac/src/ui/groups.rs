//! Group chats in the shell (F6, design S15 / D69): stacked participant glyphs in a group tab,
//! the sender chip above another Project's message, and the "+ project" control in the tab
//! header with its picker. Everything here is pure rendering over `AppState`; the shell owns
//! the open/closed state of the picker.

use gpui::{div, prelude::*, px, Div};

use crate::app::AppState;
use crate::model::{Chat, ChatKind, Project};
use crate::ui::theme::Theme;

const MAX_GLYPHS: usize = 4;
const GLYPH: f32 = 16.0;

/// A space's picture everywhere it shows (row, pill, pane): the uploaded or generated image
/// when it has one (a group's `PATCH /chats`, an agent's avatar picker — D92 amendment), else
/// the agent's leaf or the D79 initials disc.
pub fn picture(space: &Project, state: &AppState, size: f32, theme: &Theme) -> Div {
    let image = space.image_url.as_deref().and_then(|url| state.avatar(url));
    match image {
        Some(image) => div()
            .size(px(size))
            .flex_shrink_0()
            .rounded_full()
            .overflow_hidden()
            .child(
                gpui::img(crate::ui::picture_fit::fit(&image, size))
                    .size(px(size))
                    .rounded_full()
                    .object_fit(gpui::ObjectFit::Cover),
            ),
        None => crate::ui::avatars::project(space, size, theme), // avatar-lint: allow (group picture)
    }
}

/// A chat's picture in a list of chats (palette, Forward to): a DM is the other person's
/// photo, else their initials disc from their display name; anything else is [`picture`].
/// A DM space has no `image_url` and its name is the other side's handle, so [`picture`]
/// alone draws the handle's initial and never the photo.
pub fn chat_picture(space: &Project, state: &AppState, size: f32, theme: &Theme) -> Div {
    match space
        .person()
        .filter(|_| space.is_people() && !space.is_people_group())
    {
        Some(person) => {
            crate::ui::people::avatar(state.avatar(&person.avatar_url), person, size, theme)
        }
        None => picture(space, state, size, theme),
    }
}

/// Group tab: the participants' glyphs stacked, host first. The "Projects ⇄" tab shows `⇄`.
pub fn tab_glyphs(
    chat: &Chat,
    projects: &[Project],
    images: &std::collections::HashMap<String, std::sync::Arc<gpui::RenderImage>>,
    theme: &Theme,
) -> Option<Div> {
    match chat.kind {
        ChatKind::Group => {
            // Overlapping heads: laid out absolutely inside a row of a known
            // width, so the chip's title starts after them (negative margins
            // left the row measuring as zero-wide).
            let shown = chat.participants.len().min(MAX_GLYPHS);
            let step = GLYPH - 5.0;
            let width = if shown == 0 {
                0.0
            } else {
                GLYPH + step * (shown as f32 - 1.0)
            };
            let mut row = div()
                .relative()
                .flex()
                .items_center()
                .flex_shrink_0()
                .w(px(width))
                .h(px(GLYPH));
            for (i, id) in chat.participants.iter().take(MAX_GLYPHS).enumerate() {
                let head = match projects.iter().find(|p| &p.id == id) {
                    Some(p) => crate::ui::avatars::project_with(
                        images.get(&p.id).cloned(),
                        p,
                        GLYPH,
                        theme,
                    ),
                    None => crate::ui::avatars::element(id, id, None, GLYPH, theme),
                };
                row = row.child(head.absolute().left(px(step * i as f32)).top(px(0.0)));
            }
            let extra = chat.participants.len().saturating_sub(MAX_GLYPHS);
            if extra > 0 {
                return Some(
                    div()
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .gap(px(3.0))
                        .child(row)
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(theme.text_faint)
                                .child(format!("+{extra}")),
                        ),
                );
            }
            Some(row)
        }
        ChatKind::Projects => Some(
            div()
                .flex_shrink_0()
                .text_size(px(13.0))
                .text_color(theme.text_muted)
                .child("⇄"),
        ),
        // A job tab's glyph is drawn by the header from the job's state (`header::job_tab_glyph`).
        ChatKind::Job | ChatKind::Chat | ChatKind::Other => None,
    }
}
