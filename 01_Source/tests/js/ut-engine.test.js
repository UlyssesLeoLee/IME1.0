'use strict';
// Engine adapter (migration step 1.1): the Rust engine - the real pime-core over JSON Lines - must give the same answers as
// the JavaScript engine for the whole F1 edit path (snapshot, hunks, selection, plans, Prepare/Apply, grants).
// These tests need a built core: set PIME_CORE_EXE to the path of pime-core(.exe); without it they are SKIPPED (not passed).
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const {createEngine} = require('../../editor-bridges/vscode/src/engine');
const {FakeDoc, sha} = require('./helpers');

const EXE = process.env.PIME_CORE_EXE;
const skip = EXE && fs.existsSync(EXE) ? false : 'set PIME_CORE_EXE to a built pime-core to run the Rust engine tests';
const URI = 'file:///w/a.rs';
const SEL0 = {start: {line: 0, character: 0}, end: {line: 0, character: 0}};
const BASE = 'fn a() {}\n\nfn b() {}\n\nfn c() {}\n';
const PROPOSED = 'fn a() { 1 }\n\nfn b() {}\n\nfn c() { 3 }\n';
const store = () => { const m = new Map(); return {get: k => m.get(k), update: (k, v) => { m.set(k, v); return Promise.resolve(); }, map: m}; };
const code = async fn => { try { await fn(); } catch (e) { return e.code; } return 'NO_ERROR'; };
const snapArgs = (text, over = {}) => ({uri: URI, version: 42, text, languageId: 'rust', selection: SEL0, isDirty: true, fileName: 'a.rs', ...over});

async function engines() {
  const js = createEngine('js', {globalState: store()});
  const gs = store();
  const rust = createEngine('rust', {globalState: gs});
  assert.equal(await rust.start({binary: EXE, locale: 'en-US', values: {}}), true, rust.reason);
  rust.globalState = gs;
  return {js, rust};
}

test('engine: the Rust engine starts, opens a session and reports itself available', {skip}, async () => {
  const e = createEngine('rust', {globalState: store()});
  assert.equal(await e.start({binary: EXE, locale: 'ja-JP', values: {budget: {maxRequests: 3}}}), true);
  assert.equal(e.available(), true); assert.match(e.session, /^[0-9a-f]{32}$/);
  assert.equal(e.describe(), 'rust'); assert.equal(e.fallbackAllowed, false);
  e.stop();
  assert.equal(e.available(), false);
});

test('engine: a missing or dead core switches the Rust engine off with a reason, never to the JS engine', {skip}, async () => {
  let changes = 0;
  const gone = createEngine('rust', {globalState: store(), onChange: () => changes++});
  assert.equal(await gone.start({binary: EXE + '.does-not-exist', locale: 'en-US', values: {}}), false);
  assert.equal(gone.available(), false); assert.match(gone.reason, /does not exist/); assert.equal(changes, 1);
  assert.equal(await code(() => gone.snapshot(snapArgs(BASE))), 'IPC_TIMEOUT');
  assert.equal(gone.fallbackAllowed, false);
  const empty = createEngine('rust', {globalState: store()});
  assert.equal(await empty.start({binary: '', locale: 'en-US', values: {}}), false); assert.match(empty.reason, /empty/);
  // the core dies while the extension runs
  let down = 0;
  const e = createEngine('rust', {globalState: store(), onChange: () => down++});
  assert.equal(await e.start({binary: EXE, locale: 'en-US', values: {}}), true);
  const before = down;
  e.core.child.kill();
  for (let i = 0; i < 50 && e.available(); i++) await new Promise(r => setTimeout(r, 50));
  assert.equal(e.available(), false); assert.equal(down, before + 1); assert.match(e.reason, /ended|stopped/);
  assert.equal(await code(() => e.excerpt({snapshot_id: 'x'}, 0, {before: 1, after: 1})), 'IPC_TIMEOUT');
});

