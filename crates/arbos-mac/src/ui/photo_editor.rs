//! Crop and draw on a staged composer picture, before it is sent
//! (Scout, 2026-10-31; approved 2026-10-31).
//!
//! A centred plate over the dimmed window (`ui::modal::modal_overlay` with the
//! glass family's [`glass::plate`]), which is how every *committing* modal in
//! this app is presented. The media viewer's full-window lightbox is the other
//! candidate and it is the wrong one here: a lightbox is for looking, and this
//! screen changes what gets sent.
//!
//! Two modes, Crop and Draw, with Crop chosen on open — a crop changes the
//! frame the drawing lives in, so the cheap order is the default order.
//! Switching modes commits nothing and discards nothing: the crop rectangle
//! and the strokes are independent state (`ui::photo_edit`), both held until
//! Done, and a stroke outside the crop is kept and merely not drawn. The
//! iPhone half of the feature is the same design built on PencilKit.
//!
//! The floating controls wear the iPhone viewer's `ViewerChrome` recipe
//! ([`chrome_plate`]), **not** `media_viewer::{control, pill}`. Read the note
//! on [`chrome_plate`] before changing that: it is the one thing in this file
//! somebody is likely to "fix" back into a bug.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    actions, canvas, div, fill, img, point, prelude::*, px, size, AnyElement, Bounds, BoxShadow,
    ContentMask, Context, Div, ElementId, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, PathBuilder, Pixels,
    RenderImage, Role as AriaRole, Window,
};

use crate::ui::composer::TextTip;
use crate::ui::frost::{frosted, PANEL_BLUR};
use crate::ui::glass;
use crate::ui::highlight;
use crate::ui::modal::{self, modal_overlay};
use crate::ui::photo_edit::{
    drag_crop, handle_at, CropRect, Handle, Ink, PhotoEdit, Placement, Stroke, Tool,
};
use crate::ui::sf_symbols;
use crate::ui::theme::{Theme, PANEL_RADIUS, TEXT_SMALL};

actions!(
    photo_editor,
    [Commit, Undo, Original, NudgeLeft, NudgeRight, NudgeUp, NudgeDown]
);

/// The plate: a fixed size, not one fitted to the picture's aspect. A plate
/// that resized itself to the photograph would be more elegant right up to the
/// first crop, which would move the window out from under the pointer in the
/// middle of the drag that caused it.
const PLATE_W: f32 = 720.0;
const PLATE_H: f32 = 560.0;
/// [`glass::sheet_header`]'s height.
const HEAD: f32 = 60.0;
const CANVAS_W: f32 = PLATE_W;
const CANVAS_H: f32 = PLATE_H - HEAD;
/// [`glass::sheet`]'s card radius.
const RADIUS: f32 = PANEL_RADIUS + 6.0;
/// Behind the picture: near-black, so a photograph with transparent margins
/// reads as a picture on a mount and not as a hole in the plate.
const CANVAS_BG: f32 = 0.043;
/// A mount between the picture and the canvas's edges. Without it the crop
/// frame sits on the canvas boundary and the corner brackets are half clipped,
/// so the one control the mode is about has nowhere to be drawn. Photos and the
/// iPhone editor inset for the same reason.
const CANVAS_PAD: f32 = 24.0;
/// How far one arrow-key press moves the crop, in canvas points.
const NUDGE: f32 = 1.0;

/// The floating controls paint after the card rather than inside it.
///
/// On Glass the card is a frosted paint layer (`glass::plate`), and inside one
/// layer GPUI draws every sprite after every quad — so the photograph, which is
/// a sprite, covered the controls' plates and left white glyphs standing on a
/// bright picture with nothing behind them. Measured on the Mac: over the
/// picture the tool row's plate was absent, and it appeared only past the
/// picture's bottom edge. Matte has no such layer and was always right, which
/// is why this is invisible in every test and in the solid shot.
///
/// A deferred draw paints after the layer. It is the same remedy the staged
/// strip uses for its ring and its disc (`composer::tile`), one priority above
/// the sheet, which is itself deferred over that strip (`ui::modal::PRIORITY`).
const CHROME_PRIORITY: usize = modal::PRIORITY + 1;

/// How large the original is decoded for the canvas. The canvas is at most
/// 720 × 500 pt, which is 1440 × 1000 px on a Retina display, so this is the
/// first size at which the editor cannot be the blurry half of the feature.
pub const CANVAS_DECODE_PX: u32 = 1600;

