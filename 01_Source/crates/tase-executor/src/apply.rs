//! The apply: the constrained diff, handed to the host editor in one edit (detailed design §5.2 and §8.1).
//!
//! # What an apply is in this repository
//!
//! Detailed design §8.1 puts the `DocumentEditAdapter` between the executor and the `ide-kernel::Document`. This
//! repository has no `Document`: the text is written by the host editor and `pime-core` holds no buffer and no undo
//! stack (the mapping document §7-G1). The decision of that document is (a): **the apply is degraded to the
//! production of a constrained diff that the host applies in one `applyEdit`**.
//!
//! So an apply of this crate is three things and nothing else:
//!
//! * the **forward edits**: the constrained diff of [`crate::preview::diff_of`], in UTF-16 offsets of the baseline;
//! * the **inverse edits**: the same diff, turned around, in UTF-16 offsets of the new text, so that the change can
//!   be taken back;
//! * the **request to the host**: the path, the version and the hash the host must see, and the edits it must write
//!   in **one** editor edit.
//!
//! # The door to the host: `DocumentEditAdapter`
//!
//! The trait of this module is the `DocumentEditAdapter` of detailed design §8.1. The host answers two questions:
//! what does the document look like right now, and did you write the edits I gave you.
//!
//! The four-step protocol of `pime-core::edit::transaction::Coordinator` (`create_plan` → `prepare` → `begin_apply` →
//! `finish_apply`) is the model of that door, and the two fit together like this:
//!
//! | step of the coordinator | what the executor does |
//! |---|---|
//! | `create_plan` | the executor builds the [`DocumentEdit`]: the edits, the baseline hash and the hash of the result |
//! | `prepare` | the host checks the confirmation, the grant and the validation, and the executor checks that the document is still the baseline ([`apply_to_host`]) |
//! | `begin_apply` | the coordinator hands the edits to the bridge; the bridge calls [`DocumentEditAdapter::apply_edits`] with the very same edits |
//! | `finish_apply` | the coordinator verifies the read-back; the executor does the same check ([`apply_to_host`]) and answers `Unknown` when the read-back does not match |
//!
//! The two are complements, not duplicates: the coordinator owns the token, the deadline and the state of a plan, and
//! the executor owns the diff, the inverse and the rollback. A host that already runs a coordinator may use the
//! executor as the source of the edits; a host that does not may use the executor alone.
//!
//! # The atomicity of a plan
//!
//! A plan may touch several files, and the editor of the host writes one file at a time. The rule of detailed design
//! §8.1 is that a plan is **one logical undo transaction** and that a half-finished plan never looks like a success:
//!
//! * the executor stops at the first document the host refuses and answers `Rejected`; it never answers `Applied` for
//!   a plan whose documents are only half written;
//! * the outcome of an apply always names the documents that were written, so the caller can roll them back with
//!   [`rollback`], which writes the inverse edits and checks the read-back;
//! * the inverse of every document travels together in one [`crate::undo::UndoTransaction`], so the user undoes the
//!   whole plan in one gesture.

use pime_core::textrange::Edit;
use tase_contract::api::{Operation, PlanRequest};
use tase_contract::TaseError;
use tase_registry::{Policy, Registry};

use crate::preview::diff_of;
use crate::undo::inverse_edits_of;
use crate::workspace::validate_repo_relative_path;
use crate::Result;

/// The edits of one document of an apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentEdit {
    /// The path of the file, relative to the root of the repository.
    pub path: String,
    /// The version of the document the edits were computed against.
    pub base_version: i64,
    /// The SHA-256 of the text the edits were computed against.
    pub base_sha256: String,
    /// The SHA-256 of the text the edits produce.
    pub new_sha256: String,
    /// The edits of the change, in UTF-16 offsets of the baseline.
    pub edits: Vec<Edit>,
    /// The edits that take the change back, in UTF-16 offsets of the new text.
    pub inverse_edits: Vec<Edit>,
}

/// The whole apply: the documents of the plan and the base it runs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyPlan {
    /// The commit the plan starts from.
    pub base_commit: String,
    /// The digest of the plan that is applied.
    pub plan_digest: String,
    /// The documents of the plan, in the order of the operations.
    pub documents: Vec<DocumentEdit>,
}

