//! What the app remembers between launches: the projects that were open, and
//! the appearance the user picked.
//!
//! Machine-written, unlike `settings.toml` — nothing here is worth hand
//! editing, and rewriting it must never cost a user their own comments.

use crate::model::{place::Place, settings};
use bezel::theme::{TextStyle, appearance::AppearanceMode};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

/// The four things a project holds. Which one a launch lands on is the last
/// one that was open, so the window comes back where it was left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Session,
    Board,
    Article,
    Table,
}

/// One remembered entry: which kind, and which of them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub kind: Kind,
    /// The file the entry is, or a table's key. An index would drift as
    /// siblings are added and removed between launches.
    pub id: String,
}

/// The file's shape as this build writes it. A file without the key was
/// written by a build from before the key existed; `restore` uses that to
/// tell a setting the user chose from one an old default wrote for them.
pub const STATE_VERSION: u32 = 3;

/// One tab of a project's side panel, as an address rather than as content.
/// `kind` is `process`; the others are not filed yet, because nothing can
/// prove on the next launch that a terminal page or a browser page is still
/// there (see `docs/side-panels-design.md`, the kernel handover list).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelEntry {
    pub kind: String,
    /// Where it points: a job's journal path. Read on the way back in, and
    /// a tab whose address is not on disk is not restored.
    pub id: String,
    /// What the row was called. A name, not a record: the address above is
    /// what proves the thing exists.
    #[serde(default)]
    pub title: String,
}

/// A filed tab that can be put back, and what it takes to draw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredTab {
    /// The kernel's own name for the job — `j3` — read from its folder.
    pub job: String,
    pub log: PathBuf,
    /// How it ended. `None` is a job the wrapper recorded as killed.
    pub exit: Option<i32>,
    pub title: String,
}

/// Whether a filed tab can be put back, and as what.
///
/// This is the whole decision, and it is a free function over one entry and
/// the disk so that a test can make the call the app makes. The rule it holds:
/// a tab comes back only when its record is there *now* — the journal is a
/// file and the `exit` file beside it says how the job ended. A live job, a
/// terminal page and a browser page are the kernel's own state, and it does
/// not replay them to a client that reattaches, so nothing else is restorable
/// yet (`docs/side-panels-design.md`, kernel handover 1).
pub fn restorable_tab(entry: &PanelEntry) -> Option<RestoredTab> {
    if entry.kind != "process" {
        return None;
    }
    let log = PathBuf::from(&entry.id);
    if !log.is_file() {
        return None;
    }
    let folder = log.parent()?;
    let exit = std::fs::read_to_string(folder.join("exit")).ok()?;
    let job = folder.file_name()?.to_string_lossy().into_owned();
    let title = if entry.title.is_empty() {
        job.clone()
    } else {
        entry.title.clone()
    };
    Some(RestoredTab {
        job,
        log,
        exit: exit.trim().parse::<i32>().ok(),
        title,
    })
}

/// A project's side panel between launches: whether it was open, the width
/// the person dragged it to, its tabs and which was in front.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelState {
    pub open: bool,
    pub width: Option<f32>,
    pub active: usize,
    pub tabs: Vec<PanelEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    #[serde(default)]
    pub version: u32,
    /// Places as one-line strings: a local path, or `host:folder`.
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default)]
    pub recents: Vec<String>,
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub appearance: AppearanceMode,
    /// Whether the vibrancy is off — bezel composites the window opaque.
    #[serde(default)]
    pub reduce_transparency: bool,
    /// Whether the text caret blinks. Off holds it lit.
    pub cursor_blink: bool,
    /// The body size the type ladder is scaled against, in points.
    pub text_size: f32,
    /// Whether the agent's prose is set for bionic reading — the front of
    /// each word heavier than the rest. Off unless switched on. Builds before
    /// `STATE_VERSION` 2 defaulted it on and wrote that `true` into every
    /// file they saved, so `restore` reads it only from a versioned file.
    pub bionic_reading: bool,
    /// The greys' oklch hue in degrees, and how much of it they carry. Zero
    /// chroma is the shipped neutral, whatever the hue says.
    pub hue: f32,
    pub chroma: f32,
    /// What each project was last showing, by place string. Last in the struct
    /// because a map renders as TOML tables, and a bare key after one of those
    /// belongs to it.
    #[serde(default)]
    pub last: BTreeMap<String, Entry>,
    /// Sidebar titles they typed, by place string. Missing means the folder
    /// name.
    #[serde(default)]
    pub names: BTreeMap<String, String>,
    /// Kernel session ids the user deleted, by place string. The kernel
    /// still holds the log; this list is what keeps a trash from coming
    /// back on the next launch.
    #[serde(default)]
    pub dismissed: BTreeMap<String, Vec<String>>,
    /// Whether the permissions sheet has been shown once. A first launch
    /// opens Settings › Permissions; after that it is a click away.
    #[serde(default)]
    pub permissions_seen: bool,
    /// The window's last frame — x, y, width, height in points — so a
    /// relaunch opens where the window was. Restored only when most of it
    /// lies on a screen that is there; otherwise the window is centred.
    #[serde(default)]
    pub frame: Option<[f32; 4]>,
    /// Each project's side panel, by place string: whether it was open, how
    /// wide, and its tabs. After `frame` and the other maps for the reason
    /// `last` gives — a bare key written after a table belongs to it.
    #[serde(default)]
    pub panels: BTreeMap<String, PanelState>,
}

