//! Programming IME installer (milestone M5, plan phase P5).
//!
//! Layout: `plan` (pure: what an install or uninstall does), `exec` (runs a plan, rolls back), `backend` (the machine:
//! files, HKLM, processes; with an in-memory fake for tests), `real` (the Win32 implementation, UAC elevation and
//! message boxes), `payload` (the files carried inside the installer exe), `text` (the three UI languages), `cli`.
//!
//! STATUS: pre-release. The text service DLL that this installs has not been verified in a virtual machine yet, and the
//! installer is not code signed. Nothing here has ever been run against a real registry or Program Files in the
//! automated tests: those use the fake backend or a temporary folder with `--no-register --no-arp --no-vscode`.

pub mod backend;
pub mod cli;
pub mod exec;
pub mod payload;
pub mod plan;
pub mod real;
pub mod text;