impl ApplyPlan {
    /// The plan of an apply.
    pub fn new(base_commit: &str, plan_digest: &str, documents: Vec<DocumentEdit>) -> ApplyPlan {
        ApplyPlan {
            base_commit: base_commit.to_string(),
            plan_digest: plan_digest.to_string(),
            documents,
        }
    }

    /// The document of a path.
    pub fn document(&self, path: &str) -> Option<&DocumentEdit> {
        self.documents.iter().find(|d| d.path == path)
    }
}

/// What the host is asked to do: one editor edit, with the state the document must be in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEditRequest {
    /// The path of the file.
    pub path: String,
    /// The version the document must have.
    pub expected_version: i64,
    /// The hash the document must have.
    pub expected_sha256: String,
    /// The edits to write, in UTF-16 offsets of the text the host has.
    pub edits: Vec<Edit>,
    /// The hash the document will have after the write.
    pub new_sha256: String,
}

/// What the host answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostEditOutcome {
    /// The `applyEdit` resolved with a truthy value.
    Accepted,
    /// The `applyEdit` resolved with `false`: the editor refused the edit.
    Rejected,
    /// The `applyEdit` threw, or the bridge gave up waiting for it.
    Failed,
}

/// The door to the host editor: the `DocumentEditAdapter` of detailed design §8.1.
///
/// The executor never writes a file and never keeps a buffer: it asks the host for the state of a document, and it
/// hands the edits to the host, which writes them in **one** editor edit.
pub trait DocumentEditAdapter {
    /// The state of a document, as the host sees it right now.
    fn snapshot(&self, path: &str) -> Result<crate::workspace::DocumentSnapshot>;

    /// The host writes the edits in one editor edit and answers what happened.
    fn apply_edits(&mut self, request: &HostEditRequest) -> Result<HostEditOutcome>;
}

/// The result of an apply that went through the host.
///
/// Every outcome names the documents that were written, so that a caller can roll exactly those back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Every document was written and the read-back matched the expected hash.
    Applied { written: Vec<String> },
    /// The host refused the edit; the document was not written.
    Rejected {
        /// The document the host refused.
        path: String,
        /// The documents that were written before the refusal.
        written: Vec<String>,
    },
    /// The result of the write is unknown: the host did not confirm it.
    Unknown {
        /// The document whose result is unknown.
        path: String,
        /// The documents that were written before the unknown write.
        written: Vec<String>,
    },
    /// The document changed between the preview and the apply: nothing was written for it.
    Stale {
        /// The document that changed.
        path: String,
        /// The documents that were written before the change was seen.
        written: Vec<String>,
    },
}

impl ApplyOutcome {
    /// The documents that were written.
    pub fn written(&self) -> &[String] {
        match self {
            ApplyOutcome::Applied { written }
            | ApplyOutcome::Rejected { written, .. }
            | ApplyOutcome::Unknown { written, .. }
            | ApplyOutcome::Stale { written, .. } => written,
        }
    }

    /// Whether every document of the plan was written.
    pub fn is_applied(&self) -> bool {
        matches!(self, ApplyOutcome::Applied { .. })
    }
}

/// The forward edits and the inverse edits of one document.
///
/// The forward edits are the constrained diff of the crate, in UTF-16 offsets of the baseline. The inverse edits are
/// the same diff turned around, in UTF-16 offsets of the new text.
pub fn edits_of(base: &str, new: &str) -> Result<(Vec<Edit>, Vec<Edit>)> {
    let hunks = diff_of(base, new);
    let ids: Vec<String> = hunks.iter().map(|h| h.id.clone()).collect();
    let edits = pime_core::edit::diff::hunk_edits(&hunks, &ids).map_err(crate::from_core)?;
    let inverse = inverse_edits_of(&hunks);
    Ok((edits, inverse))
}

/// The edits of one document of an apply.
pub fn document_of(path: &str, base_version: i64, base: &str, new: &str) -> Result<DocumentEdit> {
    validate_repo_relative_path(path)?;
    let (edits, inverse_edits) = edits_of(base, new)?;
    Ok(DocumentEdit {
        path: path.to_string(),
        base_version,
        base_sha256: pime_core::sha256::sha256_hex(base),
        new_sha256: pime_core::sha256::sha256_hex(new),
        edits,
        inverse_edits,
    })
}

