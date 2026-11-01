//! An agent's page on the shared profile pane (D93): one [`ProfilePane`]
//! spec — hero (the plain leaf / photo with the owner's crayon, the name and
//! the label, both edited in place), the tabs **Details** (Settings: model,
//! reasoning, call voice) | **Library** (images, files, links from the
//! agent's chats) | **Computer** (the agent's box). The same three tabs as the
//! iPhone's agent page. Share project and Add to this chat left the pane on
//! 2026-10-22 (Jacob: simple, consistent, powerful); the Voice row came back
//! on 2026-10-23 (the call voice — spoken replies stay behind the voice-mode
//! switch); the avatar picker opens from the hero while the pane is up. The
//! "Who may message", "Permissions" and "Profile" cards left Details on
//! 2026-10-25 ([feedback agent-details-remove-sections-mac-2026-10-25]).
//!
//! Grok Bot parity ([feedback info-pane-chrome-2026-10-24], Jacob 2026-10-24):
//! no "Delete agent" button on the pane — deleting an agent is the sidebar
//! row's menu (`sidebar::row_menu`, Delete, at once); the hero picture is the
//! bare leaf, no pale disc behind it and no presence dot (presence stays on
//! the sidebar row); the top bar carries Grok's share disc (the invite sheet:
//! let a person in on this agent) beside the close disc.

use std::time::Instant;

use gpui::{div, prelude::*, px, Context, Div, ElementId, Focusable, SharedString, Stateful};

use crate::app::{AgentPick, ProfilePage};
use crate::model::{
    duration_s, now_ms, size_mb, BoxReport, BoxWorkspace, Id, MachineState, Project, Routine,
};
use crate::ui::avatars::{self, AvatarState};
use crate::ui::glass;
use crate::ui::hoverpick;
use crate::ui::motion::PulseClock;
use crate::ui::routines::rel_time;
use crate::ui::sf_symbols;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, TEXT_SMALL};

use super::kit::{self, Leading};
use super::pane::{self, Hero, Picture, ProfilePane, Section};
use super::{chats_of, collect, editable_name, hero_picture, HeroClick, HERO_AVATAR};

