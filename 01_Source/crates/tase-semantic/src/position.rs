//! Positions and offsets: **UTF-16 is the basis**, char and byte are derived views.
//!
//! Every edit of this repository is expressed in UTF-16 code units (see `pime-core/src/edit/transaction.rs`: "`edits`
//! are UTF-16 offsets into `snapshot.content`"), so the semantic layer must use the same unit. The detailed design
//! (§4.1 and §7.2 of the basic design) assumed a char-based editor and a byte-based parser; both assumptions are
//! wrong for this repository, so this module is the single place where the three views are converted:
//!
//! | view | unit | used for |
//! |------|------|----------|
//! | [`Utf16Offset`] | UTF-16 code unit | every edit of this repository (the basis) |
//! | [`CharOffset`] | Unicode scalar value | a human-readable cursor, the design book |
//! | [`ByteOffset`] | UTF-8 byte | the byte span of a parser |
//!
//! The conversions are exact, never approximate:
//!
//! * an emoji is one `char` but **two** UTF-16 units and four bytes;
//! * a combining mark is one `char` and one UTF-16 unit (the *grapheme* is longer, and this module does not count
//!   graphemes);
//! * a `CRLF` is one line break of two UTF-16 units, and the offset between the `CR` and the `LF` is **not** a legal
//!   boundary;
//! * a `BOM` is one `char` and one UTF-16 unit but three bytes;
//! * an offset that would isolate a surrogate (the middle of a surrogate pair) has **no** char or byte view: every
//!   conversion returns `None` instead of a wrong answer.
//!
//! A [`Position`] is a line and a column, the column counted in UTF-16 code units (the LSP convention). The line model
//! is the VS Code one: a line ends at `\r\n`, `\r` or `\n`, and the end of line is not part of the line.

use std::fmt;

/// A zero-based position: a line and a column, the column counted in UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Position {
    /// The zero-based line.
    pub line: u32,
    /// The zero-based column, in UTF-16 code units (the LSP convention).
    pub character: u32,
}

impl Position {
    /// A position from a line and a column.
    pub const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.character)
    }
}

/// A linear offset counted in UTF-16 code units: the unit of every edit of this repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Utf16Offset(pub u32);

/// A linear offset counted in Unicode scalar values (`char`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CharOffset(pub u32);

/// A linear offset counted in UTF-8 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ByteOffset(pub u32);

impl fmt::Display for Utf16Offset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "u16:{}", self.0)
    }
}

impl fmt::Display for CharOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "char:{}", self.0)
    }
}

impl fmt::Display for ByteOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "byte:{}", self.0)
    }
}

/// A half-open range of UTF-16 offsets: `start` is included, `end` is not.
///
/// This is the range of a symbol and of an edit. It is **not** `pime_core::textrange::Range`, which is a pair of
/// line/column positions; here the offsets are linear and counted in UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Range {
    /// The first UTF-16 unit of the range.
    pub start: Utf16Offset,
    /// The UTF-16 unit just after the range.
    pub end: Utf16Offset,
}

impl Range {
    /// A range from two UTF-16 offsets.
    pub const fn new(start: u32, end: u32) -> Self {
        Self {
            start: Utf16Offset(start),
            end: Utf16Offset(end),
        }
    }

    /// The first UTF-16 unit of the range.
    pub const fn start(&self) -> u32 {
        self.start.0
    }

    /// The UTF-16 unit just after the range.
    pub const fn end(&self) -> u32 {
        self.end.0
    }

    /// The length of the range in UTF-16 code units.
    pub const fn len(&self) -> u32 {
        self.end.0.saturating_sub(self.start.0)
    }

    /// Whether the range is empty.
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the offset falls inside the range (`start <= offset < end`).
    pub const fn contains(&self, offset: Utf16Offset) -> bool {
        self.start.0 <= offset.0 && offset.0 < self.end.0
    }

    /// Whether the offset falls inside the range or exactly on its end (`start <= offset <= end`).
    pub const fn touches(&self, offset: Utf16Offset) -> bool {
        self.start.0 <= offset.0 && offset.0 <= self.end.0
    }

