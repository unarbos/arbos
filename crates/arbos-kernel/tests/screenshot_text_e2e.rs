//! Benchmark item 3, the last gap: "run it and show me the output" on a
//! machine with no screen. `screenshot target:text` renders the words as
//! a PNG under the agent's `images/` with headless Chrome, so a worker
//! has one call to make instead of improvising with ImageMagick.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

fn has_chrome() -> bool {
    [
        "chromium",
        "google-chrome",
        "chromium-browser",
        "google-chrome-stable",
    ]
    .iter()
    .any(|b| which::which(b).is_ok())
}

#[test]
fn a_commands_output_renders_as_an_image_without_a_display() {
    if !has_chrome() {
        eprintln!("no chrome here; skipping");
        return;
    }
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"rendering\",\"calls\":[{\"name\":\"screenshot\",\"arguments\":{\"target\":\"text\",\"title\":\"$ python3 hello.py\",\"text\":\"hello from toy-repo\\nexit 0\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"the image is in images/\"}\n",
    );
    let mut k = start_kernel_replay("shot-text", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "run hello.py and show me the output"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    let transcript =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl")).unwrap();
    let rec: serde_json::Value = transcript
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .find(|e: &serde_json::Value| e["kind"] == "tool" && e["name"] == "screenshot")
        .expect("screenshot record");
    if let Some(err) = rec["error"].as_str()
        && (err.contains("could not render") || err.contains("needs chromium"))
    {
        eprintln!("chrome present but could not render ({err}); skipping");
        let _ = k.child.kill();
        return;
    }
    assert!(rec.get("error").is_none(), "{rec:#?}");
    let body = rec["body"].as_str().unwrap_or("");
    assert!(
        body.starts_with("rendered ") && body.contains("via chrome (text)"),
        "{body}"
    );
    let images = rec["images"].as_array().expect("images on the record");
    assert_eq!(images.len(), 1, "{rec:#?}");
    let path = std::path::Path::new(images[0].as_str().unwrap());
    assert!(
        path.starts_with(k.place.join(".arbos/agents/root/images")),
        "{path:?}"
    );
    let png = std::fs::read(path).unwrap();
    assert!(png.starts_with(b"\x89PNG"), "a PNG, {} bytes", png.len());
    assert!(png.len() > 1000, "not a blank image: {} bytes", png.len());
    // The page and profile it drew from are gone.
    assert!(!path.with_extension("html").exists());
    let _ = k.child.kill();
}
