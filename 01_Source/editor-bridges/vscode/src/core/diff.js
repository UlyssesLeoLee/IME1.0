'use strict';
// Line-level diff and hunk handling for F1 full-file changes (FR-033/035, DD §9.2). Hunks are non-overlapping
// replacements against the exact baseline text; selecting a subset rebuilds a new, re-verifiable result.
const {fail} = require('./errors');
const {applyEdits} = require('./textrange');

// Lines keep their terminator so a change of EOL style or of the trailing newline shows up as a real difference.
function splitLines(text) {
  const lines = [];
  const re = /[^\r\n]*(?:\r\n|\r|\n)|[^\r\n]+$/g;
  let m, offset = 0;
  while ((m = re.exec(text)) && m[0] !== '') { lines.push({text: m[0], start: offset}); offset += m[0].length; }
  return lines;
}
const MAX_CELLS = 4_000_000;

function diffMiddle(a, b) { // LCS over the part that differs after trimming common head/tail
  const n = a.length, m = b.length;
  if (n === 0 || m === 0 || n * m > MAX_CELLS) return null;
  const w = m + 1, table = new Uint32Array((n + 1) * w);
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--)
      table[i * w + j] = a[i] === b[j] ? table[(i + 1) * w + j + 1] + 1 : Math.max(table[(i + 1) * w + j], table[i * w + j + 1]);
  const ops = [];
  let i = 0, j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) { ops.push('=');i++; j++; }
    else if (table[(i + 1) * w + j] >= table[i * w + j + 1]) { ops.push('-'); i++; }
    else { ops.push('+'); j++; }
  }
  while (i++ < n) ops.push('-');
  while (j++ < m) ops.push('+');
  return ops;
}

// Returns hunks [{id, startOffset, endOffset, oldText, newText, oldStartLine, oldLineCount, newStartLine, newLineCount}]
function buildHunks(before, after) {
  if (before === after) return [];
  const A = splitLines(before), B = splitLines(after);
  const ta = A.map(x => x.text), tb = B.map(x => x.text);
  let head = 0;
  while (head < ta.length && head < tb.length && ta[head] === tb[head]) head++;
  let tail = 0;
  while (tail < ta.length - head && tail < tb.length - head && ta[ta.length - 1 - tail] === tb[tb.length - 1 - tail]) tail++;
  const midA = ta.slice(head, ta.length - tail), midB = tb.slice(head, tb.length - tail);
  const ops = diffMiddle(midA, midB) || [...midA.map(() => '-'), ...midB.map(() => '+')]; // too large: one replace hunk
  const hunks = [];
  let ai = head, bi = head, cur = null;
  const flush = () => {
    if (!cur) return;
    const startOffset = cur.a0 < A.length ? A[cur.a0].start : before.length;
    const endOffset = cur.a1 < A.length ? A[cur.a1].start : before.length;
    const newStart = cur.b0 < B.length ? B[cur.b0].start : after.length;
    const newEnd = cur.b1 < B.length ? B[cur.b1].start : after.length;
    hunks.push({
      id: 'h' + (hunks.length + 1), startOffset, endOffset,
      oldText: before.slice(startOffset, endOffset), newText: after.slice(newStart, newEnd),
      oldStartLine: cur.a0 + 1, oldLineCount: cur.a1 - cur.a0, newStartLine: cur.b0 + 1, newLineCount: cur.b1 - cur.b0
    });
    cur = null;
  };
  for (const op of ops) {
    if (op === '=') { flush(); ai++; bi++; continue; }
    if (!cur) cur = {a0: ai, a1: ai, b0: bi, b1: bi};
    if (op === '-') { ai++; cur.a1 = ai; } else { bi++; cur.b1 = bi; }
  }
  flush();
  return hunks;
}
function selectedHunks(hunks, selectedIds) {
  const wanted = new Set(selectedIds);
  return hunks.filter(h => wanted.has(h.id));
}
// Rebuild the text for the chosen hunks; every hunk must still match the baseline it was derived from.
function applyHunks(before, hunks, selectedIds) {
  const chosen = selectedHunks(hunks, selectedIds);
  const edits = chosen.map(h => {
    if (before.slice(h.startOffset, h.endOffset) !== h.oldText) throw fail('PATCH_INVALID', 'hunk does not match baseline');
    return {start: h.startOffset, end: h.endOffset, text: h.newText};
  });
  return applyEdits(before, edits);
}
function hunkEdits(hunks, selectedIds) {
  const edits = selectedHunks(hunks, selectedIds).map(h => ({start: h.startOffset, end: h.endOffset, text: h.newText})).sort((a, b) => a.start - b.start);
  for (let i = 1; i < edits.length; i++) if (edits[i].start < edits[i - 1].end) throw fail('PATCH_INVALID', 'overlapping hunks');
  return edits;
}
function stats(hunks) {
  let added = 0, removed = 0;
  for (const h of hunks) { added += h.newLineCount; removed += h.oldLineCount; }
  return {hunks: hunks.length, added, removed};
}
// Display model with plain-text markers (+/-/space) so meaning never depends on colour alone (FR-043).
function renderHunk(before, hunk, context = 2) {
  const A = splitLines(before);
  const lines = [];
  const from = Math.max(0, hunk.oldStartLine - 1 - context);
  for (let i = from; i < hunk.oldStartLine - 1; i++) lines.push({kind: ' ', text: A[i].text.replace(/\r?\n$|\r$/, '')});
  for (const l of splitLines(hunk.oldText)) lines.push({kind: '-', text: l.text.replace(/\r?\n$|\r$/, '')});
  for (const l of splitLines(hunk.newText)) lines.push({kind: '+', text: l.text.replace(/\r?\n$|\r$/, '')});
  const to = Math.min(A.length, hunk.oldStartLine - 1 + hunk.oldLineCount + context);
  for (let i = hunk.oldStartLine - 1 + hunk.oldLineCount; i < to; i++) lines.push({kind: ' ', text: A[i].text.replace(/\r?\n$|\r$/, '')});
  return {
    id: hunk.id,
    header: `@@ -${hunk.oldStartLine},${hunk.oldLineCount} +${hunk.newStartLine},${hunk.newLineCount} @@`,
    lines
  };
}
module.exports = {splitLines, buildHunks, applyHunks, selectedHunks, hunkEdits, stats, renderHunk};
