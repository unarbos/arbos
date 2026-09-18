//! qal-j31: a place MCP file that does not parse fell through in silence —
//! the file was skipped with a line on stderr, and every server name it
//! meant to define came from the machine's file instead. Now the person
//! reads it on root's transcript at the kernel's start, and the machine's
//! file is not used in its place.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

fn transcript(place: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn a_place_mcp_file_that_does_not_parse_is_said_on_the_transcript() {
    let k = start_kernel_replay_prepared("mcp-parse-said", "", "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        // One typo: an unclosed array.
        std::fs::write(
            place.join(".arbos/mcp.toml"),
            "[servers.notes]\ncommand = \"notes-mcp\"\nargs = [\n",
        )
        .unwrap();
        // The place's next file in the walk names the same server too: the
        // walk stops at the broken file, so this one is not read either.
        std::fs::create_dir_all(place.join(".cursor")).unwrap();
        std::fs::write(
            place.join(".cursor/mcp.json"),
            r#"{"mcpServers":{"notes":{"command":"/nonexistent/cursor-notes-mcp"}}}"#,
        )
        .unwrap();
        // The machine's file names the same server: it must not be taken
        // in the place's stead.
        let xdg = place.parent().unwrap().join("xdg").join("arbos");
        std::fs::create_dir_all(&xdg).unwrap();
        std::fs::write(
            xdg.join("mcp.toml"),
            "[servers.notes]\ncommand = \"/nonexistent/global-notes-mcp\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    assert!(
        common::wait_for(Duration::from_secs(10), || transcript(&k.place).iter().any(
            |e| e["kind"] == "notice"
                && e["failed"] == true
                && e["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("MCP: .arbos/mcp.toml does not parse"))
        )),
        "the person is told: {:?}",
        transcript(&k.place)
    );
    let text = transcript(&k.place)
        .into_iter()
        .find(|e| {
            e["kind"] == "notice" && e["text"].as_str().is_some_and(|t| t.starts_with("MCP:"))
        })
        .unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        text.contains("no MCP file after it was read in its place"),
        "{text}"
    );
    assert!(text.contains("Fix the file and restart"), "{text}");
    // The kernel's log has the same fact, and nothing about the global
    // server having been tried.
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    assert!(log.contains("mcp_config"), "{log}");
    assert!(
        !log.contains("global-notes-mcp") && !log.contains("cursor-notes-mcp"),
        "the machine's server was not started: {log}"
    );
}
