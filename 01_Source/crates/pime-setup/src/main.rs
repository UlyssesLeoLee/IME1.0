// A console-subsystem program on purpose: a shell waits for it, so `--silent` has a usable exit code and `--dry-run`
// prints where the user typed it. When it is double-clicked (interactive mode) `cli` frees the console right away.
fn main() {
    // Before anything can load a DLL by name: only System32 from now on (the copy that is installed as uninstall.exe
    // may sit in a folder that other users can write to).
    if let Err(e) = pime_setup::real::harden_dll_search() {
        eprintln!("pime-setup: warning: {e}; continuing with the default DLL search order");
    }
    std::process::exit(pime_setup::cli::run(std::env::args().skip(1).collect()));
}