/// Hands the edits of the plan to the host, one document at a time.
///
/// The function stops at the first document that is not written: the outcome names the documents that were written
/// before the stop, and the caller may roll them back with [`rollback`]. The read-back of every document is checked:
/// a host that says "accepted" but writes another text gives an `Unknown`, never an `Applied`.
pub fn apply_to_host(adapter: &mut dyn DocumentEditAdapter, plan: &ApplyPlan) -> Result<ApplyOutcome> {
    let mut written: Vec<String> = Vec::new();
    for document in &plan.documents {
        let snapshot = adapter.snapshot(&document.path)?;
        if snapshot.sha256 != document.base_sha256 {
            return Ok(ApplyOutcome::Stale {
                path: document.path.clone(),
                written,
            });
        }
        let request = HostEditRequest {
            path: document.path.clone(),
            expected_version: snapshot.version,
            expected_sha256: document.base_sha256.clone(),
            edits: document.edits.clone(),
            new_sha256: document.new_sha256.clone(),
        };
        match adapter.apply_edits(&request)? {
            HostEditOutcome::Accepted => {
                let after = adapter.snapshot(&document.path)?;
                if after.sha256 != document.new_sha256 {
                    return Ok(ApplyOutcome::Unknown {
                        path: document.path.clone(),
                        written,
                    });
                }
                written.push(document.path.clone());
            }
            HostEditOutcome::Rejected => {
                return Ok(ApplyOutcome::Rejected {
                    path: document.path.clone(),
                    written,
                });
            }
            HostEditOutcome::Failed => {
                return Ok(ApplyOutcome::Unknown {
                    path: document.path.clone(),
                    written,
                });
            }
        }
    }
    Ok(ApplyOutcome::Applied { written })
}

/// Rolls the documents that were written back to the text of the base.
///
/// The documents are taken back in the reverse order they were written. A document that changed after the apply is
/// not touched: the rollback of a document the user changed would throw away the change of the user.
pub fn rollback(
    adapter: &mut dyn DocumentEditAdapter,
    plan: &ApplyPlan,
    written: &[String],
) -> Result<usize> {
    let mut count = 0;
    for path in written.iter().rev() {
        let Some(document) = plan.document(path) else {
            continue;
        };
        let snapshot = adapter.snapshot(path)?;
        if snapshot.sha256 != document.new_sha256 {
            return Err(TaseError::stale_base(format!(
                "the document {path} changed after the apply; it is not rolled back"
            )));
        }
        let request = HostEditRequest {
            path: document.path.clone(),
            expected_version: snapshot.version,
            expected_sha256: document.new_sha256.clone(),
            edits: document.inverse_edits.clone(),
            new_sha256: document.base_sha256.clone(),
        };
        match adapter.apply_edits(&request)? {
            HostEditOutcome::Accepted => {
                let after = adapter.snapshot(path)?;
                if after.sha256 != document.base_sha256 {
                    return Err(TaseError::internal(format!(
                        "the read-back of the rollback of {path} is not the text of the base"
                    )));
                }
                count += 1;
            }
            HostEditOutcome::Rejected => {
                return Err(TaseError::internal(format!(
                    "the host refused the rollback of {path}"
                )));
            }
            HostEditOutcome::Failed => {
                return Err(TaseError::internal(format!(
                    "the rollback of {path} failed"
                )));
            }
        }
    }
    Ok(count)
}

/// Checks that the plan the caller sends is the plan it expects.
///
/// The digest of a plan is the digest of its canonical JSON, and the canonical JSON comes from `tase-planner`: the
/// executor does not compute a second digest, it compares the one of the planner with the one the caller carries.
pub fn verify_plan_digest(request: &PlanRequest, expected: &str) -> Result<()> {
    let digest = tase_planner::digest_of(request);
    if digest != expected {
        return Err(TaseError::invalid_argument(format!(
            "the plan digest is {digest}, not {expected}"
        )));
    }
    Ok(())
}

