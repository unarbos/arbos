//! A server's pane (D88 rules 5 and 6) on the shared profile pane (D93): one `ProfilePane` spec
//! — editable picture and name, `N members · N channels`, **Channels** | **Members** |
//! **Settings**, Leave / Delete server pinned in the footer. Owners and admins rename the server,
//! add / rename / delete channels and flip "Admins only post", change roles and remove or ban
//! members, and see the invite link (the owner resets it). Members read everything and can
//! leave. Nothing here says "Matrix".

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, AnyElement, Context, Div, ElementId, FontWeight, RenderImage,
    SharedString, Stateful,
};

use crate::app::{ProfilePage, ServerSheet};
use crate::model::{Person, ServerChannel, ServerInfo};
use crate::ui::glass;
use crate::ui::highlight;
use crate::ui::people;
use crate::ui::sf_symbols;
use crate::ui::shell::Shell;
use crate::ui::sidebar::server_picture;
use crate::ui::theme::{Theme, TEXT_BODY, TEXT_SMALL};

use super::pane::{self, Hero, Picture, ProfilePane, Section};
use super::{editable_name, hero_picture, kit, list_row, HeroClick, HERO_AVATAR};

pub(super) const TABS: [&str; 3] = ["Channels", "Members", "Settings"];

/// The Members tab's one add row (people only — servers hold no agents).
pub(crate) const ADD_MEMBERS_ID: &str = "server-add-members";

/// Owner first, then admins, then members, each group by handle.
fn sorted_members(server: &ServerInfo) -> Vec<Person> {
    let rank = |m: &Person| match m.role.as_str() {
        "owner" => 0,
        "admin" => 1,
        _ => 2,
    };
    let mut out = server.members.clone();
    out.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.handle.cmp(&b.handle)));
    out
}

fn role_word(role: &str) -> Option<&'static str> {
    match role {
        "owner" => Some("Owner"),
        "admin" => Some("Admin"),
        _ => None,
    }
}

