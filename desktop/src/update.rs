//! Keeping this app up to date with what CI published.
//!
//! The shape of it, in the order it happens:
//!
//! 1. ask the channel's feed what the newest build is (`arbos-update`'s
//!    [`Feed`]), and compare it with the pair this binary was built from
//!    (`build::version`)
//! 2. if it is newer, the bar turns into a blue Update button
//! 3. clicking it downloads the payload, showing how far along it is
//! 4. the bytes are checked against the feed — length, then digest, then the
//!    Ed25519 signature — before anything is unpacked
//! 5. the payload is unpacked beside the installed app and looked at
//! 6. the kernels this app started are stopped, and the tunnels with them
//! 7. the old app is swapped for the new one, with the old one kept until the
//!    new one has been checked in place
//! 8. the app relaunches, and finds its tabs and chats where it left them
//!
//! Every step from 5 on is [`arbos_update::install`]'s, which keeps the rule
//! that matters: there is no moment at which the app is half installed. A
//! failure anywhere puts the old app back and says why.
//!
//! ## Why the kernels are stopped
//!
//! The kernel ships inside the bundle and is replaced with it. A running
//! kernel holds its own copy of the old binary open, so the swap does not
//! disturb it — and that is the problem, not the relief: the new app would
//! come up and attach to a kernel from the build before it. So the kernels
//! this app started are asked to stop first, which is the shutdown they are
//! written for (`arbos-kernel` takes SIGTERM, ends its turns, and drops its
//! lock), and the new app starts new ones from the new binary. Transcripts
//! are on disk and sessions resume by id, so what is lost is the seconds of a
//! turn that was in flight and nothing else.

use crate::{
    build,
    model::{place::Place, settings},
};
use anyhow::{Context as _, Result, bail};
use arbos_update::{
    Available, Channel, Feed, Version,
    feed::{Download, Platform, current_arch},
    install, sign,
};
use bezel::gpui::{App, Context, Entity, Global, Task};
use futures::{StreamExt, channel::mpsc};
use std::{path::PathBuf, time::Duration};

/// How long after launch the first check runs. Long enough to be out of the
/// way of the window opening, short enough that the button is there before
/// anybody goes looking for it.
const FIRST_CHECK: Duration = Duration::from_secs(20);

/// And how often after that. The dev channel publishes a build per merge, so
/// checking hourly is already generous; nothing here is urgent enough to ask
/// more often than a person would.
const EVERY: Duration = Duration::from_secs(60 * 60);

/// A download that stops answering should not hold the button in
/// "Updating…" forever.
const NETWORK_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a kernel is given to stop before the update goes ahead anyway.
/// It ends its turns and drops its lock on SIGTERM; a kernel that is wedged
/// is not a reason to refuse an update, only a reason to stop waiting.
const KERNEL_STOP_WAIT: Duration = Duration::from_secs(10);

/// What the bar is showing.
#[derive(Debug, Clone)]
pub enum State {
    /// Nothing to say: this is the newest build, or nobody has looked yet.
    /// The bar shows the running version and nothing else.
    Idle,
    /// Looking. Still quiet — a check nobody asked for should not flicker.
    Checking,
    /// The channel could not be reached, or answered with something
    /// unreadable.
    ///
    /// Its own state, and visible, because the alternative was folding it into
    /// [`Self::Idle`] — and then a machine that has quietly stopped being able
    /// to reach the channel looks exactly like a machine that is up to date.
    /// The two mean opposite things and had the same face.
    ///
    /// Calm, though: a laptop on a train is not an error, and the app is
    /// working perfectly. The next check tries again.
    Unreachable { why: String },
    /// There is a newer build. This is the blue button.
    Ready(Box<Available>),
    /// Fetching it. `total` is `0` until the server says how big it is.
    Downloading {
        update: Box<Available>,
        got: u64,
        total: u64,
    },
    /// Checked, unpacked, and going into place.
    Installing(Box<Available>),
    /// Installed. The app is about to be replaced by itself.
    Restarting,
    /// It did not work, and the app is exactly as it was. The message is
    /// written to be read by a person, and the update stays offered so the
    /// button can be pressed again.
    Failed {
        why: String,
        update: Option<Box<Available>>,
    },
}