/// Checks the preconditions of an operation against the state of the repository.
///
/// The file precondition is the digest of the blob of the file: a file that is gone and a file whose content moved
/// are the same failure. The existence of the symbol is checked when the caller knows it; the caller that does not
/// know it passes `None` and no verdict is given.
pub fn verify_preconditions(
    operation: &Operation,
    current_file_sha256: Option<&str>,
    symbol_exists: Option<bool>,
) -> Result<()> {
    let Some(preconditions) = &operation.preconditions else {
        return Ok(());
    };
    if let Some(expected) = &preconditions.file_blob_sha {
        match current_file_sha256 {
            None => {
                return Err(TaseError::stale_base(format!(
                    "the file {} is not there any more",
                    operation.target.path
                )));
            }
            Some(current) if current != expected => {
                return Err(TaseError::stale_base(format!(
                    "the file {} changed since the plan was made",
                    operation.target.path
                )));
            }
            Some(_) => {}
        }
    }
    if let Some(expected) = preconditions.expected_symbol_exists
        && let Some(exists) = symbol_exists
        && exists != expected
    {
        let state = if exists { "is there" } else { "is gone" };
        return Err(TaseError::symbol_conflict(format!(
            "the symbol {} of {} {state}, the plan expected the opposite",
            operation.target.symbol.as_deref().unwrap_or(""),
            operation.target.path
        )));
    }
    Ok(())
}

/// Checks that the templates the plan pins are still the ones of the catalog.
///
/// The pin of a template is its id, its version and the digest of its package (the `TemplateRef` of the contract
/// layer). A template that is not in the catalog any more is refused by the policy; a template whose version or
/// whose digest moved is a stale base: the plan was made against another package.
pub fn verify_template_pins(registry: &Registry, request: &PlanRequest) -> Result<()> {
    for operation in &request.operations {
        let Some(template) = &operation.template else {
            continue;
        };
        let id = template.id.as_str();
        let Some(entry) = registry.get(id) else {
            return Err(TaseError::policy_rejected(format!(
                "the template {id} is not in the catalog"
            )));
        };
        if entry.version != template.version {
            return Err(TaseError::stale_base(format!(
                "the template {id} is at the version {}, the plan pins {}",
                entry.version, template.version
            )));
        }
        if entry.digest != template.digest {
            return Err(TaseError::stale_base(format!(
                "the template {id} has another digest than the one the plan pins"
            )));
        }
    }
    Ok(())
}

