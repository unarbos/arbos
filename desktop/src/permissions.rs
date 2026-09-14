//! What the system lets this app do, and how to ask: the microphone for
//! dictation and calls, the screen for the screenshot and recording tools,
//! accessibility for the driver, notifications, and the folder a project
//! lives in. One row each in Settings › Permissions, and a sheet on first
//! launch.
//!
//! On macOS every status is read from the system (TCC) and every request
//! is the real prompt, raised from inside this process so the grant is
//! filed under this bundle. Where macOS will not prompt — a second ask, or
//! an ad-hoc bundle, where `CGRequestScreenCaptureAccess` returns false
//! with no dialog — the request says so and the row offers the System
//! Settings pane; `model::permission_center` runs the rows, off the UI
//! thread, and keeps re-reading. On Linux the rows that apply
//! are the microphone (a capture program and a device), the screen (the
//! portal asks on first use under Wayland; X11 has no gate) and the
//! folder; the rest are hidden.

use std::path::Path;

/// Something the app may need the system's leave for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    Microphone,
    ScreenRecording,
    Accessibility,
    Notifications,
    /// Reading and writing the folder the active project lives in.
    ProjectFolder,
}

/// Where a permission stands right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Granted,
    /// The system has not been asked yet; a request will prompt.
    NotAsked,
    /// Asked and refused, or restricted by policy. The system will not
    /// prompt again; the pane is the way.
    Denied,
    /// Cannot be known or requested from here; `.0` says why.
    Unavailable(String),
}

/// What pressing Request did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requested {
    /// The system's own prompt is up (or the grant came back at once);
    /// re-read the status in a moment.
    Prompted,
    /// The system would not prompt; the pane at this URL is where the
    /// switch is. Already opened.
    OpenedSettings,
    /// Nothing to press on this platform.
    Unsupported,
}

impl Permission {
    /// The rows that mean something on this platform, in the order shown.
    pub fn applicable() -> &'static [Permission] {
        if cfg!(target_os = "macos") {
            &[
                Self::Microphone,
                Self::ScreenRecording,
                Self::Accessibility,
                Self::Notifications,
                Self::ProjectFolder,
            ]
        } else {
            &[Self::Microphone, Self::ScreenRecording, Self::ProjectFolder]
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Microphone => "Microphone",
            Self::ScreenRecording => "Screen Recording",
            Self::Accessibility => "Accessibility",
            Self::Notifications => "Notifications",
            Self::ProjectFolder => "Files and Folders",
        }
    }

    /// What it is for, in one line.
    pub fn purpose(self) -> &'static str {
        match self {
            Self::Microphone => "Dictation and voice calls with the agent.",
            Self::ScreenRecording => "The screenshot and screen-recording tools.",
            Self::Accessibility => "The driver: clicking and typing in other apps.",
            Self::Notifications => "A note when an agent finishes or asks.",
            Self::ProjectFolder => "Reading and writing the folder of the open project.",
        }
    }

    /// The System Settings pane that holds the switch, macOS only.
    pub fn settings_url(self) -> Option<&'static str> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        Some(match self {
            Self::Microphone => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
            }
            Self::ScreenRecording => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
            }
            Self::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            Self::Notifications => {
                "x-apple.systempreferences:com.apple.Notifications-Settings.extension"
            }
            Self::ProjectFolder => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders"
            }
        })
    }

    /// Where it stands. `project` is the open project's folder, for the
    /// folder row; the rest ignore it.
    pub fn status(self, project: Option<&Path>) -> Status {
        match self {
            Self::ProjectFolder => folder_status(project),
            Self::Microphone => platform::microphone_status(),
            Self::ScreenRecording => platform::screen_status(),
            Self::Accessibility => platform::accessibility_status(),
            Self::Notifications => platform::notifications_status(),
        }
    }

    /// Ask. The real prompt where the system still offers one; the pane
    /// where it does not (`open` is handed the URL to open).
    pub fn request(self, project: Option<&Path>, open: &mut dyn FnMut(&str)) -> Requested {
        let status = self.status(project);
        let requested = match self {
            Self::ProjectFolder => {
                // Touching the folder is the request: macOS raises its own
                // prompt for a protected folder on the first read.
                match folder_status(project) {
                    Status::Granted => Requested::Prompted,
                    _ => Requested::Unsupported,
                }
            }
            Self::Microphone => platform::request_microphone(),
            Self::ScreenRecording => platform::request_screen(),
            Self::Accessibility => platform::request_accessibility(),
            Self::Notifications => platform::request_notifications(),
        };
        // Denied means the system will not ask again; the pane is the way.
        if status == Status::Denied || requested == Requested::Unsupported {
            if let Some(url) = self.settings_url() {
                open(url);
                return Requested::OpenedSettings;
            }
        }
        requested
    }
}

/// A real screen capture attempt: on Sequoia an app appears in the Screen
/// Recording list only after it has tried, so this is what puts Arbos
/// there. True when an image came back (the grant is in).
pub fn try_screen_capture() -> bool {
    platform::try_screen_capture()
}

