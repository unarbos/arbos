//! The system share picker for a link (the iPhone's share sheet on the Mac):
//! `NSSharingServicePicker` anchored at a point of the window — Messages,
//! Mail, AirDrop, Notes and whatever else the user has installed. Elsewhere
//! the link goes to the clipboard instead.

use gpui::{App, Pixels, Point, Window};

/// What happened, so the caller can say it (a copy needs a notice; the picker speaks for itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shared {
    Picker,
    Copied,
}

/// Offer `text` to the share services, the picker's arrow at `at` (window coordinates).
pub fn link(text: &str, at: Point<Pixels>, window: &mut Window, cx: &mut App) -> Shared {
    if platform::show(text, at, window) {
        return Shared::Picker;
    }
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.to_owned()));
    Shared::Copied
}

#[cfg(target_os = "macos")]
mod platform {
    use std::cell::RefCell;

    use gpui::{Pixels, Point, Window};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSSharingServicePicker, NSView};
    use objc2_foundation::{NSArray, NSPoint, NSRect, NSRectEdge, NSSize, NSString};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    thread_local! {
        /// The picker on screen, kept alive until the next one replaces it.
        static PICKER: RefCell<Option<Retained<NSSharingServicePicker>>> =
            const { RefCell::new(None) };
    }

    pub(super) fn show(text: &str, at: Point<Pixels>, window: &mut Window) -> bool {
        if MainThreadMarker::new().is_none() {
            return false;
        }
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return false;
        };
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return false;
        };
        // SAFETY: GPUI's raw window handle is the live window's NSView*.
        let view = unsafe { appkit.ns_view.cast::<NSView>().as_ref() };
        let bounds = view.bounds();
        let x = f64::from(f32::from(at.x));
        let y = f64::from(f32::from(at.y));
        let y = if view.isFlipped() {
            y
        } else {
            bounds.size.height - y
        };
        let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(1.0, 1.0));
        let item: Retained<AnyObject> =
            Retained::into_super(Retained::into_super(NSString::from_str(text)));
        let items = NSArray::from_retained_slice(&[item]);
        // SAFETY: an NSString conforms to NSPasteboardWriting, as `initWithItems:` requires.
        let picker = unsafe {
            NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items)
        };
        picker.showRelativeToRect_ofView_preferredEdge(rect, view, NSRectEdge::MinY);
        PICKER.with(|slot| *slot.borrow_mut() = Some(picker));
        true
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use gpui::{Pixels, Point, Window};

    pub(super) fn show(_text: &str, _at: Point<Pixels>, _window: &mut Window) -> bool {
        false
    }
}
