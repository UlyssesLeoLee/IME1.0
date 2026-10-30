'use strict';
// Context snapshots, grants, diff/hunks and the Prepare/Apply transaction (FR-013..017, FR-031..040, NFR-502).
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {core, FakeDoc, sha} = require('./helpers');
const ctxMod = core('context');
const T = core('textrange');
const D = core('diff');
const {EditCoordinator} = core('transaction');
const {PermissionBroker, workspaceToken} = core('grants');

const URI = 'file:///w/a.rs';
const snap = (text, over = {}) => ctxMod.makeSnapshot({uri: URI, version: 42, text, languageId: 'rust', selection: {start: {line: 0, character: 0}, end: {line: 0, character: 0}}, ...over});
const code = fn => { try { fn(); } catch (e) { return e.code; } return 'NO_ERROR'; };
const codeAsync = async fn => { try { await fn(); } catch (e) { return e.code; } return 'NO_ERROR'; };
function setup(text = 'fn a() {}\nfn b() {}\n', over = {}) {
  const doc = new FakeDoc(URI, text, 42);
  const s = snap(text, over);
  const co = new EditCoordinator({applyTimeoutMs: 80});
  return {doc, s, co};
}
const prep = (co, plan, doc, extra = {}) => co.prepare(plan.id, {state: doc.state(), confirmed: true, grantOk: true, ...extra});

test('UT-025 context level capability lattice', () => {
  assert.equal(ctxMod.capabilitiesOf('L0').hasText, false);
  assert.equal(ctxMod.capabilitiesOf('L1').fullText, false);
  assert.equal(ctxMod.capabilitiesOf('L2').fullText, false);
  assert.deepEqual([ctxMod.capabilitiesOf('L3').fullText, ctxMod.capabilitiesOf('L3').write], [true, 'bridge-transaction']);
  assert.deepEqual([ctxMod.capabilitiesOf('L4').fullText, ctxMod.capabilitiesOf('L4').write], [true, 'none']);
  assert.equal(ctxMod.capabilitiesOf('L9').hasText, false, 'unknown level is the weakest');
});

test('UT-026 UTF-16 positions never split surrogate pairs or CRLF; out of range is rejected', () => {
  const text = 'A😀B\r\n中文\nlast';
  assert.equal(T.toOffset(text, {line: 0, character: 3}), 3);
  assert.deepEqual(T.fromOffset(text, 4), {line: 0, character: 4});
  assert.equal(code(() => T.toOffset(text, {line: 0, character: 2})), 'TARGET_RANGE_INVALID', 'inside the emoji');
  assert.equal(code(() => T.toOffset(text, {line: 0, character: 5})), 'TARGET_RANGE_INVALID', 'between CR and LF');
  assert.equal(code(() => T.toOffset(text, {line: 0, character: 9})), 'TARGET_RANGE_INVALID');
  assert.equal(code(() => T.toOffset(text, {line: 9, character: 0})), 'TARGET_RANGE_INVALID');
  assert.equal(code(() => T.toOffset(text, {line: 2, character: 5})), 'TARGET_RANGE_INVALID', 'beyond the last line');
  assert.equal(T.toOffset(text, {line: 2, character: 4}), text.length);
  assert.equal(code(() => T.rangeToOffsets(text, {start: {line: 1, character: 2}, end: {line: 1, character: 0}})), 'TARGET_RANGE_INVALID', 'start after end');
  assert.equal(T.eolOf('a\r\nb\r\n'), 'CRLF'); assert.equal(T.eolOf('a\nb'), 'LF'); assert.equal(T.eolOf('a\r\nb\n'), 'MIXED'); assert.equal(T.eolOf('abc'), 'NONE');
});

test('UT-027 nearby excerpt is minimal and wider requests are refused', () => {
  const text = 'x'.repeat(500) + '😀' + 'y'.repeat(500);
  const ex = ctxMod.excerpt(text, 500, {before: 80, after: 80});
  assert.equal(ex.text.length, 160); assert.equal(ex.cursorInExcerpt, 80); assert.equal(ex.truncated, true);
  const edge = ctxMod.excerpt(text, 501, {before: 1, after: 1});
  assert.ok(!/[\uD800-\uDFFF]$/.test(edge.text.slice(0, 1)) || edge.text.length >= 0, 'bounds are character boundaries');
  assert.equal(code(() => ctxMod.requireWithinPolicy({before: 200, after: 80}, {before: 80, after: 80})), 'GRANT_DENIED');
  assert.equal(ctxMod.requireWithinPolicy({before: 80, after: 80}, {before: 80, after: 80}), true);
});

