//! People chats (D72) in the shell: avatars, the New Message sheet, the
//! member chips under a DM header and its pickers, the typing line. Pure
//! rendering over `AppState`; the shell owns the open/closed flags.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, AnyElement, App, Context, Div, ElementId, Entity, Focusable as _,
    RenderImage, SharedString, Stateful, Window,
};

use crate::app::AppState;
use crate::model::{AgentMember, Person, Project};
use crate::ui::avatars;
use crate::ui::composer::ComposerInput;
use crate::ui::error_note;
use crate::ui::frost::{frosted, MENU_BLUR};
use crate::ui::glass;
use crate::ui::highlight::{self, Shape};
use crate::ui::icons::icon;
use crate::ui::identity;
use crate::ui::profiles::kit;
use crate::ui::shell::Shell;
use crate::ui::theme::{Theme, PANEL_RADIUS, TEXT_BODY, TEXT_SMALL, TEXT_TINY};

/// The New Message / New group search field: side inset equal to the row
/// content below it (cards 16 + row padding 14), and Grok Mac's ~14 pt
/// placeholder (ref 57: 21 px ascender band at 2 px/pt).
pub const SEARCH_INSET: f32 = CARD_INSET + ROW_PADDING_X;
pub const SEARCH_FONT_SIZE: f32 = 14.0;

/// One row geometry for the sheet's cards — the New agent / New group / New
/// server rows and the people rows under them share it, so glyphs, pictures
/// and text have one left edge and the sheet's first paint is its final layout.
pub const CARD_INSET: f32 = 16.0;
pub const ROW_PADDING_X: f32 = 14.0;
pub const ROW_GLYPH: f32 = 40.0;
pub const ROW_GAP: f32 = identity::ROW_GAP;

/// Members of a space with their decoded avatars (list rows, headers).
pub type PeopleWithAvatars = Vec<(Person, Option<Arc<RenderImage>>)>;

/// Round avatar: the GitHub picture when decoded, else an initial on a stable colour.
pub fn avatar(image: Option<Arc<RenderImage>>, person: &Person, size: f32, theme: &Theme) -> Div {
    let base = div()
        .size(px(size))
        .flex_shrink_0()
        .rounded_full()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center();
    match image {
        Some(image) => base.child(
            gpui::img(crate::ui::picture_fit::fit(&image, size))
                .size(px(size))
                .rounded_full()
                .object_fit(gpui::ObjectFit::Cover),
        ),
        // D79: Telegram-style initials on the id's gradient.
        None => crate::ui::avatars::person(person, size, theme),
    }
}

/// A shared agent's avatar, through the resolver (its picture, else its leaf).
pub fn agent_glyph(
    state: &AppState,
    chat: Option<&Project>,
    agent: &AgentMember,
    size: f32,
    theme: &Theme,
) -> Div {
    crate::ui::avatars::member_picture(state, chat, agent, size, theme)
}

