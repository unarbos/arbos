//! Settings › Permissions: the same rows the first-run sheet shows, drawn
//! from the shared [`PermissionCenter`] — one row per thing the system
//! must allow, with what it is for, where it stands, and the one thing to
//! press: the real prompt, or the System Settings pane where the system
//! will not ask again. **Enable all** asks for them in sequence. The rows
//! re-read their status every second while the section is up, so a grant
//! made in System Settings shows without a restart.

use crate::{
    model::permission_center::Permissions,
    view::{
        component::permissions_sheet::{mic_test_row, permission_row},
        settings::{self, SettingsPane},
    },
};
use bezel::{
    gpui::{AnyElement, Context, div, prelude::*, px},
    motion::Painter,
    theme::{TextStyle, Theme, Typeset},
    ui::widgets::{ButtonStyle, Buttons, Scaffolding},
};

impl SettingsPane {
    pub(super) fn permissions_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        self.keep_rechecking(cx);
        let center = cx.global::<Permissions>().0.clone();
        let project = self
            .workspace
            .read(cx)
            .active_project()
            .filter(|project| !project.is_remote())
            .map(|project| project.path.clone());
        center.update(cx, |center, cx| center.set_project(project, cx));
        let painter = Painter::of(cx);
        let (rows, settled, enabling) = {
            let c = center.read(cx);
            (c.rows.clone(), c.all_settled(), c.enabling_all)
        };
        let mut group = theme.group_box();
        for (n, row) in rows.iter().enumerate() {
            group = group.child(permission_row(n, row, &center, painter, &theme, cx));
        }
        let enable_all = (!settled).then(|| {
            let center = center.clone();
            div().flex().flex_row().justify_end().child(
                theme
                    .button(
                        if enabling {
                            "Enabling…"
                        } else {
                            "Enable all"
                        },
                        ButtonStyle::Prominent,
                        None,
                    )
                    .id("permissions-enable-all")
                    .when(enabling, |el| el.opacity(0.6))
                    .on_click(move |_, _, cx| {
                        center.update(cx, |center, cx| center.enable_all(cx));
                    }),
            )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(settings::GROUP_GAP))
            .child(group)
            .children(enable_all)
            .child(mic_test_row(painter, &theme, cx))
            .child(
                div()
                    .text_style(TextStyle::Subheadline)
                    .text_color(theme.text_faint)
                    .child(if cfg!(target_os = "macos") {
                        "Grants are filed under the bundle life.arbos.desktop. A build run from the source tree asks under its own path and does not keep them."
                    } else {
                        "On Linux the desktop portal asks for the screen on first use; the microphone needs a capture program and a device."
                    }),
            )
            .into_any_element()
    }

    /// While this section shows, the centre re-reads every second; this pane
    /// follows it. Leaving the section stops the microphone test, and so does
    /// leaving the tab — see [`SettingsPane::went_behind`], which is what
    /// clears `rechecking` from outside this loop.
    fn keep_rechecking(&mut self, cx: &mut Context<Self>) {
        if self.rechecking {
            return;
        }
        self.rechecking = true;
        let center = cx.global::<Permissions>().0.clone();
        center.update(cx, |center, cx| center.watch(cx));
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(crate::model::permission_center::POLL)
                    .await;
                let live = this.update(cx, |this, cx| {
                    let on = this.rechecking && this.section == settings::Section::Permissions;
                    if !on {
                        this.stop_rechecking(cx);
                    }
                    on
                });
                if !matches!(live, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }
}
