//! TASE contract layer: the DTOs, the JSON schemas, the error codes and the identifiers that every TASE module
//! shares.
//!
//! This crate is the lowest layer of TASE (design §1.1): `tase-registry`, `tase-semantic`, `tase-planner`,
//! `tase-executor`, `tase-worker` and `tase-metrics` all speak the types of this crate, and the REST and the
//! JSON-RPC front-ends speak them too. It holds no logic: a DTO, the JSON of a DTO and the schema of a DTO.
//!
//! Two rules the crate keeps:
//! * the JSON is written with the strict parser of `pime-core` (no comments, no trailing commas, no duplicate keys),
//!   so a payload that is not ours is refused instead of half-understood;
//! * the identifiers are checked before they are used (they end up in file names, in git refs and in the audit log),
//!   so an identifier that could leave its field is refused at the door.
//!
//! The crate has no third-party dependency: the JSON and the SHA-256 come from `pime-core`.

#![forbid(unsafe_code)]

pub mod api;
pub mod error;
pub mod ids;
pub mod version;

pub use error::{ErrorCode, TaseError};
pub use ids::{
    AgentId, AssetId, CommitId, ImplementationId, JobId, OperationId, PlanId, PreviewId,
    RepositoryId, RequestId, SessionId, TemplateId, TenantId, TraceId, WorkspaceId,
};
pub use version::{ASSET_SCHEMA_VERSION, CONTRACT_VERSION};

/// The schemas of the crate and their names.
pub const SCHEMAS: &[(&str, &str)] = &[
    ("plan", include_str!("../schemas/plan.schema.json")),
    ("template", include_str!("../schemas/template.schema.json")),
    ("job", include_str!("../schemas/job.schema.json")),
    ("preview", include_str!("../schemas/preview.schema.json")),
];

/// The schema of a name: `"plan"`, `"template"`, `"job"` or `"preview"`.
///
/// The file name without its suffix (`"plan.schema.json"`) is accepted as well. An unknown name is `None`.
pub fn schema(name: &str) -> Option<&'static str> {
    let name = name.strip_suffix(".schema.json").unwrap_or(name);
    SCHEMAS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| *text)
}

/// The names of the schemas of the crate.
pub fn schema_names() -> impl Iterator<Item = &'static str> {
    SCHEMAS.iter().map(|(name, _)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::json::{Limits, parse};

    #[test]
    fn the_four_schemas_are_there_and_they_are_json() {
        let names: Vec<&str> = schema_names().collect();
        assert_eq!(names, ["plan", "template", "job", "preview"]);
        for name in names {
            let text = schema(name).unwrap_or_else(|| panic!("the schema {name} must exist"));
            let value =
                parse(text, Limits::DEFAULT).expect("the schema must be a JSON the parser accepts");
            assert!(matches!(value, pime_core::json::Json::Obj(_)), "{name}");
        }
    }

    #[test]
    fn a_schema_can_be_asked_for_by_its_file_name() {
        assert_eq!(schema("plan"), schema("plan.schema.json"));
        assert!(schema("plan.schema.json").is_some());
    }

    #[test]
    fn an_unknown_schema_is_not_invented() {
        assert!(schema("nonsense").is_none());
        assert!(schema("").is_none());
        assert!(schema("plans").is_none());
    }

    #[test]
    fn the_schemas_are_the_ones_of_the_crate() {
        assert_eq!(SCHEMAS.len(), 4);
        for (name, text) in SCHEMAS {
            assert_eq!(schema(name), Some(*text));
            assert!(!text.is_empty());
        }
    }

    #[test]
    fn the_contract_version_of_the_crate_is_the_one_of_the_book() {
        assert_eq!(CONTRACT_VERSION, "1.1.0");
        assert!(version::is_supported_contract_version(CONTRACT_VERSION));
        assert_eq!(ASSET_SCHEMA_VERSION, "tase.asset.v1");
    }
}
