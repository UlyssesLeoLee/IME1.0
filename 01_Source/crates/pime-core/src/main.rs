//! `pime-core` JSON Lines CLI (protocol 0.3): one JSON object per line on stdin, one response line on stdout.
//! It never executes anything, never opens files or the network, and discards its own panic messages so
//! that input text can never leak through diagnostics.
#![forbid(unsafe_code)]

use std::io::{self, BufReader};

fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let stdin = io::stdin();
    let stdout = io::stdout();
    // a closed pipe simply ends the loop; there is nothing useful to report
    let _ = pime_core::protocol::serve(BufReader::new(stdin.lock()), stdout.lock());
}
