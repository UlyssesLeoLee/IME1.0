//! The conflicts of an apply: what another agent did that makes the plan unsafe (detailed design §6.2, §5.3, §5.4).
//!
//! Detailed design §6.2 says it in one line: two operations may conflict when they touch the same AST symbol, the
//! same range of lines, the same group of imports, the same public signature, the same set of cross-file
//! declarations and references, or when the automatic formatting makes their write sets overlap. The conflict is not
//! a verdict of this crate: the crate **names** the conflict, and the caller decides what to do with it (rebase,
//! ask a human, or refuse).
//!
//! Two rules hold for every conflict of this module:
//!
//! * a conflict is **never** resolved by overwriting: the only outcomes are "the plan may run again on the new
//!   head" or "the plan may not run";
//! * a conflict carries the path and, when the conflict is about a symbol, the symbol: the message of the design
//!   book is never enough to place a conflict in a repository.

use tase_contract::{ErrorCode, TaseError};

/// What kind of conflict was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    /// The base of the plan is not in the history of the new head.
    BaseNotAncestor,
    /// The head of the branch moved under the plan.
    HeadMoved,
    /// The plan writes a whole file and the head changed that file.
    FileOverlap,
    /// The plan writes a symbol and the head changed the file of that symbol.
    SymbolOverlap,
    /// The symbol the plan writes is not there any more.
    SymbolMissing,
    /// The symbol the plan writes is not unique any more.
    SymbolAmbiguous,
    /// The signature of the symbol changed.
    SignatureChanged,
    /// The plan touches the imports of a file whose imports changed.
    ImportGroupOverlap,
    /// A precondition of the operation is not true any more.
    PreconditionFailed,
}

impl ConflictKind {
    /// The stable code of the conflict.
    pub fn code(self) -> &'static str {
        match self {
            ConflictKind::BaseNotAncestor => "CONFLICT_BASE_NOT_ANCESTOR",
            ConflictKind::HeadMoved => "CONFLICT_HEAD_MOVED",
            ConflictKind::FileOverlap => "CONFLICT_FILE_OVERLAP",
            ConflictKind::SymbolOverlap => "CONFLICT_SYMBOL_OVERLAP",
            ConflictKind::SymbolMissing => "CONFLICT_SYMBOL_MISSING",
            ConflictKind::SymbolAmbiguous => "CONFLICT_SYMBOL_AMBIGUOUS",
            ConflictKind::SignatureChanged => "CONFLICT_SIGNATURE_CHANGED",
            ConflictKind::ImportGroupOverlap => "CONFLICT_IMPORT_GROUP_OVERLAP",
            ConflictKind::PreconditionFailed => "CONFLICT_PRECONDITION_FAILED",
        }
    }

    /// The business code the conflict maps to.
    ///
    /// A head that moved is a stale base (`STALE_BASE`); everything else is a conflict of symbols
    /// (`SYMBOL_CONFLICT`).
    pub fn business_code(self) -> ErrorCode {
        match self {
            ConflictKind::BaseNotAncestor | ConflictKind::HeadMoved => ErrorCode::StaleBase,
            _ => ErrorCode::SymbolConflict,
        }
    }
}

/// One conflict: what another agent did, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// What kind of conflict this is.
    pub kind: ConflictKind,
    /// The path of the file the conflict is about, when it is about a file.
    pub path: String,
    /// The symbol the conflict is about, when it is about a symbol.
    pub symbol: Option<String>,
    /// What happened, in one readable line.
    pub message: String,
}

impl Conflict {
    /// A conflict.
    pub fn new(
        kind: ConflictKind,
        path: &str,
        symbol: Option<&str>,
        message: impl Into<String>,
    ) -> Conflict {
        Conflict {
            kind,
            path: path.to_string(),
            symbol: symbol.map(str::to_string),
            message: message.into(),
        }
    }

    /// The stable code of the conflict.
    pub fn code(&self) -> &'static str {
        self.kind.code()
    }

    /// The conflict as the failure of the contract layer.
    pub fn to_error(&self) -> TaseError {
        TaseError::new(self.kind.business_code(), self.message.clone()).with_details(self.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_its_own_code() {
        let kinds = [
            ConflictKind::BaseNotAncestor,
            ConflictKind::HeadMoved,
            ConflictKind::FileOverlap,
            ConflictKind::SymbolOverlap,
            ConflictKind::SymbolMissing,
            ConflictKind::SymbolAmbiguous,
            ConflictKind::SignatureChanged,
            ConflictKind::ImportGroupOverlap,
            ConflictKind::PreconditionFailed,
        ];
        let mut codes: Vec<&str> = kinds.iter().map(|k| k.code()).collect();
        codes.sort_unstable();
        let count = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), count, "two kinds share a code");
    }

    #[test]
    fn a_head_that_moved_is_a_stale_base_and_the_rest_are_symbol_conflicts() {
        assert_eq!(
            ConflictKind::HeadMoved.business_code(),
            ErrorCode::StaleBase
        );
        assert_eq!(
            ConflictKind::BaseNotAncestor.business_code(),
            ErrorCode::StaleBase
        );
        assert_eq!(
            ConflictKind::FileOverlap.business_code(),
            ErrorCode::SymbolConflict
        );
        assert_eq!(
            ConflictKind::SymbolMissing.business_code(),
            ErrorCode::SymbolConflict
        );
    }

    #[test]
    fn a_conflict_carries_the_path_the_symbol_and_the_code() {
        let conflict = Conflict::new(
            ConflictKind::SymbolOverlap,
            "src/main.rs",
            Some("main"),
            "the symbol main changed at the new head",
        );
        assert_eq!(conflict.path, "src/main.rs");
        assert_eq!(conflict.symbol.as_deref(), Some("main"));
        assert_eq!(conflict.code(), "CONFLICT_SYMBOL_OVERLAP");
        let error = conflict.to_error();
        assert_eq!(error.code, ErrorCode::SymbolConflict);
        assert_eq!(error.details.as_deref(), Some("CONFLICT_SYMBOL_OVERLAP"));
        assert!(error.message.contains("main"));
    }
}
