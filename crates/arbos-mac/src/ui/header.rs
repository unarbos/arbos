//! The conversation header (unified top bar, Jacob 2026-10-20, mockups
//! `topbar-unify/`): one centred pill for every chat — avatar + name, the
//! same control for agents, people and groups. Click the name → that chat's
//! right pane. An agent's pill also carries a "+" that starts a new chat with
//! that agent. When the agent has more than one tab, a second hit region
//! "· Tab ⌄" folds them in: its dropdown lists the tabs (current checked,
//! last activity, × on hover closes), the previous chats, then New tab ⌘T.
//! Nothing else lives in the bar: no @handle line, no agent chips, no
//! add-person button, no member heads or counts. Calls start from the
//! composer; adding people or agents lives in the right pane's Members.

use gpui::{
    anchored, deferred, div, prelude::*, px, AnyElement, Context, Div, Edges, ElementId,
    FontWeight, SharedString, Stateful,
};

use crate::app::ProfilePage;
use crate::model::{Chat, LinkState, Project, ServerChannel, ServerInfo};
use crate::ui::avatars;
use crate::ui::blocked;
use crate::ui::composer::Composer;
use crate::ui::glass::{self, WindowDrag};
use crate::ui::groups;
use crate::ui::icons::icon;
use crate::ui::identity;
use crate::ui::motion::PulseClock;
use crate::ui::people;
use crate::ui::profile_target;
use crate::ui::shell::Shell;
use crate::ui::sidebar::{self, machine_dot};
use crate::ui::theme::{
    Theme, AVATAR_HEADER, CONTROL, CONTROL_GLYPH_WIDE, TEXT_BODY, TEXT_SMALL, TEXT_TINY,
};

/// The pinned chat reads "Main" until it is renamed away from the agent's name.
pub(crate) fn tab_title(chat: &Chat, space: &Project) -> String {
    if chat.is_pinned && (chat.title.is_empty() || chat.title == space.name || chat.title == "Main")
    {
        "Main".to_string()
    } else {
        chat.title.clone()
    }
}

pub fn render(
    shell: &Shell,
    space: &Project,
    sidebar_hidden: bool,
    cx: &mut Context<Shell>,
) -> Div {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let is_people = space.is_people();

    // ----- the pill: avatar + name (click → the right pane) --------------------
    // The pill wears a blue ring while its right pane is open (mockups b, f;
    // parity review 2026-10-20).
    let pane_open = match state.profile.as_ref() {
        // The DM's person, whichever target opened them (a chip's page has no space).
        Some(ProfilePage::User { handle, .. }) => space
            .person()
            .filter(|_| is_people && !space.is_people_group())
            .is_some_and(|p| p.handle.eq_ignore_ascii_case(handle)),
        Some(ProfilePage::Group { space_id }) => space_id == &space.id,
        Some(ProfilePage::Agent { project_id }) => project_id == &space.id,
        Some(ProfilePage::Me)
        | Some(ProfilePage::Server { .. })
        | Some(ProfilePage::Channel { .. })
        | None => false,
    };
    // Agents with multi-chat keep a trailing "+" inside the pill, so the right
    // pad is tighter than the people/group pill's. One gate with ⌘T and the
    // dropdown's New tab (`AppState::can_new_tab`): people rooms get none.
    let show_new_chat = state.can_new_tab();
    let mut pill = pill_frame(&theme, pane_open, if show_new_chat { 4.0 } else { 10.0 })
        // The header is a window_drag region: a mouse-down that bubbles to it
        // arms a window move, and the slightest pointer jitter before the
        // mouse-up turns the click into a drag that never completes — the
        // second click of a double-click most of all (#1068). Stop it here and
        // do what the root would have on the way.
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.close_popovers(cx);
                if this.renaming_tab.is_some() {
                    this.commit_tab_rename(window, cx);
                }
            }),
        );

    // Grok Mac's title: 13 pt semibold (ref 57) — the topbar-unify mock's size too.
    let name_label = |text: String, theme: &Theme| {
        div()
            .text_size(px(TEXT_SMALL))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.text)
            .truncate()
            .child(text)
    };
    // The name part: its own hit region so the tab segment can sit beside it.
    let name_part = if let Some(person) = space
        .person()
        .filter(|_| is_people && !space.is_people_group())
    {
        let image = state.avatar(&person.avatar_url);
        let page = ProfilePage::User {
            handle: person.handle.clone(),
            space_id: Some(space.id.clone()),
        };
        // Blocked ([feedback blocked-state-mac-2026-10-26]): the tag after the name.
        let label: AnyElement = if state.is_blocked(person) {
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .min_w_0()
                .child(name_label(person.display_name(), &theme))
                .child(blocked::tag(&theme))
                .into_any_element()
        } else {
            name_label(person.display_name(), &theme).into_any_element()
        };
        // The DM pill is a profile target too (`ui::profile_target`): the
        // person's pane opens, a second click — here or on their name in a
        // bubble — closes it.
        let part = profile_target::mark(
            name_part(
                people::avatar(image, person, AVATAR_HEADER, &theme),
                label,
                &theme,
            ),
            &person.display_name(),
            state.profile_shows(&page),
        );
        part.on_click(cx.listener(move |this, _, _, cx| {
            this.members_open = false;
            this.add_agent_open = false;
            let page = page.clone();
            this.state.update(cx, |s, cx| s.toggle_profile(page, cx));
        }))
    } else if space.is_people_group() {
        let sid = space.id.clone();
        name_part(
            groups::picture(space, state, AVATAR_HEADER, &theme),
            name_label(space.name.clone(), &theme).into_any_element(),
            &theme,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.members_open = false;
            this.add_agent_open = false;
            let sid = sid.clone();
            this.state.update(cx, |s, cx| {
                s.toggle_profile(ProfilePage::Group { space_id: sid }, cx)
            });
        }))
    } else {
        let avatar_state = state.avatar_state(&space.id);
        let machine = state.machine.shown_state();
        let pid = space.id.clone();
        let can_rename = !space.is_guest();
        let renaming = state
            .renaming_project
            .as_ref()
            .is_some_and(|p| p == &space.id);
        let name_el = if renaming {
            div()
                .id("header-rename")
                .w(px(160.0))
                .h(px(AVATAR_HEADER))
                .px(px(6.0))
                .rounded(px(6.0))
                .bg(theme.field)
                .border_1()
                .border_color(theme.field_border)
                .text_size(px(TEXT_SMALL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(shell.agent_name_input.clone())
                .into_any_element()
        } else {
            name_label(space.name.clone(), &theme).into_any_element()
        };
        // A project with other people: the group pane; my own agent: its
        // profile. Double-click the name to rename (owned agents only) — so
        // a single click opens the page only after the double-click window
        // (#1068).
        let groupish = space.is_groupish();
        // The agent pill is a profile target (`ui::profile_target`): its
        // click goes through `open_from_name` → `AppState::toggle_profile`
        // after the double-click window, so it opens, closes and swaps like
        // the agent's name in a bubble; the label and hand are the shared ones.
        let page = if groupish {
            ProfilePage::Group {
                space_id: space.id.clone(),
            }
        } else {
            ProfilePage::Agent {
                project_id: space.id.clone(),
            }
        };
        let showing = state.profile_shows(&page);
        // D92 amendment: the picture built before the clock takes `cx`.
        let picture_disc = groups::picture(space, state, AVATAR_HEADER, &theme);
        if avatar_state != avatars::AvatarState::Idle {
            PulseClock::request(cx);
        }
        // The machine dot hangs off the disc's corner, inside the slot's box.
        let picture = div()
            .relative()
            .size(px(AVATAR_HEADER))
            .flex_shrink_0()
            .child(picture_disc)
            .child(
                div()
                    .absolute()
                    .right(px(-2.0))
                    .bottom(px(-2.0))
                    .size(px(10.0))
                    .rounded_full()
                    .bg(theme.bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(6.0))
                            .rounded_full()
                            .bg(machine_dot(machine, &theme)),
                    ),
            );
        let part = profile_target::mark(name_part(picture, name_el, &theme), &space.name, showing);
        part.on_click(
            cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                this.members_open = false;
                this.add_agent_open = false;
                let p = pid.clone();
                if !can_rename {
                    this.open_from_name(p, groupish, cx);
                } else if event.click_count() >= 2 {
                    this.start_rename_agent(p, window, cx);
                } else {
                    this.name_clicked(p, groupish, cx);
                }
            }),
        )
    };
    pill = pill.child(name_part);

    // ----- tabs fold into the pill: "· Tab ⌄" (when several) and "+" -----------
    if !is_people {
        if let Some(segment) = tab_segment(shell, space, cx) {
            pill = pill.child(segment);
        }
        if show_new_chat {
            pill = pill.child(new_chat_plus(cx));
        }
    }

    // ----- the bar: one frame for every chat kind ---------------------------------
    let sidebar_toggle = sidebar_hidden.then(|| sidebar_toggle(shell, &theme, cx));
    bar_frame(sidebar_toggle, with_link_pill(pill, shell, cx))
}

