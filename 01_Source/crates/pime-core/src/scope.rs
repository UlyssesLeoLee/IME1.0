//! DD-04 section 6.1: context levels L0-L4 and minimal excerpts (UT-025, UT-027, UT-030).
//! Whole-file writes come only from a trusted editor buffer (L3); a file read from disk (L4) is read-only
//! preview and is never written back. Excerpt bounds never split a surrogate pair or a CRLF (UT-026 rules).

use crate::error::{fail, Result};
use crate::textrange::{is_boundary_units, units};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    L0,
    L1,
    L2,
    L3,
    L4,
}

impl Level {
    pub const ALL: [Level; 5] = [Level::L0, Level::L1, Level::L2, Level::L3, Level::L4];

    pub fn parse(s: &str) -> Option<Level> {
        Some(match s {
            "L0" => Level::L0,
            "L1" => Level::L1,
            "L2" => Level::L2,
            "L3" => Level::L3,
            "L4" => Level::L4,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::L0 => "L0",
            Level::L1 => "L1",
            Level::L2 => "L2",
            Level::L3 => "L3",
            Level::L4 => "L4",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode {
    None,
    RangeCheckedInsert,
    VerifiedSelection,
    BridgeTransaction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub rank: u8,
    pub has_text: bool,
    pub full_text: bool,
    pub write: WriteMode,
}

/// The capability lattice of DD section 6.1.
pub fn capabilities_of(level: Level) -> Capabilities {
    match level {
        Level::L0 => Capabilities {
            rank: 0,
            has_text: false,
            full_text: false,
            write: WriteMode::None,
        },
        Level::L1 => Capabilities {
            rank: 1,
            has_text: true,
            full_text: false,
            write: WriteMode::RangeCheckedInsert,
        },
        Level::L2 => Capabilities {
            rank: 2,
            has_text: true,
            full_text: false,
            write: WriteMode::VerifiedSelection,
        },
        Level::L3 => Capabilities {
            rank: 3,
            has_text: true,
            full_text: true,
            write: WriteMode::BridgeTransaction,
        },
        Level::L4 => Capabilities {
            rank: 4,
            has_text: true,
            full_text: true,
            write: WriteMode::None,
        },
    }
}

/// Unknown level text is treated as L0 (no text), like the reference engine.
pub fn capabilities_of_str(level: &str) -> Capabilities {
    capabilities_of(Level::parse(level).unwrap_or(Level::L0))
}

/// Whole-file writes require L3. L4 (disk) is preview only; L1/L2 never claim the whole file (UT-025/030).
pub fn assert_full_file_writable(level: Level) -> Result<()> {
    if level == Level::L3 {
        return Ok(());
    }
    if level == Level::L4 {
        fail(
            "FULL_CONTEXT_REQUIRED",
            "a file read from disk is preview-only and cannot be written back",
        )
    } else {
        fail(
            "FULL_CONTEXT_REQUIRED",
            "full-buffer context (L3) from a trusted editor bridge is required",
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Excerpt {
    pub text: String,
    /// UTF-16 offsets into the original text.
    pub start: usize,
    pub end: usize,
    pub cursor_in_excerpt: usize,
    pub truncated: bool,
}

/// How much text around the cursor a grant allows (UTF-16 code units).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub before: usize,
    pub after: usize,
}

impl Policy {
    pub const fn new(before: usize, after: usize) -> Policy {
        Policy { before, after }
    }
}

/// Minimal nearby excerpt (L1). Bounds never split a surrogate pair or a CRLF; a cursor that itself splits one
/// is rejected (stricter than the reference engine, which would return a malformed slice).
pub fn excerpt(text: &str, cursor_offset: usize, before: usize, after: usize) -> Result<Excerpt> {
    let t = units(text);
    let cursor = cursor_offset.min(t.len());
    if !is_boundary_units(&t, cursor) {
        return fail(
            "TARGET_RANGE_INVALID",
            "cursor splits a surrogate pair or CRLF",
        );
    }
    let mut start = cursor.saturating_sub(before);
    let mut end = (cursor.saturating_add(after)).min(t.len());
    while start < cursor && !is_boundary_units(&t, start) {
        start += 1;
    }
    while end > cursor && !is_boundary_units(&t, end) {
        end -= 1;
    }
    let slice = String::from_utf16(&t[start..end])
        .or_else(|_| fail("TARGET_RANGE_INVALID", "excerpt would split a character"))?;
    Ok(Excerpt {
        text: slice,
        start,
        end,
        cursor_in_excerpt: cursor - start,
        truncated: start > 0 || end < t.len(),
    })
}

/// A wider request than the granted bounds is refused rather than silently trimmed.
pub fn require_within_policy(requested: Policy, policy: Policy) -> Result<()> {
    if requested.before > policy.before || requested.after > policy.after {
        return fail(
            "GRANT_DENIED",
            "requested range is wider than the authorised scope",
        );
    }
    Ok(())
}

/// The only way context text leaves the buffer at L1: check the request against the grant, then cut.
pub fn fetch_nearby(
    text: &str,
    cursor_offset: usize,
    requested: Policy,
    policy: Policy,
) -> Result<Excerpt> {
    require_within_policy(requested, policy)?;
    excerpt(text, cursor_offset, requested.before, requested.after)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ut_025_capability_lattice() {
        let l0 = capabilities_of(Level::L0);
        assert!(
            !l0.has_text && !l0.full_text && l0.write == WriteMode::None,
            "L0 carries no text"
        );
        let l1 = capabilities_of(Level::L1);
        assert!(l1.has_text && !l1.full_text && l1.write == WriteMode::RangeCheckedInsert);
        let l2 = capabilities_of(Level::L2);
        assert!(l2.has_text && !l2.full_text && l2.write == WriteMode::VerifiedSelection);
        let l3 = capabilities_of(Level::L3);
        assert!(
            l3.full_text && l3.write == WriteMode::BridgeTransaction,
            "only L3 has bridge full-file write"
        );
        let l4 = capabilities_of(Level::L4);
        assert!(
            l4.full_text && l4.write == WriteMode::None,
            "L4 is read-only in v1"
        );
        let writers: Vec<Level> = Level::ALL
            .into_iter()
            .filter(|l| capabilities_of(*l).write == WriteMode::BridgeTransaction)
            .collect();
        assert_eq!(writers, vec![Level::L3]);
        let ranks: Vec<u8> = Level::ALL
            .iter()
            .map(|l| capabilities_of(*l).rank)
            .collect();
        assert_eq!(ranks, vec![0, 1, 2, 3, 4]);
        assert_eq!(capabilities_of_str("L9"), capabilities_of(Level::L0));
        for l in Level::ALL {
            assert_eq!(Level::parse(l.as_str()), Some(l));
        }
    }

    #[test]
    fn ut_027_only_the_allowed_nearby_fragment_is_returned() {
        let text: String = "abcdefghij".repeat(30); // 300 units
        let policy = Policy::new(80, 80);
        let e = fetch_nearby(&text, 150, Policy::new(80, 80), policy).unwrap();
        assert_eq!((e.start, e.end), (70, 230));
        assert_eq!(e.text.len(), 160);
        assert_eq!(e.cursor_in_excerpt, 80);
        assert!(e.truncated);
        let smaller = fetch_nearby(&text, 150, Policy::new(10, 5), policy).unwrap();
        assert_eq!(smaller.text.len(), 15);
        assert_eq!((smaller.start, smaller.end), (140, 155));
    }

    #[test]
    fn ut_027_wider_requests_are_refused_not_trimmed() {
        let text = "x".repeat(500);
        let policy = Policy::new(80, 80);
        for wide in [
            Policy::new(81, 0),
            Policy::new(0, 81),
            Policy::new(1800, 900),
        ] {
            assert_eq!(
                fetch_nearby(&text, 250, wide, policy).unwrap_err().code,
                "GRANT_DENIED"
            );
        }
        assert!(fetch_nearby(&text, 250, Policy::new(80, 80), policy).is_ok());
    }

    #[test]
    fn ut_027_excerpt_never_splits_surrogates_or_crlf() {
        let text = "a\u{1F600}b\r\nc";
        // cursor after "a", before the emoji; 'before'=0, 'after'=2 would end in the middle of the emoji's pair at 3? (a=1, emoji=2 units)
        let e = excerpt(text, 1, 5, 1).unwrap();
        assert_eq!(
            e.text, "a",
            "end boundary backs off from the middle of the pair"
        );
        let e = excerpt(text, 3, 1, 5).unwrap();
        assert!(e.text.starts_with('b') || e.text.starts_with('\u{1F600}'));
        assert_eq!(
            excerpt(text, 2, 1, 1).unwrap_err().code,
            "TARGET_RANGE_INVALID",
            "cursor inside a pair"
        );
        let crlf = excerpt("ab\r\ncd", 2, 0, 1).unwrap();
        assert_eq!(crlf.text, "", "the end may not stop between CR and LF");
    }

    #[test]
    fn ut_030_disk_content_is_preview_only() {
        assert!(assert_full_file_writable(Level::L3).is_ok());
        let e = assert_full_file_writable(Level::L4).unwrap_err();
        assert_eq!(e.code, "FULL_CONTEXT_REQUIRED");
        assert!(e.message.contains("preview-only"));
        for l in [Level::L0, Level::L1, Level::L2] {
            assert_eq!(
                assert_full_file_writable(l).unwrap_err().code,
                "FULL_CONTEXT_REQUIRED"
            );
        }
    }
}
