//! A channel's pane (D88 rule 13, approved 2026-10-22) on the shared profile pane (D93): what the
//! `# general` header pill opens. Hero = the `#` glyph tile, the name (admins rename it inline)
//! with a lock badge when admins-only, `N members · <server>`. **Profile** | **Settings**:
//! Profile reads — topic, members, privacy, who posts, the server (pushes its pane), mute;
//! Settings (owner/admin) edits — Name, Topic (inline fields), Notifications, Privacy, "Admins
//! only post". Footer on both tabs: Archive (everyone), Delete channel (owner/admin; asks first).

use gpui::{div, prelude::*, px, AnyElement, Context, Div, FontWeight, SharedString, Stateful};

use crate::app::ProfilePage;
use crate::model::{ServerChannel, ServerInfo};
use crate::ui::glass;
use crate::ui::profiles::kit;
use crate::ui::shell::Shell;
use crate::ui::sidebar::server_picture;
use crate::ui::theme::{Theme, TEXT_BODY, TEXT_SMALL};

use super::editable_name;
use super::pane::{self, Hero, Picture, ProfilePane, Section};

pub(super) const TABS: [&str; 2] = ["Profile", "Settings"];

pub(super) fn page(
    server_id: &str,
    channel_id: &str,
    shell: &Shell,
    cx: &mut Context<Shell>,
) -> Option<Stateful<Div>> {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let server = state.server(server_id)?.clone();
    let channel = server.channel(channel_id)?.clone();
    let restricted =
        channel.restricted || state.channel_row(&channel).is_some_and(|r| r.restricted);
    let can_admin = server.can_admin();
    let tab = state.profile_tab;
    let muted = state.is_muted(&channel.id);
    let renaming = shell.channel_rename.as_deref() == Some(channel.id.as_str());
    let editing_topic = shell.channel_topic_edit.as_deref() == Some(channel.id.as_str());
    let delete_armed = shell.channel_delete_armed && shell.channel_menu.is_none();

    // ----- hero: the # tile, the name (a field for admins), lock when admins-only ----
    let count = server.member_count.max(server.members.len() as u64);
    let name: AnyElement = if renaming {
        name_field(shell, &theme).into_any_element()
    } else {
        let click = can_admin.then(|| {
            let (cid, cname) = (channel.id.clone(), channel.name.clone());
            let f = cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
                this.start_rename_channel(cid.clone(), cname.clone(), window, cx);
            });
            Box::new(f) as super::HeroClick
        });
        editable_name(
            "profile-name",
            &channel.name,
            false,
            &shell.agent_name_input,
            click,
            &theme,
        )
        .into_any_element()
    };
    let hero = Hero {
        picture: Picture::Glyph("hash"),
        name,
        badge: restricted.then_some("lock"),
        subtitle: Some(
            format!(
                "{count} member{} · {}",
                if count == 1 { "" } else { "s" },
                server.name
            )
            .into(),
        ),
        subtitle_color: theme.text_muted,
        subtitle_click: None,
        subtitle_element: None,
        editor: None,
        action: None,
    };

    let body = match tab {
        0 => profile_tab(
            &server, &channel, restricted, muted, count, shell, &theme, cx,
        ),
        _ => settings_tab(
            &server,
            &channel,
            restricted,
            muted,
            can_admin,
            editing_topic,
            shell,
            &theme,
            cx,
        ),
    };
    let footer = danger_footer(&server, &channel, can_admin, delete_armed, &theme, cx);

    Some(pane::render(
        ProfilePane {
            page: ProfilePage::Channel {
                server_id: server.id.clone(),
                channel_id: channel.id.clone(),
            },
            hero,
            tabs: &TABS,
            selected: tab,
            body,
            footer,
            done: None,
            share: None,
        },
        shell,
        cx,
    ))
}

