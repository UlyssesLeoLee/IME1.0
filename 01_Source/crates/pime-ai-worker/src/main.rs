//! `pime-ai-worker.exe`: JSON Lines over stdin/stdout. Product start-up takes no option (see `config.rs`).
//!
//! Nothing is ever logged. Start-up problems are reported on stderr as fixed text through `redact::diagnostic`, and a
//! panic prints nothing (its message could contain request text).
#![deny(unsafe_code)]

use pime_ai_worker::config::{ConfigError, TEST_MODE_ENV, USAGE, parse_config};
use pime_ai_worker::engine::{Engine, serve};
use pime_ai_worker::redact::diagnostic;
use pime_ai_worker::wincred::WinCredVault;
use pime_ai_worker::winhttp::WinHttpTransport;
use std::io::{self, BufReader};
use std::process::exit;
use std::sync::Arc;

fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let test_mode = std::env::var(TEST_MODE_ENV).ok();
    let config = match parse_config(std::env::args().skip(1), test_mode.as_deref()) {
        Ok(c) => c,
        Err(ConfigError::Help) => {
            eprintln!("{USAGE}");
            exit(0);
        }
        Err(ConfigError::Invalid(message)) => {
            eprintln!("{}", diagnostic(message));
            exit(2);
        }
    };
    let transport = match WinHttpTransport::new() {
        Ok(t) => t,
        Err(code) => {
            eprintln!(
                "{}",
                diagnostic(&format!("WinHTTP could not be opened (error {code})"))
            );
            exit(1);
        }
    };
    let vault = WinCredVault::new(config.session_credentials);
    let engine = Engine::new(
        config,
        Arc::new(transport),
        Arc::new(vault),
        Box::new(io::stdout()),
    );
    let stdin = io::stdin();
    // a read error ends the loop like end of input; there is nobody to report it to
    let _ = serve(&engine, BufReader::new(stdin.lock()));
}
