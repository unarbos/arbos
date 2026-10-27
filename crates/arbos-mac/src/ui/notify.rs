//! System notifications (D49).
//!
//! The [`Coalescer`] is the policy: at most one notification per Project per
//! [`WINDOW_MS`]; events that land inside the window fold into one "N updates
//! in <Project>"; nothing fires while that Project is streaming (the turn end
//! will) or while the window is focused and the chat is on screen. The
//! [`Notifier`] wraps it with the platform backend: `UNUserNotificationCenter`
//! on macOS (the bundled app; `osascript` when run unbundled from `cargo run`),
//! `notify-send` on Linux.
//!
//! Message notifications look like Grok Bot's: title = sender, body = preview,
//! the sender's chat image as the attachment, one thread per chat. Clicking
//! one activates the app and opens that chat ([`take_clicks`]). The Dock icon
//! badge mirrors the total unread count ([`set_badge`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

pub const WINDOW_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NotifyKind {
    TurnFinished,
    NeedsInput,
    /// Raised by F2 approval gates (`AppState::notify_approval_requested`).
    #[allow(dead_code)]
    ApprovalRequested,
    WakeFailed,
    SpendCapReached,
    CrossProject,
    RoutineRan,
    /// A person wrote in a people chat (D72).
    DirectMessage,
}

impl NotifyKind {
    /// A message from someone (agent reply or a person's line): the banner is
    /// titled with the sender, like a chat app, not with the event.
    fn is_message(self) -> bool {
        matches!(
            self,
            NotifyKind::TurnFinished | NotifyKind::DirectMessage | NotifyKind::CrossProject
        )
    }

    fn title(self, project: &str) -> String {
        match self {
            NotifyKind::TurnFinished => format!("{project} · done"),
            NotifyKind::NeedsInput => format!("{project} · needs your input"),
            NotifyKind::ApprovalRequested => format!("{project} · approval requested"),
            NotifyKind::WakeFailed => format!("{project} · wake failed"),
            NotifyKind::SpendCapReached => format!("{project} · spend cap reached"),
            NotifyKind::CrossProject => format!("{project} · message from another project"),
            NotifyKind::RoutineRan => format!("{project} · routine ran"),
            NotifyKind::DirectMessage => format!("@{project}"),
        }
    }
}

/// One thing the app wants to tell the user about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotifyEvent {
    pub project_id: String,
    pub project_name: String,
    pub kind: NotifyKind,
    /// Chat title, question text, error… ≤ one line.
    pub detail: String,
    /// The chat it belongs to: the notification thread, and what a click opens.
    pub chat_id: Option<String>,
    /// Who wrote it (message kinds): the banner title.
    pub sender: Option<String>,
    /// The sender's chat image as a PNG on disk (see [`image_file`]).
    pub image: Option<PathBuf>,
}

impl NotifyEvent {
    pub fn new(
        project_id: impl Into<String>,
        project_name: impl Into<String>,
        kind: NotifyKind,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            project_id: project_id.into(),
            project_name: project_name.into(),
            kind,
            detail: detail.into(),
            chat_id: None,
            sender: None,
            image: None,
        }
    }
}

/// What actually reaches the OS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    pub project_id: String,
    pub title: String,
    pub body: String,
    /// How many events this one stands for.
    pub count: usize,
    pub chat_id: Option<String>,
    pub image: Option<PathBuf>,
}

/// Why an event may not fire right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gate {
    /// The Project has a streaming turn; hold until it finishes.
    pub streaming: bool,
    /// Window focused and this Project's chat on screen; the user sees it.
    pub visible: bool,
}

struct Pending {
    latest: NotifyEvent,
    count: usize,
}

#[derive(Default)]
pub struct Coalescer {
    last_sent: HashMap<String, u64>,
    pending: HashMap<String, Pending>,
}

impl Coalescer {
    /// Offer an event at `now_ms`. Returns a notification when it may fire
    /// immediately; otherwise it is folded into the Project's pending batch
    /// (or dropped when the user is already looking).
    pub fn offer(&mut self, event: NotifyEvent, now_ms: u64, gate: Gate) -> Option<Notification> {
        if gate.visible {
            self.pending.remove(&event.project_id);
            return None;
        }
        let project = event.project_id.clone();
        let in_window = self
            .last_sent
            .get(&project)
            .is_some_and(|last| now_ms.saturating_sub(*last) < WINDOW_MS);
        let held = self.pending.contains_key(&project);
        if gate.streaming || in_window || held {
            let slot = self.pending.entry(project).or_insert(Pending {
                latest: event.clone(),
                count: 0,
            });
            slot.latest = event;
            slot.count += 1;
            return None;
        }
        self.last_sent.insert(project.clone(), now_ms);
        Some(render(&event, 1))
    }