pub(super) fn page(
    server_id: &str,
    shell: &Shell,
    cx: &mut Context<Shell>,
) -> Option<Stateful<Div>> {
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let server = state.server(server_id)?.clone();
    let tab = state.profile_tab;
    let me_id = state.me_id();
    let can_admin = server.can_admin();
    let is_owner = server.is_owner();
    let renaming = state.profile_editing_name && can_admin;
    let members = sorted_members(&server);
    let images: Vec<Option<Arc<RenderImage>>> = members
        .iter()
        .map(|m| state.avatar(&m.avatar_url))
        .collect();
    let selected_channel = state
        .selected_channel
        .as_ref()
        .filter(|c| c.server_id == server.id)
        .map(|c| c.channel_id.clone());
    let channel_rename = shell.channel_rename.clone();
    let member_armed = shell.server_member_armed.clone();
    let leave_armed = shell.server_leave_armed;

    // ----- hero: the disc, the name as an inline field for admins ---------------
    // D92 amendment: owners / admins get the avatar picker (Bot | Generate | Upload) on the
    // picture, like an agent's.
    let picture = server_picture(&server, state, HERO_AVATAR, &theme).into_any_element();
    let sid_pic = server.id.clone();
    let pic_click = can_admin.then(|| {
        let f = cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
            this.open_avatar_picker(
                crate::ui::avatar_picker::Target::Server(sid_pic.clone()),
                cx,
            );
        });
        Box::new(f) as HeroClick
    });
    // A member who is not an admin: the picture opens full-size in the media viewer.
    let view_click = (!can_admin)
        .then(|| server.image_url.clone())
        .flatten()
        .map(|url| {
            let name = server.name.clone();
            let f = cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                this.view_picture(&name, &url, cx);
            });
            Box::new(f) as HeroClick
        });
    let avatar = hero_picture("profile-avatar", picture, pic_click, view_click).into_any_element();
    let sid_name = server.id.clone();
    let name_click = can_admin.then(|| {
        let f = cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
            this.start_rename_server(sid_name.clone(), window, cx);
        });
        Box::new(f) as HeroClick
    });
    let name = editable_name(
        "profile-name",
        &server.name,
        renaming,
        &shell.agent_name_input,
        name_click,
        &theme,
    )
    .into_any_element();
    let count = if members.is_empty() {
        server.member_count as usize
    } else {
        members.len()
    };
    let channels_n = server.channels.iter().filter(|c| !c.archived).count();
    let subtitle = format!(
        "{count} member{} · {channels_n} channel{}",
        if count == 1 { "" } else { "s" },
        if channels_n == 1 { "" } else { "s" }
    );
    let hero = Hero {
        picture: Picture::Element(avatar),
        name,
        badge: None,
        subtitle: Some(subtitle.into()),
        subtitle_color: theme.text_muted,
        subtitle_click: None,
        subtitle_element: None,
        editor: None,
        action: None,
    };

    // ----- tabs: Channels | Members | Settings (the same set on the iPhone) ------
    let body = match tab {
        0 => channels_tab(
            &server,
            selected_channel.as_deref(),
            channel_rename.as_deref(),
            shell,
            &theme,
            cx,
        ),
        1 => members_tab(
            &server,
            &members,
            &images,
            &me_id,
            member_armed.as_deref(),
            &theme,
            cx,
        ),
        _ => settings_tab(&server, is_owner, shell, &theme, cx),
    };
    let footer = leave_footer(&server, is_owner, leave_armed, &theme, cx);

    Some(pane::render(
        ProfilePane {
            page: ProfilePage::Server {
                server_id: server.id.clone(),
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

/// Channels: "Add channel" for admins, then one row per channel — `#name`, a lock word when
/// admins-only; admins get the "Admins only" switch and a hover ✕ (never on `#general`), and
/// a double-click renames in place.
fn channels_tab(
    server: &ServerInfo,
    selected: Option<&str>,
    renaming: Option<&str>,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    let can_admin = server.can_admin();
    let mut add = None;
    if can_admin {
        let sid = server.id.clone();
        add = Some(
            kit::row(
                "profile-add-channel",
                kit::Leading::Glyph("plus"),
                "Add channel",
                None,
                None,
                None,
                false,
                theme,
            )
            .cursor_pointer()
            .group(kit::HOT)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_server_sheet(
                    ServerSheet::NewChannel {
                        server_id: sid.clone(),
                    },
                    window,
                    cx,
                );
            })),
        );
    }
    let channels: Vec<ServerChannel> = server.ordered_channels().into_iter().cloned().collect();
    if channels.is_empty() && server.archived_channels().is_empty() {
        let mut out = Vec::new();
        if let Some(row) = add {
            out.push(Section::Card(vec![row.into_any_element()]));
        }
        out.push(Section::Empty("No channels yet".into()));
        return out;
    }
    let general_id = server.general().map(|c| c.id.clone());
    let mut list: Vec<AnyElement> = Vec::new();
    for ch in channels {
        let key = ch.id.clone();
        let is_selected = selected == Some(ch.id.as_str());
        if renaming == Some(ch.id.as_str()) {
            list.push(
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
                            .size(px(kit::AVATAR))
                            .flex_shrink_0()
                            .rounded(px(10.0))
                            .bg(theme.surface_2)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(sf_symbols::glyph("number", 18.0, theme.text_muted)),
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
            continue;
        }
        let subtitle: Option<SharedString> = ch.restricted.then(|| "Admins only post".into());
        let is_general = general_id.as_deref() == Some(ch.id.as_str());
        let trailing = can_admin.then(|| {
            let (sid, cid, on) = (server.id.clone(), ch.id.clone(), ch.restricted);
            let mut controls = div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .flex_shrink_0()
                .child(
                    div()
                        .id(ElementId::Name(SharedString::from(format!(
                            "channel-restricted-{key}"
                        ))))
                        .cursor_pointer()
                        .child(glass::switch(on, theme))
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            let (sid, cid) = (sid.clone(), cid.clone());
                            this.state
                                .update(cx, |s, cx| s.set_channel_restricted(sid, cid, !on, cx));
                        })),
                );
            if !is_general {
                let (sid, cid) = (server.id.clone(), ch.id.clone());
                let group = SharedString::from(kit::HOT);
                controls = controls.child(
                    div()
                        .id(ElementId::Name(SharedString::from(format!(
                            "channel-delete-{key}"
                        ))))
                        .size(px(24.0))
                        .flex_shrink_0()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(0.0)
                        .group_hover(group, |s| s.opacity(1.0))
                        .map(|el| highlight::button(el, theme))
                        .child(sf_symbols::glyph("xmark", 11.0, theme.text_muted))
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            // Deleting a channel is one click: the row is gone at once and the
                            // relay confirms; `#general` cannot go (no ✕ on it).
                            let (sid, cid) = (sid.clone(), cid.clone());
                            this.state
                                .update(cx, |s, cx| s.delete_channel(sid, cid, cx));
                        })),
                );
            }
            controls.into_any_element()
        });
        let (sid, cid) = (server.id.clone(), ch.id.clone());
        let rename_name = ch.name.clone();
        let rename_id = ch.id.clone();
        list.push(
            kit::row_state(
                ElementId::Name(SharedString::from(format!("profile-channel-{key}"))),
                kit::Leading::Glyph("number"),
                ch.name.clone(),
                subtitle,
                None,
                trailing,
                false,
                is_selected,
                theme,
            )
            .group(kit::HOT)
            .cursor_pointer()
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    if can_admin && event.click_count() >= 2 {
                        this.start_rename_channel(
                            rename_id.clone(),
                            rename_name.clone(),
                            window,
                            cx,
                        );
                        return;
                    }
                    let (sid, cid) = (sid.clone(), cid.clone());
                    this.state.update(cx, |s, cx| {
                        s.open_server = Some(sid.clone());
                        s.select_channel(sid, cid, cx);
                    });
                }),
            )
            .into_any_element(),
        );
    }
    // Channels I archived (row menu → Archive): listed under the live ones; the row's
    // "Unarchive" brings one back to my list.
    let archived: Vec<ServerChannel> = server.archived_channels().into_iter().cloned().collect();
    let mut out: Vec<Section> = Vec::new();
    if let Some(row) = add {
        out.push(Section::Card(vec![row.into_any_element()]));
    }
    if !list.is_empty() {
        out.push(Section::Card(list));
    }
    if can_admin {
        out.push(Section::Footnote(
            "Double-click a channel to rename it. The switch makes it admins-only.".into(),
        ));
    }
    if !archived.is_empty() {
        let mut rows: Vec<AnyElement> = Vec::new();
        for ch in archived {
            let (sid, cid) = (server.id.clone(), ch.id.clone());
            let restore = div()
                .id(ElementId::Name(SharedString::from(format!(
                    "channel-unarchive-{}",
                    ch.id
                ))))
                .flex_shrink_0()
                .px(px(10.0))
                .h(px(26.0))
                .flex()
                .items_center()
                .rounded_full()
                .cursor_pointer()
                .map(|el| highlight::washed(el, theme.field, theme))
                .text_size(px(TEXT_SMALL))
                .text_color(theme.text)
                .child("Unarchive")
                .on_click(cx.listener(move |this, _, _, cx| {
                    let (sid, cid) = (sid.clone(), cid.clone());
                    this.state
                        .update(cx, |s, cx| s.archive_channel(sid, cid, false, cx));
                }))
                .into_any_element();
            rows.push(
                kit::row(
                    ElementId::Name(SharedString::from(format!("channel-archived-{}", ch.id))),
                    kit::Leading::Glyph("archivebox"),
                    ch.name.clone(),
                    ch.restricted.then(|| "Admins only post".into()),
                    None,
                    Some(restore),
                    false,
                    theme,
                )
                .into_any_element(),
            );
        }
        out.push(Section::Title("Archived".into()));
        out.push(Section::Card(rows));
    }
    out
}

