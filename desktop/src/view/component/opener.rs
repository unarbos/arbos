//! Open a place in two steps: pick a machine, then a folder on it.

use crate::{
    kernel,
    model::place::{self, Place},
};
use bezel::{
    gpui::{
        self, App, Context, DragMoveEvent, Empty, Entity, EventEmitter, FocusHandle, Focusable,
        Hsla, KeyBinding, MouseButton, Pixels, Point, Render, SharedString, Task, Window, actions,
        div, prelude::*, px,
    },
    theme::{Glass, SurfaceStyle, TextStyle, Theme, Typeset},
    ui::{
        icons,
        input::{FieldEvent, TextField},
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

actions!(arbos_opener, [Submit, Next, Previous, Dismiss, Complete]);

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
    cx.bind_keys([
        KeyBinding::new("enter", Submit, ctx),
        KeyBinding::new("down", Next, ctx),
        KeyBinding::new("up", Previous, ctx),
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
                    Offer::Here(_) | Offer::Dir(_) => false,
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
                let mut out = Vec::new();
                if prefix.is_empty() {
                    out.push(Offer::Here(dir.clone()));
                }
                out.extend(
                    names
                        .iter()
                        .filter(|name| {
                            !name.starts_with('.')
                                && (prefix.is_empty() || name.to_lowercase().starts_with(&prefix_l))
                        })
                        .cloned()
                        .map(Offer::Dir),
                );
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
                let path = match offers.get(self.cursor) {
                    Some(Offer::Here(path)) => path.clone(),
                    Some(Offer::Dir(name)) => {
                        let (dir, _) = split_path(&self.query(cx));
                        join_dir(&dir, name)
                    }
                    _ => {
                        let typed = self.query(cx);
                        if typed.is_empty() {
                            "/".into()
                        } else {
                            typed.trim_end_matches('/').to_string()
                        }
                    }
                };
                self.start(host, path, cx);
            }
        }
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
                        Offer::Machine { .. } | Offer::Here(_) | Offer::Browse => None,
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
        cx.notify();
    }

    fn previous(&mut self, _: &Previous, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.offers(cx).len();
        if len == 0 {
            return;
        }
        self.cursor = self.cursor.checked_sub(1).unwrap_or(len - 1);
        cx.notify();
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.stage, Stage::Folder { .. }) {
            self.stage = Stage::Machine;
            self.cursor = 0;
            self.field.update(cx, |field, cx| {
                field.set_placeholder("machine", cx);
                field.clear(cx);
            });
            cx.notify();
            return;
        }
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
            None => Place::local(path),
        };
        cx.emit(OpenerEvent::Open(place));
        cx.notify();
    }

    fn row_label(offer: &Offer) -> String {
        match offer {
            Offer::Machine { name, .. } => name.clone(),
            Offer::Here(path) => path.clone(),
            Offer::Dir(name) => format!("{name}/"),
            Offer::Browse => "Browse folders…".to_string(),
        }
    }
}

fn list_local(dir: &str) -> Vec<String> {
    let path = PathBuf::from(if dir.is_empty() { "/" } else { dir });
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
    let typed = typed.trim();
    if typed.is_empty() || typed == "/" {
        return ("/".into(), String::new());
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
                .overflow_y_scroll(),
            |list, (ix, offer)| {
                let label = Self::row_label(offer);
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
                        .cursor_pointer()
                        .when(ix == lit, |el| el.bg(theme.surface_raised))
                        .hover(|el| el.bg(theme.surface_raised))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_style(TextStyle::Callout)
                                .text_color(theme.text)
                                .child(SharedString::from(label)),
                        )
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
            .on_action(cx.listener(Self::next))
            .on_action(cx.listener(Self::previous))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::complete))
    }
}