const TITLE: &str = "Edit photo";
const ORIGINAL_LABEL: &str = "Original";
const UNDO_LABEL: &str = "Undo stroke";
/// The staged tile's tooltip, which is how the Mac entry point is taught: the
/// tile had no click at all before this feature.
pub const EDIT_TILE_LABEL: &str = "Edit photo";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Crop,
    Draw,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::Crop => "Crop",
            Mode::Draw => "Draw",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Mode::Crop => "photo-editor-mode-crop",
            Mode::Draw => "photo-editor-mode-draw",
        }
    }

    /// The rig's spelling (`ARBOS_COMPOSER_EDIT_PHOTO=crop|draw`).
    pub fn named(s: &str) -> Option<Self> {
        match s {
            "crop" | "1" => Some(Mode::Crop),
            "draw" => Some(Mode::Draw),
            _ => None,
        }
    }
}

/// What the pointer is doing between mouse-down and mouse-up.
enum Drag {
    /// Resizing or moving the crop. `start` is the rectangle as it was when the
    /// press landed, so the whole gesture is one pure function of the total
    /// delta and a long drag never accumulates rounding.
    Crop {
        handle: Handle,
        from: (f32, f32),
        start: CropRect,
    },
    /// Laying down a stroke; it is already the last entry in `draft.strokes`.
    Stroke,
    /// Rubbing out whole strokes under the pointer.
    Erase,
}

pub enum PhotoEditorEvent {
    /// Keep this edit: the composer flattens it onto the staged attachment.
    Done,
    /// Leave the attachment as it was when the sheet opened.
    Cancel,
}

pub struct PhotoEditor {
    /// Which staged attachment is being edited.
    pub staged: usize,
    picture: Option<Arc<RenderImage>>,
    /// The picture's own pixel size: the coordinate system the edit lives in.
    source: (f32, f32),
    mode: Mode,
    /// The working copy. Only Done hands it back, which is why Cancel needs no
    /// confirmation card — nothing is lost that cannot be reached again.
    draft: PhotoEdit,
    tool: Tool,
    ink: Ink,
    drag: Option<Drag>,
    /// The canvas's window bounds, written by the canvas element each frame and
    /// read by the pointer handlers. An event carries window coordinates and
    /// the crop lives in source pixels; this is the only bridge between them.
    canvas_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    focus: FocusHandle,
}

impl EventEmitter<PhotoEditorEvent> for PhotoEditor {}

impl Focusable for PhotoEditor {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PhotoEditor {
    /// Open on a staged picture, resuming `edit` — the empty edit on a first
    /// visit, the accepted one on a later visit, which is what makes reopening
    /// the sheet feel like going back instead of starting again.
    pub fn new(
        staged: usize,
        picture: Option<Arc<RenderImage>>,
        source: (u32, u32),
        edit: PhotoEdit,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            staged,
            picture,
            source: (source.0.max(1) as f32, source.1.max(1) as f32),
            mode: Mode::Crop,
            draft: edit,
            tool: Tool::Pen,
            ink: Ink::Red,
            drag: None,
            canvas_bounds: Rc::new(Cell::new(None)),
            focus: cx.focus_handle(),
        }
    }

    /// The accepted edit, for the composer to flatten.
    pub fn edit(&self) -> PhotoEdit {
        self.draft.clone()
    }

    /// What is on screen: the whole picture in Crop mode, so the rectangle can
    /// be dragged back out to the edges; the crop itself in Draw mode, so the
    /// canvas shows what will actually be sent.
    fn visible(&self) -> CropRect {
        match self.mode {
            Mode::Crop => CropRect::whole(self.source.0, self.source.1),
            Mode::Draw => self.draft.visible(self.source),
        }
    }

    fn placement(&self) -> Placement {
        Placement::fit(
            self.visible(),
            (CANVAS_W - 2.0 * CANVAS_PAD, CANVAS_H - 2.0 * CANVAS_PAD),
        )
        .shifted((CANVAS_PAD, CANVAS_PAD))
    }

    fn crop(&self) -> CropRect {
        self.draft.visible(self.source)
    }