    /// Whether the other range lies inside this one.
    pub const fn contains_range(&self, other: Range) -> bool {
        self.start.0 <= other.start.0 && other.end.0 <= self.end.0
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start.0, self.end.0)
    }
}

/// The line table of a text: the three views of every position, computed once.
///
/// The index is built in one pass over the text and answers every conversion of the module in `O(log n)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineIndex {
    /// The UTF-16 offset of the first unit of every line.
    line_start_u16: Vec<u32>,
    /// The UTF-16 offset of the end of the content of every line (the end of line is not part of it).
    line_content_end_u16: Vec<u32>,
    /// The UTF-16 offset of every char, plus a last entry for the end of the text.
    u16_at_char: Vec<u32>,
    /// The UTF-8 byte offset of every char, plus a last entry for the end of the text.
    byte_at_char: Vec<u32>,
    /// The total length in UTF-16 code units.
    total_u16: u32,
}

impl LineIndex {
    /// Builds the line table of a text.
    pub fn new(text: &str) -> Self {
        let mut line_start_u16: Vec<u32> = vec![0];
        let mut line_content_end_u16: Vec<u32> = Vec::new();
        let mut u16_at_char: Vec<u32> = Vec::new();
        let mut byte_at_char: Vec<u32> = Vec::new();
        let mut u16_pos: u32 = 0;
        let mut byte_pos: u32 = 0;
        let mut iter = text.char_indices().peekable();
        while let Some((byte, c)) = iter.next() {
            u16_at_char.push(u16_pos);
            byte_at_char.push(byte as u32);
            if c == '\r' || c == '\n' {
                line_content_end_u16.push(u16_pos);
                let mut next_u16 = u16_pos + c.len_utf16() as u32;
                let mut next_byte = byte_pos + c.len_utf8() as u32;
                if c == '\r' && matches!(iter.peek(), Some(&(_, '\n'))) {
                    let (lf_byte, _) = iter.next().expect("the peeked line feed is there");
                    u16_at_char.push(u16_pos + 1);
                    byte_at_char.push(lf_byte as u32);
                    next_u16 += 1;
                    next_byte = lf_byte as u32 + 1;
                }
                line_start_u16.push(next_u16);
                u16_pos = next_u16;
                byte_pos = next_byte;
            } else {
                u16_pos += c.len_utf16() as u32;
                byte_pos += c.len_utf8() as u32;
            }
        }
        line_content_end_u16.push(u16_pos);
        u16_at_char.push(u16_pos);
        byte_at_char.push(byte_pos);
        Self {
            line_start_u16,
            line_content_end_u16,
            u16_at_char,
            byte_at_char,
            total_u16: u16_pos,
        }
    }

    /// The number of lines (an empty text has one line).
    pub fn line_count(&self) -> u32 {
        self.line_start_u16.len() as u32
    }

    /// The length of the text in UTF-16 code units.
    pub fn utf16_len(&self) -> u32 {
        self.total_u16
    }

    /// The length of the text in Unicode scalar values.
    pub fn char_len(&self) -> u32 {
        self.u16_at_char.len() as u32 - 1
    }

    /// The length of the text in UTF-8 bytes.
    pub fn byte_len(&self) -> u32 {
        self.byte_at_char.last().copied().unwrap_or(0)
    }

    /// The UTF-16 offset of the first unit of a line.
    pub fn line_start(&self, line: u32) -> Option<Utf16Offset> {
        self.line_start_u16
            .get(line as usize)
            .map(|&v| Utf16Offset(v))
    }

    /// The UTF-16 offset of the end of the content of a line (the end of line is not part of it).
    pub fn line_content_end(&self, line: u32) -> Option<Utf16Offset> {
        self.line_content_end_u16
            .get(line as usize)
            .map(|&v| Utf16Offset(v))
    }

    /// The number of UTF-16 code units of the content of a line.
    pub fn line_len(&self, line: u32) -> Option<u32> {
        let start = self.line_start_u16.get(line as usize)?;
        let end = self.line_content_end_u16.get(line as usize)?;
        Some(end - start)
    }

