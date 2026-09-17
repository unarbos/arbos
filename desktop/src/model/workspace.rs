//! What the app *is*, as opposed to what it draws: the projects that are
//! open, the sessions running in them, and the appearance the user picked.
//!
//! Views hold an `Entity<Workspace>` and read it; they never own a copy of any
//! of this. A mutation here notifies, and whichever views are observing repaint
//! — so nothing has to remember to tell the chrome that a session appeared.
//!
//! Everything [`crate::model::state`] persists lives here and nowhere else, which is
//! why [`Workspace::save`] can take no arguments.

use crate::{
    agent, boardhub,
    data::{ColType, Column, Data, Edit, Page, Table},
    kernel, memory,
    model::{
        article::{self, Article},
        attachment::Prompt,
        board::{self, Board},
        identity::Identity,
        place::Place,
        project::Project,
        record,
        session::{self, ChatItem, ChatSession, Command},
        settings::{self, Feature, Settings},
        state::{self, State},
        surface::{self, Bind, Surface, SurfaceId, SurfaceKind},
        watch::{self, Watch},
    },
    reading,
    view::component::transcript,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bezel::{
    gpui::{
        App, ClipboardItem, Context, EntityId, EventEmitter, Image, ImageFormat, SharedString,
        Window,
    },
    theme::{self, Brand, Theme, Tint, appearance::AppearanceMode},
    ui::input,
};
use cacp::schema::SessionConfigOptionValue;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// What a table is called before it is named.
const UNTITLED: &str = "Untitled";

/// And a column.
const COLUMN: &str = "Column";

/// A project was re-read off disk and something a pane was showing has been
/// replaced — see [`crate::model::watch`].
///
/// What a view holds *about* an entry rather than the entry itself has to be
/// let go of here: a card's position is not that card's any more, and the
/// editor that had the caret is a different entity.
pub struct Reloaded;

/// The model moved what the column shows. Which pane is open belongs to
/// the window, so the root view hears this and switches; the model only
/// says a kernel row took the column, or gave it back to the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneRequest {
    Surface(SurfaceId),
    Chat,
}

/// The performance section's figures: what is in memory right now.
pub struct Resident {
    pub projects: usize,
    pub articles: usize,
    /// Articles holding an open [`editor::Editor`]. Built on first open and
    /// never dropped, so this only climbs.
    pub editors: usize,
    /// Covers decoded and held. Bounded — see [`crate::memory`].
    pub covers: usize,
    pub sessions: usize,
    /// Transcript entries across every one of them, read back whole at launch.
    pub items: usize,
}

pub struct Workspace {
    pub settings: Settings,
    pub projects: Vec<Project>,
    pub active: Option<usize>,
    pub appearance: AppearanceMode,
    pub reduce_transparency: bool,
    pub cursor_blink: bool,
    /// Session ids are minted here and never reused, so a card's link to the
    /// session it opened stays unambiguous for the life of the process.
    next_id: u64,
    /// The body size the type ladder is scaled against, in points.
    pub text_size: f32,
    /// Whether the agent's prose is weighted for bionic reading.
    /// The window's last frame (x, y, w, h in points), persisted.
    pub frame: Option<[f32; 4]>,
    pub bionic_reading: bool,
    /// The hue the greys carry, and how much of it.
    pub tint: Tint,
    /// Whether the window is showing the frame meter. Runtime only — a switch
    /// you left on is not a preference worth restoring.
    pub meter: bool,
    /// Uncommitted changes per local project root, as the poll last read
    /// them — Cursor's Changes pill and Files Changed card.
    pub changes: HashMap<std::path::PathBuf, crate::model::changes::GitChanges>,
    /// The root chat whose Working card (one row per running worker, Stop
    /// All) is open over the pills. Cursor keeps the card behind the
    /// "Working N" pill and never opens it by itself; neither does this.
    /// The transcript's "N Working" line opens it too.
    pub working_card_open: Option<u64>,
    /// The registry's mark for each configured agent, by name. Empty until the
    /// catalog lands, and stays empty offline.
    agent_icons: HashMap<String, SharedString>,
    /// What each project was last showing, by place string — where a launch
    /// puts you back.
    last: BTreeMap<String, state::Entry>,
    /// Recently opened places, newest first. The opener lists these.
    pub recents: Vec<Place>,
    /// Skills and slash templates for the active place. Refreshed from
    /// the kernel, not from the session — they belong to the project.
    pub slash_commands: Vec<Command>,
    slash_place: Option<String>,
    /// Provider models for the active place. Same cache shape as slash
    /// commands: the catalog belongs to the kernel, not the session.
    pub models: kernel::ModelsCatalog,
    models_place: Option<String>,
    /// Snapshot senders, one per project place — the board socket's up path.
    board_out: HashMap<String, tokio::sync::mpsc::UnboundedSender<boardhub::Snapshot>>,
    /// Place keys with a kernel session list in flight, so the 2s poll
    /// does not stack per place. One bool used to skip every other
    /// project — a remote sidebar then never hydrated its own agents.
    kernel_syncing: HashSet<String>,
    /// Kernel ids the user deleted, by place string. A cache so a poll
    /// cannot remint a row we already dropped; the kernel delete is truth.
    dismissed: BTreeMap<String, Vec<String>>,
    /// Pencil: a past prompt to drop into the composer on the next sync.
    /// Taken by the chrome; not persisted.
    pub pending_composer: Option<String>,
    /// Whether the permissions sheet has been shown once.
    pub permissions_seen: bool,
}

/// Under a fork's trailing prompt when the original was still answering it.
pub const FORKED_MID_TURN: &str =
    "Forked while the original chat was still answering this — it keeps that work. Send a message to continue here.";

impl Workspace {
    pub fn new(settings: Settings, state: State, cx: &mut Context<Self>) -> Self {
        let recents: Vec<Place> = state
            .recents
            .iter()
            .filter_map(|raw| Place::parse(raw))
            .collect();
        // The tab that was in front when the window last closed: a launch
        // lands there again ("close it and come back" is a step of the
        // journey and what Jacob does all day); the home tab only when that
        // place is gone.
        let was_front = state
            .projects
            .get(state.active)
            .and_then(|raw| Place::parse(raw));
        let mut projects: Vec<Project> = state
            .projects
            .into_iter()
            .filter_map(|raw| Place::parse(&raw).map(Project::open))
            .collect();
        // The home tab: `~/.arbos`, always open and first in the strip; the
        // tabs that were open last time follow it, each with its state
        // where it was left.
        let home = Self::home_place().filter(|home| std::fs::create_dir_all(&home.path).is_ok());
        if let Some(home) = home {
            projects.retain(|project| project.place() != home);
            projects.insert(0, Project::open(home));
        }
        let active = (!projects.is_empty()).then(|| {
            was_front
                .and_then(|front| projects.iter().position(|project| project.place() == front))
                .unwrap_or(0)
        });
        let restore: Vec<usize> = (0..projects.len()).collect();
        let mut this = Self {
            settings,
            projects,
            active,
            appearance: state.appearance,
            reduce_transparency: state.reduce_transparency,
            cursor_blink: state.cursor_blink,
            text_size: state.text_size,
            bionic_reading: state.bionic_reading,
            frame: state.frame,
            tint: Tint::new(state.hue, state.chroma),
            meter: false,
            changes: HashMap::new(),
            working_card_open: None,
            next_id: 0,
            agent_icons: HashMap::new(),
            last: state.last,
            recents,
            slash_commands: Vec::new(),
            slash_place: None,
            models: kernel::ModelsCatalog::default(),
            models_place: None,
            board_out: HashMap::new(),
            kernel_syncing: HashSet::new(),
            dismissed: state.dismissed,
            pending_composer: None,
            permissions_seen: state.permissions_seen,
        };
        for ix in restore {
            this.restore_sessions(ix);
            this.watch_project(ix, cx);
            this.watch_board(ix, cx);
        }
        for ix in 0..this.projects.len() {
            this.apply_dismissed(ix);
        }
        this.open_last_entry(cx);
        // Only where there is no home to land on — a machine with no home
        // directory — does the launch fall back to where it was started.
        if this.projects.is_empty()
            && let Ok(cwd) = std::env::current_dir()
        {
            this.open_project(cwd, cx);
        }
        for ix in 0..this.projects.len() {
            this.sync_kernel_sessions(ix, true, cx);
        }
        // Names typed under the old sidebar lived in state.toml. A folder
        // that has no project.toml yet takes its name from there, once, and
        // the file is the record from then on.
        for project in &mut this.projects {
            if project.identity_saved {
                continue;
            }
            if let Some(name) = state
                .names
                .get(&project.place().encode())
                .map(|name| name.trim())
                .filter(|name| !name.is_empty())
            {
                let mut identity = project.identity.clone();
                identity.name = Some(name.to_string());
                project.set_identity(identity);
            }
        }
        this.load_agent_icons(cx);
        this.watch_activity(cx);
        this.refresh_slash_commands(cx);
        this.refresh_models(cx);
        // Temporary dev hook: `ARBOS_TEST_PROMPT` sends a prompt on launch
        // so a turn can be verified without a composer. Here rather than on
        // connect, which a resume would fire again.
        if let Ok(prompt) = std::env::var("ARBOS_TEST_PROMPT") {
            let id = this
                .active_id()
                .or_else(|| this.new_session(settings::kernel_agent(), None, cx));
            if let Some(id) = id {
                this.send(id, prompt, cx);
            }
        }
        this
    }

    fn save(&self) {
        state::save(&State {
            version: state::STATE_VERSION,
            projects: self.projects.iter().map(|p| p.place().encode()).collect(),
            recents: self.recents.iter().map(|p| p.encode()).collect(),
            active: self.active.unwrap_or_default(),
            appearance: self.appearance,
            reduce_transparency: self.reduce_transparency,
            cursor_blink: self.cursor_blink,
            text_size: self.text_size,
            bionic_reading: self.bionic_reading,
            hue: self.tint.hue,
            chroma: self.tint.chroma,
            last: self.last.clone(),
            // Names live in each project's `.arbos/project.toml` now; the
            // map stays readable for the one-time migration above.
            names: BTreeMap::new(),
            dismissed: self.dismissed.clone(),
            permissions_seen: self.permissions_seen,
            frame: self.frame,
        });
    }

    /// The first-launch sheet has been shown; it will not open on its own again.
    pub fn mark_permissions_seen(&mut self) {
        self.permissions_seen = true;
        self.save();
    }

    // ── agents ───────────────────────────────────────────────────────