/// The pill's frame, the same for every chat kind — agent, DM, group, server
/// channel, job tab: `PILL_HEIGHT` tall, round, frosted glass on Glass and
/// Matte alike (`glass::header_surface`; [feedback
/// mac-header-pill-glass-matte-2026-10-26]), a 2 pt accent ring while its
/// right pane is open (mockups b, f; parity review 2026-10-20). `pr` is the
/// right pad (tighter when a "+" follows the name).
pub(crate) fn pill_frame(theme: &Theme, pane_open: bool, pr: f32) -> Stateful<Div> {
    let pill = div()
        .id("header-title")
        .flex()
        .items_center()
        .h(px(metrics::PILL_HEIGHT))
        .pl(px(4.0))
        .pr(px(pr))
        .rounded_full()
        .border_2()
        .border_color(if pane_open {
            theme.accent
        } else {
            gpui::transparent_black()
        })
        .min_w_0();
    glass::header_surface(pill, theme)
}

/// The pill plus, hanging under it, the connection pill ([feedback
/// connection-pill], Jacob Oct 22): one small centred capsule — a status dot
/// and two words — for whatever is wrong with this chat's link (offline >
/// relay down > reconnecting > syncing > restart soon), nothing when it is
/// fine. It replaced the full-width "Reconnecting…" strip and the composer's
/// "messages queue locally" line. It is an absolute child of the pill slot,
/// so it takes no room in `bar_frame`: the bar keeps `metrics::HEIGHT`, the pill stays at
/// `metrics::PILL_TOP`, the drag catchment and the fade band are unchanged,
/// and it floats over the list like the bar itself.
fn with_link_pill(pill: Stateful<Div>, shell: &Shell, cx: &mut Context<Shell>) -> AnyElement {
    // The list blurs through the pill on both surfaces ([feedback
    // mac-header-glass-2026-10-25], [feedback mac-header-pill-glass-matte-2026-10-26]).
    let pill = glass::header_frost(metrics::PILL_HEIGHT / 2.0, pill);
    let Some(status) = link_pill(shell, cx) else {
        return pill;
    };
    div()
        .relative()
        .child(pill)
        .child(
            div()
                .absolute()
                .top(px(metrics::PILL_HEIGHT + metrics::LINK_PILL_GAP))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(status),
        )
        .into_any_element()
}

/// The connection pill itself, or `None` while the link is fine. A plain div
/// (no id): nothing to hit, so clicks and drags go to whatever is under it.
fn link_pill(shell: &Shell, cx: &mut Context<Shell>) -> Option<Div> {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let chat_id = state.active_chat_id();
    let link = state.link_pill(chat_id.as_deref())?;
    let label = link.label()?;
    let dot = match link {
        LinkState::Offline => theme.text_faint,
        LinkState::RelayDown => theme.danger,
        LinkState::Reconnecting | LinkState::RestartSoon => theme.warn,
        LinkState::Syncing => theme.accent,
        LinkState::Live => return None,
    };
    let pulsing = matches!(
        link,
        LinkState::Reconnecting | LinkState::Syncing | LinkState::RestartSoon
    );
    if pulsing {
        PulseClock::request(cx);
    }
    let phase = if pulsing {
        PulseClock::phase(cx, 1400)
    } else {
        1.0
    };
    Some(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(6.0))
            .h(px(metrics::LINK_PILL_HEIGHT))
            .pl(px(9.0))
            .pr(px(11.0))
            .rounded_full()
            .bg(theme.bubble_assistant)
            .child(
                div()
                    .size(px(6.0))
                    .rounded_full()
                    .bg(Theme::with_alpha(dot, 0.55 + 0.45 * phase)),
            )
            .child(
                div()
                    .text_size(px(TEXT_TINY))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .whitespace_nowrap()
                    .child(label),
            ),
    )
}

