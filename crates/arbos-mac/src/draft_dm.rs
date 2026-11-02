//! Draft DMs (Affinity, Arbonites 2026-10-29; [feedback draft-chat-from-search-mac-2026-10-29]):
//! starting a chat with someone I have no DM with opens a local draft — their name, the empty
//! "Say hi" screen and the composer — and nothing goes to the relay. `POST /chats/dm` makes the
//! chat (and notifies the other person) only when the first message is sent; the message goes
//! out in the new DM as soon as the relay lists it. Leaving the draft before that drops it.
//!
//! The draft's space and chat id is `draft-dm:<handle>`. [`AppState::project`] and
//! [`AppState::tabs`] answer for it so the chat screen draws it like any empty DM; the sidebar
//! lists only `projects`, so it never shows there. Commands that name the draft id never reach
//! the sync source.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::Context;

use crate::app::{AppEvent, AppState};
use crate::model::{
    BoxState, Chat, ChatKind, Id, MessageStatus, Person, Project, ReplyRef, SpaceKind,
};
use crate::sync::user_error::UserError;
use crate::sync::ClientCommand;

/// The id prefix of a draft DM's space and chat.
pub const ID_PREFIX: &str = "draft-dm:";

pub fn is_draft(id: &str) -> bool {
    id.starts_with(ID_PREFIX)
}

pub struct DraftDm {
    pub handle: String,
    pub space: Project,
    /// The draft's one chat, as a slice for [`AppState::tabs`].
    pub chat: Vec<Chat>,
    /// Sends made in the draft, replayed into the DM once the relay has made it.
    sends: Vec<DraftSend>,
}

/// One send made in the draft, as `send_with` got it.
pub struct DraftSend {
    pub text: String,
    pub attachments: Vec<PathBuf>,
    pub voice_wav: Option<Arc<Vec<u8>>>,
    pub reply_to: Option<String>,
    pub reply_ref: Option<ReplyRef>,
    pub mentions: Vec<String>,
}

impl DraftDm {
    fn new(person: Person) -> Self {
        let id = format!("{ID_PREFIX}{}", person.handle);
        let space = Project {
            id: id.clone(),
            name: person.handle.clone(),
            icon: person.initial(),
            color: "#8b7cf6".into(),
            state: BoxState::Awake,
            kind: SpaceKind::Dm,
            people: vec![person.clone()],
            vision_enabled: true,
            ..Project::default()
        };
        let chat = Chat {
            id: id.clone(),
            project_id: id,
            title: person.handle.clone(),
            is_pinned: true,
            is_system: false,
            unread: false,
            unread_count: 0,
            mention_count: 0,
            last_message_at: 0,
            archived_at: None,
            kind: ChatKind::Chat,
            participants: Vec::new(),
        };
        Self {
            handle: person.handle,
            space,
            chat: vec![chat],
            sends: Vec::new(),
        }
    }

    pub fn id(&self) -> &str {
        &self.space.id
    }

    /// The first message is on its way: the draft stays until the DM lands.
    pub fn is_sending(&self) -> bool {
        !self.sends.is_empty()
    }
}