    /// Flush batches whose window has elapsed. `gate_of` answers for each
    /// Project; held batches stay, visible ones are dropped.
    pub fn tick(
        &mut self,
        now_ms: u64,
        mut gate_of: impl FnMut(&str) -> Gate,
    ) -> Vec<Notification> {
        let mut out = Vec::new();
        let projects: Vec<String> = self.pending.keys().cloned().collect();
        for project in projects {
            let gate = gate_of(&project);
            if gate.visible {
                self.pending.remove(&project);
                continue;
            }
            if gate.streaming {
                continue;
            }
            let elapsed = self
                .last_sent
                .get(&project)
                .map(|last| now_ms.saturating_sub(*last) >= WINDOW_MS)
                .unwrap_or(true);
            if !elapsed {
                continue;
            }
            if let Some(p) = self.pending.remove(&project) {
                self.last_sent.insert(project, now_ms);
                out.push(render(&p.latest, p.count));
            }
        }
        out
    }

    #[cfg(test)]
    pub fn pending_count(&self, project_id: &str) -> usize {
        self.pending.get(project_id).map(|p| p.count).unwrap_or(0)
    }
}

fn render(event: &NotifyEvent, count: usize) -> Notification {
    if count > 1 {
        return Notification {
            project_id: event.project_id.clone(),
            title: format!("{count} updates in {}", event.project_name),
            body: event.detail.clone(),
            count,
            chat_id: event.chat_id.clone(),
            image: event.image.clone(),
        };
    }
    let title = match (&event.sender, event.kind.is_message()) {
        (Some(sender), true) if !sender.is_empty() => sender.clone(),
        _ => event.kind.title(&event.project_name),
    };
    Notification {
        project_id: event.project_id.clone(),
        title,
        body: event.detail.clone(),
        count: 1,
        chat_id: event.chat_id.clone(),
        image: event.image.clone(),
    }
}

/// Deliver through the platform's notification center.
pub fn deliver(n: &Notification) {
    if std::env::var_os("ARBOS_NOTIFY_DRY_RUN").is_some() {
        eprintln!(
            "notify: {} — {}{}{}",
            n.title,
            n.body,
            n.chat_id
                .as_deref()
                .map(|c| format!(" [chat {c}]"))
                .unwrap_or_default(),
            n.image
                .as_deref()
                .map(|p| format!(" [image {}]", p.display()))
                .unwrap_or_default()
        );
        return;
    }
    #[cfg(target_os = "macos")]
    {
        if mac::available() {
            mac::deliver(n);
            return;
        }
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            applescript_escape(&n.body),
            applescript_escape(&n.title)
        );
        spawn_quiet(Command::new("osascript").arg("-e").arg(script));
    }
    #[cfg(target_os = "linux")]
    {
        let mut cmd = Command::new("notify-send");
        cmd.arg("--app-name=Arbos").arg("--expire-time=8000");
        if let Some(image) = &n.image {
            cmd.arg(format!("--icon={}", image.display()));
        }
        match &n.chat_id {
            // libnotify ≥ 0.7.10: `--wait` prints the action the user chose.
            Some(chat) => {
                cmd.arg("--action=default=Open").arg("--wait");
                let chat = chat.clone();
                spawn_then(cmd.arg(&n.title).arg(&n.body), move |out| {
                    if out.lines().any(|l| l.trim() == "default") {
                        push_click(chat);
                    }
                });
            }
            None => spawn_quiet(cmd.arg(&n.title).arg(&n.body)),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("notify: {} — {}", n.title, n.body);
    }
}

// ----- platform glue shared by the backends --------------------------------

/// Where sender images for notifications live (`$TMPDIR/arbos-notify`).
pub fn image_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("arbos-notify");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Write `rgba` (`w`×`h`, straight alpha) as `<key>.png` in [`image_dir`],
/// once per key. Returns the path to hand to the notification backend.
pub fn image_file(key: &str, w: u32, h: u32, rgba: &[u8]) -> Option<PathBuf> {
    let safe: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = image_dir().join(format!("{safe}.png"));
    if path.is_file() {
        return Some(path);
    }
    let img = image::RgbaImage::from_raw(w, h, rgba.to_vec())?;
    img.save(&path).ok()?;
    Some(path)
}