/// Reading the folder is the test: a list comes back, or the system says no.
fn folder_status(project: Option<&Path>) -> Status {
    let Some(project) = project else {
        return Status::Unavailable("No project open.".into());
    };
    match std::fs::read_dir(project) {
        Ok(_) => Status::Granted,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Status::Denied,
        Err(e) => Status::Unavailable(e.to_string()),
    }
}

#[cfg(target_os = "macos")]
mod platform {
    //! TCC, through the frameworks that own each switch. Every call here is
    //! one the system documents as safe off the main thread except the
    //! prompts, which we raise from the UI thread that calls `request`.

    use super::{Requested, Status};
    use crate::voice_ws::{self, MicPermission};
    use block::ConcreteBlock;
    use objc::{
        class, msg_send,
        runtime::{BOOL, Object, YES},
        sel, sel_impl,
    };
    use std::{
        ffi::c_void,
        sync::{
            Mutex, mpsc,
            atomic::{AtomicI64, Ordering},
        },
        time::Duration,
    };

    #[link(name = "UserNotifications", kind = "framework")]
    unsafe extern "C" {}

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    #[link(name = "ScreenCaptureKit", kind = "framework")]
    unsafe extern "C" {}

    /// How long the shareable-content call gets to answer. It returns at
    /// once when the grant is in or refused; a prompt keeps it open until
    /// the user answers, and the row is polling by then anyway.
    const CAPTURE_WAIT: Duration = Duration::from_secs(3);

    /// A real capture attempt through ScreenCaptureKit:
    /// `SCShareableContent.getShareableContentWithCompletionHandler:` is
    /// what puts an app into Screen & System Audio Recording on macOS 15
    /// and later and raises the dialog; `CGWindowListCreateImage` does
    /// neither there (Mac worker, macOS 26). True when content came back
    /// with no error, which is the grant.
    pub fn try_screen_capture() -> bool {
        let (tx, rx) = mpsc::channel::<bool>();
        unsafe {
            let handler = ConcreteBlock::new(move |content: *mut Object, error: *mut Object| {
                let _ = tx.send(!content.is_null() && error.is_null());
            })
            .copy();
            let () = msg_send![
                class!(SCShareableContent),
                getShareableContentWithCompletionHandler: &*handler
            ];
        }
        match rx.recv_timeout(CAPTURE_WAIT) {
            Ok(granted) => granted,
            Err(_) => unsafe { CGPreflightScreenCaptureAccess() },
        }
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
        static kAXTrustedCheckOptionPrompt: *const c_void;
    }

    /// The microphone is `voice_ws`'s: the same `AVCaptureDevice` read and
    /// request the capture path uses, so this row and the first take lead to
    /// the one dialog.
    pub fn microphone_status() -> Status {
        match voice_ws::mic_permission() {
            MicPermission::Authorized | MicPermission::NotApplicable => Status::Granted,
            MicPermission::NotDetermined => Status::NotAsked,
            MicPermission::Denied | MicPermission::Restricted => Status::Denied,
        }
    }

    pub fn request_microphone() -> Requested {
        voice_ws::request_mic_permission();
        Requested::Prompted
    }

    /// The preflight says granted or not; it cannot tell "never asked"
    /// from "refused", so a refusal shows as not asked and the request
    /// falls through to the pane when the system stays quiet.
    pub fn screen_status() -> Status {
        if unsafe { CGPreflightScreenCaptureAccess() } {
            Status::Granted
        } else {
            Status::NotAsked
        }
    }

    pub fn request_screen() -> Requested {
        if unsafe { CGRequestScreenCaptureAccess() } {
            Requested::Prompted
        } else {
            // Already asked once: macOS shows nothing more from here.
            Requested::Unsupported
        }
    }

    pub fn accessibility_status() -> Status {
        if unsafe { AXIsProcessTrusted() } {
            Status::Granted
        } else {
            Status::NotAsked
        }
    }

    pub fn request_accessibility() -> Requested {
        unsafe {
            let yes: *mut Object = msg_send![class!(NSNumber), numberWithBool: YES];
            let key = kAXTrustedCheckOptionPrompt;
            let options: *mut Object =
                msg_send![class!(NSDictionary), dictionaryWithObject: yes forKey: key];
            AXIsProcessTrustedWithOptions(options as *const c_void);
        }
        Requested::Prompted
    }

    /// The last authorization the notification centre reported: -1 never
    /// read, else `UNAuthorizationStatus` (0 not determined, 1 denied,
    /// 2 authorized, 3 provisional, 4 ephemeral). The centre answers on
    /// its own thread; the row polls this.
    static NOTIFICATION_STATUS: AtomicI64 = AtomicI64::new(-1);
    /// The error the last `requestAuthorization` came back with. The
    /// centre refuses an ad-hoc bundle outside /Applications outright — no
    /// dialog, "denied" within a moment — and that is not the user's no.
    static NOTIFICATION_ERROR: Mutex<Option<String>> = Mutex::new(None);