/// Members: owner first. The owner flips admin ↔ member with the crown and removes or bans
/// with the hover ✕; admins remove or ban members; members only look.
fn members_tab(
    server: &ServerInfo,
    members: &[Person],
    images: &[Option<Arc<RenderImage>>],
    me_id: &str,
    armed: Option<&str>,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    if members.is_empty() {
        return vec![Section::Empty("Loading members…".into())];
    }
    let is_owner = server.is_owner();
    let can_admin = server.can_admin();
    let mut list: Vec<AnyElement> = Vec::new();
    if can_admin {
        // The main way in (Jacob 2026-10-22): pick people by name, the groups' picker.
        let sid = server.id.clone();
        list.push(
            kit::row(
                ADD_MEMBERS_ID,
                kit::Leading::Glyph("person.badge.plus"),
                "Add members",
                None,
                None,
                None,
                false,
                theme,
            )
            .debug_selector(|| ADD_MEMBERS_ID.into())
            .cursor_pointer()
            .group(kit::HOT)
            .on_click(cx.listener(move |this, _, _, cx| {
                let sid = sid.clone();
                this.state
                    .update(cx, |s, cx| s.open_add_server_members(sid, cx));
            }))
            .into_any_element(),
        );
    }
    for (i, m) in members.iter().enumerate() {
        let is_me = m.id == me_id;
        let key = m.id.clone();
        if armed == Some(key.as_str()) {
            list.push(remove_confirm(server, m, theme, cx).into_any_element());
            continue;
        }
        let title = if is_me {
            format!("{} (you)", m.display_name())
        } else {
            m.display_name()
        };
        let subtitle = match role_word(&m.role) {
            Some(word) => format!("@{} · {word}", m.handle),
            None => format!("@{}", m.handle),
        };
        // The owner cannot be touched; admins touch members only.
        let removable =
            !is_me && m.role != "owner" && (is_owner || (can_admin && m.role != "admin"));
        let promotable = is_owner && !is_me && m.role != "owner";
        let trailing = (removable || promotable).then(|| {
            let group = SharedString::from(kit::HOT);
            let mut controls = div().flex().items_center().gap(px(4.0)).flex_shrink_0();
            if promotable {
                let (sid, uid) = (server.id.clone(), m.id.clone());
                let is_admin = m.role == "admin";
                controls = controls.child(
                    div()
                        .id(ElementId::Name(SharedString::from(format!(
                            "member-role-{key}"
                        ))))
                        .size(px(24.0))
                        .flex_shrink_0()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .when(!is_admin, |el| {
                            el.opacity(0.0)
                                .group_hover(group.clone(), |s| s.opacity(1.0))
                        })
                        .map(|el| highlight::button(el, theme))
                        .child(sf_symbols::glyph(
                            "crown.fill",
                            12.0,
                            if is_admin {
                                theme.warn
                            } else {
                                theme.text_muted
                            },
                        ))
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            let (sid, uid) = (sid.clone(), uid.clone());
                            let role = if is_admin { "member" } else { "admin" };
                            this.state
                                .update(cx, |s, cx| s.set_member_role(sid, uid, role, cx));
                        })),
                );
            }
            if removable {
                let uid = m.id.clone();
                controls = controls.child(
                    div()
                        .id(ElementId::Name(SharedString::from(format!(
                            "member-remove-{key}"
                        ))))
                        .size(px(24.0))
                        .flex_shrink_0()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(0.0)
                        .group_hover(group, |s| s.opacity(1.0))
                        .map(|el| highlight::button(el, theme))
                        .child(sf_symbols::glyph("xmark", 11.0, theme.text_muted))
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.server_member_armed = Some(uid.clone());
                            cx.notify();
                        })),
                );
            }
            controls.into_any_element()
        });
        let handle = m.handle.clone();
        list.push(
            list_row(
                ElementId::Name(SharedString::from(format!("profile-member-{key}"))),
                people::avatar(images[i].clone(), m, kit::AVATAR, theme).into_any_element(),
                title,
                subtitle,
                theme.text_muted,
                trailing,
                theme,
            )
            .group(kit::HOT)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                let handle = handle.clone();
                this.push_profile(
                    ProfilePage::User {
                        handle,
                        space_id: None,
                    },
                    cx,
                );
            }))
            .into_any_element(),
        );
    }
    vec![Section::Card(list)]
}