/// A fresh copy of `path` for one notification (the macOS center moves the
/// attachment file into its own store, so the cached PNG must stay put).
#[cfg(target_os = "macos")]
fn image_copy(path: &Path) -> Option<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let name = path.file_stem()?.to_string_lossy().to_string();
    let copy = image_dir().join(format!("{name}-{stamp}.png"));
    std::fs::copy(path, &copy).ok()?;
    Some(copy)
}

/// Chats whose notification the user clicked since the last call.
pub fn take_clicks() -> Vec<String> {
    std::mem::take(&mut *clicks().lock().unwrap())
}

fn clicks() -> &'static Mutex<Vec<String>> {
    static CLICKS: std::sync::OnceLock<Mutex<Vec<String>>> = std::sync::OnceLock::new();
    CLICKS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Record a click on a chat's notification (the delegate on macOS; tests).
pub fn push_click(chat_id: String) {
    clicks().lock().unwrap().push(chat_id);
}

/// The chat was read — here or on another device of the user (clear-on-read): every
/// notification of that thread leaves Notification Center. Call banners are not touched
/// (`withdraw_call` owns those). No-op where the platform keeps no delivered list.
pub fn withdraw_chat(chat_id: &str) {
    if chat_id.is_empty() {
        return;
    }
    if std::env::var_os("ARBOS_NOTIFY_DRY_RUN").is_some() {
        eprintln!("notify: withdraw chat {chat_id}");
    } else {
        #[cfg(target_os = "macos")]
        if mac::available() {
            mac::withdraw_chat(chat_id);
        }
    }
}

/// Which delivered request identifiers a read of `chat_id` withdraws: the chat's message
/// banners (`chat:<id>:<stamp>`) and nothing else — not another chat's, not a call's.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn withdrawn_ids<'a>(chat_id: &str, ids: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let prefix = format!("chat:{chat_id}:");
    ids.into_iter()
        .filter(|id| {
            id.strip_prefix(&prefix)
                .is_some_and(|stamp| !stamp.contains(':'))
        })
        .map(str::to_owned)
        .collect()
}

/// The app's unread badge. `0` clears it. Dock icon on macOS; elsewhere only
/// remembered (the rail already shows the per-project counts).
pub fn set_badge(total: usize) {
    static LAST: AtomicUsize = AtomicUsize::new(usize::MAX);
    if LAST.swap(total, Ordering::Relaxed) == total {
        return;
    }
    if std::env::var_os("ARBOS_NOTIFY_DRY_RUN").is_some() {
        eprintln!("notify: badge {total}");
    }
    #[cfg(target_os = "macos")]
    mac::set_badge(total);
    #[cfg(not(target_os = "macos"))]
    let _ = total;
}

/// Install the notification-center delegate (clicks, foreground banners) and
/// ask the OS where we stand on permission. Call once on the main thread at
/// start-up; off macOS only the permission answer is recorded.
pub fn init() {
    #[cfg(target_os = "macos")]
    mac::init();
    refresh_permission();
}

/// Does this platform ask the user before showing notifications?
pub fn needs_permission() -> bool {
    if std::env::var_os("ARBOS_NOTIFY_PROMPT").is_some() {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        mac::available()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// What the OS says about our permission to post notifications. The app used
/// to remember only "the explainer was shown once" in a marker file, so a
/// "Not now" (or a build before the explainer existed) left macOS undecided
/// for good and every banner failed with "notifications are not allowed" on
/// stderr — the cause of "no Mac notifications" reports. The system's own
/// answer is the only state that counts now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// The OS has not answered yet (asked at start-up; a few ms).
    Unknown,
    /// Never asked, or asked and dismissed without an answer: ask again.
    NotDetermined,
    /// The user said no in the system prompt or turned us off in System Settings.
    Denied,
    Allowed,
}

static PERMISSION: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn permission() -> Permission {
    match PERMISSION.load(Ordering::Relaxed) {
        1 => Permission::NotDetermined,
        2 => Permission::Denied,
        3 => Permission::Allowed,
        _ => Permission::Unknown,
    }
}

fn set_permission(p: Permission) {
    let v = match p {
        Permission::Unknown => 0,
        Permission::NotDetermined => 1,
        Permission::Denied => 2,
        Permission::Allowed => 3,
    };
    PERMISSION.store(v, Ordering::Relaxed);
}

/// Ask the OS again (asynchronous on macOS; the answer lands in [`permission`]).
pub fn refresh_permission() {
    if std::env::var_os("ARBOS_NOTIFY_PROMPT").is_some() {
        // Rigs: the explainer shows until Allow / Not now is clicked.
        if permission() == Permission::Unknown {
            set_permission(Permission::NotDetermined);
        }
        return;
    }
    #[cfg(target_os = "macos")]
    {
        if mac::available() {
            mac::refresh_permission();
            return;
        }
    }
    // osascript / notify-send: nothing to ask.
    set_permission(Permission::Allowed);
}

/// Show the system permission prompt (macOS). Elsewhere nothing to ask.
pub fn request_permission() {
    #[cfg(target_os = "macos")]
    {
        if mac::available() {
            mac::request_authorization();
            return;
        }
    }
    set_permission(Permission::Allowed);
}

const ASKED_MARK: &str = "notifications-asked";
const SNOOZE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// "Not now" on the explainer snoozes it for a day (the marker's age); macOS
/// stays undecided, so after that the card comes back — before this the
/// marker was permanent and one "Not now" meant no banners ever. Rigs write
/// the marker fresh before each run to keep the card out of screenshots.
pub fn prompt_snoozed(config_dir: &Path) -> bool {
    std::fs::metadata(config_dir.join(ASKED_MARK))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < SNOOZE)
}