/// The expand control while the sidebar is hidden: the same round disc as the
/// sidebar strip's collapse toggle, beside the traffic lights (72 + 12 pt)
/// ([feedback mac-collapsed-toggle-inline], [feedback sidebar-strip-filter-2026-10-24]).
fn sidebar_toggle(shell: &Shell, theme: &Theme, cx: &mut Context<Shell>) -> AnyElement {
    let _ = shell.state.read(cx).sidebar_overlay;
    let disc = glass::control_disc("header-sidebar", "sidebar-left", CONTROL_GLYPH_WIDE, theme);
    // The header's material ([feedback mac-header-glass-2026-10-25]): frosted
    // glass on both surfaces, like the pill beside it.
    let disc = glass::header_surface(disc, theme)
        // The bar is a window-drag region; a control is not part of it.
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.close_popovers(cx);
            }),
        )
        .on_click(cx.listener(|this, _, window, cx| {
            let width: f32 = window.viewport_size().width.into();
            this.state.update(cx, |s, cx| {
                let narrow = s.window_is_narrow(width);
                s.toggle_rail(narrow, cx)
            });
        }));
    glass::header_frost(CONTROL / 2.0, disc)
}

/// Geometry every chat header shares — one number each, read by the tests.
pub mod metrics {
    use crate::ui::theme::{CONTROL, HEADER_HEIGHT};
    /// The bar: the top band shared with the sidebar strip (Grok Bot's 60 —
    /// the family's discs centred at 30; [feedback topbar-grok-parity-2026-10-24],
    /// was 44 with a 2 pt bottom pad to sit on the lights' line).
    pub const HEIGHT: f32 = HEADER_HEIGHT;
    /// The bar's content box is the whole bar: the pill is centred on
    /// `CONTROL_LINE`, not on the traffic lights' line any more.
    pub const BOTTOM_PAD: f32 = 0.0;
    /// The pill is as tall as the sidebar's collapse / search discs — one
    /// button family at Grok Bot's size (Jacob 2026-10-24, [feedback
    /// search-palette-2026-10-24]; was 34 with its top 7 pt under the window
    /// edge). Centred in the bar — Jacob's rule (#1130, #1336;
    /// `docs/mac-design-rules.md`), restored by [feedback
    /// header-pill-restore-2026-10-24] after #1493 moved it top-left.
    pub const PILL_HEIGHT: f32 = CONTROL;
    /// The pill shares the sidebar toggle's line: both are centred in the
    /// bar's content box (12 pt from the top at 60 / 0 / 36 — Grok's disc top).
    pub const PILL_TOP: f32 = (HEIGHT - BOTTOM_PAD - PILL_HEIGHT) / 2.0;
    /// Top margin inside the bar that lands the pill at `PILL_TOP`. The bar
    /// packs from the top (`items_start`): a centred cross-axis would drop the
    /// pill mid-pane if the floating overlay ever stretched taller than
    /// `HEIGHT` ([feedback local-relay-unarbos-20261024065516774-20261024065609]).
    pub const PILL_MT: f32 = PILL_TOP;
    /// The list fades out under the floating bar over this band (Grok: text
    /// gone at the edge, full ~50 pt down).
    pub const FADE_BAND: f32 = HEADER_HEIGHT + 6.0;
    /// The window-drag catchment continues this far below the bar.
    pub const DRAG_OVERHANG: f32 = 24.0;
    /// The connection pill under the name pill ([feedback connection-pill]):
    /// its height and the gap to the pill above. It floats as an absolute
    /// child of the pill slot and takes no room in the bar.
    pub const LINK_PILL_HEIGHT: f32 = 22.0;
    pub const LINK_PILL_GAP: f32 = 3.0;
}

/// The frame of every chat header — agent, DM, group, server channel: the top
/// band (`metrics::HEIGHT`), equal side slots (the sidebar toggle left when the
/// sidebar is hidden), the pill centred on the family's line, the whole bar a
/// window-drag region (the pill and the toggle stop the drag themselves). The
/// list runs under it and fades (`metrics::FADE_BAND`), so there is no hairline.
pub(crate) fn bar_frame(left: Option<AnyElement>, pill: AnyElement) -> Div {
    let side = px(CONTROL + 8.0);
    // Content box under the bottom pad — side slots centre their control in it.
    let content_h = px(metrics::HEIGHT - metrics::BOTTOM_PAD);
    div()
        .h(px(metrics::HEIGHT))
        .max_h(px(metrics::HEIGHT))
        .flex_shrink_0()
        .w_full()
        .flex()
        // Top-pack, not cross-axis centre: if this bar's box is ever taller
        // than `HEIGHT` (a stretched absolute overlay over a long transcript),
        // centre would park the pill mid-screen.
        .items_start()
        .justify_between()
        .gap(px(10.0))
        .pl(px(12.0))
        .pr(px(10.0))
        .pb(px(metrics::BOTTOM_PAD))
        .overflow_hidden()
        .window_drag()
        .debug_selector(|| "chat-header-bar".into())
        // The left slot is always there (empty when the sidebar is open) so the
        // two side slots match and the pill sits on the bar's centre line
        // ([feedback app-01M5Q1CQZ5JBNY33WCKPN0Y8G9], [feedback
        // header-pill-restore-2026-10-24]).
        .child(
            div()
                .w(side)
                .h(content_h)
                .flex_shrink_0()
                .flex()
                .items_center()
                .children(left),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .justify_center()
                .mt(px(metrics::PILL_MT))
                .child(
                    div()
                        .flex_shrink_0()
                        .max_w(px(460.0))
                        .debug_selector(|| "chat-header-pill".into())
                        .child(pill),
                ),
        )
        .child(div().w(side).h(content_h).flex_shrink_0())
}

/// The same bar for a channel (D88 rule 3): the pill carries the server's disc, the `#`
/// glyph and the channel's name; click → the channel's page in the right pane (#1317 —
/// the server's page is the server header in the sidebar).
pub fn render_channel(
    shell: &Shell,
    server: &ServerInfo,
    channel: &ServerChannel,
    sidebar_hidden: bool,
    cx: &mut Context<Shell>,
) -> Div {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let page = ProfilePage::Channel {
        server_id: server.id.clone(),
        channel_id: channel.id.clone(),
    };
    let pane_open = crate::ui::profiles::pane::is_open(state, &page);
    let overlay = state.sidebar_overlay;
    let locked = channel.restricted;

    let title = div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .min_w_0()
        .child(icon("hash", 14.0, theme.text_muted))
        .child(
            div()
                .text_size(px(TEXT_BODY))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .truncate()
                .child(channel.name.clone()),
        )
        .when(locked, |el| el.child(icon("lock", 11.0, theme.text_faint)));
    let name_part = name_part(
        sidebar::server_picture(server, state, AVATAR_HEADER, &theme),
        title.into_any_element(),
        &theme,
    )
    .on_click(cx.listener(move |this, _, _, cx| {
        this.members_open = false;
        this.add_agent_open = false;
        let page = page.clone();
        this.state.update(cx, |s, cx| s.toggle_profile(page, cx));
    }));
    let pill = pill_frame(&theme, pane_open, 10.0)
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.close_popovers(cx);
            }),
        )
        .child(name_part);

    let sidebar_toggle = sidebar_hidden.then(|| sidebar_toggle(shell, &theme, cx));
    let _ = overlay;
    bar_frame(sidebar_toggle, with_link_pill(pill, shell, cx))
}