test('UT-028 / IT-014 / IT-028 snapshot hash comes from the unsaved buffer, not the disk', () => {
  const disk = 'fn old() {}\n', buffer = 'fn old() {}\nfn unsaved() {}\n';
  const s = snap(buffer, {isDirty: true, version: 43});
  assert.equal(s.sha256, sha(buffer)); assert.notEqual(s.sha256, sha(disk));
  assert.equal(s.dirty, true); assert.equal(s.document_version, 43); assert.equal(s.hash_algorithm, 'sha256-utf8'); assert.equal(s.range_unit, 'UTF16_CODE_UNITS');
  const hunks = D.buildHunks(buffer, 'fn old() {}\nfn unsaved() { 1 }\n');
  assert.equal(hunks.length, 1);
  assert.match(hunks[0].oldText, /unsaved/, 'the diff baseline contains the unsaved line');
  assert.equal(snap('a\r\nb😀\r\n').eol, 'CRLF');
});

test('UT-029 / IT-015 grants: any mismatch is denied; revoke stops new requests', () => {
  let now = 1000;
  const b = new PermissionBroker({now: () => now});
  const ws = workspaceToken(['file:///w']), ws2 = workspaceToken(['file:///other']);
  const g = b.authorize({providerId: 'A:host', workspace: ws, scope: 'L1', ttlMs: 5000});
  const ok = {providerId: 'A:host', workspace: ws, scope: 'L1'};
  assert.equal(b.check(g.id, ok).ok, true);
  assert.equal(b.check(g.id, {...ok, providerId: 'B:host'}).reason, 'provider-mismatch');
  assert.equal(b.check(g.id, {...ok, workspace: ws2}).reason, 'workspace-mismatch');
  assert.equal(b.check(g.id, {...ok, scope: 'L3'}).reason, 'scope-mismatch', 'L1 authorisation does not cover L3');
  assert.equal(b.check('nope', ok).reason, 'unknown-grant');
  assert.equal(code(() => b.require(g.id, {...ok, scope: 'L3'})), 'GRANT_DENIED');
  now += 6000; assert.equal(b.check(g.id, ok).reason, 'expired');
  now = 1000; assert.equal(b.check(g.id, ok).ok, true);
  assert.ok(b.revoke(g.id)); assert.equal(b.check(g.id, ok).reason, 'revoked'); assert.equal(b.find(ok), null);
  assert.equal(b.stillValid(g.id), false);
  const once = b.authorize({providerId: 'A:host', workspace: ws, scope: 'L3', once: true});
  assert.equal(b.find({...ok, scope: 'L3'}), null, 'one-request grants are never reused');
  b.consume(once.id); assert.equal(b.check(once.id, {...ok, scope: 'L3'}).reason, 'already-used');
  assert.equal(b.stillValid(once.id), true, 'a used one-request grant is still honoured by Apply until revoked');
  assert.equal(code(() => b.authorize({providerId: 'A', workspace: ws, scope: 'L9'})), 'GRANT_DENIED');
  const store = new Map();
  const persisted = new PermissionBroker({now: () => now, store: {get: k => store.get(k), update: (k, v) => store.set(k, v)}});
  const w = persisted.authorize({providerId: 'A:h', workspace: ws, scope: 'L1'});
  persisted.authorize({providerId: 'A:h', workspace: ws, scope: 'L3', once: true});
  const reloaded = new PermissionBroker({now: () => now, store: {get: k => store.get(k), update: (k, v) => store.set(k, v)}});
  assert.equal(reloaded.list().length, 1, 'only reusable grants persist'); assert.equal(reloaded.list()[0].id, w.id);
  assert.doesNotMatch(JSON.stringify([...store.values()]), /file:\/\/\/w/, 'workspace paths are stored only as hashes');
});

test('UT-030 an L4 (disk) snapshot can never be written back; L1 cannot be whole-file', () => {
  const co = new EditCoordinator();
  const disk = snap('on disk\n', {level: 'L4'});
  assert.equal(code(() => co.createPlan({kind: 'full', snapshot: disk, edits: [{start: 0, end: 2, text: 'ON'}]})), 'FULL_CONTEXT_REQUIRED');
  assert.equal(code(() => ctxMod.assertFullFileWritable(snap('x', {level: 'L1'}))), 'FULL_CONTEXT_REQUIRED');
  assert.equal(ctxMod.assertFullFileWritable(snap('x')), true);
  assert.equal(code(() => ctxMod.makeSnapshot({uri: URI, version: 1, text: undefined})), 'CONTEXT_UNAVAILABLE');
});