pub fn snooze_prompt(config_dir: &Path) {
    let _ = std::fs::create_dir_all(config_dir);
    let _ = std::fs::write(config_dir.join(ASKED_MARK), b"1");
}

/// The user said "Not now": the OS stays undecided; this launch stays quiet.
pub fn permission_dismissed(config_dir: &Path) {
    snooze_prompt(config_dir);
    if std::env::var_os("ARBOS_NOTIFY_PROMPT").is_some() {
        set_permission(Permission::Denied);
    }
}

/// System Settings → Notifications → Arbos (the only place a denied
/// permission can be turned back on).
pub fn open_system_settings() {
    #[cfg(target_os = "macos")]
    {
        let bundle = mac::bundle_id().unwrap_or_else(|| "sh.arbos.mac".to_string());
        spawn_quiet(Command::new("open").arg(format!(
            "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id={bundle}"
        )));
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn spawn_quiet(cmd: &mut Command) {
    let spawned = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            // Reap in the background; the OS notifier returns at once.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(err) => eprintln!("notify: backend unavailable: {err}"),
    }
}

/// Calls v1: the ringing banner — "<caller>" / "Incoming video call" with
/// Accept and Decline (category `ARBOS_CALL`). Clicking the banner body
/// accepts. The click lands in [`take_clicks`] as `call:accept:<id>` /
/// `call:decline:<id>`.
pub fn deliver_call(call_id: &str, chat_id: &str, title: &str, body: &str, image: Option<PathBuf>) {
    if std::env::var_os("ARBOS_NOTIFY_DRY_RUN").is_some() {
        eprintln!("notify: call {call_id} in {chat_id}: {title} — {body}");
        return;
    }
    #[cfg(target_os = "macos")]
    {
        if mac::available() {
            mac::deliver_call(call_id, chat_id, title, body, image.as_deref());
            return;
        }
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            applescript_escape(body),
            applescript_escape(title)
        );
        spawn_quiet(Command::new("osascript").arg("-e").arg(script));
    }
    #[cfg(target_os = "linux")]
    {
        let mut cmd = Command::new("notify-send");
        cmd.arg("--app-name=Arbos")
            .arg("--expire-time=30000")
            .arg("--urgency=critical");
        if let Some(image) = &image {
            cmd.arg(format!("--icon={}", image.display()));
        }
        cmd.arg("--action=accept=Accept")
            .arg("--action=decline=Decline")
            .arg("--wait")
            .arg(title)
            .arg(body);
        let id = call_id.to_string();
        spawn_then(&mut cmd, move |out| {
            let choice = out.trim();
            if choice == "accept" || choice == "default" {
                push_click(format!("call:accept:{id}"));
            } else if choice == "decline" {
                push_click(format!("call:decline:{id}"));
            }
        });
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (title, body, image);
    }
}

