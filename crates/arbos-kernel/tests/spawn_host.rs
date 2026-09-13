//! qa-031: `spawn host="local"` on a place with no machines configured was
//! refused ("no machine named \"local\" in machines.toml or .arbos/machines/"),
//! so a fresh project could not spawn one worker; the model then tried
//! "host1" and gave up. The usual spellings of "here" mean a local child,
//! and with nothing configured anywhere no name can mean another machine.

use arbos_core::{Place, bootstrap};
use arbos_kernel::remote::{HostChoice, choose_host, is_local_host};

fn place(name: &str) -> Place {
    let dir = std::env::temp_dir().join(format!(
        "arbos-spawn-host-{name}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let place = Place::new(&dir);
    bootstrap(&place).unwrap();
    place
}

#[test]
fn the_spellings_of_here_are_a_local_spawn() {
    for host in [
        "local",
        "localhost",
        "here",
        "this",
        "This Machine",
        "none",
        "default",
        "auto",
        "",
        " - ",
    ] {
        assert!(is_local_host(host), "{host:?} must mean this machine");
    }
    assert!(!is_local_host("gpu-box") && !is_local_host("arboslife"));
    let p = place("here");
    assert_eq!(choose_host(&p, "local"), HostChoice::Local { note: None });
}

#[test]
fn nothing_configured_runs_here_with_a_note_and_a_wrong_name_among_real_machines_is_remote() {
    // One test, in order: both halves set XDG_CONFIG_HOME, which tests in
    // parallel threads would race on.
    let cfg = std::env::temp_dir().join(format!("arbos-spawn-host-cfg-{}", std::process::id()));
    std::fs::create_dir_all(cfg.join("arbos")).unwrap();
    // SAFETY: the only reader is this thread, below.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &cfg) };
    let p = place("nothing");
    match choose_host(&p, "host1") {
        HostChoice::Local { note: Some(note) } => assert!(note.contains("host1"), "{note}"),
        other => {
            panic!("with no machines anywhere, host1 must run here with a note, got {other:?}")
        }
    }

    std::fs::write(
        cfg.join("arbos").join("machines.toml"),
        "[[machine]]\nname = \"gpu-box\"\nssh = \"user@gpu\"\ndir = \"/work\"\n",
    )
    .unwrap();
    assert_eq!(choose_host(&p, "gpu-bocks"), HostChoice::Remote);
    assert_eq!(choose_host(&p, "local"), HostChoice::Local { note: None });
}
