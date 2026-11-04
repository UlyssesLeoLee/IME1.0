//! IME1.0 installer (milestone M5, plan phase P5).
//!
//! Layout: `plan` (pure: what an install or uninstall does), `exec` (runs a plan, rolls back), `backend` (the machine:
//! files, HKLM, processes; with an in-memory fake for tests), `real` (the Win32 implementation, UAC elevation and
//! message boxes), `payload` (the files carried inside the installer exe), `text` (the three UI languages), `cli`.
//!
//! The install folder: the program always lives in a folder of its own, named "IME1.0". The place that is chosen
//! (in the window or with `--prefix`) is the folder above it, and the product folder is put into it: the root of a
//! drive is never the install folder. The rule is in `plan::resolve_install_dir`.
//!
//! STATUS: pre-release. The text service DLL that this installs has not been verified in a virtual machine yet, and the
//! installer is not code signed. Nothing here has ever been run against a real registry or Program Files in the
//! automated tests: those use the fake backend or a temporary folder with `--no-register --no-arp --no-vscode`.

pub mod backend;
pub mod cli;
pub mod detect;
pub mod exec;
pub mod paint;
pub mod payload;
pub mod plan;
pub mod real;
pub mod text;
pub mod wizard;
