//! The small sheet a new tab is named in: its name, the glyph beside it,
//! and the glyph's colour, with the folder's own defaults already filled.
//! Shown once after the folder pick, and again from the tab's double-click
//! or menu. Enter keeps, Escape drops.

use crate::model::identity::{COLORS, GLYPHS, Identity};
use bezel::{
    gpui::{
        self, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, KeyBinding,
        MouseButton, Render, Window, actions, div, prelude::*, px,
    },
    motion::{Fade, Painter},
    theme::{Glass, SurfaceStyle, TextStyle, Theme, Typeset},
    ui::{
        icons,
        input::TextField,
        surface::Surfaced as _,
        widgets::{ButtonStyle, Buttons},
    },
};

actions!(arbos_tab_sheet, [Keep, Cancel]);

const KEY_CONTEXT: &str = "ArbosTabSheet";
const SURFACE: SurfaceStyle = SurfaceStyle::Glass(Glass::Regular);
const WIDTH: f32 = 340.;

/// A glyph button's side, and a colour swatch's diameter.
const GLYPH_CELL: f32 = 30.;
const SWATCH: f32 = 18.;

pub fn init(cx: &mut App) {
    crate::view::bind_field_editing(cx, KEY_CONTEXT, false);
    let ctx = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("enter", Keep, ctx),
        KeyBinding::new("escape", Cancel, ctx),
    ]);
}

pub enum TabSheetEvent {
    /// Keep this face on the project at the index.
    Keep(usize, Identity),
    Dismiss,
}

pub struct TabSheet {
    field: Entity<TextField>,
    /// Which project the sheet is about, while it is open.
    project: Option<usize>,
    /// The name the field opened with — the placeholder when it is cleared.
    fallback: String,
    glyph: usize,
    color: usize,
    pub open: bool,
}

impl EventEmitter<TabSheetEvent> for TabSheet {}

impl TabSheet {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| {
            TextField::new(cx)
                .with_frame(false)
                .with_key_context(KEY_CONTEXT)
                .with_placeholder("name")
        });
        Self {
            field,
            project: None,
            fallback: String::new(),
            glyph: 0,
            color: 0,
            open: false,
        }
    }

    /// Open on a project's current face. `fallback` is what the tab reads
    /// with no name of its own — the folder — and is offered as the
    /// placeholder so clearing the field means "back to that".
    pub fn show(
        &mut self,
        project: usize,
        identity: &Identity,
        fallback: String,
        cx: &mut Context<Self>,
    ) {
        self.open = true;
        self.project = Some(project);
        self.glyph = identity.glyph_index().unwrap_or(0);
        self.color = identity.color_index().unwrap_or(0);
        self.fallback = fallback.clone();
        self.field.update(cx, |field, cx| {
            field.set_placeholder(fallback, cx);
            field.set_content(identity.label().unwrap_or_default().to_string(), cx);
        });
        cx.notify();
    }

    fn identity(&self, cx: &App) -> Identity {
        let name = self.field.read(cx).content().trim().to_string();
        Identity {
            name: (!name.is_empty()).then_some(name),
            icon: GLYPHS[self.glyph].0.into(),
            color: COLORS[self.color].0.into(),
        }
    }

    fn keep(&mut self, _: &Keep, _: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project.take() else {
            return;
        };
        self.open = false;
        cx.emit(TabSheetEvent::Keep(project, self.identity(cx)));
        cx.notify();
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        self.dismiss(cx);
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.project = None;
        cx.emit(TabSheetEvent::Dismiss);
        cx.notify();
    }
}

impl Focusable for TabSheet {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.field.read(cx).focus_handle(cx)
    }
}

impl Render for TabSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div();
        }
        let theme = Theme::of(cx).clone();
        let painter = Painter::of(cx);
        let colour: Hsla = self.identity(cx).hsla();
        let glyphs = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(4.))
            .children(GLYPHS.iter().enumerate().map(|(ix, (_, path))| {
                let picked = ix == self.glyph;
                div()
                    .id(("tab-sheet-glyph", ix))
                    .size(px(GLYPH_CELL))
                    .rounded(px(Theme::control_radius()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(picked, |el| el.bg(theme.element_active))
                    .when(!picked, |el| el.hover(|el| el.bg(theme.element_hover)))
                    .child(icons::icon(path).size(px(14.)).text_color(if picked {
                        colour
                    } else {
                        theme.text_muted
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.glyph = ix;
                        cx.notify();
                    }))
            }));
        let swatches = div()
            .flex()
            .flex_row()
            .gap(px(8.))
            .children(COLORS.iter().enumerate().map(|(ix, (_, hex))| {
                let picked = ix == self.color;
                let tint: Hsla = gpui::rgb(*hex).into();
                div()
                    .id(("tab-sheet-color", ix))
                    .size(px(SWATCH + 6.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(picked, |el| el.border_1().border_color(theme.text))
                    .child(div().size(px(SWATCH)).rounded_full().bg(tint))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.color = ix;
                        cx.notify();
                    }))
            }));
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(120.))
            .bg(Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.35,
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.dismiss(cx)),
            )
            .child(
                div()
                    .w(px(WIDTH))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .p(px(14.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // The face as it will look on the tab, and the name.
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(GLYPH_CELL))
                                    .rounded(px(Theme::control_radius()))
                                    .bg(theme.element_active)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        icons::icon(GLYPHS[self.glyph].1)
                                            .size(px(15.))
                                            .text_color(colour),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(GLYPH_CELL))
                                    .px(px(10.))
                                    .rounded(px(Theme::control_radius()))
                                    .bg(theme.input_bg)
                                    .flex()
                                    .items_center()
                                    .text_style(TextStyle::Body)
                                    .cursor_text()
                                    .child(self.field.clone()),
                            ),
                    )
                    .child(glyphs)
                    .child(swatches)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.))
                            .text_style(TextStyle::Callout)
                            .child(
                                theme
                                    .button("Cancel", ButtonStyle::Ghost, None)
                                    .id("tab-sheet-cancel")
                                    .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                            )
                            .child(
                                theme
                                    .button(
                                        "Done",
                                        ButtonStyle::Prominent,
                                        Some(Fade::new(painter, "tab-sheet-done")),
                                    )
                                    .id("tab-sheet-done")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.keep(&Keep, window, cx)
                                    })),
                            ),
                    )
                    .surface(&theme, SURFACE),
            )
            .on_action(cx.listener(Self::keep))
            .on_action(cx.listener(Self::cancel))
    }
}
