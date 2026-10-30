'use strict';
// UTF-16 positions/ranges (DD §6.3). Positions are zero-based (line, character) in UTF-16 code units,
// ranges are half-open and use the same line model as VS Code (\r\n, \r and \n terminate lines).
const {fail} = require('./errors');

const EOL = /\r\n|\r|\n/g;
function lineStarts(text) {
  const starts = [0];
  EOL.lastIndex = 0;
  let m;
  while ((m = EOL.exec(text))) starts.push(m.index + m[0].length);
  return starts;
}
const isHigh = c => c >= 0xD800 && c <= 0xDBFF;
const isLow = c => c >= 0xDC00 && c <= 0xDFFF;
// An offset is a legal edit boundary unless it splits a surrogate pair or a CRLF.
function isBoundary(text, offset) {
  if (!Number.isInteger(offset) || offset < 0 || offset > text.length) return false;
  if (offset > 0 && offset < text.length) {
    if (isHigh(text.charCodeAt(offset - 1)) && isLow(text.charCodeAt(offset))) return false;
    if (text[offset - 1] === '\r' && text[offset] === '\n') return false;
  }
  return true;
}
function toOffset(text, pos) {
  const starts = lineStarts(text);
  if (!pos || !Number.isInteger(pos.line) || !Number.isInteger(pos.character) || pos.line < 0 || pos.line >= starts.length || pos.character < 0)
    throw fail('TARGET_RANGE_INVALID', 'position outside document');
  const last = pos.line + 1 >= starts.length;
  const content = last ? text.length - starts[pos.line] : starts[pos.line + 1] - eolLength(text, starts[pos.line + 1]) - starts[pos.line];
  if (pos.character > content) throw fail('TARGET_RANGE_INVALID', 'character beyond line end');
  const offset = starts[pos.line] + pos.character;
  if (!isBoundary(text, offset)) throw fail('TARGET_RANGE_INVALID', 'position splits a surrogate pair');
  return offset;
}
function eolLength(text, nextLineStart) {
  if (text[nextLineStart - 2] === '\r' && text[nextLineStart - 1] === '\n') return 2;
  return 1;
}
function fromOffset(text, offset) {
  if (!isBoundary(text, offset)) throw fail('TARGET_RANGE_INVALID', 'offset is not a character boundary');
  const starts = lineStarts(text);
  let lo = 0, hi = starts.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (starts[mid] <= offset) lo = mid; else hi = mid - 1;
  }
  return {line: lo, character: offset - starts[lo]};
}
function rangeToOffsets(text, range) {
  const start = toOffset(text, range && range.start), end = toOffset(text, range && range.end);
  if (start > end) throw fail('TARGET_RANGE_INVALID', 'range start after end');
  return {start, end};
}
function validateOffsets(text, edits) {
  const sorted = [...edits].sort((a, b) => a.start - b.start || a.end - b.end);
  let prevEnd = -1, lastInsert = -1;
  for (const e of sorted) {
    if (!isBoundary(text, e.start) || !isBoundary(text, e.end) || e.start > e.end) throw fail('TARGET_RANGE_INVALID', 'invalid edit range');
    if (typeof e.text !== 'string') throw fail('PATCH_INVALID', 'edit text missing');
    if (e.start < prevEnd) throw fail('PATCH_INVALID', 'overlapping edit ranges');
    // two inserts at one offset have no defined order
    if (e.start === e.end) { if (e.start === lastInsert) throw fail('PATCH_INVALID', 'overlapping edit ranges'); lastInsert = e.start; }
    prevEnd = Math.max(prevEnd, e.end);
  }
  return sorted;
}
function applyEdits(text, edits) {
  const sorted = validateOffsets(text, edits);
  let out = '', cursor = 0;
  for (const e of sorted) { out += text.slice(cursor, e.start) + e.text; cursor = e.end; }
  return out + text.slice(cursor);
}
function eolOf(text) {
  const crlf = (text.match(/\r\n/g) || []).length;
  const lf = (text.match(/(?<!\r)\n/g) || []).length;
  const cr = (text.match(/\r(?!\n)/g) || []).length;
  const kinds = (crlf ? 1 : 0) + (lf ? 1 : 0) + (cr ? 1 : 0);
  if (kinds === 0) return 'NONE';
  if (kinds > 1) return 'MIXED';
  return crlf ? 'CRLF' : lf ? 'LF' : 'CR';
}
module.exports = {lineStarts, isBoundary, toOffset, fromOffset, rangeToOffsets, validateOffsets, applyEdits, eolOf};
