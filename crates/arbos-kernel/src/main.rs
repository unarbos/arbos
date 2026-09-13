use anyhow::{Result, bail};

fn main() -> Result<()> {
    // A write past a file-size limit (quota, ulimit -f) raises SIGXFSZ,
    // whose default action kills the process mid-write and leaves a
    // half line on disk. Ignored, the write returns EFBIG and is handled
    // like any other failed write. Rust does the same for SIGPIPE.
    unsafe {
        libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
    }
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "serve".into());
    match cmd.as_str() {
        "serve" => {
            let place = args
                .next()
                .or_else(|| std::env::var("PWD").ok())
                .unwrap_or_else(|| ".".into());
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(arbos_kernel::serve::run(place))
        }
        "help" | "-h" | "--help" => {
            println!("arbos-kernel serve [place]");
            Ok(())
        }
        other => bail!("unknown command {other}"),
    }
}
