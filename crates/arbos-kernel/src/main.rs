use anyhow::{Context, Result, bail};

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
            // `serve [place] [--bind HOST:PORT]`: the bind goes into the
            // environment before the runtime starts, where serve reads it
            // (and a kernel `run` starts inherits it).
            let mut place = None;
            while let Some(a) = args.next() {
                match a.as_str() {
                    "--bind" => {
                        let addr = args.next().context("--bind needs host:port")?;
                        // SAFETY: before the runtime and its threads start.
                        unsafe { std::env::set_var(arbos_kernel::access::BIND_ENV, addr) };
                    }
                    other if other.starts_with('-') => bail!("serve: unknown flag {other}"),
                    other => place = Some(other.to_string()),
                }
            }
            let place = place
                .or_else(|| std::env::var("PWD").ok())
                .unwrap_or_else(|| ".".into());
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(arbos_kernel::serve::run(place))
        }
        "setup" => arbos_kernel::setup::run(arbos_kernel::setup::Args::parse(args)?),
        "run" => {
            let code = arbos_kernel::cli::run(arbos_kernel::cli::Args::parse(args)?)?;
            std::process::exit(code);
        }
        "answer" => {
            let parsed = arbos_kernel::cli::Args::parse(args)?;
            let (allow, follow) = (parsed.allow, parsed.follow);
            let code = arbos_kernel::cli::answer_cmd(parsed, allow, follow)?;
            std::process::exit(code);
        }
        "attach" => {
            let parsed = arbos_kernel::cli::Args::parse(args)?;
            let all = std::env::args()
                .skip(2)
                .all(|a| a != "--agent" && a != "-a");
            let code = arbos_kernel::cli::attach(parsed, all)?;
            std::process::exit(code);
        }
        "help" | "-h" | "--help" => {
            println!(
                "arbos-kernel serve [place] [--bind HOST:PORT]   (off loopback: tokens in <place>/.arbos/access.toml, [[client]] name/token|token_env/role)"
            );
            println!("{}", arbos_kernel::setup::USAGE);
            println!("{}", arbos_kernel::cli::USAGE);
            Ok(())
        }
        other => bail!("unknown command {other}"),
    }
}
