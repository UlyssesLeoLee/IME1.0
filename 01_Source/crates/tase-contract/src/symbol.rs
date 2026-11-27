//! The kinds of a symbol and the digest of its signature: the two values of a plan that are part of the wire.
//!
//! A plan names the symbol an operation works on ([`crate::api::Target`]) and, when the operation is checked
//! against a new head, the digest of the signature the plan was built against ([`crate::api::Preconditions`]).
//! Both are part of the contract, so the contract fixes them:
//!
//! * the **kind of a symbol** comes from a closed list ([`SymbolKind`]). A name that is not in the list is
//!   refused at the door of the contract instead of being compared as a free string: a plan that says
//!   `"func"` has to fail with "unknown symbol kind", and not with a symbol that is suddenly missing;
//! * the **digest of a signature** is the `"sha256:"` prefix and the SHA-256 of the signature text
//!   ([`symbol_signature_hash`]). The text is the header of the declaration (`pub fn run(x: u32) -> bool`),
//!   with the runs of blanks collapsed into one space and the ends trimmed - the text the semantic index
//!   stores. A symbol whose header could not be read has the digest of the empty text.
//!
//! The names of the list are the wire names of the semantic layer (`tase-semantic`, the `as_str` of its
//! `SymbolKind`): the two layers name the same kinds the same way, so a plan the contract accepted is a plan
//! the semantic layer can filter.

use crate::error::TaseError;

/// The prefix of the digest of a signature.
pub const SYMBOL_SIGNATURE_HASH_PREFIX: &str = "sha256:";

/// The number of hex characters of the digest of a signature (the length of a SHA-256).
const SIGNATURE_DIGEST_HEX_LEN: usize = 64;

/// The kind of a symbol: the closed list of the wire.
///
/// The list is the one of the semantic layer (`tase-semantic`): the names are the wire names of its kinds, so a
/// plan that names a kind here names the kind the semantic layer filters with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    /// A free function.
    Function,
    /// A function of a type: a method of an `impl` or of a trait, a method of a class.
    Method,
    /// A type alias or any other type declaration without a more precise kind.
    Type,
    /// A structure (or a union).
    Struct,
    /// An enumeration.
    Enum,
    /// A trait.
    Trait,
    /// A module or a namespace.
    Module,
    /// A constant (or a static).
    Const,
    /// A variable.
    Variable,
    /// An interface.
    Interface,
    /// A class.
    Class,
    /// A test function.
    Test,
    /// A declaration whose kind is not known.
    Unknown,
}

impl SymbolKind {
    /// Every kind of the list.
    pub const ALL: [SymbolKind; 13] = [
        SymbolKind::Function,
        SymbolKind::Method,
        SymbolKind::Type,
        SymbolKind::Struct,
        SymbolKind::Enum,
        SymbolKind::Trait,
        SymbolKind::Module,
        SymbolKind::Const,
        SymbolKind::Variable,
        SymbolKind::Interface,
        SymbolKind::Class,
        SymbolKind::Test,
        SymbolKind::Unknown,
    ];

    /// The kind on the wire (the lower-case name).
    pub const fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Type => "type",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Module => "module",
            SymbolKind::Const => "const",
            SymbolKind::Variable => "variable",
            SymbolKind::Interface => "interface",
            SymbolKind::Class => "class",
            SymbolKind::Test => "test",
            SymbolKind::Unknown => "unknown",
        }
    }

    /// The kind of a wire name; a name that is not in the list is refused.
    ///
    /// The failure says which name was refused, so a plan that names a kind the contract does not know fails
    /// with "unknown symbol kind" and not with a symbol that cannot be found.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(kind: &str) -> Result<Self, TaseError> {
        match kind {
            "function" => Ok(SymbolKind::Function),
            "method" => Ok(SymbolKind::Method),
            "type" => Ok(SymbolKind::Type),
            "struct" => Ok(SymbolKind::Struct),
            "enum" => Ok(SymbolKind::Enum),
            "trait" => Ok(SymbolKind::Trait),
            "module" => Ok(SymbolKind::Module),
            "const" => Ok(SymbolKind::Const),
            "variable" => Ok(SymbolKind::Variable),
            "interface" => Ok(SymbolKind::Interface),
            "class" => Ok(SymbolKind::Class),
            "test" => Ok(SymbolKind::Test),
            "unknown" => Ok(SymbolKind::Unknown),
            other => Err(TaseError::invalid_argument(format!(
                "unknown symbol kind {other:?}"
            ))),
        }
    }
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SymbolKind {
    type Err = TaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        SymbolKind::from_str(s)
    }
}

