//! The canonical JSON of a plan and its SHA-256 digest (detailed design §2.3).
//!
//! The plan is not a file: the plan *is* its canonical JSON, and its digest is the digest of that JSON
//! (the plan schema of `tase-contract` says it in its own description). The canonical form has to be stable, because
//! the digest is the identity of the plan: the executor compares it with the digest the caller expects, and the same
//! idempotency key with another digest is a failure (basic design §4.3, `IDEMPOTENCY_MISMATCH`).
//!
//! # The rules of the canonical form
//!
//! * the keys of an object are sorted in the lexicographic order of their bytes;
//! * the order of the elements of an array is kept;
//! * a string is escaped as JSON escapes it (the control characters become `\u00xx`);
//! * a whole number is written without a decimal point;
//! * **the table of the parameters of an operation keeps the order it was written in.** The contract layer says so
//!   ("the order of the parameters is part of the digest of the plan"), so two plans whose parameters are written in
//!   another order are two plans. Every other object is sorted.
//!
//! The same plan therefore always gives the same string and the same digest, and a plan that differs in any of its
//! parameters gives another digest.

use std::fmt::Write as _;

use pime_core::json::Json;
use tase_contract::api::{Operation, PlanRequest, Preconditions, Target, TemplateRef};

/// The prefix of a digest of this crate.
pub const DIGEST_PREFIX: &str = "sha256:";

/// The canonical JSON of a plan.
///
/// The same plan always gives the same string; the string is the input of [`plan_digest`].
pub fn canonical_json(request: &PlanRequest) -> String {
    let mut out = String::new();
    out.push('{');

    // The keys of the plan are written in the lexicographic order.
    write_key("approval_policy", &mut out);
    write_value(&Json::str(&request.approval_policy), &mut out);
    out.push(',');
    write_key("base_commit", &mut out);
    write_value(&Json::str(&request.base_commit), &mut out);
    out.push(',');
    write_key("contract_version", &mut out);
    write_value(&Json::str(&request.contract_version), &mut out);
    out.push(',');
    write_key("operations", &mut out);
    out.push('[');
    for (index, operation) in request.operations.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_operation(operation, &mut out);
    }
    out.push(']');
    out.push(',');
    write_key("repository_id", &mut out);
    write_value(&Json::str(request.repository_id.as_str()), &mut out);
    out.push(',');
    write_key("strategy", &mut out);
    write_value(&Json::str(&request.strategy), &mut out);
    out.push(',');
    write_key("target_ref", &mut out);
    write_value(&Json::str(&request.target_ref), &mut out);
    out.push(',');
    write_key("validation_profile", &mut out);
    write_value(&Json::str(&request.validation_profile), &mut out);

    out.push('}');
    out
}

/// The digest of a canonical JSON: `sha256:` and 64 hex digits in lower case.
pub fn plan_digest(canonical: &str) -> String {
    let mut out = String::with_capacity(DIGEST_PREFIX.len() + 64);
    out.push_str(DIGEST_PREFIX);
    out.push_str(&pime_core::sha256::sha256_hex(canonical));
    out
}

/// The digest of a plan: the canonical JSON of the plan and its digest.
pub fn digest_of(request: &PlanRequest) -> String {
    plan_digest(&canonical_json(request))
}

/// The digest of a plan that is read from a JSON value.
pub fn digest_of_value(value: &Json) -> Result<String, tase_contract::error::TaseError> {
    Ok(digest_of(&PlanRequest::from_json(value)?))
}

/// Writes one operation of a plan in the canonical form.
///
/// The keys are written in the lexicographic order; the table of the parameters is the one exception: it keeps the
/// order it was written in (the contract layer makes that order part of the digest of the plan).
fn write_operation(operation: &Operation, out: &mut String) {
    out.push('{');

    write_key("kind", out);
    write_value(&Json::str(operation.kind.as_str()), out);
    out.push(',');

    write_key("op_id", out);
    write_value(&Json::str(operation.op_id.as_str()), out);
    out.push(',');

    write_key("params", out);
    write_params(&operation.params, out);
    out.push(',');

    write_key("preconditions", out);
    match &operation.preconditions {
        Some(preconditions) => write_value(&preconditions_value(preconditions), out),
        None => out.push_str("null"),
    }
    out.push(',');

    write_key("target", out);
    write_value(&target_value(&operation.target), out);
    out.push(',');

    write_key("template", out);
    match &operation.template {
        Some(template) => write_value(&template_value(template), out),
        None => out.push_str("null"),
    }

    out.push('}');
}

