//! Line-level diff and hunk handling for F1 whole-file changes (FR-033/035, DD section 9.2), port of `core/diff.js`.
//! Hunks are non-overlapping replacements against the exact baseline text; selecting a subset rebuilds a new,
//! re-verifiable result.
//!
//! The results are identical to the JavaScript implementation, including the offsets: every offset and every
//! line is computed over UTF-16 code units (the unit of the editor protocol), lines keep their terminator so a
//! change of EOL style or of the trailing newline is a real difference, and a middle part larger than
//! [`MAX_CELLS`] LCS cells falls back to one replace hunk.

use crate::error::{fail, Result};
use crate::textrange::{self, Edit};

/// Largest `n * m` for which the LCS table is built (16 MB of `u32`).
pub const MAX_CELLS: usize = 4_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub id: String,
    pub start_offset: usize,
    pub end_offset: usize,
    pub old_text: String,
    pub new_text: String,
    pub old_start_line: usize,
    pub old_line_count: usize,
    pub new_start_line: usize,
    pub new_line_count: usize,
}

/// One line as a `[start, end)` range of UTF-16 units, terminator included.
#[derive(Clone, Copy, Debug)]
struct Line {
    start: usize,
    end: usize,
}

/// `text.match(/[^\r\n]*(?:\r\n|\r|\n)|[^\r\n]+$/g)` with offsets.
fn split_lines_u(t: &[u16]) -> Vec<Line> {
    let n = t.len();
    let mut lines = Vec::new();
    let mut i = 0;
    while i < n {
        let start = i;
        while i < n && t[i] != 0x0A && t[i] != 0x0D {
            i += 1;
        }
        if i < n {
            i += if t[i] == 0x0D && i + 1 < n && t[i + 1] == 0x0A {
                2
            } else {
                1
            };
        }
        lines.push(Line { start, end: i });
    }
    lines
}

/// The text of every line (terminator included).
pub fn split_lines(text: &str) -> Vec<String> {
    let t = textrange::units(text);
    split_lines_u(&t)
        .iter()
        .map(|l| String::from_utf16_lossy(&t[l.start..l.end]))
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Keep,
    Del,
    Add,
}

/// LCS over the part that differs after trimming common head and tail; `None` = too small to matter or too large.
fn diff_middle(a: &[&[u16]], b: &[&[u16]]) -> Option<Vec<Op>> {
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 || n * m > MAX_CELLS {
        return None;
    }
    let w = m + 1;
    let mut table = vec![0u32; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * w + j] = if a[i] == b[j] {
                table[(i + 1) * w + j + 1] + 1
            } else {
                table[(i + 1) * w + j].max(table[i * w + j + 1])
            };
        }
    }
    let mut ops = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push(Op::Keep);
            i += 1;
            j += 1;
        } else if table[(i + 1) * w + j] >= table[i * w + j + 1] {
            ops.push(Op::Del);
            i += 1;
        } else {
            ops.push(Op::Add);
            j += 1;
        }
    }
    ops.extend(std::iter::repeat_n(Op::Del, n - i));
    ops.extend(std::iter::repeat_n(Op::Add, m - j));
    Some(ops)
}

struct Open {
    a0: usize,
    a1: usize,
    b0: usize,
    b1: usize,
}

