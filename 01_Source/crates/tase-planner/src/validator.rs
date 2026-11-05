//! The validation of a plan (detailed design §2.3).
//!
//! The plan service of detailed design §2.3 runs a pipeline of nine steps: identity, schema, the permission of the
//! repository and of the branch, the content addresses, the pinning of the template, the static checks, the order of
//! the operations (the DAG), the canonical JSON, the SHA-256 digest of the plan and the save of the `plan_id`.
//!
//! This module holds the part of that pipeline that is **pure**: it reads the request and it tells what is wrong with
//! it. It never touches the disk, never talks to the registry and never asks the semantic layer: the checks that need
//! the repository (does the file exist? does the symbol exist? is the template approved?) belong to the steps that
//! come before or after, not to this module.
//!
//! Every finding is a [`ValidationIssue`] with a stable code, a message and the id of the operation it belongs to (when
//! it belongs to one). A plan is valid when it has no finding at all.

use tase_contract::api::{OperationKind, PlanRequest};

/// The code of a plan without any operation.
pub const CODE_OPERATIONS_EMPTY: &str = "PLAN_OPERATIONS_EMPTY";
/// The code of two operations that share their id.
pub const CODE_OP_ID_DUPLICATE: &str = "OP_ID_DUPLICATE";
/// The code of a template operation without a template.
pub const CODE_TEMPLATE_MISSING: &str = "TEMPLATE_MISSING";
/// The code of an operation that carries a template it does not use.
pub const CODE_TEMPLATE_UNEXPECTED: &str = "TEMPLATE_UNEXPECTED";
/// The code of a template whose digest is not a whole SHA-256.
pub const CODE_TEMPLATE_DIGEST_INVALID: &str = "TEMPLATE_DIGEST_INVALID";
/// The code of a target whose path is not relative to the root of the repository.
pub const CODE_TARGET_PATH_NOT_REPO_RELATIVE: &str = "TARGET_PATH_NOT_REPO_RELATIVE";
/// The code of a precondition that expects a symbol that is not named.
pub const CODE_PRECONDITION_SYMBOL_UNNAMED: &str = "PRECONDITION_SYMBOL_UNNAMED";
/// The code of a precondition that expects an existing file without pinning its blob.
pub const CODE_PRECONDITION_FILE_BLOB_MISSING: &str = "PRECONDITION_FILE_BLOB_MISSING";
/// The code of a base commit that is not a whole SHA-1.
pub const CODE_BASE_COMMIT_INVALID: &str = "BASE_COMMIT_INVALID";

/// Every code of this module.
pub const ALL_CODES: [&str; 9] = [
    CODE_OPERATIONS_EMPTY,
    CODE_OP_ID_DUPLICATE,
    CODE_TEMPLATE_MISSING,
    CODE_TEMPLATE_UNEXPECTED,
    CODE_TEMPLATE_DIGEST_INVALID,
    CODE_TARGET_PATH_NOT_REPO_RELATIVE,
    CODE_PRECONDITION_SYMBOL_UNNAMED,
    CODE_PRECONDITION_FILE_BLOB_MISSING,
    CODE_BASE_COMMIT_INVALID,
];

/// One finding of the validation of a plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationIssue {
    /// The stable code of the finding.
    pub code: &'static str,
    /// What is wrong, in one readable line.
    pub message: String,
    /// The id of the operation the finding belongs to, when it belongs to one.
    pub op_id: Option<String>,
}

