'use strict';
// Context levels L0-L4 and document snapshots (FR-013/014/015/017, DD §6.1). Whole-file access comes only from a
// trusted editor buffer (L3); a file read from disk (L4) is read-only preview and never written back.
const crypto = require('node:crypto');
const {fail} = require('./errors');
const {eolOf, isBoundary} = require('./textrange');

const LEVELS = Object.freeze({
  L0: Object.freeze({rank: 0, hasText: false, fullText: false, write: 'none'}),
  L1: Object.freeze({rank: 1, hasText: true, fullText: false, write: 'range-checked-insert'}),
  L2: Object.freeze({rank: 2, hasText: true, fullText: false, write: 'verified-selection'}),
  L3: Object.freeze({rank: 3, hasText: true, fullText: true, write: 'bridge-transaction'}),
  L4: Object.freeze({rank: 4, hasText: true, fullText: true, write: 'none'})
});
const capabilitiesOf = level => LEVELS[level] || LEVELS.L0;
const sha256 = text => crypto.createHash('sha256').update(text, 'utf8').digest('hex');

// uri/version/text come from the live editor buffer (getText), never from disk.
function makeSnapshot({uri, version, text, languageId = '', selection, isDirty = false, level = 'L3', editorInstanceId = 'editor-1', fileName = ''}) {
  if (typeof text !== 'string') throw fail('CONTEXT_UNAVAILABLE', 'no document text');
  return Object.freeze({
    editor_instance_id: editorInstanceId, document_uri: uri, document_version: version, sha256: sha256(text), hash_algorithm: 'sha256-utf8',
    language_id: languageId, file_name: fileName, eol: eolOf(text), dirty: isDirty, selection, range_unit: 'UTF16_CODE_UNITS',
    content_scope: level, encoding: 'encoding_unknown', content: text
  });
}
// Whole-file writes require L3. L4 (disk) is preview only, L1/L2 never claim the whole file (UT-025/030/053).
function assertFullFileWritable(snapshot) {
  if (!snapshot || snapshot.content_scope !== 'L3') {
    const level = snapshot && snapshot.content_scope;
    throw fail('FULL_CONTEXT_REQUIRED', level === 'L4' ? 'a file read from disk is preview-only and cannot be written back' : 'full-buffer context (L3) from a trusted editor bridge is required',
      {userAction: 'Open the file in a supported editor with the bridge enabled'});
  }
  return true;
}
// Minimal nearby excerpt (L1). Bounds never split surrogate pairs or CRLF.
function excerpt(text, cursorOffset, {before = 1800, after = 900} = {}) {
  const cursor = Math.max(0, Math.min(text.length, cursorOffset));
  let start = Math.max(0, cursor - before), end = Math.min(text.length, cursor + after);
  while (start < cursor && !isBoundary(text, start)) start++;
  while (end > cursor && !isBoundary(text, end)) end--;
  return {text: text.slice(start, end), start, end, cursorInExcerpt: cursor - start, truncated: start > 0 || end < text.length};
}
// A wider request than the granted bounds is refused rather than silently trimmed.
function requireWithinPolicy(requested, policy) {
  if (requested.before > policy.before || requested.after > policy.after) throw fail('GRANT_DENIED', 'requested range is wider than the authorised scope');
  return true;
}
module.exports = {LEVELS, capabilitiesOf, sha256, makeSnapshot, assertFullFileWritable, excerpt, requireWithinPolicy};