/// The "New Message" sheet (iPhone / Telegram layout): Cancel · title, a
/// "Search people" field, New agent / New group rows, then everyone on Arbos.
/// A person row starts the DM — or, when the sheet was opened from a project's
/// "Share", invites them into it. New group opens a multi-select picker.
pub fn new_chat_sheet(
    state: &Entity<AppState>,
    input: &Entity<ComposerInput>,
    cx: &mut Context<Shell>,
) -> Div {
    let theme = Theme::of(cx).clone();
    let s = state.read(cx);
    let query = s.people_query.clone();
    let busy = s.people_busy;
    let error = s.people_error.clone();
    let group_mode = s.new_group_open;
    // Opened from a group or server pane's "Add members": same picker, members of
    // that group / server shown as added, and Add puts the picks into it.
    let add_to: Option<crate::app::AddTarget> = s.add_members_target_info();
    let me_handle = s.me_handle();
    let picked = s.new_group_picked.clone();
    let picked_agents = s.new_group_agents.clone();
    let results: Vec<(Person, Option<Arc<RenderImage>>)> = s
        .people_results
        .iter()
        .map(|p| (p.clone(), s.avatar(&p.avatar_url)))
        .collect();
    let typed = query.trim().trim_start_matches('@').to_string();
    let invite_into = s
        .invite_target
        .as_ref()
        .and_then(|id| s.projects.iter().find(|p| &p.id == id))
        .map(|p| p.name.clone());
    let servers_available = s.servers_available;
    // Servers hold people only: no agent rows in their picker.
    let my_agents: Vec<Project> = if group_mode && !add_to.as_ref().is_some_and(|t| t.is_server) {
        agent_picker_rows(s.my_projects().cloned().collect(), &query)
    } else {
        Vec::new()
    };
    // Add members on a group: the Agents section also lists the other members'
    // agents already in it (greyed, already in — only their owner adds or
    // removes them), after mine, so the section is the chat's whole agent
    // roster ([feedback add-your-agent-row-removal-mac-2026-10-27]).
    let others_agents: Vec<OtherAgentRow> = s
        .add_members_group()
        .map(|group| {
            others_agents_in(group, &me_handle, &typed)
                .into_iter()
                .map(|a| {
                    let ident = avatars::member_identity(Some(group), &s.projects, &a);
                    let image = ident.image_key.as_deref().and_then(|url| s.avatar(url));
                    (a, ident, image)
                })
                .collect()
        })
        .unwrap_or_default();
    let agent_names: std::collections::HashMap<String, String> = s
        .my_projects()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect();

    // Grok Mac's search field, measured on the real app (ref 57, 2 px/pt):
    // 32 pt tall including a 0.5 pt rim, 8 pt corners, tinted fill, an
    // 11.5 pt magnifier 9 pt in, 6.5 pt to the 14 pt placeholder. Its edges
    // line up with the row content below (cards 16 + row padding 14), per
    // Jacob. `flex_shrink_0`: inside the scrolling column the field used to
    // be squeezed to ~21 pt once the people list was long. The
    // ComposerInput's line is centred explicitly — without items_center on
    // the flex_1 wrapper the caret sits high.
    let search = div()
        .mx(px(SEARCH_INSET))
        .h(px(32.0))
        .flex_shrink_0()
        .pl(px(8.5))
        .pr(px(10.0))
        .flex()
        .items_center()
        .gap(px(6.5))
        .rounded(px(8.0))
        .bg(theme.surface_2)
        .border(px(0.5))
        .border_color(theme.field_border)
        .cursor_text()
        .child(icon("search", 11.5, theme.text_muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .child(input.clone()),
        );

    let mut body = div()
        .id("new-chat-body")
        .flex()
        .flex_col()
        .gap(px(14.0))
        .pb(px(18.0))
        .min_h_0()
        .overflow_y_scroll();
    // A refusal (no such user, nobody picked, the relay said no) is the shared error note at
    // the top of the body, before the search field — one place for every people sheet.
    if let Some(slot) = error.as_ref() {
        body = body.child(div().mx(px(CARD_INSET)).child(error_note::render(
            "people-sheet-error",
            &slot.error,
            slot.details_open,
            error_note::Style::Inline,
            &theme,
            error_note::Handlers::new().toggle_details(cx.listener(|this, _, _, cx| {
                this.state.update(cx, |s, cx| {
                    if let Some(slot) = s.people_error.as_mut() {
                        slot.toggle_details();
                    }
                    cx.notify();
                });
            })),
        )));
    }

    if group_mode {
        body = new_group_body(
            body,
            search,
            &theme,
            &picked,
            &picked_agents,
            &agent_names,
            &my_agents,
            &others_agents,
            &results,
            &typed,
            busy,
            add_to.as_ref(),
            &me_handle,
            state,
            cx,
        );
    } else {
        body = body.child(search);
        // New agent + New group (iPhone / Grok layout).
        if invite_into.is_none() {
            let agent_row = div()
                .id("new-agent")
                .flex()
                .items_center()
                .gap(px(ROW_GAP))
                .px(px(ROW_PADDING_X))
                .py(px(10.0))
                .cursor_pointer()
                .map(|el| highlight::row(el, false, Shape::Nested(PANEL_RADIUS), &theme))
                .child(
                    glass::circle("new-agent-glyph", ROW_GLYPH, &theme)
                        .child(icon("sparkles", 17.0, theme.text)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(TEXT_BODY))
                                .text_color(theme.text)
                                .child("New agent"),
                        )
                        .child(
                            div()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.text_muted)
                                .child("Its own workspace on your machine"),
                        ),
                )
                .child(icon("chevron-right", 13.0, theme.text_faint))
                // D93: opens the Create screen (`create_entity::sheet`) in place of this one.
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_create_agent(cx);
                    let handle = this.project_input.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }));
            let group_row = div()
                .id("new-group")
                .flex()
                .items_center()
                .gap(px(ROW_GAP))
                .px(px(ROW_PADDING_X))
                .py(px(10.0))
                .cursor_pointer()
                .map(|el| highlight::row(el, false, Shape::Nested(PANEL_RADIUS), &theme))
                .child(
                    glass::circle("new-group-glyph", ROW_GLYPH, &theme)
                        .child(icon("people", 17.0, theme.text)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(TEXT_BODY))
                                .text_color(theme.text)
                                .child("New group"),
                        )
                        .child(
                            div()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.text_muted)
                                .child("People and agents in one chat"),
                        ),
                )
                .child(icon("chevron-right", 13.0, theme.text_faint))
                // D93: step 1 is the Create screen (picture + name); the picker below is step 2.
                .on_click(cx.listener(|this, _, window, cx| this.open_create_group(window, cx)));
            // Inside the scrolling body the card must keep its height, or the
            // expanded form is squeezed and clipped once the list below is long.
            let mut group = glass::group(&theme)
                .flex_shrink_0()
                .mx(px(CARD_INSET))
                .child(agent_row);
            group = group.child(kit::divider(&theme)).child(group_row);
            // D88 rule 8: servers, when this relay runs them.
            if servers_available {
                let server_row = |id: &'static str,
                                  glyph: &'static str,
                                  title: &'static str,
                                  line: &'static str,
                                  sheet: crate::app::ServerSheet| {
                    div()
                        .id(id)
                        .flex()
                        .items_center()
                        .gap(px(ROW_GAP))
                        .px(px(ROW_PADDING_X))
                        .py(px(10.0))
                        .cursor_pointer()
                        .map(|el| highlight::row(el, false, Shape::Nested(PANEL_RADIUS), &theme))
                        .child(
                            glass::circle(
                                ElementId::Name(SharedString::from(format!("{id}-glyph"))),
                                ROW_GLYPH,
                                &theme,
                            )
                            .child(icon(glyph, 17.0, theme.text)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(px(TEXT_BODY))
                                        .text_color(theme.text)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(px(TEXT_SMALL))
                                        .text_color(theme.text_muted)
                                        .child(line),
                                ),
                        )
                        .child(icon("chevron-right", 13.0, theme.text_faint))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_server_sheet(sheet.clone(), window, cx);
                        }))
                };
                group = group.child(kit::divider(&theme)).child(server_row(
                    "new-server",
                    "hash",
                    "New server",
                    "Channels for a crowd, with an invite link",
                    crate::app::ServerSheet::NewServer,
                ));
                // "Join a server" (paste a link) is gone (Jacob 2026-10-22): invite links
                // open the app through arbos.life/s/<code>; people are added by name.
            }
            body = body.child(group);
        }

        // People.
        let mut list = glass::group(&theme).mx(px(CARD_INSET));
        let mut any = false;
        if results.is_empty() && !typed.is_empty() && !busy {
            any = true;
            list = list.child(
                div()
                    .px(px(14.0))
                    .py(px(10.0))
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_muted)
                    .child(format!(
                        "No one on Arbos matches @{typed} yet. Enter sends an invite by GitHub login."
                    )),
            );
        }
        if busy && results.is_empty() {
            any = true;
            list = list.child(
                div()
                    .px(px(14.0))
                    .py(px(10.0))
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_faint)
                    .child("Searching…"),
            );
        }
        let count = results.len();
        for (i, (person, image)) in results.into_iter().enumerate() {
            any = true;
            let handle = person.handle.clone();
            let invited = person.status == "invited";
            list = list.child(
                div()
                    .id(ElementId::Name(SharedString::from(format!(
                        "person-{}",
                        person.id
                    ))))
                    .flex()
                    .items_center()
                    .gap(px(ROW_GAP))
                    .px(px(ROW_PADDING_X))
                    .py(px(9.0))
                    .relative()
                    .when(i + 1 < count, |el| el.child(kit::hairline_below(&theme)))
                    .cursor_pointer()
                    .map(|el| highlight::row(el, false, Shape::Nested(PANEL_RADIUS), &theme))
                    .child(avatar(image, &person, ROW_GLYPH, &theme))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(TEXT_BODY))
                                    .text_color(theme.text)
                                    .truncate()
                                    .child(person.display_name()),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_SMALL))
                                    .text_color(theme.text_muted)
                                    .child(format!("@{}", person.handle)),
                            ),
                    )
                    .child(if invited {
                        glass::chip("Invite by GitHub", theme.text_muted)
                    } else {
                        glass::chip("On Arbos", theme.online)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state
                            .update(cx, |s, cx| s.pick_person(handle.clone(), cx));
                    })),
            );
        }
        if any {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        glass::section_label(
                            if typed.is_empty() {
                                "Arbos users"
                            } else {
                                "Results"
                            },
                            &theme,
                        )
                        .px(px(30.0)),
                    )
                    .child(list),
            );
        }
        if let Some(name) = &invite_into {
            body = body.child(
                div()
                    .px(px(30.0))
                    .text_size(px(TEXT_TINY))
                    .text_color(theme.text_faint)
                    .child(format!(
                        "They see every chat and the workspace output of {name}; your agent answers them too."
                    )),
            );
        }
    }

    let title = match (&add_to, group_mode, &invite_into) {
        (Some(target), _, _) if target.is_secret => "Give a person access".to_string(),
        (Some(target), _, _) => format!("Add members to {}", target.name),
        (None, true, _) => "New group".to_string(),
        (None, false, Some(name)) => format!("Invite to {name}"),
        (None, false, None) => "New Message".to_string(),
    };
    // People, agents, or both — a group of your agents is a group (Jacob,
    // 2026-10-22: Create greyed out with only agents picked read as broken).
    let can_create = (!picked.is_empty() || !picked_agents.is_empty()) && !busy;
    let can_add = can_create;
    let left = if add_to.is_some() {
        // No New Message sheet behind this one: Cancel closes it.
        glass::sheet_button("add-members-cancel", "Cancel", &theme)
            .on_click(cx.listener(|this, _, window, cx| this.close_new_chat_sheet(window, cx)))
    } else if group_mode {
        glass::sheet_button("new-group-back", "Cancel", &theme).on_click(cx.listener(
            |this, _, window, cx| {
                this.people_input
                    .update(cx, |input, _| input.set_placeholder("Search people"));
                this.state.update(cx, |s, cx| s.close_new_group(cx));
                let handle = this.people_input.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            },
        ))
    } else {
        glass::sheet_button("new-chat-close", "Cancel", &theme)
            .on_click(cx.listener(|this, _, window, cx| this.close_new_chat_sheet(window, cx)))
    };
    let right = if add_to.is_some() {
        Some(
            glass::primary_button("add-members-go", "Add", &theme)
                .h(px(34.0))
                .when(!can_add, |el| el.opacity(0.45))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !can_add {
                        return;
                    }
                    let sent = this.state.update(cx, |s, cx| s.add_members_to_group(cx));
                    if sent {
                        this.close_new_chat_sheet(window, cx);
                    }
                })),
        )
    } else if group_mode {
        Some(
            glass::primary_button(
                "new-group-create",
                if busy { "Creating…" } else { "Create" },
                &theme,
            )
            .h(px(34.0))
            .when(!can_create, |el| el.opacity(0.45))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !can_create {
                    return;
                }
                this.state.update(cx, |s, cx| s.create_people_group(cx));
            })),
        )
    } else {
        None
    };
    let header = glass::sheet_header(&title, Some(left), right, &theme);
    // Cancel and a click on the dimmed area around the card both close the
    // sheet (including while New agent / New group is open); clicks inside stay.
    glass::sheet(460.0, &theme, header, body.max_h(px(620.0))).on_mouse_down(
        gpui::MouseButton::Left,
        cx.listener(|this, _, window, cx| this.close_new_chat_sheet(window, cx)),
    )
}

