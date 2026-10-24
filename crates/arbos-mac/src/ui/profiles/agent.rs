//! An agent's page on the shared profile pane (D93): one [`ProfilePane`]
//! spec — hero (the plain leaf / photo with the owner's crayon, editable name,
//! "working" or "Agent · run by @owner"), the tabs **Profile** (name, handle,
//! description) | **Settings** (model, reasoning, call voice) | **Library**
//! (images, files, links from the agent's chats). The same three tabs as the
//! iPhone's agent page. Share project and Add to this chat left the pane on
//! 2026-10-22 (Jacob: simple, consistent, powerful); the Voice row came back
//! on 2026-10-23 (the call voice — spoken replies stay behind the voice-mode
//! switch); the avatar picker opens from the hero while the pane is up.
//!
//! Grok Bot parity ([feedback info-pane-chrome-2026-10-24], Jacob 2026-10-24):
//! no "Delete agent" button on the pane — deleting an agent is the sidebar
//! row's menu (`sidebar::row_menu`, Delete, at once); the hero picture is the
//! bare leaf, no pale disc behind it and no presence dot (presence stays on
//! the sidebar row); the top bar carries Grok's share disc (the invite sheet:
//! let a person in on this agent) beside the close disc.

use gpui::{div, prelude::*, px, Context, Div, ElementId, Focusable, SharedString, Stateful};

use crate::app::{AgentPick, ProfilePage};
use crate::model::{Id, MachineState, Project};
use crate::ui::avatars::{self, AvatarState};
use crate::ui::glass;
use crate::ui::hoverpick;
use crate::ui::motion::PulseClock;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, TEXT_SMALL};

use super::kit::{self, Leading};
use super::pane::{self, Hero, Picture, ProfilePane, Section};
use super::{
    chats_of, collect, editable_name, editable_picture, file_list, link_list, HeroClick,
    HERO_AVATAR,
};

