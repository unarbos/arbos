//! The permissions, as one piece of state the sheet and Settings both draw:
//! each row's status and where its request stands. Every read of the system
//! and every request runs off the UI thread; the rows say Requesting with a
//! spinner while they wait, and the centre keeps re-reading while a sheet
//! is up so a switch flipped in System Settings turns the row green by
//! itself.
//!
//! macOS prompts once, on a first-time ask, and often not at all for an
//! ad-hoc bundle: `CGRequestScreenCaptureAccess` comes back `false` with no
//! dialog. A request that returns false, or whose status has not moved two
//! seconds after the prompt, flips its row to "needs System Settings" with
//! the pane's deep link — the sheet is never left looking frozen at "Not
//! asked" (the Mac worker's finding on the stuck Allow).

use crate::permissions::{Permission, Requested, Status};
use bezel::gpui::{Context, Entity, Global, Task};
use std::{
    collections::HashSet,
    path::PathBuf,
    time::{Duration, Instant},
};

/// Where a row's request stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Nothing in flight.
    Idle,
    /// The request call is running off the UI thread.
    Requesting { since: Instant },
    /// The system's prompt should be up; the status is re-read until it
    /// moves or the wait runs out.
    Prompted { since: Instant },
    /// The system will not prompt for this: the pane is the way, and the
    /// row keeps re-reading so a switch there turns it green.
    NeedsSettings,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub permission: Permission,
    pub status: Status,
    pub phase: Phase,
    /// A URL the request opened (the System Settings pane), if it did.
    pub opened: Option<&'static str>,
}

/// How long a prompt gets to move the status before the row says the
/// system will not ask.
pub const PROMPT_WAIT: Duration = Duration::from_secs(2);
/// How often the rows re-read the system while a sheet shows.
pub const POLL: Duration = Duration::from_secs(1);

/// The one centre, reachable from every window: the main window's sheet
/// and the Settings pane draw the same rows.
pub struct Permissions(pub Entity<PermissionCenter>);

impl Global for Permissions {}

pub struct PermissionCenter {
    pub rows: Vec<Row>,
    pub project: Option<PathBuf>,
    /// A sheet or pane is showing the rows: keep re-reading.
    watchers: usize,
    /// Permissions something the user tried needed and did not have — the
    /// dot on the gear after "Skip for now".
    pub needed: HashSet<Permission>,
    /// "Enable all" is walking the rows.
    pub enabling_all: bool,
    _poll: Option<Task<()>>,
}

impl PermissionCenter {
    pub fn new(project: Option<PathBuf>) -> Self {
        Self {
            rows: Permission::applicable()
                .iter()
                .map(|permission| Row {
                    permission: *permission,
                    status: Status::NotAsked,
                    phase: Phase::Idle,
                    opened: None,
                })
                .collect(),
            project,
            watchers: 0,
            needed: HashSet::new(),
            enabling_all: false,
            _poll: None,
        }
    }

    pub fn row(&self, permission: Permission) -> Option<&Row> {
        self.rows.iter().find(|row| row.permission == permission)
    }

    fn row_mut(&mut self, permission: Permission) -> Option<&mut Row> {
        self.rows
            .iter_mut()
            .find(|row| row.permission == permission)
    }

    /// Every applicable permission stands granted (or cannot be asked here).
    pub fn all_settled(&self) -> bool {
        self.rows
            .iter()
            .all(|row| matches!(row.status, Status::Granted | Status::Unavailable(_)))
    }

    /// The gear's dot: something the user tried needed a permission that
    /// still is not granted.
    pub fn wants_attention(&self) -> bool {
        self.needed.iter().any(|permission| {
            self.row(*permission)
                .is_some_and(|row| !matches!(row.status, Status::Granted | Status::Unavailable(_)))
        })
    }

    /// Something the user tried needed `permission` and did not have it.
    pub fn note_needed(&mut self, permission: Permission, cx: &mut Context<Self>) {
        self.needed.insert(permission);
        cx.notify();
    }

    pub fn set_project(&mut self, project: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.project != project {
            self.project = project;
            self.refresh(cx);
        }
    }

