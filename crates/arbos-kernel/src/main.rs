use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    // A write past a file-size limit (quota, ulimit -f) raises SIGXFSZ,
    // whose default action kills the process mid-write and leaves a
    // half line on disk. Ignored, the write returns EFBIG and is handled
    // like any other failed write. Rust does the same for SIGPIPE.
    unsafe {
        libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
    }
    // A launcher can hand its children a signal mask with SIGCHLD blocked
    // (it is inherited across exec). The runtime's child reaper is
    // signal-driven on macOS, so a kernel started that way never hears a
    // command end: five jobs exited 0 as zombies with no tool result
    // (Jacob's Mac, 2026-09-17). Unblocked here, before the runtime
    // starts; said on stderr when it had to be, so a log shows it.
    #[cfg(unix)]
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGCHLD);
        let mut old: libc::sigset_t = std::mem::zeroed();
        if libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, &mut old) == 0
            && libc::sigismember(&old, libc::SIGCHLD) == 1
        {
            eprintln!(
                "arbos-kernel: SIGCHLD was blocked in the inherited signal mask — unblocked, or no command would ever be seen to end"
            );
        }
    }
    // The path this process was started with, kept before anything can
    // replace the binary under it (see `binary`).
    arbos_kernel::binary::remember_start();
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "serve".into());
    match cmd.as_str() {
        // Which file a kernel started by this process would run from, and
        // why, when it is not this process's own binary. `--wait-for
        // <file>` answers only once the file exists (a test replaces the
        // binary meanwhile).
        "binary" => {
            let mut wait_for = None;
            let mut it = args;
            while let Some(a) = it.next() {
                if a == "--wait-for" {
                    wait_for = it.next();
                }
            }
            if let Some(flag) = wait_for {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                while !std::path::Path::new(&flag).exists() && std::time::Instant::now() < deadline
                {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
            let chosen = arbos_kernel::binary::kernel_binary()?;
            println!("{}", chosen.path.display());
            if let Some(note) = chosen.note {
                println!("note: {note}");
            }
            return Ok(());
        }
        "serve" => {
            // `serve [place] [--provider replay --replies FILE]`: the
            // provider choice goes into the environment before the runtime
            // starts, where every turn reads it.
            let mut place = None;
            let mut provider = None;
            let mut replies = None;
            while let Some(a) = args.next() {
                match a.as_str() {
                    "--provider" => provider = args.next(),
                    "--replies" => replies = args.next(),
                    "--bind" => {
                        let addr = args.next().context("--bind needs host:port")?;
                        // SAFETY: before the runtime and its threads start.
                        unsafe { std::env::set_var(arbos_kernel::access::BIND_ENV, addr) };
                    }
                    "--hub" => set_env(
                        arbos_kernel::hub_link::URL_ENV,
                        &args.next().context("--hub needs a wss:// url")?,
                    ),
                    "--machine" => set_env(
                        arbos_kernel::hub_link::MACHINE_ENV,
                        &args.next().context("--machine needs a name")?,
                    ),
                    "--project" => set_env(
                        arbos_kernel::hub_link::PROJECT_ENV,
                        &args.next().context("--project needs a name")?,
                    ),
                    "--until-idle" => set_env(arbos_kernel::idle::UNTIL_IDLE_ENV, "1"),
                    "--leash" => set_env(
                        arbos_kernel::idle::LEASH_ENV,
                        &args
                            .next()
                            .context("--leash needs a duration (10m, 600s)")?,
                    ),
                    "--horizon" => set_env(
                        arbos_kernel::idle::HORIZON_ENV,
                        &args
                            .next()
                            .context("--horizon needs a duration (1h, 30m)")?,
                    ),
                    "--now" => set_env(
                        arbos_core::NOW_ENV,
                        &args
                            .next()
                            .context("--now needs a time (2026-09-13T09:00:00Z)")?,
                    ),
                    other if other.starts_with('-') => bail!("serve: unknown flag {other}"),
                    other => place = Some(other.to_string()),
                }
            }
            match provider.as_deref() {
                None => {}
                Some("replay") => {
                    let file = replies.context("--provider replay needs --replies FILE")?;
                    arbos_engine::replay::select(std::path::Path::new(&file));
                }
                Some(other) => bail!(
                    "serve: unknown provider {other} (only replay is selectable here; the model provider is in config.toml)"
                ),
            }
            let place = place
                .or_else(|| std::env::var("PWD").ok())
                .unwrap_or_else(|| ".".into());
            let rt = tokio::runtime::Runtime::new()?;
            let code = rt.block_on(arbos_kernel::serve::run(place))?;
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
        "check" => {
            let code = arbos_kernel::check::run(arbos_kernel::check::Args::parse(args)?)?;
            std::process::exit(code);
        }
        "store" => {
            let mut args = args.peekable();
            let code = match args.peek().map(String::as_str) {
                Some(verb @ ("read" | "ls" | "put")) => {
                    let verb = verb.to_string();
                    args.next();
                    arbos_kernel::store_cmd::run_remote(&verb, args)?
                }
                _ => arbos_kernel::store_cmd::run(args)?,
            };
            std::process::exit(code);
        }
        "prompt" => {
            let code =
                arbos_kernel::prompt_size::run(arbos_kernel::prompt_size::Args::parse(args)?)?;
            std::process::exit(code);
        }
        "rollout" => {
            let code = arbos_kernel::rollout::run(arbos_kernel::rollout::Args::parse(args)?)?;
            std::process::exit(code);
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
        "log" => {
            let mut n = 20usize;
            let mut place = None;
            let mut it = args;
            while let Some(a) = it.next() {
                match a.as_str() {
                    "-n" => n = it.next().and_then(|v| v.parse().ok()).unwrap_or(20),
                    other => place = Some(other.to_string()),
                }
            }
            let place = arbos_core::Place::new(
                std::fs::canonicalize(place.unwrap_or_else(|| ".".into()))
                    .unwrap_or_else(|_| std::env::current_dir().unwrap()),
            );
            for line in arbos_kernel::snapshot::log(&place, n)? {
                println!("{line}");
            }
            Ok(())
        }
        "rewind" => {
            let code = arbos_kernel::rewind::run(arbos_kernel::rewind::Args::parse(args)?)?;
            std::process::exit(code);
        }
        "--version" | "-V" | "version" => {
            println!(
                "arbos-kernel {} {} protocol {}",
                arbos_kernel::klog::version(),
                arbos_kernel::klog::git_sha(),
                arbos_kernel::serve::PROTOCOL
            );
            Ok(())
        }
        "update" => {
            let code = arbos_kernel::update_cmd::run(arbos_kernel::update_cmd::Args::parse(args)?)?;
            std::process::exit(code);
        }
        "worker" => {
            let code = arbos_kernel::worker::run(arbos_kernel::worker::Args::parse(args)?)?;
            std::process::exit(code);
        }
        "help" | "-h" | "--help" => {
            println!(
                "arbos-kernel serve [place] [--provider replay --replies FILE] [--bind HOST:PORT] [--hub wss://URL --machine NAME [--project NAME]] [--until-idle] [--horizon 1h] [--leash 10m] [--now 2026-09-13T09:00:00Z]   (off loopback: tokens in <place>/.arbos/access.toml, [[client]] name/token|token_env/role; --hub registers with an arbos-hub, token from ~/.config/arbos/hub.toml or ARBOS_HUB_TOKEN)"
            );
            println!("{}", arbos_kernel::rollout::USAGE);
            println!("{}", arbos_kernel::check::USAGE);
            println!("{}", arbos_kernel::prompt_size::USAGE);
            println!("{}", arbos_kernel::store_cmd::USAGE);
            println!("{}", arbos_kernel::setup::USAGE);
            println!("{}", arbos_kernel::cli::USAGE);
            println!("{}", arbos_kernel::rewind::USAGE);
            println!("{}", arbos_kernel::update_cmd::USAGE);
            println!("{}", arbos_kernel::worker::USAGE);
            Ok(())
        }
        other => bail!(
            "unknown command {other}. Commands: serve, run, answer, attach, log, rollout, check, prompt, store, setup, rewind, update, worker, version, help. \
             A kernel that answers this to `update` is older than the feature and cannot update itself: from any newer arbos-kernel, run `arbos-kernel update --install --binary <path to the old file>`; afterwards it can."
        ),
    }
}

/// Flags become environment for the process that is about to start.
fn set_env(key: &str, value: &str) {
    // SAFETY: called from `main` before the runtime and its threads start.
    unsafe { std::env::set_var(key, value) };
}
