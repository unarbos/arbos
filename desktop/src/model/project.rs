//! A project: a directory, the sessions running in it, its boards and its
//! articles.
//!
//! The path is the whole identity — it is what every session in the project
//! is spawned with as its `cwd`, and what [`crate::model::state`] persists.

use crate::{
    data::{Data, Page, Table},
    model::{
        article::Article,
        board::Board,
        place::Place,
        session::ChatSession,
        surface::{Bind, Child, Focus, Surface, SurfaceId, SurfaceKind},
        watch::Watch,
        workspace::Workspace,
    },
};
use bezel::gpui::Context;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Kernel store: `<workspace>/.arbos/`. `web.json`, `board.json`, and
/// `sessions.db` live here. Desktop extras go under [`DESKTOP`].
const STORE: &str = ".arbos";
/// GPUI-owned files inside the store — drafts, ranks, articles, boards, the
/// table database. Isolated so a write cannot overwrite kernel JSON or SQLite.
const DESKTOP: &str = "desktop";
/// Older desktop builds wrote here. [`adopt`] copies extras out once.
const LEGACY: &str = ".cydonia";

/// How many rows the table pane reads at once. The count beside them is the
/// table's own, so a window that does not reach the end says so.
const PAGE: i64 = 200;

pub struct Project {
    /// ssh alias when this project is a folder on another machine.
    pub host: Option<String>,
    pub path: PathBuf,
    pub sessions: Vec<ChatSession>,
    /// Surfaces this project's agents have opened. Derived at runtime from
    /// board commands and `show`; not filed on disk.
    pub surfaces: Vec<Surface>,
    /// The agent in front, and whether one of its children fills the column.
    pub focus: Option<Focus>,
    /// Board-local card keys, minted here so a snapshot's "#7" is stable
    /// for the life of the process.
    pub next_key: i32,
    pub boards: Vec<Board>,
    /// Which board the board pane shows.
    pub board: Option<usize>,
    pub articles: Vec<Article>,
    /// Which article the article pane shows.
    pub article: Option<usize>,
    /// The project's database, once there is one. Opening a project must not
    /// write a database into it, so this stays `None` until a table is made.
    pub data: Option<Data>,
    pub tables: Vec<Table>,
    /// Which table the table pane shows.
    pub table: Option<usize>,
    /// The open table's window of rows, read when it is opened rather than
    /// while it is drawn — a query per frame is a query too many.
    pub page: Option<Page>,
    /// Whether the sidebar shows what is under this project's heading.
    pub expanded: bool,
    /// Whether the sidebar lists archived chats under this heading.
    /// Off by default: what was put away is not what you came back for.
    /// The header toggle is the only control; there is no Archived row.
    pub archive_open: bool,
    /// The watch on this project's `.arbos/`, once it is up. Held here so
    /// closing the project drops it, which is what takes the watch down.
    pub watch: Option<Watch>,
    /// Sidebar title, when the person named it. Empty means the folder's own
    /// name — see [`Self::name`].
    pub nickname: Option<String>,
    /// Kernel session ids the user deleted. The activity poll must not mint
    /// them again — that is why trash on a delegate used to do nothing.
    pub dismissed: HashSet<String>,
}

impl Project {
    pub fn new(path: PathBuf) -> Self {
        Self::open(Place::local(path))
    }

    pub fn open(place: Place) -> Self {
        // Chat only. Boards, articles and tables have no pane; scanning them
        // on open can fail a folder that is otherwise fine to talk in.
        Self {
            boards: Vec::new(),
            articles: Vec::new(),
            data: None,
            host: place.host,
            path: place.path,
            sessions: Vec::new(),
            surfaces: Vec::new(),
            focus: None,
            next_key: 1,
            board: None,
            article: None,
            tables: Vec::new(),
            table: None,
            page: None,
            expanded: true,
            archive_open: false,
            watch: None,
            nickname: None,
            dismissed: HashSet::new(),
        }
    }

    /// Re-read everything on disk and reconcile it with what is held. The
    /// answer to any event under `.arbos/` — see [`crate::model::watch`] for
    /// why the event itself is never read for more than where it landed.
    ///
    /// Answers whether anything a view holds *about* an entry moved: a card's
    /// place on a board, a column's place in a table, the editor a document was
    /// being read in. Everything else is drawn off the model each frame and
    /// nobody keeps a handle on it, so a re-read that only changed those has
    /// nothing to announce — and announcing it would drop the edit somebody has
    /// open over the echo of their own save.
    pub fn reload(&mut self, _cx: &mut Context<Workspace>) -> bool {
        false
    }

    /// Re-read what tables exist. The store is the list — nothing here keeps a
    /// second copy of it that a failed write could leave standing.
    pub fn reload_tables(&mut self) {
        // Held by key across the re-read, not by index: a table made or dropped
        // beside the open one shifts every index past it, and the pane would be
        // left showing whichever table slid into its place.
        let open = self
            .table
            .and_then(|ix| self.tables.get(ix))
            .map(|table| table.key.clone());
        self.tables = self
            .data
            .as_ref()
            .and_then(|data| data.list().ok())
            .unwrap_or_default();
        self.table = match open.and_then(|key| self.position(&key)) {
            found @ Some(_) => found,
            None => self.table.filter(|ix| *ix < self.tables.len()),
        };
        self.reload_page();
    }

