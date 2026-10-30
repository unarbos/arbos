//! Inline video playback in the media viewer ([feedback
//! app-01M6147S9TPNBQA7KSDEY0K8C4]).
//!
//! On macOS an `AVPlayerView` sits above GPUI's Metal view, framed to the
//! viewer's picture area (same insets as `media_viewer`'s body). Play fetches
//! the blob to a temp file; this module attaches the player. Closing the
//! viewer, paging away, or a `None` sync tears it down. Elsewhere the stub
//! does nothing — Linux builds keep the previous "open with the system" path.

use std::path::Path;

use gpui::Window;

/// Keep the native player in sync with the media viewer's playing path.
/// `path = Some` shows / replaces the player; `None` removes it.
pub fn sync(window: &mut Window, path: Option<&Path>) {
    platform::sync(window, path);
}

#[cfg(target_os = "macos")]
mod platform {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSAutoresizingMaskOptions, NSView, NSWindowOrderingMode};
    use objc2_av_foundation::AVPlayer;
    use objc2_av_kit::{AVPlayerView, AVPlayerViewControlsStyle};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSURL};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    /// Match `media_viewer`'s picture room so Close, chevrons and the bottom
    /// pills stay hittable around the player.
    const INSET_TOP: f64 = 70.0;
    const INSET_BOTTOM: f64 = 90.0;
    /// `EDGE` (28) + chevron (36) + gap (20) — the body's left/right inset.
    const INSET_SIDE: f64 = 84.0;

    struct Active {
        path: PathBuf,
        view: Retained<AVPlayerView>,
        content_key: usize,
    }

    thread_local! {
        static ACTIVE: RefCell<Option<Active>> = RefCell::new(None);
    }

    fn content_view(window: &gpui::Window) -> Option<Retained<NSView>> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };
        // SAFETY: GPUI's raw window handle is the live window's NSView*.
        let view = unsafe { appkit.ns_view.cast::<NSView>().as_ref() };
        view.window()?.contentView()
    }

    fn player_frame(content: &NSView) -> NSRect {
        let b = content.bounds();
        let width = (b.size.width - 2.0 * INSET_SIDE).max(1.0);
        let height = (b.size.height - INSET_TOP - INSET_BOTTOM).max(1.0);
        // AppKit origin is bottom-left; INSET_BOTTOM lifts the player off the pills.
        NSRect::new(
            NSPoint::new(b.origin.x + INSET_SIDE, b.origin.y + INSET_BOTTOM),
            NSSize::new(width, height),
        )
    }

    fn tear_down() {
        ACTIVE.with(|slot| {
            if let Some(active) = slot.borrow_mut().take() {
                if let Some(player) = unsafe { active.view.player() } {
                    unsafe { player.pause() };
                }
                unsafe { active.view.setPlayer(None) };
                active.view.removeFromSuperview();
            }
        });
    }

    pub(super) fn sync(window: &mut gpui::Window, path: Option<&Path>) {
        let Some(path) = path else {
            tear_down();
            return;
        };
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(content) = content_view(window) else {
            return;
        };
        let content_key = Retained::as_ptr(&content) as usize;
        let frame = player_frame(&content);

        let same = ACTIVE.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|a| a.path == path && a.content_key == content_key)
        });
        if same {
            ACTIVE.with(|slot| {
                if let Some(active) = slot.borrow().as_ref() {
                    active.view.setFrame(frame);
                    // Keep the player above GPUI's Metal view after a layout pass.
                    content.addSubview_positioned_relativeTo(
                        &active.view,
                        NSWindowOrderingMode::Above,
                        None,
                    );
                }
            });
            return;
        }

        tear_down();

        let ns_path = NSString::from_str(&path.to_string_lossy());
        let url = NSURL::fileURLWithPath(&ns_path);
        // SAFETY: `playerWithURL` / `initWithFrame` / `setPlayer` are the
        // standard AVKit setup on the main thread; we own the retained view.
        let player = unsafe { AVPlayer::playerWithURL(&url, mtm) };
        let view = unsafe { AVPlayerView::initWithFrame(AVPlayerView::alloc(mtm), frame) };
        unsafe {
            view.setPlayer(Some(&player));
            view.setControlsStyle(AVPlayerViewControlsStyle::Inline);
        }
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable
                | NSAutoresizingMaskOptions::ViewMinYMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        content.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Above, None);
        unsafe { player.play() };

        ACTIVE.with(|slot| {
            *slot.borrow_mut() = Some(Active {
                path: path.to_path_buf(),
                view,
                content_key,
            });
        });
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::path::Path;

    pub(super) fn sync(_window: &mut gpui::Window, _path: Option<&Path>) {}
}
