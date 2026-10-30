//! Context levels L0-L4 and document snapshots (FR-013/014/015/017, DD section 6.1), port of `core/context.js`.
//!
//! Whole-file access comes only from a trusted editor buffer (L3); a file read from disk (L4) is read-only preview
//! and never written back. The core keeps the snapshot TEXT in memory, addressed by `snapshot_id`, so the bridge
//! never has to send it again (see `session.rs`).
//!
//! The hash of a snapshot is `sha256-utf8`: the lowercase hex SHA-256 of the UTF-8 bytes of the text, identical to
//! the JavaScript `sha256` helper. All offsets are UTF-16 code units (`range_unit: UTF16_CODE_UNITS`).

use crate::error::{Result, fail};
use crate::json::Json;
use crate::sha256::sha256_hex;
use crate::textrange::{self, Range};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub rank: u8,
    pub has_text: bool,
    pub full_text: bool,
    pub write: &'static str,
}

/// The capability lattice. An unknown level is the weakest one (L0).
pub fn capabilities_of(level: &str) -> Capabilities {
    let c = |rank, has_text, full_text, write| Capabilities {
        rank,
        has_text,
        full_text,
        write,
    };
    match level {
        "L1" => c(1, true, false, "range-checked-insert"),
        "L2" => c(2, true, false, "verified-selection"),
        "L3" => c(3, true, true, "bridge-transaction"),
        "L4" => c(4, true, true, "none"),
        _ => c(0, false, false, "none"),
    }
}

/// What the bridge hands over when it takes a snapshot of an editor buffer (`uri`/`version`/`text` come from the
/// live buffer, never from disk).
#[derive(Clone, Debug)]
pub struct SnapshotInput {
    pub uri: String,
    pub version: i64,
    /// `None` = the bridge had no text (`CONTEXT_UNAVAILABLE`).
    pub text: Option<String>,
    pub language_id: String,
    pub selection: Option<Range>,
    pub is_dirty: bool,
    pub level: String,
    pub editor_instance_id: String,
    pub file_name: String,
}