/// The agent pane's tabs — the same three on the iPhone (`AgentProfileView`).
/// Jacob (2026-10-22 22:32 UTC): Library stays; every pane shares the
/// structure (hero, pill, options), not a tab count.
pub(super) const TABS: [&str; 3] = ["Profile", "Settings", "Library"];

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
    let status = if streaming {
        "working".to_string()
    } else if always_on || machine_state == MachineState::Awake {
        "idle".to_string()
    } else {
        format!("machine {}", machine_state.label())
    };
    let status_color = if streaming {
        theme.online
    } else {
        theme.text_muted
    };
    let model_text = if project.model.is_empty() {
        "default".to_string()
    } else {
        project.model.clone()
    };
    let voice_text = crate::ui::voicepicker::current_label(&project, &state.voices);
    let pick = state.agent_pick;
    let renaming = state.profile_editing_name && mine;
    let tab = state.profile_tab;
    let editing_description = state.profile_editing_description && mine;
    let pid: Id = project.id.clone();
    // Library tab: what the agent's chats hold (read while `state` is borrowed).
    let content = (tab == 2).then(|| {
        let chats: Vec<&crate::model::Chat> = chats_of(state, &project.id);
        collect(state, &chats)
    });

    // D92 amendment: the uploaded / generated picture wins over the leaf (read while `state`
    // is borrowed).
    let hero_image = project
        .image_url
        .as_deref()
        .and_then(|url| state.avatar(url));
    if avatar_state != AvatarState::Idle {
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
                    gpui::img(image)
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
    let avatar = editable_picture("profile-avatar", picture, pic_click).into_any_element();
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
    // ----- hero (D93): picture, name, one subtitle ---------------------------------
    // "working" while a turn runs, else who runs the agent. The @slug lives on
    // the Profile tab (Handle).
    let owner_handle = project
        .owner_handle
        .as_deref()
        .map(|o| o.trim_start_matches('@').to_owned())
        .filter(|o| !o.is_empty());
    let subtitle: SharedString = if streaming {
        "working".into()
    } else if mine {
        "Agent · run by you".into()
    } else if let Some(o) = &owner_handle {
        format!("Agent · run by @{o}").into()
    } else {
        format!("Agent · {status}").into()
    };
    let hero = Hero {
        picture: Picture::Element(avatar),
        name: name_el,
        badge: None,
        subtitle: Some(subtitle),
        subtitle_color: status_color,
        editor: None,
    };

    let description = project.description.clone().unwrap_or_default();
    let slug = format!("@{}", Project::mention_slug(&project.name));
    let mut body: Vec<Section> = Vec::new();

    match tab {
        0 => {
            // ----- Profile: who the agent is ------------------------------------------
            let mut rows: Vec<gpui::AnyElement> = Vec::new();
            let name_row = kit::value_row("agent-name-row", "Name", project.name.clone(), &theme);
            rows.push(if mine {
                let p = pid.clone();
                name_row
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_2))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_edit_agent(p.clone(), false, window, cx);
                    }))
                    .into_any_element()
            } else {
                name_row.into_any_element()
            });
            rows.push(
                kit::value_row("agent-handle-row", "Handle", slug, &theme).into_any_element(),
            );
            if editing_description {
                rows.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .px(px(kit::INSET))
                        .py(px(10.0))
                        .child(
                            div()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.text_muted)
                                .child("Description"),
                        )
                        .child(
                            div()
                                .min_h(px(60.0))
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(8.0))
                                .bg(theme.field)
                                .border_1()
                                .border_color(theme.field_border)
                                .child(shell.description_input.clone()),
                        )
                        .into_any_element(),
                );
            } else {
                let shown = if description.trim().is_empty() {
                    if mine {
                        "Add a description".to_string()
                    } else {
                        "—".to_string()
                    }
                } else {
                    description.clone()
                };
                let row = kit::row(
                    "agent-description-row",
                    Leading::None,
                    "Description",
                    Some(shown.into()),
                    None,
                    None,
                    mine,
                    &theme,
                );
                rows.push(if mine {
                    let p = pid.clone();
                    row.cursor_pointer()
                        .hover(|s| s.bg(theme.surface_2))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_edit_agent(p.clone(), true, window, cx);
                        }))
                        .into_any_element()
                } else {
                    row.into_any_element()
                });
            }
            body.push(Section::Card(rows));
            body.push(Section::Footnote(
                if mine {
                    "The name, handle, picture and description are what people and their agents see, here and on the iPhone."
                } else {
                    "Someone else's agent: what it shares with you shows in your chats."
                }
                .into(),
            ));
        }
        2 => {
            // ----- Library: everything the agent's chats hold ----------------------------
            let content = content.expect("collected for the Library tab");
            if content.media.is_empty() && content.files.is_empty() && content.links.is_empty() {
                body.push(Section::Empty("Nothing shared here yet".into()));
            } else {
                if !content.media.is_empty() {
                    body.push(Section::Title(
                        format!("Images · {}", content.media.len()).into(),
                    ));
                    body.push(Section::Grid(content.media));
                }
                if !content.files.is_empty() {
                    body.push(Section::Title(
                        format!("Files · {}", content.files.len()).into(),
                    ));
                    // Custom: the kit's file list is a list of its own rows (noted on #1330).
                    body.push(Section::Custom(
                        file_list(&content.files, "doc", "", &theme).into_any_element(),
                    ));
                }
                if !content.links.is_empty() {
                    body.push(Section::Title(
                        format!("Links · {}", content.links.len()).into(),
                    ));
                    body.push(Section::Custom(
                        link_list(&content.links, shell, &theme, cx).into_any_element(),
                    ));
                }
            }
        }
        _ => {
            // ----- Settings: how the agent runs — Model · Reasoning · Voice -------------
            // Voice is the agent's call voice (the same picker as the voice
            // sheet); spoken replies in chat stay behind the voice-mode kill
            // switch. Share project and Add to this chat left the pane on
            // 2026-10-22 — Jacob: simple, consistent, powerful.
            if mine {
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
                // When a row has just opened, the pane scrolls so its list is
                // in view (Section::Reveal). Rows are children 0 / 1 / 2.
                let open = pick.map(|open| match open {
                    AgentPick::Model => 0,
                    AgentPick::Reasoning => 1,
                    AgentPick::Voice => 2,
                });
                body.push(match open {
                    Some(open) => Section::Reveal { rows, open },
                    None => Section::Card(rows),
                });
                // Agent-to-agent messaging: who else's agents may write into this agent's
                // chat. My own agents always may; this card lists other people (D83's
                // People-card shape: avatar · name/@handle · last message · switch).
                body.push(Section::Title(
                    format!("Who may message {}", project.name).into(),
                ));
                body.push(Section::Card(message_grant_rows(
                    shell, &project, &theme, cx,
                )));
                body.push(Section::Footnote(
                    format!(
                        "Your own agents message {} freely — no approval. People listed here may have their agents message it too; those messages land in this chat and {} answers them. When {} needs your OK for a risky action, the card appears in this chat.",
                        project.name, project.name, project.name
                    )
                    .into(),
                ));
            } else {
                body.push(Section::Card(vec![kit::value_row(
                    "agent-model-row",
                    "Model",
                    model_text,
                    &theme,
                )
                .into_any_element()]));
            }
        }
    }

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
    // the Description row to edit. "Done" only while editing.
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
            tabs: &TABS,
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

