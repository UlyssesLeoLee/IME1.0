//! The registry keys our input method is registered under, and their removal.
//!
//! Windows keeps the language profiles of an input method (the entries "IME1.0 中文" / "IME1.0 English" /
//! "IME1.0 日本語" in the list of Windows) under the TSF key of its text service; the COM class of the text
//! service lives under its CLSID. The uninstall removes both keys **itself**: `regsvr32 /u` has to load the DLL,
//! and a scanner on this machine can refuse to load a freshly installed binary - then the entries of our input
//! method stay in the list of Windows while the program is already deleted, and there is no way left to remove
//! them (the report of the user on 2026-11-24).
//!
//! The keys belong to this product and to nothing else.

/// The CLSID of our text service (the same value as `CLSID_PIME` in `crates/pime-tsf/src/lib.rs`).
pub const TEXT_SERVICE_CLSID: &str = "{43F95ED1-95B4-421D-8F5A-837128C2D854}";

/// The TSF key of our text service: its language profiles and its categories live under it.
pub const TIP_KEY: &str = r"Software\Microsoft\CTF\TIP\{43F95ED1-95B4-421D-8F5A-837128C2D854}";

/// The COM class of our text service.
pub const COM_CLASS_KEY: &str = r"Software\Classes\CLSID\{43F95ED1-95B4-421D-8F5A-837128C2D854}";

/// The keys the uninstall removes: the input method itself (its language profiles and its categories) and its COM
/// class.
pub const KEYS: [&str; 2] = [TIP_KEY, COM_CLASS_KEY];

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The keys must belong to the text service of the product: a test reads the source of the DLL and checks
    /// that the two agree, so the two crates cannot drift apart.
    #[test]
    fn the_keys_belong_to_the_text_service_of_the_product() {
        let tsf = fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../pime-tsf/src/lib.rs"),
        )
        .expect("the source of the text service");
        let line = tsf
            .lines()
            .find(|l| l.contains("pub const CLSID_PIME:"))
            .expect("CLSID_PIME is not in the source of the text service");
        let hex = line
            .split("0x")
            .nth(1)
            .map(|rest| rest.split(|c: char| !c.is_ascii_hexdigit()).collect::<String>())
            .expect("the value of the constant");
        let expected = format!(
            "{{{}-{}-{}-{}-{:}}}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        );
        assert_eq!(
            TEXT_SERVICE_CLSID.to_lowercase(),
            expected.to_lowercase(),
            "the installer and the text service must use the same CLSID"
        );
        for key in KEYS {
            assert!(
                key.to_lowercase().contains(&expected.to_lowercase()),
                "{key} is not under the CLSID of the text service"
            );
        }
    }
}