/// Does `cmd` name a draft's id? Those stay in the app: the relay has no such chat.
pub fn names_draft(cmd: &ClientCommand) -> bool {
    let id = match cmd {
        ClientCommand::SetTyping { chat_id, .. }
        | ClientCommand::RemoveAgent { chat_id, .. }
        | ClientCommand::AddMembers { chat_id, .. }
        | ClientCommand::RemoveMember { chat_id, .. }
        | ClientCommand::SetMemberRole { chat_id, .. }
        | ClientCommand::LeaveChat { chat_id, .. }
        | ClientCommand::StartCall { chat_id, .. }
        | ClientCommand::SystemLine { chat_id, .. }
        | ClientCommand::Greeting { chat_id, .. }
        | ClientCommand::OpenChat { chat_id, .. }
        | ClientCommand::Send { chat_id, .. }
        | ClientCommand::Steer { chat_id, .. }
        | ClientCommand::Answer { chat_id, .. }
        | ClientCommand::Stop { chat_id, .. }
        | ClientCommand::StopJob { chat_id, .. }
        | ClientCommand::React { chat_id, .. }
        | ClientCommand::DeleteMessage { chat_id, .. }
        | ClientCommand::EditMessage { chat_id, .. }
        | ClientCommand::RemoveLinkPreview { chat_id, .. }
        | ClientCommand::Approve { chat_id, .. }
        | ClientCommand::Reject { chat_id, .. }
        | ClientCommand::Retry { chat_id, .. }
        | ClientCommand::ArchiveChat { chat_id, .. }
        | ClientCommand::MarkRead { chat_id, .. }
        | ClientCommand::RenameGroup { chat_id, .. }
        | ClientCommand::SetGroupLeaf { chat_id, .. }
        | ClientCommand::SetGroupImage { chat_id, .. }
        | ClientCommand::SetGroupImageKey { chat_id, .. }
        | ClientCommand::DeletePeopleChat { chat_id, .. }
        | ClientCommand::SendMedia { chat_id, .. }
        | ClientCommand::Forward { chat_id, .. }
        | ClientCommand::ForwardMessage { chat_id, .. }
        | ClientCommand::SecretHandoffSave { chat_id, .. }
        | ClientCommand::SendVoice { chat_id, .. } => chat_id,
        ClientCommand::SetMuted { space_id, .. } | ClientCommand::ArchiveSpace { space_id, .. } => {
            space_id
        }
        _ => return false,
    };
    is_draft(id)
}

impl AppState {
    /// `start_dm` with nobody to open: show the draft for `handle`, ask the relay for nothing.
    pub(crate) fn open_draft_dm(&mut self, handle: String, cx: &mut Context<Self>) {
        let person = self
            .people_results
            .iter()
            .chain(self.people_directory.iter())
            .chain(self.projects.iter().flat_map(|p| p.people.iter()))
            .find(|p| p.handle == handle)
            .cloned()
            .unwrap_or_else(|| Person {
                handle: handle.clone(),
                status: "member".into(),
                ..Person::default()
            });
        let reopen = self.draft_dm.as_ref().is_some_and(|d| d.handle == handle);
        if !reopen {
            self.drop_draft_dm();
            self.draft_dm = Some(DraftDm::new(person));
        }
        let id = format!("{ID_PREFIX}{handle}");
        self.new_chat_open = false;
        self.people_busy = false;
        self.active_tab.insert(id.clone(), id.clone());
        self.select_project(id, cx);
    }

    /// The draft is left before anything was sent: it is gone, with its screen state.
    pub(crate) fn drop_draft_dm(&mut self) {
        let Some(draft) = self.draft_dm.take_if(|d| !d.is_sending()) else {
            return;
        };
        let id = draft.id();
        self.messages.remove(id);
        self.chat_ui.remove(id);
        self.active_tab.remove(id);
        self.last_chats.remove(id);
        self.send_errors.remove(id);
        if self.awaiting_dm.as_deref() == Some(draft.handle.as_str()) {
            self.awaiting_dm = None;
        }
    }

    /// A send in the draft (its row already echoed): the first one asks the relay for the DM.
    pub(crate) fn queue_draft_send(&mut self, send: DraftSend) {
        let Some(draft) = self.draft_dm.as_mut() else {
            return;
        };
        let first = !draft.is_sending();
        draft.sends.push(send);
        if first {
            let handle = draft.handle.clone();
            self.awaiting_dm = Some(handle.clone());
            self.command(ClientCommand::StartDm { handle });
        }
    }

