//! Hold-Fn dictation, same rule as the Mac app's `FnKey.swift`.
//!
//! macOS delivers Fn as `flagsChanged` on key code 63 with the Function
//! modifier, not as a normal key down. Arrow keys also set that modifier, so
//! we only watch that one code.

#![cfg(target_os = "macos")]

use block::ConcreteBlock;
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};
use std::cell::Cell;
use std::ffi::c_void;
use std::rc::Rc;

const FN_KEY_CODE: u16 = 63;
const NS_EVENT_MASK_FLAGS_CHANGED: u64 = 1 << 12;
const NS_EVENT_MODIFIER_FLAG_FUNCTION: usize = 1 << 23;

pub struct Monitor {
    token: *mut Object,
    _block: Box<dyn std::any::Any>,
}

unsafe impl Send for Monitor {}

impl Monitor {
    /// Call `on_edge(true)` on Fn down, `false` on Fn up. The callback runs
    /// on the AppKit thread that saw the event.
    pub fn start(on_edge: impl Fn(bool) + 'static) -> Self {
        let held = Rc::new(Cell::new(false));
        let block = ConcreteBlock::new(move |event: *mut Object| -> *mut Object {
            if event.is_null() {
                return event;
            }
            unsafe {
                let code: u16 = msg_send![event, keyCode];
                if code != FN_KEY_CODE {
                    return event;
                }
                let flags: usize = msg_send![event, modifierFlags];
                let down = flags & NS_EVENT_MODIFIER_FLAG_FUNCTION != 0;
                if down != held.get() {
                    held.set(down);
                    on_edge(down);
                }
            }
            event
        });
        let block = block.copy();
        let token: *mut Object = unsafe {
            msg_send![
                class!(NSEvent),
                addLocalMonitorForEventsMatchingMask: NS_EVENT_MASK_FLAGS_CHANGED
                handler: &*block as *const _ as *mut c_void
            ]
        };
        Self {
            token,
            _block: Box::new(block),
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        if !self.token.is_null() {
            unsafe {
                let _: () = msg_send![class!(NSEvent), removeMonitor: self.token];
            }
            self.token = std::ptr::null_mut();
        }
    }
}
