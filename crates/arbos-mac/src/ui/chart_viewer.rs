//! The expanded chart ([feedback chart-bubble-mac-2026-10-26]): an in-window overlay (the
//! three-pane rule: no new window) — a dimmed scrim, a panel with the title and subtitle,
//! **Copy data** · **Save PNG** pills and the close disc, the plot at the panel's width with
//! the same hover readout as the card, and a Chart | Data switch whose Data view is the
//! chart's table. Esc, the disc or a click on the scrim closes it.

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, AnyElement, Context, FocusHandle, FontWeight, MouseButton, SharedString,
};

use arbos_core::chart::LayoutStyle;

use super::chart_card::{self, icon_square, legend, ChartUi};
use super::icons::icon;
use super::shell::Shell;
use super::theme::{Appearance, Theme};
use crate::app::AppState;
use crate::model::now_ms;

/// Layout style of the big plot (a size up from the card).
pub const VIEWER_STYLE: LayoutStyle = LayoutStyle {
    font: 12.0,
    label_font: 11.5,
    line_width: 2.4,
    glyph_em: 0.56,
};
/// How long the Copy data pill shows its check.
pub const COPIED_MS: u64 = 1_500;
const PANEL_MAX_W: f32 = 1140.0;
const PANEL_MAX_H: f32 = 706.0;

/// What the overlay shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChartViewer {
    pub chat_id: Option<String>,
    pub part_id: String,
    /// The `chart` part's JSON.
    pub spec: String,
    /// Chart (false) or Data (true).
    pub data_view: bool,
    /// Unix ms of the last Copy data click (the pill shows a check for [`COPIED_MS`]).
    pub copied_at: Option<u64>,
}

impl ChartViewer {
    pub fn copied_now(&self) -> bool {
        self.copied_at
            .is_some_and(|at| now_ms().saturating_sub(at) < COPIED_MS)
    }
}

fn pill(
    id: &'static str,
    glyph: &'static str,
    label: &str,
    tint: Option<gpui::Hsla>,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let color = tint.unwrap_or(theme.text);
    let hover_bg = theme.surface_2;
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .h(px(28.0))
        .px(px(12.0))
        .rounded_full()
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface_1)
        .flex()
        .items_center()
        .gap(px(6.0))
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .text_size(px(12.5))
        .text_color(color)
        .child(icon(glyph, 14.0, tint.unwrap_or(theme.text_muted)))
        .child(label.to_owned())
}