/// The agent pane's tabs — Grok Bot's ([feedback agent-pane-details-tab-2026-10-24],
/// Jacob 2026-10-24; supersedes Profile · Settings · Library of 2026-10-22):
/// **Details** (Settings; Coding and Routines ahead of it while
/// [`AGENT_DETAILS_SHOW_CODING_ROUTINES`] is on), **Library**, **Computer**
/// (the agent's box). Every pane keeps the structure (hero, pill, options),
/// not a tab count.
pub(super) const TABS: [&str; 3] = ["Details", "Library", "Computer"];
/// A connected (outside) agent has no Arbos machine: Details · Library (#2141; the
/// Connection and Key cards sit under Details, `connect_agent::details_sections`).
pub(super) const OUTSIDE_TABS: [&str; 2] = ["Details", "Library"];
/// Jacob 2026-10-25 ([feedback agent-details-remove-sections-mac-2026-10-25]):
/// the Details tab hides its Coding and Routines cards for now. Temporary —
/// flip this to bring them back; the Computer tab, the routines calls and the
/// status line are untouched. Off by default.
pub(crate) const AGENT_DETAILS_SHOW_CODING_ROUTINES: bool = false;
/// Details is tab 0 — the pane's default and where any stale index lands.
pub(crate) const TAB_LIBRARY: usize = 1;
pub(crate) const TAB_COMPUTER: usize = 2;
/// Grok's subtitle while a bot has no label.
pub(crate) const LABEL_PLACEHOLDER: &str = "Add a label";
/// Grok's Coding-row spinner: 15 pt (rows 613–642 px at 2 px/pt).
pub(crate) const CODING_SPINNER: f32 = 15.0;

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
            avatar_leaf: a.avatar_leaf,
            avatar_leaf_color: a.avatar_leaf_color,
            // The owner's picture travels with the roster copy; without it the hero drew the
            // automatic leaf for every foreign agent (Jacob, 2026-10-25: "Hezbola").
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
    let renaming = state.profile_editing_name && mine;
    let outside = project.outside.clone();
    // An outside agent has no Computer tab: a stale index lands on Details.
    let tab = match state.profile_tab {
        TAB_COMPUTER if outside.is_some() => 0,
        t => t,
    };
    let editing_description = state.profile_editing_description && mine;
    let pid: Id = project.id.clone();
    // Library tab: what the agent's chats hold (read while `state` is borrowed).
    let content = (tab == TAB_LIBRARY).then(|| {
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

    // D92 amendment: the uploaded / generated picture wins over the leaf (read while `state`
    // is borrowed) — from the newest copy the store holds, like every other surface.
    let hero_image = avatars::project_image(state, &project);
    let newest_picture_url =
        crate::sync::agent_avatar::newest(&state.projects, &project.id).and_then(|f| f.image_url); // avatar-lint: allow (resolver)
    if avatar_state != AvatarState::Idle || streaming {
        PulseClock::request(cx);
    }

    // ----- hero ----------------------------------------------------------------
    // The shared pane hero: owned agents get the crayon on the picture (the
    // avatar-colour picker) and the inline name field; others are read-only.
    // Plain, like Grok's bot mark: the leaf alone (no pastel disc), a photo
    // when one was uploaded, and no machine dot — presence is the sidebar
    // row's ([feedback info-pane-chrome-2026-10-24]).
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
            None => avatars::project_hero_bare(&project, HERO_AVATAR, &theme),
        })
        .into_any_element();
    let p_pic = pid.clone();
    let pic_click = mine.then(|| {
        let f = cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
            this.open_avatar_picker(crate::ui::avatar_picker::Target::Agent(p_pic.clone()), cx);
        });
        Box::new(f) as HeroClick
    });
    // Someone else's agent: its picture opens full-size in the media viewer (a leaf opens nothing).
    let view_click = (!mine).then_some(newest_picture_url).flatten().map(|url| {
        let name = project.name.clone();
        let f = cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
            this.view_picture(&name, &url, cx);
        });
        Box::new(f) as HeroClick
    });
    let avatar = hero_picture("profile-avatar", picture, pic_click, view_click).into_any_element();
    let p_rename = pid.clone();
    let name_click = mine.then(|| {
        let f = cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
            this.start_edit_agent(p_rename.clone(), false, window, cx);
        });
        Box::new(f) as HeroClick
    });
    let name_el = editable_name(
        "profile-name",
        &project.name,
        renaming,
        &shell.agent_name_input,
        name_click,
        &theme,
    )
    .into_any_element();
    // ----- hero (D93): picture, name, the label ------------------------------------
    // Grok's "Add a label" ([feedback agent-pane-details-tab-2026-10-24]): the
    // agent's description is its label — shown under the name, "Add a label"
    // for the owner while it is empty, edited in place on a click. Someone
    // else's agent shows its label, else who runs it. Working state is no
    // longer a subtitle: the Details tab's Coding row spins instead.
    let owner_handle = project
        .owner_handle
        .as_deref()
        .map(|o| o.trim_start_matches('@').to_owned())
        .filter(|o| !o.is_empty());
    let label = project.description.clone().filter(|d| !d.trim().is_empty());
    let subtitle: SharedString = match (&label, mine, &owner_handle) {
        (Some(d), _, _) => d.trim().to_owned().into(),
        (None, true, _) => LABEL_PLACEHOLDER.into(),
        (None, false, Some(o)) => format!("Run by @{o}").into(),
        (None, false, None) => "Agent".into(),
    };
    let subtitle_color = if label.is_some() {
        theme.label_muted()
    } else {
        theme.label_faint()
    };
    let p_label = pid.clone();
    let subtitle_click = (mine && !editing_description).then(|| {
        let f = cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
            this.start_edit_agent(p_label.clone(), true, window, cx);
        });
        Box::new(f) as HeroClick
    });
    // Someone else's agent: "Run by …" is a link to the owner's page, pushed
    // over this one with ‹ back (Jacob 2026-10-25, [feedback
    // agent-pane-run-by-link-mac-2026-10-25]). With no label it is the
    // subtitle; under a label it is the line below it.
    let run_by = (!mine)
        .then(|| owner_handle.clone())
        .flatten()
        .map(|handle| run_by_link_element(&handle, owner_name.as_deref(), &pid, shell, &theme, cx));
    let (subtitle_element, run_by_below) = match (run_by, &label) {
        (Some(link), None) => (Some(link), None),
        (Some(link), Some(_)) => (None, Some(link)),
        (None, _) => (None, None),
    };
    let editor = if editing_description {
        Some(label_editor(shell, &theme).into_any_element())
    } else {
        run_by_below.map(|link| div().pt(px(2.0)).child(link).into_any_element())
    };
    let hero = Hero {
        picture: Picture::Element(avatar),
        name: name_el,
        badge: None,
        subtitle: (!editing_description).then_some(subtitle),
        subtitle_color,
        subtitle_click,
        subtitle_element,
        editor,
        action: None,
    };

    let body: Vec<Section> = match tab {
        TAB_LIBRARY => library_sections(
            content.expect("collected for the Library tab"),
            shell,
            &theme,
            cx,
        ),
        _ if outside.is_some() => crate::ui::connect_agent::details_sections(
            &pid,
            outside.as_ref().expect("outside"),
            shell,
            &theme,
            cx,
        ),
        TAB_COMPUTER => computer_sections(
            &project,
            mine,
            machine_state,
            always_on,
            &machine,
            box_report.as_ref(),
            &theme,
            cx,
        ),
        _ => {
            let model_rows: Vec<gpui::AnyElement> = if mine {
                let model_open = pick == Some(AgentPick::Model);
                let reasoning_open = pick == Some(AgentPick::Reasoning);
                let voice_open = pick == Some(AgentPick::Voice);
                vec![
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
                ]
            } else {
                vec![
                    kit::value_row("agent-model-row", "Model", model_text, &theme)
                        .into_any_element(),
                ]
            };
            details_sections(
                &project,
                mine,
                DetailsLive {
                    streaming,
                    machine_state,
                    always_on,
                    workspace: box_report
                        .as_ref()
                        .and_then(|r| r.workspace(&project.id))
                        .cloned(),
                    routines,
                    now,
                },
                model_rows,
                pick,
                &theme,
                cx,
            )
        }
    };

    // No footer: "Delete agent" left the pane on Jacob's ask ([feedback
    // info-pane-chrome-2026-10-24]); the sidebar row's menu deletes at once.
    let footer: Vec<Section> = Vec::new();
    // Grok's share disc beside the ×: the invite sheet — a pick lets that
    // person in on this agent. Only the owner can share.
    let share_id = pid.clone();
    let share = mine.then(|| {
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

    // No "Edit" — click the avatar to change its colour; click the name or
    // the label to edit. "Done" only while editing.
    let done = (mine && (renaming || editing_description)).then(|| {
        glass::sheet_button("profile-done", "Done", &theme)
            .on_click(cx.listener(|this, _, window, cx| this.finish_edit_agent(window, cx)))
    });
    Some(pane::render(
        ProfilePane {
            page: ProfilePage::Agent {
                project_id: project.id.clone(),
            },
            hero,
            tabs: if outside.is_some() {
                &OUTSIDE_TABS
            } else {
                &TABS
            },
            selected: tab,
            body,
            footer,
            done,
            share,
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

/// The hero's label field while the label is being edited: the shared
/// description input in a single field under the name; Done (top bar) or
/// Return commits, Escape cancels (the input's own keys).
fn label_editor(shell: &Shell, theme: &Theme) -> Div {
    div()
        .w(px(super::SHEET_WIDTH - 80.0))
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(10.0))
        .bg(theme.field_fill())
        .border_1()
        .border_color(theme.primary)
        .text_size(px(TEXT_SMALL))
        .debug_selector(|| "agent-label-editor".into())
        .child(shell.description_input.clone())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RoutineTrigger;

    /// Jacob 2026-10-25 ([feedback agent-details-remove-sections-mac-2026-10-25]):
    /// Details is the Settings card alone — no "Who may message", no
    /// "Permissions", no "Profile" — and Coding / Routines come back ahead of it
    /// only while the flag is on (routines for the owner alone).
    #[test]
    fn details_is_settings_alone_until_the_live_cards_are_switched_back_on() {
        // The shipped default: Coding and Routines stay hidden.
        assert_eq!(
            details_order(true, AGENT_DETAILS_SHOW_CODING_ROUTINES),
            [DetailsBlock::Settings]
        );
        assert_eq!(details_order(true, false), [DetailsBlock::Settings]);
        assert_eq!(details_order(false, false), [DetailsBlock::Settings]);
        assert_eq!(
            details_order(true, true),
            [
                DetailsBlock::Coding,
                DetailsBlock::Routines,
                DetailsBlock::Settings
            ]
        );
        assert_eq!(
            details_order(false, true),
            [DetailsBlock::Coding, DetailsBlock::Settings]
        );
    }

    #[test]
    fn the_tabs_are_groks() {
        assert_eq!(TABS, ["Details", "Library", "Computer"]);
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

/// A newly created agent has no chat and no message yet. Its picture must still be settable from
/// its pane — the owner should never have to talk to an agent first to give it a face.
#[cfg(test)]
mod before_any_conversation_tests {
    use futures::future::BoxFuture;
    use gpui::{
        point, prelude::*, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext,
    };
    use tokio::sync::mpsc;

    use crate::app::{AppState, ProfilePage};
    use crate::model::Project;
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::avatar_picker::Tab;
    use crate::ui::motion::PulseClock;
    use crate::ui::shell::Shell;
    use crate::ui::theme::Theme;

    const AGENT: &str = "p-just-created";

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

    /// My own agent, straight out of the Create sheet: no picture, no chat, no message.
    fn just_created() -> Project {
        Project {
            id: AGENT.into(),
            name: "Pepper".into(),
            shared: false,
            ..Default::default()
        }
    }

    fn centre(b: Bounds<Pixels>) -> gpui::Point<Pixels> {
        point(b.left() + b.size.width / 2.0, b.top() + b.size.height / 2.0)
    }

    fn settle(shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        for _ in 0..3 {
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    /// Headless: the pane of an agent with no conversation draws the editable hero, the hero
    /// opens the avatar picker, and its "Set avatar" goes live as soon as a picture is picked.
    #[gpui::test]
    fn the_picture_can_be_set_with_no_chat_and_no_message(cx: &mut TestAppContext) {
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
            s.apply(SyncEvent::Projects(vec![just_created()]), cx);
            s.open_profile(
                ProfilePage::Agent {
                    project_id: AGENT.into(),
                },
                cx,
            );
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.run_until_parked();
        settle(&shell, vcx);

        // The precondition the tester's report turns on: nothing has been said to this agent.
        state.read_with(vcx, |s, _| {
            assert!(s.tabs(AGENT).is_empty(), "no chat yet");
            assert!(s.primary_chat(AGENT).is_none(), "and so no chat to open");
        });

        let hero = vcx
            .debug_bounds("profile-avatar")
            .expect("the hero picture is drawn for an agent with no conversation");
        assert!(
            vcx.debug_bounds("avatar-card").is_none(),
            "the picker is closed to begin with"
        );
        vcx.simulate_click(centre(hero), Modifiers::default());
        settle(&shell, vcx);
        assert!(
            vcx.debug_bounds("avatar-card").is_some(),
            "the hero opens the avatar picker"
        );

        // The Upload tab, then its pick as the file panel hands it over.
        let upload_tab = vcx
            .debug_bounds("avatar-tab-upload")
            .expect("the Upload tab is offered");
        vcx.simulate_click(centre(upload_tab), Modifiers::default());
        settle(&shell, vcx);
        shell.read_with(vcx, |this, _| {
            let pk = this.avatar_picker.as_ref().expect("picker open");
            assert_eq!(pk.tab, Tab::Upload, "the Upload tab took the click");
            assert!(!pk.dirty(), "nothing picked yet, so Set is inert");
        });
        shell.update(vcx, |this, cx| {
            let pk = this.avatar_picker.as_mut().expect("picker open");
            pk.upload = Some(std::path::PathBuf::from("/tmp/pepper.png"));
            pk.reset = false;
            cx.notify();
        });
        settle(&shell, vcx);

        assert!(
            vcx.debug_bounds("avatar-upload-preview").is_some()
                || vcx.debug_bounds("avatar-set").is_some(),
            "the picked file previews in the sheet"
        );
        shell.read_with(vcx, |this, _| {
            let pk = this.avatar_picker.as_ref().expect("picker open");
            assert!(pk.dirty(), "Set avatar is live for an agent with no chat");
            assert_eq!(pk.target.id().map(String::as_str), Some(AGENT));
            assert!(matches!(
                pk.draft_picture(),
                Some(crate::sync::DraftPicture::File(_))
            ));
        });

        // The press itself: the sheet closes, which is what sends the picture (`set_avatar`).
        let set = vcx.debug_bounds("avatar-set").expect("Set avatar");
        vcx.simulate_click(centre(set), Modifiers::default());
        settle(&shell, vcx);
        shell.read_with(vcx, |this, _| {
            assert!(this.avatar_picker.is_none(), "the press applied and closed");
        });
        state.read_with(vcx, |s, _| {
            assert!(
                s.tabs(AGENT).is_empty(),
                "and it still took no conversation"
            );
        });
    }
}
