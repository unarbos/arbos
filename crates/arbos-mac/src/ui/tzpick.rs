//! Settings → Bot → Timezone: the row and the zone list it unfolds.
//!
//! The same control as the agent page's Model row (`ui::hoverpick`): the label on the left, the
//! current value on the right, a chevron that turns up while the list is open, and the list
//! dropped in place under the row. A pick saves at once and folds the row.
//!
//! The first entry is always "Auto-detect", which clears the pin and hands the agents back the
//! zone the Mac itself reports.

use gpui::{
    div, prelude::*, px, AnyElement, Context, ElementId, Focusable as _, SharedString, Window,
};

use crate::ui::glass;
use crate::ui::highlight::{self, Shape};
use crate::ui::hoverpick::{self, LIST_MAX_HEIGHT};
use crate::ui::profiles::kit;
use crate::ui::sf_symbols;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, TEXT_BODY, TEXT_SMALL};

/// The Timezone row, with its list under it while it is open. `account` is the signed-in
/// account id — the zone is pinned per account, so a row drawn for nobody is not pinnable.
pub fn row(shell: &Shell, account: &str, theme: &Theme, cx: &mut Context<Shell>) -> AnyElement {
    let open = shell.timezone_pick;
    // Laid out here rather than through `kit::row_state`, for one reason: every other settings
    // row holds a short value ("On", "Dark · Matte") and `kit` lets the value win the space,
    // but a zone is long — `Auto-detect (America/Costa_Rica)` is wider than the 340 pt pane's
    // row — and that rule would spend the whole row on it and leave the word "Timezone" as an
    // ellipsis. The label is what a reader scans for, so here the label keeps its width and
    // the zone truncates. Everything else — the tile, the pill, the hung hairline, the
    // colours — is `kit`'s, so this row still moves with the rest of the list.
    let header = div()
        .id("settings-timezone-row-head")
        .relative()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(kit::GAP))
        .min_h(px(kit::ROW_MIN_H))
        .px(px(kit::INSET))
        .py(px(6.0))
        .text_size(px(TEXT_BODY))
        .text_color(theme.text)
        .child(highlight::layer(
            open,
            Shape::Nested(kit::CARD_RADIUS),
            theme,
        ))
        .child(glass::icon_tile("globe", theme))
        .child(div().flex_shrink_0().child("Timezone"))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_right()
                .text_size(px(TEXT_SMALL))
                .text_color(theme.label_muted())
                .child(SharedString::from(crate::timezone::row_value(account))),
        )
        .child(sf_symbols::glyph(
            if open { "chevron.up" } else { "chevron.right" },
            13.0,
            theme.label_faint(),
        ))
        .child(kit::hairline_below(theme))
        .cursor_pointer()
        .group(kit::HOT)
        .on_click(cx.listener(|this, _, window, cx| {
            if this.timezone_pick {
                this.close_timezone_pick(Some(window), cx);
            } else {
                this.open_timezone_pick(window, cx);
            }
        }));
    // Clicks inside stay inside, so the window's "click elsewhere" handlers do not fold the
    // list the moment the search field is used (`hoverpick::row` does the same).
    div()
        .id("settings-timezone-row")
        .relative()
        .flex()
        .flex_col()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(header)
        .children(open.then(|| panel(shell, account, theme, cx)))
        .when(open, |el| el.child(kit::hairline_below(theme)))
        .into_any_element()
}

/// The dropdown: a search field, then Auto-detect and the matching zones.
fn panel(shell: &Shell, account: &str, theme: &Theme, cx: &mut Context<Shell>) -> AnyElement {
    let query = shell.timezone_search_input.read(cx).text().to_string();
    let chosen = crate::timezone::chosen(account);
    let zones = crate::timezone::matching(&query);
    let search = div()
        .flex()
        .items_center()
        .gap(px(7.0))
        .h(px(34.0))
        .px(px(10.0))
        .border_b_1()
        .border_color(theme.border)
        .cursor_text()
        .child(sf_symbols::glyph("magnifyingglass", 13.0, theme.text_muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(shell.timezone_search_input.clone()),
        );
    let mut list = div()
        .id("settings-timezone-list")
        .flex()
        .flex_col()
        .max_h(px(LIST_MAX_HEIGHT))
        .overflow_y_scroll();
    // Auto-detect leads, and only while the query does not exclude it, so the way back to the
    // Mac's own zone is never more than a scroll to the top.
    let auto_shown =
        query.trim().is_empty() || "auto-detect".contains(&query.trim().to_lowercase());
    if auto_shown {
        let label = crate::timezone::auto_label(arbos_core::doc::device_time_zone().as_deref());
        list = list.child(
            hoverpick::item(
                ElementId::Name("settings-timezone-auto".into()),
                label,
                chosen.is_none(),
                theme,
            )
            .on_click(cx.listener(|this, _, window, cx| this.pick_timezone(None, window, cx))),
        );
    }
    if zones.is_empty() && !auto_shown {
        list = list.child(
            div()
                .px(px(12.0))
                .py(px(10.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(format!("No time zone matches “{}”.", query.trim())),
        );
    }
    for (i, zone) in zones.into_iter().enumerate() {
        list = list.child(
            hoverpick::item(
                ElementId::NamedInteger("settings-timezone-item".into(), i as u64),
                zone,
                chosen.as_deref() == Some(zone),
                theme,
            )
            .on_click(
                cx.listener(move |this, _, window, cx| this.pick_timezone(Some(zone), window, cx)),
            ),
        );
    }
    hoverpick::list_frame("settings-timezone-panel", theme)
        .child(search)
        .child(list)
        .into_any_element()
}

impl Shell {
    /// Drop the zone list down, with the search field focused and empty.
    pub(crate) fn open_timezone_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.timezone_pick {
            return;
        }
        self.timezone_pick = true;
        // The pane scrolls the opened row into view on the next frame, as a Model row does.
        self.agent_pick_reveal = true;
        self.timezone_search_input.update(cx, |i, cx| i.clear(cx));
        let handle = self.timezone_search_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Fold the zone list; `true` when it was open.
    pub(crate) fn close_timezone_pick(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.timezone_pick {
            return false;
        }
        self.timezone_pick = false;
        if let Some(window) = window {
            let search = self.timezone_search_input.read(cx).focus_handle(cx);
            if search.is_focused(window) {
                crate::ui::composer::Composer::focus(&self.composer, window, cx);
            }
        }
        cx.notify();
        true
    }

    /// Save the pick (`None` = back to auto-detect) and fold the row.
    pub(crate) fn pick_timezone(
        &mut self,
        zone: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let account = self.state.read(cx).me_id();
        if !account.is_empty() {
            crate::timezone::choose(&account, zone);
        }
        self.close_timezone_pick(Some(window), cx);
    }

    /// Enter in the zone search field picks the first match.
    pub(crate) fn pick_first_timezone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.timezone_search_input.read(cx).text().to_string();
        if let Some(first) = crate::timezone::matching(&query).first().copied() {
            self.pick_timezone(Some(first), window, cx);
        }
    }
}