/// The call ended before it was answered: take the ringing banner down.
pub fn withdraw_call(call_id: &str) {
    #[cfg(target_os = "macos")]
    {
        if mac::available() {
            mac::withdraw_call(call_id);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = call_id;
    }
}

/// Run `cmd`, hand its stdout to `then` when it exits (notification clicks).
#[cfg(target_os = "linux")]
fn spawn_then(cmd: &mut Command, then: impl FnOnce(String) + Send + 'static) {
    let spawned = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(child) => {
            std::thread::spawn(move || {
                if let Ok(out) = child.wait_with_output() {
                    then(String::from_utf8_lossy(&out.stdout).to_string());
                }
            });
        }
        Err(err) => eprintln!("notify: backend unavailable: {err}"),
    }
}

#[cfg(target_os = "macos")]
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `UNUserNotificationCenter` through objc2. Only usable from a bundle (the
/// center refuses processes without a bundle identifier), so the unbundled
/// dev binary keeps the `osascript` path.
#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicU8, Ordering};

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSSet;
    use objc2_foundation::{
        NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSString, NSURL,
    };
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent,
        UNNotification, UNNotificationAction, UNNotificationActionOptions,
        UNNotificationAttachment, UNNotificationCategory, UNNotificationCategoryOptions,
        UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
        UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };

    use super::Notification;

    /// Request identifiers carry the chat: `chat:<id>:<stamp>`.
    const CHAT_PREFIX: &str = "chat:";
    /// Ringing banners: `call:<call_id>:<chat_id>`; category `ARBOS_CALL`.
    const CALL_PREFIX: &str = "call:";
    const CALL_CATEGORY: &str = "ARBOS_CALL";
    const ACCEPT: &str = "ACCEPT";
    const DECLINE: &str = "DECLINE";

    define_class!(
        // SAFETY: NSObject has no subclassing requirements and the type
        // does not implement Drop.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "ArbosNotifyDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            // Banners while the app is frontmost too (the coalescer already
            // drops what the user is looking at).
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            unsafe fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                completion.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            unsafe fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                completion: &block2::DynBlock<dyn Fn()>,
            ) {
                let id = response.notification().request().identifier().to_string();
                if let Some(rest) = id.strip_prefix(CHAT_PREFIX) {
                    if let Some((chat, _stamp)) = rest.rsplit_once(':') {
                        super::push_click(chat.to_string());
                    }
                } else if let Some(rest) = id.strip_prefix(CALL_PREFIX) {
                    let call_id = rest.split_once(':').map(|(c, _)| c).unwrap_or(rest);
                    let action = response.actionIdentifier().to_string();
                    if action == DECLINE {
                        super::push_click(format!("call:decline:{call_id}"));
                    } else {
                        // ACCEPT, or the default action (a click on the banner).
                        super::push_click(format!("call:accept:{call_id}"));
                    }
                }
                if let Some(mtm) = MainThreadMarker::new() {
                    NSApplication::sharedApplication(mtm).activate();
                }
                completion.call(());
            }
        }
    );

    impl Delegate {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(());
            // SAFETY: plain NSObject init on a freshly allocated instance.
            unsafe { msg_send![super(this), init] }
        }
    }

    thread_local! {
        // The center holds its delegate weakly; keep ours for the app's life.
        static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
    }

    /// 0 = unknown, 1 = bundled (center usable), 2 = not bundled.
    static BUNDLED: AtomicU8 = AtomicU8::new(0);

    pub fn available() -> bool {
        match BUNDLED.load(Ordering::Relaxed) {
            1 => true,
            2 => false,
            _ => {
                let bundled = NSBundle::mainBundle().bundleIdentifier().is_some();
                BUNDLED.store(if bundled { 1 } else { 2 }, Ordering::Relaxed);
                bundled
            }
        }
    }

    pub fn init() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        if !available() {
            return;
        }
        let delegate = Delegate::new(mtm);
        let center = UNUserNotificationCenter::currentNotificationCenter();
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
        // The ringing category: Accept (foreground) and Decline.
        let accept = UNNotificationAction::actionWithIdentifier_title_options(
            &NSString::from_str(ACCEPT),
            &NSString::from_str("Accept"),
            UNNotificationActionOptions::Foreground,
        );
        let decline = UNNotificationAction::actionWithIdentifier_title_options(
            &NSString::from_str(DECLINE),
            &NSString::from_str("Decline"),
            UNNotificationActionOptions::Destructive,
        );
        let category =
            UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                &NSString::from_str(CALL_CATEGORY),
                &NSArray::from_retained_slice(&[accept, decline]),
                &NSArray::new(),
                UNNotificationCategoryOptions::CustomDismissAction,
            );
        center.setNotificationCategories(&NSSet::from_retained_slice(&[category]));
    }

    pub fn deliver_call(
        call_id: &str,
        chat_id: &str,
        title: &str,
        body: &str,
        image: Option<&std::path::Path>,
    ) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        content.setThreadIdentifier(&NSString::from_str(chat_id));
        content.setCategoryIdentifier(&NSString::from_str(CALL_CATEGORY));
        if let Some(copy) = image.and_then(super::image_copy) {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&copy.to_string_lossy()));
            // SAFETY: no options dictionary; the URL is a readable local PNG.
            let attachment = unsafe {
                UNNotificationAttachment::attachmentWithIdentifier_URL_options_error(
                    &NSString::from_str("caller"),
                    &url,
                    None,
                )
            };
            if let Ok(a) = attachment {
                content.setAttachments(&NSArray::from_retained_slice(&[a]));
            }
        }
        let id = format!("{CALL_PREFIX}{call_id}:{chat_id}");
        WITHDRAW_IDS.with(|w| {
            w.borrow_mut()
                .entry(call_id.to_string())
                .or_default()
                .push(id.clone())
        });
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&id),
            &content,
            None,
        );
        let done = RcBlock::new(|error: *mut NSError| {
            if !error.is_null() {
                // SAFETY: non-null NSError* handed to the completion block.
                let error = unsafe { &*error };
                log::warn!(
                    "notify: call banner refused: {}",
                    error.localizedDescription()
                );
                refresh_permission();
            }
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, Some(&done));
    }

    pub fn withdraw_call(call_id: &str) {
        // The identifier carries the chat too, which we do not have here; remove
        // every delivered banner of this call by prefix match is not offered by
        // the API, so remove the known shape with an empty chat and let the
        // thread collapse handle the rest.
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let ids: Vec<Retained<NSString>> = WITHDRAW_IDS.with(|w| {
            w.borrow_mut()
                .remove(call_id)
                .unwrap_or_default()
                .iter()
                .map(|s| NSString::from_str(s))
                .collect()
        });
        if !ids.is_empty() {
            let arr = NSArray::from_retained_slice(&ids);
            center.removeDeliveredNotificationsWithIdentifiers(&arr);
            center.removePendingNotificationRequestsWithIdentifiers(&arr);
        }
    }

    thread_local! {
        /// Delivered call banner ids per call, so `withdraw_call` can remove them.
        static WITHDRAW_IDS: RefCell<std::collections::HashMap<String, Vec<String>>> =
            RefCell::new(std::collections::HashMap::new());
    }

    /// Every delivered message banner of `chat_id` (identifiers `chat:<id>:<stamp>`, which
    /// survive relaunches — the center's list is the source, not an in-memory map) is
    /// removed. The list arrives on a background queue; the removal is thread-safe.
    pub fn withdraw_chat(chat_id: &str) {
        let chat_id = chat_id.to_owned();
        let handler = RcBlock::new(move |list: NonNull<NSArray<UNNotification>>| {
            // SAFETY: the center hands a live array to the block.
            let list = unsafe { list.as_ref() };
            let ids: Vec<String> = list
                .iter()
                .map(|n| n.request().identifier().to_string())
                .collect();
            let doomed = super::withdrawn_ids(&chat_id, ids.iter().map(String::as_str));
            if doomed.is_empty() {
                return;
            }
            let arr: Vec<Retained<NSString>> =
                doomed.iter().map(|s| NSString::from_str(s)).collect();
            UNUserNotificationCenter::currentNotificationCenter()
                .removeDeliveredNotificationsWithIdentifiers(&NSArray::from_retained_slice(&arr));
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .getDeliveredNotificationsWithCompletionHandler(&handler);
    }

    pub fn bundle_id() -> Option<String> {
        NSBundle::mainBundle()
            .bundleIdentifier()
            .map(|s| s.to_string())
    }

    /// `getNotificationSettings`: the answer arrives on a background queue and
    /// is kept in the crate-level [`super::permission`].
    pub fn refresh_permission() {
        let handler = RcBlock::new(|settings: NonNull<UNNotificationSettings>| {
            // SAFETY: the center hands a live settings object to the block.
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            let p = match status {
                UNAuthorizationStatus::NotDetermined => super::Permission::NotDetermined,
                UNAuthorizationStatus::Denied => super::Permission::Denied,
                _ => super::Permission::Allowed,
            };
            super::set_permission(p);
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .getNotificationSettingsWithCompletionHandler(&handler);
    }

    pub fn request_authorization() {
        if !available() {
            return;
        }
        let handler = RcBlock::new(|granted: objc2::runtime::Bool, error: *mut NSError| {
            if !granted.as_bool() && !error.is_null() {
                // SAFETY: non-null NSError* handed to the completion block.
                let error = unsafe { &*error };
                log::warn!("notify: permission: {}", error.localizedDescription());
            }
            super::set_permission(if granted.as_bool() {
                super::Permission::Allowed
            } else {
                super::Permission::Denied
            });
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert
                    | UNAuthorizationOptions::Sound
                    | UNAuthorizationOptions::Badge,
                &handler,
            );
    }

    pub fn deliver(n: &Notification) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&n.title));
        content.setBody(&NSString::from_str(&n.body));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        if let Some(chat) = &n.chat_id {
            content.setThreadIdentifier(&NSString::from_str(chat));
        }
        if let Some(copy) = n.image.as_deref().and_then(super::image_copy) {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&copy.to_string_lossy()));
            // SAFETY: no options dictionary; the URL is a readable local PNG.
            let attachment = unsafe {
                UNNotificationAttachment::attachmentWithIdentifier_URL_options_error(
                    &NSString::from_str("sender"),
                    &url,
                    None,
                )
            };
            match attachment {
                Ok(a) => content.setAttachments(&NSArray::from_retained_slice(&[a])),
                Err(e) => eprintln!("notify: attachment: {}", e.localizedDescription()),
            }
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let id = match &n.chat_id {
            Some(chat) => format!("{CHAT_PREFIX}{chat}:{stamp}"),
            None => format!("event:{}:{stamp}", n.project_id),
        };
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&id),
            &content,
            None,
        );
        let done = RcBlock::new(|error: *mut NSError| {
            if !error.is_null() {
                // SAFETY: non-null NSError* handed to the completion block.
                let error = unsafe { &*error };
                // "Notifications are not allowed for this application" = the
                // permission went missing; re-read it so Settings shows why.
                log::warn!("notify: banner refused: {}", error.localizedDescription());
                refresh_permission();
            }
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, Some(&done));
    }

    pub fn set_badge(total: usize) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let label = (total > 0).then(|| NSString::from_str(&total.to_string()));
        NSApplication::sharedApplication(mtm)
            .dockTile()
            .setBadgeLabel(label.as_deref());
    }
}