    fn position(&self, key: &str) -> Option<usize> {
        self.tables.iter().position(|table| table.key == key)
    }

    /// Read the open table's rows.
    pub fn reload_page(&mut self) {
        let key = self
            .table
            .and_then(|ix| self.tables.get(ix))
            .map(|table| table.key.clone());
        self.page = match (key, self.data.as_ref()) {
            (Some(key), Some(data)) => data.read(&key, None, false, PAGE, 0).ok(),
            _ => None,
        };
    }

    pub fn place(&self) -> Place {
        Place {
            host: self.host.clone(),
            path: self.path.clone(),
        }
    }

    pub fn is_remote(&self) -> bool {
        self.host.is_some()
    }

    /// Where `.arbos` extras are written. A remote place uses a local sidecar.
    pub fn store(&self) -> PathBuf {
        self.place().store()
    }

    /// The tab's label: a name they typed, or the place title (box name at
    /// remote home/`/`, otherwise the last folder).
    pub fn name(&self) -> String {
        self.nickname
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.place().title())
    }

    pub fn session(&self, id: u64) -> Option<&ChatSession> {
        self.sessions.iter().find(|chat| chat.id == id)
    }

    pub fn session_mut(&mut self, id: u64) -> Option<&mut ChatSession> {
        self.sessions.iter_mut().find(|chat| chat.id == id)
    }

    pub fn active_session(&self) -> Option<&ChatSession> {
        self.focus.and_then(|focus| self.session(focus.agent))
    }

    pub fn focused_agent(&self) -> Option<u64> {
        self.focus.map(|focus| focus.agent)
    }

    pub fn focus_on(&mut self, agent: u64) {
        self.focus = Some(Focus::agent(agent));
    }

    pub fn focus_surface(&mut self, agent: u64, surface: SurfaceId) {
        self.focus = Some(Focus::on(agent, surface));
    }

    pub fn surface(&self, id: SurfaceId) -> Option<&Surface> {
        self.surfaces.iter().find(|surface| surface.id == id)
    }

    pub fn surface_mut(&mut self, id: SurfaceId) -> Option<&mut Surface> {
        self.surfaces.iter_mut().find(|surface| surface.id == id)
    }

    /// Sessions that stand under the project heading: no parent, or a parent
    /// that is no longer here.
    pub fn roots(&self) -> impl Iterator<Item = &ChatSession> {
        self.sessions.iter().filter(|chat| self.is_root(chat))
    }

    /// Whether any root chat is in the archive. The header toggle
    /// only appears when this is true.
    pub fn has_archived(&self) -> bool {
        self.roots().any(|chat| chat.closed)
    }

    /// Rank just above every live sibling, so a new row lands at the top once.
    pub fn front_rank(&self, parent: Option<u64>) -> i64 {
        self.sessions
            .iter()
            .filter(|chat| chat.parent == parent && !chat.closed)
            .map(|chat| chat.rank)
            .min()
            .unwrap_or(0)
            - 1
    }

    pub fn is_root(&self, chat: &ChatSession) -> bool {
        match chat.parent {
            None => true,
            Some(parent) => self.session(parent).is_none(),
        }
    }

    /// Whether `older` is `younger` or sits above it in the parent chain.
    pub fn ancestor_of(&self, older: u64, younger: u64) -> bool {
        let mut at = Some(younger);
        while let Some(id) = at {
            if id == older {
                return true;
            }
            at = self.session(id).and_then(|chat| chat.parent);
        }
        false
    }

    /// Sessions from the root that holds `id` down to `id`, root first.
    pub fn path_to(&self, id: u64) -> Vec<u64> {
        let mut chain = Vec::new();
        let mut at = Some(id);
        while let Some(id) = at {
            chain.push(id);
            at = self.session(id).and_then(|chat| chat.parent);
        }
        chain.reverse();
        chain
    }

    /// Children of `agent` that still belong on the tree: every open
    /// subagent, plus a browser or terminal the turn still needs, or the
    /// one in front.
    pub fn children(&self, agent: u64) -> Vec<Child> {
        let focus = self.focus;
        let mut agents: Vec<(i64, u64)> = self
            .sessions
            .iter()
            .filter(|chat| chat.parent == Some(agent) && self.live_child(chat, focus))
            .map(|chat| (chat.rank, chat.id))
            .collect();
        agents.sort_by_key(|(rank, id)| (*rank, *id));
        let mut kids: Vec<Child> = agents.into_iter().map(|(_, id)| Child::Agent(id)).collect();
        kids.extend(
            self.surfaces
                .iter()
                .filter(|surface| {
                    surface.owner == Some(agent) && self.live_surface(surface, agent, focus)
                })
                .map(|surface| Child::Surface(surface.id)),
        );
        kids
    }

    /// A child agent stays on the tree while there is something to see:
    /// a turn running, work the kernel still holds for it (a standing job,
    /// a scheduled node, a question, a failure), a turn that ended a moment
    /// ago, or you looking at it (or through it). Done and unwatched means
    /// off the list; its folder stays, and it comes back if it works again.
    fn live_child(&self, chat: &ChatSession, focus: Option<Focus>) -> bool {
        if chat.closed {
            return focus.is_some_and(|focus| focus.agent == chat.id);
        }
        if chat.busy() || chat.recently_ended() || chat.plan_open().next().is_some() {
            return true;
        }
        focus.is_some_and(|focus| focus.agent == chat.id || self.ancestor_of(chat.id, focus.agent))
    }

    /// A terminal, process, or panel stays until someone closes it, and so
    /// does the kernel's own browser page — the kernel says when it ends.
    /// A URL card stays while its owner is still working, or while it
    /// fills the column.
    fn live_surface(&self, surface: &Surface, owner: u64, focus: Option<Focus>) -> bool {
        if matches!(
            surface.kind,
            SurfaceKind::Panel | SurfaceKind::Terminal | SurfaceKind::Process
        ) || matches!(surface.bind, Bind::Browser { .. })
        {
            return true;
        }
        if focus.is_some_and(|focus| focus.surface == Some(surface.id)) {
            return true;
        }
        self.session(owner).is_some_and(|chat| chat.busy())
    }

    pub fn take_key(&mut self) -> i32 {
        let key = self.next_key;
        self.next_key += 1;
        key
    }

    /// Drop surfaces this agent owns. Child agents stay; their surfaces go
    /// with them when they are closed.
    pub fn close_surfaces_of(&mut self, agent: u64) {
        self.surfaces.retain(|surface| surface.owner != Some(agent));
        if self
            .focus
            .is_some_and(|focus| focus.surface.is_some_and(|id| self.surface(id).is_none()))
        {
            if let Some(focus) = &mut self.focus {
                focus.surface = None;
            }
        }
    }
}