    /// Where the crop rectangle is on the canvas, in canvas points. The
    /// picture is letterboxed inside the canvas, so its corner is not the
    /// canvas's corner and a test cannot guess where a handle is.
    #[cfg(test)]
    pub(crate) fn crop_in_view(&self) -> (f32, f32, f32, f32) {
        self.placement().rect_in_view(self.crop())
    }

    /// A pointer position in window coordinates as a point on the canvas.
    fn local(&self, at: gpui::Point<Pixels>) -> Option<(f32, f32)> {
        let bounds = self.canvas_bounds.get()?;
        Some((
            f32::from(at.x - bounds.origin.x),
            f32::from(at.y - bounds.origin.y),
        ))
    }

    // ----- actions ---------------------------------------------------------

    /// Rig: start the sheet on `mode` instead of Crop, so one shot can show
    /// the tool row. Only the launch flag calls it.
    pub fn rig_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.set_mode(mode, cx);
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            self.drag = None;
            cx.notify();
        }
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        if self.draft.undo() {
            cx.notify();
        }
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        if !self.draft.is_empty() {
            self.draft.clear();
            cx.notify();
        }
    }

    fn undo_action(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.undo(cx);
    }

    fn original_action(&mut self, _: &Original, _: &mut Window, cx: &mut Context<Self>) {
        self.reset(cx);
    }

    fn commit_action(&mut self, _: &Commit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PhotoEditorEvent::Done);
    }

    /// Arrow keys move the whole crop rectangle by one canvas point, which is
    /// the predictable reading of "nudge": every edge keeps its distance from
    /// the others, so a nudge can never resize what the drag just sized.
    fn nudge(&mut self, by: (f32, f32), cx: &mut Context<Self>) {
        if self.mode != Mode::Crop {
            return;
        }
        let scale = self.placement().scale.max(f32::EPSILON);
        let next = drag_crop(
            self.crop(),
            Handle::Body,
            (by.0 / scale, by.1 / scale),
            self.source,
        );
        self.draft.set_crop(next, self.source);
        cx.notify();
    }

    // ----- pointer ---------------------------------------------------------

    fn mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.local(event.position) else {
            return;
        };
        let placement = self.placement();
        let source_at = placement.to_source(at);
        match self.mode {
            Mode::Crop => {
                let rect = placement.rect_in_view(self.crop());
                if let Some(handle) = handle_at(rect, at) {
                    self.drag = Some(Drag::Crop {
                        handle,
                        from: source_at,
                        start: self.crop(),
                    });
                }
            }
            Mode::Draw if self.tool == Tool::Eraser => {
                self.draft
                    .erase_at(source_at, self.eraser_reach(&placement));
                self.drag = Some(Drag::Erase);
            }
            Mode::Draw => {
                self.draft.strokes.push(Stroke {
                    ink: self.ink,
                    // The screen width turned into source pixels, so a line
                    // drawn on a zoomed-out photo is as thick in the sent file
                    // as it looked while it was being drawn.
                    width: self.tool.screen_width() / placement.scale,
                    alpha: self.tool.alpha(),
                    points: vec![source_at],
                });
                self.drag = Some(Drag::Stroke);
            }
        }
        cx.notify();
    }

    fn eraser_reach(&self, placement: &Placement) -> f32 {
        Tool::Eraser.screen_width() / 2.0 / placement.scale.max(f32::EPSILON)
    }

    /// Lives on the backdrop, not on the plate: GPUI delivers `on_mouse_move`
    /// only while the element's own hitbox is hovered, and dragging a crop
    /// corner outward takes the pointer off the plate almost immediately.
    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_none() {
            return;
        }
        let Some(at) = self.local(event.position) else {
            return;
        };
        let placement = self.placement();
        let source_at = placement.to_source(at);
        match self.drag.as_ref() {
            Some(Drag::Crop {
                handle,
                from,
                start,
            }) => {
                let delta = (source_at.0 - from.0, source_at.1 - from.1);
                let next = drag_crop(*start, *handle, delta, self.source);
                self.draft.set_crop(next, self.source);
            }
            Some(Drag::Stroke) => {
                if let Some(stroke) = self.draft.strokes.last_mut() {
                    // One sample per point of travel is plenty; closer samples
                    // only cost the flatten pass work no eye can see.
                    let moved = stroke
                        .points
                        .last()
                        .map(|p| (p.0 - source_at.0).abs() + (p.1 - source_at.1).abs())
                        .unwrap_or(f32::MAX);
                    if moved * placement.scale >= 1.0 {
                        stroke.points.push(source_at);
                    }
                }
            }
            Some(Drag::Erase) => {
                self.draft
                    .erase_at(source_at, self.eraser_reach(&placement));
            }
            None => return,
        }
        cx.notify();
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            // A press that laid down a single point is a dot, not a mistake,
            // so it stays. An empty stroke cannot happen: mouse-down seeds one.
            cx.notify();
        }
    }
}