/// What the body size may be set to, in points: the ladder's smallest measured
/// role to Title3's, so bezel's fixed chrome heights hold at either end. Read
/// on the way in as well as by the control, because a size out of range paints
/// an interface nobody can read the Settings tab to fix.
pub const TEXT_SIZE: (f32, f32) = (11., 17.);

/// The body size a fresh install reads at: Cursor's 14.
pub const DEFAULT_TEXT_SIZE: f32 = 14.;

/// Hand-written because a zeroed `text_size` is a font nobody can read, and a
/// missing state file resolves every field through here.
impl Default for State {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            projects: Vec::new(),
            recents: Vec::new(),
            active: 0,
            // Dark by default, as Cursor is; light stays a click away in
            // Settings › Appearance.
            appearance: AppearanceMode::Dark,
            // Opaque, as Cursor's window is; the palette is tuned for it.
            reduce_transparency: true,
            cursor_blink: true,
            // Cursor's prose is 14 px on the Mac (cycle 1's measurement);
            // bezel's 13 pt body read small beside it (Jacob, report
            // 2026-09-17-13: "needs to be closer to cursor in size").
            text_size: DEFAULT_TEXT_SIZE,
            // Off: Cursor's prose is plain; the weighted words are a reading
            // aid to switch on, not what a new user meets (Mac cycle 11).
            bionic_reading: false,
            hue: 0.,
            chroma: 0.,
            last: BTreeMap::new(),
            names: BTreeMap::new(),
            dismissed: BTreeMap::new(),
            permissions_seen: false,
            frame: None,
            panels: BTreeMap::new(),
        }
    }
}

/// `~/.config/arbos-desktop/state.toml`. `None` where there is no directory to
/// put it in, which is also the only case where nothing is remembered.
pub fn path() -> Option<PathBuf> {
    settings::dir().ok().map(|dir| dir.join("state.toml"))
}

/// Local folders that have since vanished are dropped — a renamed folder
/// would otherwise leave a tab no agent can spawn in. A remote place stays:
/// the folder lives on the host, not here. The active project is resolved
/// by the same string first, so dropping an earlier one doesn't shift it.
pub fn restore() -> State {
    let stored = load().unwrap_or_default();
    let active = stored.projects.get(stored.active).cloned();
    let projects: Vec<String> = stored
        .projects
        .into_iter()
        .filter_map(|raw| {
            let place = Place::parse(&raw)?;
            if !place.is_remote() && !place.path.is_dir() {
                return None;
            }
            Some(place.encode())
        })
        .collect();
    let recents: Vec<String> = stored
        .recents
        .into_iter()
        .filter_map(|raw| Place::parse(&raw).map(|place| place.encode()))
        .take(12)
        .collect();
    let active = active
        .and_then(|path| projects.iter().position(|open| *open == path))
        .unwrap_or(0);
    // Old builds keyed last by the absolute path and by `~`. One encode
    // form, or restore looks in the wrong drawer and the pane is empty.
    let mut last = BTreeMap::new();
    for (raw, entry) in stored.last {
        let key = Place::parse(&raw)
            .map(|place| place.encode())
            .unwrap_or(raw);
        last.entry(key).or_insert(entry);
    }
    // Panels are keyed the same way, and for the same reason: a key in the
    // old absolute-path form would leave the drawer looking unremembered.
    let mut panels = BTreeMap::new();
    for (raw, panel) in stored.panels {
        let key = Place::parse(&raw)
            .map(|place| place.encode())
            .unwrap_or(raw);
        panels.entry(key).or_insert(panel);
    }
    // A `true` from a file older than version 2 is the old default, not a
    // choice: the setting shipped on and every save wrote it back, so it
    // survived the default's flip (Jacob's Mac, twice). Only a versioned
    // file's word counts.
    let bionic_reading = stored.version >= 2 && stored.bionic_reading;
    State {
        version: STATE_VERSION,
        projects,
        recents,
        active,
        appearance: stored.appearance,
        reduce_transparency: stored.reduce_transparency,
        cursor_blink: stored.cursor_blink,
        // A file from before version 3 holding the old default (13, the
        // body's own size) carries the default, not a choice: it moves to
        // the new one. A size the person stepped to stays.
        text_size: if stored.version < 3 && stored.text_size == TextStyle::Body.size() {
            DEFAULT_TEXT_SIZE
        } else {
            stored.text_size.clamp(TEXT_SIZE.0, TEXT_SIZE.1)
        },
        bionic_reading,
        hue: stored.hue,
        chroma: stored.chroma,
        last,
        names: stored.names,
        dismissed: stored.dismissed,
        permissions_seen: stored.permissions_seen,
        frame: stored.frame,
        panels,
    }
}

