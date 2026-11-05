//! The candidate model of the input method (DD-IME section 20.4).
//!
//! A candidate is what the input method shows in its candidate window: a card of the catalog with the preview of the
//! text that would be inserted. **A candidate is not an action.** Nothing in this module executes anything, and the
//! model deliberately has no field for the position of the candidate window, for a key event or for the handle of the
//! focused window: DD-IME section 20.4 says that an agent which chooses the same asset receives a
//! `TasePlan`/`plan_digest` instead of those coordinates, so the model here must not be able to carry them. The
//! fields are the ones of the example of the design, and their names are part of the contract with the catalog.
//!
//! The reference of an asset is the id of the asset and its version joined by `@`
//! (`pattern.gof.behavioral.strategy@1.0.0`), the same shape as the `asset_id@version` of BD-IME section 18.2. The
//! implementation reference is the same shape: the implementation of the asset for one language
//! (`pattern.gof.behavioral.strategy.rust@1.0.0`).

use pime_core::error::{PimeError, Result, fail};
use pime_core::json::{Json, Limits};

/// The bounds of the JSON parser for one candidate.
fn limits() -> Limits {
    Limits {
        max_bytes: 64 * 1024,
        max_depth: 8,
    }
}

/// The reference of an asset: the id of the asset and the version it is at, joined by `@`.
///
/// The version is optional: a reference without `@` names the asset without a version (the newest one the catalog
/// knows). A reference with more than one `@`, without an id or without a version is refused: a half-understood
/// reference is worse than none, because the version is what tells the agent which rules it gets.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetRef {
    id: String,
    version: Option<String>,
}

impl AssetRef {
    /// The reference of a text (`id@version` or `id`); `None` when the text is not a reference.
    pub fn parse(text: &str) -> Option<AssetRef> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        match text.split_once('@') {
            Some((id, version)) => {
                let id = id.trim();
                let version = version.trim();
                // a second `@` means the text is not a reference at all
                if id.is_empty() || version.is_empty() || version.contains('@') {
                    return None;
                }
                Some(AssetRef {
                    id: id.to_string(),
                    version: Some(version.to_string()),
                })
            }
            None => Some(AssetRef {
                id: text.to_string(),
                version: None,
            }),
        }
    }

    /// The id of the asset (`pattern.gof.behavioral.strategy`).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The version of the asset, when the reference names one.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// The segments of the id (`pattern.gof.behavioral.strategy` -> `pattern`, `gof`, `behavioral`, `strategy`).
    ///
    /// The first segment is the kind of the asset (`pattern`, `algorithm`), the second one its category (`gof`,
    /// `graph`); the ranking uses them to tell whether a candidate belongs to the field the user asked for.
    pub fn segments(&self) -> Vec<&str> {
        self.id.split('.').filter(|s| !s.is_empty()).collect()
    }

    /// The kind of the asset (the first segment of the id), when the id has one.
    pub fn kind(&self) -> Option<&str> {
        self.segments().first().copied()
    }

    /// The category of the asset (the second segment of the id), when the id has one.
    pub fn category(&self) -> Option<&str> {
        let segments = self.segments();
        segments.get(1).copied()
    }
}

impl std::fmt::Display for AssetRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.version {
            Some(version) => write!(f, "{}@{version}", self.id),
            None => f.write_str(&self.id),
        }
    }
}

/// What the candidate is allowed to do (DD-IME section 20.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CandidateStatus {
    /// The candidate is a knowledge card only: the user may read it, but there is no code to insert.
    CandidateOnly,
    /// There is an approved implementation of the asset for the language of the candidate.
    ImplementationAvailable,
    /// The candidate may only be chosen after an explicit confirmation of the user.
    ConfirmationRequired,
}