/// Now, in milliseconds — the id an article or a board is made with. Sorting
/// these is sorting by age, which is the order they are listed back in.
pub fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or_default()
}

/// When a file was last written, as the same millisecond stamp ids carry — the
/// key entries are listed by, so the one you touched last is the one on top.
pub fn written(path: &Path) -> u128 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or_else(stamp, |since| since.as_millis())
}

/// The kernel store: `<project>/.arbos/`. Watch this, not [`dir`].
pub fn root(project: &Path) -> PathBuf {
    project.join(STORE)
}

/// Desktop-owned files: `<project>/.arbos/desktop/`.
pub fn dir(project: &Path) -> PathBuf {
    root(project).join(DESKTOP)
}

/// The desktop directory, made if it is not there, and carrying a `.gitignore`
/// that keeps GPUI extras out of the repo. The ignore file lives here, not
/// at `.arbos/`, so it cannot hide `web.json` or `board.json`.
///
/// Every path that creates the directory comes through here. A second
/// `create_dir_all` elsewhere would make it without the ignore file.
pub fn init(project: &Path) -> std::io::Result<PathBuf> {
    adopt(project);
    let dir = dir(project);
    std::fs::create_dir_all(&dir)?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "*\n")?;
    }
    Ok(dir)
}

/// If `.cydonia/` still has extras and `.arbos/desktop/` is empty, copy
/// sessions (drafts, ranks), articles, boards, and the table database across.
/// Also lifts leftover `.arbos/sessions/*.json` out from beside the kernel
/// SQLite. Leaves `.cydonia/` in place — other tools may still read it.
pub fn adopt(project: &Path) {
    let old = project.join(LEGACY);
    let new = dir(project);
    if old.is_dir() && vacant(&new) {
        if std::fs::create_dir_all(&new).is_ok() {
            for name in ["sessions", "articles", "boards"] {
                let from = old.join(name);
                if from.exists() {
                    let _ = copy_tree(&from, &new.join(name));
                }
            }
            let loose = old.join("board.toml");
            if loose.is_file() {
                let _ = std::fs::copy(&loose, new.join("board.toml"));
            }
            for name in ["data.db", "data.db-wal", "data.db-shm"] {
                let from = old.join(name);
                if from.is_file() {
                    let _ = std::fs::copy(&from, new.join(name));
                }
            }
        }
    }
    let stray = root(project).join("sessions");
    let dest = new.join("sessions");
    if stray.is_dir() && vacant(&dest) {
        if std::fs::create_dir_all(&new).is_ok() {
            let _ = copy_tree(&stray, &dest);
        }
    }
}

fn vacant(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return true;
    };
    entries
        .flatten()
        .all(|entry| entry.file_name() == ".gitignore")
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_file() {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to)?;
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        copy_tree(&entry.path(), &to.join(entry.file_name()))?;
    }
    Ok(())
}