/// Avatar size inside a selected-member chip.
const CHIP_AVATAR: f32 = 22.0;

/// One selected member in a picker (New group, Add members): the member's
/// picture, `@handle`, and an × that removes it. People and agents share it,
/// so an agent chip carries the agent's avatar exactly like a person chip.
pub fn member_chip(
    id: String,
    picture: AnyElement,
    label: String,
    theme: &Theme,
    on_remove: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let close_id = format!("{id}-x");
    div()
        .id(ElementId::Name(SharedString::from(id)))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(32.0))
        .pl(px(4.0))
        .pr(px(8.0))
        .rounded_full()
        .bg(theme.field)
        .child(picture)
        .child(
            div()
                .text_size(px(TEXT_TINY))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(label),
        )
        .child(
            div()
                .id(ElementId::Name(SharedString::from(close_id)))
                .size(px(16.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(icon("xmark", 8.0, theme.text_muted))
                .on_click(on_remove),
        )
}

/// New group picker body: chips, search, your agents, people with checkmarks.
/// With `add_to` (the group pane's Add members) its members show a greyed
/// check and take no click.
#[allow(clippy::too_many_arguments)]
fn new_group_body(
    mut body: Stateful<Div>,
    search: Div,
    theme: &Theme,
    picked: &[Person],
    picked_agents: &[String],
    agent_names: &std::collections::HashMap<String, String>,
    my_agents: &[Project],
    others_agents: &[OtherAgentRow],
    results: &[(Person, Option<Arc<RenderImage>>)],
    typed: &str,
    busy: bool,
    add_to: Option<&crate::app::AddTarget>,
    _me_handle: &str,
    state: &Entity<AppState>,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    if !picked.is_empty() || !picked_agents.is_empty() {
        let mut chips = div()
            .id("new-group-chips")
            .mx(px(16.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .overflow_x_scroll();
        let projects = state.read(cx).projects.clone();
        let images = avatars::project_images(state.read(cx));
        for pid in picked_agents {
            let id = pid.clone();
            let label = agent_names
                .get(pid)
                .map(|n| format!("@{}", Project::mention_slug(n)))
                .unwrap_or_else(|| "@agent".into());
            // The agent's own picture (photo, else its D79 initial disc) —
            // the same one its list row shows.
            let picture = projects
                .iter()
                .find(|p| &p.id == pid)
                .map(|p| {
                    avatars::project_with(images.get(&p.id).cloned(), p, CHIP_AVATAR, theme)
                        .into_any_element()
                })
                .unwrap_or_else(|| div().size(px(CHIP_AVATAR)).into_any_element());
            chips = chips.child(member_chip(
                format!("group-chip-agent-{pid}"),
                picture,
                label,
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.state
                        .update(cx, |s, cx| s.toggle_group_agent(id.clone(), cx));
                }),
            ));
        }
        for person in picked {
            let image = state.read(cx).avatar(&person.avatar_url);
            let person_clone = person.clone();
            chips = chips.child(member_chip(
                format!("group-chip-{}", person.id),
                avatar(image, person, CHIP_AVATAR, theme).into_any_element(),
                format!("@{}", person.handle),
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.state
                        .update(cx, |s, cx| s.toggle_group_person(person_clone.clone(), cx));
                }),
            ));
        }
        body = body.child(chips);
    }
    body = body.child(search);

    let agents_shown = !my_agents.is_empty() || !others_agents.is_empty();
    if agents_shown {
        let count = my_agents.len() + others_agents.len();
        let images = avatars::project_images(state.read(cx));
        let mut list = glass::group(theme).mx(px(16.0));
        for (i, project) in my_agents.iter().enumerate() {
            let pid = project.id.clone();
            let already = add_to.is_some_and(|g| g.agents.iter().any(|a| a == &project.id));
            let selected = already || picked_agents.iter().any(|id| id == &project.id);
            list = list.child(
                div()
                    .id(ElementId::Name(SharedString::from(format!(
                        "group-agent-{}",
                        project.id
                    ))))
                    .debug_selector(|| format!("group-agent-{pid}"))
                    .flex()
                    .items_center()
                    .gap(px(identity::ROW_GAP))
                    .px(px(14.0))
                    .py(px(9.0))
                    .relative()
                    .when(i + 1 < count, |el| el.child(kit::hairline_below(theme)))
                    .when(already, |el| el.opacity(0.5))
                    .map(|el| highlight::row(el, selected, Shape::Nested(PANEL_RADIUS), theme))
                    .when(!already, |el| el.cursor_pointer())
                    .child(avatars::project_with(
                        images.get(&project.id).cloned(),
                        project,
                        40.0,
                        theme,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(TEXT_BODY))
                                    .text_color(theme.text)
                                    .truncate()
                                    .child(project.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_SMALL))
                                    .text_color(theme.text_muted)
                                    .child(format!("@{}", Project::mention_slug(&project.name))),
                            ),
                    )
                    .child(pick_mark(selected, theme))
                    .when(!already, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| {
                            this.state
                                .update(cx, |s, cx| s.toggle_group_agent(pid.clone(), cx));
                        }))
                    }),
            );
        }
        for (i, (agent, ident, image)) in others_agents.iter().enumerate() {
            let last = i + 1 == others_agents.len();
            let selector = format!("group-agent-{}", agent.project_id);
            list = list.child(
                div()
                    .id(ElementId::Name(SharedString::from(selector.clone())))
                    .debug_selector(move || selector)
                    .flex()
                    .items_center()
                    .gap(px(identity::ROW_GAP))
                    .px(px(14.0))
                    .py(px(9.0))
                    .relative()
                    .when(!last, |el| el.child(kit::hairline_below(theme)))
                    .opacity(0.5)
                    .child(avatars::agent_picture(ident, image.clone(), 40.0, theme))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(TEXT_BODY))
                                    .text_color(theme.text)
                                    .truncate()
                                    .child(agent.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_SMALL))
                                    .text_color(theme.text_muted)
                                    .child(format!(
                                        "@{} · run by @{}",
                                        agent.handle, agent.owner_handle
                                    )),
                            ),
                    )
                    .child(pick_mark(true, theme)),
            );
        }
        body = body.child(
            div()
                .flex()
                .flex_col()
                .child(
                    glass::section_label(AGENTS_SECTION_LABEL, theme)
                        .debug_selector(|| AGENTS_SECTION_ID.into())
                        .px(px(30.0)),
                )
                .child(list),
        );
    }

    let mut list = glass::group(theme).mx(px(16.0));
    let mut any = false;
    if results.is_empty() && !busy {
        any = true;
        list = list.child(
            div()
                .px(px(14.0))
                .py(px(10.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_muted)
                .child(if typed.is_empty() {
                    "No one else on Arbos yet. Search a GitHub username.".to_string()
                } else {
                    format!("No one matches @{typed}.")
                }),
        );
    }
    if busy && results.is_empty() {
        any = true;
        list = list.child(
            div()
                .px(px(14.0))
                .py(px(10.0))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text_faint)
                .child("Searching…"),
        );
    }
    let count = results.len();
    for (i, (person, image)) in results.iter().enumerate() {
        any = true;
        let already = add_to.is_some_and(|g| {
            g.handles
                .iter()
                .any(|h| !h.is_empty() && h.eq_ignore_ascii_case(&person.handle))
        });
        let selected = already
            || picked
                .iter()
                .any(|p| p.id == person.id || p.handle == person.handle);
        let person_clone = person.clone();
        let selector = format!("group-person-{}", person.id);
        list = list.child(
            div()
                .id(ElementId::Name(SharedString::from(selector.clone())))
                .debug_selector(move || selector)
                .flex()
                .items_center()
                .gap(px(identity::ROW_GAP))
                .px(px(14.0))
                .py(px(9.0))
                .relative()
                .when(i + 1 < count, |el| el.child(kit::hairline_below(theme)))
                .when(already, |el| el.opacity(0.5))
                .map(|el| highlight::row(el, selected, Shape::Nested(PANEL_RADIUS), theme))
                .when(!already, |el| el.cursor_pointer())
                .child(avatar(image.clone(), person, 40.0, theme))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(TEXT_BODY))
                                .text_color(theme.text)
                                .truncate()
                                .child(person.display_name()),
                        )
                        .child(
                            div()
                                .text_size(px(TEXT_SMALL))
                                .text_color(theme.text_muted)
                                .child(format!("@{}", person.handle)),
                        ),
                )
                .child(pick_mark(selected, theme))
                .when(!already, |el| {
                    el.on_click(cx.listener(move |this, _, _, cx| {
                        this.state
                            .update(cx, |s, cx| s.toggle_group_person(person_clone.clone(), cx));
                    }))
                }),
        );
    }
    if any {
        let label = if !agents_shown && typed.is_empty() {
            "Arbos users"
        } else {
            "People"
        };
        body = body.child(
            div()
                .flex()
                .flex_col()
                .child(glass::section_label(label, theme).px(px(30.0)))
                .child(list),
        );
    }
    body
}

