//! An agent's page on the shared profile pane (D93): Grok Bot's **overview →
//! Edit** hierarchy ([feedback iphone-entity-profile-edit-2026-11-10]) while
//! keeping Mac's side-pane + modal-sheet idiom (not an iOS push). Landing is
//! the overview — large avatar, name, Edit pill, then grouped rows
//! **Coding · Routines · Library · Computer** (Library · Connection for an
//! outside agent). Edit opens a modal sheet (name, label, avatar, model /
//! reasoning / voice). Each overview row drills in with ‹ back
//! (`profile_subpage`). Former Details / Library / Computer **tabs** are gone
//! from the landing (supersedes [feedback agent-pane-details-tab-2026-10-24]);
//! Coding and Routines return as overview rows (supersedes
//! [feedback agent-details-remove-sections-mac-2026-10-25]).
//!
//! Grok Bot parity ([feedback info-pane-chrome-2026-10-24], Jacob 2026-10-24):
//! no "Delete agent" button on the pane — deleting an agent is the sidebar
//! row's menu (`sidebar::row_menu`, Delete, at once); the hero picture is the
//! plain disc, with no presence dot on it (presence stays on
//! the sidebar row); the top bar carries Grok's share disc (the invite sheet:
//! let a person in on this agent) beside the close disc.

use std::time::Instant;

use gpui::{
    div, prelude::*, px, Context, Div, ElementId, Focusable, MouseButton, SharedString, Stateful,
};

use crate::app::{AgentPick, ProfilePage};
use crate::model::{
    duration_s, now_ms, size_mb, BoxReport, BoxWorkspace, Id, MachineState, Project, Routine,
};
use crate::ui::avatars::{self, AvatarState};
use crate::ui::create_entity::{self, hex, palette};
use crate::ui::frost::{frosted, PANEL_BLUR};
use crate::ui::glass;
use crate::ui::highlight;
use crate::ui::hoverpick;
use crate::ui::motion::PulseClock;
use crate::ui::routines::rel_time;
use crate::ui::sf_symbols;
use crate::ui::shell::Shell;
use crate::ui::theme::Theme;

use super::kit::{self, Leading};
use super::pane::{self, Hero, Picture, ProfilePane, Section};
use super::{chats_of, collect, editable_name, hero_picture, HeroClick, HERO_AVATAR};

/// Overview row labels on a native agent — Grok Bot profile
/// ([feedback iphone-entity-profile-edit-2026-11-10]); former Details |
/// Library | Computer **tabs** are gone from the landing. Same order as the
/// iPhone's `PaneKinds.agent`.
pub(super) const TABS: [&str; 4] = ["Coding", "Routines", "Library", "Computer"];
/// Overview rows on a connected (outside) agent — no Computer (#2141; Connection
/// + Key cards live under the Connection drill-in).
pub(crate) const OUTSIDE_TABS: [&str; 2] = ["Library", "Connection"];
/// Coding and Routines are overview rows now (always shown on the landing).
/// Kept so older call sites / tests that pass the flag still compile; the
/// overview ignores it and always lists both rows for a native agent.
pub(crate) const AGENT_DETAILS_SHOW_CODING_ROUTINES: bool = true;
pub(crate) const TAB_CODING: usize = 0;
pub(crate) const TAB_ROUTINES: usize = 1;
pub(crate) const TAB_LIBRARY: usize = 2;
pub(crate) const TAB_COMPUTER: usize = 3;
/// Outside overview: Library · Connection.
pub(crate) const OUTSIDE_TAB_LIBRARY: usize = 0;
pub(crate) const OUTSIDE_TAB_CONNECTION: usize = 1;
/// Grok's subtitle while a bot has no label.
pub(crate) const LABEL_PLACEHOLDER: &str = "Add a label";
/// Grok's Coding-row spinner: 15 pt (rows 613–642 px at 2 px/pt).
pub(crate) const CODING_SPINNER: f32 = 15.0;
/// Hero Edit pill (element id / debug selector).
pub(crate) const EDIT_ID: &str = "profile-edit";

/// An agent that is only known as a member of a people chat (someone else's
/// project) is shown from that member entry.
fn project_from_member(state: &crate::app::AppState, project_id: &str) -> Option<Project> {
    state
        .projects
        .iter()
        .flat_map(|p| p.agents.iter())
        .find(|a| a.project_id == project_id)
        .map(|a| Project {
            id: a.project_id.clone(),
            name: a.name.clone(),
            icon: a.icon.clone(),
            color: a.color.clone(),
            avatar: a.avatar.clone(),
            avatar_color: a.avatar_color.clone(),
            avatar_color_index: a.avatar_color_index,
            avatar_disc_color: a.avatar_disc_color,
            // The owner's picture travels with the roster copy; without it the hero drew the
            // automatic disc for every foreign agent (Jacob, 2026-10-25: "Hezbola").
            image_url: a.image_url.clone(), // avatar-lint: allow (data copy, not a draw)
            avatar_version: a.avatar_version,
            description: a.description.clone(),
            owner_person: a.owner.clone(),
            model: a.model.clone().unwrap_or_default(),
            shared: true,
            owner_handle: Some(a.owner_handle.clone()),
            ..Default::default()
        })
}