    /// The relay listed the DM the draft asked for (now the selected space): the draft's
    /// echoed rows give way to the same sends, made for real in `dm_id`.
    pub(crate) fn land_draft_dm(&mut self, handle: &str, dm_id: &Id, cx: &mut Context<Self>) {
        let Some(draft) = self.draft_dm.take_if(|d| d.handle == handle) else {
            return;
        };
        let id = draft.id().to_string();
        self.messages.remove(&id);
        self.active_tab.remove(&id);
        self.send_errors.remove(&id);
        if let Some(ui) = self.chat_ui.remove(&id) {
            let real = self.chat_ui.entry(dm_id.clone()).or_default();
            real.composer_dropped |= ui.composer_dropped;
            real.drop_started_at = real.drop_started_at.or(ui.drop_started_at);
        }
        // The relay lists the DM's space before its chat; a people DM's one chat shares the
        // space id, so the sends need not wait for the chat list.
        if self.selected_project.as_deref() == Some(dm_id.as_str()) {
            if !self.chats.get(dm_id).is_some_and(|c| !c.is_empty()) {
                let mut chat = draft.chat[0].clone();
                chat.id = dm_id.clone();
                chat.project_id = dm_id.clone();
                chat.title = self
                    .project(dm_id)
                    .map(|p| p.name.clone())
                    .unwrap_or(chat.title);
                self.chats.insert(dm_id.clone(), vec![chat]);
            }
            self.active_tab
                .entry(dm_id.clone())
                .or_insert_with(|| dm_id.clone());
        }
        if self.active_chat_id().as_deref() != Some(dm_id.as_str()) {
            return;
        }
        for send in draft.sends {
            if let Some(r) = send.reply_ref {
                self.chat_ui.entry(dm_id.clone()).or_default().reply_ref = Some(r);
            }
            self.send_with(
                send.text,
                send.attachments,
                send.voice_wav,
                send.reply_to,
                false,
                send.mentions,
                cx,
            );
        }
    }

    /// The relay refused the DM: the draft's rows turn failed (Try again re-sends through
    /// the draft), and the composer says why.
    pub(crate) fn fail_draft_dm(&mut self, err: &UserError, cx: &mut Context<Self>) {
        let Some(draft) = self.draft_dm.as_mut() else {
            return;
        };
        if !draft.is_sending() {
            return;
        }
        draft.sends.clear();
        let id = draft.id().to_string();
        for m in self.messages.get_mut(&id).into_iter().flatten() {
            if m.pending {
                m.status = MessageStatus::Error;
                m.pending = false;
            }
        }
        self.set_send_error(id.clone(), err.clone());
        cx.emit(AppEvent::TranscriptChanged { chat_id: id });
    }
}
#[cfg(test)]
mod tests {
    //! A chat started from search is a draft until the first send: no `StartDm`, nothing the
    //! other person could be told about; backing out drops it; the first send makes the DM and
    //! the message goes out there; a DM I already have opens as before.
    use std::sync::{Arc, Mutex};

    use futures::future::BoxFuture;
    use gpui::{AppContext as _, Entity, TestAppContext};
    use tokio::sync::mpsc;

    use super::*;
    use crate::model::AccountView;
    use crate::sync::{SyncEvent, SyncSource};

    type Commands = Arc<Mutex<Option<mpsc::UnboundedReceiver<ClientCommand>>>>;

    struct Recording(Commands);