impl State {
    /// Whether a second press should do anything.
    fn busy(&self) -> bool {
        matches!(
            self,
            Self::Downloading { .. } | Self::Installing(_) | Self::Restarting
        )
    }
}

/// The window's updater, reachable from the settings window too.
///
/// A global for the same reason [`crate::model::permission_center::Permissions`]
/// is one: settings is its own window with its own view, and the thing it is
/// showing lives on the main one.
pub struct Updates(pub Entity<Updater>);

impl Global for Updates {}

/// The last time the channel was asked, and what came back.
#[derive(Debug, Clone)]
pub struct Checked {
    pub at: std::time::SystemTime,
    /// `None` when the channel answered — whether or not it had anything new.
    pub failed: Option<String>,
}

/// One step of the work, sent from the thread doing it.
enum Step {
    Progress { got: u64, total: u64 },
    Installing,
    Installed,
    Failed(String),
}

pub struct Updater {
    channel: Channel,
    state: State,
    /// The build this binary is, which is what "newer" is measured against.
    current: Version,
    /// A kernel this window is restarting, and why the last attempt failed.
    ///
    /// The restart itself is `Arbos::restart_stranger`'s; this is only what
    /// the bar shows while it happens. Stopping a kernel and starting its
    /// replacement takes long enough that without a state to show, a click on
    /// the control is indistinguishable from a click that did nothing —
    /// which is how the undispatched click was reported in the first place.
    restarting: Option<Place>,
    restart_failed: Option<(Place, String)>,
    /// Kernels serving open places that are not the build this app ships.
    ///
    /// Cached, and refreshed on a timer rather than per frame: finding one
    /// reads a file, runs a binary and asks a socket, none of which belongs in
    /// a render. The quiet case never reaches here — a stranger with nothing
    /// running in it is stopped and replaced at attach — so what is in this
    /// list is what needs a person.
    strangers: Vec<crate::kernel::Skew>,
    /// When that list was last built, so the bar can ask on every frame
    /// without the work happening on every frame.
    looked: Option<std::time::Instant>,
    /// When the channel was last asked, and whether it answered. `None` until
    /// the first check finishes.
    ///
    /// Shown in Settings as a row, not only in the bar's tooltip: a tooltip
    /// needs a pointer to rest on the control, which makes it unreachable to
    /// anything driving the app and easy to miss for anybody who is not
    /// already suspicious. The tooltip may repeat this; it may not be the only
    /// copy of it.
    checked: Option<Checked>,
    /// Held so dropping the updater stops the work.
    running: Option<Task<()>>,
    polling: Option<Task<()>>,
}