/// The digest of the signature of a symbol.
///
/// The signature is the header of the declaration (`pub fn run(x: u32) -> bool`), with the runs of blanks
/// collapsed into one space and the ends trimmed: the text the semantic index stores in the signature of a
/// symbol. The digest is the `"sha256:"` prefix and the SHA-256 of the UTF-8 bytes of that text, in
/// lower-case hex.
///
/// A symbol whose header could not be read has the digest of the empty text, so the digest of a symbol never
/// depends on how the reader found it.
pub fn symbol_signature_hash(signature: Option<&str>) -> String {
    format!(
        "{SYMBOL_SIGNATURE_HASH_PREFIX}{}",
        pime_core::sha256::sha256_hex(signature.unwrap_or(""))
    )
}

/// Whether a value is a digest of the shape the contract defines.
///
/// A digest is the `"sha256:"` prefix and 64 hex characters in lower-case. A value of another shape (another
/// prefix, another length, an upper-case hex) is not a digest of a signature and is refused.
pub fn is_symbol_signature_hash(value: &str) -> bool {
    let Some(digest) = value.strip_prefix(SYMBOL_SIGNATURE_HASH_PREFIX) else {
        return false;
    };
    digest.len() == SIGNATURE_DIGEST_HEX_LEN
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    /// The names of the list, in the order of the semantic layer.
    const NAMES: [&str; 13] = [
        "function",
        "method",
        "type",
        "struct",
        "enum",
        "trait",
        "module",
        "const",
        "variable",
        "interface",
        "class",
        "test",
        "unknown",
    ];

    #[test]
    fn the_list_of_the_kinds_is_the_list_of_the_wire_names() {
        assert_eq!(SymbolKind::ALL.len(), NAMES.len());
        for (kind, name) in SymbolKind::ALL.iter().zip(NAMES) {
            assert_eq!(kind.as_str(), name, "{kind:?}");
            assert_eq!(SymbolKind::from_str(name).unwrap(), *kind);
            assert_eq!(name.parse::<SymbolKind>().unwrap(), *kind);
            assert_eq!(kind.to_string(), name);
        }
    }

    #[test]
    fn a_name_that_is_not_in_the_list_is_refused_and_is_named() {
        for name in [
            "func",
            "Function",
            "function ",
            " func",
            "FUNCTIONS",
            "fun",
            "",
        ] {
            let error = SymbolKind::from_str(name).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidArgument, "{name:?}");
            assert!(
                error.message.contains("unknown symbol kind"),
                "{name:?}: {}",
                error.message
            );
            assert!(name.parse::<SymbolKind>().is_err(), "{name:?}");
        }
    }

    #[test]
    fn the_digest_of_a_signature_is_the_sha256_of_the_signature_text() {
        // the values are the SHA-256 of the text, computed outside of this repository
        assert_eq!(
            symbol_signature_hash(Some("pub fn run(x: u32) -> bool")),
            "sha256:f68b73098971492353b7793edfdbc828420e0be7727f60f743ea834a38c8540f"
        );
        assert_eq!(
            symbol_signature_hash(Some("abc")),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            symbol_signature_hash(Some("")),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn the_digest_of_the_contract_is_the_digest_the_executor_computes() {
        // the executor computes the digest of the same way (`tase-executor`, `rebase::signature_hash`):
        // the "sha256:" prefix and the SHA-256 of the signature text. The two must not drift apart.
        let signature = "pub fn create_user(name: &str) -> User";
        let digest = symbol_signature_hash(Some(signature));
        assert_eq!(
            digest,
            format!("sha256:{}", pime_core::sha256::sha256_hex(signature))
        );
        assert!(digest.starts_with("sha256:"));
        assert_eq!(digest.len(), "sha256:".len() + 64);
        assert!(is_symbol_signature_hash(&digest));
    }

    #[test]
    fn a_symbol_without_a_readable_header_has_the_digest_of_the_empty_text() {
        assert_eq!(
            symbol_signature_hash(None),
            symbol_signature_hash(Some("")),
            "a header that could not be read is the empty header"
        );
    }

    #[test]
    fn a_digest_of_another_shape_is_not_a_digest_of_a_signature() {
        let digest = symbol_signature_hash(Some("abc"));
        assert!(is_symbol_signature_hash(&digest));
        // another prefix
        assert!(!is_symbol_signature_hash(
            &digest.replacen("sha256:", "sha1:", 1)
        ));
        // no prefix at all
        assert!(!is_symbol_signature_hash(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        ));
        // too short and too long
        assert!(!is_symbol_signature_hash("sha256:0000"));
        assert!(!is_symbol_signature_hash(&format!("{digest}0")));
        // upper-case hex is not the case the contract defines
        assert!(!is_symbol_signature_hash(&digest.to_uppercase()));
        // a character that is not a hex character
        assert!(!is_symbol_signature_hash(&format!(
            "sha256:{}",
            "z".repeat(64)
        )));
    }
}