    /// The index of the char that holds the UTF-16 offset, or `None` when the offset isolates a surrogate.
    fn char_index_at_u16(&self, offset: u32) -> Option<u32> {
        self.u16_at_char.binary_search(&offset).ok().map(|i| i as u32)
    }

    /// The index of the char that holds the UTF-8 byte offset, or `None` when the offset splits a UTF-8 sequence.
    fn char_index_at_byte(&self, offset: u32) -> Option<u32> {
        self.byte_at_char.binary_search(&offset).ok().map(|i| i as u32)
    }

    /// The line that holds the UTF-16 offset (the end of line belongs to the line it ends).
    fn line_at_u16(&self, offset: u32) -> usize {
        match self.line_start_u16.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    }

    /// Whether the offset is a legal edit boundary: it splits neither a surrogate pair nor a `CRLF`.
    pub fn is_boundary(&self, offset: Utf16Offset) -> bool {
        let offset = offset.0;
        if offset > self.total_u16 {
            return false;
        }
        if self.char_index_at_u16(offset).is_none() {
            return false;
        }
        if offset > 0 {
            let line = self.line_at_u16(offset);
            if offset > self.line_content_end_u16[line] {
                return false;
            }
        }
        true
    }

    /// The position of a UTF-16 offset, or `None` when the offset is not a legal boundary.
    pub fn position_at(&self, offset: Utf16Offset) -> Option<Position> {
        if !self.is_boundary(offset) {
            return None;
        }
        let line = self.line_at_u16(offset.0);
        Some(Position {
            line: line as u32,
            character: offset.0 - self.line_start_u16[line],
        })
    }

    /// The UTF-16 offset of a position, or `None` when the position is outside the text or splits a surrogate pair.
    pub fn offset_at(&self, position: Position) -> Option<Utf16Offset> {
        let line = position.line as usize;
        let start = *self.line_start_u16.get(line)?;
        let end = *self.line_content_end_u16.get(line)?;
        if position.character > end - start {
            return None;
        }
        let offset = start + position.character;
        self.char_index_at_u16(offset)?;
        Some(Utf16Offset(offset))
    }

    /// The char offset of a UTF-16 offset, or `None` when the offset isolates a surrogate.
    pub fn char_at(&self, offset: Utf16Offset) -> Option<CharOffset> {
        if offset.0 > self.total_u16 {
            return None;
        }
        self.char_index_at_u16(offset.0).map(CharOffset)
    }

    /// The UTF-16 offset of a char offset.
    pub fn utf16_at_char(&self, offset: CharOffset) -> Option<Utf16Offset> {
        self.u16_at_char
            .get(offset.0 as usize)
            .map(|&v| Utf16Offset(v))
    }

    /// The byte offset of a UTF-16 offset, or `None` when the offset isolates a surrogate.
    pub fn byte_at(&self, offset: Utf16Offset) -> Option<ByteOffset> {
        if offset.0 > self.total_u16 {
            return None;
        }
        self.char_index_at_u16(offset.0)
            .map(|i| ByteOffset(self.byte_at_char[i as usize]))
    }

    /// The UTF-16 offset of a byte offset, or `None` when the offset splits a UTF-8 sequence.
    pub fn utf16_at_byte(&self, offset: ByteOffset) -> Option<Utf16Offset> {
        self.char_index_at_byte(offset.0)
            .map(|i| Utf16Offset(self.u16_at_char[i as usize]))
    }

    /// The char offset of a byte offset, or `None` when the offset splits a UTF-8 sequence.
    pub fn char_at_byte(&self, offset: ByteOffset) -> Option<CharOffset> {
        self.char_index_at_byte(offset.0).map(CharOffset)
    }

    /// The byte offset of a char offset.
    pub fn byte_at_char(&self, offset: CharOffset) -> Option<ByteOffset> {
        self.byte_at_char
            .get(offset.0 as usize)
            .map(|&v| ByteOffset(v))
    }
}

/// The position of a UTF-16 offset.
pub fn position_at(text: &str, offset: Utf16Offset) -> Option<Position> {
    LineIndex::new(text).position_at(offset)
}