impl Updater {
    pub fn new(channel: Channel, cx: &mut Context<Self>) -> Self {
        let mut updater = Self {
            channel,
            state: State::Idle,
            current: build::version(),
            restarting: None,
            restart_failed: None,
            strangers: Vec::new(),
            looked: None,
            checked: None,
            running: None,
            polling: None,
        };
        // A build that was interrupted mid-swap left the old app beside where
        // it should be. Nothing else can put it back, and this is the first
        // moment anything runs.
        if let Ok(root) = installed_root()
            && matches!(install::recover(&root.path), Ok(true))
        {
            // Nothing to say to the user: what they have is what they had.
            eprintln!("arbos: put the previous build back after an interrupted update");
        }
        updater.start_polling(FIRST_CHECK, cx);
        updater
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn current(&self) -> &Version {
        &self.current
    }

    pub fn channel(&self) -> Channel {
        self.channel
    }

    /// When the channel was last asked and what it said. `None` before the
    /// first check finishes.
    pub fn checked(&self) -> Option<&Checked> {
        self.checked.as_ref()
    }

    /// Kernels serving open places that are not this build.
    pub fn strangers(&self) -> &[crate::kernel::Skew] {
        &self.strangers
    }

    /// The place whose kernel is being restarted, if any.
    pub fn restarting(&self) -> Option<&Place> {
        self.restarting.as_ref()
    }

    /// Why the last restart failed, if it did.
    pub fn restart_failed(&self) -> Option<&(Place, String)> {
        self.restart_failed.as_ref()
    }

    /// Mark a restart as under way, so the bar can say so.
    pub fn restart_began(&mut self, place: Place, cx: &mut Context<Self>) {
        self.restarting = Some(place);
        self.restart_failed = None;
        cx.notify();
    }

    /// And as finished, with the reason when it did not work.
    pub fn restart_ended(&mut self, place: Place, why: Option<String>, cx: &mut Context<Self>) {
        self.restarting = None;
        self.restart_failed = why.map(|why| (place, why));
        cx.notify();
    }

    /// Drop what was found, so the next frame looks again — after a restart
    /// the answer has changed and waiting thirty seconds to say so would look
    /// like the click did nothing.
    pub fn forget_strangers(&mut self, cx: &mut Context<Self>) {
        self.strangers.clear();
        self.looked = None;
        cx.notify();
    }

    /// Look again, at most this often. Called from the bar's render, which
    /// happens constantly; the work does not.
    pub fn look_for_strangers(&mut self, places: Vec<Place>, cx: &mut Context<Self>) {
        const HOW_OFTEN: Duration = Duration::from_secs(30);
        if self
            .looked
            .is_some_and(|last| last.elapsed() < HOW_OFTEN)
        {
            return;
        }
        self.looked = Some(std::time::Instant::now());
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    places
                        .iter()
                        .filter_map(crate::kernel::kernel_skew)
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |updater, cx| {
                updater.strangers = found;
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether this build can install an update at all: it knows the key to
    /// check a payload against, and it is running from something it can
    /// replace. A build from `cargo run` is neither, and says so rather than
    /// offering a button that cannot work.
    pub fn can_install(&self) -> Option<String> {
        if sign::built_in_key().is_none() {
            return Some("This build carries no update key, so it cannot check an update.".into());
        }
        match installed_root() {
            Ok(_) => None,
            Err(e) => Some(format!("{e}")),
        }
    }

    /// Where the app being updated lives. Shown in the bar, because "which
    /// copy am I running" is the first question when ⌘Space opens the wrong
    /// one — or opens nothing.
    pub fn installed_at(&self) -> Option<PathBuf> {
        installed_root().ok().map(|root| root.path)
    }

    pub fn set_channel(&mut self, channel: Channel, cx: &mut Context<Self>) {
        if self.channel == channel {
            return;
        }
        self.channel = channel;
        // The other channel's newest build may be older than this one, in
        // which case there is nothing to offer and the button should go.
        self.state = State::Idle;
        cx.notify();
        self.check(cx);
    }

    /// Ask the feed. Quiet: a check that finds nothing changes nothing.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        if self.state.busy() {
            return;
        }
        self.state = State::Checking;
        cx.notify();
        let channel = self.channel;
        let current = self.current.clone();
        self.running = Some(cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { look(channel, &current) })
                .await;
            let _ = this.update(cx, |updater, cx| {
                // A channel switch while the network was busy wins.
                if updater.channel != channel || updater.state.busy() {
                    return;
                }
                let why = found.as_ref().err().map(|e| format!("{e:#}"));
                updater.checked = Some(Checked {
                    at: std::time::SystemTime::now(),
                    failed: why.clone(),
                });
                updater.state = match found {
                    Ok(Some(update)) => State::Ready(Box::new(update)),
                    Ok(None) => State::Idle,
                    // Not a red bar — a laptop on a train is not an error, and
                    // the next check tries again. But not silence either: an
                    // app that has stopped being able to reach its channel
                    // must not wear the face of one that is up to date.
                    Err(_) => State::Unreachable {
                        why: why.unwrap_or_default(),
                    },
                };
                cx.notify();
            });
        }));
    }

    /// The button. Download, check, install, relaunch.
    ///
    /// `places` are the projects this window has open: their kernels are the
    /// ones running on the binary that is about to be replaced, so they are
    /// asked to stop before the swap. They are passed in at the press rather
    /// than kept up to date on every frame, because this is the only moment
    /// they matter.
    pub fn install(&mut self, places: Vec<Place>, cx: &mut Context<Self>) {
        if self.state.busy() {
            return;
        }
        let update = match &self.state {
            State::Ready(update) => update.clone(),
            State::Failed {
                update: Some(update),
                ..
            } => update.clone(),
            // Nothing offered: treat the press as "look again".
            _ => return self.check(cx),
        };
        if let Some(why) = self.can_install() {
            self.state = State::Failed {
                why,
                update: Some(update),
            };
            cx.notify();
            return;
        }

        self.state = State::Downloading {
            update: update.clone(),
            got: 0,
            total: update.download.size,
        };
        cx.notify();

        let (mut tx, mut rx) = mpsc::unbounded::<Step>();
        let download = update.download.clone();
        std::thread::Builder::new()
            .name("arbos-update".into())
            .spawn(move || {
                let outcome = fetch_and_install(&download, &places, &mut tx);
                let last = match outcome {
                    Ok(()) => Step::Installed,
                    Err(e) => Step::Failed(format!("{e:#}")),
                };
                let _ = tx.unbounded_send(last);
            })
            .ok();

        self.running = Some(cx.spawn(async move |this, cx| {
            while let Some(step) = rx.next().await {
                let finished = matches!(step, Step::Installed | Step::Failed(_));
                let update = update.clone();
                let _ = this.update(cx, |updater, cx| {
                    updater.state = match step {
                        Step::Progress { got, total } => State::Downloading { update, got, total },
                        Step::Installing => State::Installing(update),
                        Step::Installed => State::Restarting,
                        Step::Failed(why) => State::Failed {
                            why,
                            update: Some(update),
                        },
                    };
                    cx.notify();
                });
                if finished {
                    break;
                }
            }
            // Relaunching is the main thread's: it has to be the last thing
            // this process does, and the windows have to be gone first.
            let _ = cx.update(|cx| {
                if let Some(true) = this
                    .read_with(cx, |u, _| matches!(u.state, State::Restarting))
                    .ok()
                {
                    relaunch(cx);
                }
            });
        }));
    }

    /// Check now, and then every hour.
    fn start_polling(&mut self, first: Duration, cx: &mut Context<Self>) {
        self.polling = Some(cx.spawn(async move |this, cx| {
            let mut wait = first;
            loop {
                cx.background_executor().timer(wait).await;
                if this.update(cx, |updater, cx| updater.check(cx)).is_err() {
                    return;
                }
                wait = EVERY;
            }
        }));
    }
}

