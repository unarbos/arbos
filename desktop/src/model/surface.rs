//! What an agent opens that is not itself an agent: a terminal, a browser,
//! or a panel. One type, one owner, one focus rule.
//!
//! A subagent is a [`crate::model::session::ChatSession`] with a parent, not
//! a fourth kind here. The sidebar walks [`Child`] so both share one row.

use crate::model::project;
use bezel::gpui::Image;
use std::{path::PathBuf, sync::Arc};

/// Local identity of a surface, minted once when it is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SurfaceId(pub u64);

/// What the surface is, at the tree level. Kind picks the icon and the
/// renderer; chrome, open, close, and focus stay the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    Terminal,
    Browser,
    /// A detached job the agent left running.
    Process,
    /// Canvas, file, doc, image, plan, run — a document in a view.
    Panel,
}

/// How the surface is bound to the kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bind {
    Terminal {
        id: String,
        cwd: Option<String>,
    },
    /// The kernel's own page: where it is, and the last picture it sent.
    Browser {
        id: String,
        url: String,
        shot: Option<Arc<Image>>,
    },
    /// A job folder's journal. `id` is the kernel's `jN`. `live` is the
    /// output the kernel streamed (`Frame::Job`), capped; empty until the
    /// first frame, when the file at `log` is read instead. `done` is set
    /// by the last frame: `Some(None)` means killed.
    Process {
        id: String,
        log: PathBuf,
        live: String,
        done: Option<Option<i32>>,
    },
    Url(String),
    Path(PathBuf),
    Empty,
}

/// Anything an agent opened that is not an agent.
#[derive(Debug, Clone)]
pub struct Surface {
    pub id: SurfaceId,
    /// The parent session. `None` is a user-opened surface; those stay off
    /// this tree for now.
    pub owner: Option<u64>,
    pub kind: SurfaceKind,
    pub title: String,
    pub bind: Bind,
    /// Stable board-card id, so a snapshot names the same card tomorrow.
    pub card_id: String,
    /// The model-facing board kind (`canvas`, `files`, `plan`, …).
    pub board_kind: String,
    /// Board-local handle; what a tool call means by "#7".
    pub key: i32,
    /// When it was opened — the sidebar orders on this.
    pub touched: u128,
}

/// One child of an agent, as the sidebar walks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Child {
    Agent(u64),
    Surface(SurfaceId),
}

/// Which agent is in front, and whether a child of it fills the column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Focus {
    pub agent: u64,
    pub surface: Option<SurfaceId>,
}

impl Surface {
    pub fn new(
        id: SurfaceId,
        owner: Option<u64>,
        kind: SurfaceKind,
        title: String,
        bind: Bind,
        board_kind: &str,
        key: i32,
    ) -> Self {
        Self {
            id,
            owner,
            kind,
            title,
            bind,
            card_id: format!("s{}", id.0),
            board_kind: board_kind.to_owned(),
            key,
            touched: project::stamp(),
        }
    }

    pub fn path(&self) -> Option<&std::path::Path> {
        match &self.bind {
            Bind::Path(path) => Some(path),
            _ => None,
        }
    }

    pub fn terminal_id(&self) -> Option<&str> {
        match &self.bind {
            Bind::Terminal { id, .. } => Some(id),
            _ => None,
        }
    }

    /// The kernel's name for this row — `t1`, `b1`, `j3`. What a Board
    /// close names. Files and URLs have none.
    pub fn kernel_id(&self) -> Option<&str> {
        self.bind.kernel_id()
    }
}

impl Bind {
    pub fn kernel_id(&self) -> Option<&str> {
        match self {
            Self::Terminal { id, .. } | Self::Browser { id, .. } | Self::Process { id, .. } => {
                Some(id)
            }
            _ => None,
        }
    }
}

/// `example.com` out of `https://example.com/a/b`. None without a scheme.
pub fn host_of(url: &str) -> Option<&str> {
    let rest = url.split("://").nth(1)?;
    rest.split('/').next().filter(|host| !host.is_empty())
}

impl SurfaceKind {
    pub fn from_board(kind: &str) -> Self {
        match kind {
            "terminal" => Self::Terminal,
            "browser" => Self::Browser,
            "process" => Self::Process,
            _ => Self::Panel,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal",
            Self::Browser => "Browser",
            Self::Process => "Process",
            Self::Panel => "Panel",
        }
    }
}

impl Focus {
    pub fn agent(id: u64) -> Self {
        Self {
            agent: id,
            surface: None,
        }
    }

    pub fn on(agent: u64, surface: SurfaceId) -> Self {
        Self {
            agent,
            surface: Some(surface),
        }
    }
}
