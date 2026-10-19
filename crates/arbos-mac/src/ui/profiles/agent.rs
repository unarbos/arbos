//! An agent's page: the coloured tree, name, what it is doing, call / voice
//! (soon) / mute / search / more, the info card (owner, machine, then — for
//! my own agent — Model / Reasoning / Voice rows that drop down on hover,
//! secrets, camera vision, description), an Appearance section holding the
//! tree + colour picker, then Media / Files / Links from its chats.

use gpui::{div, prelude::*, px, Context, Div, ElementId, Focusable, SharedString};

use crate::app::{AgentPick, CallTarget, ProfilePage};
use crate::model::{Id, MachineState, Person, Project};
use crate::ui::avatars::{self, AvatarState};
use crate::ui::glass;
use crate::ui::hoverpick;
use crate::ui::icons::icon;
use crate::ui::motion::PulseClock;
use crate::ui::shell::Shell;
use crate::ui::sidebar::machine_dot;
use crate::ui::theme::{Theme, TEXT_BODY, TEXT_SMALL, TEXT_TINY};

use super::{
    action_button, action_row, card, chats_of, collect, file_list, frame, hero, hero_name,
    info_row, link_list, media_grid, more_menu, tab_bar, Action,
};

const TABS: [&str; 3] = ["Media", "Files", "Links"];

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
            description: a.description.clone(),
            owner_person: a.owner.clone(),
            model: a.model.clone().unwrap_or_default(),
            shared: true,
            owner_handle: Some(a.owner_handle.clone()),
            ..Default::default()
        })
}

