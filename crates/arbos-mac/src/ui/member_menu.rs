//! The member row's menu on a group pane (Jacob, 2026-10-22): Shift-click or
//! right-click (control-click) a person or an agent in the Members list →
//! **View profile**; for the group's owner on another person's row **Make
//! admin** / **Remove admin** (ARB-46 mockup, the iPhone's same items; relay
//! `PUT /chats/{id}/members/{user} {role}` is owner-only); and — for whoever
//! may remove them — a divider and **Remove from group**. Nothing destructive
//! beyond remove.
//!
//! Remove does not act on its own: it arms the row's confirm ("Remove @x from
//! Group?" in the row's place, `profiles/group.rs`), whose Remove runs the same
//! relay call the hover ✕ uses; the relay then writes "Jacob removed Home" into
//! the chat (#1386). Your own row never offers Remove — leaving is the
//! sidebar's Delete group. Same placement rule as every menu: fully inside the
//! window (`message_menu::place`).

use gpui::{div, prelude::*, px, size, Div, Pixels, Point, Size, Window};

use crate::app::ProfilePage;
use crate::model::{Id, Project};
use crate::ui::glass::{
    self, MENU_CARD_BORDER, MENU_CARD_PADDING, MENU_DIVIDER_HEIGHT, MENU_GAP, MENU_ROW_HEIGHT,
};
use crate::ui::message_menu::place;
use crate::ui::shell::Shell;
use crate::ui::theme::Theme;

pub const MEMBER_MENU_WIDTH: f32 = 200.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberMenuItem {
    ViewProfile,
    MakeAdmin,
    RemoveAdmin,
    Remove,
}

impl MemberMenuItem {
    pub fn label(self) -> &'static str {
        match self {
            MemberMenuItem::ViewProfile => "View profile",
            MemberMenuItem::MakeAdmin => "Make admin",
            MemberMenuItem::RemoveAdmin => "Remove admin",
            MemberMenuItem::Remove => "Remove from group",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            MemberMenuItem::ViewProfile => "person.crop.circle",
            MemberMenuItem::MakeAdmin => "person.badge.key",
            MemberMenuItem::RemoveAdmin => "person.badge.minus",
            MemberMenuItem::Remove => "xmark",
        }
    }

    pub fn danger(self) -> bool {
        matches!(self, MemberMenuItem::Remove)
    }
}

/// Who the row is and who is looking: the relay's own rules (`DELETE
/// /chats/{id}/members/{user}` is for chat owners; `DELETE …/agents/{project}`
/// for the agent's owner).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberRole {
    /// The row is me.
    pub is_me: bool,
    /// The row is an agent (else a person).
    pub is_agent: bool,
    /// I own this group (people chat owner; a shared project's owner).
    pub i_own_group: bool,
    /// The agent on the row is one of my projects.
    pub agent_mine: bool,
    /// The row's group role when I may change it — I own this people group and the
    /// row is another person who is not its owner: `Some(true)` admin, `Some(false)`
    /// member. `None` everywhere else (DMs, shared projects, agents, my row, admins
    /// and members looking).
    pub row_admin: Option<bool>,
}

impl MemberRole {
    pub fn can_remove(self) -> bool {
        if self.is_me {
            return false;
        }
        if self.is_agent {
            self.agent_mine
        } else {
            self.i_own_group
        }
    }
}

/// The items for one row, in order. View profile always; Make / Remove admin
/// for the owner on another person; Remove only for someone allowed to remove
/// this member — a member sees a one-item menu.
pub fn items(role: MemberRole) -> Vec<MemberMenuItem> {
    let mut out = vec![MemberMenuItem::ViewProfile];
    match role.row_admin {
        Some(true) => out.push(MemberMenuItem::RemoveAdmin),
        Some(false) => out.push(MemberMenuItem::MakeAdmin),
        None => {}
    }
    if role.can_remove() {
        out.push(MemberMenuItem::Remove);
    }
    out
}

/// What a picked item does. Remove never acts by itself: it arms the row's
/// confirm, and only the confirm's Remove calls the relay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    OpenProfile,
    /// `PUT /chats/{id}/members/{user} {"role": …}` with this role.
    SetRole(&'static str),
    ArmRemove,
}

pub fn action(item: MemberMenuItem) -> MenuAction {
    match item {
        MemberMenuItem::ViewProfile => MenuAction::OpenProfile,
        MemberMenuItem::MakeAdmin => MenuAction::SetRole("admin"),
        MemberMenuItem::RemoveAdmin => MenuAction::SetRole("member"),
        MemberMenuItem::Remove => MenuAction::ArmRemove,
    }
}

/// My row-role answer for a person in a people group: only the group's owner changes
/// roles, and never their own row or the owner's.
fn row_admin(space: &Project, me_handle: &str, uid: &str, is_me: bool) -> Option<bool> {
    if is_me || !space.is_people_group() {
        return None;
    }
    let i_am_owner = match space.my_role.as_deref() {
        Some(role) => role == "owner",
        None => space.owner_handle.as_deref() == Some(me_handle),
    };
    if !i_am_owner {
        return None;
    }
    match space.people.iter().find(|p| p.id == uid)?.role.as_str() {
        "admin" => Some(true),
        "member" => Some(false),
        _ => None,
    }
}

