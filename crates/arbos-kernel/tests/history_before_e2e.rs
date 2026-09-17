//! iPhone loop M-54: `history` paged forward only, so a client holding
//! the replayed tail (the last 200 lines) had no way to fetch the lines
//! before it. `history {agent, before, limit}` returns the `limit` lines
//! with `seq < before` nearest to it, oldest first, with the same
//! `history_end`; at the top, `from == to == before` and no lines.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

fn seeded_transcript(n: usize) -> String {
    let mut out = String::new();
    for i in 1..=n {
        out.push_str(&format!(
            "{{\"kind\":\"assistant\",\"text\":\"line {i}\",\"ts\":{}}}\n",
            1_789_500_000_000i64 + i as i64
        ));
    }
    out
}

fn collect_page(a: &mut Attach) -> (Vec<u64>, serde_json::Value) {
    let mut seqs = Vec::new();
    let end = a
        .wait(Duration::from_secs(10), |f| {
            if f["type"] == "replayed" && f["agent"] == "root" {
                seqs.push(f["event"]["seq"].as_u64().unwrap_or(0));
            }
            f["type"] == "history_end" && f["agent"] == "root"
        })
        .expect("history_end");
    (seqs, end)
}

#[test]
fn history_pages_backwards_from_the_replayed_tail_to_the_top() {
    let mut k = start_kernel_replay_prepared("history-before", "", "", |place| {
        let root = place.join(".arbos/agents/root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("agent.md"),
            "---\nname: root\nmodel: inherit\n---\n",
        )
        .unwrap();
        std::fs::write(root.join("transcript.jsonl"), seeded_transcript(500)).unwrap();
    });
    let mut a = Attach::connect(&k.url);
    // Attach: the last 200 lines, seq 301..=500.
    let (tail, end) = collect_page(&mut a);
    assert_eq!(tail.len(), 200, "{end}");
    assert_eq!((tail[0], *tail.last().unwrap()), (301, 500));
    assert_eq!(end["total"], 500);

    // The page above it: 100 lines before 301 → 201..=300, oldest first.
    a.send(serde_json::json!({"type": "history", "agent": "root", "before": 301, "limit": 100}));
    let (page, end) = collect_page(&mut a);
    assert_eq!(page.len(), 100, "{end}");
    assert_eq!((page[0], *page.last().unwrap()), (201, 300));
    assert!(page.windows(2).all(|w| w[0] < w[1]), "oldest first");
    assert_eq!(end["from"], 201);
    assert_eq!(end["to"], 300);
    assert_eq!(end["total"], 500);

    // Again from the new top, more than remain: all 200 that are left.
    a.send(serde_json::json!({"type": "history", "agent": "root", "before": 201, "limit": 1000}));
    let (page, end) = collect_page(&mut a);
    assert_eq!(page.len(), 200, "{end}");
    assert_eq!((page[0], *page.last().unwrap()), (1, 200));

    // At the top: nothing, and the end frame says so.
    a.send(serde_json::json!({"type": "history", "agent": "root", "before": 1, "limit": 100}));
    let (page, end) = collect_page(&mut a);
    assert!(page.is_empty());
    assert_eq!(end["from"], 1);
    assert_eq!(end["to"], 1);

    // Forward paging is unchanged.
    a.send(serde_json::json!({"type": "history", "agent": "root", "since": 498, "limit": 10}));
    let (page, _) = collect_page(&mut a);
    assert_eq!(page, vec![499, 500]);
    let _ = k.child.kill();
}