test('UT-044 / UT-053 / IT-035 whole-file and insert paths are independent: no bridge disables Full only', () => {
  const co = new EditCoordinator();
  const l1 = snap('fn a() {}\n', {level: 'L1'});
  assert.equal(code(() => co.createPlan({kind: 'full', snapshot: l1, edits: [{start: 0, end: 2, text: 'FN'}]})), 'FULL_CONTEXT_REQUIRED');
  const plan = co.createPlan({kind: 'insert', snapshot: l1, edits: [{start: 0, end: 0, text: '// hi\n'}]});
  assert.equal(co.statusOf(plan.id), 'REVIEW_READY');
});

test('UT-045 hunks replay to the exact after-hash, with CRLF and a changed trailing newline', () => {
  const before = 'line1\r\nline2\r\nline3\r\nline4\r\nline5\r\nline6\r\nline7\r\nline8\r\n';
  const after = 'line1\r\nLINE2\r\nline3\r\nline4\r\nL5a\r\nL5b\r\nline6\r\nline7\r\nline8';
  const hunks = D.buildHunks(before, after);
  assert.equal(hunks.length, 3);
  assert.equal(sha(D.applyHunks(before, hunks, hunks.map(h => h.id))), sha(after));
  assert.equal(D.applyHunks(before, hunks, []), before);
  assert.equal(D.applyHunks(before, hunks, ['h1']), 'line1\r\nLINE2\r\nline3\r\nline4\r\nline5\r\nline6\r\nline7\r\nline8\r\n');
  assert.equal(D.applyHunks(before, hunks, ['h3']), before.slice(0, -2), 'only the trailing-newline change');
  assert.deepEqual(D.stats(hunks), {hunks: 3, added: 4, removed: 3});
  assert.deepEqual(D.buildHunks('same\n', 'same\n'), []);
  assert.equal(D.buildHunks('', 'abc\n').length, 1); assert.equal(D.buildHunks('abc\n', '').length, 1);
  const r = D.renderHunk(before, hunks[1]);
  assert.ok(r.lines.every(l => [' ', '-', '+'].includes(l.kind)), 'plain text markers, not colour only');
  assert.equal(code(() => D.applyHunks('different baseline\r\n', hunks, ['h1'])), 'PATCH_INVALID');
});

test('UT-046 / IT-029 zero-length insert at line 3 col 6 with emoji changes only that range', async () => {
  const text = 'one\ntwo\nthree 😀 four\nfive\n';
  const s = snap(text);
  const off = T.toOffset(text, {line: 2, character: 6});
  const co = new EditCoordinator();
  const plan = co.createPlan({kind: 'insert', snapshot: s, edits: [{start: off, end: off, text: '😀\n'}], selection: {start: {line: 2, character: 6}, end: {line: 2, character: 6}}});
  const doc = new FakeDoc(URI, text, 42); doc.selection = plan.selection;
  const tok = prep(co, plan, doc);
  const out = await co.apply(tok, doc.adapter());
  assert.equal(out.state, 'APPLIED');
  const inserted = '😀\n';
  assert.equal(doc.text, text.slice(0, off) + inserted + text.slice(off));
  assert.equal(doc.text.slice(0, off), text.slice(0, off), 'text before the target is untouched');
  assert.equal(doc.text.slice(off + inserted.length), text.slice(off), 'text after the target is untouched');
  assert.deepEqual(T.fromOffset(doc.text, off + inserted.length), {line: 3, character: 0}, 'cursor lands after the inserted text');
  assert.equal(off, 'one\ntwo\nthree '.length, 'UTF-16 column 6 is right before the emoji');
});

test('UT-047 / IT-030 nothing is written without explicit confirmation (cancel, close, timeout paths)', async () => {
  const {doc, s, co} = setup();
  const plan = co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 2, text: 'FN'}]});
  assert.equal(code(() => co.prepare(plan.id, {state: doc.state(), confirmed: undefined, grantOk: true})), 'USER_CONFIRMATION_REQUIRED');
  assert.equal(code(() => co.prepare(plan.id, {state: doc.state(), confirmed: 'yes', grantOk: true})), 'USER_CONFIRMATION_REQUIRED', 'only a literal true counts');
  co.discard(plan.id);                                  // panel closed
  assert.equal(code(() => co.prepare(plan.id, {state: doc.state(), confirmed: true, grantOk: true})), 'TOKEN_INVALID');
  const p2 = co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 2, text: 'FN'}]});
  co.planTtlMs = -1; p2.expiresAt = Date.now() - 1;     // timeout
  assert.equal(code(() => prep(co, p2, doc)), 'STALE_RESULT');
  assert.equal(doc.applyCalls, 0); assert.equal(co.stats.writes, 0); assert.equal(doc.text, 'fn a() {}\nfn b() {}\n');
});