/// Fetch the feed and pick what this machine should be offered.
fn look(channel: Channel, current: &Version) -> Result<Option<Available>> {
    let platform = Platform::current().context("Arbos does not publish builds for this system")?;
    let body = agent()
        .get(channel.feed_url())
        .call()
        .with_context(|| format!("asking the {} channel", channel.as_str()))?
        .body_mut()
        .read_to_string()
        .context("reading the update feed")?;
    Ok(Feed::parse(&body)?.available(current, platform, current_arch()))
}

/// The whole of the work, on a thread of its own because every step of it
/// blocks. Progress goes back over `tx`; the last word is the caller's.
fn fetch_and_install(
    download: &Download,
    places: &[Place],
    tx: &mut mpsc::UnboundedSender<Step>,
) -> Result<()> {
    let key = sign::built_in_key().context("this build carries no update key")?;
    download.check_url()?;

    let payload = fetch(download, tx)?;

    // Before anything is unpacked, let alone moved: is this the file the feed
    // described, and was it signed by the key this build trusts?
    download
        .check_payload(&payload, &key)
        .context("the download did not verify")?;
    let _ = tx.unbounded_send(Step::Installing);

    let root = installed_root()?;
    let staged = {
        // The payload has to be a file for `ditto` and `tar` to read.
        let dir = settings::data_dir()?.join("updates");
        std::fs::create_dir_all(&dir).with_context(|| format!("making {}", dir.display()))?;
        let file = dir.join(download.file_name());
        std::fs::write(&file, &payload).with_context(|| format!("writing {}", file.display()))?;
        let staged = install::unpack(&file, download.format, &root.path);
        let _ = std::fs::remove_file(&file);
        staged?
    };
    // Refuse a payload that is not an Arbos *before* the old one is moved, so
    // the usual bad case never touches the installed app at all.
    install::check_tree(&staged, root.executable, root.kernel)
        .context("the download is not a usable Arbos")?;

    // The kernel under this app is about to be replaced, so the kernels
    // running on the old one are stopped rather than left to be attached to
    // by the new app. Tunnels go with them.
    crate::kernel::shutdown_tunnels();
    stop_kernels(places);

    let swap = install::Swap::begin(&root.path, &staged)?;
    // In place now. If this fails, `swap` going out of scope puts the old app
    // back before the error reaches anybody.
    install::check_tree(&root.path, root.executable, root.kernel)
        .context("the new build did not survive being moved into place")?;
    swap.commit()?;
    // The app is back at the path it has always had, and macOS is told so now
    // rather than eventually: ⌘Space, "arbos", return has to work the moment
    // this finishes, not after the system next re-indexes on its own.
    install::reindex(&root.path);
    Ok(())
}

