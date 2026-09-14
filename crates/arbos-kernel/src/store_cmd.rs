//! `arbos-kernel store nosync|home <place>`: move the store out of a
//! cloud-synced folder and leave `.arbos` as a symlink to it.

use anyhow::{Result, bail};
use arbos_core::{Place, cloudsync};

pub const USAGE: &str = "arbos-kernel store nosync|home <place>   (move .arbos out of iCloud/file-provider sync: to .arbos.nosync beside the project, or under ~/.arbos/stores/; .arbos becomes a symlink)";

pub fn run(mut args: impl Iterator<Item = String>) -> Result<i32> {
    let how = match args.next().as_deref() {
        Some("nosync") => cloudsync::Relocation::Nosync,
        Some("home") => cloudsync::Relocation::Home,
        Some("status") => {
            let place = place_arg(args.next())?;
            match cloudsync::detect(place.path()) {
                Some(sync) => println!("{}: inside {} sync", place.path().display(), sync.label()),
                None => println!("{}: not in a synced folder", place.path().display()),
            }
            match cloudsync::relocated(place.path()) {
                Some(store) => println!(".arbos -> {}", store.display()),
                None => println!(".arbos is a plain folder"),
            }
            return Ok(0);
        }
        other => bail!("{USAGE}\n(got {other:?})"),
    };
    let place = place_arg(args.next())?;
    let target = cloudsync::relocate(place.path(), how)?;
    println!(
        "moved the store to {} and left .arbos as a symlink to it",
        target.display()
    );
    Ok(0)
}

fn place_arg(arg: Option<String>) -> Result<Place> {
    Ok(match arg {
        Some(p) => Place::new(p),
        None => Place::new(std::env::current_dir()?),
    })
}
