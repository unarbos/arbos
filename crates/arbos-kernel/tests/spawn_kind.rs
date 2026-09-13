//! qa-019: `spawn kind="default"` on a place with no agent definitions was
//! refused ("no agent definition named \"default\""), so a fresh project
//! could not spawn a single worker. Models fill every optional field; the
//! usual spellings for "no kind" must mean the built-in child.

use arbos_core::{Place, bootstrap};
use arbos_kernel::hooks::{Isolate, KernelHooks, is_no_kind};
use std::sync::Arc;

fn hooks(name: &str) -> (Arc<KernelHooks>, arbos_core::Agent) {
    let dir = std::env::temp_dir().join(format!(
        "arbos-spawn-kind-{name}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let place = Place::new(&dir);
    let root = bootstrap(&place).unwrap();
    let (wake_tx, _wake_rx) = tokio::sync::mpsc::unbounded_channel();
    let (kick_tx, _kick_rx) = tokio::sync::mpsc::unbounded_channel();
    (KernelHooks::new(place, wake_tx, kick_tx), root)
}

#[test]
fn default_and_none_kinds_spawn_the_built_in_child() {
    let (h, root) = hooks("default");
    for kind in ["default", "none", "", "  ", "Default", "auto"] {
        let (id, _wt) = h
            .spawn_isolated(
                &root,
                "write w1.txt",
                None,
                None,
                false,
                None,
                Isolate::None,
                Some(kind),
            )
            .unwrap_or_else(|e| panic!("kind={kind:?} must spawn the built-in child: {e:#}"));
        assert!(h.place.agent_dir(id.as_str()).join("agent.md").exists());
    }
    assert!(is_no_kind("NONE") && is_no_kind("default") && !is_no_kind("reviewer"));
}

#[test]
fn any_kind_spawns_the_built_in_child_when_no_definitions_exist() {
    // "inherit" is what the model writes for the model field; it wrote it
    // for kind too. And with no definitions at all, no name can mean one.
    let (h, root) = hooks("nodefs");
    for kind in ["inherit", "reviewer", "Worker"] {
        h.spawn_isolated(
            &root,
            "brief",
            None,
            None,
            false,
            None,
            Isolate::None,
            Some(kind),
        )
        .unwrap_or_else(|e| panic!("kind={kind:?} with no definitions must spawn: {e:#}"));
    }
    assert!(is_no_kind("inherit"));
}

#[test]
fn a_real_unknown_kind_is_still_refused_with_the_list() {
    let (h, root) = hooks("unknown");
    // One real definition exists, so an unknown name is a mistake to report.
    let defs = h.place.arbos().join("agents-defs");
    std::fs::create_dir_all(&defs).unwrap();
    std::fs::write(
        defs.join("tester.md"),
        "---\nname: tester\n---\nYou test things.\n",
    )
    .unwrap();
    let err = h
        .spawn_isolated(
            &root,
            "brief",
            None,
            None,
            false,
            None,
            Isolate::None,
            Some("reviewer"),
        )
        .expect_err("a named kind that does not exist is an error");
    assert!(format!("{err:#}").contains("no agent definition named \"reviewer\""));
}