/// Download, reporting how far along it is.
fn fetch(download: &Download, tx: &mut mpsc::UnboundedSender<Step>) -> Result<Vec<u8>> {
    use std::io::Read;

    let mut response = agent()
        .get(&download.url)
        .call()
        .with_context(|| format!("fetching {}", download.url))?;
    // The feed's size is the one to trust — it is signed over with the rest
    // of the entry — but a server that disagrees is worth noticing early.
    let total = download.size;
    let mut reader = response.body_mut().as_reader();
    let mut payload = Vec::with_capacity(total.min(256 * 1024 * 1024) as usize);
    let mut chunk = vec![0u8; 64 * 1024];
    let mut since_told = 0u64;
    loop {
        let read = reader.read(&mut chunk).context("the download stopped")?;
        if read == 0 {
            break;
        }
        payload.extend_from_slice(&chunk[..read]);
        if payload.len() as u64 > total {
            bail!("the download is longer than the feed says it is");
        }
        // Telling the bar about every 64 KB would be sixty repaints a second
        // for nothing. Half a percent is a pixel or two of the bar.
        since_told += read as u64;
        if since_told * 200 >= total.max(1) {
            since_told = 0;
            let _ = tx.unbounded_send(Step::Progress {
                got: payload.len() as u64,
                total,
            });
        }
    }
    Ok(payload)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(NETWORK_TIMEOUT))
        .build()
        .into()
}

/// Where this app is installed, and what has to be inside it.
struct Installed {
    /// What is replaced: `Arbos.app`, or the directory the Linux build was
    /// unpacked into.
    path: PathBuf,
    executable: &'static str,
    kernel: &'static str,
}

/// The tree this binary is running out of, when that is a tree an update can
/// replace.
///
/// A `cargo run` build is deliberately not one: its kernel is built somewhere
/// else entirely, so there is nothing here to swap, and saying so is better
/// than a button that fails at the last step.
fn installed_root() -> Result<Installed> {
    let exe = std::env::current_exe().context("this program cannot find itself on disk")?;
    let beside = exe
        .parent()
        .context("this program is not in a directory")?
        .to_path_buf();

    #[cfg(target_os = "macos")]
    {
        // `<something>.app/Contents/MacOS/Arbos`.
        //
        // A closure rather than `Path::parent`: the type would have to be
        // imported, and an import only this block uses is an unused one
        // everywhere else — which is the warning that hid this from the Linux
        // build until CI reached a Mac.
        let app = beside
            .parent()
            .and_then(|contents| contents.parent())
            .filter(|app| app.extension().is_some_and(|e| e == "app"))
            .context(
                "this copy of Arbos is not an installed app, so it cannot update itself — \
                 drag Arbos.app to your Applications folder and open it from there",
            )?;
        return Ok(Installed {
            path: app.to_path_buf(),
            executable: "Arbos",
            kernel: "arbos-kernel",
        });
    }

    #[cfg(not(target_os = "macos"))]
    {
        if !beside.join("arbos-kernel").is_file() {
            bail!(
                "this copy of Arbos was not installed from a release — its kernel is not beside \
                 it, so there is nothing here to replace"
            );
        }
        Ok(Installed {
            path: beside,
            executable: "arbos-desktop",
            kernel: "arbos-kernel",
        })
    }
}

