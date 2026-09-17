//! `arbos-kernel store nosync|home <place>`: move the store out of a
//! cloud-synced folder and leave `.arbos` as a symlink to it.
//! `arbos-kernel store read|ls|put <address>`: a file in another node's
//! store, by address, through the hub in `~/.config/arbos/hub.toml`.

use anyhow::{Context, Result, bail};
use arbos_core::{Place, cloudsync};

pub const USAGE: &str = "arbos-kernel store nosync|home|status <place>   (move .arbos out of iCloud/file-provider sync: to .arbos.nosync beside the project, or under ~/.arbos/stores/; .arbos becomes a symlink)\narbos-kernel store read|ls arbos://<machine>/<project>/<path>   |   store put arbos://<machine>/<project>/<path> <file> [--base HASH]   (another node's store, through the hub)";

pub fn run(mut args: impl Iterator<Item = String>) -> Result<i32> {
    let how = match args.next().as_deref() {
        Some("read") | Some("ls") | Some("put") => unreachable!("handled by run_remote"),
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

/// `store read|ls|put <address> …`: the same three operations the tools
/// use, for a person at a shell. Always through the hub (a CLI is on no
/// node itself).
pub fn run_remote(verb: &str, mut args: impl Iterator<Item = String>) -> Result<i32> {
    let address = args
        .next()
        .with_context(|| format!("store {verb} needs an address\n{USAGE}"))?;
    let place = Place::new(std::env::current_dir()?);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        match verb {
            "read" => {
                let file = crate::hub_link::store_read(&place, &address).await?;
                print!("{}", file.text);
                if file.truncated {
                    eprintln!(
                        "[{address}: {} bytes; only {} shown]",
                        file.size,
                        file.text.len()
                    );
                }
                eprintln!("[sha256 {}]", file.hash);
            }
            "ls" => {
                for e in crate::hub_link::store_list(&place, &address).await? {
                    println!("{}{}", e.name, if e.dir { "/" } else { "" });
                }
            }
            "put" => {
                let file = args
                    .next()
                    .context("store put needs <file> after the address")?;
                let mut base = None;
                while let Some(a) = args.next() {
                    match a.as_str() {
                        "--base" => base = args.next(),
                        other => bail!("store put: unknown argument {other}"),
                    }
                }
                let text = if file == "-" {
                    let mut s = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                    s
                } else {
                    std::fs::read_to_string(&file).with_context(|| format!("read {file}"))?
                };
                let w = crate::hub_link::store_write(&place, &address, text, base).await?;
                println!("wrote {address} ({} bytes) sha256 {}", w.size, w.hash);
            }
            _ => unreachable!(),
        }
        Ok(0)
    })
}