/// "+" inside the agent pill: starts a new chat with this agent (same as ⌘T).
fn new_chat_plus(cx: &mut Context<Shell>) -> Stateful<Div> {
    let theme = Theme::of(cx).clone();
    div()
        .id("header-new-chat")
        .flex_shrink_0()
        .size(px(26.0))
        .ml(px(2.0))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_2))
        .child(icon("plus", 13.0, theme.text_muted))
        // Mouse-down stops the header's window-drag region; click creates the chat.
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.close_popovers(cx);
                if this.renaming_tab.is_some() {
                    this.commit_tab_rename(window, cx);
                }
            }),
        )
        .on_click(cx.listener(|this, _, window, cx| {
            cx.stop_propagation();
            this.open_new_tab(window, cx);
        }))
}

/// "· Tab ⌄" inside the pill when an agent has more than one tab (a single tab
/// shows the name and "+" only). Click toggles the dropdown, which hangs off it.
/// The pill's picture + name for every chat kind: the one `identity::row`
/// (fixed 22 pt slot, Grok's header gap), its own hit region so the tab
/// segment and "+" can sit beside it.
fn name_part(picture: impl IntoElement, name: AnyElement, theme: &Theme) -> Stateful<Div> {
    identity::row(AVATAR_HEADER, identity::HEADER_GAP, picture, name)
        .id("header-name")
        .h_full()
        .cursor_pointer()
        .rounded_full()
        .hover(|s| s.bg(theme.surface_2))
        .pr(px(4.0))
}

fn tab_segment(shell: &Shell, space: &Project, cx: &mut Context<Shell>) -> Option<Stateful<Div>> {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let tabs = state.visible_tabs(&space.id);
    if tabs.len() < 2 {
        return None;
    }
    let active = state.active_tab.get(&space.id).cloned();
    let current = tabs
        .iter()
        .find(|c| Some(&c.id) == active.as_ref())
        .or(tabs.first())
        .map(|c| tab_title(c, space))
        .unwrap_or_else(|| "Main".into());
    let streaming = active
        .as_deref()
        .is_some_and(|id| state.chat_is_streaming(id));
    if streaming {
        PulseClock::request(cx);
    }
    let phase = PulseClock::phase(cx, 1200);
    let open = shell.tab_menu_open;
    Some(
        div()
            .id("header-tab")
            .relative()
            .flex()
            .items_center()
            .gap(px(5.0))
            .h(px(26.0))
            .ml(px(2.0))
            .pl(px(9.0))
            .pr(px(7.0))
            .rounded_full()
            .bg(if open {
                theme.surface_2
            } else {
                theme.surface_1
            })
            .text_size(px(TEXT_SMALL))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text)
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_2))
            .child(
                div()
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_faint)
                    .child("·"),
            )
            .child(div().max_w(px(160.0)).truncate().child(current))
            .when(streaming, |el| {
                el.child(
                    div()
                        .size(px(6.0))
                        .rounded_full()
                        .bg(Theme::with_alpha(theme.warn, 0.4 + 0.6 * phase)),
                )
            })
            .child(icon("chevron-down", 11.0, theme.text_muted))
            // Mouse-down, not click: the root closes every popover on
            // mouse-down, so a click handler could never toggle this one shut.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.menu_open = false;
                    if this.renaming_tab.is_some() {
                        this.commit_tab_rename(window, cx);
                    }
                    this.tab_menu_open = !this.tab_menu_open;
                    cx.notify();
                }),
            )
            .when(open, |el| {
                // Hangs under the segment: an absolute wrapper places it (taffy
                // positions it relative to the segment), the anchored child
                // keeps it inside the window, deferred paints it over the chat.
                el.child(
                    div().absolute().left(px(-8.0)).top(px(34.0)).child(
                        deferred(
                            anchored()
                                .position_mode(gpui::AnchoredPositionMode::Local)
                                .snap_to_window_with_margin(Edges::all(px(8.0)))
                                .child(glass::popover(tab_dropdown(shell, space, cx))),
                        )
                        .with_priority(1),
                    ),
                )
            }),
    )
}