/// Writes the table of the parameters of an operation, in the order it was written in.
fn write_params(params: &[(String, String)], out: &mut String) {
    out.push('{');
    for (index, (name, value)) in params.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_string(name, out);
        out.push(':');
        write_string(value, out);
    }
    out.push('}');
}

/// The value of the target of an operation.
fn target_value(target: &Target) -> Json {
    Json::obj(vec![
        ("path", Json::str(&target.path)),
        (
            "symbol",
            match &target.symbol {
                Some(symbol) => Json::str(symbol),
                None => Json::Null,
            },
        ),
        (
            "symbol_kind",
            match &target.symbol_kind {
                Some(kind) => Json::str(kind),
                None => Json::Null,
            },
        ),
    ])
}

/// The value of the template of an operation.
fn template_value(template: &TemplateRef) -> Json {
    Json::obj(vec![
        ("id", Json::str(template.id.as_str())),
        ("version", Json::str(&template.version)),
        ("digest", Json::str(&template.digest)),
    ])
}

/// The value of the preconditions of an operation.
fn preconditions_value(preconditions: &Preconditions) -> Json {
    Json::obj(vec![
        (
            "file_blob_sha",
            match &preconditions.file_blob_sha {
                Some(sha) => Json::str(sha),
                None => Json::Null,
            },
        ),
        (
            "symbol_signature_hash",
            match &preconditions.symbol_signature_hash {
                Some(hash) => Json::str(hash),
                None => Json::Null,
            },
        ),
        (
            "expected_symbol_exists",
            match preconditions.expected_symbol_exists {
                Some(exists) => Json::Bool(exists),
                None => Json::Null,
            },
        ),
    ])
}

/// Writes a key and its separator.
fn write_key(key: &str, out: &mut String) {
    write_string(key, out);
    out.push(':');
}

/// Writes a value; the keys of an object are sorted.
fn write_value(value: &Json, out: &mut String) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Json::Num(number) => write_number(*number, out),
        Json::Str(text) => write_string(text, out),
        Json::Arr(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Json::Obj(pairs) => {
            let mut sorted: Vec<&(String, Json)> = pairs.iter().collect();
            sorted.sort_by(|left, right| left.0.cmp(&right.0));
            out.push('{');
            for (index, (key, item)) in sorted.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(item, out);
            }
            out.push('}');
        }
    }
}

/// Writes a number; a whole number is written without a decimal point.
fn write_number(number: f64, out: &mut String) {
    if !number.is_finite() {
        out.push_str("null");
    } else if number.fract() == 0.0 && number.abs() < 1e15 {
        let _ = write!(out, "{}", number as i64);
    } else {
        let _ = write!(out, "{number}");
    }
}

