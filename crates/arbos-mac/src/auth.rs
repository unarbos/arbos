//! "Sign in with GitHub": the browser leg of the relay's OAuth flow.
//!
//! The relay owns GitHub (`/auth/github/start` → GitHub → `/auth/github/callback`)
//! and hands back a one-time grant code on our redirect URI. This module only
//! gets that code to the app:
//!
//! - macOS: `ASWebAuthenticationSession` with the `arbos://auth/callback`
//!   scheme (registered in Info.plist), anchored to the app's own window (the
//!   GPUI window's `NSWindow`, never a throwaway one — a sheet attached to an
//!   invisible window never shows, Tobias 2026-10-23). With no window at all
//!   the default browser gets the URL and the callback arrives through
//!   LaunchServices (`on_open_urls` → `AppState::sign_in_callback`).
//! - elsewhere (Linux CI): a loopback listener on `http://127.0.0.1:<port>/cb`
//!   and the system browser (`ARBOS_BROWSER` or `xdg-open`).
//!
//! The grant code is exchanged by the relay source (`POST /auth/token`). The
//! pill's copy, Cancel / Retry and the 60-s soft timeout live in `sign_in.rs`.

use std::time::Duration;

use anyhow::{anyhow, Context as _, Result};
use futures::FutureExt as _;
use gpui::App;
use gpui_tokio::Tokio;

/// Hard stop for the sheet / loopback leg; the pill offers Retry long before.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// What the browser leg produced.
#[derive(Debug)]
pub enum SignIn {
    /// `?code=` from the redirect: exchange it at `POST /auth/token`.
    Grant(String),
    /// The relay runs `ARBOS_AUTH_MODE=paircode` (409 `auth_mode`): show the
    /// pairing-code entry instead.
    PairCodeMode,
    /// The user closed the sheet (its Cancel, or `cancel()` from the pill).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Cancelled,
}

/// Progress of one `begin` call, delivered on the main thread.
#[derive(Debug)]
pub enum Event {
    /// The GitHub page is on screen: in the system sheet (`in_browser: false`)
    /// or in the default browser (`true`, no window to attach a sheet to).
    Opened { in_browser: bool },
    /// The leg ended. Not sent for the browser fallback on macOS — that
    /// callback comes through `arbos://auth/callback` (LaunchServices).
    Finished(Result<SignIn>),
}

/// Where the system sheet attaches. Pure so it is testable on Linux.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor<W> {
    /// Attach the sheet to this window.
    Window(W),
    /// No window at all: open the default browser and wait for the callback.
    Browser,
}

/// Pick the presentation anchor. The app's own window first (it exists even
/// when AppKit says there is no key or main window — e.g. the user switched
/// to another app while the relay was being probed), then AppKit's key /
/// main window, then any visible window of ours. Never a fresh `NSWindow`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn choose_anchor<W>(
    app_window: Option<W>,
    key: Option<W>,
    main: Option<W>,
    visible: Option<W>,
) -> Anchor<W> {
    app_window
        .or(key)
        .or(main)
        .or(visible)
        .map(Anchor::Window)
        .unwrap_or(Anchor::Browser)
}

/// `GET /auth/github/start?…` for this relay and device, redirecting to
/// `arbos://auth/callback` (the browser fallback and "Open in browser").
pub fn start_url(relay_http: &str, device_name: &str) -> String {
    start_url_to(relay_http, device_name, &platform::redirect_uri())
}

fn start_url_to(relay_http: &str, device_name: &str, redirect_uri: &str) -> String {
    format!(
        "{}/auth/github/start?redirect={}&device_name={}",
        relay_http.trim_end_matches('/'),
        url_encode(redirect_uri),
        url_encode(device_name)
    )
}

/// Start the browser leg; `on` runs on the main thread for every `Event`.
pub fn begin(
    relay_http: String,
    device_name: String,
    cx: &mut App,
    mut on: impl FnMut(Event, &mut App) + 'static,
) {
    let redirect = platform::redirect();
    let start_url = start_url_to(&relay_http, &device_name, &redirect.uri);
    let probe = Tokio::spawn(cx, probe_auth_mode(start_url.clone()));
    cx.spawn(async move |cx| {
        let probe = match probe.await {
            Ok(Ok(probe)) => probe,
            Ok(Err(err)) => {
                cx.update(|cx| on(Event::Finished(Err(err)), cx));
                return;
            }
            Err(err) => {
                cx.update(|cx| on(Event::Finished(Err(anyhow!("sign-in task: {err}"))), cx));
                return;
            }
        };
        if matches!(probe, Probe::PairCode) {
            cx.update(|cx| on(Event::Finished(Ok(SignIn::PairCodeMode)), cx));
            return;
        }
        let on = std::rc::Rc::new(std::cell::RefCell::new(on));
        let opened = {
            let on = on.clone();
            move |in_browser: bool, cx: &mut App| {
                (on.borrow_mut())(Event::Opened { in_browser }, cx);
            }
        };
        if let Some(outcome) = platform::run(start_url, redirect, opened, cx).await {
            cx.update(|cx| (on.borrow_mut())(Event::Finished(outcome), cx));
        }
    })
    .detach();
}