/// The dropdown (mockup b): every visible tab — current checked, its last
/// activity, × on hover (closes; the pinned Main cannot) — then the previous
/// (archived) chats, a separator and New tab ⌘T.
fn tab_dropdown(shell: &Shell, space: &Project, cx: &mut Context<Shell>) -> impl IntoElement {
    let theme = Theme::of(cx).clone();
    // The session menu (#1256) hangs off its row; built here, placed by the
    // row it belongs to.
    let mut anchored_menu = anchored_chat_menu(shell, cx);
    let state = shell.state.read(cx);
    let multi_chat = state.multi_chat;
    let new_tab = state.can_new_tab();
    let active = state.active_tab.get(&space.id).cloned();
    let tabs: Vec<Chat> = state.visible_tabs(&space.id).into_iter().cloned().collect();
    let mut previous: Vec<Chat> = state
        .archived_tabs(&space.id)
        .into_iter()
        .cloned()
        .collect();
    previous.sort_by_key(|c| std::cmp::Reverse(c.last_message_at));
    let projects = state.projects.clone();
    let renaming = shell.renaming_tab.clone();

    let mut body = div().flex().flex_col().gap(px(1.0));
    let mut list = glass::contained_y_scroll("tab-menu-tabs")
        .gap(px(1.0))
        .max_h(px(360.0));
    for chat in tabs {
        let is_active = Some(&chat.id) == active.as_ref();
        let editing = renaming.as_ref() == Some(&chat.id);
        let closable = multi_chat && !chat.is_pinned && !editing;
        let title = tab_title(&chat, space);
        let id = chat.id.clone();
        let close_id = chat.id.clone();
        let menu_chat_id = chat.id.clone();
        let ctrl_chat_id = chat.id.clone();
        let rename_id = chat.id.clone();
        let rename_label = title.clone();
        let key = chat.id.clone();
        let menu_here = anchored_menu.as_ref().is_some_and(|(id, _)| id == &chat.id);
        let mut row = div()
            .id(ElementId::Name(SharedString::from(format!(
                "tab-menu-{}",
                chat.id
            ))))
            .group(SharedString::from(format!("tab-row-{key}")))
            .relative()
            .flex()
            .items_center()
            .gap(px(10.0))
            .h(px(36.0))
            .pl(px(10.0))
            .pr(px(8.0))
            .rounded(px(9.0))
            .text_size(px(TEXT_SMALL))
            .text_color(theme.text)
            .bg(if is_active || menu_here {
                theme.surface_2
            } else {
                gpui::transparent_black()
            })
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_2))
            .when(menu_here, |el| {
                el.children(anchored_menu.take().map(|(_, menu)| menu))
            })
            .child(
                div()
                    .w(px(14.0))
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .when(is_active, |el| el.child(icon("check", 12.0, theme.text))),
            );
        if editing {
            row = row
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(24.0))
                        .px(px(6.0))
                        .rounded(px(6.0))
                        .bg(theme.field)
                        .border_1()
                        .border_color(theme.field_border)
                        .child(shell.tab_rename_input.clone()),
                );
            list = list.child(row);
            continue;
        }
        if let Some(glyphs) = groups::tab_glyphs(&chat, &projects, &theme) {
            row = row.child(glyphs);
        }
        // A job tab (`jobtabs`): the job's glyph before the title — the spinner while it
        // runs, a check when done — and its state instead of a time at the right.
        let job_doc = crate::jobtabs::job_id(&chat.id)
            .and_then(|jid| state.rollouts.get(jid))
            .cloned();
        let job_running = crate::jobtabs::job_id(&chat.id).is_some()
            && job_doc.as_ref().is_none_or(|r| r.is_running());
        if chat.kind == crate::model::ChatKind::Job {
            let glyph: gpui::AnyElement = if job_running {
                crate::ui::cards::activity_spinner(13.0, &theme, cx).into_any_element()
            } else {
                icon("check", 12.0, theme.online).into_any_element()
            };
            row = row.child(div().flex_shrink_0().size(px(14.0)).child(glyph));
        }
        let trailing = match &job_doc {
            Some(r) => r.status_short(),
            None if chat.kind == crate::model::ChatKind::Job => "Running".to_string(),
            None => crate::ui::sidebar::time_label(chat.last_message_at),
        };
        row = row
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(chat.unread && !is_active, |el| {
                        el.font_weight(FontWeight::SEMIBOLD)
                    })
                    .child(title),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(TEXT_TINY))
                    .text_color(theme.text_faint)
                    .child(trailing),
            );
        if closable {
            row = row.child(
                div()
                    .id(ElementId::Name(SharedString::from(format!(
                        "tab-close-{}",
                        chat.id
                    ))))
                    .size(px(18.0))
                    .flex_shrink_0()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(0.0)
                    .group_hover(SharedString::from(format!("tab-row-{key}")), |s| {
                        s.opacity(1.0)
                    })
                    .hover(|s| s.bg(theme.border_strong))
                    .child(icon("xmark", 9.0, theme.text_muted))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.state.update(cx, |s, cx| {
                            s.close_tab(close_id.clone(), cx);
                        });
                    })),
            );
        } else {
            row = row.child(div().size(px(18.0)).flex_shrink_0());
        }
        row = row
            // Right-click / control-click: the session menu (Rename, Fork,
            // Delete), anchored to this row; the dropdown stays open.
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.open_chat_menu(menu_chat_id.clone(), cx);
                }),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    if event.modifiers.control {
                        this.open_chat_menu(ctrl_chat_id.clone(), cx);
                        return;
                    }
                    if event.click_count >= 2
                        && this.renaming_tab.as_ref() != Some(&rename_id)
                        && !crate::jobtabs::is_job_chat(&rename_id)
                    {
                        this.begin_tab_rename(rename_id.clone(), rename_label.clone(), window, cx);
                    }
                }),
            )
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    if event.modifiers().control
                        || event.click_count() >= 2
                        || this.renaming_tab.as_ref() == Some(&id)
                    {
                        return;
                    }
                    this.tab_menu_open = false;
                    this.chat_menu = None;
                    this.chat_delete_armed = false;
                    this.state.update(cx, |s, cx| s.select_tab(id.clone(), cx));
                    Composer::focus(&this.composer, window, cx);
                }),
            );
        list = list.child(row);
    }
    body = body.child(list);

    if !previous.is_empty() {
        body = body.child(glass::menu_divider(&theme)).child(
            div()
                .px(px(12.0))
                .pt(px(4.0))
                .pb(px(2.0))
                .text_size(px(TEXT_TINY))
                .text_color(theme.text_faint)
                .child("Previous chats"),
        );
        let mut list = glass::contained_y_scroll("tab-menu-previous")
            .gap(px(1.0))
            .max_h(px(240.0));
        for chat in previous {
            let id = chat.id.clone();
            let menu_chat_id = chat.id.clone();
            let ctrl_chat_id = chat.id.clone();
            let menu_here = anchored_menu.as_ref().is_some_and(|(id, _)| id == &chat.id);
            list = list.child(
                div()
                    .id(ElementId::Name(SharedString::from(format!(
                        "tab-menu-previous-{}",
                        chat.id
                    ))))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .h(px(36.0))
                    .pl(px(34.0))
                    .pr(px(12.0))
                    .rounded(px(9.0))
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .when(menu_here, |el| {
                        el.bg(theme.surface_2)
                            .children(anchored_menu.take().map(|(_, menu)| menu))
                    })
                    .hover(|s| s.bg(theme.surface_2))
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.open_chat_menu(menu_chat_id.clone(), cx);
                        }),
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if event.modifiers.control {
                                this.open_chat_menu(ctrl_chat_id.clone(), cx);
                            }
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(chat.title.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(TEXT_TINY))
                            .text_color(theme.text_faint)
                            .child(crate::ui::sidebar::time_label(chat.last_message_at)),
                    )
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            if event.modifiers().control {
                                return;
                            }
                            this.tab_menu_open = false;
                            this.chat_menu = None;
                            this.chat_delete_armed = false;
                            let id = id.clone();
                            this.state.update(cx, |s, cx| s.unarchive_chat(id, cx));
                            Composer::focus(&this.composer, window, cx);
                        }),
                    ),
            );
        }
        body = body.child(list);
    }

    if new_tab {
        body = body.child(glass::menu_divider(&theme)).child(
            glass::menu_row("tab-menu-new", "plus", "New tab", false, &theme)
                .child(
                    div()
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.text_faint)
                        .child("⌘T"),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.tab_menu_open = false;
                    this.open_new_tab(window, cx);
                })),
        );
    }
    glass::frosted_menu(TAB_DROPDOWN_WIDTH, &theme, body)
}

/// The tab dropdown's card width and inner padding (`glass::menu_card`).
const TAB_DROPDOWN_WIDTH: f32 = 280.0;
const TAB_DROPDOWN_PAD: f32 = 6.0;