/// "Remove @x from Server?" in the row's place: Cancel keeps, Remove removes, Ban keeps them out.
fn remove_confirm(server: &ServerInfo, m: &Person, theme: &Theme, cx: &mut Context<Shell>) -> Div {
    let (sid, uid) = (server.id.clone(), m.id.clone());
    let (ban_sid, ban_uid) = (server.id.clone(), m.id.clone());
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
                .child(format!("Remove @{} from {}?", m.handle, server.name)),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(6.0))
                .child(
                    kit::confirm_button("server-member-cancel", "Cancel", false, theme).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.server_member_armed = None;
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    kit::confirm_button("server-member-ban", "Ban", false, theme).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.server_member_armed = None;
                            let (sid, uid) = (ban_sid.clone(), ban_uid.clone());
                            this.state
                                .update(cx, |s, cx| s.remove_server_member(sid, uid, true, cx));
                        }),
                    ),
                )
                .child(
                    kit::confirm_button("server-member-go", "Remove", true, theme).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.server_member_armed = None;
                            let (sid, uid) = (sid.clone(), uid.clone());
                            this.state
                                .update(cx, |s, cx| s.remove_server_member(sid, uid, false, cx));
                        }),
                    ),
                ),
        )
}

/// Settings (D93 order: relations): the owner's invite links — the fallback for people not on
/// Arbos yet (Jacob 2026-10-22: demoted; Add members by name is the main way in). Each link
/// shows its terms, Copy and ✕ (revoke); "New invite link" mints one with the two options
/// above it. Admins and members have nothing to set yet.
fn settings_tab(
    server: &ServerInfo,
    is_owner: bool,
    shell: &Shell,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    if !is_owner {
        return vec![Section::Empty("Nothing to set up here yet.".into())];
    }
    let now = arbos_core::ids::now_ms();
    let mut rows: Vec<AnyElement> = Vec::new();
    if server.invites.is_empty() {
        rows.push(
            kit::row(
                "server-invites-none",
                kit::Leading::Tile("link"),
                "No invite links",
                Some("People you add by name need none".into()),
                None,
                None,
                false,
                theme,
            )
            .into_any_element(),
        );
    }
    for inv in &server.invites {
        let url = inv.url.clone();
        let copy = glass::sheet_button(
            ElementId::Name(SharedString::from(format!("server-invite-copy-{}", inv.id))),
            "Copy",
            theme,
        )
        .h(px(28.0))
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(url.clone()));
            this.state.update(cx, |s, cx| {
                s.push_notice("Invite link copied");
                cx.notify();
            });
        }));
        let (sid, iid) = (server.id.clone(), inv.id.clone());
        let revoke = div()
            .id(ElementId::Name(SharedString::from(format!(
                "server-invite-revoke-{}",
                inv.id
            ))))
            .size(px(24.0))
            .flex_shrink_0()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .map(|el| highlight::button(el, theme))
            .child(sf_symbols::glyph("xmark", 11.0, theme.text_muted))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                let (sid, iid) = (sid.clone(), iid.clone());
                this.state.update(cx, |s, cx| s.revoke_invite(sid, iid, cx));
            }));
        let controls = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .flex_shrink_0()
            .when(inv.usable, |el| el.child(copy))
            .child(revoke)
            .into_any_element();
        let short = inv.url.rsplit('/').next().unwrap_or(&inv.code).to_owned();
        rows.push(
            kit::row(
                ElementId::Name(SharedString::from(format!("server-invite-{}", inv.id))),
                kit::Leading::Tile("link"),
                format!("…/s/{short}"),
                Some(inv.terms(now).into()),
                None,
                Some(controls),
                false,
                theme,
            )
            .when(!inv.usable, |el| el.opacity(0.55))
            .into_any_element(),
        );
    }

    // ----- New link: two options, then the action ---------------------------------
    let (expires, single) = (shell.invite_expires, shell.invite_single_use);
    let sid = server.id.clone();
    let new_link: Vec<AnyElement> = vec![
        kit::row(
            "server-invite-expires",
            kit::Leading::None,
            "Expires after 7 days",
            None,
            None,
            Some(glass::switch(expires, theme).into_any_element()),
            false,
            theme,
        )
        .cursor_pointer()
        .on_click(cx.listener(|this, _, _, cx| {
            this.invite_expires = !this.invite_expires;
            cx.notify();
        }))
        .into_any_element(),
        kit::row(
            "server-invite-single",
            kit::Leading::None,
            "Single use",
            Some("One person joins, then the link is spent".into()),
            None,
            Some(glass::switch(single, theme).into_any_element()),
            false,
            theme,
        )
        .cursor_pointer()
        .on_click(cx.listener(|this, _, _, cx| {
            this.invite_single_use = !this.invite_single_use;
            cx.notify();
        }))
        .into_any_element(),
        kit::row(
            "server-invite-new",
            kit::Leading::Glyph("plus"),
            "New invite link",
            None,
            None,
            None,
            false,
            theme,
        )
        .cursor_pointer()
        .group(kit::HOT)
        .on_click(cx.listener(move |this, _, _, cx| {
            let sid = sid.clone();
            let (expires, single) = (this.invite_expires, this.invite_single_use);
            this.state.update(cx, |s, cx| {
                s.create_invite(sid, expires, single.then_some(1), cx);
            });
        }))
        .into_any_element(),
    ];

    vec![
        Section::Title("Invite links".into()),
        Section::Card(rows),
        Section::Card(new_link),
        Section::Footnote(
            "A fallback for people who are not on Arbos yet: the link opens Arbos and joins \
             them. Add people who already have an account from the Members tab."
                .into(),
        ),
    ]
}

