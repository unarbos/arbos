//! My pane: profile + every app setting (mockup g). The root page is the
//! shared profile pane (D93, `profiles::pane`) with Profile | Settings — the
//! same tabs as the iPhone's Settings sheet; a Settings row pushes its page
//! into the same pane with `‹` back.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    canvas, div, prelude::*, px, Bounds, Context, Div, DragMoveEvent, ElementId, IntoElement,
    MouseButton, MouseDownEvent, Pixels, Render, SharedString, Stateful, Window,
};

use crate::app::ProfilePage;
use crate::model::{now_ms, MachineState};
use crate::sync::user_error::UserError;
use crate::ui::devices;
use crate::ui::error_note;
use crate::ui::glass;
use crate::ui::motion::PulseClock;
use crate::ui::people;
use crate::ui::profiles::pane::{self, Hero, Picture, ProfilePane, Section};
use crate::ui::profiles::{self, kit, machine};
use crate::ui::shell::{fmt_uptime, human_bytes, SettingsPage, Shell};
use crate::ui::sidebar;
use crate::ui::theme::{
    AppearancePref, Surface, Theme, GLASS_OPACITY_MAX, GLASS_OPACITY_MIN, TEXT_BODY, TEXT_SMALL,
    TEXT_TINY,
};

/// Drag payload for the Appearance glass-opacity slider (no preview view).
#[derive(Clone)]
struct GlassOpacityDrag;

struct GlassOpacityDragGhost;

impl Render for GlassOpacityDragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Profile | Settings — the iPhone's `SettingsView` has the same two.
pub(super) const TABS: [&str; 2] = ["Profile", "Settings"];

/// The open page of my pane.
pub(super) fn page(shell: &Shell, cx: &mut gpui::Context<Shell>) -> Stateful<Div> {
    shell.render_me_pane(cx)
}

impl Shell {
    // ----- my pane: profile + settings ----------------------------------------------