test('engine: snapshots, hunks, selections and previews are identical in both engines (UT-028/045)', {skip}, async () => {
  const {js, rust} = await engines();
  const pairs = [[BASE, PROPOSED], ['a\r\nb\r\nc\r\n', 'a\r\nB\r\nc'], ['a😀\nb\n', 'a😀\nB\nc\n'], ['', 'new\n'], ['x\ny\n', ''], ['same\n', 'same\n'], ['a\rb\rc', 'a\rb\rc\n'],
    [Array.from({length: 60}, (_, i) => `line ${i}\n`).join(''), Array.from({length: 60}, (_, i) => i % 7 === 0 ? `LINE ${i}\n` : `line ${i}\n`).join('')]];
  for (const [before, after] of pairs) {
    const sj = await js.snapshot(snapArgs(before)), sr = await rust.snapshot(snapArgs(before));
    for (const k of ['sha256', 'eol', 'document_uri', 'document_version', 'content_scope', 'selection', 'dirty', 'file_name', 'language_id']) assert.deepEqual(sr[k], sj[k], `snapshot ${k}`);
    assert.equal(sr.content, before); assert.match(sr.snapshot_id, /^[0-9a-f]{32}$/);
    const opts = {language: 'rust', dialect: {}};
    const pj = await js.propose(sj, after, opts), pr = await rust.propose(sr, after, opts);
    assert.deepEqual(pr.hunks, pj.hunks, JSON.stringify(before));
    assert.deepEqual(pr.result, pj.result);
    const ids = pj.hunks.map(h => h.id);
    for (const pick of [ids, [], ids.slice(0, 1), ids.slice(-1)]) {
      assert.deepEqual(await rust.select(sr, pr.review, pick, opts), await js.select(sj, pj.review, pick, opts));
      assert.equal(await rust.resultText(sr, pr.review, pick), await js.resultText(sj, pj.review, pick));
    }
    assert.deepEqual(await rust.validateSnapshot(sr, 'rust', {}), await js.validateSnapshot(sj, 'rust', {}));
    rust.release(sr);
  }
  rust.stop();
});

test('engine: excerpts and insert previews are identical in both engines (UT-027)', {skip}, async () => {
  const {js, rust} = await engines();
  const text = 'x'.repeat(500) + '😀' + 'y'.repeat(500) + '\r\nlast line\n';
  const sj = await js.snapshot(snapArgs(text)), sr = await rust.snapshot(snapArgs(text));
  for (const [cursor, before, after] of [[500, 80, 80], [502, 1, 1], [500, 1, 1], [0, 5, 5], [10_000, 10, 10], [503, 1800, 900]]) {
    assert.deepEqual(await rust.excerpt(sr, cursor, {before, after}), await js.excerpt(sj, cursor, {before, after}), `${cursor}/${before}/${after}`);
  }
  const bad = [{start: 0, end: 0, text: 'fn x( {\n'}], ok = [{start: 0, end: 0, text: '// hi\n'}];
  for (const edits of [bad, ok]) assert.deepEqual(await rust.validateSnapshot(sr, 'rust', {}, edits), await js.validateSnapshot(sj, 'rust', {}, edits));
  rust.stop();
});

// The whole write path through the coordinator facade: same outcomes with the same fake editor.
async function flow(engine, mode, {confirmed = true, revoke = false, validationOk = true, mutate} = {}) {
  const doc = new FakeDoc(URI, BASE, 42); doc.mode = mode; doc.selection = SEL0;
  const snap = await engine.snapshot(snapArgs(BASE));
  const prop = await engine.propose(snap, PROPOSED, {language: 'rust', dialect: {}});
  const edits = engine.hunkEdits(prop.review, ['h1']);
  const grant = await engine.broker.authorize({providerId: 'A:h', workspace: engine.workspaceKey(['file:///w']), scope: 'L3', ttlMs: 60_000, once: false});
  const plan = await engine.coordinator.createPlan({kind: 'full', snapshot: snap, edits, targets: [URI], selection: snap.selection});
  if (mutate) mutate(doc);
  if (revoke) await engine.broker.revoke(grant.id);
  const out = {};
  try {
    const token = await engine.coordinator.prepare(plan.id, {state: doc.state(), confirmed, grantId: grant.id, grantOk: await engine.broker.stillValid(grant.id), validationOk});
    out.token = typeof token;
    out.result = await engine.coordinator.apply(token, doc.adapter());
    out.replay = await code(() => engine.coordinator.apply(token, doc.adapter()));
  } catch (e) { out.error = e.code; }
  out.text = doc.text; out.applyCalls = doc.applyCalls; out.version = doc.version;
  return out;
}

