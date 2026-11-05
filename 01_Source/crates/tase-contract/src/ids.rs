//! The identifiers of the contract.
//!
//! Every identifier is a newtype around a string, and every identifier is checked when it is read. An identifier ends
//! up in a file name, in a git ref, in a URL, in a database key and in the audit log, so an identifier that could
//! leave its field is refused at the door instead of being quoted into it later.
//!
//! The rules are the same for every kind:
//! * it is not empty;
//! * it is at most [`MAX_ID_LEN`] characters long;
//! * it is written with `[A-Za-z0-9._:-]` only - no space, no `/`, no `\`. An identifier never names a path.
//!
//! The kind of an identifier is part of its type: a `PlanId` cannot be passed where a `JobId` is expected, and a
//! digest of a template is not the version of a template.

use crate::error::TaseError;
use std::fmt;
use std::str::FromStr;

/// The maximum length of an identifier.
pub const MAX_ID_LEN: usize = 128;

/// Checks an identifier against the rules of the module.
fn validate(kind: &str, s: &str) -> Result<(), TaseError> {
    if s.is_empty() {
        return Err(TaseError::invalid_argument(format!(
            "{kind} must not be empty"
        )));
    }
    if s.chars().count() > MAX_ID_LEN {
        return Err(TaseError::invalid_argument(format!(
            "{kind} must not be longer than {MAX_ID_LEN} characters"
        )));
    }
    for c in s.chars() {
        if !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-')) {
            return Err(TaseError::invalid_argument(format!(
                "{kind} must not contain {c:?}: only [A-Za-z0-9._:-] is allowed"
            )));
        }
    }
    Ok(())
}