impl CandidateStatus {
    /// Every status of a candidate.
    pub const ALL: &'static [CandidateStatus] = &[
        CandidateStatus::CandidateOnly,
        CandidateStatus::ImplementationAvailable,
        CandidateStatus::ConfirmationRequired,
    ];

    /// The name of the status (the value of the `status` field of the design example).
    pub fn as_str(self) -> &'static str {
        match self {
            CandidateStatus::CandidateOnly => "CANDIDATE_ONLY",
            CandidateStatus::ImplementationAvailable => "IMPLEMENTATION_AVAILABLE",
            CandidateStatus::ConfirmationRequired => "CONFIRMATION_REQUIRED",
        }
    }

    /// The status of a name; an unknown name is refused.
    pub fn parse(name: &str) -> Option<CandidateStatus> {
        match name {
            "CANDIDATE_ONLY" => Some(CandidateStatus::CandidateOnly),
            "IMPLEMENTATION_AVAILABLE" => Some(CandidateStatus::ImplementationAvailable),
            "CONFIRMATION_REQUIRED" => Some(CandidateStatus::ConfirmationRequired),
            _ => None,
        }
    }
}

/// A candidate of the input method (DD-IME section 20.4).
///
/// The field names are the ones of the example of the design and are part of the contract with the catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// The id of the candidate (`cand-104`).
    pub candidate_id: String,
    /// The asset the candidate shows.
    pub asset_ref: AssetRef,
    /// The implementation of the asset for one language, when there is an approved one.
    pub implementation_ref: Option<AssetRef>,
    /// The language of the implementation (`rust`).
    pub language: String,
    /// The line the candidate window shows (`Strategy: trait + implementations`).
    pub label: String,
    /// The text that would be inserted; it is a preview, not an insertion.
    pub insert_preview: String,
    /// Whether the choice of the candidate needs a confirmation of the user.
    pub selection_requires_confirmation: bool,
    /// Whether the candidate changes the whole project (then it goes to the IDE service for a preview, never to the
    /// commit of the text service).
    pub complete_project_edit: bool,
    /// The risk of the candidate, when it has one.
    pub risk: Option<String>,
    /// What the candidate is allowed to do.
    pub status: CandidateStatus,
}

impl Candidate {
    /// A candidate of an asset: a knowledge card without an implementation, which is the safest state.
    pub fn new(candidate_id: &str, asset_ref: AssetRef) -> Candidate {
        Candidate {
            candidate_id: candidate_id.to_string(),
            asset_ref,
            implementation_ref: None,
            language: String::new(),
            label: String::new(),
            insert_preview: String::new(),
            selection_requires_confirmation: false,
            complete_project_edit: false,
            risk: None,
            status: CandidateStatus::CandidateOnly,
        }
    }

    /// The candidate as the JSON of the contract.
    pub fn to_json(&self) -> Json {
        Json::obj(vec![
            ("candidate_id", Json::str(&self.candidate_id)),
            ("asset_ref", Json::str(&self.asset_ref.to_string())),
            (
                "implementation_ref",
                self.implementation_ref
                    .as_ref()
                    .map_or(Json::Null, |r| Json::str(&r.to_string())),
            ),
            ("language", Json::str(&self.language)),
            ("label", Json::str(&self.label)),
            ("insert_preview", Json::str(&self.insert_preview)),
            (
                "selection_requires_confirmation",
                Json::Bool(self.selection_requires_confirmation),
            ),
            (
                "complete_project_edit",
                Json::Bool(self.complete_project_edit),
            ),
            ("risk", self.risk.as_deref().map_or(Json::Null, Json::str)),
            ("status", Json::str(self.status.as_str())),
        ])
    }

