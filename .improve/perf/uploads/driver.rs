#![allow(dead_code, unused_imports)]
extern crate self as arbos_core;
mod doc;
mod model;
mod auth {
    pub fn url_encode(s: &str) -> String {
        s.bytes().map(|b| format!("%{b:02X}")).collect()
    }
}
mod sync {
    pub mod chat_send_queue;
    pub mod relay;
    pub mod user_error;
}
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
struct Meter;
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);
static TARGET: AtomicUsize = AtomicUsize::new(usize::MAX);
static TARGET_ALLOCS: AtomicUsize = AtomicUsize::new(0);
fn add(n: usize) {
    TOTAL.fetch_add(n, Ordering::Relaxed);
    if n == TARGET.load(Ordering::Relaxed) { TARGET_ALLOCS.fetch_add(1, Ordering::Relaxed); }
    let x = CURRENT.fetch_add(n, Ordering::Relaxed) + n;
    PEAK.fetch_max(x, Ordering::Relaxed);
}
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            add(l.size())
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        CURRENT.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, n) };
        if !q.is_null() {
            CURRENT.fetch_sub(l.size(), Ordering::Relaxed);
            add(n)
        }
        q
    }
}
#[global_allocator]
static METER: Meter = Meter;
fn heap_current() -> usize {
    CURRENT.load(Ordering::Relaxed)
}
fn heap_reset() {
    PEAK.store(heap_current(), Ordering::Relaxed)
}
fn heap_peak() -> usize {
    PEAK.load(Ordering::Relaxed)
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    sync::relay::run().await
}

fn heap_total()->usize{TOTAL.load(Ordering::Relaxed)}
fn heap_target(size:usize){TARGET.store(size,Ordering::Relaxed);TARGET_ALLOCS.store(0,Ordering::Relaxed);}
fn heap_target_count()->usize{TARGET_ALLOCS.load(Ordering::Relaxed)}
