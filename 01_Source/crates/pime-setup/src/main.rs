// A console-subsystem program on purpose: a shell waits for it, so `--silent` has a usable exit code and `--dry-run`
// prints where the user typed it. When it is double-clicked (interactive mode) `cli` frees the console right away.
fn main() {
    std::process::exit(pime_setup::cli::run(std::env::args().skip(1).collect()));
}