/// The open session menu (#1256) as an element its row appends: an absolute
/// wrapper at the row's right edge (taffy places it relative to the row), an
/// anchored child that keeps it inside the window, deferred above the
/// dropdown (priority 2 > the dropdown's 1) so it paints over the list and
/// the chat. Returns the chat it belongs to, so the loops know which row.
fn anchored_chat_menu(
    shell: &Shell,
    cx: &mut Context<Shell>,
) -> Option<(crate::model::Id, AnyElement)> {
    let chat_id = shell.chat_menu.clone()?;
    let menu = sidebar::chat_menu(shell, &chat_id, cx)?;
    let el = div()
        .absolute()
        .left(px(TAB_DROPDOWN_WIDTH - 2.0 * TAB_DROPDOWN_PAD - 6.0))
        .top(px(-4.0))
        .child(
            deferred(
                anchored()
                    .position_mode(gpui::AnchoredPositionMode::Local)
                    .snap_to_window_with_margin(Edges::all(px(8.0)))
                    .child(glass::popover(menu)),
            )
            .with_priority(2),
        )
        .into_any_element();
    Some((chat_id, el))
}

/// Nothing under the header any more (2026-10-20): agents of a DM or group
/// sit in the right pane's Members list.
pub fn chips(_shell: &Shell, _space: &Project, _cx: &mut Context<Shell>) -> Option<AnyElement> {
    None
}

#[cfg(test)]
mod header_tests {
    use gpui::{point, px, Modifiers, MouseButton, MouseDownEvent, TestAppContext};

    use super::metrics;
    use super::*;
    use crate::ui::glass::WindowDragPress;

    /// The four chat kinds, drawn through the one `bar_frame` with each kind's
    /// pill content (what differs in the real pages), with and without the
    /// sidebar (the toggle appears in the left slot when it is hidden).
    #[derive(Clone, Copy)]
    enum Kind {
        Agent,
        Dm,
        Group,
        Channel,
    }

    struct Bar {
        kind: Kind,
        folded: bool,
    }

    const WIDTH: f32 = 884.0;