    /// Fetch the catalog and keep each configured agent's icon. Off the UI
    /// thread — the registry is a blocking fetch on a cold cache — and a
    /// failure just leaves the map empty.
    fn load_agent_icons(&mut self, cx: &mut Context<Self>) {
        let configured = self.settings.agents.clone();
        cx.spawn(async move |this, cx| {
            let icons = cx
                .background_executor()
                .spawn(async move { agent::icons(&configured) })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                workspace.agent_icons = icons;
                cx.notify();
            });
        })
        .detach();
    }

    /// Ask the kernel what is running: sub-agents and scheduled firings,
    /// for every chat in the project in front. A quiet poll — the line
    /// above the composer is drawn from this, and is gone when the list
    /// is empty.
    fn watch_activity(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let Some((place, parents, probes)) = this
                    .update(cx, |workspace, _| {
                        let project = workspace.active_project()?;
                        let parents: Vec<(u64, String)> = project
                            .sessions
                            .iter()
                            .filter(|chat| !chat.closed)
                            .filter_map(|chat| {
                                let sid = chat.agent_session.as_ref()?;
                                if project.dismissed.contains(sid) {
                                    return None;
                                }
                                Some((chat.id, sid.clone()))
                            })
                            .collect();
                        let probes: Vec<(u64, PathBuf)> = if project.place().host.is_some() {
                            Vec::new()
                        } else {
                            project
                                .sessions
                                .iter()
                                .filter(|chat| chat.wants_probe())
                                .filter_map(|chat| {
                                    let sid = chat.agent_session.as_ref()?;
                                    Some((chat.id, transcript_path(&project.place().path, sid)))
                                })
                                .collect()
                        };
                        Some((project.place(), parents, probes))
                    })
                    .ok()
                    .flatten()
                else {
                    continue;
                };
                let place_live = place.clone();
                let place_kids = place.clone();
                let parent_sids: Vec<String> = parents.iter().map(|(_, sid)| sid.clone()).collect();
                let live_map = cx
                    .background_executor()
                    .spawn(async move { kernel::live_by_session(&place_live) })
                    .await;
                // Live work and children only — not transcripts. A history
                // fetch every 2s would rewrite files and spin the watch.
                let kids_by_parent = cx
                    .background_executor()
                    .spawn(async move { kernel::child_sessions_many(&place_kids, &parent_sids) })
                    .await;
                // The tail of a delegate's transcript, read at most every
                // ten seconds, for the turn ends this window never saw.
                let probed: Vec<(u64, bool)> = cx
                    .background_executor()
                    .spawn(async move {
                        probes
                            .into_iter()
                            .map(|(id, path)| (id, transcript_ended(&path)))
                            .collect()
                    })
                    .await;
                let _ = this.update(cx, |workspace, cx| {
                    let Some(ix) = workspace.active else {
                        return;
                    };
                    let mut changed = false;
                    if let Some(project) = workspace.projects.get_mut(ix) {
                        for chat in &mut project.sessions {
                            // A kernel that sends plans owns `live`.
                            if !chat.plan.is_empty() {
                                continue;
                            }
                            let live = chat
                                .agent_session
                                .as_ref()
                                .and_then(|sid| live_map.get(sid).cloned())
                                .unwrap_or_default();
                            changed |= chat.set_live(live);
                        }
                        for (id, ended) in probed {
                            if let Some(chat) = project.session_mut(id) {
                                chat.probed(ended);
                            }
                        }
                    }
                    for (owner, sid) in &parents {
                        if let Some(kids) = kids_by_parent.get(sid) {
                            for kid in kids {
                                workspace.ensure_child_agent(*owner, kid.clone(), cx);
                            }
                        }
                    }
                    changed |= workspace.reap_delegates(ix, cx);
                    let known: HashSet<String> = workspace
                        .projects
                        .get(ix)
                        .map(|project| {
                            project
                                .sessions
                                .iter()
                                .filter_map(|chat| chat.agent_session.clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    if live_map.keys().any(|sid| !known.contains(sid)) {
                        workspace.sync_kernel_sessions(ix, false, cx);
                    }
                    if changed {
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    /// The registry's mark for whatever this session runs on.
    pub fn agent_icon(&self, name: &str) -> Option<SharedString> {
        self.agent_icons.get(name).cloned()
    }

    /// The kernel identity a new chat attaches as. Catalog ACP rows are
    /// not a runtime.
    pub fn preferred_agent(&self) -> Option<settings::Agent> {
        Some(settings::kernel_agent())
    }

    /// Re-read `settings.toml`. Features and other prefs; leftover
    /// `[[agents]]` rows are not a chat runtime.
    pub fn reload_settings(&mut self, cx: &mut Context<Self>) {
        if let Ok(settings) = settings::load() {
            self.settings = settings;
        }
        cx.notify();
    }

    /// The Settings tab's choice. bezel repaints on `set_mode`; the state
    /// file is what makes it survive a relaunch.
    pub fn set_appearance(&mut self, mode: AppearanceMode, cx: &mut Context<Self>) {
        self.appearance = mode;
        bezel::theme::appearance::set_mode(mode, cx);
        self.save();
        cx.notify();
    }

    /// The same window's other choice.
    pub fn set_reduce_transparency(&mut self, reduce: bool, cx: &mut Context<Self>) {
        self.reduce_transparency = reduce;
        apply_transparency(reduce, cx);
        self.save();
        cx.notify();
    }

    /// Show a surface, or stop showing it. Written through to `settings.toml`
    /// rather than app state: it is the file the gate is read back from.
    /// A failed write leaves both halves alone, so the switch stays where it
    /// was rather than claiming a gate the file does not carry.
    ///
    /// Nothing is reloaded either way. What a project holds is read when it
    /// opens and the gate is applied at the accessors below, so switching one
    /// on shows what was already there rather than needing a rescan.
    pub fn set_feature(&mut self, feature: Feature, on: bool, cx: &mut Context<Self>) {
        if settings::set_feature(feature, on).is_err() {
            return;
        }
        feature.set(&mut self.settings.features, on);
        cx.notify();
    }

    /// Move the cover ceiling. Written through to `settings.toml` first, for
    /// the reason [`Self::set_agents_enabled`] is, then applied to the cache
    /// that is already holding pictures under the old one.
    pub fn set_cover_memory(&mut self, mb: u64, window: &mut Window, cx: &mut Context<Self>) {
        if settings::set_cover_memory(mb).is_err() {
            return;
        }
        self.settings.cover_memory = mb;
        memory::covers(cx).update(cx, |covers, cx| {
            covers.set_limit(mb * 1_000_000, window, cx);
        });
        cx.notify();
    }

    /// Move the watch's bounce. Written through to `settings.toml` first, for
    /// the reason [`Self::set_cover_memory`] is, and clamped on the way in for
    /// the reason [`crate::model::watch::bounce`] clamps on the way out.
    ///
    /// Nothing is re-armed. The pump reads the interval on each pass, so the
    /// next event to land uses whatever this leaves behind.
    /// Which builds this machine updates itself to. The bar along the bottom
    /// of the window reads it back on the next frame and asks the new
    /// channel's feed what it has.
    pub fn set_update_channel(&mut self, channel: arbos_update::Channel, cx: &mut Context<Self>) {
        if settings::set_update_channel(channel).is_err() {
            return;
        }
        self.settings.update.channel = channel.as_str().to_owned();
        cx.notify();
    }

    pub fn set_watch_bounce(&mut self, ms: u64, cx: &mut Context<Self>) {
        let ms = ms.clamp(watch::BOUNCE_RANGE.0, watch::BOUNCE_RANGE.1);
        if settings::set_watch_bounce(ms).is_err() {
            return;
        }
        self.settings.watch_bounce = ms;
        cx.notify();
    }

    /// The caret is bezel's, so the setting is: nothing here reads it back.
    pub fn set_cursor_blink(&mut self, blink: bool, cx: &mut Context<Self>) {
        self.cursor_blink = blink;
        input::set_caret_blink(blink, cx);
        self.save();
        cx.notify();
    }

    pub fn set_text_size(&mut self, points: f32, cx: &mut Context<Self>) {
        self.text_size = points;
        theme::set_base_text_size(points, cx);
        self.save();
        cx.notify();
    }

    /// Kept as a global as well, the way the caret and the text size are: the
    /// transcript paints while this workspace is borrowed, so it cannot read
    /// the field off it.
    /// The window moved or was resized: remember its frame for the next
    /// launch.
    pub fn set_frame(&mut self, frame: [f32; 4]) {
        if self.frame != Some(frame) {
            self.frame = Some(frame);
            self.save();
        }
    }

    pub fn set_bionic_reading(&mut self, on: bool, cx: &mut Context<Self>) {
        self.bionic_reading = on;
        reading::set_bionic(on, cx);
        self.save();
        cx.notify();
    }

    pub fn set_tint(&mut self, tint: Tint, cx: &mut Context<Self>) {
        self.tint = tint;
        apply_tint(tint, cx);
        self.save();
        cx.notify();
    }

    // ── performance ──────────────────────────────────────────────────

    /// What this process is holding, counted off the state itself rather than
    /// tracked alongside it — a tally kept in parallel is a tally that can
    /// disagree with what is actually resident.
    pub fn resident(&self, cx: &App) -> Resident {
        let articles = || self.projects.iter().flat_map(|project| &project.articles);
        let sessions = || self.projects.iter().flat_map(|project| &project.sessions);
        Resident {
            projects: self.projects.len(),
            articles: articles().count(),
            editors: articles()
                .filter(|article| article.editor.is_some())
                .count(),
            covers: memory::covers(cx).read(cx).len(),
            sessions: sessions().count(),
            items: sessions().map(|chat| chat.items.len()).sum(),
        }
    }

    // ── projects ─────────────────────────────────────────────────────

    /// The home tab's place: `~/.arbos` on this machine. `None` only where
    /// there is no home directory to put it in.
    pub fn home_place() -> Option<Place> {
        dirs::home_dir().map(|home| Place::local(home.join(".arbos")))
    }

    /// The Home tab's index, when it is open.
    pub fn home_index(&self) -> Option<usize> {
        self.projects.iter().position(Self::is_home)
    }

    /// Whether this project is the home tab.
    pub fn is_home(project: &Project) -> bool {
        Self::home_place().is_some_and(|home| home == project.place())
    }

    /// The tab's label: "Home" for an unnamed home tab, else the name the
    /// project carries — from its `project.toml`, or its folder.
    pub fn tab_label(project: &Project) -> String {
        if Self::is_home(project) && project.identity.label().is_none() {
            "Home".into()
        } else {
            project.name()
        }
    }

    /// A project just added starts talking to an agent; one already on the
    /// rail is only brought forward.
    pub fn open_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.open_place(Place::local(path), cx);
    }

    /// A line on the root chat of the project at `place`, for a thing the
    /// window did to that place's kernel (the stranger plate's restart).
    /// Nothing if the place is not open: a closed tab has no pane to say
    /// it on, and the bar has already said what the click does.
    pub fn notice_on_root(&mut self, place: &Place, failed: bool, text: &str, cx: &mut Context<Self>) {
        let Some(project) = self.projects.iter_mut().find(|p| p.place() == *place) else {
            return;
        };
        let Some(chat) = project.sessions.iter_mut().find(|chat| chat.parent.is_none() && !chat.closed) else {
            return;
        };
        chat.notice(failed, text);
        chat.flush();
        cx.notify();
    }

    pub fn open_place(&mut self, place: Place, cx: &mut Context<Self>) {
        if let Some(ix) = self.projects.iter().position(|p| p.place() == place) {
            self.select_project(ix, cx);
            return;
        }
        self.remember_recent(&place);
        self.projects.push(Project::open(place));
        let ix = self.projects.len() - 1;
        self.restore_sessions(ix);
        self.apply_dismissed(ix);
        self.watch_project(ix, cx);
        self.watch_board(ix, cx);
        self.select_project(ix, cx);
        self.sync_kernel_sessions(ix, true, cx);
    }

    fn apply_dismissed(&mut self, ix: usize) {
        let Some(project) = self.projects.get_mut(ix) else {
            return;
        };
        if let Some(ids) = self.dismissed.get(&project.place().encode()) {
            project.dismissed.extend(ids.iter().cloned());
        }
    }

    fn remember_recent(&mut self, place: &Place) {
        self.recents.retain(|open| open != place);
        self.recents.insert(0, place.clone());
        self.recents.truncate(12);
    }

    pub fn select_project(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.projects.len() {
            return;
        }
        self.active = Some(ix);
        self.open_last_entry(cx);
        self.refresh_slash_commands(cx);
        self.refresh_models(cx);
        self.save();
        cx.notify();
    }

    /// Carry a project to another place in the list. `active` follows the
    /// project it points at rather than the index it sits on: which one is in
    /// front has nothing to do with what order they are listed in.
    /// `to` is the slot the project should sit in: `0` is the top, `len` is
    /// after the last. The index is counted before the remove, so dropping
    /// a project on the gap just below itself is a no-op.
    pub fn move_project(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let n = self.projects.len();
        if from >= n || to > n || from == to || from + 1 == to {
            return;
        }
        let project = self.projects.remove(from);
        let dest = if from < to { to - 1 } else { to };
        self.projects.insert(dest, project);
        self.active = self.active.map(|at| {
            if at == from {
                dest
            } else if from < dest && (from + 1..=dest).contains(&at) {
                at - 1
            } else if dest < from && (dest..from).contains(&at) {
                at + 1
            } else {
                at
            }
        });
        self.save();
        cx.notify();
    }

    /// Drop the project: its sessions go with it, and each session's
    /// kernel socket goes with that. The kernel stays up.
    pub fn close_project(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.projects.len() {
            return;
        }
        let key = self.projects[ix].place().encode();
        self.board_out.remove(&key);
        self.projects.remove(ix);
        self.active = self.active.and_then(|active| {
            let next = if active > ix { active - 1 } else { active };
            (!self.projects.is_empty()).then(|| next.min(self.projects.len() - 1))
        });
        self.open_last_entry(cx);
        self.save();
        cx.notify();
    }

    /// Put the project in front back where it was left — the entry it was last
    /// showing, of whichever kind. Nothing is connected by it: a session opened
    /// this way stays idle until something is sent to it.
    ///
    /// The remembered entry is found by its own identity rather than by
    /// position, because a sibling added or removed between launches shifts
    /// every index after it.
    fn last_for(&self, place: &Place) -> Option<&state::Entry> {
        self.last
            .get(&place.encode())
            .or_else(|| self.last.get(&place.path.to_string_lossy().into_owned()))
    }

    fn open_last_entry(&mut self, cx: &mut Context<Self>) {
        let Some(ix) = self.active else {
            return;
        };
        let remembered = self
            .projects
            .get(ix)
            .and_then(|open| self.last_for(&open.place()).cloned());
        if let Some(entry) = remembered {
            let (kind, id) = (entry.kind, entry.id);
            let Some(project) = self.projects.get_mut(ix) else {
                return;
            };
            let at = |path: Option<&Path>| {
                path.is_some_and(|path| {
                    path.to_string_lossy() == id || path.file_name() == Path::new(&id).file_name()
                })
            };
            match kind {
                state::Kind::Session => {
                    if let Some(id) = project
                        .sessions
                        .iter()
                        .find(|chat| {
                            at(chat.file.as_deref())
                                || chat.agent_session.as_deref() == Some(id.as_str())
                        })
                        .map(|chat| chat.id)
                    {
                        project.focus_on(id);
                    }
                }
                state::Kind::Board => {
                    project.board = project
                        .boards
                        .iter()
                        .position(|board| at(Some(&board.path)));
                }
                state::Kind::Article => {
                    project.article = project
                        .articles
                        .iter()
                        .position(|article| at(Some(&article.path)));
                    if let Some(at) = project.article {
                        project.articles[at].open(cx);
                    }
                }
                state::Kind::Table => {
                    project.table = project.tables.iter().position(|table| table.key == id);
                    project.reload_page();
                }
            }
        }
        // A remembered file that is gone, or a last-map key that no longer
        // matches, would otherwise leave the chats in the sidebar and the
        // pane on "Nothing open".
        if let Some(project) = self.projects.get_mut(ix)
            && project.focus.is_none()
        {
            if let Some(id) = project.main_session().or_else(|| {
                project
                    .sessions
                    .iter()
                    .find(|chat| !chat.closed)
                    .map(|chat| chat.id)
            }) {
                project.focus_on(id);
            }
        }
        // Every open chat reconnects after a launch, not only the one
        // in front. An article or board in the pane does not keep
        // background turns from attaching.
        self.wake_open_sessions(cx);
        cx.notify();
    }

    /// Remember the chat in front by its file, or by the kernel id when
    /// the file has not been minted yet.
    fn remember_session(&mut self, project: usize, id: u64) {
        let Some(chat) = self.projects.get(project).and_then(|open| open.session(id)) else {
            return;
        };
        if let Some(file) = &chat.file {
            self.remember(
                project,
                state::Kind::Session,
                file.to_string_lossy().into_owned(),
            );
        } else if let Some(sid) = &chat.agent_session {
            self.remember(project, state::Kind::Session, sid.clone());
        }
    }

    /// Remember the entry a project is now showing, so the next launch lands on
    /// it. Every way of opening one arrives here.
    fn remember(&mut self, project: usize, kind: state::Kind, id: String) {
        let Some(open) = self.projects.get(project) else {
            return;
        };
        self.last
            .insert(open.place().encode(), state::Entry { kind, id });
        self.save();
    }

    pub fn active_project(&self) -> Option<&Project> {
        self.active.and_then(|ix| self.projects.get(ix))
    }

    fn active_ix(&self) -> Option<usize> {
        self.active.filter(|&ix| ix < self.projects.len())
    }

    pub fn active_project_mut(&mut self) -> Option<&mut Project> {
        let ix = self.active?;
        self.projects.get_mut(ix)
    }

    // ── watching ─────────────────────────────────────────────────────

    /// Put a watch on the project at `ix`, so what an agent writes into it
    /// shows up without anyone asking for it. Every way of opening a project
    /// arrives here, and closing one drops the watch with the project.
    fn watch_project(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.projects.get(ix).is_some_and(|open| open.is_remote()) {
            return;
        }
        let Some(path) = self.projects.get(ix).map(|open| open.path.clone()) else {
            return;
        };
        let watch = Watch::open(path, cx);
        if let Some(project) = self.projects.get_mut(ix) {
            project.watch = Some(watch);
        }
    }

    /// Re-read one project off disk and reconcile it. Addressed by path rather
    /// than by index because the watch that calls this outlives any index it
    /// could have been armed with — the rail is reorderable, and closing a
    /// project shifts every one after it.
    pub fn reload_project(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(ix) = self.projects.iter().position(|open| open.path == path) else {
            return;
        };
        if self.projects[ix].reload(cx) {
            cx.emit(Reloaded);
        }
        if self.active == Some(ix) {
            self.refresh_slash_commands(cx);
        }
        cx.notify();
    }

    /// Re-read every open project: the backstop under the watch.
    ///
    /// Coming back to the window is where a missed event costs the most, and
    /// it is the one moment we can be sure of catching. A file moved in from
    /// outside the tree, a network mount the platform reports nothing for, an
    /// event dropped while the queue overflowed — none of those reach the
    /// watch, and all of them are corrected here.
    pub fn reload_projects(&mut self, cx: &mut Context<Self>) {
        let mut moved = false;
        for ix in 0..self.projects.len() {
            moved |= self.projects[ix].reload(cx);
        }
        if moved {
            cx.emit(Reloaded);
        }
        self.refresh_slash_commands(cx);
        self.refresh_models(cx);
        cx.notify();
    }

    /// Re-read skills and slash templates for the place in front.
    pub fn refresh_slash_commands(&mut self, cx: &mut Context<Self>) {
        let Some(place) = self.active_project().map(|project| project.place()) else {
            if !self.slash_commands.is_empty() {
                self.slash_commands.clear();
                self.slash_place = None;
                cx.notify();
            }
            return;
        };
        let key = place.encode();
        cx.spawn(async move |this, cx| {
            let cmds = cx
                .background_executor()
                .spawn(async move { kernel::list_commands(&place) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.slash_place.as_deref() == Some(&key) && this.slash_commands == cmds {
                    return;
                }
                this.slash_commands = cmds;
                this.slash_place = Some(key);
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-read the provider catalog for the place in front.
    pub fn refresh_models(&mut self, cx: &mut Context<Self>) {
        let Some(place) = self.active_project().map(|project| project.place()) else {
            if !self.models.models.is_empty()
                || !self.models.current.is_empty()
                || !self.models.error.is_empty()
            {
                self.models = kernel::ModelsCatalog::default();
                self.models_place = None;
                cx.notify();
            }
            return;
        };
        let key = place.encode();
        cx.spawn(async move |this, cx| {
            let catalog = cx
                .background_executor()
                .spawn(async move { kernel::list_models(&place) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.models_place.as_deref() == Some(&key) && this.models == catalog {
                    return;
                }
                this.models = catalog;
                this.models_place = Some(key);
                cx.notify();
            });
        })
        .detach();
    }

    // ── sessions ─────────────────────────────────────────────────────

    /// Open a kernel chat in the active project. `seed` is its first
    /// prompt, sent as soon as the socket is up.
    pub fn new_session(
        &mut self,
        entry: settings::Agent,
        seed: Option<String>,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let ix = self.active_ix()?;
        self.new_session_in(ix, entry, seed, cx)
    }

    /// Open a root chat in the project at `ix`, whichever is in front. The
    /// launch merge makes a project's main chat this way.
    fn new_session_in(
        &mut self,
        ix: usize,
        entry: settings::Agent,
        seed: Option<String>,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let place = self.projects.get(ix)?.place();
        let id = self.next_id;
        self.next_id += 1;
        let mut chat = ChatSession::connect(id, entry, place, seed, cx);
        let project = &mut self.projects[ix];
        chat.rank = project.front_rank(None);
        project.sessions.push(chat);
        project.focus_on(id);
        self.push_snapshot(ix);
        cx.notify();
        Some(id)
    }

    /// Open a chat nested under the active project's main chat — a side
    /// thread of the person's own, listed with the sub-agents in the panel.
    /// The project has one main chat; ⌘N does not make a second.
    pub fn new_child_session(&mut self, cx: &mut Context<Self>) -> Option<u64> {
        let ix = self.active_ix()?;
        let Some(parent) = self.projects[ix].main_session() else {
            return self.new_session_in(ix, settings::kernel_agent(), None, cx);
        };
        let parent_kernel = self.projects[ix]
            .session(parent)
            .and_then(|chat| chat.agent_session.clone());
        let id = self.next_id;
        self.next_id += 1;
        let place = self.projects[ix].place();
        let mut chat = ChatSession::connect(id, settings::kernel_agent(), place, None, cx);
        chat.parent = Some(parent);
        chat.parent_kernel = parent_kernel;
        let project = &mut self.projects[ix];
        chat.rank = project.front_rank(Some(parent));
        project.sessions.push(chat);
        project.focus_on(id);
        self.number_delegates(ix);
        self.push_snapshot(ix);
        cx.notify();
        Some(id)
    }

    /// Every project's sessions are on show, so picking one brings its project
    /// forward with it.
    pub fn select_session(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self.project_of(id) else {
            return;
        };
        self.projects[ix].focus_on(id);
        self.active = Some(ix);
        self.remember_session(ix, id);
        self.wake_session(id, cx);
        cx.notify();
    }

    /// Put `from` in front of `before`, or after the last sibling when
    /// `before` is `None`. Same project, same parent, same archived-ness.
    /// A running turn does not call this; only a user drop.
    pub fn move_session(&mut self, from: u64, before: Option<u64>, cx: &mut Context<Self>) {
        if before == Some(from) {
            return;
        }
        let Some(ix) = self.project_of(from) else {
            return;
        };
        if let Some(onto) = before {
            if self.project_of(onto) != Some(ix) {
                return;
            }
        }
        let project = &self.projects[ix];
        let Some(moved) = project.session(from) else {
            return;
        };
        if let Some(onto) = before {
            let Some(target) = project.session(onto) else {
                return;
            };
            if moved.parent != target.parent || moved.closed != target.closed {
                return;
            }
        }
        let parent = moved.parent;
        let closed = moved.closed;
        let mut siblings: Vec<u64> = project
            .sessions
            .iter()
            .filter(|chat| chat.parent == parent && chat.closed == closed)
            .map(|chat| chat.id)
            .collect();
        siblings.sort_by_key(|&id| {
            project
                .session(id)
                .map(|chat| (chat.rank, id))
                .unwrap_or((0, id))
        });
        let Some(from_ix) = siblings.iter().position(|&id| id == from) else {
            return;
        };
        siblings.remove(from_ix);
        let to_ix = match before {
            Some(id) => siblings
                .iter()
                .position(|&sid| sid == id)
                .unwrap_or(siblings.len()),
            None => siblings.len(),
        };
        if to_ix == from_ix {
            return;
        }
        siblings.insert(to_ix, from);
        let project = &mut self.projects[ix];
        for (rank, id) in siblings.iter().enumerate() {
            if let Some(chat) = project.session_mut(*id) {
                chat.rank = rank as i64;
                chat.flush();
            }
        }
        cx.notify();
    }

    /// Put `from` in front of `before` in the open or archived list.
    /// Crossing from open to archived (header toggle, or among visible
    /// archived rows) archives or restores; it never kernel-deletes.
    /// A drop into archived opens that list so the landing is visible.
    pub fn place_session(
        &mut self,
        from: u64,
        before: Option<u64>,
        archived: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.project_of(from) else {
            return;
        };
        let Some(moved) = self
            .projects
            .get(ix)
            .and_then(|project| project.session(from))
        else {
            return;
        };
        let parent = moved.parent;
        let was_archived = moved.closed;
        if parent.is_none() && was_archived != archived {
            self.archive_session(from, archived, cx);
        }
        if parent.is_none() && archived {
            if let Some(project) = self.projects.get_mut(ix) {
                project.archive_open = true;
            }
        }
        self.move_session(from, before, cx);
    }

    /// Point a session at an agent, if it has none and could have one.
    ///
    /// Same attach path a click uses: ACP websocket `open` with the
    /// stored kernel id. An archived or dismissed chat stays idle.
    fn wake_session(&mut self, id: u64, cx: &mut Context<Self>) {
        let allowed = self.projects.iter().any(|project| {
            let Some(chat) = project.session(id) else {
                return false;
            };
            if !chat.idle() || !chat.resumable() || chat.closed {
                return false;
            }
            !chat
                .agent_session
                .as_ref()
                .is_some_and(|sid| project.dismissed.contains(sid))
        });
        if !allowed {
            return;
        }
        if let Some(chat) = self.session_mut(id) {
            chat.resume(cx);
            cx.notify();
        }
    }

    /// Attach every open chat that still has a kernel id. Closed,
    /// archived, and dismissed rows stay idle. A chat without a kernel
    /// id has nothing to resume — it waits for a click or a send.
    fn wake_open_sessions(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<u64> = self
            .projects
            .iter()
            .flat_map(|project| {
                project.sessions.iter().filter_map(|chat| {
                    if chat.closed {
                        return None;
                    }
                    let sid = chat.agent_session.as_ref()?;
                    if project.dismissed.contains(sid) {
                        return None;
                    }
                    Some(chat.id)
                })
            })
            .collect();
        for id in ids {
            self.wake_session(id, cx);
        }
    }

    fn wake_open_sessions_in(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(project) = self.projects.get(ix) else {
            return;
        };
        let ids: Vec<u64> = project
            .sessions
            .iter()
            .filter_map(|chat| {
                if chat.closed {
                    return None;
                }
                let sid = chat.agent_session.as_ref()?;
                if project.dismissed.contains(sid) {
                    return None;
                }
                Some(chat.id)
            })
            .collect();
        for id in ids {
            self.wake_session(id, cx);
        }
    }

    fn project_of(&self, id: u64) -> Option<usize> {
        self.projects
            .iter()
            .position(|project| project.session(id).is_some())
    }

    /// The folder of the project that holds chat `id`, for a re-read by
    /// path (`reload_project`).
    pub fn project_root_of(&self, id: u64) -> Option<PathBuf> {
        self.project_of(id).map(|ix| self.projects[ix].path.clone())
    }

    /// Read the project's filed sessions back, minting an id for each — ids
    /// mean nothing across a launch, so a reloaded one is as new as any. The
    /// agent is resolved by name; a session whose agent has since left
    /// `settings.toml` comes back readable but cannot reconnect.
    fn restore_sessions(&mut self, ix: usize) {
        let place = self.projects[ix].place();
        let store = self.projects[ix].store();
        for (file, stored) in record::list(&store) {
            // Go / Mac ids filed on a remote sidecar are leftovers from a
            // hydrate that listed another Arbos. Skip them here so Archived
            // is not a dump of someone else's chats; merge puts a row back
            // only when this place's own gateway still has the id.
            if place.is_remote() && stored.session.as_deref().is_some_and(kernel::go_kernel_id) {
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            // Old files may still name a catalog agent. The runtime is
            // always this kernel.
            let entry = settings::kernel_agent();
            let mut chat = ChatSession::restore(id, file, place.clone(), entry, stored);
            // A worker's kind does not change; read it back off its
            // `agent.md`, live or archived, so the mark survives a relaunch.
            if let Some(sid) = chat.agent_session.clone()
                && let Some((readonly, kind)) = kernel::agent_flags(&place, &sid)
            {
                chat.readonly = readonly;
                chat.agent_kind = kind;
            }
            self.projects[ix].sessions.push(chat);
        }
        self.resolve_parents(ix);
    }

    /// Reattach to the kernel's own chat list. Local files can lag a
    /// restart; the kernel is what still holds the transcript and any
    /// turn that is in flight.
    fn sync_kernel_sessions(&mut self, ix: usize, spawn_kernel: bool, cx: &mut Context<Self>) {
        let Some(place) = self.projects.get(ix).map(|project| project.place()) else {
            return;
        };
        let key = place.encode();
        if !self.kernel_syncing.insert(key.clone()) {
            return;
        }
        let mut load: Vec<String> = self
            .projects
            .get(ix)
            .map(|project| {
                project
                    .sessions
                    .iter()
                    .filter(|chat| chat.items.is_empty())
                    .filter_map(|chat| chat.agent_session.clone())
                    .collect()
            })
            .unwrap_or_default();
        if let Some(sid) = self
            .projects
            .get(ix)
            .and_then(|project| project.active_session())
            .and_then(|chat| chat.agent_session.clone())
            && !load.contains(&sid)
        {
            load.push(sid);
        }
        // A chat this window kept whose prompts have no kernel clock yet
        // (typed here, never read back): its history lends the stamps.
        for chat in self
            .projects
            .get(ix)
            .map(|p| p.sessions.as_slice())
            .unwrap_or_default()
        {
            if let Some(sid) = chat.agent_session.clone()
                && !load.contains(&sid)
                && chat.items.iter().any(|item| {
                    matches!(item, crate::model::session::ChatItem::User(m) if m.sent_at.is_none())
                })
            {
                load.push(sid);
            }
        }
        let known: HashSet<String> = self
            .projects
            .get(ix)
            .map(|project| {
                project
                    .sessions
                    .iter()
                    .filter_map(|chat| chat.agent_session.clone())
                    .collect()
            })
            .unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let listed = cx
                .background_executor()
                .spawn({
                    let place = place.clone();
                    async move { kernel::list_sessions(&place, spawn_kernel) }
                })
                .await;
            for row in &listed.rows {
                if !known.contains(&row.id) && !load.contains(&row.id) {
                    load.push(row.id.clone());
                }
            }
            let mut histories = HashMap::new();
            for sid in load {
                if let Some(items) = kernel::session_history(&place, &sid) {
                    histories.insert(sid, items);
                }
            }
            let _ = this.update(cx, |workspace, cx| {
                workspace.kernel_syncing.remove(&key);
                workspace.merge_kernel_sessions(ix, listed, histories, spawn_kernel, cx);
            });
        })
        .detach();
    }

    fn merge_kernel_sessions(
        &mut self,
        ix: usize,
        listed: kernel::PlaceSessions,
        mut histories: HashMap<String, crate::model::history::Replay>,
        launch: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.projects.get(ix) else {
            return;
        };
        let entry = settings::kernel_agent();
        let place = project.place();
        let dismissed = project.dismissed.clone();
        let reached = listed.reached;
        let rows = listed.rows;
        let kernel_ids: HashSet<String> = rows.iter().map(|row| row.id.clone()).collect();
        let mut known: HashMap<String, u64> = project
            .sessions
            .iter()
            .filter_map(|chat| chat.agent_session.clone().map(|sid| (sid, chat.id)))
            .collect();
        for row in rows {
            if dismissed.contains(&row.id) {
                continue;
            }
            let replay = histories.remove(&row.id).unwrap_or_default();
            let items = replay.items;
            let name = {
                let name = row.name.trim();
                (!name.is_empty() && !arbos_core::chattitle::is_generic(name, Some(&row.id)))
                    .then(|| name.to_string())
            };
            let listed_title = {
                let title = row.title.trim();
                (!title.is_empty() && !arbos_core::chattitle::is_generic(title, Some(&row.id)))
                    .then(|| title.to_string())
            };
            let updated = if row.updated_ms > 0 {
                UNIX_EPOCH + Duration::from_millis(row.updated_ms as u64)
            } else {
                SystemTime::now()
            };
            if let Some(&id) = known.get(&row.id) {
                if let Some(chat) = self.projects[ix].session_mut(id) {
                    if !chat.busy() {
                        chat.adopt_history(items);
                    }
                    if let Some(model) = replay.model {
                        chat.model = Some(model);
                    }
                    if chat.name.is_none()
                        && chat.title.is_empty()
                        && let Some(title) = listed_title.clone()
                    {
                        chat.title = title;
                    }
                    if chat.name.is_none() {
                        chat.name = name;
                    }
                    if chat.updated < updated {
                        chat.updated = updated;
                    }
                    if row.readonly {
                        chat.readonly = true;
                    }
                    if chat.agent_kind.is_none() {
                        chat.agent_kind = row.agent_kind.clone();
                    }
                    // The kernel's parent wins where it names one. Where it
                    // names none the desktop's stands: a ⌘N sub-chat is
                    // nested here and nowhere the kernel can see.
                    let parent = row.parent.clone().filter(|p| !p.is_empty());
                    if parent.is_some() && chat.parent_kernel != parent {
                        chat.parent_kernel = parent;
                        chat.parent = None;
                        chat.flush();
                    }
                }
                continue;
            }
            if let Some(id) =
                Self::match_orphan(&self.projects[ix].sessions, &kernel_ids, &row, &items)
            {
                if let Some(chat) = self.projects[ix].session_mut(id) {
                    chat.agent_session = Some(row.id.clone());
                    if !chat.busy() {
                        chat.adopt_history(items);
                    }
                    if let Some(model) = replay.model {
                        chat.model = Some(model);
                    }
                    if chat.name.is_none()
                        && chat.title.is_empty()
                        && let Some(title) = listed_title.clone()
                    {
                        chat.title = title;
                    }
                    if chat.name.is_none() {
                        chat.name = name;
                    }
                    if chat.updated < updated {
                        chat.updated = updated;
                    }
                    if let Some(parent) = row.parent.clone().filter(|p| !p.is_empty()) {
                        chat.parent_kernel = Some(parent);
                        chat.parent = None;
                    }
                    chat.flush();
                }
                known.insert(row.id, id);
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            let mut chat = ChatSession::from_kernel(
                id,
                place.clone(),
                entry.clone(),
                row.id.clone(),
                listed_title.unwrap_or_default(),
                name,
                items,
                updated,
            );
            chat.parent_kernel = row.parent.filter(|p| !p.is_empty());
            chat.readonly = row.readonly;
            chat.agent_kind = row.agent_kind.clone();
            if let Some(model) = replay.model {
                chat.model = Some(model);
            }
            chat.rank = self.projects[ix].front_rank(None);
            chat.flush();
            self.projects[ix].sessions.push(chat);
            known.insert(row.id, id);
        }
        for (sid, replay) in histories {
            let Some(&id) = known.get(&sid) else {
                continue;
            };
            if let Some(chat) = self.projects[ix].session_mut(id)
                && !chat.busy()
            {
                chat.adopt_history(replay.items);
            }
        }
        if reached {
            self.drop_foreign_sessions(ix, &kernel_ids);
        }
        self.resolve_parents(ix);
        // A project whose chats all came from the kernel has had nothing to
        // focus until now. The main chat is what the column shows.
        if self.projects[ix].focus.is_none()
            && let Some(main) = self.projects[ix].main_session()
        {
            self.projects[ix].focus_on(main);
        }
        if launch {
            let empty_focus = self.projects[ix]
                .active_session()
                .is_none_or(|chat| chat.items.is_empty());
            // Every project has one main chat. A folder opened for the
            // first time, or one whose chats were all archived, gets it
            // here — once the kernel has said what it already holds.
            if self.projects[ix].main_session().is_none() {
                self.new_session_in(ix, settings::kernel_agent(), None, cx);
            } else if empty_focus
                && let Some(id) = self.projects[ix]
                    .sessions
                    .iter()
                    .filter(|chat| !chat.items.is_empty() && !chat.closed)
                    .max_by_key(|chat| chat.touched())
                    .map(|chat| chat.id)
            {
                self.projects[ix].focus_on(id);
                self.remember_session(ix, id);
            }
            // Attach after merge so an orphan that just gained a kernel id
            // opens a socket even when local history was already longer.
            self.wake_open_sessions_in(ix, cx);
        }
        cx.notify();
    }

    /// Go / Mac ids that this place's listing does not have. They were
    /// injected from another Arbos. Rust agent rows stay — a missed
    /// folder must not wipe a chat the user still has on disk.
    fn drop_foreign_sessions(&mut self, ix: usize, owned: &HashSet<String>) {
        let drop: Vec<u64> = self.projects[ix]
            .sessions
            .iter()
            .filter_map(|chat| {
                let sid = chat.agent_session.as_ref()?;
                if owned.contains(sid) || chat.busy() || !kernel::go_kernel_id(sid) {
                    return None;
                }
                Some(chat.id)
            })
            .collect();
        if drop.is_empty() {
            return;
        }
        let project = &mut self.projects[ix];
        for id in drop {
            Self::forget_session(project, id);
        }
        self.save();
    }

    /// A local chat that never stored the kernel id, or whose stored id
    /// is gone: bind it to this kernel row when the first spoken line
    /// (or the title) matches, so a relaunch does not mint a second row
    /// or open a fresh empty context.
    fn match_orphan(
        sessions: &[ChatSession],
        kernel_ids: &HashSet<String>,
        row: &kernel::SessionSummary,
        items: &[session::ChatItem],
    ) -> Option<u64> {
        let key = session::conversation_key(items, &row.title)?;
        let mut hits = sessions.iter().filter_map(|chat| {
            if let Some(sid) = &chat.agent_session
                && kernel_ids.contains(sid)
            {
                return None;
            }
            let title = chat
                .name
                .as_deref()
                .filter(|name| !name.is_empty())
                .unwrap_or(&chat.title);
            let chat_key = session::conversation_key(&chat.items, title)?;
            (chat_key == key).then_some(chat.id)
        });
        let id = hits.next()?;
        hits.next().is_none().then_some(id)
    }

    /// Point each child at the local session whose kernel id matches the
    /// filed parent. Done after every session is read, so order does not
    /// matter.
    fn resolve_parents(&mut self, ix: usize) {
        // The filed parent is this window's cache of the kernel's record;
        // where the kernel's `agent.md` says otherwise, the kernel wins.
        // A parentless agent the file had under root stays a chat of its
        // own and loses its delegate number (F-137).
        let place = self.projects[ix].place();
        for chat in &mut self.projects[ix].sessions {
            let Some(sid) = chat.agent_session.as_deref() else {
                continue;
            };
            if let Some(kernel_parent) = kernel::agent_parent(&place, sid)
                && chat.parent_kernel != kernel_parent
            {
                eprintln!(
                    "session {} ({sid}): filed parent {:?}, the kernel's record says {:?}; the record wins",
                    chat.id, chat.parent_kernel, kernel_parent
                );
                chat.parent_kernel = kernel_parent;
                if chat.parent_kernel.is_none() {
                    chat.delegate_number = None;
                }
                chat.flush();
            }
        }
        let by_kernel: HashMap<String, u64> = self.projects[ix]
            .sessions
            .iter()
            .filter_map(|chat| chat.agent_session.clone().map(|sid| (sid, chat.id)))
            .collect();
        let wanted: Vec<(u64, u64)> = self.projects[ix]
            .sessions
            .iter()
            .filter(|chat| chat.parent.is_none())
            .filter_map(|chat| {
                let parent = chat
                    .parent_kernel
                    .as_ref()
                    .and_then(|sid| by_kernel.get(sid).copied())?;
                Some((chat.id, parent))
            })
            .collect();
        for (id, parent) in wanted {
            // A filed parent that is this chat's own descendant would close
            // the tree into a ring; the file is wrong, the link stays off.
            if !self.projects[ix].can_parent(id, parent) {
                eprintln!("session {id}: parent {parent} would make a ring; left unparented");
                continue;
            }
            if let Some(chat) = self.projects[ix].session_mut(id) {
                chat.parent = Some(parent);
            }
        }
        self.number_delegates(ix);
    }

    fn number_delegates(&mut self, ix: usize) {
        let sessions = &mut self.projects[ix].sessions;
        for index in ChatSession::number_delegates(sessions) {
            sessions[index].flush();
        }
    }

    /// ChatView pencil: put this prompt in the composer. The user edits
    /// and sends; this does not resubmit on its own.
    pub fn edit_in_composer(&mut self, text: String, cx: &mut Context<Self>) {
        self.pending_composer = Some(text);
        cx.notify();
    }

    /// Send to a session, attaching to the kernel when it has no socket —
    /// typing into a session read back from disk is what picks it up again.
    pub fn send(&mut self, id: u64, content: impl Into<Prompt>, cx: &mut Context<Self>) {
        let content = content.into();
        // Read before `chat` borrows the projects. This is the second
        // place a kernel socket opens.
        let found = self
            .projects
            .iter_mut()
            .find_map(|project| project.session_mut(id));
        let Some(chat) = found else {
            return;
        };
        // The place's folder is not where the window knew it — renamed,
        // moved or deleted under the kernel, which stopped itself. This
        // read as "archived" and dropped the words without a word (QA
        // `af-03`, the fourth swallowed-message path). Name the path it
        // expected, keep the line on the pane and in the queue: the next
        // attach — when the folder is back or the project is reopened
        // from its new place — sends it.
        if chat.place_gone() {
            let path = chat.cwd.display().to_string();
            chat.hold_offline(content);
            if !chat.has_place_gone_notice() {
                chat.notice(
                    true,
                    &format!(
                        "{}: expected {path}. Your line is kept and goes when the folder is back or the project is reopened.",
                        session::PLACE_GONE
                    ),
                );
            }
            chat.flush();
            cx.notify();
            return;
        }
        // A worker the kernel archived has no agent to speak to; its
        // transcript stays to read. Say so instead of holding the words.
        if chat.agent_gone() {
            // "Archived" only when the kernel's archive holds it; a folder
            // that is simply missing is said to be missing, with its path.
            let sid = chat.agent_session.clone().unwrap_or_default();
            let archived = chat
                .cwd
                .join(".arbos")
                .join("archive")
                .join("agents")
                .join(&sid)
                .is_dir();
            let text = if archived {
                "this agent is archived: its history stays, but it takes no more messages".to_owned()
            } else {
                format!(
                    "this agent's folder is gone: expected {}. Your line was not sent.",
                    chat.cwd.join(".arbos").join("agents").join(&sid).display()
                )
            };
            chat.notice(true, &text);
            chat.flush();
            cx.notify();
            return;
        }
        if chat.closed {
            chat.closed = false;
        }
        chat.reap_dead_socket();
        if chat.idle() && chat.resumable() {
            chat.resume(cx);
        }
        if !content.is_empty() {
            // Lines kept while the place was unreachable go first, in the
            // order they were typed; the new one takes its place behind
            // them (`af-03`: "back again" was answered before the two lines
            // typed into the moved folder).
            if chat.has_held_lines() {
                chat.hold_offline(content);
                chat.drain();
            } else {
                chat.send(content);
            }
        }
        if chat.idle() && chat.resumable() && !chat.queue.is_empty() {
            chat.resume(cx);
        }
        let file = chat.file.clone();
        if let (Some(file), Some(ix)) = (file, self.project_of(id)) {
            self.remember(
                ix,
                state::Kind::Session,
                file.to_string_lossy().into_owned(),
            );
        }
        cx.notify();
    }

    /// Hold `content` for the next turn on this chat: the kernel keeps it
    /// and runs it when the turn in flight ends. See
    /// [`ChatSession::queue_next`].
    pub fn queue_next(&mut self, id: u64, content: impl Into<Prompt>, cx: &mut Context<Self>) {
        let content = content.into();
        let found = self
            .projects
            .iter_mut()
            .find_map(|project| project.session_mut(id));
        let Some(chat) = found else {
            return;
        };
        if chat.closed {
            chat.closed = false;
        }
        chat.reap_dead_socket();
        if chat.idle() && chat.resumable() {
            chat.resume(cx);
        }
        chat.queue_next(content);
        let file = chat.file.clone();
        if let (Some(file), Some(ix)) = (file, self.project_of(id)) {
            self.remember(
                ix,
                state::Kind::Session,
                file.to_string_lossy().into_owned(),
            );
        }
        cx.notify();
    }

    /// Stop the in-flight turn. If the attach socket died, say Stopped
    /// and attach again so the next send works — no kernel jargon.
    /// How many automatic reconnects a chat gets before it waits for a
    /// hand.
    pub const RECONNECT_TRIES: u32 = 30;
    /// How often a fault no retry mends is looked at again.
    pub const RECHECK_SECS: u64 = 60;

    /// The connection failed or dropped: try again after 2, 4, 8, 16, 32,
    /// then 60 s, up to [`Self::RECONNECT_TRIES`] times — a first start
    /// that lost the spawn race to another row, a remote tunnel, a local
    /// kernel that stopped. The row under the composer counts down; a Send
    /// or Stop meanwhile tries at once.
    ///
    /// `slow` is the fault no retry mends — no kernel binary, a path that
    /// is not a directory: one look every [`Self::RECHECK_SECS`], not
    /// counted against the tries, so the tab is never dead but never
    /// hammers either. The reason stays on the bar meanwhile.
    pub fn schedule_reconnect(&mut self, id: u64, slow: bool, cx: &mut Context<Self>) {
        let Some(chat) = self.session_mut(id) else {
            return;
        };
        if !matches!(chat.connection, crate::model::session::Connection::Lost) {
            return;
        }
        // A timer already waits for this very connection generation.
        if chat.reconnect_at.is_some() && chat.reconnect_gen == chat.attach_gen {
            return;
        }
        if !slow && chat.reconnect_attempt >= Self::RECONNECT_TRIES {
            let why = chat
                .connect_fault
                .clone()
                .map(|why| format!(" ({why})"))
                .unwrap_or_default();
            chat.notice(
                true,
                &format!(
                    "connection lost{why}; retries stopped — send a message or press Reconnect to try again"
                ),
            );
            chat.flush();
            return;
        }
        let delay = if slow {
            Duration::from_secs(Self::RECHECK_SECS)
        } else {
            chat.reconnect_attempt += 1;
            let attempt = chat.reconnect_attempt;
            Duration::from_secs(2u64.saturating_pow(attempt.min(6)).min(60))
        };
        chat.reconnect_at = Some(std::time::Instant::now() + delay);
        chat.reconnect_gen = chat.attach_gen;
        let generation = chat.attach_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |workspace, cx| {
                let go = workspace.session_mut(id).is_some_and(|chat| {
                    if chat.attach_gen != generation {
                        // A newer generation owns the retry now.
                        return false;
                    }
                    chat.reconnect_at = None;
                    chat.resumable()
                        && matches!(chat.connection, crate::model::session::Connection::Lost)
                });
                if go {
                    if let Some(chat) = workspace.session_mut(id) {
                        chat.resume(cx);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub fn cancel(&mut self, id: u64, cx: &mut Context<Self>) {
        let found = self
            .projects
            .iter_mut()
            .find_map(|project| project.session_mut(id));
        let Some(chat) = found else {
            return;
        };
        chat.reap_dead_socket();
        chat.cancel();
        if chat.idle() && chat.resumable() {
            chat.resume(cx);
        }
        cx.notify();
    }

    /// Close the connection and keep the transcript. The row stays where it
    /// was, readable, and typing into it opens an agent again.
    /// Put a session away, or bring it back. Closing tears the agent down and
    /// keeps the transcript; opening it again is what reconnects.
    pub fn archive_session(&mut self, id: u64, archived: bool, cx: &mut Context<Self>) {
        self.with_session(id, cx, |chat| match archived {
            true => chat.close(),
            false => chat.closed = false,
        });
    }

    pub fn rename_session(&mut self, id: u64, name: String, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        let place = self.project_of(id).map(|ix| self.projects[ix].place());
        let sid = self.session(id).and_then(|chat| chat.agent_session.clone());
        self.with_session(id, cx, |chat| {
            chat.name = (!name.is_empty()).then(|| name.clone());
            chat.flush();
        });
        if let (Some(place), Some(sid)) = (place, sid) {
            kernel::rename_session(&place, &sid, &name);
        }
    }

    /// Markdown that names this chat and points at it. Paste into a transcript
    /// to get a chip; paste elsewhere to grep `arbos://` and the place.
    pub fn chat_link(&self, id: u64) -> Option<String> {
        let chat = self.session(id)?;
        let place = self
            .project_of(id)
            .map(|ix| self.projects[ix].place().encode())?;
        let title = escape_md_label(&self.display_label(id));
        let mut url = format!("arbos://chat/{id}?p={}", encode_query(&place));
        if let Some(file) = chat
            .file
            .as_ref()
            .and_then(|file| file.file_name())
            .and_then(|name| name.to_str())
        {
            url.push_str("&file=");
            url.push_str(&encode_query(file));
        }
        Some(format!("[{title}]({url} \"chip\")"))
    }

    pub fn copy_chat_link(&self, id: u64, cx: &mut Context<Self>) {
        if let Some(text) = self.chat_link(id) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Copy the focused chat's link — ⌃C when no transcript range is selected.
    pub fn copy_active_chat(&self, cx: &mut Context<Self>) {
        if let Some(id) = self.active_id() {
            self.copy_chat_link(id, cx);
        }
    }

    /// Paste a copied chat: clone it into a new root in this project.
    pub fn paste_chat(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let Some(target) = parse_clipboard_chat(&text) else {
            return;
        };
        let Some(id) = self.resolve_chat(&target) else {
            return;
        };
        self.fork_session(id, cx);
    }

    /// Thumbs on a turn's answer. Stored on the prompt that started the
    /// turn (so it reopens lit) and appended to the agent's
    /// `feedback.jsonl` in the place, for QA and the kernel. Clicking the
    /// lit thumb clears the vote.
    /// Try Live (A-02): open or close the live view of the agent's screen.
    /// While open, the kernel is asked for a frame every two seconds; for
    /// an agent on another machine the kernel forwards the request over
    /// its link, so the window sees that machine's screen.
    pub fn toggle_live(&mut self, id: u64, cx: &mut Context<Self>) {
        let mut opened = false;
        self.with_session(id, cx, |chat| {
            chat.live_open = !chat.live_open;
            opened = chat.live_open;
            if opened {
                chat.request_screen();
            }
        });
        if !opened {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let keep = this
                    .update(cx, |workspace, cx| {
                        let mut open = false;
                        workspace.with_session(id, cx, |chat| {
                            open = chat.live_open;
                            if open {
                                chat.request_screen();
                            }
                        });
                        open
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    /// "Rewind here" under a turn: chat and files back to before its prompt.
    pub fn rewind_turn(&mut self, id: u64, turn: usize, cx: &mut Context<Self>) {
        self.with_session(id, cx, |chat| chat.rewind(turn, true));
    }

    pub fn vote_turn(&mut self, id: u64, turn: usize, value: i8, cx: &mut Context<Self>) {
        let mut line: Option<serde_json::Value> = None;
        self.with_session(id, cx, |chat| {
            let answer = transcript::answer_of(&chat.items, turn).unwrap_or_default();
            // The footer's turn may start at a `From` block; the vote goes
            // on the user prompt that began it.
            let Some(start) = chat
                .items
                .iter()
                .take(turn + 1)
                .rposition(|item| matches!(item, ChatItem::User(_)))
            else {
                return;
            };
            let Some(ChatItem::User(message)) = chat.items.get_mut(start) else {
                return;
            };
            let next = if message.feedback == Some(value) {
                None
            } else {
                Some(value)
            };
            message.feedback = next;
            chat.flush();
            if chat.host.is_none() {
                if let Some(agent) = chat.agent_session.as_deref() {
                    line = Some(serde_json::json!({
                        "ts": arbos_core::now_ms(),
                        "agent": agent,
                        "turn": turn,
                        "vote": next.unwrap_or(0),
                        "answer": answer.chars().take(200).collect::<String>(),
                    }));
                    let path = chat
                        .cwd
                        .join(".arbos")
                        .join("agents")
                        .join(agent)
                        .join("feedback.jsonl");
                    if let Some(v) = &line {
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                        {
                            use std::io::Write as _;
                            let _ = writeln!(f, "{v}");
                        }
                    }
                }
            }
        });
        cx.notify();
    }

    /// Whole-log copy of a chat as a new root. The source stays where it is.
    pub fn fork_session(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self.project_of(id) else {
            return;
        };
        let Some(sid) = self
            .projects
            .get(ix)
            .and_then(|project| project.session(id))
            .and_then(|chat| chat.agent_session.clone())
        else {
            return;
        };
        let place = self.projects[ix].place();
        let title = self.display_label(id);
        // A fork taken mid-run copies the prompt the original is still
        // answering; the copy says so under it (Jacob, 2026-09-17-1).
        let source_busy = self.session(id).is_some_and(|chat| chat.busy());
        cx.spawn(async move |this, cx| {
            let cloned = cx
                .background_executor()
                .spawn(async move { kernel::clone_session(&place, &sid) })
                .await;
            let _ = this.update(cx, |workspace, cx| match cloned {
                Ok(new_sid) => workspace.adopt_clone(ix, &new_sid, &title, source_busy, cx),
                Err(err) => {
                    workspace.with_session(id, cx, |chat| {
                        chat.notice(true, &format!("could not fork: {err:#}"));
                    });
                }
            });
        })
        .detach();
    }

    fn adopt_clone(
        &mut self,
        ix: usize,
        kernel_id: &str,
        source_title: &str,
        source_busy: bool,
        cx: &mut Context<Self>,
    ) {
        if self.projects.get(ix).is_some_and(|project| {
            project
                .sessions
                .iter()
                .any(|chat| chat.agent_session.as_deref() == Some(kernel_id))
        }) {
            if let Some(id) = self.projects[ix]
                .sessions
                .iter()
                .find(|chat| chat.agent_session.as_deref() == Some(kernel_id))
                .map(|chat| chat.id)
            {
                self.select_session(id, cx);
            }
            return;
        }
        let Some(entry) = self.preferred_agent() else {
            return;
        };
        let place = self.projects[ix].place();
        let replay = kernel::session_history(&place, kernel_id).unwrap_or_default();
        let local = self.next_id;
        self.next_id += 1;
        let name = format!("{source_title} copy");
        let mut chat = ChatSession::from_kernel(
            local,
            place,
            entry,
            kernel_id.to_owned(),
            source_title.to_owned(),
            Some(name),
            replay.items,
            SystemTime::now(),
        );
        if let Some(model) = replay.model {
            chat.model = Some(model);
        }
        // The copy ends on a prompt the original is still answering: with
        // nothing under it and an idle composer it read as a chat that
        // never replied ("Forked the chat mid run no response from sub
        // agent", Jacob's first report). Say what it is, once.
        let unanswered_tail = matches!(chat.items.last(), Some(ChatItem::User(_)));
        if source_busy && unanswered_tail {
            chat.notice(false, FORKED_MID_TURN);
        }
        chat.rank = self.projects[ix].front_rank(None);
        chat.flush();
        self.projects[ix].sessions.push(chat);
        self.select_session(local, cx);
    }

    /// Open a chat from an `arbos://chat/…` link. True when the URL is ours,
    /// whether or not that chat is still in the workspace.
    pub fn open_chat_link(&mut self, url: &str, cx: &mut Context<Self>) -> bool {
        let Some(target) = parse_chat_link(url) else {
            return false;
        };
        if let Some(id) = self.resolve_chat(&target) {
            self.select_session(id, cx);
        }
        true
    }

    fn resolve_chat(&self, target: &ChatRef) -> Option<u64> {
        let by_file = |project: &Project| {
            let file = target.file.as_deref()?;
            project.sessions.iter().find_map(|chat| {
                let name = chat.file.as_ref()?.file_name()?.to_str()?;
                (name == file).then_some(chat.id)
            })
        };
        let by_id = |project: &Project| target.id.filter(|&id| project.session(id).is_some());
        let scoped = target.place.as_ref().map(|place| {
            self.projects
                .iter()
                .filter(|project| project.place().encode() == *place)
        });
        if let Some(projects) = scoped {
            if let Some(id) = projects.clone().find_map(by_file) {
                return Some(id);
            }
            if let Some(id) = projects.clone().find_map(by_id) {
                return Some(id);
            }
        }
        self.projects
            .iter()
            .find_map(by_file)
            .or_else(|| self.projects.iter().find_map(by_id))
    }

    /// Put a face on the project at `ix` — name, glyph, colour — and file
    /// it in the folder's `.arbos/project.toml`. An empty name clears it
    /// back to the folder's own.
    pub fn set_identity(&mut self, ix: usize, mut identity: Identity, cx: &mut Context<Self>) {
        identity.name = identity.label().map(str::to_string);
        if let Some(project) = self.projects.get_mut(ix) {
            project.set_identity(identity);
        }
        cx.notify();
    }

    /// Remove a chat from the kernel, then drop the local row. Archive and
    /// closing a pane are not this — only the sidebar Delete is.
    ///
    /// The row leaves the sidebar on this turn. HTTP and ssh run in the
    /// background with a short timeout; a miss still keeps the row gone.
    pub fn delete_session(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self.project_of(id) else {
            return;
        };
        let place = self.projects[ix].place();
        let parent = self.projects[ix].session(id).and_then(|chat| chat.parent);
        let focused = self.projects[ix]
            .focused_agent()
            .is_some_and(|at| at == id || self.projects[ix].ancestor_of(id, at));
        let kids: Vec<u64> = self.projects[ix]
            .sessions
            .iter()
            .filter(|chat| chat.parent == Some(id))
            .map(|chat| chat.id)
            .collect();
        let mut kernel_ids = Vec::new();
        for local in std::iter::once(id).chain(kids.iter().copied()) {
            if let Some(sid) = self.projects[ix]
                .session(local)
                .and_then(|chat| chat.agent_session.clone())
            {
                if !kernel_ids.iter().any(|held| held == &sid) {
                    kernel_ids.push(sid);
                }
            }
        }
        let project = &mut self.projects[ix];
        for sid in &kernel_ids {
            project.dismissed.insert(sid.clone());
        }
        for kid in kids {
            Self::forget_session(project, kid);
        }
        Self::forget_session(project, id);
        if focused {
            let next = parent
                .filter(|&parent| project.session(parent).is_some())
                .or_else(|| {
                    project
                        .roots()
                        .find(|chat| !chat.closed)
                        .map(|chat| chat.id)
                });
            match next {
                Some(next) => project.focus_on(next),
                None => project.focus = None,
            }
        }
        self.remember_dismissed(ix);
        self.save();
        self.push_snapshot(ix);
        cx.notify();
        if !kernel_ids.is_empty() {
            cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .spawn(async move {
                        for sid in kernel_ids {
                            let _ = kernel::delete_session(&place, &sid);
                        }
                    })
                    .await;
            })
            .detach();
        }
    }

    fn remember_dismissed(&mut self, ix: usize) {
        let Some(project) = self.projects.get(ix) else {
            return;
        };
        let key = project.place().encode();
        if project.dismissed.is_empty() {
            self.dismissed.remove(&key);
        } else {
            self.dismissed
                .insert(key, project.dismissed.iter().cloned().collect());
        }
    }

    /// Drop a local session row. The kernel id leaves `dismissed` because
    /// the store delete is what keeps the poll from minting it again.
    fn forget_session(project: &mut crate::model::project::Project, id: u64) {
        if let Some(sid) = project
            .session(id)
            .and_then(|chat| chat.agent_session.clone())
        {
            project.dismissed.remove(&sid);
        }
        if let Some(file) = project.session(id).and_then(|chat| chat.file.clone()) {
            record::remove(&file);
        }
        project.close_surfaces_of(id);
        project.sessions.retain(|chat| chat.id != id);
    }

    /// Any session, in whichever project holds it — the pump that feeds a
    /// session knows only its id, and must not care which tab it sits behind.
    /// Everything this window knows about `place`, for a report to carry.
    ///
    /// Jacob's ruling: package the state needed to debug into the bug itself.
    /// The test is whether a reader could reconstruct what he was looking at and
    /// what each side believed — so this holds the rows as the window has them,
    /// the session records on disk behind them, and where the focus was. The
    /// kernel's own roster and agent list ride in the same bundle, so the two
    /// can be set against each other; a report carrying one side cannot show a
    /// disagreement, and a disagreement is what F-137 was.
    ///
    /// `budget` bounds the on-disk records. They hold whole chats, so their
    /// messages are counted rather than copied and the result says so.
    /// `store` is the place's own local folder — `Project::store()` — and not its
    /// path.
    ///
    /// A remote place's path belongs to the far machine: `ArbosLife:~` has the
    /// path `~`, which is not even absolute here. Reading records from it reads a
    /// relative path against the app's working directory, so a remote report
    /// would carry either nothing or whatever happened to sit there. `store()` is
    /// the local sidecar the desktop already keeps a remote project's records in.
    pub fn desktop_state(&self, store: &std::path::Path, budget: usize) -> serde_json::Value {
        let place = store;
        let project = self
            .projects
            .iter()
            .find(|project| project.store() == store);
        let rows: Vec<serde_json::Value> = project
            .map(|project| project.sessions.iter().map(ChatSession::row_facts).collect())
            .unwrap_or_default();

        // The records are what the window will draw again after a relaunch, so
        // a row that disagrees with the kernel disagrees here too. Their
        // `items` are whole chats; the metadata is the part that explains a
        // wrong row, so the messages are counted and left out.
        let mut records = Vec::new();
        let mut records_clipped = 0usize;
        let mut bytes = 0usize;
        for (path, record) in crate::model::record::list(place) {
            let row = serde_json::json!({
                "file": path.file_name().and_then(|n| n.to_str()),
                "agent": record.agent,
                "session": record.session,
                "parent": record.parent,
                "delegate_number": record.delegate_number,
                "title": record.title,
                "name": record.name,
                "closed": record.closed,
                "rank": record.rank,
                "updated": record.updated,
                "items": record.items.len(),
                "draft_chars": record.draft.chars().count(),
            });
            let size = row.to_string().len();
            if bytes + size > budget {
                records_clipped += 1;
                continue;
            }
            bytes += size;
            records.push(row);
        }

        serde_json::json!({
            "place": place.to_string_lossy(),
            // The rows on screen, and the facts each label and status came from.
            "rows": rows,
            "records": records,
            // Never a silent drop: what went, and why.
            "records_clipped": records_clipped,
            "records_note": if records_clipped > 0 {
                serde_json::Value::String(format!(
                    "{records_clipped} session record(s) left out for size; every record's messages are counted rather than copied"
                ))
            } else {
                serde_json::Value::String(
                    "every session record is here; their messages are counted rather than copied".into(),
                )
            },
            "focus": project.and_then(|p| p.focus.as_ref()).map(|f| serde_json::json!({
                "agent": f.agent,
                "surface": f.surface.map(|s| format!("{s:?}")),
            })),
            "archived_shown": project.map(|p| p.archive_open),
            // The tabs, so "which project was in front" is answerable.
            "tabs": self.projects.iter().map(|p| serde_json::json!({
                "path": p.path.to_string_lossy(),
                "host": p.host,
                "sessions": p.sessions.len(),
            })).collect::<Vec<_>>(),
            "active_tab": self.active,
        })
    }

    pub fn session(&self, id: u64) -> Option<&ChatSession> {
        self.projects.iter().find_map(|project| project.session(id))
    }

    pub fn session_mut(&mut self, id: u64) -> Option<&mut ChatSession> {
        self.projects
            .iter_mut()
            .find_map(|project| project.session_mut(id))
    }

    /// Sidebar and empty-state name. Untitled chats in one project are
    /// numbered so two empty rows are not both "New chat".
    pub fn display_label(&self, id: u64) -> String {
        let Some(chat) = self.session(id) else {
            return "New chat".into();
        };
        let untitled = |chat: &ChatSession| {
            !chat.is_delegate()
                && chat.name.as_deref().is_none_or(|n| {
                    arbos_core::chattitle::is_generic(n, chat.agent_session.as_deref())
                })
                && (chat.title.is_empty()
                    || arbos_core::chattitle::is_generic(
                        &chat.title,
                        chat.agent_session.as_deref(),
                    ))
                && chat.items.is_empty()
        };
        if !untitled(chat) {
            return chat.label();
        }
        let Some(ix) = self.project_of(id) else {
            return chat.label();
        };
        let mut ids: Vec<u64> = self.projects[ix]
            .sessions
            .iter()
            .filter(|chat| untitled(chat) && !chat.closed)
            .map(|chat| chat.id)
            .collect();
        ids.sort_unstable();
        if ids.len() <= 1 {
            return "New chat".into();
        }
        let n = ids.iter().position(|&held| held == id).unwrap_or(0) + 1;
        format!("New chat {n}")
    }

    /// Run `f` on the session (when it still exists) and repaint.
    pub fn with_session(
        &mut self,
        id: u64,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut ChatSession),
    ) {
        let found = self
            .projects
            .iter_mut()
            .find_map(|project| project.session_mut(id));
        if let Some(chat) = found {
            f(chat);
            cx.notify();
        }
    }

    /// Put what the transcript has selected on the clipboard — see
    /// [`crate::view::component::transcript::State::copied`].
    ///
    /// Answers whether there was anything, so a `cmd-c` that finds no selection
    /// can be left to whatever else wanted it.
    pub fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self
            .active_session()
            .and_then(|chat| chat.transcript.copied(chat))
        else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    /// Switch a session's mode — what the composer's mode picker reports.
    /// See [`ChatSession::set_mode`].
    pub fn set_session_mode(&mut self, id: u64, mode_id: String, cx: &mut Context<Self>) {
        self.with_session(id, cx, |chat| chat.set_mode(&mode_id));
    }

    /// Plan mode with approval (P-10): the agent wrote its checklist in
    /// plan mode (read-only); the user approves, so the mode becomes auto
    /// and the next turn executes the list.
    pub fn approve_plan(&mut self, id: u64, cx: &mut Context<Self>) {
        self.with_session(id, cx, |chat| chat.set_mode("auto"));
        self.send(
            id,
            "Plan approved. Execute your checklist now: work the open items in order, check each off with plan check and a one-line readout as you go, and report when done.".to_string(),
            cx,
        );
    }

    /// Switch the model later turns run on. See [`ChatSession::set_model`].
    pub fn set_session_model(&mut self, id: u64, model: String, cx: &mut Context<Self>) {
        self.models.current = model.clone();
        self.with_session(id, cx, |chat| chat.set_model(&model));
    }

    pub fn undo_session(&mut self, id: u64, cx: &mut Context<Self>) {
        self.with_session(id, cx, |chat| chat.undo_checkpoint());
    }

    /// The same for a config option, which is where the model lives.
    pub fn set_session_config(
        &mut self,
        id: u64,
        config_id: String,
        value: SessionConfigOptionValue,
        cx: &mut Context<Self>,
    ) {
        self.with_session(id, cx, |chat| chat.set_config(&config_id, value));
    }

    /// The session reached an agent: send it whatever was typed while it had
    /// none, and write the agent's own id down so a later launch can load the
    /// conversation back.
    pub fn session_connected(&mut self, id: u64, cx: &mut Context<Self>) {
        self.with_session(id, cx, |chat| {
            // The kernel's copy first: a "reconnected" line is this
            // window's to say, not a transcript record to seed. And what it
            // wrote while no window was attached comes in before anything
            // live does (F-105).
            chat.adopt_kernel_tail();
            chat.sync_kernel_history();
            if chat.reconnect_attempt > 0 {
                chat.notice(false, "reconnected");
            }
            chat.reconnect_attempt = 0;
            chat.reconnect_at = None;
            chat.connect_fault = None;
            // What the agent is on right now, from its status file, so a
            // fresh attach draws the line without waiting for a frame.
            if chat.host.is_none()
                && let Some(sid) = chat.agent_session.as_deref()
            {
                chat.status = arbos_core::status::read(&arbos_core::Place::new(&chat.cwd), sid)
                    .map(|s| s.step);
            }
            // Whatever was typed while the connection was down goes now, in order.
            chat.drain();
            chat.flush();
        });
        if let Some(ix) = self.project_of(id) {
            self.stamp_children(ix, id);
            self.resolve_parents(ix);
            self.push_snapshot(ix);
        }
        cx.notify();
    }

    /// A sub-chat opened under a parent that had no kernel id yet (⌘N on a
    /// fresh main chat) was filed without one. Now the parent has it, write
    /// it on each child so a relaunch nests them again.
    fn stamp_children(&mut self, ix: usize, parent: u64) {
        let Some(parent_kernel) = self.projects[ix]
            .session(parent)
            .and_then(|chat| chat.agent_session.clone())
        else {
            return;
        };
        for chat in &mut self.projects[ix].sessions {
            if chat.parent == Some(parent) && chat.parent_kernel.is_none() {
                chat.parent_kernel = Some(parent_kernel.clone());
                chat.flush();
            }
        }
    }

    /// The session the chat pane would show. Gated, and it is the gate that
    /// matters most: sessions are read back off disk when a project opens,
    /// whatever the switch says, so without this a filed transcript would put
    /// the pane on screen with no composer under it.
    pub fn active_session(&self) -> Option<&ChatSession> {
        self.active_project()?.active_session()
    }

    pub fn active_id(&self) -> Option<u64> {
        self.active_project()
            .and_then(|project| project.focused_agent())
    }

    pub fn active_surface(&self) -> Option<&Surface> {
        let project = self.active_project()?;
        let id = project.focus?.surface?;
        project.surface(id)
    }

    /// Put a surface in the column. The parent agent stays the focused
    /// agent, so its children stay listed.
    pub fn select_surface(&mut self, id: SurfaceId, cx: &mut Context<Self>) {
        let Some(ix) = self
            .projects
            .iter()
            .position(|project| project.surface(id).is_some())
        else {
            return;
        };
        let Some(owner) = self.projects[ix]
            .surface(id)
            .and_then(|surface| surface.owner)
            .or_else(|| self.projects[ix].focused_agent())
        else {
            return;
        };
        self.projects[ix].focus_surface(owner, id);
        self.active = Some(ix);
        self.push_snapshot(ix);
        self.wake_session(owner, cx);
        cx.notify();
    }

    pub fn close_surface(&mut self, id: SurfaceId, cx: &mut Context<Self>) {
        let Some(ix) = self
            .projects
            .iter()
            .position(|project| project.surface(id).is_some())
        else {
            return;
        };
        let project = &mut self.projects[ix];
        if project.focus.is_some_and(|focus| focus.surface == Some(id)) {
            if let Some(focus) = &mut project.focus {
                focus.surface = None;
            }
        }
        project.surfaces.retain(|surface| surface.id != id);
        self.push_snapshot(ix);
        cx.notify();
    }

    /// Cursor's Review: the working tree's diff (one file, or all of it),
    /// written under the project's desktop folder and opened in the column
    /// as code.
    pub fn review_changes(
        &mut self,
        root: &std::path::Path,
        file: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let text = crate::model::changes::GitChanges::diff_text(root, file);
        let path = crate::model::changes::GitChanges::review_path(root, file);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&path, text).is_err() {
            return;
        }
        let title = match file {
            Some(f) => format!("{f} · changes"),
            None => "Changes".to_string(),
        };
        let Some(owner) = self.active_id() else {
            return;
        };
        self.open_shown(
            owner,
            path.to_string_lossy().into_owned(),
            title,
            "code".to_string(),
            None,
            None,
            cx,
        );
    }

    /// Something the agent put under its chat: a file it `show`ed, or a
    /// terminal, browser, or process the kernel opened. For those, `path`
    /// is the kernel's id (`t1`, `b1`, `j3`); `url` is a page address or a
    /// job's log file. The new row takes the column.
    pub fn open_shown(
        &mut self,
        owner: u64,
        path: String,
        title: String,
        kind: String,
        cwd: Option<String>,
        url: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.project_of(owner) else {
            return;
        };
        let board_kind = match kind.as_str() {
            "dir" => "files",
            other => other,
        };
        let surface_kind = SurfaceKind::from_board(board_kind);
        let title = if title.is_empty() {
            match surface_kind {
                SurfaceKind::Terminal => "Terminal".into(),
                SurfaceKind::Browser => url
                    .as_deref()
                    .and_then(surface::host_of)
                    .unwrap_or("Browser")
                    .to_owned(),
                SurfaceKind::Process => "Process".into(),
                SurfaceKind::Panel => PathBuf::from(&path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| board_kind.to_owned()),
            }
        } else {
            title
        };
        let bind = match surface_kind {
            SurfaceKind::Terminal => Bind::Terminal { id: path, cwd },
            SurfaceKind::Browser => {
                // A reopen keeps the picture the page already had.
                let shot = self.projects[ix]
                    .surfaces
                    .iter()
                    .find(|surface| {
                        surface.owner == Some(owner) && surface.kernel_id() == Some(&path)
                    })
                    .and_then(|surface| match &surface.bind {
                        Bind::Browser { shot, .. } => shot.clone(),
                        _ => None,
                    });
                Bind::Browser {
                    id: path,
                    url: url.unwrap_or_default(),
                    shot,
                }
            }
            SurfaceKind::Process => Bind::Process {
                log: url
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.job_log(ix, owner, &path)),
                id: path,
                live: String::new(),
                done: None,
            },
            SurfaceKind::Panel => Bind::Path(PathBuf::from(path)),
        };
        let id = self.upsert_surface(
            ix,
            Some(owner),
            surface_kind,
            title,
            bind,
            board_kind,
            |surface, bind| match (&surface.bind, bind) {
                (Bind::Path(held), Bind::Path(next)) => {
                    held == next && surface.board_kind == board_kind
                }
                (Bind::Terminal { .. }, Bind::Terminal { .. })
                | (Bind::Browser { .. }, Bind::Browser { .. })
                | (Bind::Process { .. }, Bind::Process { .. }) => {
                    surface.kernel_id().is_some() && surface.kernel_id() == bind.kernel_id()
                }
                _ => false,
            },
        );
        // The surface comes to the column for the chat that is in front. A
        // worker's terminal or job opening under its parent's turn goes to
        // the panel's Processes and stays there: the view does not jump
        // from the conversation to a sub-agent's shell.
        let in_front = self.projects[ix]
            .focused_agent()
            .is_none_or(|focused| focused == owner);
        // A process row never takes the column on its own: the kernel opens
        // one for any command past twenty seconds (#362), and a board over
        // the chat, composer gone, is not what a person mid-sentence wants
        // — Jacob's screen would have swapped to `python3 bubble_sort.py`
        // while he typed. It lands in the panel's Processes, one click away.
        let takes_column = in_front && surface_kind != SurfaceKind::Process;
        if takes_column {
            self.projects[ix].focus_surface(owner, id);
            if self.active == Some(ix) {
                cx.emit(PaneRequest::Surface(id));
            }
        }
        self.push_snapshot(ix);
        cx.notify();
    }

    /// The kernel closed a row it opened: the shell exited, the job
    /// ended, the page was dropped. If it filled the column, the chat
    /// takes it back.
    pub fn close_shown(&mut self, owner: u64, kernel_id: &str, cx: &mut Context<Self>) {
        let Some(ix) = self.project_of(owner) else {
            return;
        };
        let Some(id) = self.projects[ix]
            .surfaces
            .iter()
            .find(|surface| surface.owner == Some(owner) && surface.kernel_id() == Some(kernel_id))
            .map(|surface| surface.id)
        else {
            return;
        };
        let was_front = self.projects[ix]
            .focus
            .is_some_and(|focus| focus.surface == Some(id));
        self.close_surface(id, cx);
        if was_front && self.active == Some(ix) {
            cx.emit(PaneRequest::Chat);
        }
    }

    /// A detached job ended: the kernel closes its board row, but the
    /// output stays readable here. The row keeps its streamed tail and
    /// exit status until the user closes it; only the oldest finished rows
    /// go when more than `KEEP_FINISHED` of an owner's have piled up.
    pub fn finish_shown_process(&mut self, owner: u64, kernel_id: &str, cx: &mut Context<Self>) {
        const KEEP_FINISHED: usize = 3;
        let Some(ix) = self.project_of(owner) else {
            return;
        };
        let project = &mut self.projects[ix];
        let Some(pos) = project.surfaces.iter().position(|surface| {
            surface.owner == Some(owner) && surface.kernel_id() == Some(kernel_id)
        }) else {
            return;
        };
        if let Bind::Process { done, log, .. } = &mut project.surfaces[pos].bind
            && done.is_none()
        {
            // The job frame with `running: false` usually came first; when
            // it did not (a remote place, a missed tick), read the exit the
            // wrapper shell wrote.
            let code = log
                .parent()
                .and_then(|dir| std::fs::read_to_string(dir.join("exit")).ok())
                .and_then(|s| s.trim().parse::<i32>().ok());
            *done = Some(code);
        }
        let mut finished: Vec<(u128, SurfaceId)> = project
            .surfaces
            .iter()
            .filter(|s| s.owner == Some(owner))
            .filter_map(|s| match &s.bind {
                Bind::Process { done: Some(_), .. } => Some((s.touched, s.id)),
                _ => None,
            })
            .collect();
        finished.sort_by_key(|(touched, _)| *touched);
        let extra: Vec<SurfaceId> = finished
            .iter()
            .take(finished.len().saturating_sub(KEEP_FINISHED))
            .map(|(_, id)| *id)
            .collect();
        for id in extra {
            self.close_surface(id, cx);
        }
        self.push_snapshot(ix);
        cx.notify();
    }

    /// The agent's browser page moved, or sent a picture. Creates the row
    /// if the open was missed; never takes the column on its own.
    /// New output from one of `owner`'s detached jobs. Appended to the
    /// process row's streamed tail (capped), so the row shows the job as it
    /// runs — on a remote place too, where the journal file is out of reach.
    pub fn job_output(
        &mut self,
        owner: u64,
        job: String,
        delta: String,
        running: bool,
        exit: Option<i32>,
        cx: &mut Context<Self>,
    ) {
        const LIVE_CAP: usize = 64 * 1024;
        if !delta.is_empty()
            && let Some(chat) = self.session_mut(owner)
        {
            chat.mark_progress();
        }
        let Some(ix) = self.project_of(owner) else {
            return;
        };
        let held = self.projects[ix]
            .surfaces
            .iter_mut()
            .find(|surface| surface.owner == Some(owner) && surface.kernel_id() == Some(&job));
        let Some(surface) = held else {
            return;
        };
        let Bind::Process { live, done, .. } = &mut surface.bind else {
            return;
        };
        live.push_str(&delta);
        if live.len() > LIVE_CAP {
            let cut = live.len() - LIVE_CAP;
            let at = live
                .char_indices()
                .map(|(i, _)| i)
                .find(|&i| i >= cut)
                .unwrap_or(cut);
            live.replace_range(..at, "");
        }
        if !running {
            *done = Some(exit);
        }
        cx.notify();
    }

    pub fn browser_moved(
        &mut self,
        owner: u64,
        page: String,
        url: String,
        screenshot: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.project_of(owner) else {
            return;
        };
        let shot = screenshot
            .as_deref()
            .and_then(|b64| STANDARD.decode(b64).ok())
            .map(|bytes| Arc::new(Image::from_bytes(ImageFormat::Png, bytes)));
        let title = surface::host_of(&url).unwrap_or("Browser").to_owned();
        let held = self.projects[ix]
            .surfaces
            .iter_mut()
            .find(|surface| surface.owner == Some(owner) && surface.kernel_id() == Some(&page));
        if let Some(surface) = held {
            if let Bind::Browser {
                url: at,
                shot: last,
                ..
            } = &mut surface.bind
            {
                *at = url;
                if shot.is_some() {
                    *last = shot;
                }
            }
            surface.title = title;
            surface.touched = crate::model::project::stamp();
        } else {
            self.upsert_surface(
                ix,
                Some(owner),
                SurfaceKind::Browser,
                title,
                Bind::Browser {
                    id: page,
                    url,
                    shot,
                },
                "browser",
                |_, _| false,
            );
        }
        cx.notify();
    }

    /// Where a job's journal lives when the frame did not say.
    fn job_log(&self, ix: usize, owner: u64, job: &str) -> PathBuf {
        let project = &self.projects[ix];
        let agent = project
            .session(owner)
            .and_then(|chat| chat.agent_session.clone())
            .unwrap_or_else(|| "root".into());
        project
            .place()
            .path
            .join(".arbos")
            .join("agents")
            .join(agent)
            .join("jobs")
            .join(job)
            .join("out.log")
    }

    /// A kernel child session that belongs under `owner`.
    pub fn ensure_child_agent(
        &mut self,
        owner: u64,
        kernel_id: String,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        if kernel_id.is_empty() {
            return None;
        }
        let ix = self.project_of(owner)?;
        if self.projects[ix].dismissed.contains(&kernel_id) {
            return None;
        }
        // The kernel's record decides. A local agent it has no `agent.md`
        // for does not get a row — the spawn tool names its child before
        // the spawn has returned, and a spawn that failed would otherwise
        // leave a session behind with nothing under it (F-137, Jacob's
        // capture: three session files stamped before the spawn results).
        // The roster brings it round within two seconds once it exists.
        // And a kernel record that names no parent is a chat of its own,
        // not `owner`'s delegate, whatever row was clicked: the
        // "Delegate 1" of F-137 was a parentless `chat-…` adopted under
        // root by a panel click and numbered from then on.
        let place = self.projects[ix].place();
        let owner_kernel = self.projects[ix]
            .session(owner)
            .and_then(|chat| chat.agent_session.clone());
        let kernel_parent = kernel::agent_parent(&place, &kernel_id);
        if place.host.is_none() && kernel_parent.is_none() {
            return None;
        }
        // The kernel's parent, in this window's ids: `owner` when the
        // record agrees with the caller, none when the record says the
        // agent stands alone.
        let parent_of = |workspace: &Self| -> Option<u64> {
            match &kernel_parent {
                None => Some(owner),
                Some(None) => None,
                Some(Some(sid)) if Some(sid) == owner_kernel.as_ref() => Some(owner),
                Some(Some(sid)) => workspace.projects[ix]
                    .sessions
                    .iter()
                    .find(|chat| chat.agent_session.as_deref() == Some(sid.as_str()))
                    .map(|chat| chat.id)
                    .or(Some(owner)),
            }
        };
        if let Some(id) = self.projects[ix]
            .sessions
            .iter()
            .find(|chat| chat.agent_session.as_deref() == Some(&kernel_id))
            .map(|chat| chat.id)
        {
            let parent = parent_of(self);
            let parent_kernel = match &kernel_parent {
                Some(parent) => parent.clone(),
                None => owner_kernel.clone(),
            };
            let mut dirty = false;
            let ring = parent.is_some_and(|parent| !self.projects[ix].can_parent(id, parent));
            if ring {
                eprintln!(
                    "session {id} ({kernel_id}) listed as a child of {owner}, its own descendant; the link stays as it was"
                );
            }
            if let Some(chat) = self.projects[ix].session_mut(id) {
                if !ring && chat.parent != parent {
                    chat.parent = parent;
                    // A delegate's number belongs to a delegate.
                    if parent.is_none() {
                        chat.delegate_number = None;
                    }
                    dirty = true;
                }
                if chat.parent_kernel != parent_kernel {
                    chat.parent_kernel = parent_kernel;
                    dirty = true;
                }
                // The kernel lists it as a live child; an old reap or a
                // relaunch must not keep it hidden.
                if chat.closed && !kernel::go_kernel_id(&kernel_id) {
                    chat.closed = false;
                    dirty = true;
                }
                if dirty {
                    chat.flush();
                }
            }
            if dirty {
                self.number_delegates(ix);
                self.push_snapshot(ix);
                cx.notify();
            }
            self.wake_session(id, cx);
            return Some(id);
        }
        let entry = self.projects[ix]
            .session(owner)
            .map(|chat| chat.entry.clone())
            .unwrap_or_else(settings::kernel_agent);
        let parent = parent_of(self);
        let parent_kernel = match &kernel_parent {
            Some(parent) => parent.clone(),
            None => owner_kernel.clone(),
        };
        let id = self.next_id;
        self.next_id += 1;
        // The kernel named the child after its brief; the row says that,
        // not "Delegate N", from the first frame.
        let name = kernel::agent_name(&place, &kernel_id);
        let brief = kernel::agent_brief(&place, &kernel_id);
        let flags = kernel::agent_flags(&place, &kernel_id);
        let mut chat = ChatSession::adopt(
            id,
            entry,
            place.clone(),
            kernel_id,
            parent,
            parent_kernel,
            cx,
        );
        chat.name = name;
        if let Some((readonly, kind)) = flags {
            chat.readonly = readonly;
            chat.agent_kind = kind;
        }
        // Cursor shows a subagent's brief as its first card; the kernel
        // wrote it as the worker's first wake.
        if chat.items.is_empty()
            && let Some(brief) = brief
        {
            chat.items
                .push(ChatItem::User(crate::model::attachment::UserMessage::from(
                    brief,
                )));
        }
        chat.rank = self.projects[ix].front_rank(parent);
        self.projects[ix].sessions.push(chat);
        self.number_delegates(ix);
        self.push_snapshot(ix);
        cx.notify();
        Some(id)
    }

    /// A chat's pump drained a batch. A finished delegate that is working
    /// again comes back on the tree; one whose turn just ended is reaped
    /// after [`session::DELEGATE_GRACE`]. Its own idle children are reaped
    /// now — the spawn call they waited on may have just returned.
    pub(crate) fn settle_delegate(&mut self, id: u64, ended: bool, cx: &mut Context<Self>) {
        let Some(ix) = self.project_of(id) else {
            return;
        };
        let Some(chat) = self.projects[ix].session(id) else {
            return;
        };
        let reopen = chat.is_delegate() && chat.closed && chat.busy();
        let delegate = chat.is_delegate();
        if reopen {
            if let Some(chat) = self.projects[ix].session_mut(id) {
                chat.closed = false;
                chat.flush();
            }
            cx.notify();
        }
        if self.reap_delegates(ix, cx) {
            cx.notify();
        }
        // Only a Go delegate (a one-shot run) leaves the tree when its
        // turn ends. A rust-kernel child is an agent folder with a plan of
        // its own: it stays, marked done, until archived — the parent's
        // transcript and the task rail keep pointing at it.
        let one_shot = self.projects[ix]
            .session(id)
            .and_then(|chat| chat.agent_session.as_deref().map(kernel::go_kernel_id))
            .unwrap_or(false);
        if ended && delegate && one_shot {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(session::DELEGATE_GRACE + Duration::from_millis(100))
                    .await;
                let _ = this.update(cx, |workspace, cx| {
                    if workspace.reap_delegate(id, cx) {
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    /// This chat's direct sub-agents, in tree order, as the transcript and
    /// the task rail show them. Finished ones stay: the record of what was
    /// delegated is part of the parent's story.
    pub fn child_summaries(&self, id: u64) -> Vec<session::ChildSummary> {
        let Some(ix) = self.project_of(id) else {
            return Vec::new();
        };
        let project = &self.projects[ix];
        let mut kids: Vec<&ChatSession> = project
            .sessions
            .iter()
            .filter(|chat| chat.parent == Some(id))
            .collect();
        kids.sort_by(|a, b| {
            a.delegate_number
                .cmp(&b.delegate_number)
                .then_with(|| a.id.cmp(&b.id))
        });
        kids.into_iter()
            .map(|chat| session::ChildSummary {
                id: chat.id,
                kernel_id: chat.agent_session.clone(),
                title: self.display_label(chat.id),
                state: chat.child_state(),
                readonly: chat.readonly,
                agent_kind: chat.agent_kind.clone(),
                step: chat.current_step(),
            })
            .collect()
    }

    /// Put the current child summaries on `id` so the transcript can draw
    /// them without reaching into other sessions. Cheap; call before a draw.
    pub fn refresh_children(&mut self, id: u64) {
        let kids = self.child_summaries(id);
        // The command this chat has running as a job — an attached `bash`
        // is a process row, not a tool item, until it returns — so the
        // live line can say "Running sleep 75; echo waited" over a status
        // the agent set before it (F-139; Jacob's report 2026-09-17-6).
        let job = self.project_of(id).and_then(|ix| {
            self.projects[ix]
                .surfaces
                .iter()
                .rev()
                .find(|surface| {
                    surface.owner == Some(id)
                        && matches!(surface.bind, Bind::Process { done: None, .. })
                })
                .map(|surface| surface.title.clone())
        });
        if let Some(chat) = self.session_mut(id) {
            // A status the agent set over its workers loses its subject
            // when the last of them finishes: "Waiting on three sorting
            // workers" over three Done lines (Jacob's report 2026-09-17-6;
            // the drawing half of #432). The kernel writes no waiting line
            // while the agent's own status stands, so the children's
            // states are the signal here.
            let any_working = kids
                .iter()
                .any(|child| matches!(child.state, session::ChildState::Working));
            chat.settle_status_over_workers(any_working);
            if chat.children != kids {
                chat.children = kids;
            }
            if chat.running_job != job {
                chat.running_job = job;
            }
        }
    }

    /// Every finished delegate in the project, off the tree. The poll's
    /// safety net for a `Turn idle` the pump did not act on.
    fn reap_delegates(&mut self, ix: usize, cx: &mut Context<Self>) -> bool {
        let done: Vec<u64> = self
            .projects
            .get(ix)
            .map(|project| {
                project
                    .sessions
                    .iter()
                    // A rust-kernel child is an agent folder with a plan
                    // of its own; it stays on the tree until archived.
                    // Only Go delegates (one-shot runs) are reaped.
                    .filter(|chat| {
                        chat.delegate_done()
                            && chat
                                .agent_session
                                .as_deref()
                                .is_some_and(kernel::go_kernel_id)
                    })
                    .map(|chat| chat.id)
                    .collect()
            })
            .unwrap_or_default();
        let mut changed = false;
        for id in done {
            changed |= self.reap_delegate(id, cx);
        }
        changed
    }

    /// Drop a finished delegate from the tree: its turn ended, the grace
    /// passed, nothing runs, and the parent's spawn call has returned. The
    /// session file stays, so its history is still there to read; the
    /// row comes back if it works again. Focus moves up to the parent.
    pub(crate) fn reap_delegate(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.project_of(id) else {
            return false;
        };
        let Some(chat) = self.projects[ix].session(id) else {
            return false;
        };
        if !chat.delegate_done() {
            return false;
        }
        let parent = chat.parent;
        let sid = chat.agent_session.clone();
        if let (Some(parent), Some(sid)) = (parent, sid.as_deref())
            && self.spawn_running(ix, parent, sid)
        {
            return false;
        }
        let focused = self.projects[ix].focused_agent() == Some(id);
        if let Some(chat) = self.projects[ix].session_mut(id) {
            chat.closed = true;
            chat.flush();
        }
        if focused && let Some(parent) = parent.filter(|&p| self.projects[ix].session(p).is_some())
        {
            self.select_session(parent, cx);
        }
        cx.notify();
        true
    }

    /// Whether `parent` has a spawn call for `sid` that has not returned:
    /// a background delegate it is waiting on, or talking to.
    fn spawn_running(&self, ix: usize, parent: u64, sid: &str) -> bool {
        self.projects[ix].session(parent).is_some_and(|chat| {
            chat.items.iter().any(|item| {
                matches!(
                    item,
                    session::ChatItem::Tool {
                        child_session: Some(child),
                        status: session::ToolStatus::Running,
                        ..
                    } if child == sid
                )
            })
        })
    }

    fn upsert_surface(
        &mut self,
        ix: usize,
        owner: Option<u64>,
        kind: SurfaceKind,
        title: String,
        bind: Bind,
        board_kind: &str,
        matches: impl Fn(&Surface, &Bind) -> bool,
    ) -> SurfaceId {
        if let Some(id) = self.projects[ix]
            .surfaces
            .iter()
            .find(|surface| surface.owner == owner && matches(surface, &bind))
            .map(|surface| surface.id)
        {
            if let Some(surface) = self.projects[ix].surface_mut(id) {
                surface.title = title;
                surface.bind = bind;
                surface.touched = crate::model::project::stamp();
            }
            return id;
        }
        let id = SurfaceId(self.next_id);
        self.next_id += 1;
        let key = self.projects[ix].take_key();
        self.projects[ix]
            .surfaces
            .push(Surface::new(id, owner, kind, title, bind, board_kind, key));
        id
    }

    fn watch_board(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(place) = self.projects.get(ix).map(|project| project.place()) else {
            return;
        };
        let key = place.encode();
        if self.board_out.contains_key(&key) {
            return;
        }
        let (out, mut events) = boardhub::listen(place);
        self.board_out.insert(key.clone(), out);
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                let _ = this.update(cx, |workspace, cx| match event {
                    boardhub::Event::Ready => {
                        if let Some(ix) = workspace
                            .projects
                            .iter()
                            .position(|project| project.place().encode() == key)
                        {
                            workspace.push_snapshot(ix);
                        }
                    }
                    boardhub::Event::Command { owner, command } => {
                        workspace.apply_board(&key, &owner, command, cx);
                    }
                });
            }
        })
        .detach();
        self.push_snapshot(ix);
    }

    fn push_snapshot(&mut self, _ix: usize) {
        // This window is not the board authority. The kernel keeps the last
        // snapshot it was given; a stacked dummy layout would wipe the Mac
        // board and the agent's `board list`.
    }

    fn apply_board(
        &mut self,
        place: &str,
        owner_sid: &str,
        cmd: boardhub::Command,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self
            .projects
            .iter()
            .position(|project| project.place().encode() == place)
        else {
            return;
        };
        let owner = owner_sid
            .is_empty()
            .then(|| None)
            .unwrap_or_else(|| {
                self.projects[ix]
                    .sessions
                    .iter()
                    .find(|chat| chat.agent_session.as_deref() == Some(owner_sid))
                    .map(|chat| chat.id)
            })
            .or_else(|| self.projects[ix].focused_agent());
        match cmd.action.as_str() {
            "open" => self.board_open(ix, owner, cmd, cx),
            "close" => self.board_close(ix, &cmd, cx),
            "focus" => {
                if let Some(target) = cmd.target.as_deref() {
                    self.board_focus(ix, target, cx);
                }
            }
            _ => {}
        }
    }

    fn board_open(
        &mut self,
        ix: usize,
        owner: Option<u64>,
        cmd: boardhub::Command,
        cx: &mut Context<Self>,
    ) {
        let panel = cmd
            .panel
            .as_deref()
            .map(str::trim)
            .map(|name| {
                if name == "telegram" {
                    "messenger"
                } else {
                    name
                }
            })
            .unwrap_or("")
            .to_ascii_lowercase();
        if panel.is_empty() {
            return;
        }
        if panel == "chat" {
            if let Some(sid) = cmd.session.filter(|sid| !sid.is_empty()) {
                if let Some(parent) = owner {
                    self.ensure_child_agent(parent, sid, cx);
                }
            } else {
                self.active = Some(ix);
                self.new_session(settings::kernel_agent(), None, cx);
            }
            return;
        }
        if matches!(panel.as_str(), "settings" | "history" | "activity") {
            return;
        }
        let count = cmd.count.unwrap_or(1).clamp(1, 24) as usize;
        let kind = SurfaceKind::from_board(&panel);
        for n in 0..count {
            let bind = match kind {
                SurfaceKind::Terminal => {
                    let id = cmd
                        .terminal_ids
                        .get(n)
                        .cloned()
                        .unwrap_or_else(|| format!("term-{}", crate::model::project::stamp()));
                    Bind::Terminal {
                        id,
                        cwd: cmd.cwd.clone(),
                    }
                }
                SurfaceKind::Browser => cmd
                    .path
                    .clone()
                    .filter(|path| path.contains("://"))
                    .map(Bind::Url)
                    .unwrap_or(Bind::Empty),
                SurfaceKind::Panel | SurfaceKind::Process => cmd
                    .path
                    .clone()
                    .map(|path| Bind::Path(PathBuf::from(path)))
                    .unwrap_or(Bind::Empty),
            };
            let title = cmd
                .path
                .as_deref()
                .and_then(|path| {
                    std::path::Path::new(path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned)
                })
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| kind.label().to_owned());
            let singleton = !matches!(panel.as_str(), "terminal" | "browser" | "messenger")
                || cmd.path.is_some();
            let id = self.upsert_surface(ix, owner, kind, title, bind, &panel, |surface, bind| {
                if !singleton {
                    return false;
                }
                surface.board_kind == panel
                    && match (&surface.bind, bind) {
                        (Bind::Path(held), Bind::Path(next)) => held == next,
                        (Bind::Url(held), Bind::Url(next)) => held == next,
                        (Bind::Terminal { id: held, .. }, Bind::Terminal { id: next, .. }) => {
                            held == next
                        }
                        (Bind::Empty, Bind::Empty) => true,
                        _ => false,
                    }
            });
            if let Some(agent) = owner {
                self.projects[ix].focus_surface(agent, id);
            }
        }
        self.push_snapshot(ix);
        cx.notify();
    }

    fn board_close(&mut self, ix: usize, cmd: &boardhub::Command, cx: &mut Context<Self>) {
        if cmd.all {
            self.projects[ix].surfaces.clear();
            if let Some(focus) = &mut self.projects[ix].focus {
                focus.surface = None;
            }
            self.push_snapshot(ix);
            cx.notify();
            return;
        }
        if let Some(target) = cmd.target.as_deref() {
            if let Some(id) = self.card_surface(ix, target) {
                self.close_surface(id, cx);
            }
            return;
        }
        let kinds = &cmd.kinds;
        let match_text = cmd
            .match_text
            .as_deref()
            .map(str::to_ascii_lowercase)
            .filter(|s| !s.is_empty());
        let gone: Vec<SurfaceId> = self.projects[ix]
            .surfaces
            .iter()
            .filter(|surface| {
                let kind_ok = kinds.is_empty() || kinds.iter().any(|k| k == &surface.board_kind);
                let match_ok = match_text.as_ref().is_none_or(|needle| {
                    surface.title.to_ascii_lowercase().contains(needle)
                        || surface.path().is_some_and(|path| {
                            path.to_string_lossy().to_ascii_lowercase().contains(needle)
                        })
                });
                kind_ok && match_ok
            })
            .map(|surface| surface.id)
            .collect();
        for id in gone {
            self.close_surface(id, cx);
        }
    }

    fn board_focus(&mut self, ix: usize, target: &str, cx: &mut Context<Self>) {
        if let Some(id) = self.card_surface(ix, target) {
            self.select_surface(id, cx);
            return;
        }
        if let Some(id) = self.card_session(ix, target) {
            self.select_session(id, cx);
        }
    }

    fn card_surface(&self, ix: usize, target: &str) -> Option<SurfaceId> {
        let project = self.projects.get(ix)?;
        if let Ok(key) = target.parse::<i32>() {
            if let Some(id) = project
                .surfaces
                .iter()
                .find(|surface| surface.key == key)
                .map(|surface| surface.id)
            {
                return Some(id);
            }
        }
        project
            .surfaces
            .iter()
            .find(|surface| {
                surface.card_id == target
                    || surface.title.eq_ignore_ascii_case(target)
                    || surface
                        .path()
                        .is_some_and(|path| path.to_string_lossy().contains(target))
            })
            .map(|surface| surface.id)
    }

    fn card_session(&self, ix: usize, target: &str) -> Option<u64> {
        let project = self.projects.get(ix)?;
        if let Some(rest) = target.strip_prefix('c') {
            if let Ok(id) = rest.parse::<u64>() {
                if project.session(id).is_some() {
                    return Some(id);
                }
            }
        }
        project
            .sessions
            .iter()
            .find(|chat| {
                chat.agent_session.as_deref() == Some(target)
                    || chat.label().eq_ignore_ascii_case(target)
            })
            .map(|chat| chat.id)
    }

    // ── boards ───────────────────────────────────────────────────────

    /// A fresh board in the active project, opened as it lands. Gated here as
    /// well as in the menus that call it: this is where a board is born.
    pub fn new_board(&mut self, cx: &mut Context<Self>) -> Option<usize> {
        if !self.settings.features.boards {
            return None;
        }
        let project = self.active_ix()?;
        let board = board::create(&self.projects[project].path)?;
        self.projects[project].boards.insert(0, board);
        self.open_board(project, 0, cx);
        Some(0)
    }

    /// Every project's boards are on show, so picking one brings its project
    /// forward with it.
    pub fn open_board(&mut self, project: usize, ix: usize, cx: &mut Context<Self>) {
        let Some(open) = self.projects.get_mut(project) else {
            return;
        };
        if ix >= open.boards.len() {
            return;
        }
        open.board = Some(ix);
        let id = open.boards[ix].path.to_string_lossy().into_owned();
        self.active = Some(project);
        self.remember(project, state::Kind::Board, id);
        cx.notify();
    }

    /// Drop the board: the file goes with it.
    pub fn delete_board(&mut self, project: usize, ix: usize, cx: &mut Context<Self>) {
        let Some(project) = self.projects.get_mut(project) else {
            return;
        };
        if ix >= project.boards.len() {
            return;
        }
        project.boards.remove(ix).remove();
        project.board = project
            .board
            .filter(|open| *open != ix)
            .map(|open| if open > ix { open - 1 } else { open });
        cx.notify();
    }

    pub fn rename_board(&mut self, path: &Path, name: String, cx: &mut Context<Self>) {
        let Some(board) = self.board_at_mut(path) else {
            return;
        };
        board.name = name.trim().to_owned();
        board.save();
        cx.notify();
    }

    pub fn archive_board(&mut self, path: &Path, archived: bool, cx: &mut Context<Self>) {
        let Some(board) = self.board_at_mut(path) else {
            return;
        };
        board.archived = archived;
        board.save();
        cx.notify();
    }

    /// The board the board pane would show, and the choke point the boards
    /// switch bites at: with nothing to hand back, the pane is unreachable —
    /// nothing to render, nothing to step to, nothing for the sidebar to light.
    /// The files stay where they are.
    pub fn active_board(&self) -> Option<&Board> {
        if !self.settings.features.boards {
            return None;
        }
        let project = self.active_project()?;
        project.boards.get(project.board?)
    }

    pub fn active_board_mut(&mut self) -> Option<&mut Board> {
        if !self.settings.features.boards {
            return None;
        }
        let project = self.projects.get_mut(self.active?)?;
        project.boards.get_mut(project.board?)
    }

    /// The board a file names, wherever it is open. What a rename holds onto:
    /// an index moves the moment a neighbour is made or dropped, and the file
    /// is the board — it is where [`Board::save`] writes.
    pub fn board_at(&self, path: &Path) -> Option<&Board> {
        self.projects
            .iter()
            .flat_map(|open| open.boards.iter())
            .find(|board| board.path == path)
    }

    pub fn board_at_mut(&mut self, path: &Path) -> Option<&mut Board> {
        self.projects
            .iter_mut()
            .flat_map(|open| open.boards.iter_mut())
            .find(|board| board.path == path)
    }

    // ── articles ─────────────────────────────────────────────────────

    /// A fresh document in the active project, opened as it lands — an empty
    /// article has nothing to look at but the caret.
    pub fn new_article(&mut self, cx: &mut Context<Self>) -> Option<usize> {
        let project = self.active_ix()?;
        let article = article::create(&self.projects[project].path)?;
        // Where a re-read would put it: the list is newest first, and a new one
        // appended would sit at the bottom until the next load moved it.
        self.projects[project].articles.insert(0, article);
        self.open_article(project, 0, cx);
        Some(0)
    }

    pub fn rename_article(&mut self, path: &Path, name: String, cx: &mut Context<Self>) {
        let Some(article) = self.article_at_mut(path) else {
            return;
        };
        let name = name.trim().to_owned();
        article.rename(&name, cx);
        cx.notify();
    }

    pub fn archive_article(&mut self, path: &Path, archived: bool, cx: &mut Context<Self>) {
        let Some(article) = self.article_at_mut(path) else {
            return;
        };
        article.archive(archived);
        cx.notify();
    }

    /// The article a file names, wherever it is open — what the sidebar
    /// addresses one by, since an index moves when a neighbour is made.
    pub fn article_at_mut(&mut self, path: &Path) -> Option<&mut Article> {
        self.projects
            .iter_mut()
            .flat_map(|open| open.articles.iter_mut())
            .find(|article| article.path == path)
    }

    /// Every project's articles are on show, so picking one brings its project
    /// forward with it.
    pub fn open_article(&mut self, project: usize, ix: usize, cx: &mut Context<Self>) {
        let Some(article) = self
            .projects
            .get_mut(project)
            .and_then(|open| open.articles.get_mut(ix))
        else {
            return;
        };
        article.open(cx);
        self.projects[project].article = Some(ix);
        let id = self.projects[project].articles[ix]
            .path
            .to_string_lossy()
            .into_owned();
        self.active = Some(project);
        self.remember(project, state::Kind::Article, id);
        cx.notify();
    }

    /// Drop the article: the file goes with it.
    pub fn delete_article(&mut self, project: usize, ix: usize, cx: &mut Context<Self>) {
        let Some(project) = self.projects.get_mut(project) else {
            return;
        };
        if ix >= project.articles.len() {
            return;
        }
        project.articles.remove(ix).remove();
        project.article = project
            .article
            .filter(|open| *open != ix)
            .map(|open| if open > ix { open - 1 } else { open });
        cx.notify();
    }

    /// Take the file over the buffer, for an article the watch found had moved
    /// underneath one — what the pane's notice offers. See [`Article::revert`].
    pub fn revert_article(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(article) = self.article_at_mut(path) else {
            return;
        };
        article.revert(cx);
        cx.notify();
    }

    /// Keep the buffer instead, and write it over what landed — the other half
    /// of the same notice. See [`Article::keep`].
    pub fn keep_article(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(article) = self.article_at_mut(path) else {
            return;
        };
        article.keep(cx);
        cx.notify();
    }

    pub fn active_article(&self) -> Option<&Article> {
        let project = self.active_project()?;
        project.articles.get(project.article?)
    }

    /// Put a cover on the open article, or take it off — see
    /// [`Article::set_cover`].
    pub fn set_cover(&mut self, source: Option<&Path>, cx: &mut Context<Self>) {
        if let Some(article) = self.article_mut() {
            article.set_cover(source);
            cx.notify();
        }
    }

    /// Cut the open article a new cover — see [`Article::shuffle_cover`].
    pub fn shuffle_cover(&mut self, cx: &mut Context<Self>) {
        if let Some(article) = self.article_mut() {
            article.shuffle_cover();
            cx.notify();
        }
    }

    fn article_mut(&mut self) -> Option<&mut Article> {
        let project = self.projects.get_mut(self.active?)?;
        project.articles.get_mut(project.article?)
    }

    // ── tables ───────────────────────────────────────────────────────

    /// A fresh table in the active project, opened as it lands.
    ///
    /// One text column, because the store will not make a table without one
    /// and a column you can rename is a better start than a dialog asking for
    /// the shape before anything exists to shape.
    pub fn new_table(&mut self, cx: &mut Context<Self>) -> Option<usize> {
        if !self.settings.features.tables {
            return None;
        }
        let at = self.active?;
        let project = self.projects.get_mut(at)?;
        // The one place a store is created: making a table is the moment the
        // project has something to keep in one.
        if project.data.is_none() {
            project.data = Data::open(&project.path).ok();
        }
        let mut name = UNTITLED.to_owned();
        for n in 2.. {
            if !project.tables.iter().any(|table| table.name == name) {
                break;
            }
            name = format!("{UNTITLED} {n}");
        }
        let column = Column {
            name: "Name".to_owned(),
            kind: ColType::Text,
        };
        let key = project
            .data
            .as_mut()?
            .create(&name, None, &[column], None)
            .ok()?
            .key;
        project.reload_tables();
        // Found by key rather than taken as a known row: the list is ordered by
        // age, and where the newest lands is the list's business, not this one's.
        let ix = project.tables.iter().position(|table| table.key == key)?;
        self.open_table(at, ix, cx);
        Some(ix)
    }

    /// Every project's tables are on show, so picking one brings its project
    /// forward with it.
    pub fn open_table(&mut self, project: usize, ix: usize, cx: &mut Context<Self>) {
        let Some(open) = self.projects.get_mut(project) else {
            return;
        };
        if ix >= open.tables.len() {
            return;
        }
        open.table = Some(ix);
        open.reload_page();
        let id = open.tables[ix].key.clone();
        self.active = Some(project);
        self.remember(project, state::Kind::Table, id);
        cx.notify();
    }

    /// Drop the table: its rows go with it.
    pub fn delete_table(&mut self, project: usize, ix: usize, cx: &mut Context<Self>) {
        let Some(open) = self.projects.get_mut(project) else {
            return;
        };
        let Some(key) = open.tables.get(ix).map(|table| table.key.clone()) else {
            return;
        };
        if let Some(data) = open.data.as_mut() {
            let _ = data.remove(&key);
        }
        open.table = open
            .table
            .filter(|shown| *shown != ix)
            .map(|shown| if shown > ix { shown - 1 } else { shown });
        open.reload_tables();
        cx.notify();
    }

    /// Run `f` against the open table's store, then re-read what it did.
    ///
    /// Every table mutation goes through here, so none of them can forget the
    /// reload — a grid still showing the row you just deleted is the bug this
    /// shape makes unwritable.
    fn with_table<T>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Data, &str) -> T,
    ) -> Option<T> {
        let at = self.active?;
        let project = self.projects.get_mut(at)?;
        let key = project
            .table
            .and_then(|ix| project.tables.get(ix))
            .map(|table| table.key.clone())?;
        let data = project.data.as_mut()?;
        let done = f(data, &key);
        // Working in a table is what makes it the table you were last in, and
        // the list is ordered by that.
        let _ = data.touch(&key);
        project.reload_tables();
        cx.notify();
        Some(done)
    }

    /// The name of column `at`, which is what the store addresses one by.
    fn column_name(&self, at: usize) -> Option<String> {
        let page = self.active_page()?;
        page.columns.get(at).map(|column| column.name.clone())
    }

    /// Write one cell. The text goes in as text whatever the column holds —
    /// SQLite's affinity converts it on the way, so a number typed into a
    /// number column lands as one and the same text in a text column stays put.
    pub fn write_cell(&mut self, rowid: i64, at: usize, text: String, cx: &mut Context<Self>) {
        let Some(column) = self.column_name(at) else {
            return;
        };
        let value = match text.is_empty() {
            true => serde_json::Value::Null,
            false => serde_json::Value::String(text),
        };
        self.with_table(cx, |data, key| {
            let _ = data.write_cells(
                key,
                &[Edit {
                    rowid,
                    column,
                    value,
                }],
            );
        });
    }

    pub fn add_row(&mut self, cx: &mut Context<Self>) -> Option<i64> {
        self.with_table(cx, |data, key| {
            data.add_rows(key, 1)
                .ok()
                .and_then(|ids| ids.first().copied())
        })
        .flatten()
    }

    pub fn delete_row(&mut self, rowid: i64, cx: &mut Context<Self>) {
        self.with_table(cx, |data, key| {
            let _ = data.delete_rows(key, &[rowid]);
        });
    }

    /// A fresh text column, named so it does not collide with one already
    /// there — the header is where it gets its real name.
    pub fn add_column(&mut self, cx: &mut Context<Self>) {
        let taken: Vec<String> = self
            .active_page()
            .map(|page| page.columns.iter().map(|col| col.name.clone()).collect())
            .unwrap_or_default();
        let mut name = COLUMN.to_owned();
        for n in 2.. {
            if !taken.contains(&name) {
                break;
            }
            name = format!("{COLUMN} {n}");
        }
        self.with_table(cx, |data, key| {
            let _ = data.write_column(key, &name, Some(ColType::Text), None);
        });
    }

    /// Rename column `at`, retype it, or both — one call, as the store has it.
    pub fn write_column(
        &mut self,
        at: usize,
        kind: Option<ColType>,
        rename: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(column) = self.column_name(at) else {
            return;
        };
        self.with_table(cx, |data, key| {
            let _ = data.write_column(key, &column, kind, rename.as_deref());
        });
    }

    pub fn delete_column(&mut self, at: usize, cx: &mut Context<Self>) {
        let Some(column) = self.column_name(at) else {
            return;
        };
        self.with_table(cx, |data, key| {
            let _ = data.drop_column(key, &column);
        });
    }

    /// The display name only. The key stays where it is, so a query already
    /// written against this table goes on running.
    pub fn rename_table(&mut self, key: &str, name: String, cx: &mut Context<Self>) {
        self.with_store(key, cx, |data, key| {
            let _ = data.update(key, Some(name.trim()), None);
        });
    }

    pub fn archive_table(&mut self, key: &str, archived: bool, cx: &mut Context<Self>) {
        self.with_store(key, cx, |data, key| {
            let _ = data.archive(key, archived);
        });
    }

    /// Run `f` against whichever store holds `key`, then re-read what it did.
    /// Named rather than open: the sidebar acts on rows the pane is not showing.
    fn with_store(
        &mut self,
        key: &str,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Data, &str),
    ) -> Option<()> {
        let project = self
            .projects
            .iter_mut()
            .find(|open| open.tables.iter().any(|table| table.key == key))?;
        f(project.data.as_mut()?, key);
        project.reload_tables();
        cx.notify();
        Some(())
    }

    /// The open table's rows, as the pane last read them.
    /// The rows on screen. Gated beside [`Self::active_table`]: the table pane
    /// reads the page, not the table, so both have to be shut for it to close.
    pub fn active_page(&self) -> Option<&Page> {
        if !self.settings.features.tables {
            return None;
        }
        self.active_project()?.page.as_ref()
    }

    pub fn active_table(&self) -> Option<&Table> {
        if !self.settings.features.tables {
            return None;
        }
        let project = self.active_project()?;
        project.tables.get(project.table?)
    }

    /// The title or the content changed. Found by the entity because an article
    /// has two surfaces and either can be the one that moved.
    pub fn write_article(&mut self, changed: EntityId, cx: &mut Context<Self>) {
        let found = self.projects.iter_mut().find_map(|project| {
            project.articles.iter_mut().find(|article| {
                article
                    .field
                    .as_ref()
                    .is_some_and(|field| field.entity_id() == changed)
                    || article
                        .editor
                        .as_ref()
                        .is_some_and(|editor| editor.entity_id() == changed)
            })
        });
        if found.is_some_and(|article| article.write(cx)) {
            cx.notify();
        }
    }
}

impl EventEmitter<Reloaded> for Workspace {}
impl EventEmitter<PaneRequest> for Workspace {}

/// Point bezel's tint at the preference. Free rather than a method
/// because the window reads its background appearance while it is being opened,
/// which is before there is a workspace to ask.
pub fn apply_tint(tint: Tint, cx: &mut App) {
    theme::set_brand(
        Brand {
            tint,
            ..theme::brand(cx)
        },
        cx,
    );
}

/// Two answers, because bezel asks two questions: the window stops compositing
/// translucent, and the tint over it goes opaque. Chrome keeps its layers —
/// an opaque window carrying them is what this setting asks for, and what the
/// system's own does not do.
pub fn apply_transparency(reduce: bool, cx: &mut App) {
    let alpha = if reduce { 1.0 } else { Theme::VIBRANCY_ALPHA };
    theme::set_brand(
        Brand {
            vibrancy_alpha: alpha,
            vibrancy: !reduce,
            ..theme::brand(cx)
        },
        cx,
    );
}

struct ChatRef {
    id: Option<u64>,
    place: Option<String>,
    file: Option<String>,
}

fn parse_clipboard_chat(text: &str) -> Option<ChatRef> {
    let text = text.trim();
    if let Some(start) = text.find("arbos://chat/") {
        let rest = &text[start..];
        let url: String = rest
            .chars()
            .take_while(|ch| !ch.is_whitespace() && *ch != ')')
            .collect();
        return parse_chat_link(&url);
    }
    parse_chat_link(text)
}

fn parse_chat_link(url: &str) -> Option<ChatRef> {
    let rest = url.strip_prefix("arbos://chat/")?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut place = None;
    let mut file = None;
    let mut id = path.parse().ok();
    if id.is_none()
        && let Some((encoded, n)) = path.rsplit_once('/')
    {
        place = Some(decode_query(encoded));
        id = n.parse().ok();
    }
    for part in query.split('&').filter(|part| !part.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        match key {
            "p" | "place" => place = Some(decode_query(value)),
            "file" => file = Some(decode_query(value)),
            _ => {}
        }
    }
    (id.is_some() || place.is_some() || file.is_some()).then_some(ChatRef { id, place, file })
}

fn escape_md_label(s: &str) -> String {
    s.replace('\\', "\\\\").replace(']', "\\]")
}

fn encode_query(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn decode_query(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The kernel's transcript for agent `sid` in a local place.
fn transcript_path(workspace: &Path, sid: &str) -> PathBuf {
    workspace
        .join(".arbos")
        .join("agents")
        .join(sid)
        .join("transcript.jsonl")
}

/// Whether the last record in a transcript ends a turn. False for a file
/// that is missing, empty, or mid-turn.
fn transcript_ended(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .and_then(|line| serde_json::from_str::<arbos_core::Event>(line).ok())
        .is_some_and(|event| event.ends_turn())
}