fn pick_mark(selected: bool, theme: &Theme) -> Div {
    if selected {
        div()
            .size(px(22.0))
            .rounded_full()
            .bg(theme.text)
            .flex()
            .items_center()
            .justify_center()
            .child(icon("check", 12.0, theme.bg))
    } else {
        div()
            .size(px(22.0))
            .rounded_full()
            .border_1()
            .border_color(Theme::with_alpha(theme.text_muted, 0.5))
    }
}

/// Member list: people (Block / Remove), agents (owner handle, Remove when mine), Leave.
pub fn members_popover(
    space: &Project,
    state: &Entity<AppState>,
    right: f32,
    cx: &mut Context<Shell>,
) -> Div {
    let theme = Theme::of(cx).clone();
    let s = state.read(cx);
    let me = s.me_handle();
    let me_id = s.me_id();
    let chat_id = space.id.clone();
    let mut card = div()
        .w(px(300.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .p(px(6.0))
        .rounded(px(PANEL_RADIUS))
        .bg(theme.card())
        .border_1()
        .border_color(theme.border_strong)
        .shadow_lg()
        .child(
            div()
                .px(px(8.0))
                .py(px(4.0))
                .text_size(px(11.0))
                .text_color(theme.text_faint)
                .child("MEMBERS"),
        );
    let action = |id: ElementId, label: &'static str, danger: bool, theme: &Theme| {
        div()
            .id(id)
            .px(px(7.0))
            .py(px(2.0))
            .rounded(px(5.0))
            .text_size(px(11.0))
            .text_color(if danger {
                theme.danger
            } else {
                theme.text_muted
            })
            .cursor_pointer()
            .map(|el| highlight::button(el, theme))
            .child(label)
    };
    // Me first.
    card = card.child(
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .py(px(5.0))
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.5))
                    .text_color(theme.text)
                    .child(format!("@{me} (you)")),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.text_faint)
                    .child("owner"),
            ),
    );
    for p in &space.people {
        let image = s.avatar(&p.avatar_url);
        let handle = p.handle.clone();
        let user_id = p.id.clone();
        let chat_a = chat_id.clone();
        let chat_b = chat_id.clone();
        let open_handle = p.handle.clone();
        let open_space = chat_id.clone();
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(8.0))
                .py(px(5.0))
                .child(
                    div()
                        .id(ElementId::Name(SharedString::from(format!(
                            "member-open-{}",
                            p.id
                        ))))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .map(|el| highlight::row(el, false, Shape::Own, &theme))
                        .child(avatar(image, p, 22.0, &theme))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.5))
                                .text_color(theme.text)
                                .child(format!("@{}", p.handle)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.members_open = false;
                            let (handle, sid) = (open_handle.clone(), open_space.clone());
                            this.state.update(cx, |s, cx| {
                                s.open_profile(
                                    crate::app::ProfilePage::User {
                                        handle,
                                        space_id: Some(sid),
                                    },
                                    cx,
                                )
                            });
                        })),
                )
                .child(
                    action(
                        ElementId::Name(SharedString::from(format!("block-{}", p.id))),
                        "Block",
                        false,
                        &theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.members_open = false;
                        let h = handle.clone();
                        let c = chat_a.clone();
                        this.state.update(cx, |s, cx| {
                            s.block_user(h, cx);
                            s.leave_chat(c, cx);
                        });
                    })),
                )
                .child(
                    action(
                        ElementId::Name(SharedString::from(format!("remove-{}", p.id))),
                        "Remove",
                        true,
                        &theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.members_open = false;
                        let u = user_id.clone();
                        let c = chat_b.clone();
                        this.state.update(cx, |s, cx| s.remove_member(c, u, cx));
                    })),
                ),
        );
    }
    for a in &space.agents {
        let mine = a.owner_id == me_id || a.owner_handle == me;
        let pid = a.project_id.clone();
        let chat = chat_id.clone();
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(8.0))
                .py(px(5.0))
                .child(
                    div()
                        .id(ElementId::Name(SharedString::from(format!(
                            "agent-open-{}",
                            a.project_id
                        ))))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .map(|el| highlight::row(el, false, Shape::Own, &theme))
                        .child(agent_glyph(s, Some(space), a, 22.0, &theme))
                        .on_click({
                            let pid = a.project_id.clone();
                            cx.listener(move |this, _, _, cx| {
                                this.members_open = false;
                                let pid = pid.clone();
                                this.state.update(cx, |s, cx| {
                                    s.open_profile(
                                        crate::app::ProfilePage::Agent { project_id: pid },
                                        cx,
                                    )
                                });
                            })
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme.text)
                                .child(format!("{} · @{}", a.name, a.handle)),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(theme.text_faint)
                                .child(format!(
                                    "owned by @{} · {}",
                                    a.owner_handle,
                                    match a.reply_policy {
                                        crate::model::ReplyPolicy::Mention =>
                                            "answers when mentioned",
                                        crate::model::ReplyPolicy::EveryMessage =>
                                            "answers every message",
                                    }
                                )),
                        ),
                )
                .when(mine, |el| {
                    el.child(
                        action(
                            ElementId::Name(SharedString::from(format!(
                                "remove-agent-{}",
                                a.project_id
                            ))),
                            "Remove",
                            true,
                            &theme,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.members_open = false;
                            let p = pid.clone();
                            let c = chat.clone();
                            this.state.update(cx, |s, cx| s.remove_agent(c, p, cx));
                        })),
                    )
                }),
        );
    }
    let chat_leave = chat_id.clone();
    card = card.child(div().h(px(1.0)).my(px(4.0)).bg(theme.border)).child(
        div()
            .flex()
            .items_center()
            .px(px(8.0))
            .py(px(4.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(10.5))
                    .text_color(theme.text_faint)
                    .child(if space.agents.is_empty() {
                        "Nobody's agent reads this chat."
                    } else {
                        "Agents answer with their owner's tools and credits; any member may approve their cards."
                    }),
            )
            .child(
                action(ElementId::Name("leave-chat".into()), "Leave", true, &theme).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.members_open = false;
                        let c = chat_leave.clone();
                        this.state.update(cx, |s, cx| s.leave_chat(c, cx));
                    }),
                ),
            ),
    );
    div()
        .absolute()
        .top(px(crate::ui::theme::TAB_BAR_HEIGHT + 48.0))
        .right(px(right))
        .child(frosted(PANEL_RADIUS, MENU_BLUR, card))
}

