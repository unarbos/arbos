//! Per-agent call voice picker (Grok settings look): "Default" first, then
//! every voice from `GET /voices` as a row with name + description, a play
//! button that plays the cached sample, and a checkmark on the selected one.
//! Saving is a click (`PATCH /projects/{id} {voice}`).

use gpui::{div, prelude::*, px, Context, Div, ElementId, FontWeight, SharedString, Stateful};

use crate::model::{Project, Voice};
use crate::ui::glass;
use crate::ui::highlight::{self, Shape};
use crate::ui::icons::icon;
use crate::ui::shell::Shell;
use crate::ui::theme::{Appearance, Theme, TEXT_BODY, TEXT_SMALL, TEXT_TINY};
use crate::ui::widgets::CardPalette;

/// The sheet. `space` is the agent whose voice is being picked.
pub fn sheet(shell: &Shell, space: &Project, cx: &mut Context<Shell>) -> Div {
    let theme = Theme::of(cx).clone();

    let done = glass::sheet_button("voice-done", "Done", &theme).on_click(cx.listener(
        |this, _, window, cx| {
            this.voice_picker_open = false;
            this.state.update(cx, |s, cx| s.stop_voice_sample(cx));
            crate::ui::composer::Composer::focus(&this.composer, window, cx);
            cx.notify();
        },
    ));
    let header = glass::sheet_header("Voice", None, Some(done), &theme);
    let list = list(shell, space, "voice", 520.0, false, cx);

    let body = div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .px(px(16.0))
        .pb(px(18.0))
        .child(
            div()
                .px(px(4.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(format!(
                    "How {} sounds on calls. Press play to hear a sample.",
                    space.name
                )),
        )
        .child(list);
    glass::sheet(440.0, &theme, header, body.max_h(px(620.0)))
}

/// The voice list on its own: "Default" first, then every voice with its
/// play button and a checkmark on the selected one; a click saves. Shared by
/// the sheet and the agent page's Voice dropdown (`ui::hoverpick`). `ids`
/// prefixes the element ids so two lists may be on screen at once.
/// `fold_agent_pick` closes the agent-page Voice row after a save (the sheet
/// keeps itself open until Done).
/// Row height: a body title over a tiny one-line description, 10 pt of air
/// above and below (the pane kit's list rows).
const ROW_HEIGHT: f32 = 56.0;
/// The list's corners; each row's highlight pill is concentric with them.
const LIST_RADIUS: f32 = 14.0;

pub fn list(
    shell: &Shell,
    space: &Project,
    ids: &'static str,
    max_height: f32,
    fold_agent_pick: bool,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let theme = Theme::of(cx).clone();
    let palette = CardPalette::of(&theme);
    let state = shell.state.read(cx);
    let voices: Vec<Voice> = state.voices.clone();
    let selected = space.voice.clone();
    let pid = space.id.clone();
    let default_voice = state
        .default_voice
        .as_deref()
        .and_then(|id| voices.iter().find(|v| v.id == id))
        .cloned();
    let default_label = match &default_voice {
        Some(v) => format!("Default ({})", v.name),
        None => "Default".to_string(),
    };
    // The named voice's own row carries its description; this row says what
    // "Default" means instead of repeating it (#1316).
    let default_desc = "What the relay uses when nothing is chosen";
    // White in light, near-black in dark (the question-card palette).
    let list_bg = match theme.appearance {
        Appearance::Light => palette.inner,
        Appearance::Dark => palette.outer,
    };

    // One fixed-height row, title and a one-line description (truncated):
    // GPUI measures a wrapping text at the row's full width before the play
    // button and checkmark take theirs, so a two-line description used to
    // paint over the next row (#1316). Fixed height keeps the play button
    // and the checkmark centred on every row.
    let row_base = |id: ElementId, theme: &Theme| {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(12.0))
            .h(px(ROW_HEIGHT))
            .px(px(14.0))
            .cursor_pointer()
            .map(|el| highlight::row(el, false, Shape::Nested(LIST_RADIUS), theme))
    };
    let check = |on: bool, theme: &Theme| {
        div()
            .flex_shrink_0()
            .size(px(22.0))
            .flex()
            .items_center()
            .justify_center()
            .when(on, |el| el.child(icon("check", 16.0, theme.accent)))
    };

    let eid = |suffix: String| ElementId::Name(SharedString::from(format!("{ids}-{suffix}")));
    let mut list = div()
        .id(eid("list".into()))
        .flex()
        .flex_col()
        .rounded(px(LIST_RADIUS))
        .overflow_y_scroll()
        .max_h(px(max_height))
        .bg(list_bg)
        .border_1()
        .border_color(palette.hairline);

    let play_button = |id: ElementId, url: Option<String>, playing: bool, theme: &Theme| {
        let has = url.is_some();
        div()
            .id(id)
            .flex_shrink_0()
            .size(px(34.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(if playing {
                theme.text
            } else {
                Theme::with_alpha(theme.text, 0.08)
            })
            .when(!has, |el| el.opacity(0.35))
            .hover(|s| s.opacity(0.85))
            .child(icon(
                if playing { "stop" } else { "play" },
                13.0,
                if playing { theme.bg } else { theme.text },
            ))
            .when_some(url, |el, url| {
                el.on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.state
                            .update(cx, |s, cx| s.toggle_voice_sample(url.clone(), cx));
                    }),
                )
            })
    };

    // "Default (Marin)" first: what the relay uses when the agent has none set.
    {
        let p = pid.clone();
        let url = default_voice
            .as_ref()
            .map(|v| v.sample_url.clone())
            .filter(|u| !u.is_empty());
        let playing = url
            .as_deref()
            .is_some_and(|u| state.voice_sample_playing(u));
        list = list.child(
            row_base(eid("default".into()), &theme)
                .border_b_1()
                .border_color(palette.hairline)
                .child(play_button(
                    eid("play-default".into()),
                    url,
                    playing,
                    &theme,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(TEXT_BODY))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .truncate()
                                .child(default_label),
                        )
                        .child(
                            div()
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.text_muted)
                                .truncate()
                                .child(default_desc),
                        ),
                )
                .child(check(selected.is_none(), &theme))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.state
                        .update(cx, |s, cx| s.set_voice(p.clone(), None, cx));
                    if fold_agent_pick {
                        this.close_agent_pick(Some(window), cx);
                    }
                })),
        );
    }
    if voices.is_empty() {
        list = list.child(
            div()
                .px(px(14.0))
                .py(px(14.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child("Loading voices…"),
        );
    }
    let n = voices.len();
    for (i, v) in voices.into_iter().enumerate() {
        let on = selected.as_deref() == Some(v.id.as_str());
        let playing = state.voice_sample_playing(&v.sample_url);
        let p = pid.clone();
        let vid = v.id.clone();
        let url = v.sample_url.clone();
        let mut row = row_base(eid(v.id.clone()), &theme)
            .when(i + 1 < n, |el| {
                el.border_b_1().border_color(palette.hairline)
            })
            .child(play_button(
                eid(format!("play-{}", v.id)),
                Some(url).filter(|u| !u.is_empty()),
                playing,
                &theme,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(TEXT_BODY))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .truncate()
                            .child(v.name.clone()),
                    )
                    .when_some(
                        v.description.clone().filter(|d| !d.trim().is_empty()),
                        |el, d| {
                            el.child(
                                div()
                                    .text_size(px(TEXT_TINY))
                                    .text_color(theme.text_muted)
                                    .truncate()
                                    .child(d),
                            )
                        },
                    ),
            )
            .child(check(on, &theme));
        row = row.on_click(cx.listener(move |this, _, window, cx| {
            this.state
                .update(cx, |s, cx| s.set_voice(p.clone(), Some(vid.clone()), cx));
            if fold_agent_pick {
                this.close_agent_pick(Some(window), cx);
            }
        }));
        list = list.child(row);
    }
    list
}

/// The current voice's name for the agent pane's Voice row ("Default" when
/// the agent uses the relay's default; the raw id until the list arrives).
pub fn current_label(space: &Project, voices: &[Voice]) -> String {
    match space.voice.as_deref() {
        None => "Default".into(),
        Some(id) => voices
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.name.clone())
            .unwrap_or_else(|| id.to_string()),
    }
}