/// The UTF-16 offset of a position.
pub fn offset_at(text: &str, position: Position) -> Option<Utf16Offset> {
    LineIndex::new(text).offset_at(position)
}

/// The char offset of a UTF-16 offset.
pub fn char_offset_at(text: &str, offset: Utf16Offset) -> Option<CharOffset> {
    LineIndex::new(text).char_at(offset)
}

/// The UTF-16 offset of a char offset.
pub fn utf16_offset_at_char(text: &str, offset: CharOffset) -> Option<Utf16Offset> {
    LineIndex::new(text).utf16_at_char(offset)
}

/// The byte offset of a UTF-16 offset.
pub fn byte_offset_at(text: &str, offset: Utf16Offset) -> Option<ByteOffset> {
    LineIndex::new(text).byte_at(offset)
}

/// The UTF-16 offset of a byte offset.
pub fn utf16_offset_at_byte(text: &str, offset: ByteOffset) -> Option<Utf16Offset> {
    LineIndex::new(text).utf16_at_byte(offset)
}

/// The char offset of a byte offset.
pub fn char_offset_at_byte(text: &str, offset: ByteOffset) -> Option<CharOffset> {
    LineIndex::new(text).char_at_byte(offset)
}

/// The byte offset of a char offset.
pub fn byte_offset_at_char(text: &str, offset: CharOffset) -> Option<ByteOffset> {
    LineIndex::new(text).byte_at_char(offset)
}

/// Whether the UTF-16 offset is a legal edit boundary.
pub fn is_boundary(text: &str, offset: Utf16Offset) -> bool {
    LineIndex::new(text).is_boundary(offset)
}