pub(super) fn page(
    project_id: &str,
    shell: &Shell,
    cx: &mut Context<Shell>,
) -> Option<Stateful<Div>> {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let (project, mine): (Project, bool) = match state.project(project_id) {
        Some(p) => (p.clone(), !p.is_guest()),
        None => (project_from_member(state, project_id)?, false),
    };
    let avatar_state = state.avatar_state(&project.id);
    let streaming = state.project_is_streaming(&project.id);
    let machine_state: MachineState = match &project.machine_info {
        Some(m) if !mine => match m.state.as_str() {
            "awake" => MachineState::Awake,
            "waking" => MachineState::Waking,
            "error" => MachineState::Error,
            _ => MachineState::Asleep,
        },
        _ => state.machine.shown_state(),
    };
    let always_on = project
        .machine_info
        .as_ref()
        .map(|m| m.always_on)
        .unwrap_or(state.machine.always_on);
    let model_text = if project.model.is_empty() {
        "default".to_string()
    } else {
        project.model.clone()
    };
    let voice_text = crate::ui::voicepicker::current_label(&project, &state.voices);
    let pick = state.agent_pick;
    let outside = project.outside.clone();
    let subpage = state.profile_subpage;
    let pid: Id = project.id.clone();
    // Library drill-in: what the agent's chats hold (read while `state` is borrowed).
    let on_library = match (outside.is_some(), subpage) {
        (true, Some(OUTSIDE_TAB_LIBRARY)) => true,
        (false, Some(TAB_LIBRARY)) => true,
        _ => false,
    };
    let content = on_library.then(|| {
        let chats: Vec<&crate::model::Chat> = chats_of(state, &project.id);
        collect(state, &chats)
    });
    // The owner's display name for the "Run by …" link (API §9 `owner`, else a member row).
    let owner_name: Option<String> = project
        .owner_handle
        .as_deref()
        .map(|o| o.trim_start_matches('@'))
        .and_then(|o| state.person_by_handle(o))
        .and_then(|p| p.name.clone())
        .filter(|n| !n.trim().is_empty());
    let routines: Vec<Routine> = state.routines_of(&project.id).to_vec();
    let box_report = state.box_report.clone();
    let machine = state.machine.clone();
    let now = now_ms() as i64;

    // D92 amendment: the uploaded / generated picture wins over the disc (read while `state`
    // is borrowed) — from the newest copy the store holds, like every other surface.
    let hero_image = avatars::project_image(state, &project);
    let newest_picture_url =
        crate::sync::agent_avatar::newest(&state.projects, &project.id).and_then(|f| f.image_url); // avatar-lint: allow (resolver)
    if avatar_state != AvatarState::Idle || streaming {
        PulseClock::request(cx);
    }

    // ----- hero ----------------------------------------------------------------
    // Overview: plain disc / photo, read-only name, Edit pill for the owner.
    // Subpage drill-in: glyph tile + row title, project name as subtitle.
    // ([feedback info-pane-chrome-2026-10-24], [feedback iphone-entity-profile-edit-2026-11-10]).
    let drilled = subpage.is_some();
    let subpage_title: Option<&'static str> = match (outside.is_some(), subpage) {
        (false, Some(TAB_CODING)) => Some("Coding"),
        (false, Some(TAB_ROUTINES)) => Some("Routines"),
        (false, Some(TAB_LIBRARY)) | (true, Some(OUTSIDE_TAB_LIBRARY)) => Some("Library"),
        (false, Some(TAB_COMPUTER)) => Some("Computer"),
        (true, Some(OUTSIDE_TAB_CONNECTION)) => Some("Connection"),
        _ => None,
    };
    let picture = if let Some(title) = subpage_title {
        let glyph = match title {
            "Library" => "books.vertical",
            "Computer" => "desktopcomputer",
            "Coding" => "chevron.left.forwardslash.chevron.right",
            "Routines" => "clock",
            "Connection" => "link",
            _ => "desktopcomputer",
        };
        Picture::Glyph(glyph)
    } else {
        let picture = div()
            .size(px(HERO_AVATAR))
            .debug_selector(|| "agent-hero-picture".into())
            .child(match hero_image {
                Some(image) => div()
                    .size(px(HERO_AVATAR))
                    .flex_shrink_0()
                    .rounded_full()
                    .overflow_hidden()
                    .child(
                        gpui::img(crate::ui::picture_fit::fit(&image, HERO_AVATAR))
                            .size(px(HERO_AVATAR))
                            .rounded_full()
                            .object_fit(gpui::ObjectFit::Cover),
                    ),
                None => avatars::project_hero(&project, HERO_AVATAR, &theme),
            })
            .into_any_element();
        // Overview: Edit owns the crayon — tapping the picture opens Edit for mine.
        let pic_click = mine.then(|| {
            let f = cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
                this.open_agent_edit(window, cx);
            });
            Box::new(f) as HeroClick
        });
        let view_click = (!mine).then_some(newest_picture_url).flatten().map(|url| {
            let name = project.name.clone();
            let f = cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                this.view_picture(&name, &url, cx);
            });
            Box::new(f) as HeroClick
        });
        Picture::Element(
            hero_picture("profile-avatar", picture, pic_click, view_click).into_any_element(),
        )
    };
    let name_el = if let Some(title) = subpage_title {
        pane::name(title, &theme)
    } else {
        editable_name(
            "profile-name",
            &project.name,
            false,
            &shell.agent_name_input,
            None,
            &theme,
        )
        .into_any_element()
    };
    let owner_handle = project
        .owner_handle
        .as_deref()
        .map(|o| o.trim_start_matches('@').to_owned())
        .filter(|o| !o.is_empty());
    let label = project.description.clone().filter(|d| !d.trim().is_empty());
    // Overview: no subtitle for mine (label lives on Edit); foreign agents keep
    // label or "Run by …". Subpage: the project name under the row title.
    let (subtitle, subtitle_color, subtitle_element, editor, action) = if drilled {
        (
            Some(project.name.clone().into()),
            theme.label_muted(),
            None,
            None,
            None,
        )
    } else {
        let run_by = (!mine)
            .then(|| owner_handle.clone())
            .flatten()
            .map(|handle| {
                run_by_link_element(&handle, owner_name.as_deref(), &pid, shell, &theme, cx)
            });
        let (subtitle_element, run_by_below) = match (run_by, &label) {
            (Some(link), None) => (Some(link), None),
            (Some(link), Some(_)) => (None, Some(link)),
            (None, _) => (None, None),
        };
        let subtitle: Option<SharedString> = if mine {
            None
        } else if let Some(d) = &label {
            Some(d.trim().to_owned().into())
        } else if owner_handle.is_some() {
            None // "Run by …" is subtitle_element
        } else {
            Some("Agent".into())
        };
        let edit_action = mine.then(|| {
            kit::edit_pill(EDIT_ID, &theme)
                .on_click(cx.listener(|this, _, window, cx| this.open_agent_edit(window, cx)))
                .into_any_element()
        });
        (
            subtitle,
            if label.is_some() {
                theme.label_muted()
            } else {
                theme.label_faint()
            },
            subtitle_element,
            run_by_below.map(|link| div().pt(px(2.0)).child(link).into_any_element()),
            edit_action,
        )
    };
    let hero = Hero {
        picture,
        name: name_el,
        badge: None,
        subtitle,
        subtitle_color,
        subtitle_click: None,
        subtitle_element,
        editor,
        action,
    };

    let live = DetailsLive {
        streaming,
        machine_state,
        always_on,
        workspace: box_report
            .as_ref()
            .and_then(|r| r.workspace(&project.id))
            .cloned(),
        routines: routines.clone(),
        now,
    };
    let body: Vec<Section> = match (outside.as_ref(), subpage) {
        // ----- overview landing -------------------------------------------------
        (_, None) => overview_sections(
            &project,
            mine,
            outside.is_some(),
            &live,
            machine_state,
            always_on,
            &box_report,
            &theme,
            cx,
        ),
        // ----- outside drill-ins -----------------------------------------------
        (Some(_), Some(OUTSIDE_TAB_LIBRARY)) => {
            library_sections(content.expect("collected for Library"), shell, &theme, cx)
        }
        (Some(info), _) => {
            crate::ui::connect_agent::details_sections(&pid, info, shell, &theme, cx)
        }
        // ----- native drill-ins ------------------------------------------------
        (None, Some(TAB_LIBRARY)) => {
            library_sections(content.expect("collected for Library"), shell, &theme, cx)
        }
        (None, Some(TAB_COMPUTER)) => computer_sections(
            &project,
            mine,
            machine_state,
            always_on,
            &machine,
            box_report.as_ref(),
            &theme,
            cx,
        ),
        (None, Some(TAB_CODING)) => coding_sections(&project, &live, &theme, cx),
        (None, Some(TAB_ROUTINES)) if mine => routines_sections(&project, &live, &theme, cx),
        (None, Some(TAB_ROUTINES)) => Vec::new(),
        (None, Some(_)) => Vec::new(),
    };
    // Silence unused on overview (settings live on Edit).
    let _ = (model_text, voice_text, pick);

    // No footer: "Delete agent" left the pane on Jacob's ask ([feedback
    // info-pane-chrome-2026-10-24]); the sidebar row's menu deletes at once.
    let footer: Vec<Section> = Vec::new();
    // Grok's share disc beside the ×: the invite sheet — a pick lets that
    // person in on this agent. Only the owner can share; hide on drill-in.
    let share_id = pid.clone();
    let share = (mine && !drilled).then(|| {
        let f = cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
            let id = share_id.clone();
            this.people_input.update(cx, |i, cx| {
                i.clear(cx);
                i.set_placeholder("Search people");
            });
            this.state.update(cx, |s, cx| s.open_invite(id, cx));
            let handle = this.people_input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        });
        Box::new(f) as HeroClick
    });

    Some(pane::render(
        ProfilePane {
            page: ProfilePage::Agent {
                project_id: project.id.clone(),
            },
            hero,
            // Overview / drill-in: no tab pill (rows replace tabs).
            tabs: &[],
            selected: 0,
            body,
            footer,
            done: None,
            share,
            overview_back: drilled,
        },
        shell,
        cx,
    ))
}