    fn notification_error() -> Option<String> {
        NOTIFICATION_ERROR
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The app is somewhere macOS registers notifications for; an ad-hoc
    /// build launched from a build folder is not.
    fn in_applications() -> bool {
        std::env::current_exe()
            .ok()
            .is_some_and(|exe| exe.starts_with("/Applications") || exe.starts_with("/System/Applications"))
    }

    fn describe(error: *mut Object) -> String {
        unsafe {
            let text: *mut Object = msg_send![error, localizedDescription];
            if text.is_null() {
                return "the notification centre refused the request".into();
            }
            let utf8: *const std::os::raw::c_char = msg_send![text, UTF8String];
            if utf8.is_null() {
                return "the notification centre refused the request".into();
            }
            std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
        }
    }

    /// A bare binary has no bundle proxy and the notification centre
    /// aborts on it; only a bundle may ask.
    fn bundled() -> bool {
        unsafe {
            let bundle: *mut Object = msg_send![class!(NSBundle), mainBundle];
            let id: *mut Object = msg_send![bundle, bundleIdentifier];
            !id.is_null()
        }
    }

    pub fn notifications_status() -> Status {
        if !bundled() {
            return Status::Unavailable("Needs the app bundle (Arbos.app), not a bare binary.".into());
        }
        unsafe {
            let center: *mut Object =
                msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
            let handler = ConcreteBlock::new(move |settings: *mut Object| {
                let status: i64 = msg_send![settings, authorizationStatus];
                NOTIFICATION_STATUS.store(status, Ordering::SeqCst);
            })
            .copy();
            let () = msg_send![center, getNotificationSettingsWithCompletionHandler: &*handler];
        }
        match NOTIFICATION_STATUS.load(Ordering::SeqCst) {
            2 | 3 | 4 => Status::Granted,
            1 => match notification_error() {
                // Refused by the system, not by the user: nothing to flip
                // in a pane. Say what to do instead.
                Some(why) if !in_applications() => Status::Unavailable(format!(
                    "macOS registers notifications only for an app in /Applications; move Arbos.app there. ({why})"
                )),
                Some(why) => Status::Unavailable(why),
                None => Status::Denied,
            },
            0 => Status::NotAsked,
            _ => Status::NotAsked,
        }
    }

    pub fn request_notifications() -> Requested {
        if !bundled() {
            return Requested::Unsupported;
        }
        unsafe {
            let center: *mut Object =
                msg_send![class!(UNUserNotificationCenter), currentNotificationCenter];
            // badge | sound | alert
            let options: u64 = 1 | 2 | 4;
            let handler = ConcreteBlock::new(move |_granted: BOOL, error: *mut Object| {
                let why = (!error.is_null()).then(|| describe(error));
                *NOTIFICATION_ERROR.lock().unwrap_or_else(|p| p.into_inner()) = why;
            })
            .copy();
            let () = msg_send![
                center,
                requestAuthorizationWithOptions: options
                completionHandler: &*handler
            ];
        }
        Requested::Prompted
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    //! Linux: no TCC. The microphone row reads whether a capture program
    //! and a device are there; the screen row says how the desktop gates
    //! capture; the rest do not apply and are not listed.

    use super::{Requested, Status};

    /// No gate to try against: the portal asks on first use under Wayland,
    /// X11 has none. The preflight's answer stands.
    pub fn try_screen_capture() -> bool {
        matches!(screen_status(), Status::Granted)
    }

    pub fn microphone_status() -> Status {
        match crate::voice_ws::mic_program() {
            Ok(program) => {
                if std::path::Path::new("/dev/snd").read_dir().is_ok_and(|mut d| d.next().is_some())
                    || std::env::var_os("PULSE_SERVER").is_some()
                    || std::env::var_os("PIPEWIRE_RUNTIME_DIR").is_some()
                {
                    Status::Granted
                } else {
                    Status::Unavailable(format!("{program} is installed, but no capture device is present."))
                }
            }
            Err(e) => Status::Unavailable(e.to_string()),
        }
    }

    pub fn request_microphone() -> Requested {
        Requested::Unsupported
    }

    pub fn screen_status() -> Status {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            Status::NotAsked
        } else {
            Status::Granted
        }
    }

    pub fn request_screen() -> Requested {
        // The xdg-desktop-portal asks on the first capture; nothing to
        // press ahead of it.
        Requested::Unsupported
    }

    pub fn accessibility_status() -> Status {
        Status::Unavailable("Not a Linux permission.".into())
    }

    pub fn request_accessibility() -> Requested {
        Requested::Unsupported
    }

    pub fn notifications_status() -> Status {
        Status::Unavailable("Not a Linux permission.".into())
    }

    pub fn request_notifications() -> Requested {
        Requested::Unsupported
    }
}