test('engine: prepare/apply give the same outcome in both engines (UT-047..052)', {skip}, async () => {
  const {js, rust} = await engines();
  const cases = [
    ['ok', {}], ['throw', {}], ['wrong-text', {}], ['reject', {}],
    ['ok', {confirmed: undefined}], ['ok', {confirmed: 'yes'}], ['ok', {revoke: true}], ['ok', {validationOk: false}],
    ['ok', {mutate: d => d.userEdit('fn a() {}\n\nfn b() {}\n\nfn c() {}\n// typed\n')}],
    ['ok', {mutate: d => { d.text = 'fn a() {}\n\nfn b() {}\n\nfn X() {}\n'; }}], ['ok', {mutate: d => { d.closed = true; }}]
  ];
  for (const [mode, opts] of cases) {
    const a = await flow(js, mode, opts), b = await flow(rust, mode, opts);
    assert.deepEqual(b, a, `${mode} ${JSON.stringify(opts)}`);
  }
  const ok = await flow(rust, 'ok');
  assert.equal(ok.text, 'fn a() { 1 }\n\nfn b() {}\n\nfn c() {}\n'); assert.equal(ok.result.state, 'APPLIED'); assert.equal(ok.result.newHash, sha(ok.text));
  assert.equal(ok.replay, 'TOKEN_INVALID'); assert.equal(ok.applyCalls, 1);
  rust.stop();
});

test('engine: an insert plan checks the selection; whole-file plans need an L3 snapshot; one file only (UT-030/054, IT-029)', {skip}, async () => {
  const {js, rust} = await engines();
  for (const engine of [js, rust]) {
    const snap = await engine.snapshot(snapArgs('abc\ndef\n'));
    const ws = engine.workspaceKey(['file:///w']);
    const g = await engine.broker.authorize({providerId: 'A:h', workspace: ws, scope: 'L1', ttlMs: 60_000});
    const sel0 = {start: {line: 0, character: 1}, end: {line: 0, character: 1}};
    const plan = await engine.coordinator.createPlan({kind: 'insert', snapshot: snap, edits: [{start: 1, end: 1, text: 'X'}], targets: [URI], selection: sel0});
    const doc = new FakeDoc(URI, 'abc\ndef\n', 42); doc.selection = {start: {line: 1, character: 0}, end: {line: 1, character: 0}};
    assert.equal(await code(() => engine.coordinator.prepare(plan.id, {state: doc.state(), confirmed: true, grantId: g.id, grantOk: true})), 'STALE_SELECTION', engine.kind);
    assert.equal(await code(() => engine.coordinator.createPlan({kind: 'full', snapshot: snap, edits: [{start: 0, end: 1, text: 'X'}], targets: [URI, 'file:///w/other.rs']})), 'CROSS_FILE_DENIED', engine.kind);
    const disk = await engine.snapshot(snapArgs('on disk\n', {level: 'L4'}));
    assert.equal(await code(() => engine.coordinator.createPlan({kind: 'full', snapshot: disk, edits: [{start: 0, end: 2, text: 'ON'}], targets: [URI]})), 'FULL_CONTEXT_REQUIRED', engine.kind);
    assert.equal(await code(() => engine.coordinator.createPlan({kind: 'full', snapshot: snap, edits: [{start: 0, end: 99, text: 'x'}], targets: [URI]})), 'TARGET_RANGE_INVALID', engine.kind);
    assert.equal(await code(() => engine.coordinator.createPlan({kind: 'full', snapshot: snap, edits: [{start: 0, end: 5, text: 'a'}, {start: 3, end: 7, text: 'b'}], targets: [URI]})), 'PATCH_INVALID', engine.kind);
  }
  rust.stop();
});