/// The footer on every tab: Leave server — or Delete server for the owner — asking first.
fn leave_footer(
    server: &ServerInfo,
    is_owner: bool,
    leave_armed: bool,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> Vec<Section> {
    let (label, question, note) = if is_owner {
        (
            "Delete server",
            format!("Delete “{}”?", server.name),
            "Deletes the server and its channels for everyone in it.",
        )
    } else {
        (
            "Leave server",
            format!("Leave “{}”?", server.name),
            "You leave the server. It stays for everyone else.",
        )
    };
    let sid = server.id.clone();
    let leave: AnyElement = if leave_armed {
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
                    .child(question),
            )
            .child(
                div()
                    .text_size(px(TEXT_SMALL))
                    .text_color(theme.text_muted)
                    .child(note),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.0))
                    .child(
                        kit::confirm_button("server-leave-cancel", "Cancel", false, theme)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.server_leave_armed = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        kit::confirm_button(
                            "server-leave-go",
                            if is_owner { "Delete" } else { "Leave" },
                            true,
                            theme,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.server_leave_armed = false;
                                let sid = sid.clone();
                                this.state.update(cx, |s, cx| s.leave_server(sid, cx));
                                this.close_profile(window, cx);
                            },
                        )),
                    ),
            )
            .into_any_element()
    } else {
        kit::danger_row("server-leave", label, theme)
            .on_click(cx.listener(|this, _, _, cx| {
                this.server_leave_armed = true;
                cx.notify();
            }))
            .into_any_element()
    };
    vec![Section::Card(vec![leave])]
}