pub(super) fn page(project_id: &str, shell: &Shell, cx: &mut Context<Shell>) -> Option<Div> {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let (project, mine): (Project, bool) = match state.project(project_id) {
        Some(p) => (p.clone(), !p.is_guest()),
        None => (project_from_member(state, project_id)?, false),
    };
    let secrets_data = state.project_secrets.get(&project.id).cloned();
    let avatar_state = state.avatar_state(&project.id);
    let streaming = state.project_is_streaming(&project.id);
    let voice_text = crate::ui::voicepicker::current_label(&project, &state.voices);
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
    let machine_class = project
        .machine_info
        .as_ref()
        .map(|m| m.class.clone())
        .filter(|c| !c.is_empty() && c != "none")
        .or_else(|| state.machine.class.clone());
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
    let owner: Option<Person> = project
        .owner_person
        .clone()
        .or_else(|| project.owner().cloned());
    let owner_text = match (&owner, mine) {
        (Some(o), _) => format!(
            "@{}{}",
            o.handle,
            if state.is_online(o) { " · online" } else { "" }
        ),
        (None, true) => format!("@{} (you)", state.me_handle()),
        (None, false) => project
            .owner_handle
            .clone()
            .map(|h| format!("@{h}"))
            .unwrap_or_else(|| "—".into()),
    };
    let model_text = if project.model.is_empty() {
        "default".to_string()
    } else {
        project.model.clone()
    };
    let machine_text = {
        let mut s = machine_state.label().to_string();
        if always_on {
            s.push_str(" · always on");
        }
        if let Some(c) = &machine_class {
            s.push_str(&format!(" · {c}"));
        }
        s
    };
    let tab = state.profile_tab;
    let more_open = state.profile_more_open;
    let picker_open = state.profile_picker_open;
    let pick = state.agent_pick;
    let editing = state.profile_editing_description && mine;
    let renaming = state.profile_editing_name && mine;
    let muted = state.is_muted(&project.id);
    let content = collect(state, &chats_of(state, &project.id));
    let pid: Id = project.id.clone();

    if avatar_state != AvatarState::Idle {
        PulseClock::request(cx);
    }

    // ----- hero ----------------------------------------------------------------
    let avatar = div()
        .relative()
        .size(px(112.0))
        .child(avatars::project(&project, 112.0, &theme))
        .child(
            div()
                .absolute()
                .right(px(0.0))
                .bottom(px(0.0))
                .size(px(30.0))
                .rounded_full()
                .bg(theme.card())
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .size(px(20.0))
                        .rounded_full()
                        .bg(machine_dot(machine_state, &theme)),
                ),
        )
        .into_any_element();
    let name_el = if renaming {
        div()
            .id("profile-rename")
            .w(px(220.0))
            .h(px(36.0))
            .px(px(10.0))
            .rounded(px(10.0))
            .bg(theme.field)
            .border_1()
            .border_color(theme.field_border)
            .text_size(px(TEXT_BODY + 8.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(theme.text)
            .child(shell.agent_name_input.clone())
            .into_any_element()
    } else if mine {
        let p_rename = pid.clone();
        hero_name(&project.name, &theme)
            .id("profile-name")
            .cursor_pointer()
            .hover(|s| s.opacity(0.85))
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_edit_agent(p_rename.clone(), false, window, cx);
                    }
                }),
            )
            .into_any_element()
    } else {
        hero_name(&project.name, &theme).into_any_element()
    };
    let hero = hero(avatar, name_el, &status, status_color, &theme).child(
        div()
            .text_size(px(TEXT_SMALL))
            .text_color(theme.text_faint)
            .child(format!("@{}", Project::mention_slug(&project.name))),
    );

    // ----- actions --------------------------------------------------------------
    let p_call = pid.clone();
    let p_voice = pid.clone();
    let p_mute = pid.clone();
    let name_q = project.name.clone();
    let actions = action_row(vec![
        action_button(&Action::new("profile-call", "phone", "call"), &theme).on_click(cx.listener(
            move |this, _, _, cx| {
                let p = p_call.clone();
                this.state
                    .update(cx, |s, cx| s.start_call(CallTarget::Agent(p), false, cx));
            },
        )),
        action_button(&Action::new("profile-voice", "waveform", "voice"), &theme).on_click(
            cx.listener(move |this, _, _, cx| {
                let p = p_voice.clone();
                this.state
                    .update(cx, |s, cx| s.start_call(CallTarget::Agent(p), false, cx));
            }),
        ),
        action_button(
            &Action::new(
                "profile-mute",
                if muted { "bell-slash" } else { "bell" },
                if muted { "unmute" } else { "mute" },
            ),
            &theme,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            let p = p_mute.clone();
            this.state.update(cx, |s, cx| s.toggle_mute(p, cx));
        })),
        action_button(&Action::new("profile-search", "search", "search"), &theme).on_click(
            cx.listener(move |this, _, window, cx| {
                let q = name_q.clone();
                this.state.update(cx, |s, cx| {
                    s.close_profile(cx);
                });
                this.search_input
                    .update(cx, |input, cx| input.set_text(q, cx));
                let handle = this.search_input.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }),
        ),
        action_button(&Action::new("profile-more", "ellipsis", "more"), &theme).on_click(
            cx.listener(|this, _, _, cx| {
                this.state.update(cx, |s, cx| s.toggle_profile_more(cx));
            }),
        ),
    ]);

    let more = more_open.then(|| {
        let p_open = pid.clone();
        let mut rows = vec![glass::menu_row(
            "profile-more-open",
            "chevron-right",
            "Open chat",
            false,
            &theme,
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            let p = p_open.clone();
            this.state.update(cx, |s, cx| {
                s.close_profile(cx);
                s.select_project(p, cx);
            });
            crate::ui::composer::Composer::focus(&this.composer, window, cx);
        }))];
        if project.is_groupish() {
            let p_group = pid.clone();
            rows.push(
                glass::menu_row("profile-more-group", "people", "Members", false, &theme).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let p = p_group.clone();
                        this.state.update(cx, |s, cx| {
                            s.open_profile(ProfilePage::Group { space_id: p }, cx)
                        });
                    }),
                ),
            );
        }
        if let Some(o) = owner.as_ref().filter(|_| !mine) {
            let h = o.handle.clone();
            rows.push(
                glass::menu_row(
                    "profile-more-owner",
                    "person",
                    "Owner's profile",
                    false,
                    &theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let h = h.clone();
                    this.state.update(cx, |s, cx| {
                        s.open_profile(
                            ProfilePage::User {
                                handle: h,
                                space_id: None,
                            },
                            cx,
                        )
                    });
                })),
            );
        }
        more_menu(rows, &theme)
    });

    // ----- info card -------------------------------------------------------------
    let mut info = card(&theme)
        .child(info_row("owner", owner_text, &theme))
        .child(info_row("machine", machine_text, &theme));
    if mine {
        // The agent's own settings: Model (searchable), Reasoning and Voice
        // drop down on hover (or a click) and apply at once.
        let model_open = pick == Some(AgentPick::Model);
        let reasoning_open = pick == Some(AgentPick::Reasoning);
        let voice_open = pick == Some(AgentPick::Voice);
        info = info
            .child(hoverpick::row(
                "model",
                model_text,
                AgentPick::Model,
                model_open,
                model_open.then(|| hoverpick::model_panel(shell, &project, cx)),
                &theme,
                cx,
            ))
            .child(hoverpick::row(
                "reasoning",
                project.effort.label(),
                AgentPick::Reasoning,
                reasoning_open,
                reasoning_open.then(|| hoverpick::reasoning_panel(&project, cx)),
                &theme,
                cx,
            ))
            .child(hoverpick::row(
                "voice",
                voice_text,
                AgentPick::Voice,
                voice_open,
                voice_open.then(|| hoverpick::voice_panel(shell, &project, cx)),
                &theme,
                cx,
            ));
    } else {
        info = info.child(info_row("model", model_text, &theme));
    }
    // D83: what this agent may use; opens the agent's Secrets page.
    let secrets_text = match secrets_data.as_ref() {
        Some(d) => {
            let on: Vec<String> = d
                .items
                .iter()
                .filter(|i| i.enabled)
                .map(|i| i.title())
                .collect();
            if on.is_empty() {
                if d.requests.iter().any(|r| r.is_pending()) {
                    "waiting for you".to_string()
                } else {
                    "none".to_string()
                }
            } else {
                format!("{} · {}", on.len(), on.join(", "))
            }
        }
        None => "—".to_string(),
    };
    let p_secrets = pid.clone();
    info = info.child(
        div()
            .id("agent-secrets-row")
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(16.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(theme.border)
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_2))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(TEXT_TINY))
                            .text_color(theme.text_muted)
                            .child("secrets"),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_BODY))
                            .text_color(theme.text)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(secrets_text),
                    ),
            )
            .child(icon("chevron-right", 14.0, theme.text_faint))
            .on_click(cx.listener(move |this, _, _, cx| {
                let pid = p_secrets.clone();
                this.secrets.update(cx, |p, cx| p.show_agent(pid, cx));
                cx.notify();
            })),
    );
    if mine {
        // Camera vision (voice API §11): a switch row, default on.
        let vision_on = project.vision_enabled;
        let p_vision = pid.clone();
        info = info.child(
            div()
                .id("agent-vision-row")
                .flex()
                .items_center()
                .gap(px(10.0))
                .px(px(16.0))
                .py(px(10.0))
                .border_b_1()
                .border_color(theme.border)
                .cursor_pointer()
                .hover(|s| s.bg(theme.surface_2))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(TEXT_TINY))
                                .text_color(theme.text_muted)
                                .child("camera vision"),
                        )
                        .child(div().text_size(px(TEXT_BODY)).text_color(theme.text).child(
                            if vision_on {
                                "On — the agent can look through your camera on calls"
                            } else {
                                "Off"
                            },
                        )),
                )
                .child(glass::switch(vision_on, &theme))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let p = p_vision.clone();
                    this.state
                        .update(cx, |s, cx| s.set_vision_enabled(p, !vision_on, cx));
                })),
        );
    }
    let description = project.description.clone().filter(|d| !d.trim().is_empty());
    if editing {
        // Inline editor: the owner types, Save → PATCH {description}.
        let p_save = pid.clone();
        info = info.child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .px(px(16.0))
                .py(px(10.0))
                .child(
                    div()
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.text_muted)
                        .child("about"),
                )
                .child(
                    div()
                        .min_h(px(60.0))
                        .px(px(10.0))
                        .py(px(8.0))
                        .rounded(px(10.0))
                        .bg(theme.field)
                        .border_1()
                        .border_color(theme.field_border)
                        .text_size(px(TEXT_BODY))
                        .child(shell.description_input.clone()),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.0))
                        .child(
                            glass::quiet_button("profile-desc-cancel", "Cancel", &theme)
                                .h(px(32.0))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |s, cx| {
                                        s.set_profile_editing_description(false, cx)
                                    });
                                })),
                        )
                        .child(
                            glass::primary_button("profile-desc-save", "Save", &theme)
                                .h(px(32.0))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let text = this.description_input.read(cx).text().to_string();
                                    let p = p_save.clone();
                                    this.state
                                        .update(cx, |s, cx| s.set_description(p, text, cx));
                                })),
                        ),
                ),
        );
    } else {
        let row = info_row(
            "about",
            description.clone().unwrap_or_else(|| {
                if mine {
                    "Add a description".into()
                } else {
                    "—".into()
                }
            }),
            &theme,
        );
        if mine {
            let current = description.clone().unwrap_or_default();
            info = info.child(
                div()
                    .id("profile-desc-edit")
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.surface_2))
                    .child(row)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let current = current.clone();
                        this.description_input
                            .update(cx, |input, cx| input.set_text(current, cx));
                        this.state
                            .update(cx, |s, cx| s.set_profile_editing_description(true, cx));
                        let handle = this.description_input.read(cx).focus_handle(cx);
                        window.focus(&handle, cx);
                    })),
            );
        } else {
            info = info.child(row);
        }
    }

    // ----- appearance (mine only): Automatic + the seven gradients, folded ----
    let appearance = mine.then(|| {
        let current_index = avatars::index_for(&project.id, project.avatar_color_override);
        let label = match project.avatar_color_override {
            None => format!("Automatic · {}", avatars::GRADIENTS[current_index as usize].name),
            Some(i) => avatars::GRADIENTS[(i as usize) % 7].name.to_string(),
        };
        let mut section = card(&theme).child(
            glass::nav_row(
                "profile-appearance",
                "Avatar colour",
                Some(if picker_open { "pick".into() } else { label }),
                &theme,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.state.update(cx, |s, cx| s.toggle_profile_picker(cx));
            })),
        );
        if picker_open {
            section = section
                .child(colour_row(&project, &theme, cx))
                .child(
                    div()
                        .px(px(16.0))
                        .pb(px(12.0))
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.text_muted)
                        .child("Automatic picks the colour from the agent's id, the same on every device; choose one to make it yours."),
                );
        }
        section
    });

    // ----- tabs -----------------------------------------------------------------
    let tabs = tab_bar(&TABS, tab, &theme, cx);
    let body_tab = match tab {
        0 => media_grid(&content.media, &theme),
        1 => file_list(&content.files, "doc", "No files shared yet", &theme),
        _ => link_list(&content.links, &theme, cx),
    };

    let body = div()
        .flex()
        .flex_col()
        .pb(px(16.0))
        .child(hero)
        .child(actions)
        .children(more)
        // Avatar colour sits right under the big avatar it changes.
        .children(appearance)
        .child(info)
        .child(tabs)
        .child(body_tab);
    // "Edit" turns the card into its editor, Contacts-style: the name and the
    // description become fields and the button reads "Done" until they are saved.
    let edit = mine.then(|| {
        if renaming || editing {
            glass::sheet_button("profile-done", "Done", &theme)
                .on_click(cx.listener(|this, _, window, cx| this.finish_edit_agent(window, cx)))
        } else {
            let p = pid.clone();
            glass::sheet_button("profile-edit", "Edit", &theme).on_click(cx.listener(
                move |this, _, window, cx| this.start_edit_agent(p.clone(), true, window, cx),
            ))
        }
    });
    Some(frame(body, edit, &theme, cx))
}