/// The "Run by …" chip: pointer states from `Shell.run_by_chip` (14 / 22 /
/// 30 % accent, 120 ms ease-out), a release on it pushes the owner's page
/// (`ProfilePage::User`) over the agent's — `‹` in the top bar brings the
/// agent back with its tab and scroll.
fn run_by_link_element(
    owner_handle: &str,
    owner_name: Option<&str>,
    project_id: &Id,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> gpui::AnyElement {
    let now = Instant::now();
    let (alpha, fading) = shell.run_by_chip.alpha(&(), now, false);
    if fading {
        PulseClock::request(cx);
    }
    let label = super::run_by_label(owner_name, owner_handle);
    let (handle, pid) = (owner_handle.to_owned(), project_id.clone());
    super::run_by_link(&label, alpha, theme)
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            if this
                .run_by_chip
                .hover(hovered.then_some(()), Instant::now())
            {
                cx.notify();
            }
        }))
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.run_by_chip.press((), Instant::now());
                cx.notify();
            }),
        )
        .on_mouse_up(
            gpui::MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                if this.run_by_chip.release(Instant::now()).is_some() {
                    this.push_profile(
                        ProfilePage::User {
                            handle: handle.clone(),
                            space_id: Some(pid.clone()),
                        },
                        cx,
                    );
                }
                cx.notify();
            }),
        )
        .into_any_element()
}

/// Overview landing rows — Coding · Routines · Library · Computer (or Library ·
/// Connection for an outside agent). Each row sets `profile_subpage`.
fn overview_sections(
    project: &Project,
    mine: bool,
    outside: bool,
    live: &DetailsLive,
    machine_state: MachineState,
    always_on: bool,
    box_report: &Option<BoxReport>,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    let mut rows: Vec<gpui::AnyElement> = Vec::new();
    if outside {
        rows.push(overview_row(
            "agent-overview-library",
            "books.vertical",
            "Library",
            None,
            OUTSIDE_TAB_LIBRARY,
            theme,
            cx,
        ));
        let status = project
            .outside
            .as_ref()
            .map(|o| o.status_line(live.now))
            .unwrap_or_default();
        rows.push(overview_row(
            "agent-overview-connection",
            "link",
            "Connection",
            Some(status),
            OUTSIDE_TAB_CONNECTION,
            theme,
            cx,
        ));
    } else {
        let coding = coding_status(
            live.streaming,
            live.workspace.as_ref(),
            machine_state,
            always_on,
        );
        rows.push(overview_row(
            "agent-overview-coding",
            "chevron.left.forwardslash.chevron.right",
            "Coding",
            Some(coding),
            TAB_CODING,
            theme,
            cx,
        ));
        if mine {
            let on_count = live.routines.iter().filter(|r| r.enabled).count();
            let routines_value = (!live.routines.is_empty()).then(|| format!("{on_count} on"));
            rows.push(overview_row(
                "agent-overview-routines",
                "clock",
                "Routines",
                routines_value,
                TAB_ROUTINES,
                theme,
                cx,
            ));
        }
        rows.push(overview_row(
            "agent-overview-library",
            "books.vertical",
            "Library",
            None,
            TAB_LIBRARY,
            theme,
            cx,
        ));
        let computer_status = computer_secondary(project, box_report);
        rows.push(overview_row(
            "agent-overview-computer",
            "desktopcomputer",
            "Computer",
            Some(computer_status),
            TAB_COMPUTER,
            theme,
            cx,
        ));
    }
    vec![Section::Card(rows)]
}

fn computer_secondary(project: &Project, box_report: &Option<BoxReport>) -> String {
    match box_report
        .as_ref()
        .and_then(|r| r.workspace(&project.id))
        .map(|w| w.running)
    {
        Some(true) => "Running".into(),
        Some(false) => "Idle".into(),
        None => "—".into(),
    }
}

fn overview_row(
    id: &'static str,
    glyph: &'static str,
    title: &'static str,
    detail: Option<String>,
    page: usize,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> gpui::AnyElement {
    kit::nav_row(id, glyph, title, detail, theme)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.state
                .update(cx, |s, cx| s.set_profile_subpage(Some(page), cx));
        }))
        .into_any_element()
}

/// Coding drill-in: the live coding card.
fn coding_sections(
    project: &Project,
    live: &DetailsLive,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    let working = live.streaming || live.workspace.as_ref().is_some_and(|w| w.running);
    let leading: gpui::AnyElement = if working {
        PulseClock::request(cx);
        div()
            .size(px(kit::AVATAR))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(crate::ui::cards::activity_spinner(
                CODING_SPINNER,
                theme,
                cx,
            ))
            .debug_selector(|| "agent-coding-spinner".into())
            .into_any_element()
    } else {
        div()
            .size(px(kit::AVATAR))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(sf_symbols::glyph(
                "chevron.left.forwardslash.chevron.right",
                18.0,
                theme.label_muted(),
            ))
            .into_any_element()
    };
    let status = coding_status(
        live.streaming,
        live.workspace.as_ref(),
        live.machine_state,
        live.always_on,
    );
    let row = kit::row(
        "agent-coding-row",
        Leading::Avatar(leading),
        project.name.clone(),
        Some(status.into()),
        None,
        None,
        false,
        theme,
    );
    vec![
        Section::Title("Coding".into()),
        Section::Card(vec![row.into_any_element()]),
    ]
}

/// Routines drill-in: every routine with its switch.
fn routines_sections(
    project: &Project,
    live: &DetailsLive,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    let mut rows: Vec<gpui::AnyElement> = Vec::new();
    for r in &live.routines {
        let (rid, on) = (r.id.clone(), r.enabled);
        let switch = div()
            .id(ElementId::Name(SharedString::from(format!(
                "agent-routine-switch-{}",
                r.id
            ))))
            .cursor_pointer()
            .child(glass::switch(on, theme))
            .on_click(cx.listener(move |this, _, _, cx| {
                let rid = rid.clone();
                this.state
                    .update(cx, |s, cx| s.routine_set_enabled(rid, !on, cx));
            }))
            .into_any_element();
        rows.push(
            kit::row(
                ElementId::Name(SharedString::from(format!("agent-routine-{}", r.id))),
                Leading::None,
                r.name.clone(),
                Some(routine_status(r, live.now).into()),
                None,
                Some(switch),
                false,
                theme,
            )
            .into_any_element(),
        );
    }
    if rows.is_empty() {
        rows.push(
            kit::row(
                "agent-routines-none",
                Leading::None,
                "No routines yet",
                Some(format!("Ask {} to schedule one", project.name).into()),
                None,
                None,
                false,
                theme,
            )
            .text_color(theme.label_muted())
            .into_any_element(),
        );
    }
    vec![Section::Title("Routines".into()), Section::Card(rows)]
}

/// What the Details tab's live cards read.
struct DetailsLive {
    streaming: bool,
    machine_state: MachineState,
    always_on: bool,
    /// This agent's workspace on the box, when the last report listed it.
    workspace: Option<BoxWorkspace>,
    routines: Vec<Routine>,
    now: i64,
}