/// Coalescer + backend + the user's on/off switch.
pub struct Notifier {
    coalescer: Coalescer,
    pub enabled: bool,
    /// `ARBOS_NOTIFY_ALWAYS=1`: ignore the focus gate (demos, screen clips).
    always: bool,
}

impl Default for Notifier {
    fn default() -> Self {
        Self::new()
    }
}

impl Notifier {
    pub fn new() -> Self {
        Self {
            coalescer: Coalescer::default(),
            enabled: true,
            always: std::env::var_os("ARBOS_NOTIFY_ALWAYS").is_some(),
        }
    }

    fn gate(&self, gate: Gate) -> Gate {
        if self.always {
            Gate {
                streaming: gate.streaming,
                visible: false,
            }
        } else {
            gate
        }
    }

    pub fn offer(&mut self, event: NotifyEvent, now_ms: u64, gate: Gate) {
        if !self.enabled {
            return;
        }
        let gate = self.gate(gate);
        if let Some(n) = self.coalescer.offer(event, now_ms, gate) {
            deliver(&n);
        }
    }

    pub fn tick(&mut self, now_ms: u64, mut gate_of: impl FnMut(&str) -> Gate) {
        if !self.enabled {
            return;
        }
        let always = self.always;
        for n in self.coalescer.tick(now_ms, |p| {
            let g = gate_of(p);
            if always {
                Gate {
                    streaming: g.streaming,
                    visible: false,
                }
            } else {
                g
            }
        }) {
            deliver(&n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: NotifyKind, detail: &str) -> NotifyEvent {
        NotifyEvent::new("comms", "Comms", kind, detail)
    }

    const OFF: Gate = Gate {
        streaming: false,
        visible: false,
    };

    #[test]
    fn at_most_one_per_project_per_window_and_coalesces() {
        let mut c = Coalescer::default();
        let mut fired: Vec<(u64, Notification)> = Vec::new();

        // One event per second for two minutes, plus ticks.
        for s in 0..=120u64 {
            let now = s * 1_000;
            if let Some(n) = c.offer(ev(NotifyKind::TurnFinished, &format!("turn {s}")), now, OFF) {
                fired.push((now, n));
            }
            for n in c.tick(now, |_| OFF) {
                fired.push((now, n));
            }
        }

        for pair in fired.windows(2) {
            assert!(
                pair[1].0 - pair[0].0 >= WINDOW_MS,
                "two notifications inside one window: {:?}",
                pair.iter().map(|(t, _)| t).collect::<Vec<_>>()
            );
        }
        assert_eq!(fired.len(), 3, "0 s, 60 s and 120 s");
        assert_eq!(fired[0].1.count, 1);
        assert_eq!(fired[0].1.title, "Comms · done");
        assert_eq!(fired[1].1.count, 60, "events 1..=60 fold into one");
        assert_eq!(fired[1].1.title, "60 updates in Comms");
        assert_eq!(fired[1].1.body, "turn 60");
    }

    #[test]
    fn three_updates_wording() {
        let mut c = Coalescer::default();
        assert!(c.offer(ev(NotifyKind::TurnFinished, "a"), 0, OFF).is_some());
        for (i, d) in ["b", "c", "d"].iter().enumerate() {
            assert!(c
                .offer(ev(NotifyKind::NeedsInput, d), 1_000 * (i as u64 + 1), OFF)
                .is_none());
        }
        assert_eq!(c.pending_count("comms"), 3);
        let out = c.tick(WINDOW_MS, |_| OFF);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "3 updates in Comms");
        assert_eq!(out[0].body, "d");
    }

    #[test]
    fn nothing_while_streaming_then_fires_when_done() {
        let mut c = Coalescer::default();
        let streaming = Gate {
            streaming: true,
            visible: false,
        };
        assert!(c
            .offer(ev(NotifyKind::NeedsInput, "q"), 0, streaming)
            .is_none());
        assert!(c.tick(5_000, |_| streaming).is_empty());
        let out = c.tick(6_000, |_| OFF);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Comms · needs your input");
    }

    #[test]
    fn visible_drops_instead_of_queueing() {
        let mut c = Coalescer::default();
        let visible = Gate {
            streaming: false,
            visible: true,
        };
        assert!(c
            .offer(ev(NotifyKind::TurnFinished, "x"), 0, visible)
            .is_none());
        assert_eq!(c.pending_count("comms"), 0);
        assert!(c.tick(WINDOW_MS, |_| OFF).is_empty());
    }

    #[test]
    fn projects_are_independent() {
        let mut c = Coalescer::default();
        let other = NotifyEvent::new("arbos", "Arbos", NotifyKind::RoutineRan, "Morning digest");
        assert!(c.offer(ev(NotifyKind::TurnFinished, "a"), 0, OFF).is_some());
        assert!(c.offer(other, 1, OFF).is_some());
    }

    /// Clear-on-read withdraws exactly the chat's message banners: not another chat's that
    /// shares a prefix, not a call banner, not a project event.
    #[test]
    fn withdraw_picks_only_the_chats_message_banners() {
        let delivered = [
            "chat:01ABC:1700000000000",
            "chat:01ABC:1700000005000",
            "chat:01ABCD:1700000000000",
            "chat:01XYZ:1700000000000",
            "call:c1:01ABC",
            "event:p1:1700000000000",
        ];
        assert_eq!(
            withdrawn_ids("01ABC", delivered),
            vec!["chat:01ABC:1700000000000", "chat:01ABC:1700000005000"]
        );
        assert!(withdrawn_ids("", delivered).is_empty());
        assert!(withdrawn_ids("01NOPE", delivered).is_empty());
    }

    #[test]
    fn message_kinds_are_titled_with_the_sender() {
        let mut c = Coalescer::default();
        let mut e = ev(NotifyKind::TurnFinished, "On it.");
        e.sender = Some("Comms".into());
        e.chat_id = Some("c1".into());
        let n = c.offer(e, 0, OFF).unwrap();
        assert_eq!(n.title, "Comms");
        assert_eq!(n.body, "On it.");
        assert_eq!(n.chat_id.as_deref(), Some("c1"));

        let mut c = Coalescer::default();
        let mut e = ev(NotifyKind::DirectMessage, "hey");
        e.sender = Some("Sam".into());
        assert_eq!(c.offer(e, 0, OFF).unwrap().title, "Sam");

        // Non-message kinds keep the event wording even with a sender.
        let mut c = Coalescer::default();
        let mut e = ev(NotifyKind::NeedsInput, "Which branch?");
        e.sender = Some("Comms".into());
        assert_eq!(
            c.offer(e, 0, OFF).unwrap().title,
            "Comms · needs your input"
        );
    }
}
