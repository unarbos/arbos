//! Process parity, slice 5: the project page follows Cursor's `notes.md`
//! algorithm on its own. A fourth finished item in a section moves to
//! `archived.md` (never dropped); once the page is big the tool keeps a
//! `<tldr>` of the freshest four; a retired worker's row links its PR
//! when it opened one.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::time::{Duration, Instant};

fn wait_for(timeout: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    ok()
}

fn coordinator(place: &Path) {
    std::fs::create_dir_all(place.join(".arbos")).unwrap();
    std::fs::write(
        place.join(".arbos/project.toml"),
        "schema = 2\n[root]\nrole = \"coordinator\"\n",
    )
    .unwrap();
}

#[test]
fn a_fourth_finished_item_moves_to_archived_md_and_a_big_page_gets_a_tldr() {
    // Six items in two sections, then four checks in the first section.
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"laying out the page\",\"calls\":[{\"name\":\"plan\",\"arguments\":{\"op\":\"set\",\"items\":[",
        "{\"section\":\"Voice\",\"text\":\"[Echo gate](agents/echo) — landing\"},",
        "{\"section\":\"Voice\",\"text\":\"[Duplex model](agents/duplex) — evaluating\"},",
        "{\"section\":\"Voice\",\"text\":\"[Phone build](agents/phone) — building\"},",
        "{\"section\":\"Voice\",\"text\":\"[DNS names](agents/dns) — waiting on Jacob\"},",
        "{\"section\":\"Loops\",\"text\":\"[QA loop](agents/qa) — hourly\"},",
        "{\"section\":\"Loops\",\"text\":\"[SWE-bench](agents/swe) — 12 of 16\"}",
        "]}}]}\n",
        "{\"agent\":\"root\",\"content\":\"checking four\",\"calls\":[",
        "{\"name\":\"plan\",\"arguments\":{\"op\":\"check\",\"n\":1,\"readout\":\"echo gate live\",\"target\":\"docs/echo.md\"}},",
        "{\"name\":\"plan\",\"arguments\":{\"op\":\"check\",\"n\":1,\"readout\":\"duplex chosen\",\"target\":\"docs/duplex.md\"}},",
        "{\"name\":\"plan\",\"arguments\":{\"op\":\"check\",\"n\":1,\"readout\":\"build 12 on the phone\",\"target\":\"docs/phone.md\"}},",
        "{\"name\":\"plan\",\"arguments\":{\"op\":\"check\",\"n\":1,\"readout\":\"CNAMEs added\",\"target\":\"docs/dns.md\"}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"Voice is done; loops continue.\"}\n",
    );
    let mut k = start_kernel_replay_prepared("page-algo", replies, "", coordinator);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "lay out the page and close out voice"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));

    let page = std::fs::read_to_string(k.place.join(".arbos/notes.md")).unwrap();
    // Three checked stay in Voice, the oldest left.
    assert!(
        !page.contains("[Echo gate]"),
        "the oldest finished item left the page: {page}"
    );
    for kept in [
        "[Duplex model](docs/duplex.md)",
        "[Phone build](docs/phone.md)",
        "[DNS names](docs/dns.md)",
    ] {
        assert!(page.contains(kept), "{page}");
    }
    // It moved to archived.md under its section, not into thin air.
    let archived = std::fs::read_to_string(k.place.join(".arbos/archived.md")).unwrap();
    assert!(archived.contains("## Voice"), "{archived}");
    assert!(
        archived.contains("- [x] [Echo gate](docs/echo.md) — echo gate live"),
        "{archived}"
    );
    // Two sections and six items: the tool keeps a tldr, freshest first,
    // capped at four; the preamble's context link stays above it.
    let open = page.find("<tldr>").expect("a tldr on a big page");
    let close = page.find("</tldr>").unwrap();
    assert!(page.find("project-context.md").unwrap() < open, "{page}");
    assert!(open < page.find("## Voice").unwrap(), "{page}");
    let bullets: Vec<&str> = page[open..close]
        .lines()
        .filter(|l| l.starts_with("- "))
        .collect();
    // Four items were touched; the first one has since left the page, so
    // its bullet went with it: three stay, freshest first.
    assert_eq!(bullets.len(), 3, "{page}");
    assert!(
        bullets[0].starts_with("- [DNS names](docs/dns.md) — CNAMEs added"),
        "freshest first: {page}"
    );
    assert!(bullets[2].starts_with("- [Duplex model]"), "{page}");
    assert!(
        bullets.iter().all(|b| !b.contains("[Echo gate]")),
        "a bullet whose item left the page is gone: {page}"
    );
    // The lint is content with the shape.
    let place = arbos_core::Place::new(&k.place);
    let problems = arbos_core::store::lint_notes(&page);
    assert!(problems.is_empty(), "{problems:?}\n{page}");
    let _ = place;
    let _ = k.child.kill();
}

#[test]
fn a_retired_workers_row_links_its_pr_when_it_opened_one() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"plan\",\"arguments\":{\"op\":\"add\",\"section\":\"Kernel\",\"text\":\"[Echo gate](agents/echo-gate) — worker running\"}},{\"name\":\"spawn\",\"arguments\":{\"name\":\"echo-gate\",\"task\":\"add the echo gate\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        "{\"content\":\"gate added; PR opened\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    );
    let mut k = start_kernel_replay_prepared("page-pr-link", replies, "", |place| {
        coordinator(place);
        // The record the kernel keeps when a worker's `gh pr create`
        // succeeds, authored here: the worker echo-gate opened #7.
        std::fs::write(
            place.join(".arbos/prs.jsonl"),
            "{\"ts\":1,\"agent\":\"echo-gate\",\"url\":\"https://github.com/unarbos/arbos/pull/7\",\"repo\":\"unarbos/arbos\",\"number\":7,\"branch\":\"arbos/echo-gate\"}\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "add the echo gate"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let page = k.place.join(".arbos/notes.md");
    let retired = "- [x] [Echo gate](https://github.com/unarbos/arbos/pull/7) — worker finished: gate added; PR opened";
    assert!(
        wait_for(Duration::from_secs(30), || std::fs::read_to_string(&page)
            .unwrap_or_default()
            .contains(retired)),
        "the retired row links the PR, not the worker: {}",
        std::fs::read_to_string(&page).unwrap_or_default()
    );
    let text = std::fs::read_to_string(&page).unwrap();
    assert!(
        !text.contains("archive/agents/echo-gate"),
        "never both: {text}"
    );
    let _ = k.child.kill();
}
