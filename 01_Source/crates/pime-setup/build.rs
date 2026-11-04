// Embeds the icon of the product into the program: the exe shows it in Explorer, in the taskbar and in the list of
// installed apps of Windows.
//
// The resource is compiled by `windres` (the MinGW toolchain that builds this crate; the Windows SDK's `rc.exe` is
// not installed here). When `windres` is not on the PATH the program is built without an icon: a missing icon is
// better than a broken build.
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set by cargo"),
    );
    let icon = manifest.join("assets").join("ime1.0.ico");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", icon.display());

    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.starts_with("x86_64-pc-windows") {
        println!(
            "cargo:warning=the icon is only built for x86_64 Windows (this target is {target})"
        );
        return;
    }
    if !icon.is_file() {
        println!(
            "cargo:warning=the logo {} is missing: the program is built without an icon",
            icon.display()
        );
        return;
    }
    let Some(windres) = find_windres() else {
        println!("cargo:warning=windres was not found: the program is built without an icon");
        return;
    };

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is always set by cargo"));
    let script = out_dir.join("ime1.0.rc");
    let object = out_dir.join("ime1.0_res.o");
    // the path in the resource script uses forward slashes
    let icon_path = icon.to_string_lossy().replace('\\', "/");
    std::fs::write(&script, format!("1 ICON \"{icon_path}\"\n"))
        .expect("the resource script cannot be written");

    let result = Command::new(&windres)
        .arg("-o")
        .arg(&object)
        .arg(&script)
        .status();
    match result {
        Ok(status) if status.success() => {
            // the resource belongs to the program, not to the library or to the tests
            println!("cargo:rustc-link-arg-bins={}", object.display());
        }
        Ok(status) => println!(
            "cargo:warning=windres failed ({status}): the program is built without an icon"
        ),
        Err(error) => println!(
            "cargo:warning=windres could not be started ({error}): the program is built without an icon"
        ),
    }
}

/// The `windres` of the toolchain that builds this crate (it is on the PATH of the build).
fn find_windres() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        for name in ["windres.exe", "windres"] {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