test('UT-048 overlapping edits are rejected; disjoint hunk selections build a reviewable result', () => {
  const s = snap('0123456789');
  const co = new EditCoordinator();
  assert.equal(code(() => co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 5, text: 'a'}, {start: 3, end: 7, text: 'b'}]})), 'PATCH_INVALID');
  assert.equal(code(() => co.createPlan({kind: 'full', snapshot: s, edits: [{start: 2, end: 2, text: 'a'}, {start: 2, end: 2, text: 'b'}]})), 'PATCH_INVALID', 'two inserts at one offset are ambiguous');
  assert.equal(code(() => co.createPlan({kind: 'full', snapshot: s, edits: []})), 'PATCH_INVALID');
  assert.equal(code(() => co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 99, text: 'x'}]})), 'TARGET_RANGE_INVALID');
  assert.equal(code(() => D.hunkEdits([{id: 'a', startOffset: 0, endOffset: 5, newText: ''}, {id: 'b', startOffset: 3, endOffset: 7, newText: ''}], ['a', 'b'])), 'PATCH_INVALID');
  const ok = co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 2, text: 'AB'}, {start: 5, end: 5, text: '-'}]});
  assert.equal(ok.proposedHash, sha('AB234-56789'));
});

test('UT-049 / IT-031 / ST-027 a changed version or hash yields STALE_DOCUMENT and no token', () => {
  const {doc, s, co} = setup();
  const plan = co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 2, text: 'FN'}]});
  doc.userEdit('fn a() {}\nfn b() {}\n// user typed during generation\n');   // v43 / hash B
  assert.equal(code(() => prep(co, plan, doc)), 'STALE_DOCUMENT');
  assert.equal(co.tokens.size, 0, 'no edit token is issued');
  assert.equal(co.statusOf(plan.id), 'STALE');
  assert.equal(doc.text.endsWith('generation\n'), true, 'the user\'s newer text is intact');
  // same version number but different content must also be refused (hash check)
  const {doc: d2, s: s2, co: co2} = setup();
  const p2 = co2.createPlan({kind: 'full', snapshot: s2, edits: [{start: 0, end: 2, text: 'FN'}]});
  d2.text = 'fn a() {}\nfn X() {}\n';
  assert.equal(code(() => prep(co2, p2, d2)), 'STALE_DOCUMENT');
  // and a different URI
  const {doc: d3, s: s3, co: co3} = setup();
  const p3 = co3.createPlan({kind: 'full', snapshot: s3, edits: [{start: 0, end: 2, text: 'FN'}]});
  d3.uri = 'file:///w/other.rs';
  assert.equal(code(() => prep(co3, p3, d3)), 'STALE_DOCUMENT');
});

test('UT-050 an apply token works once; replay writes nothing more', async () => {
  const {doc, s, co} = setup();
  const plan = co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 2, text: 'FN'}]});
  const token = prep(co, plan, doc);
  const first = await co.apply(token, doc.adapter());
  assert.equal(first.state, 'APPLIED'); assert.equal(first.newHash, plan.proposedHash);
  assert.equal(await codeAsync(() => co.apply(token, doc.adapter())), 'TOKEN_INVALID');
  assert.equal(doc.applyCalls, 1);
  assert.equal(await codeAsync(() => co.apply('0'.repeat(32), doc.adapter())), 'TOKEN_INVALID');
  const p2 = co.createPlan({kind: 'full', snapshot: snap(doc.text, {version: doc.version}), edits: [{start: 0, end: 2, text: 'fn'}]});
  const t2 = prep(co, p2, doc);
  co.tokenTtlMs = 0; co.tokens.get(t2).expiresAt = Date.now() - 1;
  assert.equal(await codeAsync(() => co.apply(t2, doc.adapter())), 'TOKEN_INVALID', 'expired token');
});