/// A member row's menu, open at `at`.
#[derive(Clone, Debug)]
pub struct MemberMenu {
    pub space_id: Id,
    /// `user:<id>` or `agent:<project id>` — the key `group.rs` arms its confirm with.
    pub key: String,
    pub at: Point<Pixels>,
}

fn menu_size(items: &[MemberMenuItem]) -> Size<Pixels> {
    let dividers = usize::from(items.iter().any(|i| i.danger()));
    let rows = items.len() as f32;
    let h = 2.0 * (MENU_CARD_BORDER + MENU_CARD_PADDING)
        + rows * MENU_ROW_HEIGHT
        + (rows - 1.0).max(0.0) * MENU_GAP
        + dividers as f32 * (MENU_DIVIDER_HEIGHT + MENU_GAP);
    size(px(MEMBER_MENU_WIDTH), px(h))
}

/// The open menu, appended straight to the shell's root like the other menus.
pub fn render(shell: &Shell, window: &Window, cx: &mut Context<Shell>) -> Option<Div> {
    let menu = shell.member_menu.clone()?;
    let theme = Theme::of(cx).clone();
    let state = shell.state.read(cx);
    let space = state.project(&menu.space_id)?.clone();
    let me_handle = state.me_handle();
    let is_people = space.is_people();
    let i_own_group = if is_people {
        space.owner_handle.as_deref().is_none_or(|o| o == me_handle)
    } else {
        !space.is_guest()
    };
    // The row: a person (by `user:<id>`) or an agent (by `agent:<project>`).
    let (role, page) = if let Some(uid) = menu.key.strip_prefix("user:") {
        // The pane lists me first (`user:<my id>`), then the relay's other members.
        let is_me = uid == state.me_id();
        let handle = if is_me {
            me_handle.clone()
        } else {
            space.people.iter().find(|p| p.id == uid)?.handle.clone()
        };
        (
            MemberRole {
                is_me,
                is_agent: false,
                i_own_group,
                agent_mine: false,
                row_admin: row_admin(&space, &me_handle, uid, is_me),
            },
            ProfilePage::User {
                handle,
                space_id: Some(space.id.clone()),
            },
        )
    } else {
        let pid = menu.key.strip_prefix("agent:")?;
        let mine = if is_people {
            space
                .agents
                .iter()
                .find(|a| a.project_id == pid)
                .is_some_and(|a| a.owner_handle == me_handle)
        } else {
            !space.is_guest()
        };
        (
            MemberRole {
                is_me: false,
                is_agent: true,
                i_own_group,
                agent_mine: mine,
                row_admin: None,
            },
            ProfilePage::Agent {
                project_id: pid.to_owned(),
            },
        )
    };
    let items = items(role);
    let origin = place(menu.at, menu_size(&items), window.viewport_size());
    let mut body = div().flex().flex_col().gap(px(MENU_GAP));
    for item in items {
        if item.danger() {
            body = body.child(glass::menu_divider(&theme));
        }
        let (key, page, space_id) = (menu.key.clone(), page.clone(), space.id.clone());
        body = body.child(
            glass::menu_row(
                format!(
                    "member-menu-{}",
                    item.label().to_ascii_lowercase().replace(' ', "-")
                ),
                item.glyph(),
                item.label(),
                item.danger(),
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.member_menu = None;
                match action(item) {
                    MenuAction::OpenProfile => {
                        let page = page.clone();
                        this.state.update(cx, |s, cx| s.open_profile(page, cx));
                    }
                    MenuAction::SetRole(role) => {
                        if let Some(uid) = key.strip_prefix("user:") {
                            let (chat, uid) = (space_id.clone(), uid.to_owned());
                            this.state
                                .update(cx, |s, cx| s.set_people_member_role(chat, uid, role, cx));
                        }
                    }
                    MenuAction::ArmRemove => {
                        // The row swaps for "Remove @x from Group?" — Cancel / Remove.
                        this.member_remove_armed = Some(key.clone());
                    }
                }
                cx.notify();
            })),
        );
    }
    Some(
        div()
            .absolute()
            .left(origin.x)
            .top(origin.y)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(glass::frosted_menu(MEMBER_MENU_WIDTH, &theme, body)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Person, SpaceKind};

    fn labels(role: MemberRole) -> Vec<&'static str> {
        items(role).iter().map(|i| i.label()).collect()
    }

    #[test]
    fn owner_sees_remove_on_others_never_on_their_own_row() {
        let other = MemberRole {
            is_me: false,
            is_agent: false,
            i_own_group: true,
            agent_mine: false,
            row_admin: None,
        };
        assert_eq!(labels(other), ["View profile", "Remove from group"]);
        let me = MemberRole {
            is_me: true,
            ..other
        };
        assert_eq!(
            labels(me),
            ["View profile"],
            "leaving is Delete group, not here"
        );
    }

    #[test]
    fn a_plain_member_gets_view_profile_only() {
        let role = MemberRole {
            is_me: false,
            is_agent: false,
            i_own_group: false,
            agent_mine: false,
            row_admin: None,
        };
        assert_eq!(labels(role), ["View profile"]);
    }

    #[test]
    fn agents_follow_their_owner_not_the_group_owner() {
        let my_agent_in_someones_group = MemberRole {
            is_me: false,
            is_agent: true,
            i_own_group: false,
            agent_mine: true,
            row_admin: None,
        };
        assert_eq!(
            labels(my_agent_in_someones_group),
            ["View profile", "Remove from group"]
        );
        let their_agent_in_my_group = MemberRole {
            is_me: false,
            is_agent: true,
            i_own_group: true,
            agent_mine: false,
            row_admin: None,
        };
        assert_eq!(labels(their_agent_in_my_group), ["View profile"]);
    }

    #[test]
    fn remove_arms_the_confirm_and_never_removes_by_itself() {
        assert_eq!(action(MemberMenuItem::Remove), MenuAction::ArmRemove);
        assert_eq!(action(MemberMenuItem::ViewProfile), MenuAction::OpenProfile);
        // Nothing destructive beyond remove: one danger item at most.
        let all = items(MemberRole {
            is_me: false,
            is_agent: false,
            i_own_group: true,
            agent_mine: false,
            row_admin: None,
        });
        assert_eq!(all.iter().filter(|i| i.danger()).count(), 1);
    }

    #[test]
    fn the_menu_stays_inside_the_window() {
        let viewport = size(px(1180.0), px(780.0));
        let two = items(MemberRole {
            is_me: false,
            is_agent: false,
            i_own_group: true,
            agent_mine: false,
            row_admin: None,
        });
        // A right-click on the last member row near the pane's bottom-right.
        let origin = place(
            gpui::point(px(1170.0), px(770.0)),
            menu_size(&two),
            viewport,
        );
        let (x, y): (f32, f32) = (origin.x.into(), origin.y.into());
        let (w, h): (f32, f32) = (menu_size(&two).width.into(), menu_size(&two).height.into());
        assert!(
            x >= 0.0 && y >= 0.0 && x + w <= 1180.0 && y + h <= 780.0,
            "{origin:?}"
        );
    }

    #[test]
    fn the_owner_makes_a_member_admin_and_an_admin_a_member_again() {
        let member = MemberRole {
            is_me: false,
            is_agent: false,
            i_own_group: true,
            agent_mine: false,
            row_admin: Some(false),
        };
        assert_eq!(
            labels(member),
            ["View profile", "Make admin", "Remove from group"]
        );
        let admin = MemberRole {
            row_admin: Some(true),
            ..member
        };
        assert_eq!(
            labels(admin),
            ["View profile", "Remove admin", "Remove from group"]
        );
        assert_eq!(
            action(MemberMenuItem::MakeAdmin),
            MenuAction::SetRole("admin")
        );
        assert_eq!(
            action(MemberMenuItem::RemoveAdmin),
            MenuAction::SetRole("member")
        );
        assert!(!MemberMenuItem::MakeAdmin.danger() && !MemberMenuItem::RemoveAdmin.danger());
    }

    fn group(my_role: Option<&str>, people: &[(&str, &str)]) -> Project {
        Project {
            kind: SpaceKind::Group,
            my_role: my_role.map(str::to_owned),
            owner_handle: Some("boss".into()),
            people: people
                .iter()
                .map(|(id, role)| Person {
                    id: (*id).into(),
                    handle: (*id).into(),
                    role: (*role).into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn only_the_owner_gets_role_items_and_never_on_the_owner_or_themself() {
        let people = [("boss", "owner"), ("ada", "admin"), ("mo", "member")];
        let mine = group(Some("owner"), &people);
        assert_eq!(row_admin(&mine, "me", "mo", false), Some(false));
        assert_eq!(row_admin(&mine, "me", "ada", false), Some(true));
        assert_eq!(
            row_admin(&mine, "me", "boss", false),
            None,
            "the owner stays"
        );
        assert_eq!(row_admin(&mine, "me", "me", true), None, "not my own row");
        for role in ["admin", "member"] {
            let theirs = group(Some(role), &people);
            assert_eq!(
                row_admin(&theirs, "me", "mo", false),
                None,
                "{role} looking"
            );
        }
        // No role from the relay: the owner by handle counts.
        assert_eq!(
            row_admin(&group(None, &people), "boss", "mo", false),
            Some(false)
        );
        assert_eq!(row_admin(&group(None, &people), "me", "mo", false), None);
        let mut dm = group(Some("owner"), &people);
        dm.kind = SpaceKind::Dm;
        assert_eq!(row_admin(&dm, "me", "mo", false), None, "DMs have no roles");
    }
}