/// The order of the Details tab, top to bottom — live content first, the
/// options after it (Jacob 2026-10-24). Jacob 2026-10-25 ([feedback
/// agent-details-remove-sections-mac-2026-10-25]) took "Who may message X",
/// "Permissions" and "Profile" off the pane for good (the access list, the
/// standing shell answers and the relay calls behind them are unchanged; the
/// name and the label are edited in the hero) and parked Coding and Routines
/// behind [`AGENT_DETAILS_SHOW_CODING_ROUTINES`]. Pure, so the order is
/// testable without a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DetailsBlock {
    /// "Coding": the agent's workspace, spinning while a turn runs.
    Coding,
    /// "Routines": every routine with its switch (owner only).
    Routines,
    /// "Settings": Model · Reasoning · Voice (Model alone on someone else's agent).
    Settings,
}

pub(crate) fn details_order(mine: bool, show_coding_routines: bool) -> Vec<DetailsBlock> {
    let mut order = Vec::new();
    if show_coding_routines {
        order.push(DetailsBlock::Coding);
        if mine {
            order.push(DetailsBlock::Routines);
        }
    }
    order.push(DetailsBlock::Settings);
    order
}

/// The Coding row's subtitle: what the agent is doing right now.
pub(crate) fn coding_status(
    streaming: bool,
    workspace: Option<&BoxWorkspace>,
    machine_state: MachineState,
    always_on: bool,
) -> String {
    if streaming || workspace.is_some_and(|w| w.running) {
        let mut s = String::from("Working");
        if let Some(w) = workspace.filter(|w| w.running) {
            if let Some(ms) = w.elapsed_ms {
                s.push_str(&format!(" · {}", duration_s(ms / 1000)));
            }
            if let Some(n) = w.tool_calls.filter(|n| *n > 0) {
                s.push_str(&format!(
                    " · {n} tool call{}",
                    if n == 1 { "" } else { "s" }
                ));
            }
        }
        s
    } else if always_on || machine_state == MachineState::Awake {
        "Idle".to_owned()
    } else {
        format!("Machine {}", machine_state.label())
    }
}

/// A routine row's subtitle: its trigger, then the next or last run.
pub(crate) fn routine_status(r: &Routine, now: i64) -> String {
    let when = match (r.enabled, r.next_run_at, r.last_run_at) {
        (true, Some(next), _) => Some(format!("next {}", rel_time(next, now))),
        (true, None, Some(last)) => Some(format!("last {}", rel_time(last, now))),
        (true, None, None) => None,
        (false, _, _) => Some("Paused".to_owned()),
    };
    match when {
        Some(w) if r.enabled => format!("{} · {w}", r.trigger.label()),
        Some(w) => w,
        None => r.trigger.label(),
    }
}

/// Former Details-tab body (Coding / Routines / Settings). Overview + Edit
/// replaced the landing; kept for the Settings card shape tests / callers.
#[allow(dead_code)]
fn details_sections(
    project: &Project,
    mine: bool,
    live: DetailsLive,
    model_rows: Vec<gpui::AnyElement>,
    pick: Option<AgentPick>,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    let mut body: Vec<Section> = Vec::new();
    let mut model_rows = Some(model_rows);
    for block in details_order(mine, AGENT_DETAILS_SHOW_CODING_ROUTINES) {
        match block {
            DetailsBlock::Coding => {
                let working = live.streaming || live.workspace.as_ref().is_some_and(|w| w.running);
                let leading: gpui::AnyElement = if working {
                    PulseClock::request(cx);
                    div()
                        .size(px(kit::AVATAR))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(crate::ui::cards::activity_spinner(
                            CODING_SPINNER,
                            theme,
                            cx,
                        ))
                        .debug_selector(|| "agent-coding-spinner".into())
                        .into_any_element()
                } else {
                    div()
                        .size(px(kit::AVATAR))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(sf_symbols::glyph(
                            "chevron.left.forwardslash.chevron.right",
                            18.0,
                            theme.label_muted(),
                        ))
                        .into_any_element()
                };
                let status = coding_status(
                    live.streaming,
                    live.workspace.as_ref(),
                    live.machine_state,
                    live.always_on,
                );
                let row = kit::row(
                    "agent-coding-row",
                    Leading::Avatar(leading),
                    project.name.clone(),
                    Some(status.into()),
                    None,
                    None,
                    true,
                    theme,
                )
                .cursor_pointer()
                .group(kit::HOT)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state
                        .update(cx, |s, cx| s.set_profile_tab(TAB_COMPUTER, cx));
                }));
                body.push(Section::Title("Coding".into()));
                body.push(Section::Card(vec![row.into_any_element()]));
            }
            DetailsBlock::Routines => {
                body.push(Section::Title("Routines".into()));
                let mut rows: Vec<gpui::AnyElement> = Vec::new();
                for r in &live.routines {
                    let (rid, on) = (r.id.clone(), r.enabled);
                    let switch = div()
                        .id(ElementId::Name(SharedString::from(format!(
                            "agent-routine-switch-{}",
                            r.id
                        ))))
                        .cursor_pointer()
                        .child(glass::switch(on, theme))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let rid = rid.clone();
                            this.state
                                .update(cx, |s, cx| s.routine_set_enabled(rid, !on, cx));
                        }))
                        .into_any_element();
                    rows.push(
                        kit::row(
                            ElementId::Name(SharedString::from(format!("agent-routine-{}", r.id))),
                            Leading::None,
                            r.name.clone(),
                            Some(routine_status(r, live.now).into()),
                            None,
                            Some(switch),
                            false,
                            theme,
                        )
                        .into_any_element(),
                    );
                }
                if rows.is_empty() {
                    rows.push(
                        kit::row(
                            "agent-routines-none",
                            Leading::None,
                            "No routines yet",
                            Some(format!("Ask {} to schedule one", project.name).into()),
                            None,
                            None,
                            false,
                            theme,
                        )
                        .text_color(theme.label_muted())
                        .into_any_element(),
                    );
                }
                body.push(Section::Card(rows));
            }
            DetailsBlock::Settings => {
                body.push(Section::Title("Settings".into()));
                // When a row has just opened, the pane scrolls so its list is
                // in view (Section::Reveal). Rows are children 0 / 1 / 2.
                let open = pick.filter(|_| mine).map(|open| match open {
                    AgentPick::Model => 0,
                    AgentPick::Reasoning => 1,
                    AgentPick::Voice => 2,
                });
                let rows = model_rows.take().unwrap_or_default();
                body.push(match open {
                    Some(open) => Section::Reveal { rows, open },
                    None => Section::Card(rows),
                });
            }
        }
    }
    body
}

fn library_sections(
    content: super::Content,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    // ----- Library: everything the agent's chats hold, one list by day ------------
    // ([feedback library-list-hover-mac-2026-10-25], `profiles/library.rs`).
    super::library::sections(content, shell, theme, cx)
}

/// The Computer tab's blocks, in order. Pure, for the test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ComputerBlock {
    /// "Machine": status · where it runs · spec · image · spend today.
    Machine,
    /// "Resources": disk · memory · load — when the box answered.
    Resources,
    /// "Running now": every workspace with a turn running, or "Nothing running".
    Running,
}

pub(crate) fn computer_order(mine: bool, report: Option<&BoxReport>) -> Vec<ComputerBlock> {
    let mut out = vec![ComputerBlock::Machine];
    if mine {
        if report.is_some_and(|r| r.reachable) {
            out.push(ComputerBlock::Resources);
        }
        out.push(ComputerBlock::Running);
    }
    out
}