/// The findings of a plan.
///
/// The findings come in the order of the pipeline: the plan itself first, then the operations in the order they are
/// written.
pub fn validate_plan(request: &PlanRequest) -> Vec<ValidationIssue> {
    let mut issues: Vec<ValidationIssue> = Vec::new();

    // The plan has to hold at least one operation.
    if request.operations.is_empty() {
        issues.push(ValidationIssue {
            code: CODE_OPERATIONS_EMPTY,
            message: "the plan holds no operation".to_string(),
            op_id: None,
        });
    }

    // The base of the plan is a whole commit, not a prefix of one.
    if !is_full_commit(&request.base_commit) {
        issues.push(ValidationIssue {
            code: CODE_BASE_COMMIT_INVALID,
            message: format!(
                "the base commit {:?} is not a whole SHA-1 of 40 hex digits",
                request.base_commit
            ),
            op_id: None,
        });
    }

    // The id of an operation is the id of the operation inside its plan.
    let mut seen: Vec<&str> = Vec::with_capacity(request.operations.len());
    for operation in &request.operations {
        let op_id = operation.op_id.as_str();
        if seen.contains(&op_id) {
            issues.push(ValidationIssue {
                code: CODE_OP_ID_DUPLICATE,
                message: format!("the operation id {op_id:?} is used twice in the plan"),
                op_id: Some(op_id.to_string()),
            });
        } else {
            seen.push(op_id);
        }
    }

    for operation in &request.operations {
        let op_id = operation.op_id.as_str().to_string();

        // Only the template operation carries a template.
        match (operation.kind, &operation.template) {
            (OperationKind::InstantiateTemplate, None) => issues.push(ValidationIssue {
                code: CODE_TEMPLATE_MISSING,
                message: "the operation instantiates a template but names none".to_string(),
                op_id: Some(op_id.clone()),
            }),
            (OperationKind::InstantiateTemplate, Some(template)) => {
                if !is_sha256_digest(&template.digest) {
                    issues.push(ValidationIssue {
                        code: CODE_TEMPLATE_DIGEST_INVALID,
                        message: format!(
                            "the digest {:?} of the template {} is not \"sha256:\" followed by 64 hex digits",
                            template.digest, template.id
                        ),
                        op_id: Some(op_id.clone()),
                    });
                }
            }
            (_, Some(template)) => issues.push(ValidationIssue {
                code: CODE_TEMPLATE_UNEXPECTED,
                message: format!(
                    "the operation of kind {} carries the template {} but does not instantiate it",
                    operation.kind.as_str(),
                    template.id
                ),
                op_id: Some(op_id.clone()),
            }),
            (_, None) => {}
        }

        // The path of a target is relative to the root of the repository.
        if !is_repo_relative(&operation.target.path) {
            issues.push(ValidationIssue {
                code: CODE_TARGET_PATH_NOT_REPO_RELATIVE,
                message: format!(
                    "the path {:?} is not relative to the root of the repository",
                    operation.target.path
                ),
                op_id: Some(op_id.clone()),
            });
        }

        // The preconditions of a symbol that has to exist.
        if let Some(preconditions) = &operation.preconditions
            && preconditions.expected_symbol_exists == Some(true)
        {
            // The shape of the request: an existing symbol is named, and an existing file is pinned.
            if operation.target.symbol.is_none() {
                issues.push(ValidationIssue {
                    code: CODE_PRECONDITION_SYMBOL_UNNAMED,
                    message: format!(
                        "the operation expects the symbol of {} to exist but names none",
                        operation.target.path
                    ),
                    op_id: Some(op_id.clone()),
                });
            }
            if preconditions.file_blob_sha.is_none() {
                issues.push(ValidationIssue {
                    code: CODE_PRECONDITION_FILE_BLOB_MISSING,
                    message: format!(
                        "the operation expects the symbol of {} to exist but does not pin the blob of the file",
                        operation.target.path
                    ),
                    op_id: Some(op_id),
                });
            }
        }
    }

    issues
}

/// Whether a plan has no finding at all.
pub fn is_valid(issues: &[ValidationIssue]) -> bool {
    issues.is_empty()
}