/// The length of a BOM (`U+FEFF`) at the start of a text, in UTF-16 code units (0 or 1).
///
/// The BOM is content: the first character of the first line. It is three bytes and one UTF-16 unit, so a byte offset
/// and a UTF-16 offset of the same place are never the same number after it.
pub fn bom_len(text: &str) -> u32 {
    if text.starts_with('\u{feff}') { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIXED: &str = "A\u{1f600}B\n\u{6c49}\u{5b57}\r\nend";

    #[test]
    fn ascii_positions_are_the_offsets() {
        let text = "fn a() {}\nfn b() {}\n";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 3);
        assert_eq!(index.position_at(Utf16Offset(0)), Some(Position::new(0, 0)));
        assert_eq!(index.position_at(Utf16Offset(3)), Some(Position::new(0, 3)));
        assert_eq!(index.position_at(Utf16Offset(9)), Some(Position::new(0, 9)));
        assert_eq!(index.position_at(Utf16Offset(10)), Some(Position::new(1, 0)));
        assert_eq!(index.offset_at(Position::new(1, 4)), Some(Utf16Offset(14)));
        assert_eq!(index.offset_at(Position::new(0, 10)), None);
        assert_eq!(index.offset_at(Position::new(3, 0)), None);
    }

    #[test]
    fn the_line_end_is_not_part_of_the_line() {
        let text = "ab\r\ncd";
        let index = LineIndex::new(text);
        assert_eq!(index.line_len(0), Some(2));
        assert_eq!(index.line_len(1), Some(2));
        assert_eq!(index.offset_at(Position::new(0, 2)), Some(Utf16Offset(2)));
        assert_eq!(index.position_at(Utf16Offset(2)), Some(Position::new(0, 2)));
        assert_eq!(index.position_at(Utf16Offset(4)), Some(Position::new(1, 0)));
        assert_eq!(index.position_at(Utf16Offset(5)), Some(Position::new(1, 1)));
    }

    #[test]
    fn a_crlf_is_never_split() {
        let text = "a\r\nb";
        let index = LineIndex::new(text);
        assert!(!index.is_boundary(Utf16Offset(2)));
        assert!(index.is_boundary(Utf16Offset(1)));
        assert!(index.is_boundary(Utf16Offset(3)));
        assert_eq!(index.position_at(Utf16Offset(2)), None);
        assert_eq!(index.position_at(Utf16Offset(3)), Some(Position::new(1, 0)));
    }

    #[test]
    fn an_emoji_is_two_utf16_units_and_four_bytes() {
        let text = "A\u{1f600}B";
        let index = LineIndex::new(text);
        assert_eq!(index.utf16_len(), 4);
        assert_eq!(index.char_len(), 3);
        assert_eq!(index.byte_len(), 6);
        // the middle of the surrogate pair has no char view and no byte view
        assert!(!index.is_boundary(Utf16Offset(2)));
        assert_eq!(index.char_at(Utf16Offset(2)), None);
        assert_eq!(index.byte_at(Utf16Offset(2)), None);
        assert_eq!(index.position_at(Utf16Offset(2)), None);
        assert_eq!(index.char_at(Utf16Offset(1)), Some(CharOffset(1)));
        assert_eq!(index.byte_at(Utf16Offset(1)), Some(ByteOffset(1)));
        assert_eq!(index.byte_at(Utf16Offset(3)), Some(ByteOffset(5)));
        assert_eq!(index.offset_at(Position::new(0, 3)), Some(Utf16Offset(3)));
        assert_eq!(index.offset_at(Position::new(0, 2)), None);
    }

    #[test]
    fn a_combining_mark_is_one_char_and_one_utf16_unit() {
        let text = "e\u{301}x";
        let index = LineIndex::new(text);
        assert_eq!(index.utf16_len(), 3);
        assert_eq!(index.char_len(), 3);
        assert_eq!(index.byte_len(), 4);
        assert_eq!(index.char_at(Utf16Offset(1)), Some(CharOffset(1)));
        assert_eq!(index.byte_at(Utf16Offset(1)), Some(ByteOffset(1)));
        assert_eq!(index.byte_at(Utf16Offset(2)), Some(ByteOffset(3)));
        assert_eq!(index.utf16_at_char(CharOffset(2)), Some(Utf16Offset(2)));
        assert_eq!(index.byte_at_char(CharOffset(2)), Some(ByteOffset(3)));
    }

    #[test]
    fn a_chinese_line_keeps_its_columns() {
        let text = "\u{6c49}\u{5b57}\u{6c49}\u{5b57}";
        let index = LineIndex::new(text);
        assert_eq!(index.utf16_len(), 4);
        assert_eq!(index.byte_len(), 12);
        assert_eq!(index.position_at(Utf16Offset(2)), Some(Position::new(0, 2)));
        assert_eq!(index.byte_at(Utf16Offset(2)), Some(ByteOffset(6)));
        assert_eq!(index.utf16_at_byte(ByteOffset(6)), Some(Utf16Offset(2)));
        assert_eq!(index.char_at_byte(ByteOffset(6)), Some(CharOffset(2)));
    }

    #[test]
    fn a_bom_is_content_and_three_bytes() {
        let text = "\u{feff}ab";
        let index = LineIndex::new(text);
        assert_eq!(bom_len(text), 1);
        assert_eq!(bom_len("ab"), 0);
        assert_eq!(index.utf16_len(), 3);
        assert_eq!(index.byte_len(), 5);
        assert_eq!(index.char_at(Utf16Offset(1)), Some(CharOffset(1)));
        assert_eq!(index.byte_at(Utf16Offset(1)), Some(ByteOffset(3)));
        assert_eq!(index.position_at(Utf16Offset(0)), Some(Position::new(0, 0)));
        assert_eq!(index.offset_at(Position::new(0, 1)), Some(Utf16Offset(1)));
    }

    #[test]
    fn a_byte_offset_inside_a_utf8_sequence_has_no_char_view() {
        let text = "\u{6c49}";
        let index = LineIndex::new(text);
        assert_eq!(index.byte_len(), 3);
        assert_eq!(index.char_at_byte(ByteOffset(1)), None);
        assert_eq!(index.utf16_at_byte(ByteOffset(1)), None);
        assert_eq!(index.char_at_byte(ByteOffset(3)), Some(CharOffset(1)));
        assert_eq!(index.utf16_at_byte(ByteOffset(3)), Some(Utf16Offset(1)));
    }

    #[test]
    fn a_mixed_text_round_trips_on_every_boundary() {
        let index = LineIndex::new(MIXED);
        for offset in 0..=index.utf16_len() {
            if !index.is_boundary(Utf16Offset(offset)) {
                continue;
            }
            let position = index.position_at(Utf16Offset(offset)).expect("a boundary has a position");
            assert_eq!(index.offset_at(position), Some(Utf16Offset(offset)), "offset {offset}");
            let char_offset = index.char_at(Utf16Offset(offset)).expect("a boundary has a char");
            assert_eq!(index.utf16_at_char(char_offset), Some(Utf16Offset(offset)));
            let byte_offset = index.byte_at(Utf16Offset(offset)).expect("a boundary has a byte");
            assert_eq!(index.utf16_at_byte(byte_offset), Some(Utf16Offset(offset)));
            assert_eq!(index.char_at_byte(byte_offset), Some(char_offset));
        }
    }

    #[test]
    fn the_lines_of_a_mixed_text_are_the_expected_ones() {
        let index = LineIndex::new(MIXED);
        assert_eq!(index.line_count(), 3);
        assert_eq!(index.line_len(0), Some(4));
        assert_eq!(index.line_len(1), Some(2));
        assert_eq!(index.line_len(2), Some(3));
        assert_eq!(index.position_at(Utf16Offset(3)), Some(Position::new(0, 3)));
        assert_eq!(index.position_at(Utf16Offset(4)), Some(Position::new(0, 4)));
        assert_eq!(index.position_at(Utf16Offset(5)), Some(Position::new(1, 0)));
        assert_eq!(index.position_at(Utf16Offset(7)), Some(Position::new(1, 2)));
        // the offset between the CR and the LF belongs to no line
        assert_eq!(index.position_at(Utf16Offset(8)), None);
        assert_eq!(index.position_at(Utf16Offset(9)), Some(Position::new(2, 0)));
    }

    #[test]
    fn the_one_shot_functions_agree_with_the_index() {
        let index = LineIndex::new(MIXED);
        for offset in 0..=index.utf16_len() {
            let u16 = Utf16Offset(offset);
            assert_eq!(position_at(MIXED, u16), index.position_at(u16));
            assert_eq!(char_offset_at(MIXED, u16), index.char_at(u16));
            assert_eq!(byte_offset_at(MIXED, u16), index.byte_at(u16));
        }
        assert_eq!(offset_at(MIXED, Position::new(1, 1)), Some(Utf16Offset(6)));
        assert_eq!(utf16_offset_at_char(MIXED, CharOffset(1)), Some(Utf16Offset(1)));
        assert_eq!(utf16_offset_at_byte(MIXED, ByteOffset(1)), Some(Utf16Offset(1)));
        assert_eq!(char_offset_at_byte(MIXED, ByteOffset(1)), Some(CharOffset(1)));
        assert_eq!(byte_offset_at_char(MIXED, CharOffset(1)), Some(ByteOffset(1)));
        assert!(is_boundary(MIXED, Utf16Offset(1)));
        assert!(!is_boundary(MIXED, Utf16Offset(2)));
    }

    #[test]
    fn an_empty_text_has_one_empty_line() {
        let index = LineIndex::new("");
        assert_eq!(index.line_count(), 1);
        assert_eq!(index.utf16_len(), 0);
        assert_eq!(index.position_at(Utf16Offset(0)), Some(Position::new(0, 0)));
        assert_eq!(index.offset_at(Position::new(0, 0)), Some(Utf16Offset(0)));
        assert_eq!(index.offset_at(Position::new(0, 1)), None);
        assert_eq!(index.offset_at(Position::new(1, 0)), None);
    }

    #[test]
    fn a_range_knows_its_bounds() {
        let range = Range::new(4, 9);
        assert_eq!(range.start(), 4);
        assert_eq!(range.end(), 9);
        assert_eq!(range.len(), 5);
        assert!(!range.is_empty());
        assert!(range.contains(Utf16Offset(4)));
        assert!(range.contains(Utf16Offset(8)));
        assert!(!range.contains(Utf16Offset(9)));
        assert!(range.touches(Utf16Offset(9)));
        assert!(range.contains_range(Range::new(5, 8)));
        assert!(!range.contains_range(Range::new(5, 10)));
    }
}