test('engine: the grant broker behaves the same in both engines and its persistence is interchangeable (UT-029, IT-015)', {skip}, async () => {
  const {js, rust} = await engines();
  const shape = g => g && {providerId: g.providerId, scope: g.scope, once: g.once, used: g.used, revoked: g.revoked, ttl: g.expiresAt - g.createdAt};
  const results = [];
  for (const engine of [js, rust]) {
    const b = engine.broker, ws = engine.workspaceKey(['file:///w']), other = engine.workspaceKey(['file:///other']);
    const r = [];
    const g = await b.authorize({providerId: 'A:host', workspace: ws, scope: 'L1', ttlMs: 5000});
    r.push(shape(g));
    const ok = {providerId: 'A:host', workspace: ws, scope: 'L1'};
    r.push(await b.check(g.id, ok), await b.check(g.id, {...ok, providerId: 'B:host'}), await b.check(g.id, {...ok, workspace: other}), await b.check(g.id, {...ok, scope: 'L3'}), await b.check('nope', ok));
    r.push(await code(() => b.require(g.id, {...ok, scope: 'L3'})), shape(await b.require(g.id, ok)), shape(await b.find(ok)), await b.find({...ok, scope: 'L3'}));
    const once = await b.authorize({providerId: 'A:host', workspace: ws, scope: 'L3', once: true, ttlMs: 60_000});
    r.push(await b.find({...ok, scope: 'L3'}), await b.stillValid(once.id));
    await b.consume(once.id);
    r.push((await b.check(once.id, {...ok, scope: 'L3'})).reason, await b.stillValid(once.id));
    r.push(await code(() => b.authorize({providerId: 'A', workspace: ws, scope: 'L9'})), await code(() => b.authorize({providerId: '', workspace: ws, scope: 'L1'})));
    r.push((await b.list()).map(x => ({providerId: x.providerId, scope: x.scope, once: x.once})));
    r.push(await b.revoke(g.id), await b.revoke('nope'), await b.stillValid(g.id), await b.find(ok));
    await b.authorize({providerId: 'mockA:127.0.0.1:1', workspace: ws, scope: 'L3', once: true, ttlMs: 60_000});
    await b.authorize({providerId: 'mockA:127.0.0.1:2', workspace: ws, scope: 'L1', ttlMs: 60_000});
    await b.authorize({providerId: 'mockAB:h', workspace: ws, scope: 'L1', ttlMs: 60_000});
    r.push(await b.revokeProviderPrefix('mockA:'), (await b.list()).map(x => x.providerId));
    r.push(await b.revokeProvider('mockAB:h'), await b.revokeAll(), await b.list());
    results.push(r);
  }
  assert.deepEqual(results[1], results[0]);
  // persistence: only reusable, not revoked grants, in the shape the JS broker always stored
  const {js: js2, rust: rust2} = await engines();
  await js2.broker.authorize({providerId: 'A:h', workspace: js2.workspaceKey(['file:///w']), scope: 'L1'});
  await js2.broker.authorize({providerId: 'A:h', workspace: js2.workspaceKey(['file:///w']), scope: 'L3', once: true});
  const saved = JSON.parse(JSON.stringify([...js2.broker.grants.values()].filter(g => !g.once && !g.revoked)));
  const gs = store(); gs.map.set('pime.grants.v1', saved);
  const fresh = createEngine('rust', {globalState: gs});
  assert.equal(await fresh.start({binary: EXE, locale: 'en-US', values: {}}), true);
  assert.equal((await fresh.broker.list()).length, 1, 'a grant saved by the JS engine is loaded by the Rust engine');
  const kept = await fresh.broker.find({providerId: 'A:h', workspace: fresh.workspaceKey(['file:///w']), scope: 'L1'});
  assert.equal(kept.id, saved[0].id);
  await fresh.broker.authorize({providerId: 'B:h', workspace: fresh.workspaceKey(['file:///w']), scope: 'L1'});
  const written = gs.map.get('pime.grants.v1');
  assert.equal(written.length, 2); assert.deepEqual(Object.keys(written[0]).sort(), Object.keys(saved[0]).sort());
  assert.doesNotMatch(JSON.stringify(written), /file:\/\/\/w/, 'workspace paths are stored only as hashes');
  const back = createEngine('js', {globalState: gs});
  assert.equal(back.broker.list().length, 2, 'and the JS engine loads what the Rust engine saved');
  await fresh.broker.revokeAll();
  assert.equal(gs.map.get('pime.grants.v1').length, 0);
  js2.stop(); rust.stop(); rust2.stop(); fresh.stop();
});

