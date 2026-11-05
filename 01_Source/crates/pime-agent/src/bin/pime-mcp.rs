//! The MCP server of the input method: JSON-RPC 2.0 over stdio.
//!
//! The server speaks the MCP protocol on stdin and stdout, so stdout carries nothing but the protocol: the answer
//! of the input method travels in the protocol, never in a diagnostic.
//!
//! The core binary is looked for in the environment variable `PIME_CORE_EXE`, and next to this binary otherwise.

#![deny(unsafe_code)]

fn main() {
    // a panic must not print the text of the user, and it must not take the host of the client down silently
    std::panic::set_hook(Box::new(|_| {
        eprintln!("pime-mcp: the server stopped unexpectedly");
    }));
    let backend = pime_agent::Backend::system();
    if let Err(error) = pime_agent::mcp::run_stdio(&backend) {
        eprintln!("pime-mcp: {error}");
        std::process::exit(1);
    }
}