// ---------------------------------------------------------------------------
// The floating controls' plate
// ---------------------------------------------------------------------------

/// The plate under a control that floats over the picture: the iPhone viewer's
/// `ViewerChrome` recipe, which both platforms share for this screen.
///
/// **Do not replace this with `media_viewer::control` / `media_viewer::pill`.**
/// Those are a bare `white(0.16)`–`white(0.18)` wash with no plate, no rim and
/// no blur, and they are readable only because the viewer paints a black 0.88
/// backdrop under everything it draws. This editor has no such backdrop: the
/// photograph reaches the controls, and a white 16 % pill over a pale
/// screenshot — which is most of what gets cropped — cannot be read at all. So
/// the controls take a dark plate, a white wash and a white rim, which is a
/// recipe built to carry white glyphs over an arbitrary picture.
fn chrome_plate<E: Styled>(el: E, radius: f32, theme: &Theme) -> E {
    /// Black over the glass: what keeps a white picture from showing through.
    const GLASS_PLATE: f32 = 0.6;
    /// White over that plate.
    const GLASS_WASH: f32 = 0.08;
    /// Matte: the face's grey level, opaque.
    const SOLID_LEVEL: f32 = 0.16;
    const RIM: f32 = 0.18;
    const LIFT: f32 = 0.28;

    let face = if theme.is_frost() {
        // One fill cannot be two layers, so the plate and the wash are
        // composited to the colour GPUI would have painted them as.
        highlight::over(
            Theme::with_alpha(gpui::black(), GLASS_PLATE),
            Theme::with_alpha(gpui::white(), GLASS_WASH),
        )
    } else {
        gpui::hsla(0.0, 0.0, SOLID_LEVEL, 1.0)
    };
    el.rounded(px(radius)).bg(face).shadow(vec![
        BoxShadow::new(px(0.0), px(0.0), white(RIM))
            .spread_radius(px(1.0))
            .inset(),
        BoxShadow::new(px(0.0), px(1.0), Theme::with_alpha(gpui::black(), LIFT))
            .blur_radius(px(3.0)),
    ])
}

fn white(a: f32) -> Hsla {
    Theme::with_alpha(gpui::white(), a)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

impl Render for PhotoEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let body = div()
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .bg(gpui::hsla(0.0, 0.0, CANVAS_BG, 1.0))
            .child(self.canvas_layer(cx))
            .child(lift(self.segment(&theme, cx)))
            .children(self.mode_controls(&theme, cx).into_iter().map(lift));
        let card = div()
            .id("photo-editor")
            .debug_selector(|| "photo-editor".into())
            .key_context("PhotoEditor")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::undo_action))
            .on_action(cx.listener(Self::original_action))
            .on_action(cx.listener(Self::commit_action))
            .on_action(cx.listener(|this, _: &NudgeLeft, _, cx| this.nudge((-NUDGE, 0.0), cx)))
            .on_action(cx.listener(|this, _: &NudgeRight, _, cx| this.nudge((NUDGE, 0.0), cx)))
            .on_action(cx.listener(|this, _: &NudgeUp, _, cx| this.nudge((0.0, -NUDGE), cx)))
            .on_action(cx.listener(|this, _: &NudgeDown, _, cx| this.nudge((0.0, NUDGE), cx)))
            .w(px(PLATE_W))
            .h(px(PLATE_H))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(RADIUS))
            .child(self.header(&theme, cx))
            .child(body);
        let card = glass::plate(card, &theme);
        let weak = cx.weak_entity();
        modal_overlay(
            "photo-editor-backdrop",
            // A click outside is Cancel, like every other sheet in the app.
            // The edit is not lost by it: it was never taken off the staged
            // attachment in the first place.
            move |_, cx| {
                let _ = weak.update(cx, |_, cx| cx.emit(PhotoEditorEvent::Cancel));
            },
            frosted(RADIUS, PANEL_BLUR, card),
        )
        // The backdrop spans the window, so a drag that leaves the plate keeps
        // being delivered. See the note on [`PhotoEditor::mouse_move`].
        .on_mouse_move(cx.listener(Self::mouse_move))
        .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
    }
}

