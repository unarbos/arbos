//! Inline video playback in the media viewer ([feedback
//! app-01M6147S9TPNBQA7KSDEY0K8C4]).
//!
//! On macOS an `AVPlayerView` sits above GPUI's Metal view, framed to the
//! viewer's picture area (same insets as `media_viewer`'s body). Play fetches
//! the blob to a temp file; this module attaches the player. Closing the
//! viewer, paging away, or a `None` sync tears it down. Elsewhere the stub
//! does nothing — Linux builds keep the previous "open with the system" path.
//!
//! Poster frames at send ([feedback app-01M61H3VVBQZ6NCE708XB41NCC]): macOS
//! grabs one JPEG near 0.5 s with `AVAssetImageGenerator`; elsewhere returns
//! `None` so Linux builds stay green.

use std::path::Path;

use gpui::Window;

/// A JPEG frame grabbed from a local video file for the chat bubble / viewer.
#[derive(Clone, Debug)]
pub struct PosterGrab {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
}

/// Keep the native player in sync with the media viewer's playing path.
/// `path = Some` shows / replaces the player; `None` removes it.
pub fn sync(window: &mut Window, path: Option<&Path>) {
    platform::sync(window, path);
}

/// One frame near 0.5 s as a JPEG (longer side ≤ 840). `None` on failure or
/// non-macOS so a video still sends without a poster.
pub fn grab_poster(path: &Path) -> Option<PosterGrab> {
    platform::grab_poster(path)
}

#[cfg(target_os = "macos")]
mod platform {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::ptr;

    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSBitmapImageFileType, NSBitmapImageRep, NSView,
        NSWindowOrderingMode,
    };
    use objc2_av_foundation::{AVAsset, AVAssetImageGenerator, AVPlayer};
    use objc2_av_kit::{AVPlayerView, AVPlayerViewControlsStyle};
    use objc2_core_foundation::CGSize;
    use objc2_core_graphics::CGImage;
    use objc2_core_media::{CMTimeGetSeconds, CMTimeMakeWithSeconds};
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString, NSURL};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::PosterGrab;

    /// Match `media_viewer`'s picture room so Close, chevrons and the bottom
    /// pills stay hittable around the player.
    const INSET_TOP: f64 = 70.0;
    const INSET_BOTTOM: f64 = 90.0;
    /// `EDGE` (28) + chevron (36) + gap (20) — the body's left/right inset.
    const INSET_SIDE: f64 = 84.0;
    /// Longer side of the poster JPEG (feedback app-01M61H3VVBQZ6NCE708XB41NCC).
    const POSTER_MAX_SIDE: f64 = 840.0;
    /// Grab near half a second so the first frame is not a black fade-in.
    const POSTER_TIME_SECS: f64 = 0.5;

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

    /// Encode a `CGImage` as JPEG via `NSBitmapImageRep`.
    fn cgimage_jpeg(image: &CGImage) -> Option<(Vec<u8>, u32, u32)> {
        let width = CGImage::width(Some(image)) as u32;
        let height = CGImage::height(Some(image)) as u32;
        if width == 0 || height == 0 {
            return None;
        }
        // `initWithCGImage` / `representationUsingType` are the AppKit path for
        // CGImage → JPEG; we own the retained rep and data.
        let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
        // SAFETY: empty properties dictionary is valid for JPEG encoding.
        let data = unsafe {
            rep.representationUsingType_properties(
                NSBitmapImageFileType::JPEG,
                &NSDictionary::new(),
            )
        }?;
        Some((data.to_vec(), width, height))
    }

    pub(super) fn grab_poster(path: &Path) -> Option<PosterGrab> {
        let ns_path = NSString::from_str(&path.to_string_lossy());
        let url = NSURL::fileURLWithPath(&ns_path);
        // SAFETY: AVAsset / AVAssetImageGenerator are used on a blocking
        // worker for a local file; we own every retained object we create.
        let asset = unsafe { AVAsset::assetWithURL(&url) };
        let duration = unsafe { asset.duration() };
        let duration_secs = unsafe { CMTimeGetSeconds(duration) };
        if !duration_secs.is_finite() || duration_secs <= 0.0 {
            return None;
        }
        let duration_ms = (duration_secs * 1000.0).round().max(0.0) as u64;
        let time_secs = POSTER_TIME_SECS.min(duration_secs * 0.5);
        let time = unsafe { CMTimeMakeWithSeconds(time_secs, 600) };

        let gen = unsafe { AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset) };
        unsafe {
            gen.setAppliesPreferredTrackTransform(true);
            gen.setMaximumSize(CGSize::new(POSTER_MAX_SIDE, POSTER_MAX_SIDE));
        }
        let cg = unsafe {
            gen.copyCGImageAtTime_actualTime_error(time, ptr::null_mut())
                .ok()?
        };
        let (jpeg, width, height) = cgimage_jpeg(&cg)?;
        Some(PosterGrab {
            jpeg,
            width,
            height,
            duration_ms,
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::path::Path;

    use super::PosterGrab;

    pub(super) fn sync(_window: &mut gpui::Window, _path: Option<&Path>) {}

    pub(super) fn grab_poster(_path: &Path) -> Option<PosterGrab> {
        None
    }
}
