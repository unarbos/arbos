//! Print the remote bootstrap pass, so it can be read and run by hand.
//!
//!     cargo run -p arbos-core --example bootstrap-script -- \
//!         ~/.local/bin/arbos-kernel ~/newer-arbos-kernel
//!
//! The script is generated rather than shipped as a file, so this is how
//! somebody checks what a machine would actually be asked to do before
//! asking it — and how the live pass on a disposable host is repeated.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (bin, incoming) = match args.as_slice() {
        [bin, incoming] => (bin.as_str(), incoming.as_str()),
        _ => {
            eprintln!("usage: bootstrap-script <installed-kernel> <new-kernel>");
            std::process::exit(2);
        }
    };
    print!(
        "{}",
        arbos_core::remote_kernel::bootstrap_script(bin, incoming, 20, 10)
    );
}