test('engine: the realtime gate answers from the last verdict, is primed by authorize and cleared by revocation', {skip}, async () => {
  const {rust} = await engines();
  const b = rust.broker, ws = rust.workspaceKey(['file:///w']), c = {providerId: 'A:h', workspace: ws, scope: 'L1'};
  assert.equal(b.findNow(c), null, 'unknown until the first answer arrives');
  await b.authorize({...c, ttlMs: 60_000});
  assert.ok(b.findNow(c), 'primed by authorize');
  await b.revokeAll();
  assert.equal(b.findNow(c), null, 'revocation clears the verdict');
  await new Promise(r => setTimeout(r, 100));
  assert.equal(b.findNow(c), null, 'the background refresh confirms it');
  rust.stop();
});

test('engine: a document of about 1.5 MiB goes through the whole edit path and the Diff text can be read back', {skip}, async () => {
  const {rust} = await engines();
  const line = 'let value = compute(items, 中文, 😀);\n';
  const rows = Array.from({length: Math.ceil(1_500_000 / line.length)}, () => line);
  const base = rows.join('');
  rows[3] = 'let changed = 1;\n'; rows[5] = 'let also = 2;\r\n';
  const after = rows.join('');
  const snap = await rust.snapshot(snapArgs(base));
  assert.equal(snap.sha256, sha(base));
  const prop = await rust.propose(snap, after, {language: 'rust', dialect: {}});
  assert.equal(prop.hunks.length, 2);
  const ids = prop.hunks.map(h => h.id);
  const text = await rust.resultText(snap, prop.review, ids);
  assert.equal(text, after, 'a reply larger than 1 MiB is read in full (multi-byte characters across chunk borders included)');
  const grant = await rust.broker.authorize({providerId: 'A:h', workspace: rust.workspaceKey(['file:///w']), scope: 'L3', ttlMs: 60_000});
  const doc = new FakeDoc(URI, base, 42); doc.selection = SEL0;
  const plan = await rust.coordinator.createPlan({kind: 'full', snapshot: snap, edits: rust.hunkEdits(prop.review, ids), targets: [URI]});
  const token = await rust.coordinator.prepare(plan.id, {state: doc.state(), confirmed: true, grantId: grant.id, grantOk: true, validationOk: true});
  const out = await rust.coordinator.apply(token, doc.adapter());
  assert.equal(out.state, 'APPLIED'); assert.equal(doc.text, after);
  rust.stop();
});

test('engine: settings reach the core and scenarios resolve there as in JS', {skip}, async () => {
  const {rust} = await engines();
  const values = {providers: {mockA: {api: 'openai-responses', endpoint: 'https://127.0.0.1:9/v1/responses'}}, scenarios: {f1Full: {provider: 'mockA', model: 'm-1'}}};
  rust.applySettings(values, 'zh-CN');
  const r = await rust.rpc({op: 'settings.resolve', scenario: 'f1Full'});
  assert.deepEqual([r.scenario.providerId, r.scenario.family, r.scenario.model], ['mockA', 'openai-responses', 'm-1']);
  assert.equal(await code(() => rust.rpc({op: 'settings.resolve', scenario: 'nope'})), 'SETTINGS_INVALID');
  const t = await rust.rpc({op: 'grant.workspace_token', uris: ['file:///b', 'file:///a']});
  assert.equal(t.workspace, require('../../editor-bridges/vscode/src/core/grants').workspaceToken(['file:///a', 'file:///b']));
  rust.stop();
});
