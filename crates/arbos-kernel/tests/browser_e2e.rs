//! P-06c: the browser tool's page actions against a local page, through a
//! scripted model. Skipped when no Chrome is on the machine.

mod common;

use common::{Attach, start_kernel_replay};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

const PAGE: &str = r#"<!doctype html><html><head><title>Form page</title></head><body>
<h1>Hello form</h1>
<form onsubmit="event.preventDefault(); document.getElementById('out').textContent = 'submitted ' + document.getElementById('name').value + ' / ' + document.getElementById('color').value; console.log('form submitted'); return false;">
<input id="name" type="text" value="old" placeholder="Your name">
<select id="color"><option value="r">Red</option><option value="g">Green</option><option value="b">Blue</option></select>
<button id="go" type="submit">Go</button>
</form>
<p id="out">nothing yet</p>
<div style="height: 3000px"></div>
<p id="bottom">the bottom</p>
<script>console.warn('page ready');</script>
</body></html>"#;

fn page_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{PAGE}",
                    PAGE.len()
                );
            });
        }
    });
    port
}

fn has_chrome() -> bool {
    ["chromium", "google-chrome", "chromium-browser"]
        .iter()
        .any(|b| which::which(b).is_ok())
}

fn tool_results(place: &std::path::Path) -> Vec<(String, String)> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == "tool")
        .map(|e| {
            (
                e["args"]["action"].as_str().unwrap_or("").to_string(),
                format!(
                    "{}{}",
                    e["body"].as_str().unwrap_or(""),
                    e["error"].as_str().unwrap_or("")
                ),
            )
        })
        .collect()
}

#[test]
fn fill_select_press_scroll_wait_eval_console_drive_a_page() {
    if !has_chrome() {
        eprintln!("no chrome here; skipping");
        return;
    }
    let port = page_server();
    let b = |action: &str, extra: &str| {
        format!(r#"{{"name":"browser","arguments":{{"action":"{action}"{extra}}}}}"#)
    };
    // One call per step so each result is its own tool line.
    let steps = [
        b("navigate", &format!(r#","url":"http://127.0.0.1:{port}/""#)),
        b("fill", r#","ref":"1","text":"Ada""#),
        b("select", r#","ref":"2","value":"Green""#),
        b("hover", r#","ref":"3""#),
        b("press", r#","key":"Enter""#),
        b("wait", r#","text":"submitted Ada / g","ms":"3000""#),
        b("scroll", r#","direction":"bottom""#),
        b(
            "eval",
            r#","expression":"document.getElementById('out').textContent""#,
        ),
        b("console", ""),
        b("back", ""),
    ];
    let mut replies = String::new();
    for s in &steps {
        replies.push_str(&format!(
            "{{\"agent\":\"root\",\"content\":\"step\",\"calls\":[{s}]}}\n"
        ));
    }
    replies.push_str("{\"agent\":\"root\",\"content\":\"done driving\"}\n");
    let mut k = start_kernel_replay("browser", &replies);
    // The scripted root needs the browser tool: an existing place keeps
    // every tool, but a fresh one is a coordinator (browser included).
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "drive the form"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(120)));
    let results = tool_results(&k.place);
    // Chrome is there but cannot run on this machine (no display server
    // parts, a locked-down container): the tool says so; nothing to
    // assert about page actions then.
    if let Some((_, first)) = results.first()
        && (first.contains("never announced a devtools port") || first.contains("no chrome"))
    {
        eprintln!("chrome present but did not come up ({first}); skipping");
        return;
    }
    let get = |action: &str| {
        results
            .iter()
            .find(|(a, _)| a == action)
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| panic!("no {action} result in {results:#?}"))
    };
    assert!(get("navigate").contains("Form page"), "{}", get("navigate"));
    assert!(get("fill").starts_with("filled"), "{}", get("fill"));
    assert_eq!(get("select"), "selected Green in 2");
    assert!(get("hover").starts_with("hovering 3"), "{}", get("hover"));
    assert!(get("wait").starts_with("found"), "{}", get("wait"));
    assert!(
        get("scroll").contains("scrolled bottom"),
        "{}",
        get("scroll")
    );
    assert!(get("eval").contains("submitted Ada / g"), "{}", get("eval"));
    let console = get("console");
    assert!(
        console.contains("page ready") || console.contains("form submitted"),
        "{console}"
    );
    assert!(get("back").starts_with("went back"), "{}", get("back"));
    let _ = k.child.kill();
}