impl PhotoEditor {
    /// [`glass::sheet_header`]: Cancel · "Edit photo" · Done — the app's own
    /// bar, the same one every other sheet wears.
    fn header(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let cancel = glass::sheet_button("photo-editor-cancel", "Cancel", theme)
            .debug_selector(|| "photo-editor-cancel".into())
            .role(AriaRole::Button)
            .on_click(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
                cx.emit(PhotoEditorEvent::Cancel);
            }));
        let done = div()
            .id("photo-editor-done")
            .debug_selector(|| "photo-editor-done".into())
            .h(px(34.0))
            .px(px(18.0))
            .rounded_full()
            .bg(theme.primary)
            .flex()
            .items_center()
            .text_size(px(TEXT_SMALL))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.on_primary)
            .cursor_pointer()
            .hover(|s| s.opacity(0.88))
            .role(AriaRole::Button)
            .child("Done")
            .on_click(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
                cx.emit(PhotoEditorEvent::Done);
            }));
        glass::sheet_header(TITLE, Some(cancel), Some(done), theme)
            .border_b_1()
            .border_color(theme.border)
    }

    /// The two-segment Crop | Draw control, floating at the top of the canvas.
    /// A one-pointer drag has to mean one thing, and on a drawing surface it
    /// means "draw" — which leaves nothing for "drag the crop rectangle". So
    /// the modes are exclusive instead of one canvas with tools.
    fn segment(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let mut inner = chrome_plate(div().p(px(3.0)).flex().gap(px(2.0)), 17.0, theme);
        for mode in [Mode::Crop, Mode::Draw] {
            let on = self.mode == mode;
            inner = inner.child(
                div()
                    .id(ElementId::Name(mode.id().into()))
                    .debug_selector(move || mode.id().into())
                    .h(px(28.0))
                    .px(px(18.0))
                    .rounded(px(14.0))
                    .flex()
                    .items_center()
                    .text_size(px(TEXT_SMALL))
                    .font_weight(if on {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .bg(if on {
                        gpui::white()
                    } else {
                        gpui::transparent_black()
                    })
                    .text_color(if on { gpui::black() } else { white(0.72) })
                    .cursor_pointer()
                    .role(AriaRole::Button)
                    .child(mode.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_mode(mode, cx);
                    })),
            );
        }
        floating(div().top(px(12.0)).left_0().right_0().justify_center()).child(swallow(inner))
    }

    /// `Original` is in both modes because it clears both halves of the edit,
    /// and the tool row is only in Draw. Nothing else is shared, so there is no
    /// toolbar for the two modes to fight over.
    fn mode_controls(&self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = vec![self.original_pill(theme, cx).into_any_element()];
        if self.mode == Mode::Draw {
            out.push(self.tool_row(theme, cx).into_any_element());
        }
        out
    }

    /// `Original` — named for the state it returns you to, not for the act of
    /// resetting, because that state is the whole model. Dimmed while the
    /// picture is untouched.
    fn original_pill(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let live = !self.draft.is_empty();
        let pill = div()
            .id("photo-editor-original")
            .debug_selector(|| "photo-editor-original".into())
            .h(px(34.0))
            .px(px(16.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_size(px(TEXT_SMALL))
            .font_weight(FontWeight::MEDIUM)
            .text_color(gpui::white())
            .when(!live, |el| el.opacity(0.45))
            .when(live, |el| el.cursor_pointer())
            .role(AriaRole::Button)
            .child(sf_symbols::glyph(
                "arrow.counterclockwise",
                14.0,
                gpui::white(),
            ))
            .child(ORIGINAL_LABEL)
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.reset(cx);
            }));
        floating(div().left(px(16.0)).bottom(px(16.0)))
            .child(swallow(chrome_plate(pill, 17.0, theme)))
    }

    /// Mac draws its own tool row: there is no `PKToolPicker` outside UIKit and
    /// this app is GPUI. That is also why the row is three tools, five inks and
    /// one undo and nothing else — every control on it is code somebody has to
    /// maintain, and the width is a property of the tool rather than a slider.
    fn tool_row(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let mut row = div()
            .id("photo-editor-tools")
            .debug_selector(|| "photo-editor-tools".into())
            .h(px(48.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(12.0));
        for tool in Tool::ALL {
            let on = self.tool == tool;
            row = row.child(
                div()
                    .id(ElementId::Name(tool.label().into()))
                    .size(px(32.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(if on {
                        gpui::white()
                    } else {
                        gpui::transparent_black()
                    })
                    .role(AriaRole::Button)
                    .aria_label(tool.label())
                    .tooltip(move |_, cx| cx.new(|_| TextTip(tool.label().into())).into())
                    .child(sf_symbols::glyph(
                        tool.symbol(),
                        17.0,
                        if on { gpui::black() } else { gpui::white() },
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.tool = tool;
                        cx.notify();
                    })),
            );
        }
        row = row.child(rule());
        for ink in Ink::ALL {
            let on = self.ink == ink;
            row = row.child(
                div()
                    .id(ElementId::Name(ink.label().into()))
                    .size(px(18.0))
                    .rounded_full()
                    .flex_shrink_0()
                    .bg(ink.hsla())
                    .cursor_pointer()
                    .role(AriaRole::Button)
                    .aria_label(ink.label())
                    .tooltip(move |_, cx| cx.new(|_| TextTip(ink.label().into())).into())
                    // The chosen swatch wears a dark gap and then a white ring,
                    // so the mark reads on the white ink and on the blue alike.
                    .shadow(if on {
                        vec![
                            BoxShadow::new(px(0.0), px(0.0), gpui::hsla(0.0, 0.0, 0.04, 1.0))
                                .spread_radius(px(2.0)),
                            BoxShadow::new(px(0.0), px(0.0), gpui::white()).spread_radius(px(3.5)),
                        ]
                    } else {
                        vec![BoxShadow::new(px(0.0), px(0.0), white(0.35))
                            .spread_radius(px(1.0))
                            .inset()]
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.ink = ink;
                        // Picking a colour while the eraser is up means the
                        // user wants to draw in it, not to go on erasing.
                        if this.tool == Tool::Eraser {
                            this.tool = Tool::Pen;
                        }
                        cx.notify();
                    })),
            );
        }
        // PencilKit's compact palette carries the inks, the colour and the
        // eraser but no undo (Notes puts undo in its own navigation bar), so
        // both platforms supply one. On Mac it rides the row.
        let can_undo = !self.draft.strokes.is_empty();
        row = row.child(rule()).child(
            div()
                .id("photo-editor-undo")
                .debug_selector(|| "photo-editor-undo".into())
                .size(px(32.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .when(can_undo, |el| el.cursor_pointer())
                .when(!can_undo, |el| el.opacity(0.4))
                .role(AriaRole::Button)
                .aria_label(UNDO_LABEL)
                .tooltip(|_, cx| cx.new(|_| TextTip(UNDO_LABEL.into())).into())
                .child(sf_symbols::glyph(
                    "arrow.uturn.backward",
                    16.0,
                    gpui::white(),
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.undo(cx);
                })),
        );
        floating(div().bottom(px(16.0)).left_0().right_0().justify_center())
            .child(swallow(chrome_plate(row, 24.0, theme)))
    }

    /// The picture, the strokes and — in Crop mode — the crop overlay.
    ///
    /// The photograph is a `RenderImage` placed by the same [`Placement`] the
    /// pointer maths uses, so what is drawn and what is hit-tested cannot
    /// disagree. The strokes and the overlay are painted by a `canvas` over it:
    /// GPUI has no freehand element, but it does have `PathBuilder::stroke`.
    fn canvas_layer(&self, cx: &mut Context<Self>) -> Div {
        let placement = self.placement();
        let visible = self.visible();
        let (pw, ph) = (
            self.source.0 * placement.scale,
            self.source.1 * placement.scale,
        );
        let mut layer = div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .debug_selector(|| "photo-editor-canvas".into())
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down));
        if let Some(image) = self.picture.clone() {
            layer = layer.child(
                img(image)
                    .absolute()
                    .left(px(placement.origin.0))
                    .top(px(placement.origin.1))
                    .w(px(pw))
                    .h(px(ph))
                    // The box is already the picture's aspect, so Contain fills
                    // it exactly and never letterboxes.
                    .object_fit(ObjectFit::Contain),
            );
        }
        let strokes = self.draft.strokes.clone();
        let crop = self.crop();
        let overlay = self.mode == Mode::Crop;
        let bounds_cell = self.canvas_bounds.clone();
        layer.child(
            canvas(
                move |bounds, _, _| {
                    bounds_cell.set(Some(bounds));
                },
                move |bounds, _, window, _| {
                    let o = bounds.origin;
                    let at = |p: (f32, f32)| point(o.x + px(p.0), o.y + px(p.1));
                    // Its own layer, because within one layer GPUI draws every
                    // sprite after every quad and path — so the photograph,
                    // which is a sprite, would cover the ink and the crop
                    // frame that are drawn here. (The staged strip solves the
                    // same problem with `deferred`; that is not available
                    // inside a paint closure, and a layer is the narrower fix.)
                    window.paint_layer(bounds, |window| {
                        // The visible window clips the strokes, which is also
                        // the proof that a stroke outside the crop is kept
                        // rather than deleted: go back to Crop and it is still
                        // there.
                        let (vx, vy, vw, vh) = placement.rect_in_view(visible);
                        let clip = Bounds::new(at((vx, vy)), size(px(vw), px(vh)));
                        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                            for stroke in &strokes {
                                paint_stroke(window, stroke, &placement, o);
                            }
                        });
                        if overlay {
                            paint_crop(window, placement.rect_in_view(crop), bounds);
                        }
                    });
                },
            )
            .absolute()
            .size_full(),
        )
    }
}