/// Non-overlapping hunks that turn `before` into `after` (empty when the texts are equal).
pub fn build_hunks(before: &str, after: &str) -> Vec<Hunk> {
    if before == after {
        return Vec::new();
    }
    let (tb, ta_) = (textrange::units(before), textrange::units(after));
    let (a, b) = (split_lines_u(&tb), split_lines_u(&ta_));
    let ta: Vec<&[u16]> = a.iter().map(|l| &tb[l.start..l.end]).collect();
    let tbl: Vec<&[u16]> = b.iter().map(|l| &ta_[l.start..l.end]).collect();
    let mut head = 0;
    while head < ta.len() && head < tbl.len() && ta[head] == tbl[head] {
        head += 1;
    }
    let mut tail = 0;
    while tail < ta.len() - head
        && tail < tbl.len() - head
        && ta[ta.len() - 1 - tail] == tbl[tbl.len() - 1 - tail]
    {
        tail += 1;
    }
    let mid_a = &ta[head..ta.len() - tail];
    let mid_b = &tbl[head..tbl.len() - tail];
    // too large: one replace hunk
    let ops = diff_middle(mid_a, mid_b).unwrap_or_else(|| {
        std::iter::repeat_n(Op::Del, mid_a.len())
            .chain(std::iter::repeat_n(Op::Add, mid_b.len()))
            .collect()
    });
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut ai, mut bi) = (head, head);
    let mut cur: Option<Open> = None;
    let flush = |cur: &mut Option<Open>, hunks: &mut Vec<Hunk>| {
        let Some(c) = cur.take() else { return };
        let start = if c.a0 < a.len() {
            a[c.a0].start
        } else {
            tb.len()
        };
        let end = if c.a1 < a.len() {
            a[c.a1].start
        } else {
            tb.len()
        };
        let new_start = if c.b0 < b.len() {
            b[c.b0].start
        } else {
            ta_.len()
        };
        let new_end = if c.b1 < b.len() {
            b[c.b1].start
        } else {
            ta_.len()
        };
        hunks.push(Hunk {
            id: format!("h{}", hunks.len() + 1),
            start_offset: start,
            end_offset: end,
            old_text: String::from_utf16_lossy(&tb[start..end]),
            new_text: String::from_utf16_lossy(&ta_[new_start..new_end]),
            old_start_line: c.a0 + 1,
            old_line_count: c.a1 - c.a0,
            new_start_line: c.b0 + 1,
            new_line_count: c.b1 - c.b0,
        });
    };
    for op in ops {
        if op == Op::Keep {
            flush(&mut cur, &mut hunks);
            ai += 1;
            bi += 1;
            continue;
        }
        let c = cur.get_or_insert(Open {
            a0: ai,
            a1: ai,
            b0: bi,
            b1: bi,
        });
        if op == Op::Del {
            ai += 1;
            c.a1 = ai;
        } else {
            bi += 1;
            c.b1 = bi;
        }
    }
    flush(&mut cur, &mut hunks);
    hunks
}

fn selected<'a>(hunks: &'a [Hunk], ids: &[String]) -> Vec<&'a Hunk> {
    hunks.iter().filter(|h| ids.contains(&h.id)).collect()
}

/// Rebuilds the text for the chosen hunks; every hunk must still match the baseline it was derived from.
pub fn apply_hunks(before: &str, hunks: &[Hunk], selected_ids: &[String]) -> Result<String> {
    let t = textrange::units(before);
    let mut edits = Vec::new();
    for h in selected(hunks, selected_ids) {
        let matches = t
            .get(h.start_offset..h.end_offset)
            .is_some_and(|s| s == h.old_text.encode_utf16().collect::<Vec<_>>().as_slice());
        if !matches {
            return fail("PATCH_INVALID", "hunk does not match baseline");
        }
        edits.push(Edit {
            start: h.start_offset,
            end: h.end_offset,
            text: h.new_text.clone(),
        });
    }
    textrange::apply_edits(before, &edits)
}

/// The edits of the chosen hunks, ordered by position; overlapping hunks are refused.
pub fn hunk_edits(hunks: &[Hunk], selected_ids: &[String]) -> Result<Vec<Edit>> {
    let mut edits: Vec<Edit> = selected(hunks, selected_ids)
        .into_iter()
        .map(|h| Edit {
            start: h.start_offset,
            end: h.end_offset,
            text: h.new_text.clone(),
        })
        .collect();
    edits.sort_by_key(|e| e.start);
    for w in edits.windows(2) {
        if w[1].start < w[0].end {
            return fail("PATCH_INVALID", "overlapping hunks");
        }
    }
    Ok(edits)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    pub hunks: usize,
    pub added: usize,
    pub removed: usize,
}

pub fn stats(hunks: &[Hunk]) -> Stats {
    Stats {
        hunks: hunks.len(),
        added: hunks.iter().map(|h| h.new_line_count).sum(),
        removed: hunks.iter().map(|h| h.old_line_count).sum(),
    }
}

/// Display model with plain-text markers (`+`, `-`, space) so meaning never depends on colour alone (FR-043).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HunkView {
    pub id: String,
    pub header: String,
    pub lines: Vec<(char, String)>,
}

/// `text.replace(/\r?\n$|\r$/, '')`
fn strip_eol(s: &str) -> &str {
    s.strip_suffix("\r\n")
        .or_else(|| s.strip_suffix('\n'))
        .or_else(|| s.strip_suffix('\r'))
        .unwrap_or(s)
}