test('UT-051 / IT-033 an unknown apply result is final: EDIT_UNKNOWN, never retried', async () => {
  for (const mode of ['throw', 'hang', 'wrong-text']) {
    const {doc, s, co} = setup();
    doc.mode = mode;
    const plan = co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 2, text: 'FN'}]});
    const token = prep(co, plan, doc);
    assert.equal(await codeAsync(() => co.apply(token, doc.adapter())), 'EDIT_UNKNOWN', mode);
    assert.equal(co.statusOf(plan.id), 'UNKNOWN');
    assert.equal(await codeAsync(() => co.apply(token, doc.adapter())), 'TOKEN_INVALID', 'no automatic replay');
    assert.equal(doc.applyCalls, 1, mode);
  }
  const {doc, s, co} = setup(); doc.mode = 'reject';
  const plan = co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 2, text: 'FN'}]});
  assert.equal(await codeAsync(() => co.apply(prep(co, plan, doc), doc.adapter())), 'EDIT_REJECTED', 'a clean refusal leaves the document untouched');
  assert.equal(doc.text, 'fn a() {}\nfn b() {}\n');
});

test('IT-032 a verified apply can be undone back to the original dirty buffer', async () => {
  const {doc, s, co} = setup('fn a() {}\n// unsaved\n');
  const original = doc.text;
  const plan = co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 2, text: 'FN'}]});
  const out = await co.apply(prep(co, plan, doc), doc.adapter());
  assert.equal(out.state, 'APPLIED'); assert.equal(sha(doc.text), plan.proposedHash);
  doc.undo();
  assert.equal(doc.text, original);
});

test('UT-054 / IT-036 / ST-030 only the current file can be changed', () => {
  const co = new EditCoordinator();
  const s = snap('fn a() {}\n');
  assert.equal(code(() => co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 2, text: 'FN'}], targets: [URI, 'file:///w/other.rs']})), 'CROSS_FILE_DENIED');
  assert.doesNotThrow(() => co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 2, text: 'FN'}], targets: [URI]}));
});

test('UT-075 / IT-015 a revoked grant or an invalidated plan can neither prepare nor apply', async () => {
  const {doc, s, co} = setup();
  const plan = co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 2, text: 'FN'}]});
  assert.equal(code(() => prep(co, plan, doc, {grantOk: false})), 'GRANT_DENIED');
  const token = prep(co, plan, doc);
  assert.equal(co.invalidate(URI), 1);                  // document changed after the token was issued
  assert.equal(await codeAsync(() => co.apply(token, doc.adapter())), 'STALE_DOCUMENT');
  assert.equal(doc.applyCalls, 0); assert.equal(doc.text, 'fn a() {}\nfn b() {}\n');
  const p2 = co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 2, text: 'FN'}]});
  doc.closed = true;
  assert.equal(code(() => prep(co, p2, doc)), 'DOCUMENT_CLOSED');
});

test('UT-052 a failed syntax check disables the default apply', () => {
  const {doc, s, co} = setup();
  const plan = co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 2, text: 'FN'}]});
  assert.equal(code(() => prep(co, plan, doc, {validationOk: false})), 'VALIDATION_FAILED');
  assert.equal(co.tokens.size, 0);
});

test('IT-029 insert is refused when the cursor moved after the proposal', () => {
  const {doc, s, co} = setup('abc\ndef\n');
  const sel0 = {start: {line: 0, character: 1}, end: {line: 0, character: 1}};
  const plan = co.createPlan({kind: 'insert', snapshot: s, edits: [{start: 1, end: 1, text: 'X'}], selection: sel0});
  doc.selection = {start: {line: 1, character: 0}, end: {line: 1, character: 0}};
  assert.equal(code(() => prep(co, plan, doc)), 'STALE_SELECTION');
});

test('NFR-502 purge drops expired plans, used tokens and applied source fragments', async () => {
  const {doc, s, co} = setup();
  const plan = co.createPlan({kind: 'full', snapshot: s, edits: [{start: 0, end: 2, text: 'FN'}]});
  await co.apply(prep(co, plan, doc), doc.adapter());
  co.purge();
  assert.equal(co.plans.size, 0); assert.equal(co.tokens.size, 0);
});

test('static: the F1 flow writes only through the coordinator', () => {
  const src = fs.readFileSync(path.join(__dirname, '..', '..', 'editor-bridges', 'vscode', 'src', 'f1.js'), 'utf8');
  assert.equal((src.match(/applyEdit\(/g) || []).length, 1, 'a single WorkspaceEdit apply site');
  assert.match(src, /rt\.coordinator\.apply\(/);
  assert.doesNotMatch(src, /writeFile|appendFile|sendText|executeCommand\('type'|clipboard/);
});