/// Where a floating control sits over the canvas. The row itself takes **no**
/// mouse events: it stretches across the canvas to centre its child, and a
/// hitbox that wide swallows every press in the band it covers — which is the
/// band the top crop handle and the first stroke of a drawing live in. The
/// press is stopped by the control instead; see [`swallow`].
fn floating(el: Div) -> Div {
    el.absolute().flex()
}

/// Paint a floating control above the card's frosted layer, keeping its layout
/// where it is. See [`CHROME_PRIORITY`].
fn lift(el: impl IntoElement) -> AnyElement {
    gpui::deferred(el)
        .with_priority(CHROME_PRIORITY)
        .into_any_element()
}

/// A press on this control stops here rather than reaching the canvas: a press
/// on a gap in the tool row must not start a stroke behind it.
fn swallow<E: InteractiveElement>(el: E) -> E {
    el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

fn rule() -> Div {
    div().w(px(1.0)).h(px(22.0)).bg(white(0.2))
}

/// One stroke, in canvas points.
fn paint_stroke(
    window: &mut Window,
    stroke: &Stroke,
    placement: &Placement,
    o: gpui::Point<Pixels>,
) {
    let width = (stroke.width * placement.scale).max(1.0);
    let colour = Theme::with_alpha(stroke.ink.hsla(), stroke.alpha);
    let at = |p: (f32, f32)| {
        let v = placement.to_view(p);
        point(o.x + px(v.0), o.y + px(v.1))
    };
    match stroke.points.as_slice() {
        [] => {}
        // A single press is a dot. A zero-length path paints nothing, so the
        // dot is a round quad instead.
        [only] => {
            let c = at(*only);
            let r = width / 2.0;
            window.paint_quad(
                fill(
                    Bounds::new(point(c.x - px(r), c.y - px(r)), size(px(width), px(width))),
                    colour,
                )
                .corner_radii(px(r)),
            );
        }
        points => {
            let mut pb = PathBuilder::stroke(px(width));
            pb.move_to(at(points[0]));
            for p in &points[1..] {
                pb.line_to(at(*p));
            }
            if let Ok(path) = pb.build() {
                window.paint_path(path, colour);
            }
        }
    }
}

/// The crop overlay: shade outside it, a thirds grid inside, a 1 pt rim, four
/// corner brackets and four mid-edge bars.
///
/// Every light part is drawn over a dark one inflated by [`HALO`]. Plain white
/// is what the mockup shows and what Photos appears to draw, but a crop sits
/// over whatever the picture happens to be, and the first thing most people
/// crop is a screenshot — a white frame on a white page is not there at all.
/// Measured, not guessed: the first build of this overlay was invisible in the
/// shot of a pale picture, and nothing but the halo changed between that shot
/// and the one in the pull request.
fn paint_crop(window: &mut Window, rect: (f32, f32, f32, f32), bounds: Bounds<Pixels>) {
    /// How far a corner bracket runs along each edge.
    const BRACKET: f32 = 22.0;
    /// How long a mid-edge bar is.
    const BAR: f32 = 28.0;
    /// The brackets' and bars' thickness.
    const THICK: f32 = 3.0;
    /// How far the dark backing stands out past the light part it carries.
    const HALO: f32 = 1.5;

    let o = bounds.origin;
    let (x, y, w, h) = rect;
    let (cw, ch) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let mut quad = |bx: f32, by: f32, bw: f32, bh: f32, c: Hsla| {
        if bw <= 0.0 || bh <= 0.0 {
            return;
        }
        window.paint_quad(fill(
            Bounds::new(point(o.x + px(bx), o.y + px(by)), size(px(bw), px(bh))),
            c,
        ));
    };
    // Four rectangles of shade around the crop, not one over the lot with a
    // hole in it: GPUI quads do not punch holes.
    let dim = Theme::with_alpha(gpui::black(), 0.55);
    for (rx, ry, rw, rh) in [
        (0.0, 0.0, cw, y),
        (0.0, y + h, cw, (ch - y - h).max(0.0)),
        (0.0, y, x, h),
        (x + w, y, (cw - x - w).max(0.0), h),
    ] {
        quad(rx, ry, rw, rh, dim);
    }
    // One light bar on its dark backing. Two quads, because a quad has no
    // outline and a shadow on a 1 pt line is a blur nobody can see.
    let mut lit = |bx: f32, by: f32, bw: f32, bh: f32, light: Hsla, backing: Hsla| {
        quad(
            bx - HALO,
            by - HALO,
            bw + 2.0 * HALO,
            bh + 2.0 * HALO,
            backing,
        );
        quad(bx, by, bw, bh, light);
    };
    // Rule of thirds: the crop guide everybody already knows. Quieter than the
    // frame — it is a reading aid, not an edge.
    let grid = white(0.5);
    let grid_backing = Theme::with_alpha(gpui::black(), 0.22);
    for i in 1..3 {
        let f = i as f32 / 3.0;
        lit(x, y + h * f, w, 1.0, grid, grid_backing);
        lit(x + w * f, y, 1.0, h, grid, grid_backing);
    }
    // The rim, just inside the edge, so the boundary reads wherever it lands.
    let rim = white(0.8);
    let backing = Theme::with_alpha(gpui::black(), 0.55);
    lit(x, y, w, 1.0, rim, backing);
    lit(x, y + h - 1.0, w, 1.0, rim, backing);
    lit(x, y, 1.0, h, rim, backing);
    lit(x + w - 1.0, y, 1.0, h, rim, backing);
    // Corner brackets, drawn inside the rectangle so a crop sitting on the
    // picture's edge does not lose them off the canvas.
    let b = BRACKET.min(w / 2.0).min(h / 2.0);
    for (corner_x, corner_y, sx, sy) in [
        (x, y, 1.0, 1.0),
        (x + w, y, -1.0, 1.0),
        (x, y + h, 1.0, -1.0),
        (x + w, y + h, -1.0, -1.0),
    ] {
        let (hx, hy) = (
            if sx > 0.0 { corner_x } else { corner_x - b },
            if sy > 0.0 { corner_y } else { corner_y - THICK },
        );
        let (vx, vy) = (
            if sx > 0.0 { corner_x } else { corner_x - THICK },
            if sy > 0.0 { corner_y } else { corner_y - b },
        );
        lit(hx, hy, b, THICK, gpui::white(), backing);
        lit(vx, vy, THICK, b, gpui::white(), backing);
    }
    // The four side handles, the same affordance Photos uses.
    let bar_w = BAR.min(w / 3.0);
    let bar_h = BAR.min(h / 3.0);
    let mid_x = x + (w - bar_w) / 2.0;
    let mid_y = y + (h - bar_h) / 2.0;
    lit(mid_x, y, bar_w, THICK, gpui::white(), backing);
    lit(mid_x, y + h - THICK, bar_w, THICK, gpui::white(), backing);
    lit(x, mid_y, THICK, bar_h, gpui::white(), backing);
    lit(x + w - THICK, mid_y, THICK, bar_h, gpui::white(), backing);
}