pub const DEFAULT_CONTEXT: usize = 2;

/// Renders every hunk against `before` (the baseline is split into lines once).
pub fn render_hunks(before: &str, hunks: &[Hunk], context: usize) -> Vec<HunkView> {
    let base = split_lines(before);
    hunks
        .iter()
        .map(|h| {
            let first = h.old_start_line - 1;
            let mut lines = Vec::new();
            for l in &base[first.saturating_sub(context)..first.min(base.len())] {
                lines.push((' ', strip_eol(l).to_string()));
            }
            for l in split_lines(&h.old_text) {
                lines.push(('-', strip_eol(&l).to_string()));
            }
            for l in split_lines(&h.new_text) {
                lines.push(('+', strip_eol(&l).to_string()));
            }
            let after_old = first + h.old_line_count;
            let to = base.len().min(after_old + context);
            for l in base.iter().take(to).skip(after_old) {
                lines.push((' ', strip_eol(l).to_string()));
            }
            HunkView {
                id: h.id.clone(),
                header: format!(
                    "@@ -{},{} +{},{} @@",
                    h.old_start_line, h.old_line_count, h.new_start_line, h.new_line_count
                ),
                lines,
            }
        })
        .collect()
}

pub fn render_hunk(before: &str, hunk: &Hunk, context: usize) -> HunkView {
    render_hunks(before, std::slice::from_ref(hunk), context).remove(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sha256::sha256_hex;

    fn ids(h: &[Hunk]) -> Vec<String> {
        h.iter().map(|x| x.id.clone()).collect()
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    const BEFORE: &str = "line1\r\nline2\r\nline3\r\nline4\r\nline5\r\nline6\r\nline7\r\nline8\r\n";
    const AFTER: &str = "line1\r\nLINE2\r\nline3\r\nline4\r\nL5a\r\nL5b\r\nline6\r\nline7\r\nline8";

    #[test]
    fn ut_045_hunks_replay_to_the_exact_after_hash_with_crlf_and_a_changed_trailing_newline() {
        let hunks = build_hunks(BEFORE, AFTER);
        assert_eq!(hunks.len(), 3);
        assert_eq!(
            sha256_hex(&apply_hunks(BEFORE, &hunks, &ids(&hunks)).unwrap()),
            sha256_hex(AFTER)
        );
        assert_eq!(apply_hunks(BEFORE, &hunks, &[]).unwrap(), BEFORE);
        assert_eq!(
            apply_hunks(BEFORE, &hunks, &s(&["h1"])).unwrap(),
            "line1\r\nLINE2\r\nline3\r\nline4\r\nline5\r\nline6\r\nline7\r\nline8\r\n"
        );
        assert_eq!(
            apply_hunks(BEFORE, &hunks, &s(&["h3"])).unwrap(),
            &BEFORE[..BEFORE.len() - 2],
            "only the trailing-newline change"
        );
        assert_eq!(
            stats(&hunks),
            Stats {
                hunks: 3,
                added: 4,
                removed: 3
            }
        );
        assert!(build_hunks("same\n", "same\n").is_empty());
        assert_eq!(build_hunks("", "abc\n").len(), 1);
        assert_eq!(build_hunks("abc\n", "").len(), 1);
        let r = render_hunk(BEFORE, &hunks[1], DEFAULT_CONTEXT);
        assert!(
            r.lines.iter().all(|(k, _)| [' ', '-', '+'].contains(k)),
            "plain text markers, not colour only"
        );
        assert_eq!(
            apply_hunks("different baseline\r\n", &hunks, &s(&["h1"]))
                .unwrap_err()
                .code,
            "PATCH_INVALID"
        );
    }

    #[test]
    fn hunk_geometry_matches_the_js_engine() {
        let hunks = build_hunks(BEFORE, AFTER);
        let h = &hunks[1];
        assert_eq!(h.id, "h2");
        assert_eq!((h.old_start_line, h.old_line_count), (5, 1));
        assert_eq!((h.new_start_line, h.new_line_count), (5, 2));
        assert_eq!(h.old_text, "line5\r\n");
        assert_eq!(h.new_text, "L5a\r\nL5b\r\n");
        assert_eq!((h.start_offset, h.end_offset), (28, 35));
        let view = render_hunk(BEFORE, h, DEFAULT_CONTEXT);
        assert_eq!(view.header, "@@ -5,1 +5,2 @@");
        let lines: Vec<String> = view.lines.iter().map(|(k, t)| format!("{k}{t}")).collect();
        assert_eq!(
            lines,
            [" line3", " line4", "-line5", "+L5a", "+L5b", " line6", " line7"]
        );
    }

    #[test]
    fn offsets_are_utf16_units_not_bytes_or_chars() {
        let before = "a\u{1F600}\nb\nc\n";
        let after = "a\u{1F600}\nB\nc\n";
        let h = &build_hunks(before, after)[0];
        // the first line "a😀\n" is 4 UTF-16 units (but 6 bytes and 3 chars)
        assert_eq!((h.start_offset, h.end_offset), (4, 6));
        assert_eq!(
            apply_hunks(before, std::slice::from_ref(h), &s(&["h1"])).unwrap(),
            after
        );
    }

    #[test]
    fn lone_cr_and_missing_trailing_newline_are_real_differences() {
        let h = build_hunks("a\rb\rc", "a\rb\rc\n");
        assert_eq!(h.len(), 1);
        assert_eq!(
            (h[0].old_text.as_str(), h[0].new_text.as_str()),
            ("c", "c\n")
        );
        assert_eq!(split_lines("a\r\nb\rc\nd"), ["a\r\n", "b\r", "c\n", "d"]);
        assert!(split_lines("").is_empty());
        assert_eq!(split_lines("\n\n"), ["\n", "\n"]);
    }

    #[test]
    fn insertion_at_the_end_and_deletion_of_everything() {
        let h = build_hunks("a\nb\n", "a\nb\nc\n");
        assert_eq!(h.len(), 1);
        assert_eq!((h[0].start_offset, h[0].end_offset), (4, 4));
        assert_eq!((h[0].old_start_line, h[0].old_line_count), (3, 0));
        let view = render_hunk("a\nb\n", &h[0], DEFAULT_CONTEXT);
        assert_eq!(
            view.lines,
            [
                (' ', "a".to_string()),
                (' ', "b".to_string()),
                ('+', "c".to_string())
            ]
        );
        let h = build_hunks("a\nb\n", "");
        assert_eq!(h.len(), 1);
        assert_eq!(apply_hunks("a\nb\n", &h, &ids(&h)).unwrap(), "");
    }

    #[test]
    fn a_middle_part_over_the_cell_limit_is_one_replace_hunk() {
        // 2001 x 2001 differing lines is over 4M cells: no LCS, one hunk that replaces the whole middle
        let before: String = (0..2001).map(|i| format!("old {i}\n")).collect();
        let after: String = (0..2001).map(|i| format!("new {i}\n")).collect();
        let hunks = build_hunks(&before, &after);
        assert_eq!(hunks.len(), 1);
        assert_eq!(
            (hunks[0].old_line_count, hunks[0].new_line_count),
            (2001, 2001)
        );
        assert_eq!(apply_hunks(&before, &hunks, &ids(&hunks)).unwrap(), after);
        // common head and tail are still trimmed before the limit applies
        let before2 = format!("keep\n{before}end\n");
        let after2 = format!("keep\n{after}end\n");
        let h2 = build_hunks(&before2, &after2);
        assert_eq!((h2.len(), h2[0].old_start_line), (1, 2));
    }

    #[test]
    fn disjoint_hunks_make_edits_in_order_and_overlaps_are_refused() {
        let hunks = build_hunks(BEFORE, AFTER);
        let edits = hunk_edits(&hunks, &s(&["h3", "h1"])).unwrap();
        assert_eq!(edits.len(), 2);
        assert!(edits[0].start < edits[1].start);
        let h = |id: &str, a: usize, b: usize| Hunk {
            id: id.to_string(),
            start_offset: a,
            end_offset: b,
            old_text: String::new(),
            new_text: String::new(),
            old_start_line: 1,
            old_line_count: 0,
            new_start_line: 1,
            new_line_count: 0,
        };
        assert_eq!(
            hunk_edits(&[h("a", 0, 5), h("b", 3, 7)], &s(&["a", "b"]))
                .unwrap_err()
                .code,
            "PATCH_INVALID"
        );
        assert!(hunk_edits(&[h("a", 0, 5), h("b", 5, 7)], &s(&["a", "b"])).is_ok());
        assert!(hunk_edits(&hunks, &s(&["nope"])).unwrap().is_empty());
    }
}