fn colour_row(project: &Project, theme: &Theme, cx: &mut Context<Shell>) -> Div {
    let mut colours = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(10.0))
        .px(px(16.0))
        .pb(px(14.0));
    let auto_index = avatars::color_index(&project.id);
    let override_index = project.avatar_color_override;
    let ring = |selected: bool, theme: &Theme| {
        div()
            .size(px(52.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .border_2()
            .border_color(if selected {
                theme.text
            } else {
                gpui::transparent_black()
            })
            .cursor_pointer()
            .hover(|st| st.border_color(theme.border_strong))
    };
    // "Automatic": the id-derived gradient with the agent's initials.
    {
        let pid = project.id.clone();
        colours = colours.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.0))
                .child(
                    ring(override_index.is_none(), theme)
                        .id("avatar-color-auto")
                        .child(avatars::element(
                            &project.id,
                            &project.name,
                            None,
                            44.0,
                            theme,
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let p = pid.clone();
                            this.state
                                .update(cx, |st, cx| st.set_avatar_color_index(p, None, cx));
                        })),
                )
                .child(
                    div()
                        .text_size(px(TEXT_TINY))
                        .text_color(theme.text_muted)
                        .child("Automatic"),
                ),
        );
    }
    for (i, g) in avatars::GRADIENTS.iter().enumerate() {
        let index = i as u8;
        let selected = override_index == Some(index);
        let pid = project.id.clone();
        colours = colours.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.0))
                .child(
                    ring(selected, theme)
                        .id(ElementId::Name(SharedString::from(format!(
                            "avatar-color-{index}"
                        ))))
                        .child(avatars::swatch(index, 44.0, theme))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let p = pid.clone();
                            this.state
                                .update(cx, |st, cx| st.set_avatar_color_index(p, Some(index), cx));
                        })),
                )
                .child(
                    div()
                        .text_size(px(TEXT_TINY))
                        .text_color(if index == auto_index && override_index.is_none() {
                            theme.text
                        } else {
                            theme.text_muted
                        })
                        .child(g.name),
                ),
        );
    }
    colours
}