/// The overlay, laid over the shell root while `state.chart_viewer` is `Some`.
pub fn overlay(shell: &Shell, focus: &FocusHandle, cx: &mut Context<Shell>) -> AnyElement {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let Some(viewer) = state.chart_viewer.clone() else {
        return div().into_any_element();
    };
    let Some(prepared) = chart_card::prepare(&viewer.spec) else {
        return div().into_any_element();
    };
    let model = &prepared.model;
    let title = prepared
        .chart
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "Chart".into());
    let subtitle = prepared.chart.subtitle.clone();
    let scrim = match theme.appearance {
        Appearance::Dark => 0.58,
        Appearance::Light => 0.40,
    };

    let close = shell.state.clone();
    let close_disc = shell.state.clone();
    let copy_state = shell.state.clone();
    let save_state = shell.state.clone();
    let seg_chart = shell.state.clone();
    let seg_data = shell.state.clone();
    let this = cx.entity().downgrade();
    let ui = Rc::new(ChartUi {
        on_hover: Rc::new(move |_, cx| {
            let _ = this.update(cx, |_, cx| cx.notify());
        }),
        on_save: Rc::new({
            let state = save_state.clone();
            move |spec, title, _, cx| chart_card::save_png(&spec, &title, &state, cx)
        }),
        on_expand: Rc::new(|_, _, _, _| {}),
    });
    let tsv = prepared.chart.table_tsv();
    let copied = viewer.copied_now();
    let spec_for_save = viewer.spec.clone();
    let title_for_save = title.clone();

    let header = div()
        .flex()
        .items_start()
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(17.0))
                        .line_height(px(22.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .debug_selector(|| "chart-viewer-title".into())
                        .child(title.clone()),
                )
                .when_some(subtitle, |el, s| {
                    el.child(
                        div()
                            .text_size(px(12.5))
                            .line_height(px(17.0))
                            .text_color(theme.text_muted)
                            .mt(px(3.0))
                            .child(s),
                    )
                }),
        )
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    pill(
                        "chart-viewer-copy",
                        if copied { "check" } else { "copy" },
                        if copied { "Copied" } else { "Copy data" },
                        copied.then_some(theme.online),
                        &theme,
                    )
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(tsv.clone()));
                        copy_state.update(cx, |s, cx| s.chart_viewer_copied(cx));
                    }),
                )
                .child(
                    pill("chart-viewer-save", "download", "Save PNG", None, &theme).on_click(
                        move |_, _, cx| {
                            cx.stop_propagation();
                            chart_card::save_png(&spec_for_save, &title_for_save, &save_state, cx);
                        },
                    ),
                )
                .child(
                    icon_square(
                        SharedString::from("chart-viewer-close"),
                        "xmark",
                        Some(theme.text),
                        &theme,
                    )
                    .rounded_full()
                    .size(px(28.0))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        close_disc.update(cx, |s, cx| s.close_chart_viewer(cx));
                    }),
                ),
        );

    let seg = |id: &'static str, label: &'static str, on: bool, theme: &Theme| {
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .px(px(12.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .text_size(px(12.0))
            .when(on, |el| {
                el.bg(theme.bg)
                    .text_color(theme.text)
                    .font_weight(FontWeight::MEDIUM)
            })
            .when(!on, |el| el.text_color(theme.text_muted))
            .child(label)
    };
    let switch = div()
        .flex()
        .p(px(2.0))
        .rounded(px(8.0))
        .bg(theme.surface_2)
        .child(
            seg("chart-viewer-seg-chart", "Chart", !viewer.data_view, &theme).on_click(
                move |_, _, cx| {
                    cx.stop_propagation();
                    seg_chart.update(cx, |s, cx| s.chart_viewer_show_data(false, cx));
                },
            ),
        )
        .child(
            seg("chart-viewer-seg-data", "Data", viewer.data_view, &theme).on_click(
                move |_, _, cx| {
                    cx.stop_propagation();
                    seg_data.update(cx, |s, cx| s.chart_viewer_show_data(true, cx));
                },
            ),
        );

    let body: AnyElement = if viewer.data_view {
        data_table(&prepared.chart, &theme)
    } else {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .when(model.show_legend, |el| {
                el.child(legend(model, &theme, 12.5, 9.0))
            })
            .child(div().flex_1().min_h_0().child(chart_card::plot(
                SharedString::from("chart-viewer-plot"),
                prepared.clone(),
                theme.clone(),
                None,
                VIEWER_STYLE,
                Some(ui),
            )))
            .into_any_element()
    };

    let panel = div()
        .id("chart-viewer-panel")
        .debug_selector(|| "chart-viewer-panel".into())
        .w_full()
        .max_w(px(PANEL_MAX_W))
        .h_full()
        .max_h(px(PANEL_MAX_H))
        .rounded(px(16.0))
        .bg(theme.bg)
        .border_1()
        .border_color(theme.border)
        .shadow_lg()
        .p(px(24.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(header)
        .child(body)
        .child(div().flex().justify_start().child(switch));

    div()
        .id("chart-viewer")
        .debug_selector(|| "chart-viewer".into())
        .key_context("ChartViewer")
        .track_focus(focus)
        .absolute()
        .inset_0()
        .occlude()
        .bg(Theme::with_alpha(gpui::black(), scrim))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            close.update(cx, |s, cx| s.close_chart_viewer(cx));
        })
        .flex()
        .items_center()
        .justify_center()
        .p(px(44.0))
        .child(panel)
        .into_any_element()
}

/// The Data view: the chart's table in the markdown table's style (header on `code_bg`,
/// muted cells, hairline grid), scrolling past the panel.
fn data_table(chart: &arbos_core::chart::Chart, theme: &Theme) -> AnyElement {
    let (head, rows) = chart.table_cells();
    let cell = |text: String, head: bool, i: usize, theme: &Theme| {
        div()
            .flex_1()
            .min_w(px(72.0))
            .px(px(10.0))
            .py(px(6.0))
            .when(i > 0, |el| el.border_l_1().border_color(theme.border))
            .text_size(px(12.5))
            .text_color(if head { theme.text } else { theme.text_muted })
            .when(head, |el| el.font_weight(FontWeight::SEMIBOLD))
            .child(text)
    };
    let mut table = div()
        .flex()
        .flex_col()
        .w_full()
        .rounded(px(10.0))
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .debug_selector(|| "chart-viewer-table".into());
    let mut head_row = div().flex().bg(theme.code_bg);
    for (i, h) in head.iter().enumerate() {
        head_row = head_row.child(cell(h.clone(), true, i, theme));
    }
    table = table.child(head_row);
    for row in rows {
        let mut r = div().flex().border_t_1().border_color(theme.border);
        for (i, c) in row.into_iter().enumerate() {
            r = r.child(cell(c, false, i, theme));
        }
        table = table.child(r);
    }
    div()
        .id("chart-viewer-data")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .child(table)
        .into_any_element()
}

/// `AppState` side of the overlay.
impl AppState {
    pub fn open_chart_viewer(&mut self, viewer: ChartViewer, cx: &mut Context<Self>) {
        self.chart_viewer = Some(viewer);
        self.chart_viewer_generation += 1;
        cx.notify();
    }

    pub fn close_chart_viewer(&mut self, cx: &mut Context<Self>) {
        self.chart_viewer = None;
        cx.notify();
    }

    pub fn chart_viewer_show_data(&mut self, data: bool, cx: &mut Context<Self>) {
        if let Some(v) = self.chart_viewer.as_mut() {
            v.data_view = data;
            cx.notify();
        }
    }

    /// Copy data clicked: the pill shows a check for [`COPIED_MS`], then reads Copy data again.
    pub fn chart_viewer_copied(&mut self, cx: &mut Context<Self>) {
        if let Some(v) = self.chart_viewer.as_mut() {
            v.copied_at = Some(now_ms());
            cx.notify();
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(COPIED_MS))
                    .await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }
}