/// Checks that the assets the plan instantiates may be instantiated.
///
/// The policy of the registry decides: a knowledge card is not a certified implementation, and an asset without an
/// approved implementation of the language of the change may not be instantiated. The check is separate from the
/// check of the pin, because the policy is the business of the caller and the pin is the business of the plan.
pub fn verify_template_policy(
    registry: &Registry,
    policy: &Policy,
    request: &PlanRequest,
    language: &str,
) -> Result<()> {
    for operation in &request.operations {
        let Some(template) = &operation.template else {
            continue;
        };
        let Some(entry) = registry.get(template.id.as_str()) else {
            continue;
        };
        policy.may_instantiate(entry, language)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::{ChangeKind, FileChange, Preview};
    use crate::undo::UndoTransaction;
    use crate::workspace::DocumentSnapshot;
    use std::collections::BTreeMap;
    use std::path::Path;
    use tase_contract::ErrorCode;
    use tase_contract::api::{OperationKind, Preconditions, Target};
    use tase_contract::OperationId;

    const BASE: &str = "fn main() {\n    println!(\"hello\");\n}\n";
    const NEW: &str = "fn main() {\n    println!(\"hello, world\");\n}\n";

    /// A host that keeps its documents in the memory.
    struct MemoryHost {
        documents: BTreeMap<String, (i64, String)>,
        refuse: bool,
        break_read_back: bool,
    }

    impl MemoryHost {
        fn new() -> MemoryHost {
            MemoryHost {
                documents: BTreeMap::new(),
                refuse: false,
                break_read_back: false,
            }
        }

        fn with(path: &str, version: i64, text: &str) -> MemoryHost {
            let mut host = MemoryHost::new();
            host.documents
                .insert(path.to_string(), (version, text.to_string()));
            host
        }

        fn text(&self, path: &str) -> String {
            self.documents
                .get(path)
                .map(|(_, text)| text.clone())
                .unwrap_or_default()
        }
    }

    impl DocumentEditAdapter for MemoryHost {
        fn snapshot(&self, path: &str) -> Result<DocumentSnapshot> {
            match self.documents.get(path) {
                Some((version, text)) => {
                    Ok(DocumentSnapshot::new(path, *version, text).with_dirty(true))
                }
                None => Err(TaseError::not_found(format!("there is no document {path}"))),
            }
        }

        fn apply_edits(&mut self, request: &HostEditRequest) -> Result<HostEditOutcome> {
            if self.refuse {
                return Ok(HostEditOutcome::Rejected);
            }
            let Some((version, text)) = self.documents.get(&request.path).cloned() else {
                return Ok(HostEditOutcome::Failed);
            };
            if request.expected_sha256 != pime_core::sha256::sha256_hex(&text) {
                return Ok(HostEditOutcome::Rejected);
            }
            let new_text = pime_core::textrange::apply_edits(&text, &request.edits)
                .map_err(crate::from_core)?;
            let new_text = if self.break_read_back {
                "the host wrote something else".to_string()
            } else {
                new_text
            };
            self.documents
                .insert(request.path.clone(), (version + 1, new_text));
            Ok(HostEditOutcome::Accepted)
        }
    }

    fn plan_of() -> ApplyPlan {
        ApplyPlan::new(
            "commit-1",
            "sha256:plan",
            vec![document_of("src/main.rs", 1, BASE, NEW).expect("the document is built")],
        )
    }

    /// A plan of one operation, without a template.
    fn plan_request() -> PlanRequest {
        PlanRequest {
            contract_version: tase_contract::CONTRACT_VERSION.to_string(),
            repository_id: tase_contract::RepositoryId::parse("repo_ide1").expect("the id is legal"),
            target_ref: "refs/heads/main".to_string(),
            base_commit: "commit-1".to_string(),
            strategy: "semantic".to_string(),
            operations: vec![Operation {
                op_id: OperationId::parse("op_001").expect("the id is legal"),
                kind: OperationKind::ReplaceSymbolBody,
                target: Target {
                    path: "src/main.rs".to_string(),
                    symbol: Some("main".to_string()),
                    symbol_kind: Some("function".to_string()),
                },
                template: None,
                params: Vec::new(),
                preconditions: None,
            }],
            validation_profile: "rust".to_string(),
            approval_policy: "default".to_string(),
        }
    }

    /// A plan of one operation that instantiates a template.
    fn plan_request_with_template(id: &str, version: &str, digest: &str) -> PlanRequest {
        let mut request = plan_request();
        request.operations[0].kind = OperationKind::InstantiateTemplate;
        request.operations[0].template = Some(tase_contract::api::TemplateRef {
            id: tase_contract::TemplateId::parse(id).expect("the id is legal"),
            version: version.to_string(),
            digest: digest.to_string(),
        });
        request
    }

    #[test]
    fn the_edits_of_the_apply_are_the_diff_of_the_crate() {
        let (edits, inverse) = edits_of(BASE, NEW).expect("the edits are built");
        let hunks = diff_of(BASE, NEW);
        assert_eq!(edits.len(), hunks.len());
        assert_eq!(edits[0].start, hunks[0].start_offset);
        assert_eq!(edits[0].end, hunks[0].end_offset);
        assert_eq!(edits[0].text, hunks[0].new_text);
        // the inverse takes the change back
        let applied = pime_core::textrange::apply_edits(BASE, &edits).expect("the edits apply");
        assert_eq!(applied, NEW);
        let restored = pime_core::textrange::apply_edits(&applied, &inverse).expect("the inverse applies");
        assert_eq!(restored, BASE);
    }

    #[test]
    fn the_apply_hands_the_edits_to_the_host_and_checks_the_read_back() {
        let mut host = MemoryHost::with("src/main.rs", 1, BASE);
        let plan = plan_of();
        let outcome = apply_to_host(&mut host, &plan).expect("the apply runs");
        assert!(outcome.is_applied());
        assert_eq!(outcome.written(), ["src/main.rs"]);
        assert_eq!(host.text("src/main.rs"), NEW);
    }

    #[test]
    fn a_document_that_changed_after_the_preview_is_not_written() {
        let mut host = MemoryHost::with("src/main.rs", 1, "fn main() { /* changed */ }\n");
        let plan = plan_of();
        let outcome = apply_to_host(&mut host, &plan).expect("the apply runs");
        assert!(matches!(outcome, ApplyOutcome::Stale { .. }));
        assert!(outcome.written().is_empty());
        assert_eq!(host.text("src/main.rs"), "fn main() { /* changed */ }\n");
    }

    #[test]
    fn a_host_that_refuses_leaves_the_document_alone_and_names_it() {
        let mut host = MemoryHost::with("src/main.rs", 1, BASE);
        host.refuse = true;
        let plan = plan_of();
        let outcome = apply_to_host(&mut host, &plan).expect("the apply runs");
        match outcome {
            ApplyOutcome::Rejected { path, written } => {
                assert_eq!(path, "src/main.rs");
                assert!(written.is_empty());
            }
            other => panic!("the outcome is {other:?}"),
        }
        assert_eq!(host.text("src/main.rs"), BASE);
    }

    #[test]
    fn the_rollback_takes_the_documents_back_to_the_base() {
        let mut host = MemoryHost::with("src/main.rs", 1, BASE);
        let plan = plan_of();
        let outcome = apply_to_host(&mut host, &plan).expect("the apply runs");
        assert!(outcome.is_applied());
        let count = rollback(&mut host, &plan, outcome.written()).expect("the rollback runs");
        assert_eq!(count, 1);
        assert_eq!(host.text("src/main.rs"), BASE);
    }

    #[test]
    fn a_document_that_changed_after_the_apply_is_not_rolled_back() {
        let mut host = MemoryHost::with("src/main.rs", 1, BASE);
        let plan = plan_of();
        let outcome = apply_to_host(&mut host, &plan).expect("the apply runs");
        // the user types something after the apply
        host.documents
            .insert("src/main.rs".to_string(), (2, "the user typed".to_string()));
        let error = rollback(&mut host, &plan, outcome.written()).unwrap_err();
        assert_eq!(error.code, ErrorCode::StaleBase);
        assert_eq!(host.text("src/main.rs"), "the user typed");
    }

    #[test]
    fn the_read_back_that_does_not_match_is_an_unknown_and_not_a_success() {
        let mut host = MemoryHost::with("src/main.rs", 1, BASE);
        host.break_read_back = true;
        let plan = plan_of();
        let outcome = apply_to_host(&mut host, &plan).expect("the apply runs");
        assert!(!outcome.is_applied());
        match outcome {
            ApplyOutcome::Unknown { path, written } => {
                assert_eq!(path, "src/main.rs");
                assert!(written.is_empty());
            }
            other => panic!("the outcome is {other:?}"),
        }
    }

    #[test]
    fn the_preconditions_of_an_operation_are_checked() {
        let operation = Operation {
            op_id: OperationId::parse("op_001").expect("the id is legal"),
            kind: OperationKind::ReplaceSymbolBody,
            target: Target {
                path: "src/main.rs".to_string(),
                symbol: Some("main".to_string()),
                symbol_kind: Some("function".to_string()),
            },
            template: None,
            params: Vec::new(),
            preconditions: Some(Preconditions {
                file_blob_sha: Some("sha256:a".to_string()),
                symbol_signature_hash: None,
                expected_symbol_exists: Some(true),
            }),
        };
        // the file is the one the plan pinned
        verify_preconditions(&operation, Some("sha256:a"), Some(true)).expect("the preconditions hold");
        // the file moved
        let error = verify_preconditions(&operation, Some("sha256:b"), Some(true)).unwrap_err();
        assert_eq!(error.code, ErrorCode::StaleBase);
        // the file is gone
        let error = verify_preconditions(&operation, None, Some(true)).unwrap_err();
        assert_eq!(error.code, ErrorCode::StaleBase);
        // the symbol is gone
        let error = verify_preconditions(&operation, Some("sha256:a"), Some(false)).unwrap_err();
        assert_eq!(error.code, ErrorCode::SymbolConflict);
    }

    #[test]
    fn an_operation_without_preconditions_passes() {
        let operation = Operation {
            op_id: OperationId::parse("op_001").expect("the id is legal"),
            kind: OperationKind::Format,
            target: Target {
                path: "src/main.rs".to_string(),
                symbol: None,
                symbol_kind: None,
            },
            template: None,
            params: Vec::new(),
            preconditions: None,
        };
        verify_preconditions(&operation, None, None).expect("there is nothing to check");
    }

    #[test]
    fn the_diff_of_the_preview_is_the_diff_of_the_apply() {
        let change = FileChange::new("src/main.rs", ChangeKind::Modified, BASE, NEW, &[])
            .expect("the change is built");
        let preview = Preview::build("commit-1", "sha256:plan", vec![change]);
        let (edits, _) = edits_of(BASE, NEW).expect("the edits are built");

        // the edits of the apply are the hunks of the preview, one by one
        let hunks = &preview.changes[0].hunks;
        assert_eq!(edits.len(), hunks.len());
        for (edit, hunk) in edits.iter().zip(hunks.iter()) {
            assert_eq!(edit.start, hunk.start_offset);
            assert_eq!(edit.end, hunk.end_offset);
            assert_eq!(edit.text, hunk.new_text);
        }
        // and the diff the preview shows is the diff of the apply
        let applied = pime_core::textrange::apply_edits(BASE, &edits).expect("the edits apply");
        assert_eq!(applied, NEW);
        assert!(preview.diff_text.contains("+    println!(\"hello, world\");"));
    }

    #[test]
    fn the_inverse_of_the_apply_takes_the_change_back() {
        let (edits, inverse) = edits_of(BASE, NEW).expect("the edits are built");
        let applied = pime_core::textrange::apply_edits(BASE, &edits).expect("the edits apply");
        assert_eq!(applied, NEW);
        let restored = pime_core::textrange::apply_edits(&applied, &inverse).expect("the inverse applies");
        assert_eq!(restored, BASE);
    }

    #[test]
    fn the_undo_transaction_is_applied_by_the_host_in_one_edit() {
        let mut host = MemoryHost::with("src/main.rs", 1, BASE);
        let plan = plan_of();
        let outcome = apply_to_host(&mut host, &plan).expect("the apply runs");
        assert!(outcome.is_applied());
        assert_eq!(host.text("src/main.rs"), NEW);

        // the host undoes the plan in one gesture: one transaction, one edit for the document
        let transaction = UndoTransaction::of(&plan);
        let document = transaction.document("src/main.rs").expect("the document is there");
        let applied = plan.document("src/main.rs").expect("the document is there");
        let request = HostEditRequest {
            path: document.path.clone(),
            expected_version: 2,
            expected_sha256: document.expected_sha256.clone(),
            edits: document.edits.clone(),
            new_sha256: applied.base_sha256.clone(),
        };
        assert_eq!(
            host.apply_edits(&request).expect("the host answers"),
            HostEditOutcome::Accepted
        );
        assert_eq!(host.text("src/main.rs"), BASE);
    }

    #[test]
    fn the_plan_the_caller_sends_is_the_plan_it_expects() {
        let request = plan_request();
        let digest = tase_planner::digest_of(&request);
        verify_plan_digest(&request, &digest).expect("the digest is the digest of the plan");
        let error = verify_plan_digest(&request, "sha256:0000").unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_pins_of_the_templates_are_checked_against_the_catalog() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog");
        if !dir.exists() {
            eprintln!("skipped: the catalog of the repository is not there");
            return;
        }
        let registry = Registry::load(&dir).expect("the catalog of the repository is valid");
        let entry = registry
            .get("algorithm.graph.kruskal")
            .expect("the asset is there");

        // a plan that pins the template of the catalog
        let pinned = plan_request_with_template(&entry.id, &entry.version, &entry.digest);
        verify_template_pins(&registry, &pinned).expect("the pin is the pin of the catalog");

        // a plan that pins another version
        let other_version = plan_request_with_template(&entry.id, "9.9.9", &entry.digest);
        let error = verify_template_pins(&registry, &other_version).unwrap_err();
        assert_eq!(error.code, ErrorCode::StaleBase);

        // a plan that pins another digest
        let other_digest = plan_request_with_template(&entry.id, &entry.version, "sha256:0000");
        let error = verify_template_pins(&registry, &other_digest).unwrap_err();
        assert_eq!(error.code, ErrorCode::StaleBase);

        // a plan that pins a template the catalog does not hold
        let unknown = plan_request_with_template("algorithm.graph.nothing", "1.0.0", "sha256:0000");
        let error = verify_template_pins(&registry, &unknown).unwrap_err();
        assert_eq!(error.code, ErrorCode::PolicyRejected);
    }

    #[test]
    fn a_knowledge_card_may_not_be_instantiated() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tase-catalog");
        if !dir.exists() {
            eprintln!("skipped: the catalog of the repository is not there");
            return;
        }
        let registry = Registry::load(&dir).expect("the catalog of the repository is valid");
        let entry = registry
            .get("algorithm.graph.kruskal")
            .expect("the asset is there");
        let pinned = plan_request_with_template(&entry.id, &entry.version, &entry.digest);
        // the catalog of this repository holds knowledge cards and no certified implementation
        let error = verify_template_policy(&registry, &Policy::default(), &pinned, "rust").unwrap_err();
        assert_eq!(error.code, ErrorCode::PolicyRejected);
        assert!(error.message.contains("knowledge card"));
    }
}
