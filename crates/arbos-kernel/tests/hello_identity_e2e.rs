//! A client learns the place's face — name, glyph, colour from
//! `project.toml` — on `hello`, the first frame, so a phone draws the
//! same identity as the desktop's tab before it has read anything.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

#[test]
fn hello_carries_the_places_face_and_a_bare_place_gets_the_folder() {
    let mut k = start_kernel_replay_prepared("hello-face", "", "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"Arbos\"\nicon = \"terminal\"\ncolor = \"teal\"\n\n[root]\nrole = \"coordinator\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    let hello = a
        .wait(Duration::from_secs(5), |f| f["type"] == "hello")
        .expect("hello");
    assert_eq!(hello["identity"]["name"], "Arbos");
    assert_eq!(hello["identity"]["icon"], "terminal");
    assert_eq!(hello["identity"]["color"], "teal");
    let _ = k.child.kill();

    // No face in the file: the folder glyph, no name, no colour.
    let mut k = start_kernel_replay_prepared("hello-bare", "", "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(place.join(".arbos/project.toml"), "schema = 2\n").unwrap();
    });
    let mut a = Attach::connect(&k.url);
    let hello = a
        .wait(Duration::from_secs(5), |f| f["type"] == "hello")
        .expect("hello");
    assert_eq!(hello["identity"]["icon"], "folder");
    assert!(hello["identity"].get("name").is_none(), "{hello}");
    assert!(hello["identity"].get("color").is_none(), "{hello}");
    let _ = k.child.kill();
}
