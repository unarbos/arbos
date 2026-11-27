//! Shared 30 fps pulse clock for streaming dots and spinners (zeron-study
//! §2.6): one timer drives every pulsing view instead of per-frame redraws.
//! A view that needs the next tick calls [`PulseClock::request`] while it
//! renders; when nobody asks, the clock stays idle.

use std::time::{Duration, Instant};

use gpui::{App, Global};

pub const PULSE_FPS: u64 = 30;
pub const COMPOSER_DROP_MS: u64 = 220;
pub const SHAKE_MS: u64 = 280;

pub struct PulseClock {
    started: Instant,
    wanted: bool,
}

impl Global for PulseClock {}

impl PulseClock {
    pub fn init(cx: &mut App) {
        cx.set_global(PulseClock {
            started: Instant::now(),
            wanted: false,
        });
        cx.spawn(async move |cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(1000 / PULSE_FPS))
                .await;
            cx.update(|cx| {
                let wanted = std::mem::take(&mut cx.global_mut::<PulseClock>().wanted);
                if wanted {
                    for window in cx.windows() {
                        let _ = window.update(cx, |_, window, _| window.refresh());
                    }
                }
            });
        })
        .detach();
    }

    /// Ask for one more tick. Call from `render` while something animates.
    pub fn request(cx: &mut App) {
        cx.global_mut::<PulseClock>().wanted = true;
    }

    /// 0..1 triangle wave with the given period.
    pub fn phase(cx: &App, period_ms: u64) -> f32 {
        let t = Self::ramp(cx, period_ms);
        if t < 0.5 {
            t * 2.0
        } else {
            2.0 - t * 2.0
        }
    }

    /// 0 → 1 over `period_ms`, then round again (a sawtooth: for things that
    /// turn, not breathe — the activity spinner).
    pub fn ramp(cx: &App, period_ms: u64) -> f32 {
        let elapsed = cx.global::<PulseClock>().started.elapsed().as_millis() as u64;
        (elapsed % period_ms) as f32 / period_ms as f32
    }
}

pub fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t;
    1.0 - u * u * u
}

/// The refused-close header shake `dt_ms` in: two cycles of 3 pt across [`SHAKE_MS`], decaying
/// to rest.
pub fn shake_offset(dt_ms: u64) -> f32 {
    let t = (dt_ms as f32 / SHAKE_MS as f32).min(1.0);
    (t * std::f32::consts::PI * 4.0).sin() * 3.0 * (1.0 - t)
}

/// macOS Accessibility → Display → Reduce motion, read once at launch; GPUI's
/// `with_animation` and the fades that check `cx.reduce_motion()` follow it.
pub fn system_reduce_motion() -> bool {
    std::process::Command::new("defaults")
        .args(["read", "com.apple.universalaccess", "reduceMotion"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shake_starts_and_ends_at_rest_and_decays() {
        assert_eq!(shake_offset(0), 0.0);
        assert!(shake_offset(SHAKE_MS).abs() < 1e-3);
        assert!(shake_offset(SHAKE_MS * 2).abs() < 1e-3);
        // First peak (an eighth in) is the largest; the last (seven eighths) is under 1 pt.
        let peak = |eighths: u64| shake_offset(SHAKE_MS * eighths / 8).abs();
        assert!(peak(1) > 2.0 && peak(1) <= 3.0);
        assert!(peak(7) < 1.0);
    }
}