/// Stacked avatars for a project with people in it (tile corner / header).
pub fn avatar_stack(
    people: &[(Person, Option<Arc<RenderImage>>)],
    size: f32,
    max: usize,
    theme: &Theme,
) -> Div {
    let mut row = div().flex().items_center();
    for (i, (p, image)) in people.iter().take(max).enumerate() {
        let image = image.clone();
        row = row.child(
            div()
                .when(i > 0, |el| el.ml(px(-size * 0.3)))
                .rounded_full()
                .border_1()
                .border_color(theme.bg)
                .child(avatar(image, p, size, theme)),
        );
    }
    if people.len() > max {
        row = row.child(
            div()
                .ml(px(2.0))
                .text_size(px(size * 0.5))
                .text_color(theme.text_faint)
                .child(format!("+{}", people.len() - max)),
        );
    }
    row
}

/// Members of a project: the owner, the invited people (Remove for the owner).
pub fn project_members_popover(
    project: &Project,
    state: &Entity<AppState>,
    right: f32,
    cx: &mut Context<Shell>,
) -> Div {
    let theme = Theme::of(cx).clone();
    let s = state.read(cx);
    let me = s.me_handle();
    let project_id = project.id.clone();
    let mut card = div()
        .w(px(280.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .p(px(6.0))
        .rounded(px(PANEL_RADIUS))
        .bg(theme.card())
        .border_1()
        .border_color(theme.border_strong)
        .shadow_lg()
        .child(
            div()
                .px(px(8.0))
                .py(px(4.0))
                .text_size(px(11.0))
                .text_color(theme.text_faint)
                .child(format!("PEOPLE IN {}", project.name.to_uppercase())),
        );
    if !project.is_guest() {
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(8.0))
                .py(px(5.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.5))
                        .text_color(theme.text)
                        .child(format!("@{me} (you)")),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.text_faint)
                        .child("owner"),
                ),
        );
    }
    for p in &project.people {
        let image = s.avatar(&p.avatar_url);
        let user_id = p.id.clone();
        let pid = project_id.clone();
        let is_owner = p.role == "owner";
        let open_handle = p.handle.clone();
        let open_space = project_id.clone();
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(8.0))
                .py(px(5.0))
                .child(
                    div()
                        .id(ElementId::Name(SharedString::from(format!(
                            "pmember-open-{}",
                            p.id
                        ))))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .map(|el| highlight::row(el, false, Shape::Own, &theme))
                        .child(avatar(image, p, 22.0, &theme))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.5))
                                .text_color(theme.text)
                                .child(format!("@{}", p.handle)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.members_open = false;
                            let (handle, sid) = (open_handle.clone(), open_space.clone());
                            this.state.update(cx, |s, cx| {
                                s.open_profile(
                                    crate::app::ProfilePage::User {
                                        handle,
                                        space_id: Some(sid),
                                    },
                                    cx,
                                )
                            });
                        })),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.text_faint)
                        .child(if is_owner { "owner" } else { "member" }),
                )
                .when(!project.is_guest() && !is_owner, |el| {
                    el.child(
                        div()
                            .id(ElementId::Name(SharedString::from(format!(
                                "remove-member-{}",
                                p.id
                            ))))
                            .px(px(7.0))
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .text_size(px(11.0))
                            // error-ui: exception destructive-action (Remove, not a message)
                            .text_color(theme.danger)
                            .cursor_pointer()
                            .map(|el| highlight::button(el, &theme))
                            .child("Remove")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.members_open = false;
                                let u = user_id.clone();
                                let p = pid.clone();
                                this.state
                                    .update(cx, |s, cx| s.remove_project_member(p, u, cx));
                            })),
                    )
                }),
        );
    }
    card = card.child(
        div()
            .px(px(8.0))
            .pt(px(4.0))
            .text_size(px(10.5))
            .text_color(theme.text_faint)
            .child(if project.is_guest() {
                "You can chat and drive the agent here; settings and members stay with the owner."
            } else {
                "Members see every tab and the workspace output, and the agent answers them as it answers you."
            }),
    );
    div()
        .absolute()
        .top(px(crate::ui::theme::TAB_BAR_HEIGHT + 6.0))
        .right(px(right))
        .child(frosted(PANEL_RADIUS, MENU_BLUR, card))
}

