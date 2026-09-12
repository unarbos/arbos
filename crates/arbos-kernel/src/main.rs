use anyhow::{Result, bail};

fn main() -> Result<()> {
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
        "run" => {
            let code = arbos_kernel::cli::run(arbos_kernel::cli::Args::parse(args)?)?;
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
            println!("arbos-kernel serve [place]");
            println!("{}", arbos_kernel::cli::USAGE);
            Ok(())
        }
        other => bail!("unknown command {other}"),
    }
}