    /// A sheet or pane started showing the rows: read now and keep reading.
    pub fn watch(&mut self, cx: &mut Context<Self>) {
        self.watchers += 1;
        self.refresh(cx);
        if self._poll.is_none() {
            self._poll = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(POLL).await;
                    let live = this
                        .update(cx, |this, cx| {
                            if this.watchers == 0 && !this.enabling_all && !this.requesting() {
                                this._poll = None;
                                return false;
                            }
                            this.refresh(cx);
                            true
                        })
                        .unwrap_or(false);
                    if !live {
                        break;
                    }
                }
            }));
        }
    }

    pub fn unwatch(&mut self) {
        self.watchers = self.watchers.saturating_sub(1);
    }

    fn requesting(&self) -> bool {
        self.rows
            .iter()
            .any(|row| matches!(row.phase, Phase::Requesting { .. } | Phase::Prompted { .. }))
    }

    /// Re-read every status off the UI thread and settle the phases: a
    /// prompt that moved the status is done; one that did not, two seconds
    /// on, says the system will not ask.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let project = self.project.clone();
        let permissions: Vec<Permission> = self.rows.iter().map(|row| row.permission).collect();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    permissions
                        .into_iter()
                        .map(|permission| (permission, permission.status(project.as_deref())))
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                for (permission, status) in read {
                    if let Some(row) = this.row_mut(permission) {
                        let granted = status == Status::Granted;
                        row.status = status;
                        row.phase = match (&row.phase, granted) {
                            (_, true) => Phase::Idle,
                            (Phase::Prompted { since }, false) if since.elapsed() > PROMPT_WAIT => {
                                Phase::NeedsSettings
                            }
                            (phase, false) => phase.clone(),
                        };
                    }
                }
                this.step_enable_all(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Ask for one permission, off the UI thread. The row shows Requesting,
    /// then Prompted while the status is re-read; a request the system
    /// answers with nothing goes straight to NeedsSettings.
    pub fn request(&mut self, permission: Permission, cx: &mut Context<Self>) {
        let Some(row) = self.row_mut(permission) else {
            return;
        };
        if matches!(row.phase, Phase::Requesting { .. }) {
            return;
        }
        row.phase = Phase::Requesting {
            since: Instant::now(),
        };
        let project = self.project.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (requested, opened) = cx
                .background_executor()
                .spawn(async move {
                    let mut opened: Option<&'static str> = None;
                    let requested = permission.request(project.as_deref(), &mut |url| {
                        opened = permission.settings_url().filter(|known| *known == url);
                    });
                    (requested, opened)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(row) = this.row_mut(permission) {
                    row.opened = opened;
                    row.phase = match requested {
                        Requested::Prompted => Phase::Prompted {
                            since: Instant::now(),
                        },
                        Requested::OpenedSettings | Requested::Unsupported => Phase::NeedsSettings,
                    };
                }
                this.watch_for_result(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Keep re-reading while a prompt is out, even with no sheet up.
    fn watch_for_result(&mut self, cx: &mut Context<Self>) {
        if self._poll.is_none() {
            self.watch(cx);
            self.unwatch();
        }
    }

    /// Ask for everything not yet granted, one prompt at a time: the next
    /// request goes out when the last one's status moved or its wait ran
    /// out, so the dialogs come one after another and never stack.
    pub fn enable_all(&mut self, cx: &mut Context<Self>) {
        self.enabling_all = true;
        self.step_enable_all(cx);
        self.watch_for_result(cx);
        cx.notify();
    }

    fn step_enable_all(&mut self, cx: &mut Context<Self>) {
        if !self.enabling_all {
            return;
        }
        if self.requesting() {
            return;
        }
        let next = self.rows.iter().find(|row| {
            matches!(row.status, Status::NotAsked | Status::Denied) && row.phase == Phase::Idle
        });
        match next.map(|row| row.permission) {
            Some(permission) => self.request(permission, cx),
            None => {
                self.enabling_all = false;
            }
        }
    }

    /// Open the System Settings pane for a row the system will not prompt
    /// for. The row stays NeedsSettings and keeps re-reading.
    pub fn open_settings(&mut self, permission: Permission, cx: &mut Context<Self>) {
        if let Some(url) = permission.settings_url() {
            cx.open_url(url);
            if let Some(row) = self.row_mut(permission) {
                row.opened = Some(url);
                row.phase = Phase::NeedsSettings;
            }
            self.watch_for_result(cx);
            cx.notify();
        }
    }

    /// A real capture attempt through ScreenCaptureKit. On macOS 15 and
    /// later an app appears in the Screen Recording list only after it has
    /// asked for shareable content, and that ask is what raises the dialog;
    /// the row's "Try a capture now" is that attempt, off the UI thread.
    pub fn try_capture(&mut self, cx: &mut Context<Self>) {
        if let Some(row) = self.row_mut(Permission::ScreenRecording) {
            row.phase = Phase::Requesting {
                since: Instant::now(),
            };
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let captured = cx
                .background_executor()
                .spawn(async move { crate::permissions::try_screen_capture() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(row) = this.row_mut(Permission::ScreenRecording) {
                    // Not granted yet: the dialog may be up, so give it the
                    // prompt's window before the row points at the pane.
                    row.phase = if captured {
                        Phase::Idle
                    } else {
                        Phase::Prompted {
                            since: Instant::now(),
                        }
                    };
                }
                this.refresh(cx);
                this.watch_for_result(cx);
            });
        })
        .detach();
    }
}
