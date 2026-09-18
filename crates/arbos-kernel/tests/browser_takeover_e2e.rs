//! Side-panels handover 7: the browser panel goes live, and a person can
//! take the wheel.
//!
//! `browser_watch` streams the agent's page as JPEG frames to the asking
//! window. `browser_drive user` hands the page to the person: their
//! `browser_input` lands, and the agent's driving actions are refused in
//! its own turn while its reads still work; `browser_drive agent` hands
//! it back and the person's input is refused. Skipped when no Chrome is
//! on the machine, as `browser_e2e` is.

mod common;

use common::{Attach, start_kernel_replay};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

/// A page with one big button at the top-left corner, so a click at
/// (50, 50) CSS pixels hits it whatever the viewport.
const PAGE: &str = r#"<!doctype html><html><head><title>Takeover page</title>
<style>body{margin:0} #big{position:absolute;left:0;top:0;width:300px;height:200px;font-size:40px}</style>
</head><body>
<button id="big" onclick="document.getElementById('out').textContent='clicked by hand'">Press</button>
<p id="out" style="position:absolute;top:220px">untouched</p>
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
        .filter(|e| e["kind"] == "tool" && e["name"] == "browser")
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
fn a_person_watches_the_page_live_and_takes_the_wheel() {
    if !has_chrome() {
        eprintln!("no chrome here; skipping");
        return;
    }
    let port = page_server();
    let url = format!("http://127.0.0.1:{port}/");
    let replies = format!(
        concat!(
            "{{\"agent\":\"root\",\"content\":\"opening\",\"calls\":[{{\"name\":\"browser\",\"arguments\":{{\"action\":\"navigate\",\"url\":\"{url}\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"opened\"}}\n",
            "{{\"agent\":\"root\",\"content\":\"reloading\",\"calls\":[{{\"name\":\"browser\",\"arguments\":{{\"action\":\"navigate\",\"url\":\"{url}\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"tried\"}}\n",
            "{{\"agent\":\"root\",\"content\":\"looking\",\"calls\":[{{\"name\":\"browser\",\"arguments\":{{\"action\":\"snapshot\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"looked\"}}\n",
            "{{\"agent\":\"root\",\"content\":\"reloading again\",\"calls\":[{{\"name\":\"browser\",\"arguments\":{{\"action\":\"navigate\",\"url\":\"{url}\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"reloaded\"}}\n",
        ),
        url = url
    );
    let k = start_kernel_replay("browser-takeover", &replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let mut b = Attach::connect(&k.url);
    let _ = b.wait(Duration::from_secs(5), |f| f["type"] == "hello");

    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "open the page"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(120)));
    let results = tool_results(&k.place);
    if let Some((_, first)) = results.first()
        && (first.contains("never announced a devtools port") || first.contains("no chrome"))
    {
        eprintln!("chrome present but did not come up ({first}); skipping");
        return;
    }
    assert!(results[0].1.contains("Takeover page"), "{results:?}");

    // Live: frames arrive on the watching window.
    a.send(serde_json::json!({"type":"browser_watch","agent":"root","on":true}));
    let frame = a
        .wait(Duration::from_secs(20), |f| f["type"] == "browser_frame")
        .expect("a frame of the page");
    assert_eq!(frame["agent"], "root");
    assert!(
        frame["data"].as_str().unwrap().len() > 100,
        "{}",
        frame["data"].as_str().unwrap().len()
    );
    assert!(frame["width"].as_u64().unwrap() > 0 && frame["height"].as_u64().unwrap() > 0);
    assert!(frame["ts"].as_i64().unwrap() > 0);

    // The agent drives by default: a person's click is refused.
    a.send(serde_json::json!({"type":"browser_input","agent":"root","kind":"click","x":50,"y":50}));
    let err = a
        .wait(Duration::from_secs(5), |f| f["type"] == "error")
        .expect("refused");
    assert!(
        err["detail"]
            .as_str()
            .unwrap()
            .contains("the agent is driving"),
        "{err}"
    );

    // The person takes the wheel; both windows hear it.
    a.send(serde_json::json!({"type":"browser_drive","agent":"root","driver":"user"}));
    let d = a
        .wait(Duration::from_secs(5), |f| f["type"] == "browser_driver")
        .expect("driver said");
    assert_eq!(d["driver"], "user");
    assert!(d["since_ms"].as_i64().unwrap() > 0, "{d}");
    assert!(!d["by"].as_str().unwrap_or("").is_empty(), "{d}");
    let on_b = b
        .wait(Duration::from_secs(5), |f| f["type"] == "browser_driver")
        .expect("the other window hears");
    assert_eq!(on_b["driver"], "user");

    // The agent's driving action is refused in its own turn.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "reload"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    let results = tool_results(&k.place);
    assert!(
        results[1].1.contains("the person is driving this page"),
        "{}",
        results[1].1
    );

    // The person's click lands; the agent can still read what it did.
    a.send(serde_json::json!({"type":"browser_input","agent":"root","kind":"click","x":50,"y":50}));
    assert!(
        a.wait(Duration::from_secs(2), |f| f["type"] == "error")
            .is_none(),
        "the click was taken"
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "look"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    let results = tool_results(&k.place);
    assert!(
        results[2].1.contains("clicked by hand"),
        "the snapshot shows the person's click: {}",
        results[2].1
    );

    // Handed back: the person's input is refused, the agent drives.
    a.send(serde_json::json!({"type":"browser_drive","agent":"root","driver":"agent"}));
    let d = a
        .wait(Duration::from_secs(5), |f| f["type"] == "browser_driver")
        .expect("driver said");
    assert_eq!(d["driver"], "agent");
    assert!(d.get("since_ms").is_none(), "{d}");
    a.send(serde_json::json!({"type":"browser_input","agent":"root","kind":"click","x":50,"y":50}));
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "error")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "reload again"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    let results = tool_results(&k.place);
    assert!(results[3].1.contains("Takeover page"), "{}", results[3].1);

    a.send(serde_json::json!({"type":"browser_watch","agent":"root","on":false}));
    a.send(serde_json::json!({"type":"browser_drive","agent":"root","driver":"sideways"}));
    let err = a
        .wait(Duration::from_secs(5), |f| f["type"] == "error")
        .expect("refused");
    assert!(
        err["detail"].as_str().unwrap().contains("user or agent"),
        "{err}"
    );
}