fn load() -> Option<State> {
    let path = path()?;
    let body = std::fs::read_to_string(&path).ok()?;
    if let Ok(state) = toml::from_str(&body) {
        return Some(state);
    }
    // A half-written file must not become an empty workspace on the next
    // launch. Keep the broken copy, then try the last good one.
    let bak = path.with_extension("toml.bak");
    let _ = std::fs::copy(&path, path.with_extension("toml.bad"));
    std::fs::read_to_string(&bak)
        .ok()
        .and_then(|body| toml::from_str(&body).ok())
}

/// Best effort: a state file that cannot be written is not worth failing a
/// click over. Write beside the live file and rename over it so a crash
/// mid-save cannot leave TOML the next launch cannot read.
pub fn save(state: &State) {
    let Some(path) = path() else {
        return;
    };
    let Ok(body) = toml::to_string_pretty(state) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if path.is_file() {
        let _ = std::fs::copy(&path, path.with_extension("toml.bak"));
    }
    let tmp = path.with_extension("toml.tmp");
    if std::fs::write(&tmp, body).is_ok() {
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::testing::Scratch;

    fn job(dir: &std::path::Path, name: &str, exit: Option<&str>) -> PanelEntry {
        let folder = dir.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let log = folder.join("out.log");
        std::fs::write(&log, "tick 1\ntick 2\n").unwrap();
        if let Some(code) = exit {
            std::fs::write(folder.join("exit"), code).unwrap();
        }
        PanelEntry {
            kind: "process".into(),
            id: log.display().to_string(),
            title: "cargo test".into(),
        }
    }

    #[test]
    fn a_finished_job_comes_back_with_how_it_ended() {
        let dir = Scratch::new("panel-tab");
        let restored = restorable_tab(&job(dir.path(), "j3", Some("0\n"))).unwrap();
        assert_eq!(restored.job, "j3");
        assert_eq!(restored.exit, Some(0));
        assert_eq!(restored.title, "cargo test");
    }

    #[test]
    fn a_job_that_never_recorded_an_end_is_not_put_back() {
        let dir = Scratch::new("panel-tab");
        assert_eq!(
            restorable_tab(&job(dir.path(), "j4", None)),
            None,
            "with no exit file we cannot say what it did, so it is not drawn"
        );
    }

    #[test]
    fn a_journal_that_has_gone_takes_its_tab_with_it() {
        let dir = Scratch::new("panel-tab");
        let entry = job(dir.path(), "j5", Some("1"));
        std::fs::remove_file(&entry.id).unwrap();
        assert_eq!(restorable_tab(&entry), None);
    }

    #[test]
    fn a_killed_job_says_so_rather_than_reading_as_a_clean_exit() {
        let dir = Scratch::new("panel-tab");
        let restored = restorable_tab(&job(dir.path(), "j6", Some("killed"))).unwrap();
        assert_eq!(restored.exit, None, "not parseable as a code, and not 0");
    }

    #[test]
    fn a_terminal_or_a_page_is_not_restorable_yet() {
        let dir = Scratch::new("panel-tab");
        let mut entry = job(dir.path(), "t1", Some("0"));
        entry.kind = "terminal".into();
        assert_eq!(restorable_tab(&entry), None);
    }
}