/// "6.9 GB free of 10 GB".
pub(crate) fn free_of(available_mb: Option<u64>, total_mb: Option<u64>) -> Option<String> {
    match (available_mb, total_mb) {
        (Some(a), Some(t)) => Some(format!("{} free of {}", size_mb(a), size_mb(t))),
        (Some(a), None) => Some(format!("{} free", size_mb(a))),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn computer_sections(
    project: &Project,
    mine: bool,
    machine_state: MachineState,
    always_on: bool,
    machine: &crate::model::Machine,
    report: Option<&BoxReport>,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    // ----- Computer: the agent's box (sandbox state, disk, running jobs) --------
    // Grok's third tab. Own agents: `GET /machine` (state, spec, image, spend)
    // plus the box's own `/box/state` through `GET /machine/debug`; someone
    // else's agent: what its project carries (state, class).
    let mut body: Vec<Section> = Vec::new();
    // The Status row's trailing mark (Jacob, 2026-10-27: never a coloured dot):
    // the spinner while the machine wakes, the red `!` on a fault, nothing at rest.
    let dot = |state: MachineState, cx: &mut gpui::App| -> Option<gpui::AnyElement> {
        crate::ui::marks::machine_mark_element(state, 10.0, theme, cx)
    };
    for block in computer_order(mine, report) {
        match block {
            ComputerBlock::Machine => {
                body.push(Section::Title("Machine".into()));
                let mut rows: Vec<gpui::AnyElement> = Vec::new();
                let shown = if always_on && machine_state != MachineState::Error {
                    MachineState::Awake
                } else {
                    machine_state
                };
                let mut status = capitalise(shown.label());
                if let Some(s) = report
                    .and_then(|r| r.awake_for_s)
                    .filter(|_| shown == MachineState::Awake)
                {
                    status = format!("{status} · {}", duration_s(s));
                }
                rows.push(
                    kit::row(
                        "agent-machine-status",
                        Leading::None,
                        "Status",
                        None,
                        Some(status.into()),
                        dot(shown, cx),
                        false,
                        theme,
                    )
                    .debug_selector(|| "agent-machine-status".into())
                    .into_any_element(),
                );
                if mine {
                    rows.push(
                        kit::value_row(
                            "agent-machine-where",
                            "Where it runs",
                            machine.where_it_runs(),
                            theme,
                        )
                        .into_any_element(),
                    );
                    if let Some(spec) = machine.spec.clone().or_else(|| machine.class.clone()) {
                        rows.push(
                            kit::value_row("agent-machine-spec", "Machine", spec, theme)
                                .into_any_element(),
                        );
                    }
                    if let Some(image) = report
                        .and_then(|r| r.image.clone())
                        .or_else(|| machine.image.clone())
                    {
                        rows.push(
                            kit::value_row("agent-machine-image", "Image", image, theme)
                                .into_any_element(),
                        );
                    }
                    if machine.spend_cap_usd > 0.0 || machine.spend_today_usd > 0.0 {
                        let spend = if machine.spend_cap_usd > 0.0 {
                            format!(
                                "${:.2} of ${:.0}",
                                machine.spend_today_usd, machine.spend_cap_usd
                            )
                        } else {
                            format!("${:.2}", machine.spend_today_usd)
                        };
                        rows.push(
                            kit::value_row("agent-machine-spend", "Spend today", spend, theme)
                                .into_any_element(),
                        );
                    }
                } else if let Some(m) = &project.machine_info {
                    if !m.class.is_empty() {
                        rows.push(
                            kit::value_row(
                                "agent-machine-class",
                                "Machine",
                                m.class.clone(),
                                theme,
                            )
                            .into_any_element(),
                        );
                    }
                }
                body.push(Section::Card(rows));
            }
            ComputerBlock::Resources => {
                let Some(r) = report else { continue };
                body.push(Section::Title("Resources".into()));
                let mut rows: Vec<gpui::AnyElement> = Vec::new();
                if let Some(disk) = free_of(r.disk_available_mb, r.disk_total_mb) {
                    rows.push(
                        kit::value_row("agent-box-disk", "Disk", disk, theme).into_any_element(),
                    );
                }
                if let Some(mem) = free_of(r.mem_available_mb, r.mem_total_mb) {
                    rows.push(
                        kit::value_row("agent-box-memory", "Memory", mem, theme).into_any_element(),
                    );
                }
                // CPU share from the box's cgroup (agentd ≥ 0.6.277); the load average is
                // the host's inside Daytona (`load_is_host`), so it is drawn only when it is
                // really the box's and no CPU figure exists.
                if let Some(cpu) = r.cpu_pct {
                    rows.push(
                        kit::value_row(
                            "agent-box-cpu",
                            "CPU",
                            format!("{} %", cpu.round() as u64),
                            theme,
                        )
                        .into_any_element(),
                    );
                } else if let Some(load) = r.load_1m.filter(|_| !r.load_is_host) {
                    rows.push(
                        kit::value_row("agent-box-load", "Load", format!("{load:.2}"), theme)
                            .into_any_element(),
                    );
                }
                if let Some(up) = r.uptime_s {
                    rows.push(
                        kit::value_row("agent-box-uptime", "Up for", duration_s(up), theme)
                            .into_any_element(),
                    );
                }
                body.push(Section::Card(rows));
            }
            ComputerBlock::Running => {
                body.push(Section::Title("Running now".into()));
                let mut rows: Vec<gpui::AnyElement> = Vec::new();
                match report {
                    None => rows.push(
                        kit::row(
                            "agent-box-checking",
                            Leading::None,
                            "Checking the machine…",
                            None,
                            None,
                            None,
                            false,
                            theme,
                        )
                        .text_color(theme.label_muted())
                        .into_any_element(),
                    ),
                    Some(r) if !r.reachable => rows.push(
                        kit::row(
                            "agent-box-unreachable",
                            Leading::None,
                            "The machine did not answer",
                            r.unreachable_reason.clone().map(SharedString::from),
                            None,
                            None,
                            false,
                            theme,
                        )
                        .text_color(theme.label_muted())
                        .into_any_element(),
                    ),
                    Some(r) => {
                        for w in r.running() {
                            PulseClock::request(cx);
                            let mut detail = w
                                .elapsed_ms
                                .map(|ms| duration_s(ms / 1000))
                                .unwrap_or_default();
                            if let Some(n) = w.tool_calls.filter(|n| *n > 0) {
                                if !detail.is_empty() {
                                    detail.push_str(" · ");
                                }
                                detail.push_str(&format!(
                                    "{n} tool call{}",
                                    if n == 1 { "" } else { "s" }
                                ));
                            }
                            let spinner = div()
                                .size(px(kit::AVATAR))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(crate::ui::cards::activity_spinner(
                                    CODING_SPINNER,
                                    theme,
                                    cx,
                                ))
                                .into_any_element();
                            rows.push(
                                kit::row(
                                    ElementId::Name(SharedString::from(format!(
                                        "agent-box-job-{}",
                                        w.project_id
                                    ))),
                                    Leading::Avatar(spinner),
                                    w.name.clone(),
                                    (!detail.is_empty()).then(|| detail.into()),
                                    None,
                                    None,
                                    false,
                                    theme,
                                )
                                .into_any_element(),
                            );
                        }
                        if rows.is_empty() {
                            rows.push(
                                kit::row(
                                    "agent-box-idle",
                                    Leading::None,
                                    "Nothing running",
                                    Some(
                                        format!(
                                            "{} workspace{} on this machine",
                                            r.workspaces.len(),
                                            if r.workspaces.len() == 1 { "" } else { "s" }
                                        )
                                        .into(),
                                    ),
                                    None,
                                    None,
                                    false,
                                    theme,
                                )
                                .text_color(theme.label_muted())
                                .into_any_element(),
                            );
                        }
                    }
                }
                body.push(Section::Card(rows));
            }
        }
    }
    body
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Edit Agent modal sheet — Grok Bot Edit hierarchy as a Mac sheet (not a push):
/// Done pill, avatar with pencil, Name / Label / Appearance / Voice Calls cards.
pub fn edit_sheet(shell: &Shell, cx: &mut Context<Shell>) -> Option<Stateful<Div>> {
    let theme = Theme::of(cx).clone();
    let p = palette(&theme);
    let state = shell.state.read(cx);
    let ProfilePage::Agent { project_id } = state.profile.clone()? else {
        return None;
    };
    let project = state.project(&project_id)?.clone();
    let mine = !project.is_guest();
    let model_text = if project.model.is_empty() {
        "default".to_string()
    } else {
        project.model.clone()
    };
    let voice_text = crate::ui::voicepicker::current_label(&project, &state.voices);
    let pick = state.agent_pick;
    let hero_image = avatars::project_image(state, &project);
    let _ = state;

    const WIDTH: f32 = create_entity::WIDTH;
    const RADIUS: f32 = 16.0;
    const HEADER: f32 = 54.5;
    const PREVIEW: f32 = create_entity::PREVIEW;

    let header = div()
        .relative()
        .flex_none()
        .w_full()
        .h(px(HEADER))
        .border_b_1()
        .border_color(p.line)
        .flex()
        .items_center()
        .px(px(16.0))
        .child(
            div()
                .id("agent-edit-close")
                .size(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .map(|el| highlight::button(el, &theme))
                .child(sf_symbols::glyph("xmark", 10.0, hex(0xD2D2D2)))
                .on_click(cx.listener(|this, _, window, cx| this.close_agent_edit(window, cx))),
        )
        .child(
            div()
                .flex_1()
                .px(px(12.0))
                .text_size(px(14.2))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(p.title)
                .text_center()
                .child("Edit Agent"),
        )
        .child(
            kit::done_pill("edit-done", "Done", true, &theme)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| this.save_agent_edit(window, cx))),
        );

    let pic = div().size(px(PREVIEW)).child(match hero_image {
        Some(image) => div()
            .size(px(PREVIEW))
            .rounded_full()
            .overflow_hidden()
            .child(
                gpui::img(crate::ui::picture_fit::fit(&image, PREVIEW))
                    .size(px(PREVIEW))
                    .rounded_full()
                    .object_fit(gpui::ObjectFit::Cover),
            )
            .into_any_element(),
        None => avatars::project_hero(&project, PREVIEW, &theme).into_any_element(),
    });
    let pid_av = project.id.clone();
    let hero = div()
        .id("edit-avatar")
        .debug_selector(|| "edit-avatar".into())
        .w_full()
        .flex()
        .justify_center()
        .pt(px(18.0))
        .pb(px(8.0))
        .child(
            div()
                .id("edit-avatar-disc")
                .relative()
                .size(px(PREVIEW))
                .when(mine, |el| {
                    el.cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_avatar_picker(
                                crate::ui::avatar_picker::Target::Agent(pid_av.clone()),
                                cx,
                            );
                        }))
                })
                .child(pic)
                .when(mine, |el| {
                    el.child(
                        div()
                            .absolute()
                            .right(px(2.0))
                            .bottom(px(2.0))
                            .size(px(28.0))
                            .rounded_full()
                            .bg(theme.control_fill())
                            .border_1()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(sf_symbols::glyph("pencil", 12.0, theme.text)),
                    )
                }),
        );

    let name_section = div()
        .w_full()
        .flex()
        .flex_col()
        .child(kit::edit_section_header("Name", &theme))
        .child(
            kit::card(&theme).child(
                div()
                    .id("entity-name-field")
                    .debug_selector(|| "entity-name-field".into())
                    .h(px(41.0))
                    .px(px(kit::INSET))
                    .flex()
                    .items_center()
                    .text_size(px(15.0))
                    .text_color(p.title)
                    .child(shell.agent_name_input.clone()),
            ),
        );

    let label_section = div()
        .w_full()
        .flex()
        .flex_col()
        .child(kit::edit_section_header("Label", &theme))
        .child(
            kit::card(&theme).child(
                div()
                    .id("agent-label-field")
                    .debug_selector(|| "agent-label-field".into())
                    .h(px(41.0))
                    .px(px(kit::INSET))
                    .flex()
                    .items_center()
                    .text_size(px(15.0))
                    .text_color(p.title)
                    .child(shell.description_input.clone()),
            ),
        );

    let pid_row = project.id.clone();
    let appearance = div()
        .w_full()
        .flex()
        .flex_col()
        .child(kit::edit_section_header("Appearance", &theme))
        .child(
            kit::card(&theme).child(
                kit::row(
                    "edit-avatar-row",
                    Leading::None,
                    "Avatar",
                    None,
                    None,
                    None,
                    mine,
                    &theme,
                )
                .when(mine, |el| {
                    el.cursor_pointer().group(kit::HOT).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.open_avatar_picker(
                                crate::ui::avatar_picker::Target::Agent(pid_row.clone()),
                                cx,
                            );
                        },
                    ))
                })
                .into_any_element(),
            ),
        );

    let settings = if mine {
        let model_open = pick == Some(AgentPick::Model);
        let reasoning_open = pick == Some(AgentPick::Reasoning);
        let voice_open = pick == Some(AgentPick::Voice);
        let rows = vec![
            hoverpick::row(
                "Model",
                model_text,
                AgentPick::Model,
                model_open,
                model_open.then(|| hoverpick::model_panel(shell, &project, cx)),
                &theme,
                cx,
            )
            .into_any_element(),
            hoverpick::row(
                "Reasoning",
                project.effort.label(),
                AgentPick::Reasoning,
                reasoning_open,
                reasoning_open.then(|| hoverpick::reasoning_panel(&project, cx)),
                &theme,
                cx,
            )
            .into_any_element(),
            hoverpick::row(
                "Voice",
                voice_text,
                AgentPick::Voice,
                voice_open,
                voice_open.then(|| hoverpick::voice_panel(shell, &project, cx)),
                &theme,
                cx,
            )
            .into_any_element(),
        ];
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(kit::edit_section_header("Voice Calls", &theme))
            .child(kit::card(&theme).children(rows))
    } else {
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(kit::edit_section_header("Settings", &theme))
            .child(kit::card(&theme).child(
                kit::value_row("agent-model-row", "Model", model_text, &theme).into_any_element(),
            ))
    };

    let card = div()
        .id("agent-edit-card")
        .debug_selector(|| "agent-edit-card".into())
        .relative()
        .w(px(WIDTH))
        .max_w_full()
        .max_h_full()
        .flex()
        .flex_col()
        .rounded(px(RADIUS))
        .bg(p.bg)
        .border_1()
        .border_color(p.line)
        .shadow_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(header)
        .child(
            div()
                .id("agent-edit-body")
                .flex()
                .flex_col()
                .flex_1()
                .w_full()
                .overflow_y_scroll()
                .pb(px(24.0))
                .child(hero)
                .child(name_section)
                .child(label_section)
                .child(appearance)
                .child(settings),
        );

    Some(
        div()
            .id("agent-edit")
            .debug_selector(|| "agent-edit".into())
            .absolute()
            .inset_0()
            .occlude()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .flex()
            .items_center()
            .justify_center()
            .p(px(24.0))
            .bg(Theme::with_alpha(theme.bg, 0.55))
            .child(frosted(RADIUS, PANEL_BLUR, card))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_agent_edit(window, cx)),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RoutineTrigger;

    /// Overview rows always include Coding / Routines; Settings live on Edit.
    /// `details_order` still describes the old Details blocks for the flag.
    #[test]
    fn details_order_puts_coding_routines_ahead_of_settings_when_shown() {
        assert_eq!(
            details_order(true, AGENT_DETAILS_SHOW_CODING_ROUTINES),
            [
                DetailsBlock::Coding,
                DetailsBlock::Routines,
                DetailsBlock::Settings
            ]
        );
        assert_eq!(details_order(true, false), [DetailsBlock::Settings]);
        assert_eq!(details_order(false, false), [DetailsBlock::Settings]);
        assert_eq!(
            details_order(false, true),
            [DetailsBlock::Coding, DetailsBlock::Settings]
        );
    }

    #[test]
    fn the_overview_rows_match_iphone_pane_kinds() {
        assert_eq!(TABS, ["Coding", "Routines", "Library", "Computer"]);
        assert_eq!(OUTSIDE_TABS, ["Library", "Connection"]);
        assert_eq!(TABS[TAB_CODING], "Coding");
        assert_eq!(TABS[TAB_ROUTINES], "Routines");
        assert_eq!(TABS[TAB_LIBRARY], "Library");
        assert_eq!(TABS[TAB_COMPUTER], "Computer");
        assert!(AGENT_DETAILS_SHOW_CODING_ROUTINES);
        assert_eq!(LABEL_PLACEHOLDER, "Add a label");
        assert_eq!(CODING_SPINNER, 15.0, "Grok's spinner: 30 px at 2 px/pt");
    }

    #[test]
    fn coding_status_reads_the_turn_then_the_machine() {
        let ws = BoxWorkspace {
            running: true,
            elapsed_ms: Some(42_000),
            tool_calls: Some(7),
            ..Default::default()
        };
        assert_eq!(
            coding_status(true, Some(&ws), MachineState::Awake, false),
            "Working · 42 s · 7 tool calls"
        );
        // Streaming per the chat, no report yet: still "Working".
        assert_eq!(
            coding_status(true, None, MachineState::Asleep, false),
            "Working"
        );
        assert_eq!(
            coding_status(false, None, MachineState::Awake, false),
            "Idle"
        );
        assert_eq!(
            coding_status(false, None, MachineState::Asleep, true),
            "Idle"
        );
        assert_eq!(
            coding_status(false, None, MachineState::Asleep, false),
            "Machine asleep"
        );
    }

    #[test]
    fn routine_status_names_the_trigger_and_the_run() {
        let now = 1_700_000_000_000;
        let mut r = Routine {
            id: "r".into(),
            project_id: "p".into(),
            chat_id: "c".into(),
            name: "Stand-up".into(),
            prompt: String::new(),
            trigger: RoutineTrigger::Webhook {
                secret: String::new(),
            },
            enabled: true,
            created_at: 0,
            last_run_at: None,
            next_run_at: Some(now + 3_600_000),
            fire_url: None,
        };
        let s = routine_status(&r, now);
        assert!(s.starts_with("Webhook · next "), "{s}");
        r.enabled = false;
        assert_eq!(routine_status(&r, now), "Paused");
        r.enabled = true;
        r.next_run_at = None;
        assert_eq!(routine_status(&r, now), "Webhook");
    }

    /// The Computer tab: the machine card always; resources only when the box
    /// answered; running jobs for my own agents.
    #[test]
    fn computer_order_follows_the_report() {
        assert_eq!(computer_order(false, None), [ComputerBlock::Machine]);
        assert_eq!(
            computer_order(true, None),
            [ComputerBlock::Machine, ComputerBlock::Running]
        );
        let unreachable = BoxReport {
            reachable: false,
            ..Default::default()
        };
        assert_eq!(
            computer_order(true, Some(&unreachable)),
            [ComputerBlock::Machine, ComputerBlock::Running]
        );
        let reachable = BoxReport {
            reachable: true,
            ..Default::default()
        };
        assert_eq!(
            computer_order(true, Some(&reachable)),
            [
                ComputerBlock::Machine,
                ComputerBlock::Resources,
                ComputerBlock::Running
            ]
        );
        assert_eq!(
            free_of(Some(6_900), Some(10 * 1024)).as_deref(),
            Some("6.7 GB free of 10 GB")
        );
        assert_eq!(free_of(None, Some(1)), None);
    }

    /// `GET /machine/debug` as the relay answers it → the report the tab reads.
    #[test]
    fn box_report_reads_the_relay_and_the_box() {
        let v: serde_json::Value = serde_json::json!({
            "relay": { "state": "awake", "awake_for_s": 18720, "agentd": { "image": "arbos-agent-0.1.114" } },
            "box": { "reachable": true, "state": {
                "uptime_s": 18720, "load": [0.42, 0.3, 0.2],
                "memory_mb": { "total": 8192, "available": 5300 },
                "disk": { "path": "/work", "total_mb": 10240, "available_mb": 6900 },
                "workspaces": [
                    { "project_id": "p1", "name": "Arbos", "healthy": true,
                      "turn": { "running": true, "current": { "elapsed_ms": 42000, "tool_calls": 7 } } },
                    { "project_id": "p2", "name": "Affine", "healthy": true, "turn": { "running": false, "current": null } }
                ]
            } }
        });
        let r = BoxReport::from_debug(&v, 5);
        assert!(r.reachable);
        assert_eq!(r.state, "awake");
        assert_eq!(r.image.as_deref(), Some("arbos-agent-0.1.114"));
        assert_eq!(r.disk_available_mb, Some(6900));
        assert_eq!(r.load_1m, Some(0.42));
        assert!(!r.load_is_host);
        assert_eq!(r.cpu_pct, None);
        assert!(r.usage_rows().is_empty());
        assert_eq!(r.jobs_running(), None);
        assert_eq!(r.workspaces.len(), 2);
        assert_eq!(r.running().count(), 1);
        assert_eq!(r.workspace("p1").unwrap().tool_calls, Some(7));
        assert!(!r.workspace("p2").unwrap().running);
        let asleep = BoxReport::from_debug(
            &serde_json::json!({ "relay": { "state": "asleep" }, "box": { "reachable": false, "error": "box has no url yet" } }),
            5,
        );
        assert!(!asleep.reachable);
        assert_eq!(
            asleep.unreachable_reason.as_deref(),
            Some("box has no url yet")
        );
        assert!(asleep.workspaces.is_empty());
    }

    /// agentd ≥ 0.6.277 ([feedback machine-page-agentd-2026-10-26]): `resources` carries the
    /// cgroup cap (inside Daytona `/proc/meminfo` is the host's), `load_scope` says the load is
    /// the host's, and every workspace has its share and job count.
    #[test]
    fn box_report_reads_the_samplers_resources_and_per_workspace_usage() {
        let v: serde_json::Value = serde_json::json!({
            "relay": { "state": "awake", "awake_for_s": 100, "agentd": { "image": "arbos-agent-0.1.152" } },
            "box": { "reachable": true, "state": {
                "uptime_s": 100, "load": [38.5, 36.9, 32.1], "load_scope": "host",
                "memory_mb": { "total": 8192, "available": 7039, "source": "cgroup" },
                "disk": { "path": "/work", "total_mb": 10240, "available_mb": 9925 },
                "resources": {
                    "sampled_at": 1, "load_scope": "host",
                    "cpu": { "cores": 4.0, "pct": 23.4, "source": "cgroup" },
                    "memory": { "total_mb": 8192, "used_mb": 1153, "peak_mb": 1204, "source": "cgroup" },
                    "disk": { "total_mb": 10240, "used_mb": 315, "available_mb": 9925, "inodes_total": 131072, "inodes_avail": 93000 },
                    "net": { "rx_bps": 1200000.0, "tx_bps": 80000.0, "rx_bytes_today": 2000000000u64, "tx_bytes_today": 100000000u64 }
                },
                "workspaces": [
                    { "project_id": "p1", "name": "Arbos", "healthy": true, "jobs_running": 1,
                      "usage": { "cpu_pct": 61.0, "mem_mb": 1900, "pids": 40, "disk_mb": 2300 },
                      "turn": { "running": true, "current": { "elapsed_ms": 42000, "tool_calls": 7 } } },
                    { "project_id": "p2", "name": "Comms", "healthy": true, "jobs_running": 0,
                      "usage": { "cpu_pct": 0.0, "mem_mb": 600, "pids": 12, "disk_mb": 400 },
                      "turn": { "running": false, "current": null } }
                ]
            } }
        });
        let r = BoxReport::from_debug(&v, 5);
        assert!(r.load_is_host);
        assert_eq!(r.cpu_cores, Some(4.0));
        assert_eq!(r.cpu_pct, Some(23.4));
        assert_eq!(r.mem_total_mb, Some(8192));
        assert_eq!(r.mem_available_mb, Some(8192 - 1153));
        assert_eq!(r.inodes_avail, Some(93000));
        assert_eq!(r.net_rx_bps, Some(1_200_000.0));
        assert_eq!(r.net_tx_today, Some(100_000_000));
        assert_eq!(r.jobs_running(), Some(1));
        let rows = r.usage_rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Arbos");
        assert_eq!(rows[0].disk_mb, Some(2300));
        assert_eq!(rows[1].jobs_running, Some(0));
    }
}