    /// My profile and every app setting in the right pane (mockup g): picture,
    /// name, Profile / Settings tabs; a Settings row pushes its page into the
    /// same pane with `‹` back. This replaced the Settings window (2026-10-20).
    pub(crate) fn render_me_pane(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = Theme::of(cx).clone();
        let state = self.state.read(cx);
        let source = state.source_name;
        let link = state.link;
        let device = state.device_id.clone();
        let show_details = state.show_details;
        let account = state.account.clone();
        let account_image = account.as_ref().and_then(|a| state.avatar(&a.avatar_url));
        let profile_tab = state.profile_tab;
        let notify_on = state.notifier.enabled;
        let permission = state.notification_permission();
        let read_receipts = state.read_receipts;
        let link_previews = state.link_previews;
        let secrets_value = if state.secrets.loaded {
            state.secrets.row_value()
        } else {
            String::new()
        };
        let devices_view = state.devices.clone();
        let devices_summary = match state.devices.items.len() {
            0 => String::new(),
            1 => "1 device".to_string(),
            n => format!("{n} devices"),
        };
        let store_usage = state.store_usage;
        let update = state.update.clone();
        let update_failed = matches!(update.phase, crate::update::UpdatePhase::Failed { .. });
        let auto_update = update.auto;
        let update_action: Option<(&'static str, &'static str, bool)> = match &update.phase {
            crate::update::UpdatePhase::Available(_)
            | crate::update::UpdatePhase::Failed {
                available: Some(_), ..
            } => Some(("update-install", "Install and relaunch", true)),
            crate::update::UpdatePhase::Installed { .. } => {
                Some(("update-restart", "Restart now", true))
            }
            crate::update::UpdatePhase::Idle
            | crate::update::UpdatePhase::UpToDate
            | crate::update::UpdatePhase::Failed {
                available: None, ..
            } => Some(("update-check", "Check now", false)),
            crate::update::UpdatePhase::Checking
            | crate::update::UpdatePhase::Downloading { .. }
            | crate::update::UpdatePhase::Installing(_)
            | crate::update::UpdatePhase::Restarting(_) => None,
        };
        let machine = state.machine.clone();
        let shown = machine.shown_state();
        // Settings › Machine: the box report and the breakdown's pictures, built while
        // `state` is borrowed (the pulse clock below needs `cx`).
        let box_report = state.box_report.clone();
        let machine_pictures: Vec<(crate::model::Id, gpui::AnyElement)> = box_report
            .as_ref()
            .map(|r| {
                r.usage_rows()
                    .into_iter()
                    .map(|ws| {
                        let el = match state.project(&ws.project_id) {
                            Some(p) => crate::ui::avatars::project_in(state, p, 28.0, &theme),
                            None => crate::ui::avatars::neutral_element(&ws.name, 28.0, &theme),
                        }
                        .into_any_element();
                        (ws.project_id.clone(), el)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if shown == MachineState::Waking {
            PulseClock::request(cx);
        }
        let glass_ok = Theme::glass_available();

        let value = |text: String, theme: &Theme| {
            div()
                .flex_shrink_0()
                .text_size(px(TEXT_BODY))
                .text_color(theme.text_muted)
                .child(text)
        };
        let link_button_text =
            |id: &'static str, label: SharedString, danger: bool, theme: &Theme| {
                div()
                    .id(id)
                    .flex_shrink_0()
                    .text_size(px(TEXT_BODY))
                    .text_color(if danger { theme.danger } else { theme.accent })
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.8))
                    .child(label)
            };
        let link_button = |id: &'static str, label: &'static str, danger: bool, theme: &Theme| {
            link_button_text(id, label.into(), danger, theme)
        };
        let footnote = |text: String, theme: &Theme| {
            kit::footnote(text, theme)
                .px(px(kit::INSET))
                .mt(px(6.0))
                .mb(px(0.0))
        };
        let settings_group = |theme: &Theme| kit::card(theme).mx(px(0.0)).mb(px(0.0));
        let seg_row = |label: &str, segs: Div, theme: &Theme| {
            glass::row(label.to_string(), theme).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .p(px(2.0))
                    .rounded_full()
                    .bg(theme.field)
                    .border_1()
                    .border_color(theme.field_border)
                    .child(segs),
            )
        };
        let page_row = |id: &'static str,
                        glyph: &'static str,
                        page: SettingsPage,
                        detail: Option<String>,
                        theme: &Theme,
                        cx: &mut gpui::Context<Self>| {
            kit::nav_row(id, glyph, page.title(), detail, theme).on_click(
                cx.listener(move |this, _, window, cx| this.open_settings_page(page, window, cx)),
            )
        };
        let surface_label = match theme.surface {
            Surface::Matte => "Matte",
            Surface::Glass => "Glass",
        };
        let appearance_summary = format!("{} · {surface_label}", theme.pref.label());
        let on_this_mac = if store_usage.1 == 0 {
            "Nothing saved yet".to_string()
        } else {
            format!(
                "{} · {} chat{}",
                human_bytes(store_usage.0),
                store_usage.1,
                if store_usage.1 == 1 { "" } else { "s" }
            )
        };
        let machine_footer = if machine.always_on {
            "Your one cloud computer, always on. Agents are workspaces on it."
        } else {
            "Your one cloud computer. A message to any agent wakes it."
        };

        // ----- Credits page body -------------------------------------------------
        let credits_body = {
            let mut body = div().flex().flex_col().gap(px(22.0));
            if let Some(account) = account.as_ref() {
                let mut group = settings_group(&theme).child(
                    glass::row("Credits", &theme)
                        .child(value(format!("${:.2} left", account.balance_usd), &theme)),
                );
                group = group.child(glass::row("Today", &theme).child(value(
                    match account.daily_cap_usd {
                        Some(cap) => {
                            format!("${:.2} of ${cap:.0} cap", account.spent_today_usd)
                        }
                        None => format!("${:.2}", account.spent_today_usd),
                    },
                    &theme,
                )));
                body = body.child(group).child(footnote(
                    "Credits pay for your agents' model use. Machine time is billed separately. During the beta, usage credit is included — request a top-up when you run low.".into(),
                    &theme,
                ));
            } else {
                body = body.child(settings_group(&theme).child(
                    glass::row("Balance", &theme).child(value("Not available yet".into(), &theme)),
                ));
            }
            body =
                body.child(settings_group(&theme).child(glass::row("", &theme).child(
                    link_button("credits-top-up", "Request a top-up", false, &theme).on_click(
                        cx.listener(|this, _, window, cx| this.request_top_up(window, cx)),
                    ),
                )));
            body
        };

        // ----- Machine page body -------------------------------------------------
        let machine_body = {
            // Settings › Machine ([feedback machine-page-mac-2026-10-26]): the plan is pure
            // (`machine::plan`); every row below is one of its entries.
            let report = box_report;
            let mut machine_pictures = machine_pictures;
            let breakdown_open = self.machine_breakdown_open;
            let plan = machine::plan(&machine, report.as_ref(), breakdown_open);
            let live = report.as_ref().filter(|r| r.reachable);
            let dot = sidebar::machine_dot(shown, &theme);
            let uptime = machine
                .since
                .filter(|_| shown == MachineState::Awake)
                .map(|since| fmt_uptime(now_ms().saturating_sub(since)));
            let two_line = |title: &str, sub: String, theme: &Theme| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .px(px(kit::INSET))
                    .py(px(10.0))
                    .relative()
                    .child(kit::hairline_below(theme))
                    .child(
                        div()
                            .text_size(px(TEXT_BODY))
                            .text_color(theme.text)
                            .child(title.to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_SMALL))
                            .text_color(theme.text_muted)
                            .child(sub),
                    )
            };
            let action_row = |action: Stateful<Div>| {
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .min_h(px(kit::ROW_MIN_H))
                    .px(px(kit::INSET))
                    .py(px(6.0))
                    .child(action)
                    .into_any_element()
            };
            let mut body = div().flex().flex_col().gap(px(22.0));