    impl SyncSource for Recording {
        fn name(&self) -> &'static str {
            "recording"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            *self.0.lock().expect("commands slot") = Some(commands);
            Box::pin(std::future::pending())
        }
    }

    fn person(id: &str, handle: &str) -> Person {
        Person {
            id: id.into(),
            handle: handle.into(),
            name: Some(handle.to_uppercase()),
            status: "member".into(),
            ..Person::default()
        }
    }

    fn space(id: &str, kind: SpaceKind, people: Vec<Person>) -> Project {
        Project {
            id: id.into(),
            name: id.into(),
            kind,
            people,
            ..Project::default()
        }
    }

    fn chat(id: &str) -> Chat {
        Chat {
            id: id.into(),
            project_id: id.into(),
            title: id.into(),
            is_pinned: true,
            is_system: false,
            unread: false,
            unread_count: 0,
            mention_count: 0,
            last_message_at: 1_000,
            archived_at: None,
            kind: ChatKind::Chat,
            participants: Vec::new(),
        }
    }

    /// Agent `arbos` open, a DM with Sam, a group with Kenji in it; Kenji found by search.
    fn mount(cx: &mut TestAppContext) -> (tokio::runtime::Runtime, Commands, Entity<AppState>) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| gpui_tokio::init_from_handle(cx, handle));
        let commands: Commands = Arc::new(Mutex::new(None));
        let state = cx.new(|cx| AppState::new(Box::new(Recording(commands.clone())), cx));
        cx.run_until_parked();
        for _ in 0..3 {
            rt.block_on(tokio::task::yield_now());
        }
        state.update(cx, |s, cx| {
            s.account = Some(AccountView {
                id: "gh_1".into(),
                login: "affinity".into(),
                name: "Affinity".into(),
                avatar_url: String::new(),
                device_name: "test".into(),
                balance_usd: 0.0,
                spent_today_usd: 0.0,
                daily_cap_usd: None,
            });
            s.projects = vec![
                space("arbos", SpaceKind::Project, Vec::new()),
                space("dm-sam", SpaceKind::Dm, vec![person("gh_2", "sam")]),
                space(
                    "group",
                    SpaceKind::Group,
                    vec![person("gh_4", "dana"), person("gh_3", "kenjigb")],
                ),
            ];
            for id in ["arbos", "dm-sam", "group"] {
                s.chats.insert(id.into(), vec![chat(id)]);
            }
            s.people_results = vec![person("gh_3", "kenjigb")];
            s.select_project("arbos".into(), cx);
        });
        let _ = drain(&commands);
        (rt, commands, state)
    }

    fn drain(commands: &Commands) -> Vec<ClientCommand> {
        let mut out = Vec::new();
        if let Some(rx) = commands.lock().expect("commands").as_mut() {
            while let Ok(c) = rx.try_recv() {
                out.push(c);
            }
        }
        out
    }

    /// Every command that names the relay chat with Kenji, or would make one.
    fn relay_writes(commands: &[ClientCommand]) -> Vec<String> {
        commands
            .iter()
            .filter_map(|c| match c {
                ClientCommand::StartDm { handle } => Some(format!("start_dm {handle}")),
                c if names_draft(c) => Some(format!("{c:?}")),
                _ => None,
            })
            .collect()
    }

    fn send(state: &Entity<AppState>, cx: &mut TestAppContext, text: &str) {
        state.update(cx, |s, cx| {
            s.send(text.into(), Vec::new(), None, Vec::new(), cx);
        });
    }

    #[gpui::test]
    fn starting_a_chat_from_search_asks_the_relay_for_nothing(cx: &mut TestAppContext) {
        let (_rt, commands, state) = mount(cx);
        state.update(cx, |s, cx| s.start_dm("@kenjigb".into(), cx));
        let (selected, chat, name, sidebar) = state.read_with(cx, |s, _| {
            (
                s.selected_project.clone(),
                s.active_chat_id(),
                s.selected()
                    .and_then(|p| p.person())
                    .map(|p| p.display_name()),
                s.people_spaces().count(),
            )
        });
        assert_eq!(selected.as_deref(), Some("draft-dm:kenjigb"));
        assert_eq!(chat.as_deref(), Some("draft-dm:kenjigb"));
        assert_eq!(name.as_deref(), Some("KENJIGB"), "the hero names them");
        assert_eq!(sidebar, 2, "the draft is not a row in the sidebar");
        // Typing and looking at the draft write nothing either.
        state.update(cx, |s, _| s.typed_in(&"draft-dm:kenjigb".to_string()));
        assert_eq!(relay_writes(&drain(&commands)), Vec::<String>::new());
    }

    #[gpui::test]
    fn backing_out_of_the_draft_drops_it(cx: &mut TestAppContext) {
        let (_rt, commands, state) = mount(cx);
        state.update(cx, |s, cx| {
            s.start_dm("kenjigb".into(), cx);
            s.select_project("dm-sam".into(), cx);
        });
        let (draft, found) = state.read_with(cx, |s, _| {
            (
                s.draft_dm.is_some(),
                s.project("draft-dm:kenjigb").is_some(),
            )
        });
        assert!(!draft && !found, "the draft is gone");
        assert_eq!(relay_writes(&drain(&commands)), Vec::<String>::new());
        // Starting again gives a fresh, empty draft.
        state.update(cx, |s, cx| s.start_dm("kenjigb".into(), cx));
        let rows = state.read_with(cx, |s, _| s.messages_of("draft-dm:kenjigb").len());
        assert_eq!(rows, 0);
    }

    #[gpui::test]
    fn the_first_send_makes_the_dm_and_goes_out_there(cx: &mut TestAppContext) {
        let (_rt, commands, state) = mount(cx);
        state.update(cx, |s, cx| s.start_dm("kenjigb".into(), cx));
        send(&state, cx, "hey Kenji");
        let sent = drain(&commands);
        assert_eq!(
            relay_writes(&sent),
            vec!["start_dm kenjigb".to_string()],
            "the first send asks for the DM, and only that"
        );
        let echoed = state.read_with(cx, |s, _| {
            s.messages_of("draft-dm:kenjigb")
                .iter()
                .map(|m| m.plain_text())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            echoed,
            vec!["hey Kenji".to_string()],
            "the row shows at once"
        );
        // A poll that lands before the DM exists changes nothing.
        let before = state.read_with(cx, |s, _| s.projects.clone());
        state.update(cx, |s, cx| s.apply(SyncEvent::Projects(before.clone()), cx));
        assert_eq!(
            state
                .read_with(cx, |s, _| s.selected_project.clone())
                .as_deref(),
            Some("draft-dm:kenjigb"),
            "the group with Kenji in it is not the DM"
        );
        // The relay lists the new DM (its space first, its chat list after): it opens and the
        // message is sent in it.
        let mut after = before;
        after.push(space(
            "dm-kenji",
            SpaceKind::Dm,
            vec![person("gh_3", "kenjigb")],
        ));
        state.update(cx, |s, cx| s.apply(SyncEvent::Projects(after), cx));
        let (selected, draft) = state.read_with(cx, |s, _| {
            (s.selected_project.clone(), s.draft_dm.is_some())
        });
        assert_eq!(selected.as_deref(), Some("dm-kenji"));
        assert!(!draft);
        let sends: Vec<(String, String)> = drain(&commands)
            .into_iter()
            .filter_map(|c| match c {
                ClientCommand::Send { chat_id, text, .. } => Some((chat_id, text)),
                _ => None,
            })
            .collect();
        assert_eq!(
            sends,
            vec![("dm-kenji".to_string(), "hey Kenji".to_string())]
        );
        state.update(cx, |s, cx| {
            s.apply(
                SyncEvent::Chats {
                    project_id: "dm-kenji".into(),
                    chats: vec![chat("dm-kenji")],
                },
                cx,
            )
        });
        let (active, shown) = state.read_with(cx, |s, _| {
            (
                s.active_chat_id(),
                s.messages_of("dm-kenji")
                    .iter()
                    .map(|m| m.plain_text())
                    .collect::<Vec<_>>(),
            )
        });
        assert_eq!(active.as_deref(), Some("dm-kenji"));
        assert_eq!(shown, vec!["hey Kenji".to_string()], "the sent row stays");
    }

    #[gpui::test]
    fn a_refused_dm_leaves_the_draft_with_a_failed_row(cx: &mut TestAppContext) {
        let (_rt, _commands, state) = mount(cx);
        state.update(cx, |s, cx| s.start_dm("kenjigb".into(), cx));
        send(&state, cx, "hey");
        state.update(cx, |s, cx| {
            s.apply(
                SyncEvent::PeopleError(UserError::from_text(
                    "start the chat",
                    "403 not_available: this person is not available",
                )),
                cx,
            )
        });
        let (status, note) = state.read_with(cx, |s, _| {
            (
                s.messages_of("draft-dm:kenjigb").last().map(|m| m.status),
                s.send_error(&"draft-dm:kenjigb".to_string()).is_some(),
            )
        });
        assert_eq!(status, Some(MessageStatus::Error));
        assert!(note, "the composer says why");
    }

    #[gpui::test]
    fn a_dm_i_already_have_opens_as_before(cx: &mut TestAppContext) {
        let (_rt, commands, state) = mount(cx);
        state.update(cx, |s, cx| s.start_dm("sam".into(), cx));
        let (selected, draft) = state.read_with(cx, |s, _| {
            (s.selected_project.clone(), s.draft_dm.is_some())
        });
        assert_eq!(selected.as_deref(), Some("dm-sam"));
        assert!(!draft);
        assert!(drain(&commands)
            .iter()
            .all(|c| !matches!(c, ClientCommand::StartDm { .. })));
    }
}