impl SnapshotInput {
    pub fn new(uri: &str, version: i64, text: &str) -> SnapshotInput {
        SnapshotInput {
            uri: uri.to_string(),
            version,
            text: Some(text.to_string()),
            language_id: String::new(),
            selection: None,
            is_dirty: false,
            level: "L3".to_string(),
            editor_instance_id: "editor-1".to_string(),
            file_name: String::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Assigned by the session when the snapshot is stored (empty for a free-standing snapshot).
    pub snapshot_id: String,
    pub editor_instance_id: String,
    pub document_uri: String,
    pub document_version: i64,
    pub sha256: String,
    pub language_id: String,
    pub file_name: String,
    pub eol: &'static str,
    pub dirty: bool,
    pub selection: Option<Range>,
    pub content_scope: String,
    pub content: String,
}

pub fn make_snapshot(input: SnapshotInput) -> Result<Snapshot> {
    let Some(text) = input.text else {
        return fail("CONTEXT_UNAVAILABLE", "no document text");
    };
    Ok(Snapshot {
        snapshot_id: String::new(),
        editor_instance_id: input.editor_instance_id,
        document_uri: input.uri,
        document_version: input.version,
        sha256: sha256_hex(&text),
        language_id: input.language_id,
        file_name: input.file_name,
        eol: textrange::eol_of(&text).as_str(),
        dirty: input.is_dirty,
        selection: input.selection,
        content_scope: input.level,
        content: text,
    })
}

pub fn position_json(p: textrange::Position) -> Json {
    Json::obj(vec![
        ("line", Json::int(p.line)),
        ("character", Json::int(p.character)),
    ])
}

pub fn selection_json(s: &Option<Range>) -> Json {
    match s {
        Some(r) => Json::obj(vec![
            ("start", position_json(r.start)),
            ("end", position_json(r.end)),
        ]),
        None => Json::Null,
    }
}

impl Snapshot {
    /// The snapshot as the bridge sees it: the JS snapshot fields without `content` (the core keeps the text).
    pub fn meta_json(&self) -> Json {
        Json::obj(vec![
            ("snapshot_id", Json::str(&self.snapshot_id)),
            ("editor_instance_id", Json::str(&self.editor_instance_id)),
            ("document_uri", Json::str(&self.document_uri)),
            ("document_version", Json::int(self.document_version)),
            ("sha256", Json::str(&self.sha256)),
            ("hash_algorithm", Json::str("sha256-utf8")),
            ("language_id", Json::str(&self.language_id)),
            ("file_name", Json::str(&self.file_name)),
            ("eol", Json::str(self.eol)),
            ("dirty", Json::Bool(self.dirty)),
            ("selection", selection_json(&self.selection)),
            ("range_unit", Json::str("UTF16_CODE_UNITS")),
            ("content_scope", Json::str(&self.content_scope)),
            ("encoding", Json::str("encoding_unknown")),
            (
                "length",
                Json::int(crate::util::utf16_len(&self.content) as i64),
            ),
        ])
    }
}

/// Whole-file writes require L3. L4 (disk) is preview only, L1/L2 never claim the whole file (UT-025/030/053).
pub fn assert_full_file_writable(snapshot: Option<&Snapshot>) -> Result<()> {
    match snapshot {
        Some(s) if s.content_scope == "L3" => Ok(()),
        Some(s) if s.content_scope == "L4" => fail(
            "FULL_CONTEXT_REQUIRED",
            "a file read from disk is preview-only and cannot be written back",
        ),
        _ => fail(
            "FULL_CONTEXT_REQUIRED",
            "full-buffer context (L3) from a trusted editor bridge is required",
        ),
    }
}

/// A nearby excerpt (L1). Bounds never split a surrogate pair or a CRLF; all numbers are UTF-16 code units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Excerpt {
    pub text: String,
    pub start: usize,
    pub end: usize,
    pub cursor_in_excerpt: usize,
    pub truncated: bool,
}

pub const DEFAULT_BEFORE: usize = 1800;
pub const DEFAULT_AFTER: usize = 900;

pub fn excerpt(text: &str, cursor_offset: i64, before: usize, after: usize) -> Excerpt {
    let t = textrange::units(text);
    let len = t.len();
    let cursor = cursor_offset.clamp(0, len as i64) as usize;
    let mut start = cursor.saturating_sub(before);
    let mut end = (cursor + after).min(len);
    while start < cursor && !textrange::is_boundary_units(&t, start) {
        start += 1;
    }
    while end > cursor && !textrange::is_boundary_units(&t, end) {
        end -= 1;
    }
    Excerpt {
        text: String::from_utf16_lossy(&t[start..end]),
        start,
        end,
        cursor_in_excerpt: cursor - start,
        truncated: start > 0 || end < len,
    }
}

/// A wider request than the granted bounds is refused rather than silently trimmed.
pub fn require_within_policy(requested: (usize, usize), policy: (usize, usize)) -> Result<()> {
    if requested.0 > policy.0 || requested.1 > policy.1 {
        return fail(
            "GRANT_DENIED",
            "requested range is wider than the authorised scope",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::textrange::pos;

    const URI: &str = "file:///w/a.rs";

    fn snap(text: &str) -> Snapshot {
        let mut i = SnapshotInput::new(URI, 42, text);
        i.language_id = "rust".to_string();
        i.selection = Some(Range {
            start: pos(0, 0),
            end: pos(0, 0),
        });
        make_snapshot(i).unwrap()
    }

    fn snap_at(text: &str, level: &str) -> Snapshot {
        let mut i = SnapshotInput::new(URI, 42, text);
        i.level = level.to_string();
        make_snapshot(i).unwrap()
    }

    #[test]
    fn ut_025_context_level_capability_lattice() {
        assert!(!capabilities_of("L0").has_text);
        assert!(!capabilities_of("L1").full_text);
        assert!(!capabilities_of("L2").full_text);
        let l3 = capabilities_of("L3");
        assert_eq!((l3.full_text, l3.write), (true, "bridge-transaction"));
        let l4 = capabilities_of("L4");
        assert_eq!((l4.full_text, l4.write), (true, "none"));
        assert!(
            !capabilities_of("L9").has_text,
            "unknown level is the weakest"
        );
        assert!(capabilities_of("L1").rank < capabilities_of("L2").rank);
    }

    #[test]
    fn ut_027_nearby_excerpt_is_minimal_and_wider_requests_are_refused() {
        let text = format!("{}\u{1F600}{}", "x".repeat(500), "y".repeat(500));
        let ex = excerpt(&text, 500, 80, 80);
        assert_eq!(ex.text.encode_utf16().count(), 160);
        assert_eq!(ex.cursor_in_excerpt, 80);
        assert!(ex.truncated);
        assert_eq!((ex.start, ex.end), (420, 580));
        // a bound that would fall inside the emoji moves to a character boundary
        let edge = excerpt(&text, 502, 1, 1); // start 501 splits the surrogate pair and moves to 502
        assert_eq!((edge.start, edge.end, edge.text.as_str()), (502, 503, "y"));
        let edge = excerpt(&text, 500, 1, 1); // end 501 splits the pair and moves back to 500
        assert_eq!((edge.start, edge.end, edge.text.as_str()), (499, 500, "x"));
        assert!(!edge.text.contains('\u{fffd}'));
        // whole text: not truncated
        let all = excerpt("abc", 99, 10, 10);
        assert_eq!(
            (all.text.as_str(), all.start, all.end, all.truncated),
            ("abc", 0, 3, false)
        );
        assert_eq!(all.cursor_in_excerpt, 3);
        assert_eq!(excerpt("abc", -5, 10, 10).cursor_in_excerpt, 0);
        assert_eq!(
            require_within_policy((200, 80), (80, 80)).unwrap_err().code,
            "GRANT_DENIED"
        );
        assert_eq!(
            require_within_policy((80, 200), (80, 80)).unwrap_err().code,
            "GRANT_DENIED"
        );
        assert!(require_within_policy((80, 80), (80, 80)).is_ok());
    }

    #[test]
    fn excerpt_bounds_never_split_a_crlf() {
        let text = "aaaa\r\nbbbb";
        let ex = excerpt(text, 6, 2, 0); // start would be 4, between nothing; cursor 6 is after the CRLF
        assert!(textrange::is_boundary(text, ex.start));
        let ex = excerpt(text, 7, 2, 3); // start 5 splits CR|LF, so it moves to 6
        assert_eq!(ex.start, 6);
        assert_eq!(ex.cursor_in_excerpt, 1);
    }

    #[test]
    fn ut_028_snapshot_hash_comes_from_the_unsaved_buffer_not_the_disk() {
        let disk = "fn old() {}\n";
        let buffer = "fn old() {}\nfn unsaved() {}\n";
        let mut i = SnapshotInput::new(URI, 43, buffer);
        i.is_dirty = true;
        let s = make_snapshot(i).unwrap();
        assert_eq!(s.sha256, sha256_hex(buffer));
        assert_ne!(s.sha256, sha256_hex(disk));
        assert!(s.dirty);
        assert_eq!(s.document_version, 43);
        let m = s.meta_json();
        assert_eq!(
            m.get("hash_algorithm").and_then(Json::as_str),
            Some("sha256-utf8")
        );
        assert_eq!(
            m.get("range_unit").and_then(Json::as_str),
            Some("UTF16_CODE_UNITS")
        );
        assert_eq!(m.get("content_scope").and_then(Json::as_str), Some("L3"));
        assert!(m.get("content").is_none(), "the text stays in the core");
        assert_eq!(snap("a\r\nb\u{1F600}\r\n").eol, "CRLF");
        assert_eq!(snap("").eol, "NONE");
    }

    #[test]
    fn ut_030_an_l4_disk_snapshot_can_never_be_written_back_and_l1_is_not_whole_file() {
        let disk = snap_at("on disk\n", "L4");
        assert_eq!(
            assert_full_file_writable(Some(&disk)).unwrap_err().code,
            "FULL_CONTEXT_REQUIRED"
        );
        assert!(
            assert_full_file_writable(Some(&disk))
                .unwrap_err()
                .message
                .contains("preview-only")
        );
        assert_eq!(
            assert_full_file_writable(Some(&snap_at("x", "L1")))
                .unwrap_err()
                .code,
            "FULL_CONTEXT_REQUIRED"
        );
        assert_eq!(
            assert_full_file_writable(None).unwrap_err().code,
            "FULL_CONTEXT_REQUIRED"
        );
        assert!(assert_full_file_writable(Some(&snap("x"))).is_ok());
        let mut none = SnapshotInput::new(URI, 1, "");
        none.text = None;
        assert_eq!(make_snapshot(none).unwrap_err().code, "CONTEXT_UNAVAILABLE");
    }
}
