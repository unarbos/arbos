//! How long the two opens take, measured the same way each time.
//!
//! "Opening a Terminal is very slow, and opening the Browser is very slow"
//! is a claim about wall-clock time, so it is answered with wall-clock time
//! rather than with a reading of the code. Every number here is taken from
//! the client's side of the attach socket — the same place the window sits —
//! and printed with `--nocapture`, so a run before a change and a run after
//! it are comparable line for line.
//!
//! Four measures, because "slow" is four different questions:
//!
//! - **shell → board**: the kernel minted the pty and said so. What the
//!   window waits for before it can draw a terminal tab at all.
//! - **shell → first pty**: the prompt is on the wire. What a person means
//!   by "the terminal opened".
//! - **browse → board**: the row exists. The tab can be drawn.
//! - **browse → picture**: the first frame of the page.
//!
//! And one more that is not about either open: while a browser is coming
//! up, is the kernel still answering anything else? `Frame::Browse` used to
//! run Chromium's whole cold start on the kernel's one frame loop, so every
//! other client frame — a keystroke into a terminal, a message to an agent —
//! queued behind it. `a_second_open_during_a_browser_start` measures that
//! directly: send `browse`, then `shell` right behind it, and time the
//! shell's board.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::{Duration, Instant};

/// Generous ceilings. The point of this file is the printed numbers; the
/// assertions only catch a return to the old behaviour, so they sit far
/// above a healthy run and well below what was measured before the fix.
const BOARD_BUDGET: Duration = Duration::from_millis(3_000);
const PTY_BUDGET: Duration = Duration::from_millis(6_000);
/// Chromium's own cold start is not ours to speed up, so the picture keeps
/// a wide budget. What must not happen is the *kernel* waiting on it.
const PICTURE_BUDGET: Duration = Duration::from_secs(45);

fn has_chrome() -> bool {
    ["chromium", "google-chrome", "chromium-browser"]
        .iter()
        .any(|b| which::which(b).is_ok())
}

fn ms(at: Instant) -> u128 {
    at.elapsed().as_millis()
}

/// Terminal: the board frame, then the shell's own first output.
#[test]
fn opening_a_terminal_is_prompt() {
    let mut k = start_kernel_replay("open-speed-terminal", "");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(10), |f| f["type"] == "snapshot")
            .is_some()
    );

    let asked = Instant::now();
    a.send(serde_json::json!({"type": "shell"}));
    let board = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "board" && f["panel"] == "terminal" && f["action"] == "open"
        })
        .expect("a board frame for the shell");
    let board_ms = ms(asked);
    let id = board["terminal_ids"][0].as_str().unwrap().to_string();
    assert!(
        a.wait(Duration::from_secs(30), |f| {
            f["type"] == "pty" && f["page"] == id.as_str()
        })
        .is_some(),
        "no pty output for {id}"
    );
    let pty_ms = ms(asked);

    println!("MEASURE terminal board_ms={board_ms} first_pty_ms={pty_ms}");
    assert!(
        board_ms <= BOARD_BUDGET.as_millis(),
        "shell → board took {board_ms} ms (budget {} ms)",
        BOARD_BUDGET.as_millis()
    );
    assert!(
        pty_ms <= PTY_BUDGET.as_millis(),
        "shell → first pty took {pty_ms} ms (budget {} ms)",
        PTY_BUDGET.as_millis()
    );
    let _ = k.child.kill();
}

/// Browser: the row, then the first picture of the page.
#[test]
fn opening_a_browser_is_prompt() {
    if !has_chrome() {
        eprintln!("no chrome here; skipping");
        return;
    }
    let mut k = start_kernel_replay("open-speed-browser", "");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(10), |f| f["type"] == "snapshot")
            .is_some()
    );

    let asked = Instant::now();
    a.send(serde_json::json!({"type": "browse", "url": "about:blank"}));
    assert!(
        a.wait(Duration::from_secs(30), |f| {
            f["type"] == "board" && f["panel"] == "browser" && f["action"] == "open"
        })
        .is_some(),
        "no board frame for the page"
    );
    let board_ms = ms(asked);
    let picture = a.wait(PICTURE_BUDGET, |f| {
        f["type"] == "browser" && f["page"] == "b1"
    });
    let picture_ms = ms(asked);
    let Some(picture) = picture else {
        // Chrome is on the machine but cannot come up here (no display
        // parts, a locked-down container). The board number still stands.
        println!("MEASURE browser board_ms={board_ms} picture_ms=none (chrome did not come up)");
        let _ = k.child.kill();
        return;
    };

    println!("MEASURE browser board_ms={board_ms} picture_ms={picture_ms}");
    assert!(
        board_ms <= BOARD_BUDGET.as_millis(),
        "browse → board took {board_ms} ms (budget {} ms)",
        BOARD_BUDGET.as_millis()
    );
    assert!(
        picture["screenshot"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "the first browser frame carries no picture: {picture}"
    );
    let _ = k.child.kill();
}

/// The kernel goes on serving while a browser starts.
///
/// `shell` is sent one frame behind `browse` and its board is timed. It has
/// nothing to do with Chromium, so the only thing that can delay it is the
/// kernel itself being busy.
#[test]
fn a_second_open_during_a_browser_start_is_not_held_up() {
    if !has_chrome() {
        eprintln!("no chrome here; skipping");
        return;
    }
    let mut k = start_kernel_replay("open-speed-both", "");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(10), |f| f["type"] == "snapshot")
            .is_some()
    );

    let asked = Instant::now();
    a.send(serde_json::json!({"type": "browse", "url": "about:blank"}));
    a.send(serde_json::json!({"type": "shell"}));
    assert!(
        a.wait(Duration::from_secs(60), |f| {
            f["type"] == "board" && f["panel"] == "terminal" && f["action"] == "open"
        })
        .is_some(),
        "no board frame for the shell asked for behind the browse"
    );
    let shell_ms = ms(asked);

    println!("MEASURE stall shell_behind_browse_ms={shell_ms}");
    assert!(
        shell_ms <= BOARD_BUDGET.as_millis(),
        "a shell asked for behind a browse waited {shell_ms} ms for the kernel (budget {} ms)",
        BOARD_BUDGET.as_millis()
    );
    let _ = k.child.kill();
}
