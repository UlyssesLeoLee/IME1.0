//! UTF-16 positions and ranges (DD section 6.3), port of `textrange.js`.
//! Positions are zero-based `(line, character)` in UTF-16 code units, ranges are half-open, and lines end at
//! `\r\n`, `\r` or `\n` (the VS Code line model). Offsets never split a surrogate pair or a CRLF.

use crate::error::{Result, fail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub line: i64,
    pub character: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// One text edit in UTF-16 offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

pub fn pos(line: i64, character: i64) -> Position {
    Position { line, character }
}

/// The text as UTF-16 code units (the unit of every IPC position and range).
pub fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// [`is_boundary`] over pre-encoded code units (avoids re-encoding in loops).
pub fn is_boundary_units(t: &[u16], offset: usize) -> bool {
    boundary_u(t, offset)
}

fn line_starts_u(t: &[u16]) -> Vec<usize> {
    let mut starts = vec![0usize];
    let mut i = 0;
    while i < t.len() {
        if t[i] == 0x0A {
            starts.push(i + 1);
            i += 1;
        } else if t[i] == 0x0D {
            let n = if i + 1 < t.len() && t[i + 1] == 0x0A {
                2
            } else {
                1
            };
            starts.push(i + n);
            i += n;
        } else {
            i += 1;
        }
    }
    starts
}

/// Start offsets of every line (UTF-16 units).
pub fn line_starts(text: &str) -> Vec<usize> {
    line_starts_u(&units(text))
}

fn is_high(c: u16) -> bool {
    (0xD800..=0xDBFF).contains(&c)
}

fn is_low(c: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&c)
}

fn boundary_u(t: &[u16], offset: usize) -> bool {
    if offset > t.len() {
        return false;
    }
    if offset > 0 && offset < t.len() {
        if is_high(t[offset - 1]) && is_low(t[offset]) {
            return false;
        }
        if t[offset - 1] == 0x0D && t[offset] == 0x0A {
            return false;
        }
    }
    true
}

/// An offset is a legal edit boundary unless it is out of range or splits a surrogate pair or a CRLF.
pub fn is_boundary(text: &str, offset: usize) -> bool {
    boundary_u(&units(text), offset)
}

fn eol_length(t: &[u16], next_line_start: usize) -> usize {
    if next_line_start >= 2 && t[next_line_start - 2] == 0x0D && t[next_line_start - 1] == 0x0A {
        2
    } else {
        1
    }
}

fn to_offset_u(t: &[u16], starts: &[usize], p: Position) -> Result<usize> {
    if p.line < 0 || p.character < 0 || p.line as usize >= starts.len() {
        return fail("TARGET_RANGE_INVALID", "position outside document");
    }
    let line = p.line as usize;
    let last = line + 1 >= starts.len();
    let content = if last {
        t.len() - starts[line]
    } else {
        starts[line + 1] - eol_length(t, starts[line + 1]) - starts[line]
    };
    if p.character as usize > content {
        return fail("TARGET_RANGE_INVALID", "character beyond line end");
    }
    let offset = starts[line] + p.character as usize;
    if !boundary_u(t, offset) {
        return fail("TARGET_RANGE_INVALID", "position splits a surrogate pair");
    }
    Ok(offset)
}

pub fn to_offset(text: &str, p: Position) -> Result<usize> {
    let t = units(text);
    let starts = line_starts_u(&t);
    to_offset_u(&t, &starts, p)
}

