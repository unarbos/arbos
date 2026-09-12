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
        "setup" => arbos_kernel::setup::run(arbos_kernel::setup::Args::parse(args)?),
        "help" | "-h" | "--help" => {
            println!("arbos-kernel serve [place]");
            println!("{}", arbos_kernel::setup::USAGE);
            Ok(())
        }
        other => bail!("unknown command {other}"),
    }
}