/// Whether a digest is a whole SHA-256: the prefix `sha256:` and 64 hex digits.
///
/// The sample of detailed design §2.3 writes `sha256:FULL64HEX`; the book says that the placeholders of the sample
/// have to be replaced by real digests, so a placeholder is refused here.
fn is_sha256_digest(digest: &str) -> bool {
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether a commit is a whole SHA-1: 40 hex digits.
fn is_full_commit(commit: &str) -> bool {
    commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether a path is relative to the root of the repository.
///
/// A path is refused when it is empty, when it starts with a separator, when it holds a backslash (the separator of
/// the paths of a repository is `/`), when it holds a colon (the usual reason is a drive letter) or when one of its
/// segments is `..`.
fn is_repo_relative(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return false;
    }
    if path.contains('\\') || path.contains(':') {
        return false;
    }
    !path.split('/').any(|segment| segment == "..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tase_contract::api::{Operation, Preconditions, Target, TemplateRef};
    use tase_contract::ids::{OperationId, RepositoryId, TemplateId};

    /// A whole SHA-1.
    const COMMIT: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";

    /// A whole SHA-256.
    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// An operation that instantiates a template.
    fn template_operation(op_id: &str) -> Operation {
        Operation {
            op_id: OperationId::parse(op_id).expect("a legal id"),
            kind: OperationKind::InstantiateTemplate,
            target: Target {
                path: "src/api/user.rs".to_string(),
                symbol: Some("crate::api::user::create_user".to_string()),
                symbol_kind: Some("function".to_string()),
            },
            template: Some(TemplateRef {
                id: TemplateId::parse("rust.handler.create").expect("a legal id"),
                version: "1.0.0".to_string(),
                digest: DIGEST.to_string(),
            }),
            params: vec![("entity".to_string(), "User".to_string())],
            preconditions: Some(Preconditions {
                file_blob_sha: Some("0".repeat(40)),
                symbol_signature_hash: None,
                expected_symbol_exists: Some(false),
            }),
        }
    }

    /// A plan that holds one valid operation.
    fn valid_plan() -> PlanRequest {
        PlanRequest {
            contract_version: "1.1.0".to_string(),
            repository_id: RepositoryId::parse("repo_ide1").expect("a legal id"),
            target_ref: "refs/heads/agent-feature".to_string(),
            base_commit: COMMIT.to_string(),
            strategy: "template".to_string(),
            operations: vec![template_operation("op_001")],
            validation_profile: "rust-ci-default".to_string(),
            approval_policy: "project-default".to_string(),
        }
    }

    /// The codes of the findings of a plan.
    fn codes(issues: &[ValidationIssue]) -> Vec<&'static str> {
        issues.iter().map(|issue| issue.code).collect()
    }

    #[test]
    fn a_valid_plan_has_no_finding() {
        let issues = validate_plan(&valid_plan());
        assert!(is_valid(&issues), "{issues:?}");
        assert!(issues.is_empty());
    }

    #[test]
    fn a_plan_without_an_operation_is_refused() {
        let mut plan = valid_plan();
        plan.operations.clear();
        let issues = validate_plan(&plan);
        assert!(codes(&issues).contains(&CODE_OPERATIONS_EMPTY));
        assert!(!is_valid(&issues));
    }

    #[test]
    fn two_operations_with_the_same_id_are_refused() {
        let mut plan = valid_plan();
        plan.operations.push(template_operation("op_001"));
        let issues = validate_plan(&plan);
        assert!(codes(&issues).contains(&CODE_OP_ID_DUPLICATE));
        let duplicate = issues
            .iter()
            .find(|issue| issue.code == CODE_OP_ID_DUPLICATE)
            .expect("the duplicate must be reported");
        assert_eq!(duplicate.op_id.as_deref(), Some("op_001"));
    }

    #[test]
    fn a_template_operation_without_a_template_is_refused() {
        let mut plan = valid_plan();
        plan.operations[0].template = None;
        let issues = validate_plan(&plan);
        assert!(codes(&issues).contains(&CODE_TEMPLATE_MISSING));
    }

    #[test]
    fn an_operation_that_carries_a_template_it_does_not_use_is_refused() {
        let mut plan = valid_plan();
        plan.operations[0].kind = OperationKind::Format;
        let issues = validate_plan(&plan);
        assert!(codes(&issues).contains(&CODE_TEMPLATE_UNEXPECTED));
    }

    #[test]
    fn a_digest_that_is_not_a_whole_sha256_is_refused() {
        let mut plan = valid_plan();
        plan.operations[0]
            .template
            .as_mut()
            .expect("a template")
            .digest = "sha256:FULL64HEX".to_string();
        let issues = validate_plan(&plan);
        assert!(codes(&issues).contains(&CODE_TEMPLATE_DIGEST_INVALID));
    }

    #[test]
    fn a_digest_without_the_prefix_is_refused() {
        let mut plan = valid_plan();
        plan.operations[0]
            .template
            .as_mut()
            .expect("a template")
            .digest =
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string();
        let issues = validate_plan(&plan);
        assert!(codes(&issues).contains(&CODE_TEMPLATE_DIGEST_INVALID));
    }

    #[test]
    fn a_digest_of_the_wrong_length_is_refused() {
        let mut plan = valid_plan();
        plan.operations[0]
            .template
            .as_mut()
            .expect("a template")
            .digest = "sha256:0123".to_string();
        let issues = validate_plan(&plan);
        assert!(codes(&issues).contains(&CODE_TEMPLATE_DIGEST_INVALID));
    }

    #[test]
    fn a_path_that_leaves_the_repository_is_refused() {
        for path in [
            "/etc/passwd",
            "C:\\src\\api\\user.rs",
            "../outside.rs",
            "src/../../outside.rs",
            "src/api/../user.rs",
            "",
            "src\\api\\user.rs",
        ] {
            let mut plan = valid_plan();
            plan.operations[0].target.path = path.to_string();
            let issues = validate_plan(&plan);
            assert!(
                codes(&issues).contains(&CODE_TARGET_PATH_NOT_REPO_RELATIVE),
                "the path {path:?} must be refused"
            );
        }
    }

    #[test]
    fn a_path_inside_the_repository_is_accepted() {
        for path in ["src/api/user.rs", "user.rs", "src/api/user_test.rs"] {
            let mut plan = valid_plan();
            plan.operations[0].target.path = path.to_string();
            let issues = validate_plan(&plan);
            assert!(
                !codes(&issues).contains(&CODE_TARGET_PATH_NOT_REPO_RELATIVE),
                "the path {path:?} must be accepted"
            );
        }
    }

    #[test]
    fn a_precondition_that_expects_a_symbol_it_does_not_name_is_refused() {
        let mut plan = valid_plan();
        plan.operations[0].target.symbol = None;
        plan.operations[0]
            .preconditions
            .as_mut()
            .expect("a precondition")
            .expected_symbol_exists = Some(true);
        let issues = validate_plan(&plan);
        assert!(codes(&issues).contains(&CODE_PRECONDITION_SYMBOL_UNNAMED));
    }

    #[test]
    fn a_precondition_that_expects_an_existing_file_without_a_blob_is_refused() {
        let mut plan = valid_plan();
        plan.operations[0]
            .preconditions
            .as_mut()
            .expect("a precondition")
            .expected_symbol_exists = Some(true);
        plan.operations[0]
            .preconditions
            .as_mut()
            .expect("a precondition")
            .file_blob_sha = None;
        let issues = validate_plan(&plan);
        assert!(codes(&issues).contains(&CODE_PRECONDITION_FILE_BLOB_MISSING));
    }

    #[test]
    fn a_new_file_may_expect_no_symbol_at_all() {
        let mut plan = valid_plan();
        plan.operations[0].preconditions = Some(Preconditions {
            file_blob_sha: None,
            symbol_signature_hash: None,
            expected_symbol_exists: Some(false),
        });
        let issues = validate_plan(&plan);
        assert!(is_valid(&issues), "{issues:?}");
    }

    #[test]
    fn a_base_commit_that_is_not_a_whole_sha1_is_refused() {
        for commit in ["a1b2c3d4fullsha", "a1b2c3", ""] {
            let mut plan = valid_plan();
            plan.base_commit = commit.to_string();
            let issues = validate_plan(&plan);
            assert!(
                codes(&issues).contains(&CODE_BASE_COMMIT_INVALID),
                "the commit {commit:?} must be refused"
            );
        }
    }

    #[test]
    fn the_finding_of_the_plan_itself_has_no_operation() {
        let mut plan = valid_plan();
        plan.base_commit = "short".to_string();
        let issues = validate_plan(&plan);
        let finding = issues
            .iter()
            .find(|issue| issue.code == CODE_BASE_COMMIT_INVALID)
            .expect("the base must be reported");
        assert_eq!(finding.op_id, None);
    }

    #[test]
    fn the_codes_of_the_module_are_the_ones_it_reports() {
        assert_eq!(ALL_CODES.len(), 9);
        for code in ALL_CODES {
            assert!(!code.is_empty());
        }
    }
}