/// Close the system sheet of the attempt in flight (macOS). Its completion
/// block then reports `SignIn::Cancelled`. No-op elsewhere.
pub fn cancel() {
    platform::cancel();
}

/// Hand the GitHub page to the default browser (the no-window fallback and
/// the pill's "Open in browser"). The relay's redirect to
/// `arbos://auth/callback` comes back through LaunchServices on macOS.
pub fn open_in_browser(start_url: &str) -> Result<()> {
    platform::open_browser(start_url)
}

enum Probe {
    Browser,
    PairCode,
}

/// One request without following redirects: 302 means GitHub is wired up,
/// 409 `auth_mode` means the relay wants a pairing code.
async fn probe_auth_mode(start_url: String) -> Result<Probe> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .context("http client")?;
    let resp = client
        .get(&start_url)
        .send()
        .await
        .context("reach the relay")?;
    let status = resp.status();
    if status.is_redirection() {
        return Ok(Probe::Browser);
    }
    let body = resp.text().await.unwrap_or_default();
    if status.as_u16() == 409 || status.as_u16() == 410 {
        return Ok(Probe::PairCode);
    }
    let message = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(|m| m.as_str())
                .map(str::to_owned)
        })
        .unwrap_or(body);
    Err(anyhow!("relay answered {status}: {message}"))
}

/// Is this the relay's answer on our redirect URI (`arbos://auth/callback…`)?
pub fn is_callback_url(url: &str) -> bool {
    url.starts_with("arbos://auth/callback")
}

/// `code=` / `error=` from a callback URL of either shape.
pub fn parse_callback(url: &str) -> Result<String> {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut code = None;
    let mut error = None;
    let mut message = None;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = url_decode(v);
        match k {
            "code" => code = Some(v),
            "error" => error = Some(v),
            "message" => message = Some(v),
            _ => {}
        }
    }
    if let Some(error) = error {
        let message = message.unwrap_or_default();
        return Err(anyhow!(
            "{}",
            match error.as_str() {
                "oauth_denied" => "GitHub sign-in was cancelled.".to_string(),
                "invite_required" =>
                    "This relay needs an invite. Ask for one and try again.".to_string(),
                _ if !message.is_empty() => message,
                other => format!("sign-in failed ({other})"),
            }
        ));
    }
    code.filter(|c| !c.is_empty())
        .ok_or_else(|| anyhow!("the callback carried no code"))
}