/// The hero's name as the inline rename field (`server_input`: Return commits, Escape cancels).
fn name_field(shell: &Shell, theme: &Theme) -> Div {
    div()
        .h(px(32.0))
        .min_w(px(160.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .rounded(px(8.0))
        .bg(theme.field)
        .border_1()
        .border_color(theme.primary)
        .text_size(px(TEXT_BODY))
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(shell.server_input.clone())
}

fn topic_row(topic: &str, theme: &Theme) -> AnyElement {
    div()
        .px(px(kit::INSET))
        .py(px(12.0))
        .text_size(px(TEXT_BODY))
        .text_color(theme.text)
        .child(topic.to_owned())
        .into_any_element()
}

fn server_row(
    server: &ServerInfo,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let page = ProfilePage::Server {
        server_id: server.id.clone(),
    };
    let picture =
        server_picture(server, shell.state.read(cx), kit::AVATAR, theme).into_any_element();
    kit::row(
        "channel-server",
        kit::Leading::Avatar(picture),
        server.name.clone(),
        Some("Server".into()),
        None,
        None,
        true,
        theme,
    )
    .cursor_pointer()
    .group(kit::HOT)
    .on_click(cx.listener(move |this, _, _, cx| {
        this.push_profile(page.clone(), cx);
    }))
    .into_any_element()
}

/// A switch row whose click flips the value through `on_flip`.
fn toggle_row(
    id: &'static str,
    title: &str,
    subtitle: Option<&str>,
    on: bool,
    theme: &Theme,
    on_flip: impl Fn(&mut Shell, &mut Context<Shell>) + 'static,
    cx: &mut Context<Shell>,
) -> AnyElement {
    kit::row(
        id,
        kit::Leading::None,
        title.to_owned(),
        subtitle.map(SharedString::from),
        None,
        Some(glass::switch(on, theme).into_any_element()),
        false,
        theme,
    )
    .cursor_pointer()
    .on_click(cx.listener(move |this, _, _, cx| on_flip(this, cx)))
    .into_any_element()
}

fn privacy_label(restricted: bool) -> &'static str {
    if restricted {
        "Everyone reads · admins post · likes allowed"
    } else {
        "Everyone in the server"
    }
}

/// Profile: what the channel is, who is in it, how it is set up — read-only, plus mute.
#[allow(clippy::too_many_arguments)]
fn profile_tab(
    server: &ServerInfo,
    channel: &ServerChannel,
    restricted: bool,
    muted: bool,
    count: u64,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    let mut about: Vec<AnyElement> = Vec::new();
    if !channel.topic.trim().is_empty() {
        about.push(topic_row(&channel.topic, theme));
    }
    let members_page = ProfilePage::Server {
        server_id: server.id.clone(),
    };
    about.push(
        kit::row(
            "channel-members",
            kit::Leading::None,
            "Members",
            None,
            Some(count.to_string().into()),
            None,
            true,
            theme,
        )
        .cursor_pointer()
        .group(kit::HOT)
        .on_click(cx.listener(move |this, _, _, cx| {
            // The server's Members tab, pushed so ‹ comes back here.
            this.push_profile(members_page.clone(), cx);
            this.state.update(cx, |s, cx| s.set_profile_tab(1, cx));
        }))
        .into_any_element(),
    );
    about.push(
        kit::value_row(
            "channel-privacy",
            "Privacy",
            privacy_label(restricted),
            theme,
        )
        .into_any_element(),
    );
    about.push(
        kit::value_row(
            "channel-posting",
            "Posting",
            if restricted {
                "Announcement · likes, no replies"
            } else {
                "Everyone"
            },
            theme,
        )
        .into_any_element(),
    );
    let cid = channel.id.clone();
    vec![
        Section::Card(about),
        Section::Card(vec![server_row(server, shell, theme, cx)]),
        Section::Card(vec![toggle_row(
            "channel-mute",
            "Mute channel",
            Some("No banners from this channel on this Mac"),
            muted,
            theme,
            move |this, cx| {
                let cid = cid.clone();
                this.state.update(cx, |s, cx| s.toggle_mute(cid, cx));
            },
            cx,
        )]),
    ]
}

/// Settings (D93 order): identity (Name, Topic) → behaviour (Notifications, Privacy, who may
/// post) → relations (the server). Owners/admins edit; members read.
#[allow(clippy::too_many_arguments)]
fn settings_tab(
    server: &ServerInfo,
    channel: &ServerChannel,
    restricted: bool,
    muted: bool,
    can_admin: bool,
    editing_topic: bool,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    // ----- identity -------------------------------------------------------------
    let mut identity: Vec<AnyElement> = Vec::new();
    let (cid, cname) = (channel.id.clone(), channel.name.clone());
    let mut name_row = kit::row(
        "channel-set-name",
        kit::Leading::None,
        "Name",
        None,
        Some(channel.name.clone().into()),
        None,
        can_admin,
        theme,
    );
    if can_admin {
        name_row = name_row
            .cursor_pointer()
            .group(kit::HOT)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.start_rename_channel(cid.clone(), cname.clone(), window, cx);
            }));
    }
    identity.push(name_row.into_any_element());
    if editing_topic {
        identity.push(
            div()
                .flex()
                .items_center()
                .gap(px(kit::GAP))
                .min_h(px(kit::ROW_MIN_H))
                .px(px(kit::INSET))
                .py(px(6.0))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_size(px(TEXT_BODY))
                        .text_color(theme.text)
                        .child("Topic"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(30.0))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .rounded(px(8.0))
                        .bg(theme.field)
                        .border_1()
                        .border_color(theme.primary)
                        .text_size(px(TEXT_BODY))
                        .child(shell.server_input.clone()),
                )
                .into_any_element(),
        );
    } else {
        let (cid, ctopic) = (channel.id.clone(), channel.topic.clone());
        let shown: SharedString = if channel.topic.trim().is_empty() {
            if can_admin { "Add a topic" } else { "None" }.into()
        } else {
            channel.topic.clone().into()
        };
        let mut topic_row = kit::row(
            "channel-set-topic",
            kit::Leading::None,
            "Topic",
            None,
            Some(shown),
            None,
            can_admin,
            theme,
        );
        if can_admin {
            topic_row = topic_row
                .cursor_pointer()
                .group(kit::HOT)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.start_edit_channel_topic(cid.clone(), ctopic.clone(), window, cx);
                }));
        }
        identity.push(topic_row.into_any_element());
    }

    // ----- behaviour ------------------------------------------------------------
    let mut behaviour: Vec<AnyElement> = Vec::new();
    let mute_id = channel.id.clone();
    behaviour.push(toggle_row(
        "channel-set-mute",
        "Notifications",
        Some(if muted {
            "Muted on this Mac"
        } else {
            "Banners on this Mac"
        }),
        !muted,
        theme,
        move |this, cx| {
            let cid = mute_id.clone();
            this.state.update(cx, |s, cx| s.toggle_mute(cid, cx));
        },
        cx,
    ));
    behaviour.push(
        kit::value_row(
            "channel-set-privacy",
            "Privacy",
            privacy_label(restricted),
            theme,
        )
        .into_any_element(),
    );
    if can_admin {
        let (sid, cid) = (server.id.clone(), channel.id.clone());
        behaviour.push(toggle_row(
            "channel-set-restricted",
            "Announcement channel",
            Some("Admins post · members can like, not reply"),
            restricted,
            theme,
            move |this, cx| {
                let (sid, cid) = (sid.clone(), cid.clone());
                this.state.update(cx, |s, cx| {
                    s.set_channel_restricted(sid, cid, !restricted, cx)
                });
            },
            cx,
        ));
    } else {
        behaviour.push(
            kit::value_row(
                "channel-set-posting",
                "Posting",
                if restricted {
                    "Announcement · likes, no replies"
                } else {
                    "Everyone"
                },
                theme,
            )
            .into_any_element(),
        );
    }

    let mut out = vec![
        Section::Card(identity),
        Section::Card(behaviour),
        Section::Card(vec![server_row(server, shell, theme, cx)]),
    ];
    if can_admin {
        out.push(Section::Footnote(
            "Name and topic change for everyone in the server.".into(),
        ));
    }
    out
}