/// Writes a string as JSON writes it.
fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            control if (control as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::json::{Limits, parse};
    use tase_contract::api::{Operation, OperationKind};
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
            params: vec![
                ("entity".to_string(), "User".to_string()),
                ("database".to_string(), "postgres".to_string()),
            ],
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

    #[test]
    fn the_same_plan_gives_the_same_canonical_json() {
        let plan = valid_plan();
        assert_eq!(canonical_json(&plan), canonical_json(&plan));
    }

    #[test]
    fn the_same_plan_gives_the_same_digest() {
        let plan = valid_plan();
        assert_eq!(digest_of(&plan), digest_of(&plan));
        assert_eq!(digest_of(&plan), plan_digest(&canonical_json(&plan)));
    }

    #[test]
    fn the_digest_has_the_shape_of_the_design_book() {
        let digest = digest_of(&valid_plan());
        assert!(digest.starts_with("sha256:"), "{digest}");
        let hex = digest.strip_prefix("sha256:").expect("the prefix");
        assert_eq!(hex.len(), 64);
        assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(
            hex.bytes().all(|b| !b.is_ascii_uppercase()),
            "the digest is in lower case: {digest}"
        );
    }

    #[test]
    fn a_change_of_a_parameter_changes_the_digest() {
        let mut plan = valid_plan();
        let before = digest_of(&plan);
        plan.operations[0].params[0].1 = "Order".to_string();
        assert_ne!(before, digest_of(&plan));
    }

    #[test]
    fn a_change_of_any_field_changes_the_digest() {
        let base = digest_of(&valid_plan());

        let mut plan = valid_plan();
        plan.base_commit = "b".repeat(40);
        assert_ne!(base, digest_of(&plan), "base_commit");

        let mut plan = valid_plan();
        plan.target_ref = "refs/heads/other".to_string();
        assert_ne!(base, digest_of(&plan), "target_ref");

        let mut plan = valid_plan();
        plan.strategy = "semantic".to_string();
        assert_ne!(base, digest_of(&plan), "strategy");

        let mut plan = valid_plan();
        plan.validation_profile = "other-profile".to_string();
        assert_ne!(base, digest_of(&plan), "validation_profile");

        let mut plan = valid_plan();
        plan.approval_policy = "other-policy".to_string();
        assert_ne!(base, digest_of(&plan), "approval_policy");

        let mut plan = valid_plan();
        plan.contract_version = "1.0.0".to_string();
        assert_ne!(base, digest_of(&plan), "contract_version");

        let mut plan = valid_plan();
        plan.operations[0].target.path = "src/api/order.rs".to_string();
        assert_ne!(base, digest_of(&plan), "target.path");

        let mut plan = valid_plan();
        plan.operations[0].op_id = OperationId::parse("op_002").expect("a legal id");
        assert_ne!(base, digest_of(&plan), "op_id");
    }

    #[test]
    fn the_order_of_the_operations_is_part_of_the_plan() {
        let mut plan = valid_plan();
        plan.operations.push(template_operation("op_002"));
        let before = digest_of(&plan);
        plan.operations.reverse();
        assert_ne!(before, digest_of(&plan));
    }

    #[test]
    fn the_order_of_the_parameters_is_part_of_the_digest() {
        // The contract layer says so: "the order of the parameters is part of the digest of the plan".
        let mut plan = valid_plan();
        let before = digest_of(&plan);
        plan.operations[0].params.reverse();
        assert_ne!(before, digest_of(&plan));
    }

    #[test]
    fn two_equivalent_json_give_the_same_canonical_json() {
        // The same plan, written with the keys in another order.
        let first = r#"{
            "contract_version": "1.1.0",
            "repository_id": "repo_ide1",
            "target_ref": "refs/heads/agent-feature",
            "base_commit": "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
            "strategy": "template",
            "operations": [
                {
                    "op_id": "op_001",
                    "kind": "instantiate_template",
                    "target": {
                        "path": "src/api/user.rs",
                        "symbol": "crate::api::user::create_user",
                        "symbol_kind": "function"
                    },
                    "template": {
                        "id": "rust.handler.create",
                        "version": "1.0.0",
                        "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    },
                    "params": { "entity": "User", "database": "postgres" },
                    "preconditions": {
                        "file_blob_sha": "0000000000000000000000000000000000000000",
                        "symbol_signature_hash": null,
                        "expected_symbol_exists": false
                    }
                }
            ],
            "validation_profile": "rust-ci-default",
            "approval_policy": "project-default"
        }"#;

        let second = r#"{
            "approval_policy": "project-default",
            "validation_profile": "rust-ci-default",
            "operations": [
                {
                    "preconditions": {
                        "expected_symbol_exists": false,
                        "symbol_signature_hash": null,
                        "file_blob_sha": "0000000000000000000000000000000000000000"
                    },
                    "params": { "entity": "User", "database": "postgres" },
                    "template": {
                        "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                        "version": "1.0.0",
                        "id": "rust.handler.create"
                    },
                    "target": {
                        "symbol_kind": "function",
                        "symbol": "crate::api::user::create_user",
                        "path": "src/api/user.rs"
                    },
                    "kind": "instantiate_template",
                    "op_id": "op_001"
                }
            ],
            "strategy": "template",
            "base_commit": "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
            "target_ref": "refs/heads/agent-feature",
            "repository_id": "repo_ide1",
            "contract_version": "1.1.0"
        }"#;

        let plan_first =
            PlanRequest::from_json(&parse(first, Limits::DEFAULT).expect("valid JSON"))
                .expect("a valid plan");
        let plan_second =
            PlanRequest::from_json(&parse(second, Limits::DEFAULT).expect("valid JSON"))
                .expect("a valid plan");

        assert_eq!(canonical_json(&plan_first), canonical_json(&plan_second));
        assert_eq!(digest_of(&plan_first), digest_of(&plan_second));
    }

    #[test]
    fn the_canonical_json_is_a_json_the_parser_accepts() {
        let canonical = canonical_json(&valid_plan());
        let value =
            parse(&canonical, Limits::DEFAULT).expect("the canonical form must be valid JSON");
        assert!(matches!(value, Json::Obj(_)));
    }

    #[test]
    fn the_keys_of_the_canonical_json_are_sorted() {
        let canonical = canonical_json(&valid_plan());
        let value = parse(&canonical, Limits::DEFAULT).expect("valid JSON");
        assert_sorted(&value, "");
    }

    /// Checks that the keys of every object are sorted; the table of the parameters is the one exception.
    fn assert_sorted(value: &Json, path: &str) {
        match value {
            Json::Obj(pairs) => {
                if !path.ends_with(".params") {
                    let keys: Vec<&str> = pairs.iter().map(|(key, _)| key.as_str()).collect();
                    let mut sorted = keys.clone();
                    sorted.sort_unstable();
                    assert_eq!(keys, sorted, "the keys of {path} are not sorted");
                }
                for (key, item) in pairs {
                    assert_sorted(item, &format!("{path}.{key}"));
                }
            }
            Json::Arr(items) => {
                for (index, item) in items.iter().enumerate() {
                    assert_sorted(item, &format!("{path}[{index}]"));
                }
            }
            _ => {}
        }
    }

    #[test]
    fn the_canonical_json_holds_no_space_and_no_newline() {
        let canonical = canonical_json(&valid_plan());
        assert!(!canonical.contains('\n'), "the canonical form is one line");
        assert!(!canonical.contains(' '), "the canonical form has no space");
        assert!(canonical.starts_with('{') && canonical.ends_with('}'));
    }

    #[test]
    fn a_string_with_a_quote_or_a_newline_is_escaped() {
        let mut plan = valid_plan();
        plan.operations[0].params[0].1 = "a \"quoted\"\nvalue".to_string();
        let canonical = canonical_json(&plan);
        assert!(canonical.contains("\\\""), "{canonical}");
        assert!(canonical.contains("\\n"), "{canonical}");
        let value =
            parse(&canonical, Limits::DEFAULT).expect("the canonical form must be valid JSON");
        assert_eq!(
            value
                .get("operations")
                .and_then(|o| o.as_array())
                .map(|a| a.len()),
            Some(1)
        );
    }

    #[test]
    fn the_digest_of_a_value_is_the_digest_of_the_plan() {
        let plan = valid_plan();
        let value = plan.to_json_value();
        assert_eq!(
            digest_of_value(&value).expect("a valid plan"),
            digest_of(&plan)
        );
    }

    #[test]
    fn a_plan_without_a_precondition_holds_a_null() {
        let mut plan = valid_plan();
        plan.operations[0].preconditions = None;
        let canonical = canonical_json(&plan);
        assert!(canonical.contains("\"preconditions\":null"), "{canonical}");
    }

    #[test]
    fn a_plan_without_a_template_holds_a_null() {
        let mut plan = valid_plan();
        plan.operations[0].template = None;
        let canonical = canonical_json(&plan);
        assert!(canonical.contains("\"template\":null"), "{canonical}");
    }
}
