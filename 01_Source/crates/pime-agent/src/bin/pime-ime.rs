//! The command line front-end of the input method.
//!
//! `pime-ime <op> [arguments]` runs one operation and answers with one line (or with the raw JSON answer when
//! `--json` is given). `pime-ime serve` starts the local server.
//!
//! The core binary is looked for in the environment variable `PIME_CORE_EXE`, and next to this binary otherwise.

#![deny(unsafe_code)]

fn main() {
    // a panic must not print the text of the user
    std::panic::set_hook(Box::new(|_| {
        eprintln!("pime-ime: the command stopped unexpectedly");
    }));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let backend = pime_agent::Backend::system();
    let code = pime_agent::cli::run(&args, &backend);
    std::process::exit(code);
}