            // Warning card first, when a limit is near.
            if let (Some(r), false) = (live, plan.warning.is_empty()) {
                if let Some(w) = machine::warning(r) {
                    let mut rows: Vec<gpui::AnyElement> = Vec::new();
                    for row in &plan.warning {
                        match row {
                            machine::Row::Warning => {
                                rows.push(machine::warning_row(&w, &theme).into_any_element())
                            }
                            machine::Row::AskToFreeSpace => {
                                if let Some(ws) = machine::biggest_disk_user(r) {
                                    let pid = ws.project_id.clone();
                                    let label: SharedString =
                                        format!("Ask {} to free space", ws.name).into();
                                    rows.push(action_row(
                                        link_button_text(
                                            "machine-free-space",
                                            label,
                                            false,
                                            &theme,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.ask_agent_to_free_space(
                                                    pid.clone(),
                                                    window,
                                                    cx,
                                                );
                                            }),
                                        ),
                                    ));
                                }
                            }
                            machine::Row::RequestBiggerMachineInWarning => rows.push(action_row(
                                link_button(
                                    "machine-bigger-warning",
                                    "Request a bigger machine",
                                    false,
                                    &theme,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.request_bigger_machine(window, cx);
                                    },
                                )),
                            )),
                            _ => {}
                        }
                    }
                    body = body.child(kit::section(rows, &theme).mx(px(0.0)).mb(px(0.0)));
                }
            }

            // Machine card.
            let mut group = settings_group(&theme);
            for row in &plan.machine {
                group = match row {
                    machine::Row::WhereItRuns => group.child(
                        glass::row("Where it runs", &theme)
                            .child(value(machine.where_it_runs().to_string(), &theme)),
                    ),
                    machine::Row::InLine => {
                        group.child(glass::row("In line", &theme).child(value(
                            format!(
                                "Number {} · we'll let you know",
                                machine.waitlist_position.unwrap_or_default()
                            ),
                            &theme,
                        )))
                    }
                    machine::Row::State => group.child(
                        glass::row("State", &theme).child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(7.0))
                                .child(div().size(px(8.0)).rounded_full().bg(Theme::with_alpha(
                                    dot,
                                    if shown == MachineState::Waking {
                                        0.4 + 0.6 * PulseClock::phase(cx, 1100)
                                    } else {
                                        1.0
                                    },
                                )))
                                .child(value(
                                    match &uptime {
                                        Some(up) => format!("{} · {up}", shown.label()),
                                        None => shown.label().to_string(),
                                    },
                                    &theme,
                                )),
                        ),
                    ),
                    machine::Row::Spec => group.child(two_line(
                        "Spec",
                        format!(
                            "{}{}",
                            machine
                                .spec
                                .clone()
                                .unwrap_or_else(|| "spec unknown".into()),
                            machine
                                .class
                                .as_deref()
                                .map(|c| format!(" · {c}"))
                                .unwrap_or_default()
                        ),
                        &theme,
                    )),
                    machine::Row::MachineTime => group.child(
                        glass::row(
                            if machine.always_on {
                                "Machine time today"
                            } else {
                                "Spend today"
                            },
                            &theme,
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_end()
                                .child(value(
                                    if machine.always_on {
                                        format!("${:.2}", machine.spend_today_usd)
                                    } else {
                                        format!(
                                            "${:.2} of ${:.0}",
                                            machine.spend_today_usd, machine.spend_cap_usd
                                        )
                                    },
                                    &theme,
                                ))
                                .when(machine.always_on, |el| {
                                    el.child(
                                        div()
                                            .text_size(px(TEXT_TINY))
                                            .text_color(theme.text_muted)
                                            .child("Not taken from credits"),
                                    )
                                }),
                        ),
                    ),
                    _ => group,
                };
            }
            if let Some(detail) = machine.detail.clone() {
                group = group.child(footnote(detail, &theme).pb(px(8.0)));
            }
            body = body.child(group);

            // Resources card (only what the box reports).
            if !plan.resources.is_empty() {
                let mut card = settings_group(&theme);
                for row in &plan.resources {
                    card = match (row, live) {
                        (machine::Row::Disk, Some(r)) => match machine::disk_gauge(r) {
                            Some(g) => card.child(machine::gauge_row("Disk", &g, &theme)),
                            None => card,
                        },
                        (machine::Row::Memory, Some(r)) => match machine::mem_gauge(r) {
                            Some(g) => card.child(machine::gauge_row("Memory", &g, &theme)),
                            None => card,
                        },
                        (machine::Row::Cpu, Some(r)) => match machine::cpu_gauge(r) {
                            Some(g) => card.child(machine::gauge_row("CPU", &g, &theme)),
                            None => card,
                        },
                        (machine::Row::Network, Some(r)) => match machine::net_value(r) {
                            Some((v, today)) => card.child(
                                glass::row("Network", &theme).child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .items_end()
                                        .child(value(v, &theme))
                                        .when_some(today, |el, t| {
                                            el.child(
                                                div()
                                                    .text_size(px(TEXT_TINY))
                                                    .text_color(theme.text_muted)
                                                    .child(t),
                                            )
                                        }),
                                ),
                            ),
                            None => card,
                        },
                        (machine::Row::NotAnswering, _) => card.child(two_line(
                            "The machine did not answer",
                            report
                                .as_ref()
                                .and_then(|r| r.unreachable_reason.clone())
                                .unwrap_or_else(|| "Trying again in a few seconds".into()),
                            &theme,
                        )),
                        _ => card,
                    };
                }
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .child(kit::section_title("Resources", &theme))
                        .child(card),
                );
            }

            // Agents card: running now, jobs, the breakdown.
            if let (Some(r), false) = (live, plan.agents.is_empty()) {
                let mut card = settings_group(&theme);
                let usage = r.usage_rows();
                let mut agent_ix = 0usize;
                for row in &plan.agents {
                    card = match row {
                        machine::Row::RunningNow => card.child(
                            glass::row("Running now", &theme)
                                .child(value(machine::running_now(r), &theme)),
                        ),
                        machine::Row::Jobs => {
                            card.child(glass::row("Background jobs", &theme).child(value(
                                machine::jobs_value(r.jobs_running().unwrap_or(0)),
                                &theme,
                            )))
                        }
                        machine::Row::Breakdown => card.child(
                            glass::hot_row("machine-breakdown", "Per-agent usage", &theme)
                                .child(crate::ui::icons::icon(
                                    if breakdown_open {
                                        "chevron-up"
                                    } else {
                                        "chevron-right"
                                    },
                                    13.0,
                                    theme.text_faint,
                                ))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_machine_breakdown(cx);
                                })),
                        ),
                        machine::Row::Agent => {
                            let Some(ws) = usage.get(agent_ix) else {
                                continue;
                            };
                            agent_ix += 1;
                            let picture = machine_pictures
                                .iter()
                                .position(|(id, _)| *id == ws.project_id)
                                .map(|ix| machine_pictures.remove(ix).1)
                                .unwrap_or_else(|| {
                                    crate::ui::avatars::neutral_element(&ws.name, 28.0, &theme)
                                        .into_any_element()
                                });
                            card.child(machine::agent_row(picture, ws, &theme, cx))
                        }
                        _ => card,
                    };
                }
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .child(kit::section_title("Agents", &theme))
                        .child(card),
                );
            }

            // Actions: plain rows through kit::section (separators only between rows).
            let mut actions: Vec<gpui::AnyElement> = Vec::new();
            for row in &plan.actions {
                match row {
                    machine::Row::RequestBiggerMachine => actions.push(action_row(
                        link_button("machine-bigger", "Request a bigger machine", false, &theme)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.request_bigger_machine(window, cx);
                            })),
                    )),
                    machine::Row::Restart => actions.push(action_row(
                        link_button("machine-restart", "Restart machine", false, &theme).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |s, cx| s.machine_restart(cx));
                            }),
                        ),
                    )),
                    machine::Row::Start => actions.push(action_row(
                        link_button("machine-start", "Start machine", false, &theme).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |s, cx| s.machine_start(cx));
                            }),
                        ),
                    )),
                    machine::Row::Stop => actions.push(action_row(
                        link_button("machine-stop", "Stop machine", true, &theme).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |s, cx| s.machine_stop(cx));
                            }),
                        ),
                    )),
                    _ => {}
                }
            }
            body = body
                .child(kit::section(actions, &theme).mx(px(0.0)).mb(px(0.0)))
                .child(footnote(
                    if machine.always_on {
                        "Your one cloud computer, always on. Agents are workspaces on it. Machine time \
                         is billed separately from your credits."
                            .into()
                    } else {
                        "Your one cloud computer. Agents are workspaces on it; a message to any agent \
                         wakes it."
                            .into()
                    },
                    &theme,
                ));
            body
        };

        // ----- Appearance page body ----------------------------------------------
        let appearance_body = {
            let mut group = settings_group(&theme)
                .child(seg_row(
                    "Theme",
                    div().flex().gap(px(2.0)).children(
                        [
                            (AppearancePref::System, "mode-system"),
                            (AppearancePref::Light, "mode-light"),
                            (AppearancePref::Dark, "mode-dark"),
                        ]
                        .into_iter()
                        .map(|(pref, id)| {
                            glass::segment(id, pref.label(), theme.pref == pref, &theme).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.pick_appearance(pref, cx);
                                }),
                            )
                        }),
                    ),
                    &theme,
                ))
                .child(seg_row(
                    "Surface",
                    div()
                        .flex()
                        .gap(px(2.0))
                        .child(
                            glass::segment(
                                "surface-matte",
                                "Matte",
                                theme.surface == Surface::Matte,
                                &theme,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pick_surface(Surface::Matte, cx);
                            })),
                        )
                        .child(
                            glass::segment(
                                "surface-glass",
                                "Glass",
                                theme.surface == Surface::Glass,
                                &theme,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pick_surface(Surface::Glass, cx);
                            })),
                        ),
                    &theme,
                ));
            // Opacity slider only while Glass is selected (the tint has no
            // effect on Matte).
            if theme.surface == Surface::Glass {
                let opacity = theme
                    .glass_opacity
                    .clamp(GLASS_OPACITY_MIN, GLASS_OPACITY_MAX);
                let frac = ((opacity - GLASS_OPACITY_MIN)
                    / (GLASS_OPACITY_MAX - GLASS_OPACITY_MIN))
                    .clamp(0.0, 1.0);
                let pct = format!("{}%", (opacity * 100.0).round() as i32);
                let track_bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
                let track_for_down = track_bounds.clone();
                let track_for_paint = track_bounds.clone();
                let slider = div()
                    .id("glass-opacity-slider")
                    .flex_1()
                    .h(px(28.0))
                    .relative()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .w_full()
                            .h(px(4.0))
                            .rounded_full()
                            .bg(theme.border)
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(theme.accent)
                                    .w(gpui::relative(frac.max(0.02))),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .child(div().w(gpui::relative(frac)))
                            .child(
                                div()
                                    .ml(px(-8.0))
                                    .size(px(16.0))
                                    .rounded_full()
                                    .bg(theme.primary)
                                    .border_1()
                                    .border_color(theme.field_border)
                                    .shadow_md(),
                            ),
                    )
                    .child(
                        canvas(
                            move |bounds, _, _| track_for_paint.set(bounds),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.set_glass_opacity_at(ev.position.x, track_for_down.get(), cx);
                        }),
                    )
                    .on_drag(GlassOpacityDrag, |_, _, _, cx| {
                        cx.new(|_| GlassOpacityDragGhost)
                    })
                    .on_drag_move(cx.listener(
                        |this, ev: &DragMoveEvent<GlassOpacityDrag>, _, cx| {
                            this.set_glass_opacity_at(ev.event.position.x, ev.bounds, cx);
                        },
                    ));
                group = group.child(
                    glass::row("Opacity", &theme).child(slider).child(
                        div()
                            .flex_shrink_0()
                            .w(px(44.0))
                            .text_size(px(TEXT_BODY))
                            .text_color(theme.text_muted)
                            .child(pct),
                    ),
                );
            }
            // Link previews ([feedback link-embeds-mac-2026-10-27]): the cards under links in
            // chat, on or off on this Mac; off leaves every link plain.
            let group = group.child(seg_row(
                "Link previews",
                div()
                    .flex()
                    .gap(px(2.0))
                    .child(
                        glass::segment("link-previews-on", "On", link_previews, &theme).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |s, cx| s.set_link_previews(true, cx));
                            }),
                        ),
                    )
                    .child(
                        glass::segment("link-previews-off", "Off", !link_previews, &theme)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state
                                    .update(cx, |s, cx| s.set_link_previews(false, cx));
                            })),
                    ),
                &theme,
            ));
            div()
                .flex()
                .flex_col()
                .gap(px(22.0))
                .child(group)
                .child(footnote(
                    if theme.surface == Surface::Glass {
                        if glass_ok {
                            "Glass: window vibrancy and frosted panels. Opacity sets how strong the tint is over the blur."
                                .into()
                        } else {
                            "Glass here: frosted panels only; window vibrancy needs macOS. Opacity sets the tint strength."
                                .into()
                        }
                    } else if glass_ok {
                        "Glass: window vibrancy and frosted panels.".into()
                    } else {
                        "Glass here: frosted panels only; window vibrancy needs macOS.".into()
                    },
                    &theme,
                ))
        };

        // ----- Notifications page body -------------------------------------------
        let notifications_body = {
            // macOS's own answer first: a denial here is why "no banners" — and the
            // one place to fix it is System Settings, so the row opens it.
            let system_row = match permission {
                crate::ui::notify::Permission::Denied => Some(
                    glass::row("Allowed by macOS".to_string(), &theme)
                        .child(value("Off".to_string(), &theme))
                        .child(
                            link_button(
                                "notify-system-settings",
                                "Open System Settings",
                                false,
                                &theme,
                            )
                            .ml(px(10.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |s, _| s.open_notification_settings());
                            })),
                        ),
                ),
                crate::ui::notify::Permission::NotDetermined => Some(
                    glass::row("Allowed by macOS".to_string(), &theme)
                        .child(value("Not asked yet".to_string(), &theme))
                        .child(
                            link_button("notify-system-allow", "Allow", false, &theme)
                                .ml(px(10.0))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |s, cx| s.allow_notifications(cx));
                                })),
                        ),
                ),
                crate::ui::notify::Permission::Allowed | crate::ui::notify::Permission::Unknown => {
                    None
                }
            };
            let mut group = settings_group(&theme);
            if let Some(row) = system_row {
                group = group.child(row);
            }
            let group = group
                .child(seg_row(
                    "Notifications",
                    div()
                        .flex()
                        .gap(px(2.0))
                        .child(
                            glass::segment("notify-on", "On", notify_on, &theme).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.state
                                        .update(cx, |s, cx| s.set_notifications_enabled(true, cx));
                                }),
                            ),
                        )
                        .child(
                            glass::segment("notify-off", "Off", !notify_on, &theme).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |s, cx| {
                                        s.set_notifications_enabled(false, cx);
                                    });
                                }),
                            ),
                        ),
                    &theme,
                ))
                .child(seg_row(
                    "Read receipts",
                    div()
                        .flex()
                        .gap(px(2.0))
                        .child(
                            glass::segment("receipts-on", "On", read_receipts, &theme).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |s, cx| s.set_read_receipts(true, cx));
                                }),
                            ),
                        )
                        .child(
                            glass::segment("receipts-off", "Off", !read_receipts, &theme).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.state
                                        .update(cx, |s, cx| s.set_read_receipts(false, cx));
                                }),
                            ),
                        ),
                    &theme,
                ));
            div()
                .flex()
                .flex_col()
                .gap(px(22.0))
                .child(group)
                .child(footnote(
                "A banner with the sender's picture when an agent finishes or a person writes, \
                     only while you are not looking. Read receipts tell others when you have \
                     opened their messages — turn them off to stop sending that."
                    .into(),
                &theme,
            ))
        };

        // ----- Updates page body -------------------------------------------------
        let updates_body = {
            // A failed install is the page's note (under the pane header, before the card):
            // the shared element with Try again (install again) and Details (the log's first
            // line); the row underneath reads the plain version line.
            let note = match &update.phase {
                crate::update::UpdatePhase::Failed { message, .. } => {
                    let error = UserError::from_text("install the update", message.clone());
                    Some(error_note::render(
                        "update-error",
                        &error,
                        self.update_error_details_open,
                        error_note::Style::Inline,
                        &theme,
                        error_note::Handlers::new()
                            .try_again(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |s, cx| s.install_update(cx));
                            }))
                            .toggle_details(cx.listener(|this, _, _, cx| {
                                this.update_error_details_open = !this.update_error_details_open;
                                cx.notify();
                            })),
                    ))
                }
                _ => None,
            };
            let mut actions = div().flex().items_center().gap(px(14.0));
            if let Some((id, label, installs)) = update_action {
                actions = actions.child(link_button(id, label, false, &theme).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |s, cx| {
                            if installs {
                                s.install_update(cx);
                            } else {
                                s.check_for_updates(cx);
                            }
                        });
                    }),
                ));
            }
            if update_failed {
                actions = actions.child(
                    link_button("settings-update-download", "Download", false, &theme).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |s, cx| s.open_download_page(cx));
                        }),
                    ),
                );
            }
            let group = settings_group(&theme)
                .child(seg_row(
                    "Automatic updates",
                    div()
                        .flex()
                        .gap(px(2.0))
                        .child(
                            glass::segment("auto-update-on", "On", auto_update, &theme).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |s, cx| s.set_auto_update(true, cx));
                                }),
                            ),
                        )
                        .child(
                            glass::segment("auto-update-off", "Off", !auto_update, &theme)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |s, cx| s.set_auto_update(false, cx));
                                })),
                        ),
                    &theme,
                ))
                .child({
                    // A failure row carries a sentence and two actions; side by side in a 280 pt
                    // pane the sentence got ~40 pt and wrapped one word per line. Stack it.
                    let status = div()
                        .min_w_0()
                        .text_size(px(TEXT_BODY))
                        .text_color(theme.text)
                        .child(if update_failed {
                            format!("Arbos {}", update.current_version)
                        } else {
                            update.status_line()
                        });
                    let row = div().min_h(px(44.0)).px(px(16.0)).py(px(10.0));
                    if update_failed {
                        row.flex()
                            .flex_col()
                            .items_start()
                            .gap(px(8.0))
                            .child(status.w_full())
                            .child(actions)
                    } else {
                        row.flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(status.flex_1())
                            .child(actions)
                    }
                });
            div()
                .flex()
                .flex_col()
                .gap(px(22.0))
                .children(note)
                .child(group)
                .child(footnote(
                    if auto_update {
                        "New versions install in the background and Arbos restarts when you pause \
                         in the app — never while another app is focused, during a call, or while \
                         you type. The open chat comes back."
                            .into()
                    } else {
                        "Arbos only tells you about new versions; the blue update button at the bottom of the chat list installs one when you press it."
                            .into()
                    },
                    &theme,
                ))
        };

        // ----- About page body ---------------------------------------------------
        let about_body = {
            let group = settings_group(&theme)
                .child(seg_row(
                    "Turn details",
                    div()
                        .flex()
                        .gap(px(2.0))
                        .child(
                            glass::segment("details-off", "Off", !show_details, &theme).on_click(
                                cx.listener(|this, _, _, cx| {
                                    if this.state.read(cx).show_details {
                                        this.state.update(cx, |s, cx| s.toggle_show_details(cx));
                                    }
                                }),
                            ),
                        )
                        .child(
                            glass::segment("details-on", "On", show_details, &theme).on_click(
                                cx.listener(|this, _, _, cx| {
                                    if !this.state.read(cx).show_details {
                                        this.state.update(cx, |s, cx| s.toggle_show_details(cx));
                                    }
                                }),
                            ),
                        ),
                    &theme,
                ))
                .child(
                    glass::row("Version", &theme)
                        .child(value(update.current_version.to_string(), &theme)),
                )
                .child(glass::row("On this Mac", &theme).child(value(on_this_mac.clone(), &theme)))
                .child(glass::row("Relay", &theme).child(value(
                    format!(
                        "{source} · {}{}",
                        format!("{link:?}").to_lowercase(),
                        device.map(|d| format!(" · {d}")).unwrap_or_default()
                    ),
                    &theme,
                )));
            div()
                .flex()
                .flex_col()
                .gap(px(22.0))
                .child(group)
                .child(footnote(
                    "Turn details adds a \"Show details\" line under each reply with the model, \
                     tools and tokens."
                        .into(),
                    &theme,
                ))
        };

        // ----- Devices page body (Oct 23) -----------------------------------------
        // One card, a row per signed-in device; this Mac first and marked; every
        // other row has Sign out, which asks first and then revokes on the relay.
        let devices_body = {
            let view = devices_view;
            let armed = self.device_revoke_armed.clone();
            let mut body = div().flex().flex_col().gap(px(22.0));
            if let Some(err) = view.error.clone().filter(|_| view.items.is_empty()) {
                // The page's note: under the pane header, before the first card.
                body = body.child(error_note::render(
                    "devices-error",
                    &err,
                    self.devices_error_details_open,
                    error_note::Style::Inline,
                    &theme,
                    error_note::Handlers::new()
                        .try_again(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |s, cx| s.devices_refresh(cx));
                        }))
                        .toggle_details(cx.listener(|this, _, _, cx| {
                            this.devices_error_details_open = !this.devices_error_details_open;
                            cx.notify();
                        })),
                ));
            } else if !view.loaded && view.items.is_empty() {
                body = body.child(kit::empty("Loading…", &theme));
            } else if view.items.is_empty() {
                body = body.child(kit::empty("No devices yet", &theme));
            } else {
                let mut rows: Vec<gpui::AnyElement> = Vec::new();
                for r in devices::rows(&view.items, now_ms() as i64) {
                    if armed.as_deref() == Some(r.id.as_str()) {
                        let id = r.id.clone();
                        rows.push(
                            kit::confirm_row(
                                &devices::confirm_question(&r.name),
                                "Sign out",
                                kit::confirm_button(
                                    "device-revoke-cancel",
                                    "Cancel",
                                    false,
                                    &theme,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.device_revoke_armed = None;
                                        cx.notify();
                                    },
                                )),
                                kit::confirm_button("device-revoke-go", "Sign out", true, &theme)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.device_revoke_armed = None;
                                        let id = id.clone();
                                        this.state.update(cx, |s, cx| s.revoke_device(id, cx));
                                    })),
                                &theme,
                            )
                            .into_any_element(),
                        );
                        continue;
                    }
                    let title: SharedString = r.name.clone().into();
                    // The kind tag ("read-only") and the control share the trailing
                    // slot, in flow — never over the name (seen on the gate build).
                    let mut trailing = div().flex().flex_shrink_0().items_center().gap(px(8.0));
                    if let Some(tag) = r.tag {
                        trailing = trailing.child(
                            div()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded_full()
                                .bg(theme.surface_2)
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.text_muted)
                                .whitespace_nowrap()
                                .child(tag),
                        );
                    }
                    trailing = if !r.revokable() {
                        trailing.child(
                            div()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.text_muted)
                                .whitespace_nowrap()
                                .child("This Mac"),
                        )
                    } else {
                        let id = r.id.clone();
                        trailing.child(
                            link_button("device-revoke", "Sign out", true, &theme)
                                .id(ElementId::Name(SharedString::from(format!(
                                    "device-revoke-{}",
                                    r.id
                                ))))
                                .whitespace_nowrap()
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.device_revoke_armed = Some(id.clone());
                                    cx.notify();
                                })),
                        )
                    };
                    let row = kit::row(
                        ElementId::Name(SharedString::from(format!("device-{}", r.id))),
                        kit::Leading::Tile(r.glyph),
                        title,
                        Some(r.subtitle.clone().into()),
                        None,
                        Some(trailing.into_any_element()),
                        false,
                        &theme,
                    );
                    rows.push(row.into_any_element());
                }
                body = body.child(kit::section(rows, &theme).mx(px(0.0)).mb(px(0.0)));
                body = body.child(footnote(
                    "Every GitHub sign-in is a device. Signing one out ends its session at once; it can sign in again. This Mac signs out from the Settings tab."
                        .into(),
                    &theme,
                ));
            }
            body
        };

        let page = self.settings_page;
        let tab = profile_tab;
        let pane_width = profiles::SHEET_WIDTH.max(page.sheet_width());

        // ----- a pushed page: `‹ Title` in the header, the page fills the pane ----
        if page != SettingsPage::Root {
            let back = glass::control_disc(
                "settings-back",
                "chevron-left",
                crate::ui::theme::CONTROL_GLYPH,
                &theme,
            )
            .on_click(cx.listener(|this, _, _, cx| this.settings_back(cx)));
            let body: Div = if page.is_content() {
                let content = match page {
                    SettingsPage::Credits => credits_body,
                    SettingsPage::Machine => machine_body,
                    SettingsPage::Appearance => appearance_body,
                    SettingsPage::Notifications => notifications_body,
                    SettingsPage::Updates => updates_body,
                    SettingsPage::About => about_body,
                    SettingsPage::Devices => devices_body,
                    SettingsPage::Root
                    | SettingsPage::Connectors
                    | SettingsPage::Routines
                    | SettingsPage::Channels
                    | SettingsPage::Skills
                    | SettingsPage::ThisMac
                    | SettingsPage::Secrets => unreachable!("content pages only"),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(22.0))
                    .px(px(16.0))
                    .pb(px(20.0))
                    .child(content)
            } else {
                let panel: gpui::AnyElement = match page {
                    SettingsPage::Root
                    | SettingsPage::Credits
                    | SettingsPage::Machine
                    | SettingsPage::Appearance
                    | SettingsPage::Notifications
                    | SettingsPage::Updates
                    | SettingsPage::About
                    | SettingsPage::Devices => unreachable!("handled above"),
                    SettingsPage::Connectors => self.connectors.clone().into_any_element(),
                    SettingsPage::Routines => self.routines.clone().into_any_element(),
                    SettingsPage::Channels => self.channels.clone().into_any_element(),
                    SettingsPage::Skills => self.skills.clone().into_any_element(),
                    SettingsPage::ThisMac => self.local.clone().into_any_element(),
                    SettingsPage::Secrets => self.secrets.clone().into_any_element(),
                };
                div()
                    .flex()
                    .flex_col()
                    .px(px(6.0))
                    .pb(px(12.0))
                    .child(panel)
            };
            return profiles::pane_frame(
                pane_width,
                Some((page.title(), back)),
                None,
                body,
                None,
                None,
                Some(&self.settings_scroll),
                &theme,
                cx,
            );
        }

        // ----- hero: my picture (GitHub) and my name (inline field) ---------------
        let who = account
            .as_ref()
            .filter(|a| !a.login.is_empty())
            .map(|a| format!("@{}", a.login))
            .unwrap_or_else(|| "Signed in".to_string());
        let display_name = account
            .as_ref()
            .map(|a| a.name.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| who.clone());
        let person = account.as_ref().map(|account| crate::model::Person {
            id: account.id.clone(),
            handle: account.login.clone(),
            name: Some(account.name.clone()),
            avatar_url: account.avatar_url.clone(),
            status: String::new(),
            role: String::new(),
            last_seen_at: None,
            online: false,
            avatar_color_index: None,
        });
        let picture = match person.as_ref() {
            Some(p) => people::avatar(account_image, p, profiles::HERO_AVATAR, &theme),
            None => div()
                .size(px(profiles::HERO_AVATAR))
                .rounded_full()
                .bg(theme.surface_2),
        }
        .into_any_element();
        // The crayon: the agents' avatar picker (Generate | Upload, Reset) for my picture; it
        // replaces the GitHub one everywhere and on every device.
        let pic_click = account.is_some().then(|| {
            let f = cx.listener(|this, _: &gpui::ClickEvent, _, cx| this.pick_my_avatar(cx));
            Box::new(f) as profiles::HeroClick
        });
        let avatar = profiles::editable_picture("me-avatar", picture, pic_click).into_any_element();
        let name_click = account.is_some().then(|| {
            let f = cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                this.start_profile_edit(window, cx);
            });
            Box::new(f) as profiles::HeroClick
        });
        let name_el = profiles::editable_name(
            "me-name",
            &display_name,
            self.profile_edit_open,
            &self.name_input,
            name_click,
            &theme,
        )
        .into_any_element();
        // Editing: the handle under the name, Cancel / Save (Return saves);
        // pinned with the hero (D93 `Hero.editor`).
        let editor = self.profile_edit_open.then(|| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .pt(px(4.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(px(TEXT_BODY))
                                .text_color(theme.text_muted)
                                .child("@"),
                        )
                        .child(
                            div()
                                .w(px(200.0))
                                .px(px(10.0))
                                .py(px(6.0))
                                .rounded(px(10.0))
                                .bg(theme.field)
                                .border_1()
                                .border_color(theme.field_border)
                                .text_size(px(TEXT_BODY))
                                .child(self.handle_input.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(
                            glass::quiet_button("profile-edit-cancel", "Cancel", &theme)
                                .h(px(30.0))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.profile_edit_open = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            glass::primary_button("profile-edit-save", "Save", &theme)
                                .h(px(30.0))
                                .on_click(cx.listener(|this, _, _, cx| this.save_profile(cx))),
                        ),
                )
                .into_any_element()
        });
        let hero = Hero {
            picture: Picture::Element(avatar),
            name: name_el,
            badge: None,
            subtitle: Some(who.clone().into()),
            subtitle_color: theme.text_muted,
            subtitle_click: None,
            subtitle_element: None,
            editor,
            action: None,
        };

        let mut body: Vec<Section> = Vec::new();
        let mut footer: Vec<Section> = Vec::new();
        if tab == 0 {
            // ----- Profile: who I am on the relay -------------------------------------
            let rows: Vec<gpui::AnyElement> = vec![
                kit::value_row("me-name-row", "Name", display_name.clone(), &theme)
                    .cursor_pointer()
                    .group(kit::HOT)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.start_profile_edit(window, cx)),
                    )
                    .into_any_element(),
                kit::value_row("me-handle-row", "Handle", who.clone(), &theme)
                    .cursor_pointer()
                    .group(kit::HOT)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.start_profile_edit(window, cx)),
                    )
                    .into_any_element(),
            ];
            body.push(Section::Card(rows));
            body.push(Section::Footnote(
                "Your name, handle and picture are what other people and their agents see, here and on the iPhone. Signed in with GitHub.".into(),
            ));
        } else {
            // ----- Settings: D80 cards, mockup g order, every Mac setting kept --------
            // Appearance · Notifications.
            body.push(Section::Card(vec![
                page_row(
                    "settings-appearance",
                    "circle-half",
                    SettingsPage::Appearance,
                    Some(appearance_summary),
                    &theme,
                    cx,
                )
                .into_any_element(),
                page_row(
                    "settings-notifications",
                    "bell",
                    SettingsPage::Notifications,
                    Some(
                        if permission == crate::ui::notify::Permission::Denied {
                            "Off in macOS"
                        } else if notify_on {
                            "On"
                        } else {
                            "Off"
                        }
                        .into(),
                    ),
                    &theme,
                    cx,
                )
                .into_any_element(),
            ]));
            // Secrets · Devices (Credits · Connected apps hidden until implemented).
            body.push(Section::Card(vec![
                page_row(
                    "settings-secrets",
                    "key",
                    SettingsPage::Secrets,
                    Some(secrets_value),
                    &theme,
                    cx,
                )
                .into_any_element(),
                page_row(
                    "settings-devices",
                    "laptop",
                    SettingsPage::Devices,
                    Some(devices_summary),
                    &theme,
                    cx,
                )
                .into_any_element(),
            ]));
            // On this Mac (Routines · Channels · Skills hidden until implemented).
            body.push(Section::Card(vec![kit::nav_row(
                "settings-local",
                "window",
                "On this Mac",
                Some(on_this_mac),
                &theme,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_settings_page(SettingsPage::ThisMac, window, cx)
            }))
            .into_any_element()]));
            // Machine · Updates.
            body.push(Section::Card(vec![
                page_row(
                    "settings-machine",
                    "desktop",
                    SettingsPage::Machine,
                    Some(shown.label().to_string()),
                    &theme,
                    cx,
                )
                .into_any_element(),
                page_row(
                    "settings-updates",
                    "arrow-down",
                    SettingsPage::Updates,
                    Some(if auto_update { "On" } else { "Off" }.into()),
                    &theme,
                    cx,
                )
                .into_any_element(),
            ]));
            body.push(Section::Footnote(machine_footer.into()));
            // Send feedback · About.
            body.push(Section::Card(vec![
                kit::nav_row(
                    "settings-feedback",
                    "square-pencil",
                    "Send feedback",
                    Some("⌘⌥F".into()),
                    &theme,
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_feedback(window, cx)))
                .into_any_element(),
                page_row(
                    "settings-about",
                    "info",
                    SettingsPage::About,
                    Some(update.current_version.to_string()),
                    &theme,
                    cx,
                )
                .into_any_element(),
            ]));
            if account.is_some() {
                footer.push(Section::Card(vec![kit::danger_row(
                    "sign-out", "Sign out", &theme,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close_settings(cx);
                    this.state.update(cx, |s, cx| s.sign_out(cx));
                }))
                .into_any_element()]));
            }
        }
        pane::render(
            ProfilePane {
                page: ProfilePage::Me,
                hero,
                tabs: &TABS,
                selected: tab,
                body,
                footer,
                done: None,
                share: None,
            },
            self,
            cx,
        )
    }
}