pub(crate) fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = &s[i + 1..i + 3];
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub struct Redirect {
    pub uri: String,
    #[cfg(not(target_os = "macos"))]
    listener: Option<std::net::TcpListener>,
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use gpui::AsyncApp;

    pub fn redirect_uri() -> String {
        "arbos://auth/callback".into()
    }

    pub fn redirect() -> Redirect {
        Redirect {
            uri: redirect_uri(),
        }
    }

    /// The sheet leg. `Some(outcome)` when the sheet ended (or the hard
    /// timeout fired); `None` when the page went to the browser — that
    /// callback arrives through LaunchServices, not here.
    pub async fn run(
        start_url: String,
        _redirect: Redirect,
        opened: impl FnOnce(bool, &mut App),
        cx: &mut AsyncApp,
    ) -> Option<Result<SignIn>> {
        let (tx, rx) = futures::channel::oneshot::channel::<Result<SignIn>>();
        let started = cx.update(|cx| {
            let view = app_window_view(cx);
            session::start(&start_url, view, tx)
        });
        match started {
            session::Started::Sheet => cx.update(|cx| opened(false, cx)),
            session::Started::NoWindow => {
                if let Err(err) = open_browser(&start_url) {
                    return Some(Err(err));
                }
                cx.update(|cx| opened(true, cx));
                return None;
            }
            session::Started::Failed(err) => return Some(Err(err)),
        }
        let outcome = futures::select! {
            r = rx.fuse() => r.unwrap_or_else(|_| Err(anyhow!("sign-in window closed"))),
            _ = cx.background_executor().timer(SIGN_IN_TIMEOUT).fuse() => {
                session::cancel();
                Err(anyhow!("GitHub did not answer in 5 minutes. Try again."))
            }
        };
        Some(outcome)
    }

    pub fn cancel() {
        session::cancel();
    }

    pub fn open_browser(url: &str) -> Result<()> {
        std::process::Command::new("/usr/bin/open")
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("open the browser")?;
        Ok(())
    }

    /// The `NSView` behind the first GPUI window (`raw-window-handle`): the
    /// anchor's window is `view.window`, whatever AppKit thinks is key.
    fn app_window_view(cx: &mut App) -> Option<std::ptr::NonNull<std::ffi::c_void>> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        cx.windows().into_iter().find_map(|handle| {
            handle
                .update(cx, |_, window, _| {
                    match HasWindowHandle::window_handle(&*window) {
                        Ok(h) => match h.as_raw() {
                            RawWindowHandle::AppKit(appkit) => Some(appkit.ns_view),
                            _ => None,
                        },
                        Err(_) => None,
                    }
                })
                .ok()
                .flatten()
        })
    }

    /// `ASWebAuthenticationSession` + the presentation anchor it insists on.
    mod session {
        use std::cell::RefCell;
        use std::ffi::c_void;
        use std::ptr::NonNull;
        use std::rc::Rc;

        use anyhow::{anyhow, Result};
        use block2::RcBlock;
        use objc2::rc::Retained;
        use objc2::runtime::ProtocolObject;
        use objc2::{
            define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
        };
        use objc2_app_kit::{NSApplication, NSView, NSWindow};
        use objc2_authentication_services::{
            ASPresentationAnchor, ASWebAuthenticationPresentationContextProviding,
            ASWebAuthenticationSession,
        };
        use objc2_foundation::{NSError, NSObject, NSObjectProtocol, NSString, NSURL};

        use crate::auth::{choose_anchor, Anchor as Choice, SignIn};

        pub enum Started {
            /// The sheet is up, anchored to one of our windows.
            Sheet,
            /// Not one window to attach to: the caller opens the browser.
            NoWindow,
            Failed(anyhow::Error),
        }

        define_class!(
            // SAFETY: NSObject has no subclassing requirements and the type
            // does not implement Drop.
            #[unsafe(super(NSObject))]
            #[thread_kind = MainThreadOnly]
            #[name = "ArbosAuthAnchor"]
            #[ivars = Retained<NSWindow>]
            struct Anchor;

            unsafe impl NSObjectProtocol for Anchor {}

            unsafe impl ASWebAuthenticationPresentationContextProviding for Anchor {
                #[unsafe(method_id(presentationAnchorForWebAuthenticationSession:))]
                unsafe fn anchor(
                    &self,
                    _session: &ASWebAuthenticationSession,
                ) -> Retained<ASPresentationAnchor> {
                    let window = self.ivars().clone();
                    // ASPresentationAnchor is NSWindow on macOS; the binding
                    // spells it NSObject, so walk up the class chain.
                    Retained::into_super(Retained::into_super(window))
                }
            }
        );

        impl Anchor {
            fn new(mtm: MainThreadMarker, window: Retained<NSWindow>) -> Retained<Self> {
                let this = Self::alloc(mtm).set_ivars(window);
                // SAFETY: plain NSObject init on a freshly allocated instance.
                unsafe { msg_send![super(this), init] }
            }
        }

        thread_local! {
            // Keep the session and its anchor alive until the block fires.
            static LIVE: RefCell<Option<(Retained<ASWebAuthenticationSession>, Retained<Anchor>)>> =
                const { RefCell::new(None) };
        }

        /// The window the sheet attaches to; `None` means "no window of ours
        /// is on screen" and the caller falls back to the browser.
        fn anchor_window(
            mtm: MainThreadMarker,
            view: Option<NonNull<c_void>>,
        ) -> Option<Retained<NSWindow>> {
            let app = NSApplication::sharedApplication(mtm);
            // SAFETY: the pointer comes from GPUI's raw window handle for a
            // live window; `raw-window-handle` documents it as an NSView*.
            let app_window = view.and_then(|v| unsafe { v.cast::<NSView>().as_ref().window() });
            let visible = app.windows().to_vec().into_iter().find(|w| w.isVisible());
            match choose_anchor(app_window, app.keyWindow(), app.mainWindow(), visible) {
                Choice::Window(w) => Some(w),
                Choice::Browser => None,
            }
        }

        pub fn start(
            start_url: &str,
            view: Option<NonNull<c_void>>,
            tx: futures::channel::oneshot::Sender<Result<SignIn>>,
        ) -> Started {
            let Some(mtm) = MainThreadMarker::new() else {
                return Started::Failed(anyhow!("sign-in must start on the main thread"));
            };
            // A sheet from an earlier attempt (Retry after the soft timeout)
            // goes away first; its block reports Cancelled to a stale attempt.
            cancel();
            let Some(window) = anchor_window(mtm, view) else {
                return Started::NoWindow;
            };
            // The consent alert and the sheet are keyed to the active app:
            // bring ours forward in case the user wandered off during the probe.
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
            window.makeKeyAndOrderFront(None);
            let tx = Rc::new(RefCell::new(Some(tx)));
            let block_tx = tx.clone();
            let handler = RcBlock::new(move |url: *mut NSURL, error: *mut NSError| {
                let result = if !url.is_null() {
                    // SAFETY: non-null NSURL* handed to the completion block.
                    let url = unsafe { &*url };
                    url.absoluteString()
                        .map(|s| s.to_string())
                        .ok_or_else(|| anyhow!("callback URL was empty"))
                        .and_then(|url| crate::auth::parse_callback(&url))
                        .map(SignIn::Grant)
                } else if !error.is_null() {
                    // SAFETY: non-null NSError* handed to the completion block.
                    let error = unsafe { &*error };
                    // ASWebAuthenticationSessionErrorCodeCanceledLogin = 1.
                    if error.code() == 1 {
                        Ok(SignIn::Cancelled)
                    } else {
                        Err(anyhow!("{}", error.localizedDescription()))
                    }
                } else {
                    Err(anyhow!("sign-in ended without a result"))
                };
                if let Some(tx) = block_tx.borrow_mut().take() {
                    let _ = tx.send(result);
                }
                LIVE.with(|live| live.borrow_mut().take());
            });
            let Some(url) = NSURL::URLWithString(&NSString::from_str(start_url)) else {
                return Started::Failed(anyhow!("the sign-in URL is not valid"));
            };
            let anchor = Anchor::new(mtm, window);
            // SAFETY: plain init with a valid URL, scheme and handler block.
            // The scheme-based initializer is deprecated in favour of
            // `initWithURL:callback:completionHandler:` (macOS 14.4); the app
            // targets 14.0, so the older one stays.
            #[allow(deprecated)]
            let session = unsafe {
                ASWebAuthenticationSession::initWithURL_callbackURLScheme_completionHandler(
                    ASWebAuthenticationSession::alloc(),
                    &url,
                    Some(&NSString::from_str("arbos")),
                    &*handler as *const _ as *mut _,
                )
            };
            // SAFETY: the anchor outlives the session (both live in LIVE).
            unsafe {
                session.setPresentationContextProvider(Some(ProtocolObject::from_ref(&*anchor)));
                // Ephemeral: no "Arbos wants to use relay… to sign in" consent
                // alert before the GitHub page. That alert is a free-floating
                // system window — it came up late or not at all when the app
                // was not active, outlived the app, and `cancel()` cannot
                // close it. The price is a GitHub login without Safari's
                // cookies, once per device (the tokens live in the keychain).
                session.setPrefersEphemeralWebBrowserSession(true);
            }
            // SAFETY: documented to be called once on the main thread.
            let started = unsafe { session.start() };
            if started {
                LIVE.with(|live| *live.borrow_mut() = Some((session, anchor)));
                Started::Sheet
            } else {
                Started::Failed(anyhow!("the sign-in window could not be opened"))
            }
        }

        /// Close the live sheet; its completion block reports Cancelled.
        pub fn cancel() {
            let live = LIVE.with(|live| live.borrow_mut().take());
            if let Some((session, _anchor)) = live {
                // SAFETY: main thread (LIVE is thread-local to it); the
                // session is still retained here while cancel runs.
                unsafe { session.cancel() };
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    use super::*;
    use gpui::AsyncApp;

    /// The port is only known once the listener exists (`redirect()`); this
    /// is the shape `start_url` shows outside the real flow.
    pub fn redirect_uri() -> String {
        "http://127.0.0.1:0/cb".into()
    }

    pub fn redirect() -> Redirect {
        let listener = TcpListener::bind("127.0.0.1:0").ok();
        let port = listener
            .as_ref()
            .and_then(|l| l.local_addr().ok())
            .map(|a| a.port())
            .unwrap_or(0);
        Redirect {
            uri: format!("http://127.0.0.1:{port}/cb"),
            listener,
        }
    }

    pub async fn run(
        start_url: String,
        redirect: Redirect,
        opened: impl FnOnce(bool, &mut App),
        cx: &mut AsyncApp,
    ) -> Option<Result<SignIn>> {
        let Some(listener) = redirect.listener else {
            return Some(Err(anyhow!("no loopback port for the sign-in callback")));
        };
        let wait = Tokio::spawn_result(cx, async move {
            tokio::task::spawn_blocking(move || accept_callback(listener))
                .await
                .map_err(|e| anyhow!("{e}"))?
        });
        if let Err(err) = open_browser(&start_url) {
            return Some(Err(err));
        }
        cx.update(|cx| opened(true, cx));
        let url = futures::select! {
            r = wait.fuse() => match r { Ok(url) => url, Err(err) => return Some(Err(err)) },
            _ = cx.background_executor().timer(SIGN_IN_TIMEOUT).fuse() => {
                return Some(Err(anyhow!("GitHub did not answer in 5 minutes. Try again.")));
            }
        };
        Some(parse_callback(&url).map(SignIn::Grant))
    }

    pub fn cancel() {}

    pub fn open_browser(url: &str) -> Result<()> {
        let browser = std::env::var("ARBOS_BROWSER").unwrap_or_else(|_| "xdg-open".into());
        std::process::Command::new(&browser)
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("open the browser with {browser}"))?;
        Ok(())
    }

    /// One request on the loopback listener: the redirect from the relay.
    fn accept_callback(listener: TcpListener) -> Result<String> {
        listener.set_nonblocking(false)?;
        loop {
            let (mut stream, _) = listener.accept()?;
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]);
            let target = head
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/");
            if !target.starts_with("/cb") {
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                continue;
            }
            let body = "<!doctype html><title>Arbos</title><body style=\"font-family:system-ui;padding:40px\"><p>Signed in. You can close this tab and go back to Arbos.</p></body>";
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .as_bytes(),
            );
            return Ok(format!("http://127.0.0.1{target}"));
        }
    }
}