/// Ask every kernel this window started to stop, and wait for it.
///
/// SIGTERM is the kernel's own shutdown: it ends the turns it is running,
/// stops its remote children, and drops the place lock. What it is not is a
/// kill — a kernel that ignores it is left alone and the update goes ahead,
/// because the new app will find the port busy and say so, which is a better
/// end than an update that refuses to happen.
///
/// Only the kernels on this machine. A remote place's kernel runs from that
/// host's own binary, which this update does not touch; the tunnels to it are
/// already down by the time this is called, and the new app dials them again.
///
/// **This is the first of two layers and it cannot be the only one.** It
/// reaches the places the caller knows about, which is not every kernel on the
/// machine — one started by the CLI, by a worker, or by another window is
/// invisible here — and a kernel that does not stop inside
/// [`KERNEL_STOP_WAIT`] is deliberately left running rather than blocking the
/// update. So the app must also refuse to *attach* to a kernel that is not the
/// build it ships, which is the layer that holds however this one fails. See
/// `docs/kernel-self-update-design.md`.
fn stop_kernels(places: &[Place]) {
    let mut asked = Vec::new();
    for place in places.iter().filter(|place| !place.is_remote()) {
        let Some(info) = live_kernel(&arbos_core::Place::new(place.path.clone())) else {
            continue;
        };
        #[cfg(unix)]
        if info.pid > 0 {
            // SAFETY: a signal to a pid. The worst a stale pid can do is
            // reach a process that is not ours, which the kernel is not
            // allowed to do and the OS refuses.
            unsafe {
                libc::kill(info.pid, libc::SIGTERM);
            }
        }
        asked.push(info);
    }
    if asked.is_empty() {
        return;
    }
    let until = std::time::Instant::now() + KERNEL_STOP_WAIT;
    while std::time::Instant::now() < until {
        if !asked.iter().any(answers) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct LiveKernel {
    pid: i32,
    address: Option<std::net::SocketAddr>,
}

/// What a place's `kernel.json` says, when there is one. `kernel_json_read`
/// is the one that also looks where older kernels wrote it.
fn live_kernel(place: &arbos_core::Place) -> Option<LiveKernel> {
    let text = std::fs::read_to_string(place.kernel_json_read()).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(LiveKernel {
        pid: json.get("pid")?.as_i64()? as i32,
        address: json
            .get("url")
            .and_then(|u| u.as_str())
            .and_then(crate::kernel::tcp_addr),
    })
}

/// Whether it is still listening. The port going quiet is the kernel being
/// gone; the pid is not, because a pid is reused.
fn answers(kernel: &LiveKernel) -> bool {
    kernel.address.is_some_and(|address| {
        std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok()
    })
}

/// Start the app that is now on disk, and leave.
///
/// The new process comes up and reads the same `state.toml` and the same
/// session files this one has been writing all along, so it opens the same
/// projects, the same tab, and the same chat. `on_app_quit` flushes what is
/// still queued before the process ends.
fn relaunch(cx: &mut App) {
    if let Ok(root) = installed_root() {
        #[cfg(target_os = "macos")]
        {
            // `open -n` rather than exec'ing the binary: it is what asks
            // Launch Services to start a *new* instance of the bundle, with
            // the bundle's identity, its icon in the Dock, and its
            // permissions — an exec'd binary has none of that.
            let _ = std::process::Command::new("/usr/bin/open")
                .arg("-n")
                .arg(&root.path)
                .spawn();
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = std::process::Command::new(root.path.join(root.executable)).spawn();
        }
    }
    cx.quit();
}

/// What the settings file says, as a channel.
pub fn channel_of(settings: &settings::Settings) -> Channel {
    // An environment variable wins, so a machine can be put on dev for one
    // run without the file being edited.
    std::env::var("ARBOS_UPDATE_CHANNEL")
        .ok()
        .and_then(|name| Channel::parse(&name))
        .unwrap_or_else(|| Channel::parse(&settings.update.channel).unwrap_or_default())
}

/// The channel this app follows, read from the settings file directly.
///
/// [`channel_of`] wants a `Settings` a model is holding. Placing a kernel
/// on another machine happens on a background thread with no window and
/// no model, and it needs the same answer — the kernel it puts there has
/// to come from the same channel the app updates itself from, or the two
/// ends of a tunnel drift apart by design.
pub fn channel_now() -> Channel {
    settings::load()
        .map(|settings| channel_of(&settings))
        .unwrap_or_default()
}

/// Whether this build knows the key an update has to be signed with.
///
/// False for anything built before the repository had a signing key, which
/// can never install an update — see `crates/arbos-update/update-key.pub`.
pub fn built_in_key_present() -> bool {
    sign::built_in_key().is_some()
}