    /// The candidate of a JSON value; a field of the wrong type is refused (`OUTPUT_INVALID`).
    pub fn from_json(value: &Json) -> Result<Candidate> {
        let bad = |what: &str| -> PimeError { PimeError::new("OUTPUT_INVALID", what.to_string()) };
        let Some(candidate_id) = value.get("candidate_id").and_then(Json::as_str) else {
            return fail("OUTPUT_INVALID", "the candidate has no candidate_id");
        };
        let Some(asset_ref) = value.get("asset_ref").and_then(Json::as_str) else {
            return fail("OUTPUT_INVALID", "the candidate has no asset_ref");
        };
        let Some(asset_ref) = AssetRef::parse(asset_ref) else {
            return fail("OUTPUT_INVALID", "the asset_ref is not an id@version reference");
        };
        let mut candidate = Candidate::new(candidate_id, asset_ref);
        match value.get("implementation_ref") {
            None | Some(Json::Null) => {}
            Some(Json::Str(s)) => {
                candidate.implementation_ref = Some(
                    AssetRef::parse(s)
                        .ok_or_else(|| bad("the implementation_ref is not an id@version reference"))?,
                );
            }
            Some(_) => return fail("OUTPUT_INVALID", "the implementation_ref is not a string"),
        }
        for (key, slot) in [
            ("language", &mut candidate.language),
            ("label", &mut candidate.label),
            ("insert_preview", &mut candidate.insert_preview),
        ] {
            match value.get(key) {
                None => {}
                Some(Json::Str(s)) => *slot = s.clone(),
                Some(_) => return fail("OUTPUT_INVALID", "the candidate shape"),
            }
        }
        for (key, slot) in [
            (
                "selection_requires_confirmation",
                &mut candidate.selection_requires_confirmation,
            ),
            ("complete_project_edit", &mut candidate.complete_project_edit),
        ] {
            match value.get(key) {
                None => {}
                Some(Json::Bool(b)) => *slot = *b,
                Some(_) => return fail("OUTPUT_INVALID", "the candidate shape"),
            }
        }
        match value.get("risk") {
            None | Some(Json::Null) => {}
            Some(Json::Str(s)) => candidate.risk = Some(s.clone()),
            Some(_) => return fail("OUTPUT_INVALID", "the risk is not a string"),
        }
        if let Some(status) = value.get("status") {
            candidate.status = status
                .as_str()
                .and_then(CandidateStatus::parse)
                .ok_or_else(|| bad("the status of the candidate is unknown"))?;
        }
        Ok(candidate)
    }

    /// The candidate as the text of the contract.
    pub fn to_json_string(&self) -> String {
        self.to_json().to_json_string()
    }