#[cfg(test)]
mod anchor_tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Win(&'static str);

    #[test]
    fn the_apps_own_window_wins_even_when_appkit_has_no_key_or_main() {
        // The user switched to another app while the relay was being probed:
        // keyWindow and mainWindow are nil, the GPUI window still exists.
        let anchor = choose_anchor(Some(Win("gpui")), None, None, Some(Win("visible")));
        assert_eq!(anchor, Anchor::Window(Win("gpui")));
    }

    #[test]
    fn the_apps_own_window_wins_over_key_and_main() {
        let anchor = choose_anchor(
            Some(Win("gpui")),
            Some(Win("key")),
            Some(Win("main")),
            Some(Win("visible")),
        );
        assert_eq!(anchor, Anchor::Window(Win("gpui")));
    }

    #[test]
    fn appkit_key_then_main_then_any_visible_window() {
        assert_eq!(
            choose_anchor(
                None,
                Some(Win("key")),
                Some(Win("main")),
                Some(Win("visible"))
            ),
            Anchor::Window(Win("key"))
        );
        assert_eq!(
            choose_anchor(None, None, Some(Win("main")), Some(Win("visible"))),
            Anchor::Window(Win("main"))
        );
        assert_eq!(
            choose_anchor(None, None, None, Some(Win("visible"))),
            Anchor::Window(Win("visible"))
        );
    }

    #[test]
    fn no_window_at_all_means_the_browser_never_a_throwaway_window() {
        assert_eq!(
            choose_anchor::<Win>(None, None, None, None),
            Anchor::Browser
        );
    }

    #[test]
    fn callback_urls() {
        assert!(is_callback_url("arbos://auth/callback?code=abc"));
        assert!(!is_callback_url("arbos://join/xyz"));
        assert_eq!(
            parse_callback("arbos://auth/callback?code=abc%20d").unwrap(),
            "abc d"
        );
        let err = parse_callback("arbos://auth/callback?error=oauth_denied").unwrap_err();
        assert_eq!(err.to_string(), "GitHub sign-in was cancelled.");
        assert!(parse_callback("arbos://auth/callback").is_err());
    }

    #[test]
    fn start_url_carries_the_redirect_and_device() {
        let url = start_url("https://relay.example/", "Tobias's Mac");
        assert!(url.starts_with("https://relay.example/auth/github/start?redirect="));
        assert!(url.contains("&device_name=Tobias%27s%20Mac"));
    }
}