/// The rows of "Who may message X": one per grant (person or agent of another owner) with a
/// switch, then "Give a person access…" → the people picker.
fn message_grant_rows(
    shell: &Shell,
    project: &Project,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<gpui::AnyElement> {
    let state = shell.state.read(cx);
    let mut rows: Vec<gpui::AnyElement> = Vec::new();
    for g in &project.message_grants {
        let person = crate::model::Person {
            id: g.id.clone(),
            handle: g.handle.clone().unwrap_or_else(|| g.id.clone()),
            name: g.name.clone(),
            avatar_url: g.avatar_url.clone(),
            ..Default::default()
        };
        let title = g
            .name
            .clone()
            .or_else(|| g.handle.clone().map(|h| format!("@{h}")))
            .unwrap_or_else(|| g.id.clone());
        let mut subtitle = match (&g.handle, g.kind.as_str()) {
            (Some(h), "person") => format!("@{h}"),
            (Some(h), _) => format!("@{h} · agent"),
            (None, k) => k.to_owned(),
        };
        if let Some(at) = g.last_at {
            subtitle = format!("{subtitle} · messaged {}", crate::model::ago(at));
        } else if !g.note.is_empty() {
            subtitle = format!("{subtitle} · {}", g.note);
        }
        let (pid, grantee, on) = (project.id.clone(), g.grantee.clone(), g.enabled);
        let avatar = if g.kind == "agent" {
            avatars::element(&g.id, &title, None, kit::AVATAR, theme).into_any_element()
        } else {
            crate::ui::people::avatar(state.avatar(&g.avatar_url), &person, kit::AVATAR, theme)
                .into_any_element()
        };
        let switch = div()
            .id(ElementId::Name(SharedString::from(format!(
                "message-grant-switch-{}",
                g.grantee
            ))))
            .cursor_pointer()
            .child(glass::switch(on, theme))
            .on_click(cx.listener({
                let (pid, grantee) = (pid.clone(), grantee.clone());
                move |this, _, _, cx| {
                    let (pid, grantee) = (pid.clone(), grantee.clone());
                    this.state
                        .update(cx, |s, cx| s.set_message_grant(pid, grantee, !on, cx));
                }
            }))
            .into_any_element();
        rows.push(
            kit::row(
                ElementId::Name(SharedString::from(format!("message-grant-{}", g.grantee))),
                Leading::Avatar(avatar),
                title,
                Some(subtitle.into()),
                None,
                Some(switch),
                false,
                theme,
            )
            // Right-click removes the row (the switch only pauses it).
            .on_mouse_down(gpui::MouseButton::Right, {
                cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let (pid, grantee) = (pid.clone(), grantee.clone());
                    this.state
                        .update(cx, |s, cx| s.remove_message_grant(pid, grantee, cx));
                })
            })
            .into_any_element(),
        );
    }
    let pid = project.id.clone();
    rows.push(
        kit::row(
            "message-grant-add",
            Leading::Glyph("person-plus"),
            "Give a person access…",
            None,
            None,
            None,
            false,
            theme,
        )
        .text_color(theme.accent)
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_2))
        .on_click(cx.listener(move |this, _, window, cx| {
            let id = pid.clone();
            this.state
                .update(cx, |s, cx| s.open_message_grant_picker(id, cx));
            let handle = this.people_input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }))
        .into_any_element(),
    );
    rows
}