/// Defines the identifier types: one newtype per kind, with the same behaviour.
macro_rules! define_ids {
    ($($(#[$meta:meta])* $name:ident),* $(,)?) => {
        $(
            $(#[$meta])*
            #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
            pub struct $name(String);

            impl $name {
                /// The kind of the identifier, used in the error messages.
                const KIND: &'static str = stringify!($name);

                /// Reads an identifier.
                pub fn parse(s: &str) -> Result<Self, TaseError> {
                    validate(Self::KIND, s)?;
                    Ok(Self(s.to_string()))
                }

                /// Reads an identifier from anything that can become a string.
                pub fn new(s: impl Into<String>) -> Result<Self, TaseError> {
                    let s = s.into();
                    validate(Self::KIND, &s)?;
                    Ok(Self(s))
                }

                /// The identifier as a string.
                pub fn as_str(&self) -> &str {
                    &self.0
                }
            }

            impl fmt::Display for $name {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str(&self.0)
                }
            }

            impl FromStr for $name {
                type Err = TaseError;

                fn from_str(s: &str) -> Result<Self, Self::Err> {
                    Self::parse(s)
                }
            }
        )*
    };
}

define_ids! {
    /// The tenant a request belongs to. It is a hint in the body; the real subject comes from the credential.
    TenantId,
    /// The workspace a request works in.
    WorkspaceId,
    /// The repository a request works in.
    RepositoryId,
    /// The agent that asks.
    AgentId,
    /// The id of a request. It is used for the trace only, never as the key of an idempotency.
    RequestId,
    /// The id of a trace, shared by every call of one job.
    TraceId,
    /// The id of a plan.
    PlanId,
    /// The id of a preview.
    PreviewId,
    /// The id of a job.
    JobId,
    /// The id of a template (`rust.handler.create`).
    TemplateId,
    /// The id of an asset of the catalog (`algorithm.graph.dijkstra`).
    AssetId,
    /// The id of an operation inside a plan.
    OperationId,
    /// The id of a session of the local multi-agent service.
    SessionId,
    /// The id of a commit.
    CommitId,
    /// The id of one implementation of an asset.
    ImplementationId,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind of identifier, with a value that is legal for it.
    const SAMPLES: [(&str, &str); 15] = [
        ("TenantId", "tn_demo"),
        ("WorkspaceId", "ws_demo"),
        ("RepositoryId", "repo_ide1"),
        ("AgentId", "agent_17"),
        ("RequestId", "req_01HXYZ"),
        ("TraceId", "trace_01HXYZ"),
        ("PlanId", "plan_001"),
        ("PreviewId", "prev_001"),
        ("JobId", "job_001"),
        ("TemplateId", "rust.handler.create"),
        ("AssetId", "algorithm.graph.dijkstra"),
        ("OperationId", "op_001"),
        ("SessionId", "sess_01"),
        ("CommitId", "e5f6fullsha"),
        (
            "ImplementationId",
            "algorithm.graph.dijkstra.rust.binary_heap",
        ),
    ];

    /// Checks every identifier type with the same rules.
    macro_rules! check_kind {
        ($($ty:ty),* $(,)?) => {
            $(
                {
                    let ok = <$ty>::parse("rust.handler.create").expect("a legal identifier");
                    assert_eq!(ok.as_str(), "rust.handler.create");
                    assert_eq!(ok.to_string(), "rust.handler.create");
                    assert_eq!("rust.handler.create".parse::<$ty>().unwrap(), ok);

                    let built = <$ty>::new(String::from("rust.handler.create")).expect("a legal identifier");
                    assert_eq!(built, ok);

                    // empty
                    let empty = <$ty>::parse("").unwrap_err();
                    assert_eq!(empty.code, crate::error::ErrorCode::InvalidArgument);
                    // too long
                    let long = "a".repeat(MAX_ID_LEN + 1);
                    assert!(<$ty>::parse(&long).is_err(), "a long identifier must be refused");
                    // a space
                    assert!(<$ty>::parse("a b").is_err(), "a space must be refused");
                    // a slash
                    assert!(<$ty>::parse("a/b").is_err(), "a slash must be refused");
                    // a backslash
                    assert!(<$ty>::parse("a\\b").is_err(), "a backslash must be refused");
                    // a dot dot
                    assert!(<$ty>::parse("../etc").is_err(), "a traversal must be refused");
                    // the longest legal one is still legal
                    let longest = "a".repeat(MAX_ID_LEN);
                    assert!(<$ty>::parse(&longest).is_ok(), "the longest legal identifier must be accepted");
                }
            )*
        };
    }

    #[test]
    fn every_kind_follows_the_same_rules() {
        check_kind!(
            TenantId,
            WorkspaceId,
            RepositoryId,
            AgentId,
            RequestId,
            TraceId,
            PlanId,
            PreviewId,
            JobId,
            TemplateId,
            AssetId,
            OperationId,
            SessionId,
            CommitId,
            ImplementationId,
        );
    }

    #[test]
    fn the_values_of_the_design_book_are_legal() {
        assert_eq!(TenantId::parse("tn_demo").unwrap().as_str(), "tn_demo");
        assert_eq!(WorkspaceId::parse("ws_demo").unwrap().as_str(), "ws_demo");
        assert_eq!(
            RepositoryId::parse("repo_ide1").unwrap().as_str(),
            "repo_ide1"
        );
        assert_eq!(AgentId::parse("agent_17").unwrap().as_str(), "agent_17");
        assert_eq!(
            RequestId::parse("req_01HXYZ").unwrap().as_str(),
            "req_01HXYZ"
        );
        assert_eq!(
            TraceId::parse("trace_01HXYZ").unwrap().as_str(),
            "trace_01HXYZ"
        );
        assert_eq!(
            TemplateId::parse("rust.handler.create").unwrap().as_str(),
            "rust.handler.create"
        );
    }

    #[test]
    fn every_kind_has_a_sample_that_is_legal() {
        assert_eq!(SAMPLES.len(), 15);
        // the samples are the values of the design book; they must all be legal for the kind they belong to
        for (kind, value) in SAMPLES {
            let legal = value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'));
            assert!(legal, "{kind} {value:?} must be legal");
            assert!(!value.is_empty());
            assert!(value.chars().count() <= MAX_ID_LEN);
        }
    }

    #[test]
    fn an_identifier_that_leaves_its_field_is_refused() {
        for bad in [
            "a b",
            "a/b",
            "a\\b",
            "../etc/passwd",
            "a\tb",
            "a\nb",
            "a\u{0}b",
            "héllo",
            "a;b",
            "a*b",
            "a?b",
            "a#b",
        ] {
            assert!(TemplateId::parse(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn the_error_names_the_kind_that_was_refused() {
        let error = PlanId::parse("").unwrap_err();
        assert!(
            error.message.contains("PlanId"),
            "the message must name the kind: {}",
            error.message
        );
        assert_eq!(error.code, crate::error::ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_identifiers_of_two_kinds_are_not_the_same_type() {
        // the types differ even when the text is the same
        let plan = PlanId::parse("x").unwrap();
        let job = JobId::parse("x").unwrap();
        assert_eq!(plan.as_str(), job.as_str());
        assert!(format!("{plan}") != format!("{job:?}"));
    }
}