    impl gpui::Render for Bar {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let pill_w = match self.kind {
                Kind::Agent => 150.0,
                Kind::Dm => 110.0,
                Kind::Group => 130.0,
                Kind::Channel => 170.0,
            };
            let pill = div()
                .id("pill")
                .h(px(metrics::PILL_HEIGHT))
                .w(px(pill_w))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .into_any_element();
            let left = self.folded.then(|| {
                div()
                    .id("toggle")
                    .size(px(crate::ui::sidebar::STRIP_BUTTON))
                    .debug_selector(|| "toggle".into())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .into_any_element()
            });
            div().w(px(WIDTH)).h(px(400.0)).child(
                div()
                    .when(self.folded, |el| el.pl(px(72.0)))
                    .child(bar_frame(left, pill)),
            )
        }
    }

    fn moves_after(cx: &mut TestAppContext, kind: Kind, folded: bool, x: f32) -> u32 {
        cx.update(|cx| {
            cx.set_global(WindowDragPress {
                record_only: true,
                ..Default::default()
            })
        });
        let (view, vcx) = cx.add_window_view(|_, _| Bar { kind, folded });
        vcx.run_until_parked();
        let at = point(px(x), px(22.0));
        vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        vcx.simulate_mouse_move(
            point(at.x + px(12.0), at.y + px(3.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        vcx.run_until_parked();
        vcx.update(|_, cx| cx.default_global::<WindowDragPress>().moves)
    }

    fn pill_w(kind: Kind) -> f32 {
        match kind {
            Kind::Agent => 150.0,
            Kind::Dm => 110.0,
            Kind::Group => 130.0,
            Kind::Channel => 170.0,
        }
    }

    #[gpui::test]
    fn every_kind_has_the_same_bar_height_and_pill_position(cx: &mut TestAppContext) {
        for kind in [Kind::Agent, Kind::Dm, Kind::Group, Kind::Channel] {
            for folded in [false, true] {
                let (_view, vcx) = cx.add_window_view(|_, _| Bar { kind, folded });
                vcx.run_until_parked();
                let bar = vcx.debug_bounds("chat-header-bar").expect("bar drawn");
                let pill = vcx.debug_bounds("chat-header-pill").expect("pill drawn");
                let h: f32 = bar.size.height.into();
                let top: f32 = (pill.origin.y - bar.origin.y).into();
                let ph: f32 = pill.size.height.into();
                assert_eq!(h, metrics::HEIGHT, "bar height, folded={folded}");
                assert_eq!(top, metrics::PILL_TOP, "pill top, folded={folded}");
                assert_eq!(ph, metrics::PILL_HEIGHT, "pill height, folded={folded}");
                if folded {
                    // One button family (Jacob 2026-10-24): the pill is exactly
                    // as tall as the sidebar toggle beside it and shares its line.
                    let toggle = vcx.debug_bounds("toggle").expect("toggle drawn");
                    let th: f32 = toggle.size.height.into();
                    let t_top: f32 = (toggle.origin.y - bar.origin.y).into();
                    assert_eq!(ph, th, "pill as tall as the toggle");
                    assert!(
                        (t_top - top).abs() <= 0.5,
                        "pill ({top}) and toggle ({t_top}) share one line"
                    );
                }
                let cx_pill: f32 = (pill.origin.x + pill.size.width / 2.0).into();
                let cx_bar: f32 = (bar.origin.x + bar.size.width / 2.0).into();
                assert!(
                    (cx_pill - cx_bar).abs() <= 1.0,
                    "pill centred, folded={folded}"
                );
            }
        }
    }

    /// Jacob's rule (#1130, #1336; `docs/mac-design-rules.md`): the pill sits
    /// on the bar's centre line — for every chat kind, sidebar open or folded —
    /// at the button family's height (`CONTROL`, picture `CONTROL − 8`;
    /// [feedback search-palette-2026-10-24] sized it, the placement is this
    /// item's). #1493 moved it to the left
    /// edge on a tester's "personal preference"; [feedback
    /// header-pill-restore-2026-10-24] put it back. This pins the geometry so
    /// the next drift fails here.
    #[gpui::test]
    fn the_pill_is_centred_not_parked_at_the_left_edge(cx: &mut TestAppContext) {
        assert_eq!(
            metrics::PILL_HEIGHT,
            CONTROL,
            "pill height: the button family's"
        );
        assert_eq!(
            metrics::PILL_TOP,
            (metrics::HEIGHT - metrics::BOTTOM_PAD - CONTROL) / 2.0,
            "pill top: centred on the toggle's line"
        );
        assert_eq!(AVATAR_HEADER, CONTROL - 8.0, "header picture");
        for kind in [Kind::Agent, Kind::Dm, Kind::Group, Kind::Channel] {
            for folded in [false, true] {
                let (_view, vcx) = cx.add_window_view(|_, _| Bar { kind, folded });
                vcx.run_until_parked();
                let bar = vcx.debug_bounds("chat-header-bar").expect("bar drawn");
                let pill = vcx.debug_bounds("chat-header-pill").expect("pill drawn");
                let left: f32 = (pill.origin.x - bar.origin.x).into();
                let right: f32 =
                    ((bar.origin.x + bar.size.width) - (pill.origin.x + pill.size.width)).into();
                let w: f32 = pill.size.width.into();
                assert_eq!(w, pill_w(kind), "pill keeps its own width, folded={folded}");
                // Equal air either side (the bar pads 12 left / 10 right, so
                // the margins differ by those 2 pt): not hugging the leading
                // pad, not the trailing one.
                assert!(
                    (left - right).abs() <= 2.0,
                    "equal margins, folded={folded}: left={left} right={right}"
                );
                assert!(
                    left > 100.0,
                    "pill is not at the left edge, folded={folded}: left={left}"
                );
            }
        }
    }

    /// Floating header over a tall pane (the long-rows screenshot rig): the
    /// pill stays in the header row at the window top, not mid-screen.
    struct FloatingOverTall;

    impl gpui::Render for FloatingOverTall {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let pill = div()
                .id("pill")
                .h(px(metrics::PILL_HEIGHT))
                .w(px(130.0))
                .into_any_element();
            div()
                .w(px(WIDTH))
                .h(px(800.0))
                .relative()
                .flex()
                .flex_col()
                .child(div().flex_1().min_h_0())
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(metrics::HEIGHT))
                        .max_h(px(metrics::HEIGHT))
                        .overflow_hidden()
                        .child(bar_frame(None, pill)),
                )
        }
    }

    #[gpui::test]
    fn floating_header_pill_stays_in_the_header_row_over_a_tall_pane(cx: &mut TestAppContext) {
        let (_view, vcx) = cx.add_window_view(|_, _| FloatingOverTall);
        vcx.run_until_parked();
        let bar = vcx.debug_bounds("chat-header-bar").expect("bar drawn");
        let pill = vcx.debug_bounds("chat-header-pill").expect("pill drawn");
        let bar_top: f32 = bar.origin.y.into();
        let bar_h: f32 = bar.size.height.into();
        let pill_top: f32 = pill.origin.y.into();
        assert!(
            bar_top.abs() <= 1.0,
            "bar at pane top, not mid-screen: y={bar_top}"
        );
        assert_eq!(
            bar_h,
            metrics::HEIGHT,
            "bar stays {} pt tall",
            metrics::HEIGHT
        );
        assert!(
            (pill_top - (bar_top + metrics::PILL_TOP)).abs() <= 1.0,
            "pill in the header row: y={pill_top} want {}",
            bar_top + metrics::PILL_TOP
        );
    }

    #[gpui::test]
    fn the_empty_band_drags_for_every_kind_and_the_pill_does_not(cx: &mut TestAppContext) {
        for kind in [Kind::Agent, Kind::Dm, Kind::Group, Kind::Channel] {
            // Left of the centred pill, right of the side slot.
            assert_eq!(moves_after(cx, kind, false, 120.0), 1, "band drags");
            assert_eq!(
                moves_after(cx, kind, true, 72.0 + 60.0),
                1,
                "band drags, folded"
            );
            // The pill's centre.
            assert_eq!(
                moves_after(cx, kind, false, WIDTH / 2.0),
                0,
                "pill is a control"
            );
            // The toggle.
            assert_eq!(
                moves_after(cx, kind, true, 72.0 + 12.0 + 14.0),
                0,
                "toggle is a control"
            );
        }
    }

    #[test]
    fn the_fade_band_covers_the_bar_and_the_drag_overhang_follows_it() {
        // Read through a function so a future edit to one constant trips
        // this instead of clippy folding the comparison away.
        let m = [
            metrics::FADE_BAND,
            metrics::HEIGHT,
            metrics::DRAG_OVERHANG,
            metrics::PILL_TOP,
            metrics::PILL_HEIGHT,
        ];
        let [fade, height, overhang, pill_top, pill_h] = std::hint::black_box(m);
        assert!(fade >= height, "the fade must cover the bar");
        assert_eq!(fade, height + 6.0);
        assert_eq!(overhang, 24.0);
        // One button family (Jacob 2026-10-24): the pill is a CONTROL tall,
        // like the strip discs and the sidebar toggle, and sits on their line —
        // centred in the bar's content box (60 − 0 pad − 36) / 2 = 12, Grok's
        // disc top ([feedback topbar-grok-parity-2026-10-24]).
        assert_eq!(pill_h, CONTROL, "pill height is the button family's");
        assert_eq!(pill_h, crate::ui::sidebar::STRIP_BUTTON);
        assert_eq!(
            pill_top,
            (height - metrics::BOTTOM_PAD - pill_h) / 2.0,
            "the pill is centred on the toggle's line"
        );
        assert!(pill_top + pill_h <= height - metrics::BOTTOM_PAD);
        assert_eq!(pill_top, 12.0, "Grok's disc top");
        assert_eq!(
            pill_top + pill_h / 2.0,
            crate::ui::theme::CONTROL_LINE,
            "the pill is centred on the family's line"
        );
        assert_eq!(AVATAR_HEADER, CONTROL - 8.0, "picture 4 pt inside the pill");
    }
}

/// [feedback mac-header-glass-2026-10-25] and [feedback
/// mac-header-pill-glass-matte-2026-10-26]: the header pill and disc are
/// frosted glass for every chat kind on BOTH surfaces — #1830 left Matte
/// solid, and Jacob's Mac is on Matte (his AffineTeam group, v0.6.268) — and
/// the frosted wrap leaves the bar's geometry untouched. The iPhone twin is
/// `HeaderGlassStructureTests`.
#[cfg(test)]
mod glass_tests {
    use gpui::{div, prelude::*, px, TestAppContext};

    use super::{bar_frame, metrics, pill_frame};
    use crate::ui::glass;
    use crate::ui::theme::{Appearance, Surface, Theme, CONTROL};

    /// The chat kinds the header serves, with the pill's right pad for each
    /// (4 after an agent's "+", 10 otherwise). A job tab is an agent tab:
    /// the same `render`, the same pill.
    const KINDS: [(&str, f32); 5] = [
        ("agent", 4.0),
        ("job tab", 4.0),
        ("dm", 10.0),
        ("group", 10.0),
        ("channel", 10.0),
    ];
    const SURFACES: [Surface; 2] = [Surface::Glass, Surface::Matte];
    const APPEARANCES: [Appearance; 2] = [Appearance::Dark, Appearance::Light];