/// The foreign agent's "Run by …" owner link: a mention-chip pill under the
/// name, a click pushes the owner's page over the agent's and `‹` brings the
/// agent back (Jacob 2026-10-25, [feedback agent-pane-run-by-link-mac-2026-10-25]).
#[cfg(test)]
mod run_by_tests {
    use futures::future::BoxFuture;
    use gpui::{
        point, prelude::*, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext,
    };
    use tokio::sync::mpsc;

    use crate::app::{AppState, ProfilePage};
    use crate::model::{Person, Project};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;
    use crate::ui::profiles::{run_by_label, run_by_link};
    use crate::ui::shell::Shell;
    use crate::ui::theme::Theme;

    #[test]
    fn the_label_names_the_owner_by_display_name_then_handle() {
        assert_eq!(
            run_by_label(Some("Sami Kassab"), "sami"),
            "Run by Sami Kassab"
        );
        assert_eq!(run_by_label(Some("  "), "sami"), "Run by @sami");
        assert_eq!(run_by_label(None, "sami"), "Run by @sami");
    }

    struct Idle;

    impl SyncSource for Idle {
        fn name(&self) -> &'static str {
            "idle"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            _commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            Box::pin(std::future::pending())
        }
    }

    fn foreign_agent() -> Project {
        Project {
            id: "p-test-agent".into(),
            name: "TestAgent".into(),
            shared: true,
            owner_handle: Some("sami".into()),
            owner_person: Some(Person {
                id: "gh_2".into(),
                handle: "sami".into(),
                name: Some("Sami Kassab".into()),
                role: "owner".into(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn mount(
        cx: &mut TestAppContext,
    ) -> (
        tokio::runtime::Runtime,
        Entity<AppState>,
        Entity<Shell>,
        &mut VisualTestContext,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        state.update(cx, |s, cx| {
            s.apply(SyncEvent::Projects(vec![foreign_agent()]), cx);
            s.open_profile(
                ProfilePage::Agent {
                    project_id: "p-test-agent".into(),
                },
                cx,
            );
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.run_until_parked();
        (rt, state, shell, vcx)
    }

    fn settle(shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        for _ in 0..3 {
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    fn centre(b: Bounds<Pixels>) -> gpui::Point<Pixels> {
        point(b.left() + b.size.width / 2.0, b.top() + b.size.height / 2.0)
    }

    /// Headless: the link renders under the name of someone else's agent, a
    /// click pushes the owner's page (one frame on the stack, `‹` in the top
    /// bar), and `‹` returns to the agent's page with an empty stack.
    #[gpui::test]
    fn the_run_by_link_pushes_the_owner_and_back_returns(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        settle(&shell, vcx);
        let link = vcx
            .debug_bounds("run-by-link")
            .expect("the Run by link is drawn");
        let name = vcx.debug_bounds("hero-subtitle");
        assert!(name.is_none(), "the link replaces the plain subtitle");
        assert!(
            vcx.debug_bounds("profile-back").is_none(),
            "no back disc on the first page"
        );
        vcx.simulate_click(centre(link), Modifiers::default());
        settle(&shell, vcx);
        state.read_with(vcx, |s, _| {
            assert_eq!(
                s.profile,
                Some(ProfilePage::User {
                    handle: "sami".into(),
                    space_id: Some("p-test-agent".into()),
                }),
                "the owner's page is open"
            );
            assert_eq!(
                s.profile_stack.len(),
                1,
                "the agent's page waits underneath"
            );
        });
        let back = vcx
            .debug_bounds("profile-back")
            .expect("‹ back disc on the pushed page");
        assert_eq!(
            f32::from(back.size.width),
            crate::ui::theme::CONTROL,
            "the family's disc"
        );
        vcx.simulate_click(centre(back), Modifiers::default());
        settle(&shell, vcx);
        state.read_with(vcx, |s, _| {
            assert_eq!(
                s.profile,
                Some(ProfilePage::Agent {
                    project_id: "p-test-agent".into()
                }),
                "back on the agent's page"
            );
            assert!(s.profile_stack.is_empty());
        });
        assert!(
            vcx.debug_bounds("run-by-link").is_some(),
            "the link is back too"
        );
    }

    struct Pill;

    impl gpui::Render for Pill {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            gpui::div()
                .w(gpui::px(340.0))
                .h(gpui::px(200.0))
                .flex()
                .items_start()
                .justify_center()
                .child(run_by_link("Run by Sami Kassab", 0.14, &theme))
        }
    }

    /// The pill alone: one line, the chip's rest alpha, inside the hero width.
    #[gpui::test]
    fn the_link_is_one_chip_sized_line(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte)));
        let (_view, vcx) = cx.add_window_view(|_, _| Pill);
        vcx.run_until_parked();
        let b = vcx.debug_bounds("run-by-link").expect("link");
        let h: f32 = b.size.height.into();
        let w: f32 = b.size.width.into();
        assert!(h < 30.0, "one 12 pt line in a pill: {h}");
        assert!(w > 60.0 && w <= 340.0 - 64.0, "hugging its text: {w}");
    }
}