    /// The candidate of a JSON text; the text is bounded before it is parsed.
    pub fn from_json_string(text: &str) -> Result<Candidate> {
        let value = pime_core::json::parse(text, limits())
            .map_err(|_| PimeError::new("OUTPUT_INVALID", "the candidate is not JSON"))?;
        Candidate::from_json(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The candidate of the example of DD-IME section 20.4.
    fn design_example() -> Candidate {
        let mut candidate = Candidate::new(
            "cand-104",
            AssetRef::parse("pattern.gof.behavioral.strategy@1.0.0").expect("the reference of the example"),
        );
        candidate.implementation_ref =
            AssetRef::parse("pattern.gof.behavioral.strategy.rust@1.0.0");
        candidate.language = "rust".to_string();
        candidate.label = "Strategy: trait + implementations".to_string();
        candidate.insert_preview = "trait SortStrategy { /* ... */ }".to_string();
        candidate.selection_requires_confirmation = true;
        candidate.complete_project_edit = false;
        candidate.risk = Some("新 trait 带来抽象与维护成本".to_string());
        candidate.status = CandidateStatus::CandidateOnly;
        candidate
    }

    #[test]
    fn dd_0204_the_example_survives_the_json_round_trip() {
        let candidate = design_example();
        let text = candidate.to_json_string();
        let back = Candidate::from_json_string(&text).expect("the candidate is read back");
        assert_eq!(back, candidate);
        assert_eq!(back.status, CandidateStatus::CandidateOnly);
        assert_eq!(back.asset_ref.id(), "pattern.gof.behavioral.strategy");
        assert_eq!(back.asset_ref.version(), Some("1.0.0"));
    }

    #[test]
    fn dd_0204_the_json_has_the_fields_of_the_design_example() {
        let json = design_example().to_json();
        for key in [
            "candidate_id",
            "asset_ref",
            "implementation_ref",
            "language",
            "label",
            "insert_preview",
            "selection_requires_confirmation",
            "complete_project_edit",
            "risk",
            "status",
        ] {
            assert!(json.get(key).is_some(), "the field {key} is missing");
        }
        assert_eq!(
            json.get("asset_ref").and_then(Json::as_str),
            Some("pattern.gof.behavioral.strategy@1.0.0")
        );
        assert_eq!(
            json.get("implementation_ref").and_then(Json::as_str),
            Some("pattern.gof.behavioral.strategy.rust@1.0.0")
        );
        assert_eq!(
            json.get("status").and_then(Json::as_str),
            Some("CANDIDATE_ONLY")
        );
        assert_eq!(
            json.get("selection_requires_confirmation").and_then(Json::as_bool),
            Some(true)
        );
    }

    #[test]
    fn asset_ref_parses_the_id_and_the_version() {
        let reference = AssetRef::parse("pattern.gof.behavioral.strategy@1.0.0").expect("a reference");
        assert_eq!(reference.id(), "pattern.gof.behavioral.strategy");
        assert_eq!(reference.version(), Some("1.0.0"));
        assert_eq!(reference.to_string(), "pattern.gof.behavioral.strategy@1.0.0");
        assert_eq!(reference.kind(), Some("pattern"));
        assert_eq!(reference.category(), Some("gof"));
        assert_eq!(
            reference.segments(),
            vec!["pattern", "gof", "behavioral", "strategy"]
        );
    }

    #[test]
    fn asset_ref_without_a_version_has_none() {
        let reference = AssetRef::parse("pattern.gof.behavioral.strategy").expect("a reference");
        assert_eq!(reference.id(), "pattern.gof.behavioral.strategy");
        assert_eq!(reference.version(), None);
        assert_eq!(reference.to_string(), "pattern.gof.behavioral.strategy");
    }

    #[test]
    fn asset_ref_refuses_what_is_not_a_reference() {
        for bad in [
            "",
            "   ",
            "@1.0.0",
            "pattern.gof@",
            "pattern.gof@1.0.0@2",
            "a@b@c@d",
        ] {
            assert!(AssetRef::parse(bad).is_none(), "{bad} is not a reference");
        }
        assert!(AssetRef::parse("   pattern.gof@1.0.0   ")
            .is_some(),
            "the spaces around a reference are not part of it");
    }

    #[test]
    fn every_candidate_status_has_a_name_and_comes_back_from_it() {
        for status in CandidateStatus::ALL {
            assert_eq!(CandidateStatus::parse(status.as_str()), Some(*status));
        }
        assert_eq!(CandidateStatus::ALL.len(), 3);
        assert_eq!(
            CandidateStatus::parse("CANDIDATE_ONLY"),
            Some(CandidateStatus::CandidateOnly)
        );
        assert_eq!(
            CandidateStatus::parse("IMPLEMENTATION_AVAILABLE"),
            Some(CandidateStatus::ImplementationAvailable)
        );
        assert_eq!(
            CandidateStatus::parse("CONFIRMATION_REQUIRED"),
            Some(CandidateStatus::ConfirmationRequired)
        );
        assert_eq!(CandidateStatus::parse("candidate_only"), None);
        assert_eq!(CandidateStatus::parse("MAYBE"), None);
    }

    #[test]
    fn a_broken_candidate_json_is_refused() {
        for bad in [
            r#"{"asset_ref":"a@1"}"#,
            r#"{"candidate_id":"c"}"#,
            r#"{"candidate_id":"c","asset_ref":""}"#,
            r#"{"candidate_id":"c","asset_ref":"a@b@c"}"#,
            r#"{"candidate_id":"c","asset_ref":"a@1","status":"MAYBE"}"#,
            r#"{"candidate_id":"c","asset_ref":"a@1","language":7}"#,
            r#"{"candidate_id":"c","asset_ref":"a@1","complete_project_edit":"yes"}"#,
            r#"{"candidate_id":"c","asset_ref":"a@1","risk":3}"#,
            r#"{"candidate_id":"c","asset_ref":"a@1","implementation_ref":5}"#,
            "not json",
        ] {
            let error = Candidate::from_json_string(bad).expect_err("{bad} is refused");
            assert_eq!(error.code, "OUTPUT_INVALID", "{bad}");
        }
    }

    #[test]
    fn a_candidate_without_the_optional_parts_is_read_as_the_defaults() {
        let back = Candidate::from_json_string(r#"{"candidate_id":"c","asset_ref":"a@1"}"#)
            .expect("the candidate is read");
        assert_eq!(back.candidate_id, "c");
        assert_eq!(back.implementation_ref, None);
        assert_eq!(back.language, "");
        assert_eq!(back.risk, None);
        assert!(!back.selection_requires_confirmation);
        assert!(!back.complete_project_edit);
        assert_eq!(back.status, CandidateStatus::CandidateOnly);
    }
}