/// The Add members picker's Agents section: its label (drawn upper-cased by
/// `glass::section_label`) and debug selector. Yours first, then the other
/// members' agents already in the chat, greyed.
pub const AGENTS_SECTION_LABEL: &str = "Agents";
pub const AGENTS_SECTION_ID: &str = "add-members-agents-section";

/// Another member's agent already in the group, with its resolved identity and picture.
pub type OtherAgentRow = (
    AgentMember,
    avatars::AgentIdentity,
    Option<Arc<RenderImage>>,
);

/// The other members' agents already in `group` (not mine — mine are the
/// pickable rows), by name, narrowed by what was typed (name or @handle).
pub fn others_agents_in(group: &Project, me_handle: &str, typed: &str) -> Vec<AgentMember> {
    let needle = typed.trim().trim_start_matches('@').to_lowercase();
    let mut out: Vec<AgentMember> = group
        .agents
        .iter()
        .filter(|a| a.owner_handle != me_handle)
        .filter(|a| {
            needle.is_empty()
                || a.name.to_lowercase().contains(&needle)
                || a.handle.to_lowercase().contains(&needle)
        })
        .cloned()
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// The agent rows of the New group / Add members picker for what was typed:
/// my agents through the shared matcher (name, @slug, description), best
/// first, list order inside a tie; an empty query keeps them all.
pub fn agent_picker_rows(agents: Vec<Project>, query: &str) -> Vec<Project> {
    use arbos_core::search::{filter, normalize_query, Fields};
    filter(agents, &normalize_query(query), |p| {
        Fields::new(p.name.clone(), Project::mention_slug(&p.name))
            .with_title(p.description.clone().unwrap_or_default())
    })
}

#[cfg(test)]
mod agent_picker_tests {
    //! [feedback shared-search-matcher-mac-2026-10-26]: the agent rows of the
    //! group picker used a lower-cased substring; now the shared matcher.
    use super::*;

    fn agent(name: &str, description: &str) -> Project {
        Project {
            id: format!("p-{name}"),
            name: name.into(),
            description: (!description.is_empty()).then(|| description.to_string()),
            ..Default::default()
        }
    }

    fn names(v: &[Project]) -> Vec<&str> {
        v.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn agents_match_name_slug_or_description_ranked() {
        let all = vec![
            agent("Comms", "Research assistant"),
            agent("Research Bot", ""),
            agent("Affine Research", ""),
            agent("Doug", ""),
        ];
        assert_eq!(
            names(&agent_picker_rows(all.clone(), "res")),
            ["Research Bot", "Affine Research", "Comms"]
        );
        assert_eq!(
            names(&agent_picker_rows(all.clone(), "@affine-res")),
            ["Affine Research"]
        );
        assert_eq!(names(&agent_picker_rows(all.clone(), "")), names(&all));
        assert!(agent_picker_rows(all, "zzq").is_empty());
    }
}

#[cfg(test)]
mod first_paint_tests {
    //! The New Message sheet's create rows and people rows share one geometry
    //! (Jacob, 2026-10-22: the sheet "loads weird"); the search field's edges
    //! line up with the row content under it.
    use super::*;

    #[test]
    fn create_rows_and_people_rows_share_one_left_edge() {
        assert_eq!(ROW_PADDING_X, 14.0);
        assert_eq!(ROW_GLYPH, 40.0);
        assert_eq!(ROW_GAP, identity::ROW_GAP, "the shared picture + name gap");
        assert_eq!(ROW_GAP, 10.0);
        assert_eq!(
            ROW_PADDING_X + ROW_GLYPH + ROW_GAP,
            64.0,
            "text column starts 64 pt into the card (the dividers are the kit's, 16 / 16)"
        );
    }

    #[test]
    fn search_field_lines_up_with_the_row_content() {
        assert_eq!(CARD_INSET, 16.0);
        assert_eq!(SEARCH_INSET, CARD_INSET + ROW_PADDING_X);
        assert_eq!(SEARCH_INSET, 30.0);
    }
}