/// Footer on both tabs: Archive / Unarchive (per user), and for owners/admins Delete channel —
/// which first asks "Delete #name?" in the row's place. `#general` cannot be deleted.
fn danger_footer(
    server: &ServerInfo,
    channel: &ServerChannel,
    can_admin: bool,
    delete_armed: bool,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    let is_general = server.general().is_some_and(|g| g.id == channel.id);
    let archived = channel.archived;
    let (sid, cid) = (server.id.clone(), channel.id.clone());
    let mut rows: Vec<AnyElement> = vec![kit::row(
        "channel-archive",
        kit::Leading::Tile(if archived { "tray-up" } else { "archive" }),
        if archived {
            "Unarchive channel"
        } else {
            "Archive channel"
        },
        Some(
            if archived {
                "Back in your channel list"
            } else {
                "Hidden from your list; the server page brings it back"
            }
            .into(),
        ),
        None,
        None,
        false,
        theme,
    )
    .cursor_pointer()
    .group(kit::HOT)
    .on_click(cx.listener(move |this, _, _, cx| {
        let (sid, cid) = (sid.clone(), cid.clone());
        this.state
            .update(cx, |s, cx| s.archive_channel(sid, cid, !archived, cx));
    }))
    .into_any_element()];
    if can_admin && !is_general {
        if delete_armed {
            let (sid, cid) = (server.id.clone(), channel.id.clone());
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .px(px(kit::INSET))
                    .py(px(10.0))
                    .child(
                        div()
                            .text_size(px(TEXT_SMALL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(format!("Delete #{}?", channel.name)),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_SMALL))
                            .text_color(theme.text_muted)
                            .child(
                                "Deletes the channel and its messages for everyone in the server.",
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(6.0))
                            .child(
                                kit::confirm_button(
                                    "channel-delete-cancel",
                                    "Cancel",
                                    false,
                                    theme,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.channel_delete_armed = false;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                kit::confirm_button("channel-delete-go", "Delete", true, theme)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.channel_delete_armed = false;
                                        let (sid, cid) = (sid.clone(), cid.clone());
                                        this.state
                                            .update(cx, |s, cx| s.delete_channel(sid, cid, cx));
                                        this.close_profile(window, cx);
                                    })),
                            ),
                    )
                    .into_any_element(),
            );
        } else {
            rows.push(
                kit::danger_row("channel-delete", "Delete channel", theme)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.channel_delete_armed = true;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
    }
    vec![Section::Card(rows)]
}