pub fn from_offset(text: &str, offset: usize) -> Result<Position> {
    let t = units(text);
    if !boundary_u(&t, offset) {
        return fail("TARGET_RANGE_INVALID", "offset is not a character boundary");
    }
    let starts = line_starts_u(&t);
    let (mut lo, mut hi) = (0usize, starts.len() - 1);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if starts[mid] <= offset {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Ok(Position {
        line: lo as i64,
        character: (offset - starts[lo]) as i64,
    })
}

pub fn range_to_offsets(text: &str, range: Range) -> Result<(usize, usize)> {
    let t = units(text);
    let starts = line_starts_u(&t);
    let start = to_offset_u(&t, &starts, range.start)?;
    let end = to_offset_u(&t, &starts, range.end)?;
    if start > end {
        return fail("TARGET_RANGE_INVALID", "range start after end");
    }
    Ok((start, end))
}

fn sorted_valid(t: &[u16], edits: &[Edit]) -> Result<Vec<Edit>> {
    let mut sorted = edits.to_vec();
    sorted.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
    let mut prev_end: i64 = -1;
    let mut last_insert: i64 = -1;
    for e in &sorted {
        if !boundary_u(t, e.start) || !boundary_u(t, e.end) || e.start > e.end {
            return fail("TARGET_RANGE_INVALID", "invalid edit range");
        }
        if (e.start as i64) < prev_end {
            return fail("PATCH_INVALID", "overlapping edit ranges");
        }
        // two inserts at one offset have no defined order
        if e.start == e.end {
            if e.start as i64 == last_insert {
                return fail("PATCH_INVALID", "overlapping edit ranges");
            }
            last_insert = e.start as i64;
        }
        prev_end = prev_end.max(e.end as i64);
    }
    Ok(sorted)
}

/// Validates offsets (boundaries, order, overlaps) and returns the edits sorted by position.
pub fn validate_offsets(text: &str, edits: &[Edit]) -> Result<Vec<Edit>> {
    sorted_valid(&units(text), edits)
}

pub fn apply_edits(text: &str, edits: &[Edit]) -> Result<String> {
    let t = units(text);
    let sorted = sorted_valid(&t, edits)?;
    let mut out: Vec<u16> = Vec::with_capacity(t.len());
    let mut cursor = 0usize;
    for e in &sorted {
        out.extend_from_slice(&t[cursor..e.start]);
        out.extend(e.text.encode_utf16());
        cursor = e.end;
    }
    out.extend_from_slice(&t[cursor..]);
    String::from_utf16(&out).or_else(|_| fail("PATCH_INVALID", "edit produced malformed text"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    None,
    Lf,
    Crlf,
    Cr,
    Mixed,
}

impl Eol {
    /// The labels of the JS engine (`NONE`, `LF`, `CRLF`, `CR`, `MIXED`).
    pub fn as_str(self) -> &'static str {
        match self {
            Eol::None => "NONE",
            Eol::Lf => "LF",
            Eol::Crlf => "CRLF",
            Eol::Cr => "CR",
            Eol::Mixed => "MIXED",
        }
    }
}

pub fn eol_of(text: &str) -> Eol {
    let t = units(text);
    let (mut crlf, mut lf, mut cr) = (false, false, false);
    let mut i = 0;
    while i < t.len() {
        if t[i] == 0x0D {
            if i + 1 < t.len() && t[i + 1] == 0x0A {
                crlf = true;
                i += 1;
            } else {
                cr = true;
            }
        } else if t[i] == 0x0A {
            lf = true;
        }
        i += 1;
    }
    match (crlf, lf, cr) {
        (false, false, false) => Eol::None,
        (true, false, false) => Eol::Crlf,
        (false, true, false) => Eol::Lf,
        (false, false, true) => Eol::Cr,
        _ => Eol::Mixed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "A\u{1F600}B\n\u{6C49}\u{5B57}\r\nend";

    #[test]
    fn ut_026_positions_never_split_surrogates() {
        // line 0 is "A😀B" = 4 UTF-16 units
        assert_eq!(to_offset(SAMPLE, pos(0, 1)).unwrap(), 1);
        assert_eq!(to_offset(SAMPLE, pos(0, 3)).unwrap(), 3);
        let mid = to_offset(SAMPLE, pos(0, 2)).unwrap_err();
        assert_eq!(mid.code, "TARGET_RANGE_INVALID");
        assert!(!is_boundary(SAMPLE, 2));
        assert!(is_boundary(SAMPLE, 1) && is_boundary(SAMPLE, 3));
    }

    #[test]
    fn ut_026_out_of_range_is_rejected() {
        for p in [pos(0, 5), pos(3, 0), pos(-1, 0), pos(0, -1), pos(1, 3)] {
            assert_eq!(
                to_offset(SAMPLE, p).unwrap_err().code,
                "TARGET_RANGE_INVALID",
                "{p:?}"
            );
        }
        assert!(to_offset(SAMPLE, pos(1, 2)).is_ok()); // end of the CJK line, before CRLF
        assert!(to_offset(SAMPLE, pos(2, 3)).is_ok());
        let r = Range {
            start: pos(1, 1),
            end: pos(0, 0),
        };
        assert_eq!(
            range_to_offsets(SAMPLE, r).unwrap_err().code,
            "TARGET_RANGE_INVALID"
        );
    }

    #[test]
    fn ut_026_crlf_is_not_split() {
        let t = "a\r\nb";
        assert!(!is_boundary(t, 2));
        assert!(is_boundary(t, 1) && is_boundary(t, 3));
        assert_eq!(from_offset(t, 3).unwrap(), pos(1, 0));
        assert!(from_offset(t, 2).is_err());
    }

    #[test]
    fn ut_026_offset_round_trip() {
        for off in 0..=utf16_total(SAMPLE) {
            if is_boundary(SAMPLE, off) {
                let p = from_offset(SAMPLE, off).unwrap();
                assert_eq!(to_offset(SAMPLE, p).unwrap(), off, "offset {off}");
            }
        }
    }

    fn utf16_total(s: &str) -> usize {
        s.encode_utf16().count()
    }

    #[test]
    fn edits_validate_and_apply() {
        let e = |s, e, t: &str| Edit {
            start: s,
            end: e,
            text: t.to_string(),
        };
        assert_eq!(
            apply_edits("hello", &[e(0, 1, "J"), e(5, 5, "!")]).unwrap(),
            "Jello!"
        );
        assert_eq!(
            apply_edits("hello", &[e(0, 3, "x"), e(2, 4, "y")])
                .unwrap_err()
                .code,
            "PATCH_INVALID"
        );
        assert_eq!(
            apply_edits("hello", &[e(2, 2, "x"), e(2, 2, "y")])
                .unwrap_err()
                .code,
            "PATCH_INVALID"
        );
        assert_eq!(
            apply_edits("a\u{1F600}", &[e(2, 2, "x")]).unwrap_err().code,
            "TARGET_RANGE_INVALID"
        );
    }

    #[test]
    fn eol_detection() {
        assert_eq!(eol_of("a"), Eol::None);
        assert_eq!(eol_of("a\r\nb\r\n"), Eol::Crlf);
        assert_eq!(eol_of("a\nb"), Eol::Lf);
        assert_eq!(eol_of("a\rb"), Eol::Cr);
        assert_eq!(eol_of("a\r\nb\n"), Eol::Mixed);
    }
}
