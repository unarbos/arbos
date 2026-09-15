//! Open a place in two steps: pick a machine, then a folder on it.

use crate::{
    kernel,
    model::place::{self, Place},
};
use bezel::{
    gpui::{
        self, App, Context, DragMoveEvent, Empty, Entity, EventEmitter, FocusHandle, Focusable,
        Hsla, KeyBinding, MouseButton, Pixels, Point, Render, ScrollHandle, SharedString, Task,
        TextAlign, Window, actions, div, prelude::*, px,
    },
    theme::{Glass, SurfaceStyle, TextStyle, Theme, Typeset},
    ui::{
        icons,
        input::{self, FieldEvent, TextField},
        surface::Surfaced as _,
    },
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The local machine's row in the picker: named for what it is (ui-007).
const LOCAL_MACHINE: &str = if cfg!(target_os = "macos") {
    "This Mac"
} else {
    "This machine"
};

actions!(
    arbos_opener,
    [
        Submit, PickHere, Descend, Ascend, Back, Next, Previous, Dismiss, Complete
    ]
);

const KEY_CONTEXT: &str = "ArbosOpener";

/// The panel on its way somewhere else in the window.
#[derive(Clone)]
struct PanelDrag;
const SURFACE: SurfaceStyle = SurfaceStyle::Glass(Glass::Regular);
const WIDTH: f32 = 420.;
const LIST_MAX: f32 = 320.;
const LIST_CAP: usize = 200;
const CACHE_TTL: Duration = Duration::from_secs(45);
const LIST_DEBOUNCE: Duration = Duration::from_millis(120);

struct DirListing {
    names: Vec<String>,
    fetched: Instant,
}

impl DirListing {
    fn fresh(&self) -> bool {
        self.fetched.elapsed() < CACHE_TTL
    }
}

pub fn init(cx: &mut App) {
    crate::view::bind_field_editing(cx, KEY_CONTEXT, false);
    let ctx = Some(KEY_CONTEXT);
    // Finder's quick-open, on the field: the arrows walk the list and the
    // tree; Enter does what the lit row says — "Open <folder>" opens, a
    // folder row steps in; ⌘↩ opens the folder you are in. Bound after
    // the field's own editing chords on this context, so these win the
    // shared keys.
    cx.bind_keys([
        KeyBinding::new("enter", Submit, ctx),
        KeyBinding::new("cmd-enter", PickHere, ctx),
        KeyBinding::new("down", Next, ctx),
        KeyBinding::new("up", Previous, ctx),
        KeyBinding::new("ctrl-n", Next, ctx),
        KeyBinding::new("ctrl-p", Previous, ctx),
        KeyBinding::new("right", Descend, ctx),
        KeyBinding::new("left", Ascend, ctx),
        KeyBinding::new("backspace", Back, ctx),
        KeyBinding::new("escape", Dismiss, ctx),
        KeyBinding::new("tab", Complete, ctx),
    ]);
}

pub enum OpenerEvent {
    Open(Place),
    Browse,
    Dismiss,
}

#[derive(Clone, PartialEq, Eq)]
enum Stage {
    Machine,
    Folder { host: Option<String> },
}

#[derive(Clone, PartialEq, Eq)]
enum Offer {
    Machine {
        name: String,
        host: Option<String>,
    },
    Here(String),
    Dir(String),
    /// A folder that does not exist yet, at the path typed: Enter makes it.
    Create(String),
    /// The native folder picker, for whoever would rather click than type.
    Browse,
}

pub struct Opener {
    field: Entity<TextField>,
    hosts: Vec<String>,
    recent_hosts: Vec<String>,
    stage: Stage,
    cursor: usize,
    /// Last query the field handler acted on. The field notifies on caret
    /// blink and on keys the list already ate; resetting the highlight
    /// then would put it back on the first row.
    last_query: String,
    listings: HashMap<(Option<String>, String), DirListing>,
    inflight: HashSet<(Option<String>, String)>,
    debounce: Option<Task<()>>,
    /// The list's scroll, so a step from the keyboard brings its landing
    /// into view.
    scroll: ScrollHandle,
    pub open: bool,
    /// Where the user has dragged the panel to, as an offset from its
    /// centered resting place. Reset each time the opener shows.
    shift: Point<Pixels>,
    /// Where the pointer took hold of the panel and the shift at that
    /// moment; a drag move is the pointer's travel since, added on.
    grip: Option<(Point<Pixels>, Point<Pixels>)>,
}

impl EventEmitter<OpenerEvent> for Opener {}

impl Opener {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| {
            TextField::new(cx)
                .with_frame(false)
                .with_key_context(KEY_CONTEXT)
                .with_placeholder("machine")
        });
        cx.subscribe(&field, |this: &mut Self, _, event: &FieldEvent, cx| {
            if *event != FieldEvent::Changed {
                return;
            }
            this.on_query(cx);
        })
        .detach();
        Self {
            field,
            hosts: Vec::new(),
            recent_hosts: Vec::new(),
            stage: Stage::Machine,
            cursor: 0,
            last_query: String::new(),
            listings: HashMap::new(),
            inflight: HashSet::new(),
            debounce: None,
            scroll: ScrollHandle::new(),
            open: false,
            shift: Point::default(),
            grip: None,
        }
    }

    pub fn show(&mut self, recents: Vec<Place>, cx: &mut Context<Self>) {
        self.open = true;
        self.shift = Point::default();
        self.grip = None;
        self.hosts = place::ssh_hosts();
        let mut seen = std::collections::HashSet::new();
        self.recent_hosts = recents
            .into_iter()
            .filter_map(|place| place.host)
            .filter(|host| seen.insert(host.clone()))
            .collect();
        self.stage = Stage::Machine;
        self.cursor = 0;
        self.last_query.clear();
        self.debounce = None;
        self.field.update(cx, |field, cx| {
            field.set_placeholder("machine", cx);
            field.clear(cx);
        });
        cx.notify();
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    fn query(&self, cx: &App) -> String {
        self.field.read(cx).content().trim().to_string()
    }

    /// Rebuild the list only when the typed query actually changed.
    /// Caret blink and window activation notify the field without
    /// changing the text; those must not send the highlight to row 0.
    fn on_query(&mut self, cx: &mut Context<Self>) {
        let q = self.query(cx);
        if q == self.last_query {
            return;
        }
        self.last_query = q;
        self.cursor = 0;
        self.maybe_host_prefix(cx);
        self.maybe_local_path(cx);
        if matches!(self.stage, Stage::Folder { .. }) {
            self.queue_listing(cx);
        }
        cx.notify();
    }

    /// A path typed at the machine stage (`/Users/…`, `~/src`) means This
    /// Mac: step into the folder stage with the text kept, instead of
    /// filtering the machine list down to "no machines".
    fn maybe_local_path(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.stage, Stage::Machine) {
            return;
        }
        let q = self.query(cx);
        if !(q.starts_with('/') || q.starts_with('~')) {
            return;
        }
        self.stage = Stage::Folder { host: None };
        self.field.update(cx, |field, cx| {
            field.set_placeholder(LOCAL_MACHINE, cx);
        });
        self.prefetch(cx);
        self.ensure_listing(cx);
    }

    fn current_offer(&self, cx: &App) -> Option<Offer> {
        self.offers(cx).get(self.cursor).cloned()
    }

    /// Keep the highlighted row if it is still in the list. Otherwise
    /// clamp — do not jump to 0 unless the list is empty or the path
    /// itself changed (`on_query` handles that).
    fn keep_cursor(&mut self, keep: Option<Offer>, cx: &App) {
        let offers = self.offers(cx);
        if offers.is_empty() {
            self.cursor = 0;
            return;
        }
        if let Some(keep) = keep {
            if let Some(ix) = offers.iter().position(|offer| offer == &keep) {
                self.cursor = ix;
                return;
            }
        }
        if self.cursor >= offers.len() {
            self.cursor = offers.len() - 1;
        }
    }

    fn machines(&self) -> Vec<Offer> {
        let mut out = vec![Offer::Machine {
            name: LOCAL_MACHINE.into(),
            host: None,
        }];
        let mut seen = std::collections::HashSet::from([LOCAL_MACHINE.to_string()]);
        let push =
            |out: &mut Vec<Offer>, seen: &mut std::collections::HashSet<String>, host: &str| {
                if seen.insert(host.to_string()) {
                    out.push(Offer::Machine {
                        name: host.to_string(),
                        host: Some(host.to_string()),
                    });
                }
            };
        for host in &self.recent_hosts {
            push(&mut out, &mut seen, host);
        }
        for host in &self.hosts {
            push(&mut out, &mut seen, host);
        }
        out.push(Offer::Browse);
        out
    }

    fn offers(&self, cx: &App) -> Vec<Offer> {
        let q = self.query(cx);
        let needle = q.to_lowercase();
        match &self.stage {
            Stage::Machine => self
                .machines()
                .into_iter()
                .filter(|offer| match offer {
                    Offer::Machine { name, host } => {
                        q.is_empty()
                            || name.to_lowercase().contains(&needle)
                            || host
                                .as_deref()
                                .is_some_and(|h| h.to_lowercase().contains(&needle))
                    }
                    Offer::Browse => q.is_empty() || "browse".contains(&needle),
                    Offer::Here(_) | Offer::Dir(_) | Offer::Create(_) => false,
                })
                .collect(),
            Stage::Folder { host } => {
                let (dir, prefix) = split_path(&q);
                let key = (host.clone(), dir.clone());
                let names = self
                    .listings
                    .get(&key)
                    .map(|listing| listing.names.as_slice())
                    .unwrap_or(&[]);
                let prefix_l = prefix.to_lowercase();
                // The folder the text names, when there is one: `~/Code`
                // typed whole is `Code`, with or without its slash, even
                // when `Code2` sits beside it. Locally the file system
                // says; on a remote host the parent's listing does.
                let exact = if prefix.is_empty() {
                    Some(dir.clone())
                } else if names.iter().any(|name| *name == prefix)
                    || (host.is_none() && local_path(&join_dir(&dir, &prefix)).is_dir())
                {
                    Some(join_dir(&dir, &prefix))
                } else {
                    None
                };
                let matches: Vec<String> = names
                    .iter()
                    .filter(|name| {
                        !name.starts_with('.')
                            && (prefix.is_empty() || name.to_lowercase().starts_with(&prefix_l))
                            && Some(name.as_str())
                                != exact.as_deref().and_then(|e| e.rsplit('/').next())
                    })
                    .cloned()
                    .collect();
                let mut out = Vec::new();
                // The first row always says what Enter does. The folder
                // named outright; else the one folder the text narrows to.
                if let Some(path) = &exact {
                    out.push(Offer::Here(path.clone()));
                } else if matches.len() == 1 {
                    out.push(Offer::Here(join_dir(&dir, &matches[0])));
                }
                out.extend(matches.into_iter().map(Offer::Dir));
                // Nothing there by that name on this machine: offer to make
                // the folder typed.
                if host.is_none() && exact.is_none() && out.is_empty() {
                    let typed = q.trim().trim_end_matches('/');
                    if !typed.is_empty() && !local_path(typed).is_dir() {
                        out.push(Offer::Create(typed.to_string()));
                    }
                }
                out
            }
        }
    }

    fn maybe_host_prefix(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.stage, Stage::Machine) {
            return;
        }
        let q = self.query(cx);
        let Some((typed, path)) = q.split_once(':') else {
            return;
        };
        let Some(host) = self
            .hosts
            .iter()
            .find(|host| host.eq_ignore_ascii_case(typed))
            .cloned()
        else {
            return;
        };
        let path = if path.is_empty() {
            "/".into()
        } else {
            path.to_string()
        };
        self.stage = Stage::Folder {
            host: Some(host.clone()),
        };
        self.field.update(cx, |field, cx| {
            field.set_placeholder(&host, cx);
            field.set_content(path, cx);
        });
        self.prefetch(cx);
    }

    fn enter_machine(&mut self, host: Option<String>, cx: &mut Context<Self>) {
        self.stage = Stage::Folder { host: host.clone() };
        self.cursor = 0;
        let name = host.as_deref().unwrap_or(LOCAL_MACHINE);
        self.field.update(cx, |field, cx| {
            field.set_placeholder(name, cx);
            field.set_content("/", cx);
        });
        self.prefetch(cx);
        self.ensure_listing(cx);
        cx.notify();
    }

    fn take(&mut self, offer: Offer, cx: &mut Context<Self>) {
        match offer {
            Offer::Machine { host, .. } => self.enter_machine(host, cx),
            Offer::Browse => {
                self.open = false;
                cx.emit(OpenerEvent::Browse);
                cx.notify();
            }
            Offer::Here(path) => {
                let host = match &self.stage {
                    Stage::Folder { host } => host.clone(),
                    Stage::Machine => None,
                };
                self.start(host, path, cx);
            }
            Offer::Create(path) => {
                if std::fs::create_dir_all(local_path(&path)).is_ok() {
                    self.start(None, path, cx);
                }
            }
            Offer::Dir(name) => {
                let q = self.query(cx);
                let (dir, _) = split_path(&q);
                let next = join_dir(&dir, &name);
                self.field.update(cx, |field, cx| {
                    field.set_content(format!("{next}/"), cx);
                });
                self.cursor = 0;
                self.ensure_listing(cx);
                cx.notify();
            }
        }
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        match &self.stage {
            Stage::Machine => {
                let offers = self.offers(cx);
                if let Some(offer) = offers.get(self.cursor).cloned() {
                    self.take(offer, cx);
                }
            }
            Stage::Folder { host } => {
                let host = host.clone();
                let offers = self.offers(cx);
                match offers.get(self.cursor).cloned() {
                    // "Open <folder>", lit at the top of the list.
                    Some(Offer::Here(path)) => self.start(host, path, cx),
                    Some(Offer::Create(path)) => self.take(Offer::Create(path), cx),
                    // A folder row is a step in, like → and a click; the
                    // Open row above it is how it opens. Enter never means
                    // two things depending on how many siblings matched.
                    Some(Offer::Dir(name)) => self.take(Offer::Dir(name), cx),
                    Some(Offer::Machine { .. } | Offer::Browse) | None => {
                        let typed = self.query(cx);
                        let path = if typed.is_empty() {
                            "/".into()
                        } else {
                            typed.trim_end_matches('/').to_string()
                        };
                        self.start(host, path, cx);
                    }
                }
            }
        }
    }

    /// ⌘↩: open the folder you are in, whatever is lit.
    fn pick_here(&mut self, _: &PickHere, _: &mut Window, cx: &mut Context<Self>) {
        match &self.stage {
            Stage::Machine => {
                let offers = self.offers(cx);
                if let Some(offer) = offers.get(self.cursor).cloned() {
                    self.take(offer, cx);
                }
            }
            Stage::Folder { host } => {
                let host = host.clone();
                let (dir, _) = split_path(&self.query(cx));
                self.start(host, dir, cx);
            }
        }
    }

    /// →: step into the lit folder, or the lit machine.
    fn descend(&mut self, _: &Descend, _: &mut Window, cx: &mut Context<Self>) {
        let offers = self.offers(cx);
        match offers.get(self.cursor).cloned() {
            Some(offer @ (Offer::Dir(_) | Offer::Machine { .. })) => self.take(offer, cx),
            Some(Offer::Here(_) | Offer::Create(_) | Offer::Browse) | None => {}
        }
    }

    /// ←: up one folder; at the top of the tree, back to the machines.
    fn ascend(&mut self, _: &Ascend, _: &mut Window, cx: &mut Context<Self>) {
        self.go_up(cx);
    }

    /// ⌫ with nothing typed is ←; with text it deletes, as in any field.
    fn back(&mut self, _: &Back, window: &mut Window, cx: &mut Context<Self>) {
        if self.query(cx).is_empty() {
            self.go_up(cx);
        } else {
            window.dispatch_action(Box::new(input::Backspace), cx);
        }
    }

    fn go_up(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.stage, Stage::Folder { .. }) {
            return;
        }
        let (dir, _) = split_path(&self.query(cx));
        let parent = match dir.trim_end_matches('/') {
            "" | "~" => None,
            rest => match rest.rfind('/') {
                Some(0) => Some("/".to_string()),
                Some(i) => Some(format!("{}/", &rest[..i])),
                None => None,
            },
        };
        match parent {
            Some(parent) => {
                self.field
                    .update(cx, |field, cx| field.set_content(parent, cx));
                self.cursor = 0;
                self.ensure_listing(cx);
                cx.notify();
            }
            // The tree's top: back to picking a machine.
            None => self.to_machines(cx),
        }
    }

    /// Back to the first step, the field cleared for a machine.
    fn to_machines(&mut self, cx: &mut Context<Self>) {
        self.stage = Stage::Machine;
        self.cursor = 0;
        self.field.update(cx, |field, cx| {
            field.set_placeholder("machine", cx);
            field.clear(cx);
        });
        cx.notify();
    }

    fn complete(&mut self, _: &Complete, _: &mut Window, cx: &mut Context<Self>) {
        match &self.stage {
            Stage::Machine => {
                let offers = self.offers(cx);
                if let Some(Offer::Machine { host, .. }) = offers.get(self.cursor).cloned() {
                    self.enter_machine(host, cx);
                }
            }
            Stage::Folder { .. } => {
                let offers = self.offers(cx);
                let names: Vec<String> = offers
                    .into_iter()
                    .filter_map(|offer| match offer {
                        Offer::Dir(name) => Some(name),
                        Offer::Machine { .. } | Offer::Here(_) | Offer::Create(_) | Offer::Browse => None,
                    })
                    .collect();
                if names.is_empty() {
                    return;
                }
                let q = self.query(cx);
                let (dir, _) = split_path(&q);
                let filled = if names.len() == 1 {
                    format!("{}/", join_dir(&dir, &names[0]))
                } else if let Some(shared) = common_prefix(&names) {
                    if shared.is_empty() {
                        return;
                    }
                    join_dir(&dir, &shared)
                } else {
                    return;
                };
                self.field
                    .update(cx, |field, cx| field.set_content(filled, cx));
                self.cursor = 0;
                self.ensure_listing(cx);
                cx.notify();
            }
        }
    }

    fn next(&mut self, _: &Next, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.offers(cx).len();
        if len == 0 {
            return;
        }
        self.cursor = (self.cursor + 1) % len;
        self.scroll.scroll_to_item(self.cursor);
        cx.notify();
    }

    fn previous(&mut self, _: &Previous, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.offers(cx).len();
        if len == 0 {
            return;
        }
        self.cursor = self.cursor.checked_sub(1).unwrap_or(len - 1);
        self.scroll.scroll_to_item(self.cursor);
        cx.notify();
    }

    /// Escape closes, from either step. Going back up to the machines is
    /// what ← and ⌫ at the top of the tree do.
    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        cx.emit(OpenerEvent::Dismiss);
        cx.notify();
    }

    /// Wait out a burst of path keystrokes, then list. Cached rows already
    /// paint; this only starts SSH.
    fn queue_listing(&mut self, cx: &mut Context<Self>) {
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LIST_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                this.ensure_listing(cx);
                cx.notify();
            });
        }));
    }

    fn prefetch(&mut self, cx: &mut Context<Self>) {
        let Stage::Folder { host: Some(host) } = &self.stage else {
            return;
        };
        let host = host.clone();
        for dir in ["~", "/"] {
            self.start_remote_list(host.clone(), dir.to_string(), cx);
        }
    }

    fn is_listing(&self, cx: &App) -> bool {
        let Stage::Folder { host } = &self.stage else {
            return false;
        };
        let (dir, _) = split_path(&self.query(cx));
        let key = (host.clone(), dir);
        self.inflight.contains(&key) && !self.listings.contains_key(&key)
    }

    fn ensure_listing(&mut self, cx: &mut Context<Self>) {
        let Stage::Folder { host } = &self.stage else {
            return;
        };
        let host = host.clone();
        let q = self.query(cx);
        let (dir, _) = split_path(&q);
        let key = (host.clone(), dir.clone());
        if self
            .listings
            .get(&key)
            .is_some_and(|listing| listing.fresh())
        {
            return;
        }
        if host.is_none() {
            self.listings.insert(
                key,
                DirListing {
                    names: list_local(&dir),
                    fetched: Instant::now(),
                },
            );
            return;
        }
        let Some(host) = host else {
            return;
        };
        self.start_remote_list(host, dir, cx);
    }

    fn start_remote_list(&mut self, host: String, dir: String, cx: &mut Context<Self>) {
        let key = (Some(host.clone()), dir.clone());
        if self.inflight.contains(&key) {
            return;
        }
        if self
            .listings
            .get(&key)
            .is_some_and(|listing| listing.fresh())
        {
            return;
        }
        self.inflight.insert(key);
        let list_host = host.clone();
        let list_dir = dir.clone();
        cx.spawn(async move |this, cx| {
            let names = cx
                .background_executor()
                .spawn(async move {
                    kernel::list_remote_dirs(&list_host, &list_dir).unwrap_or_default()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let keep = this.current_offer(cx);
                this.inflight.remove(&(Some(host.clone()), dir.clone()));
                this.listings.insert(
                    (Some(host), dir),
                    DirListing {
                        names,
                        fetched: Instant::now(),
                    },
                );
                this.keep_cursor(keep, cx);
                this.ensure_listing(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn start(&mut self, host: Option<String>, path: String, cx: &mut Context<Self>) {
        self.open = false;
        let path = if path.is_empty() { "/".into() } else { path };
        let place = match host {
            Some(host) => Place::remote(host, path),
            // `~/Code` typed is the user's home, not a folder called `~`.
            None => Place::local(local_path(&path)),
        };
        cx.emit(OpenerEvent::Open(place));
        cx.notify();
    }

    fn row_label(offer: &Offer) -> String {
        match offer {
            Offer::Machine { name, .. } => name.clone(),
            Offer::Here(path) => format!("Open {}", folder_name(path)),
            Offer::Dir(name) => format!("{name}/"),
            Offer::Create(path) => format!("Create {path}"),
            Offer::Browse => "Browse folders…".to_string(),
        }
    }

    /// The dim readout beside a row: the whole path behind "Open <name>",
    /// the step-in mark behind a folder.
    fn row_detail(offer: &Offer) -> Option<String> {
        match offer {
            Offer::Here(path) => Some(path.clone()),
            Offer::Dir(_) => Some("›".to_string()),
            Offer::Machine { .. } | Offer::Create(_) | Offer::Browse => None,
        }
    }
}

/// The last name in a path, for "Open <name>": `/` for the root, the home
/// folder's own name for `~`.
fn folder_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    if trimmed == "~" {
        return dirs::home_dir()
            .and_then(|home| home.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "~".to_string());
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed).to_string()
}

/// `/~/Code` — a `~` typed after the field's own prefilled `/` — is `~/Code`.
fn untilde_slash(typed: &str) -> &str {
    match typed.strip_prefix('/') {
        Some(rest) if rest.starts_with('~') => rest,
        _ => typed,
    }
}

/// A typed local path as the file system knows it: `~` and `~/…` are the
/// user's home (this process's, the one the window runs as).
fn local_path(typed: &str) -> PathBuf {
    let typed = untilde_slash(typed);
    let typed = if typed.is_empty() { "/" } else { typed };
    if typed == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    }
    if let Some(rest) = typed.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(typed)
}

fn list_local(dir: &str) -> Vec<String> {
    let path = local_path(dir);
    let Ok(entries) = std::fs::read_dir(&path) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_dir() || kind.is_symlink())
                .unwrap_or(false)
        })
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "." || name == ".." {
                return None;
            }
            if name.starts_with('.') {
                return None;
            }
            if entry.path().is_dir() {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    names.sort_unstable();
    names.truncate(LIST_CAP);
    names
}

fn split_path(typed: &str) -> (String, String) {
    let typed = untilde_slash(typed.trim());
    if typed.is_empty() || typed == "/" {
        return ("/".into(), String::new());
    }
    // `~` alone is the home folder, not a name to look for under `/`.
    if typed == "~" {
        return ("~".into(), String::new());
    }
    if typed.ends_with('/') {
        let dir = typed.trim_end_matches('/');
        return (
            if dir.is_empty() {
                "/".into()
            } else {
                dir.into()
            },
            String::new(),
        );
    }
    match typed.rfind('/') {
        Some(0) => ("/".into(), typed[1..].into()),
        Some(i) => (typed[..i].into(), typed[i + 1..].into()),
        None => ("/".into(), typed.into()),
    }
}

fn join_dir(dir: &str, name: &str) -> String {
    if dir == "/" {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

fn common_prefix(names: &[String]) -> Option<String> {
    let first = names.first()?;
    let mut end = first.len();
    for name in names.iter().skip(1) {
        end = first
            .as_bytes()
            .iter()
            .zip(name.as_bytes())
            .take_while(|(a, b)| a == b)
            .count()
            .min(end);
    }
    if end == 0 {
        return None;
    }
    Some(first[..end].to_string())
}

impl Focusable for Opener {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.field.read(cx).focus_handle(cx)
    }
}

impl Render for Opener {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div();
        }
        let theme = Theme::of(cx).clone();
        self.keep_cursor(self.current_offer(cx), cx);
        let offers = self.offers(cx);
        let lit = self.cursor;
        let machine = match &self.stage {
            Stage::Machine => None,
            Stage::Folder { host } => Some(host.as_deref().unwrap_or(LOCAL_MACHINE).to_string()),
        };
        let empty = if offers.is_empty() {
            if self.is_listing(cx) {
                Some("listing…")
            } else if matches!(self.stage, Stage::Folder { .. }) {
                Some("no folders")
            } else {
                Some("no machines")
            }
        } else {
            None
        };

        let list = offers.iter().enumerate().fold(
            div()
                .id("opener-list")
                .flex()
                .flex_col()
                .pt(px(4.))
                .pb(px(6.))
                .max_h(px(LIST_MAX))
                .overflow_y_scroll()
                .track_scroll(&self.scroll),
            |list, (ix, offer)| {
                let label = Self::row_label(offer);
                let detail = Self::row_detail(offer);
                let opens = matches!(offer, Offer::Here(_));
                let offer = offer.clone();
                list.child(
                    div()
                        .id(("opener-row", ix))
                        .px(px(12.))
                        .h(px(32.))
                        .rounded(px(Theme::control_radius()))
                        .mx(px(6.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(10.))
                        .cursor_pointer()
                        .when(ix == lit, |el| el.bg(theme.surface_raised))
                        .hover(|el| el.bg(theme.surface_raised))
                        .child(
                            icons::icon(if opens {
                                icons::files::FOLDER_WITH_FILES
                            } else {
                                icons::files::FOLDER
                            })
                            .size(px(14.))
                            .text_color(if opens {
                                theme.accent
                            } else {
                                theme.text_faint
                            }),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_style(TextStyle::Callout)
                                .text_color(theme.text)
                                .child(SharedString::from(label)),
                        )
                        .when_some(detail, |el, detail| {
                            el.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_align(TextAlign::Right)
                                    .text_style(TextStyle::Caption)
                                    .text_color(theme.text_faint)
                                    .child(SharedString::from(detail)),
                            )
                        })
                        .when(ix == lit && opens, |el| {
                            el.child(
                                div()
                                    .flex_none()
                                    .text_style(TextStyle::Caption)
                                    .text_color(theme.text_faint)
                                    .child("↵"),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.take(offer.clone(), cx))),
                )
            },
        );

        let list = match empty {
            Some(note) => list.child(
                div()
                    .px(px(12.))
                    .h(px(32.))
                    .mx(px(6.))
                    .flex()
                    .items_center()
                    .text_style(TextStyle::Callout)
                    .text_color(theme.text_faint)
                    .child(note),
            ),
            None => list,
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(120.))
            .bg(Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.35,
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.open = false;
                    cx.emit(OpenerEvent::Dismiss);
                    cx.notify();
                }),
            )
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<PanelDrag>, _, cx| {
                    if let Some((from, shift)) = this.grip {
                        this.shift = shift + (event.event.position - from);
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .w(px(WIDTH))
                    .relative()
                    .left(self.shift.x)
                    .top(self.shift.y)
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        // The head of the panel is its grip: drag it and the
                        // whole panel follows.
                        div()
                            .id("opener-grip")
                            .flex()
                            .flex_col()
                            .cursor_grab()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &gpui::MouseDownEvent, _, _| {
                                    this.grip = Some((event.position, this.shift));
                                }),
                            )
                            .on_drag(PanelDrag, |_, _, _, cx| cx.new(|_| Empty))
                            .children(machine.as_ref().map(|name| {
                                div()
                                    .px(px(14.))
                                    .pt(px(10.))
                                    .text_style(TextStyle::Caption)
                                    .text_color(theme.text_faint)
                                    .child(SharedString::from(name.clone()))
                            }))
                            .child(
                                div()
                                    .px(px(14.))
                                    .pt(px(if machine.is_some() { 4. } else { 12. }))
                                    .pb(px(8.))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        icons::icon(if machine.is_some() {
                                            icons::files::FOLDER
                                        } else {
                                            icons::system::MAGNIFER
                                        })
                                        .size(px(13.))
                                        .text_color(theme.text_faint),
                                    )
                                    .child(
                                        // Clicks in the field select text; they
                                        // are not a hold on the grip.
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .cursor_text()
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation()
                                            })
                                            .child(self.field.clone()),
                                    ),
                            ),
                    )
                    .child(list)
                    .surface(&theme, SURFACE),
            )
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::pick_here))
            .on_action(cx.listener(Self::descend))
            .on_action(cx.listener(Self::ascend))
            .on_action(cx.listener(Self::back))
            .on_action(cx.listener(Self::next))
            .on_action(cx.listener(Self::previous))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::complete))
    }
}