    #[test]
    fn every_kind_gets_a_frosted_pill_and_disc_on_glass_and_on_matte() {
        for appearance in APPEARANCES {
            for surface in SURFACES {
                let theme = Theme::with(appearance, surface);
                let style = glass::header_style(&theme);
                for (kind, _) in KINDS {
                    assert!(
                        style.is_frosted(),
                        "{kind} on {appearance:?} {surface:?}: frosted glass, not a flat fill"
                    );
                    assert!(
                        style.fill.a < 1.0,
                        "{kind} on {appearance:?} {surface:?}: the list shows through"
                    );
                    // Not the solids the pill and the disc were filled with
                    // before #1830 (Glass) and before this item (Matte).
                    assert_ne!(style.fill, theme.bubble_assistant, "{kind}: old pill fill");
                    assert_ne!(style.fill, theme.surface_2, "{kind}: old disc fill");
                    assert!(style.rim.a > 0.0, "{kind}: a visible rim");
                    assert!(style.highlight.a > 0.0, "{kind}: a top highlight");
                }
            }
            // One material on both surfaces: Grok Bot's pill over its flat
            // black transcript is the reference, so the Matte header gets the
            // very fill, rim and highlight the Glass header has.
            let glass = glass::header_style(&Theme::with(appearance, Surface::Glass));
            let matte = glass::header_style(&Theme::with(appearance, Surface::Matte));
            assert_eq!(glass, matte, "{appearance:?}: the same glass on Matte");
        }
    }

    /// [feedback mac-header-pill-see-through-2026-10-26]: the dark pill is
    /// see-through — a thin fill and a light blur, so the transcript shows
    /// through it and it is read as glass by its rim (Grok Bot's pill over a
    /// black page is ≈ +12/255 lighter than the page) — while white text on
    /// it keeps ≥ 4.5:1 over the brightest backdrop the pill ever sits on
    /// (the chat column's 85 % page wash over a white desktop, ≈ `#373737`).
    #[test]
    fn the_dark_pill_is_see_through_and_its_text_still_reads() {
        let theme = Theme::with(Appearance::Dark, Surface::Glass);
        let style = glass::header_style(&theme);
        assert!(
            style.fill.a <= 0.35,
            "a thin fill, the list shows through: {}",
            style.fill.a
        );
        // Through a function so clippy does not fold the comparison away.
        let blur = std::hint::black_box(glass::HEADER_BLUR);
        assert!(blur <= 6.0, "a light blur keeps the text behind legible");
        assert!(style.rim.a >= 0.25, "the rim carries the shape");
        let lum = |c: gpui::Hsla| {
            let rgb = c.to_rgb();
            let ch = |v: f32| {
                if v <= 0.039_28 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(rgb.r) + 0.7152 * ch(rgb.g) + 0.0722 * ch(rgb.b)
        };
        let over = |top: gpui::Hsla, under: gpui::Hsla| {
            let (t, u) = (top.to_rgb(), under.to_rgb());
            gpui::Rgba {
                r: t.a * t.r + (1.0 - t.a) * u.r,
                g: t.a * t.g + (1.0 - t.a) * u.g,
                b: t.a * t.b + (1.0 - t.a) * u.b,
                a: 1.0,
            }
            .into()
        };
        for backdrop in [theme.bg, gpui::rgb(0x373737).into()] {
            let pill = over(style.fill, backdrop);
            let contrast = (lum(theme.text) + 0.05) / (lum(pill) + 0.05);
            assert!(contrast >= 4.5, "white on the pill: {contrast:.2}:1");
        }
        // Over the flat Matte page the pill is a step above it, not a plate.
        let lift = (over(style.fill, theme.bg).to_rgb().r - theme.bg.to_rgb().r) * 255.0;
        assert!(
            (6.0..=24.0).contains(&lift),
            "pill lift over its page: {lift:.1}/255"
        );
    }

    struct GlassBar {
        folded: bool,
        pr: f32,
    }

    impl gpui::Render for GlassBar {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            let pill = pill_frame(&theme, false, self.pr).w(px(140.0));
            let pill = glass::header_frost(metrics::PILL_HEIGHT / 2.0, pill);
            let left = self.folded.then(|| {
                let disc = glass::header_surface(
                    div().id("toggle").size(px(CONTROL)).rounded_full(),
                    &theme,
                );
                glass::header_frost(CONTROL / 2.0, disc)
            });
            div().w(px(884.0)).h(px(400.0)).child(
                div()
                    .when(self.folded, |el| el.pl(px(72.0)))
                    .child(bar_frame(left, pill)),
            )
        }
    }

    /// Headless render on Glass and on Matte, dark and light: the frosted
    /// pill and disc draw for every kind, folded and not, and the bar keeps
    /// `metrics` (sizes and placements are not this item's to move).
    #[gpui::test]
    fn frosted_header_renders_on_both_surfaces_and_keeps_the_bar_geometry(cx: &mut TestAppContext) {
        for appearance in APPEARANCES {
            for surface in SURFACES {
                cx.update(|cx| cx.set_global(Theme::with(appearance, surface)));
                for (kind, pr) in KINDS {
                    for folded in [false, true] {
                        let (_view, vcx) = cx.add_window_view(|_, _| GlassBar { folded, pr });
                        vcx.run_until_parked();
                        let bar = vcx.debug_bounds("chat-header-bar").expect("bar drawn");
                        let pill = vcx.debug_bounds("chat-header-pill").expect("pill drawn");
                        let h: f32 = bar.size.height.into();
                        let top: f32 = (pill.origin.y - bar.origin.y).into();
                        let ph: f32 = pill.size.height.into();
                        let pw: f32 = pill.size.width.into();
                        let at = format!("{kind} on {appearance:?} {surface:?}, folded={folded}");
                        assert_eq!(h, metrics::HEIGHT, "bar height, {at}");
                        assert_eq!(top, metrics::PILL_TOP, "pill top, {at}");
                        assert_eq!(ph, metrics::PILL_HEIGHT, "pill height, {at}");
                        assert_eq!(pw, 140.0, "pill width, {at}");
                        let cx_pill: f32 = (pill.origin.x + pill.size.width / 2.0).into();
                        let cx_bar: f32 = (bar.origin.x + bar.size.width / 2.0).into();
                        assert!((cx_pill - cx_bar).abs() <= 1.0, "pill centred, {at}");
                    }
                }
            }
        }
    }
}
