//! What a terminal pane waits for before it can draw the shell, measured
//! both ways against one real kernel.
//!
//! Run it with `--nocapture`; every line it prints starts with `MEASURE`.
//!
//! - **window socket** — the window's own kernel connection, which is
//!   attached before the shell is asked for. This is what the pane reads
//!   now.
//! - **preflight** — `attach_or_spawn_place`, which the pane used to run on
//!   every open before it connected: canonicalize the folder, bootstrap
//!   `.arbos/`, probe the attach port, ask `/healthz`, compare commits.
//! - **pane socket** — a second attach of the pane's own, asking for the
//!   shell's output. The prompt had already gone out over the window's
//!   connection, so this one hears nothing at all until the person types:
//!   the measure is a timeout, which is the whole point of it.
//!
//! Both routes are timed off the same clock on the same kernel, so the two
//! numbers can be read against each other.

use arbos_desktop::{kernel, model::place::Place};
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    time::{Duration, Instant},
};

/// How long to give the pane's own socket before calling it silent.
const PANE_WAIT: Duration = Duration::from_secs(5);

struct Attach {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Attach {
    fn connect(url: &str) -> Self {
        let addr = kernel::tcp_addr(url).expect("a tcp kernel url");
        let stream = TcpStream::connect(addr).expect("attach");
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        Self {
            writer: stream.try_clone().unwrap(),
            reader: BufReader::new(stream),
        }
    }

    fn send(&mut self, frame: Value) {
        let mut line = frame.to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).unwrap();
    }

    fn wait(&mut self, timeout: Duration, mut pred: impl FnMut(&Value) -> bool) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        let mut line = String::new();
        while Instant::now() < deadline {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => return None,
                Ok(_) => {
                    if let Ok(frame) = serde_json::from_str::<Value>(&line)
                        && pred(&frame)
                    {
                        return Some(frame);
                    }
                }
                Err(_) => continue,
            }
        }
        None
    }
}

#[test]
fn a_shell_reaches_the_window_before_a_pane_could_have_asked_for_it() {
    let place = Place::local(std::env::temp_dir().join(format!(
        "arbos-open-speed-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    )));
    std::fs::create_dir_all(&place.path).unwrap();

    let started = Instant::now();
    let info = match kernel::attach_or_spawn_place(&place) {
        Ok(info) => info,
        Err(err) => {
            // No kernel binary on this machine: build the workspace, or set
            // ARBOS_KERNEL_BIN. Nothing to measure without one.
            eprintln!("skipped: {err:#}");
            let _ = std::fs::remove_dir_all(&place.path);
            return;
        }
    };
    println!("MEASURE kernel_start_ms={}", started.elapsed().as_millis());

    let mut window = Attach::connect(&info.url);
    assert!(
        window
            .wait(Duration::from_secs(10), |f| f["type"] == "snapshot")
            .is_some(),
        "the window's connection never got its snapshot"
    );

    // The window's own connection, which is what the pane reads now.
    let asked = Instant::now();
    window.send(serde_json::json!({"type": "shell"}));
    let board = window
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "board" && f["panel"] == "terminal" && f["action"] == "open"
        })
        .expect("a board frame for the shell");
    let board_ms = asked.elapsed().as_millis();
    let page = board["terminal_ids"][0].as_str().unwrap().to_owned();
    let prompt = window.wait(Duration::from_secs(30), |f| {
        f["type"] == "pty" && f["page"] == page.as_str()
    });
    let window_ms = asked.elapsed().as_millis();
    assert!(
        prompt.is_some(),
        "no output for {page} on the window's own connection"
    );
    println!("MEASURE terminal board_ms={board_ms} window_socket_first_pty_ms={window_ms}");

    // What the pane used to run before it could connect at all.
    let preflight = Instant::now();
    let info = kernel::attach_or_spawn_place(&place).expect("the kernel is up");
    let preflight_ms = preflight.elapsed().as_millis();

    // And what it heard once it had.
    let mut pane = Attach::connect(&info.url);
    let opened = Instant::now();
    pane.send(serde_json::json!({
        "type": "pty_in", "agent": "root", "page": page, "data": "",
    }));
    let heard = pane.wait(PANE_WAIT, |f| {
        f["type"] == "pty" && f["page"] == page.as_str()
    });
    let pane_ms = match heard {
        Some(_) => opened.elapsed().as_millis().to_string(),
        None => format!("none in {}s", PANE_WAIT.as_secs()),
    };
    println!("MEASURE terminal preflight_ms={preflight_ms} pane_socket_first_pty_ms={pane_ms}");

    #[cfg(unix)]
    if info.pid > 0 {
        // SAFETY: a signal to a pid this test started.
        unsafe {
            libc::kill(info.pid, libc::SIGTERM);
        }
    }
    let _ = std::fs::remove_dir_all(&place.path);
}
