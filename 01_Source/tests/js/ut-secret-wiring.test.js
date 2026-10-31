'use strict';
// The secret-file read gate wired into the bridge (src/filegate.js and every place that reads a document: extension.js, languageState.js, f1.js,
// f2.js, annotate.js). core/secretfile.js (classification and the decision store) is tested in ut-secretfile.test.js (SEC-01..15); here the
// question is "is anything of a secret-class file READ before the user said yes?".
// The `vscode` module, the runtime, the file system (realpath) and the clock are injected fakes; every fake document counts (and can refuse)
// every touch of its content (getText, lineAt, offsetAt, positionAt). The engine, the broker and the privacy gate are the real JS ones.
// SEC-45 loads the real extension.js, languageState.js and runtime.js against a fake `vscode` module (a scoped Module._load hook). Nothing here ran
// inside a real VS Code window (see the UAT spec 07-secret-files.spec.js, which is a separate run), and no real link / junction was created:
// symlink creation is blocked on this machine, so the real-path cases use a fake realpath.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const Module = require('node:module');
const {core, tick} = require('./helpers');
const {createEngine} = require('../../editor-bridges/vscode/src/engine');
const SRC = path.join(__dirname, '..', '..', 'editor-bridges', 'vscode', 'src');
const {createFileGate} = require(path.join(SRC, 'filegate'));
const f1mod = require(path.join(SRC, 'f1'));
const f2mod = require(path.join(SRC, 'f2'));
const annMod = require(path.join(SRC, 'annotate'));
const i18n = core('i18n'), settingsMod = core('settings'), secretfile = core('secretfile');
const {Diagnostics} = core('diagnostics');
const {codeOf} = core('errors');

const root = path.join(__dirname, '..', '..', 'editor-bridges', 'vscode');
const ROOT = process.platform === 'win32' ? 'C:\\proj' : '/proj';
const P = (...p) => path.join(ROOT, ...p);
const store = () => { const m = new Map(); return {get: k => m.get(k), update: (k, v) => { m.set(k, v); return Promise.resolve(); }}; };
const L = (k, ...a) => i18n.t('en-US', k, ...a);
const ALLOW_ONCE = 'Allow once', ALLOW_SESSION = 'Allow for this session', NOTE_ALLOW = 'Allow this file for this session', NOTE_OFF = 'Keep off';
const EOLRE = /\r\n|\r|\n/g;
const RUST = 'fn main() {\n    println!("hi");\n}\n';

// ---------------------------------------------------------------- fakes
class Range {
  constructor(a, b, c, d) { if (typeof a === 'object') { this.start = a; this.end = b; } else { this.start = {line: a, character: b}; this.end = {line: c, character: d}; } }
}
const starts = text => { const s = [0]; EOLRE.lastIndex = 0; for (let m; (m = EOLRE.exec(text));) s.push(m.index + m[0].length); return s; };
// A document that counts every touch of its content in `log` ('read') and, with `throws`, refuses it. Metadata (uri, name, version, language id, line count) is free.
function doc(fsPath, {text = 'API_KEY=placeholder\nMODE=dev\n', scheme = 'file', languageId = 'plaintext', throws = false, log = []} = {}) {
  const d = {uri: {scheme, fsPath, toString: () => `${scheme}:///${String(fsPath).replace(/\\/g, '/')}`}, fileName: fsPath, languageId, version: 1, isDirty: false, isUntitled: scheme === 'untitled', isClosed: false, reads: 0, text, log, throws};
  const touch = () => { d.reads++; log.push('read'); if (d.throws) throw new Error('the document was read before the gate said yes'); };
  Object.defineProperty(d, 'lineCount', {get: () => text.split(EOLRE).length});
  const offsetAt = p => starts(text)[p.line] + p.character;
  d.getText = r => { touch(); return r ? text.slice(offsetAt(r.start), offsetAt(r.end)) : text; };
  d.lineAt = n => { touch(); const s = starts(text); return {text: text.slice(s[n], n + 1 < s.length ? s[n + 1] : text.length).replace(/\r?\n$/, '')}; };
  d.offsetAt = p => { touch(); return offsetAt(p); };
  d.positionAt = off => { touch(); const s = starts(text); let line = 0; while (line + 1 < s.length && s[line + 1] <= off) line++; return {line, character: off - s[line]}; };
  return d;
}
const editorOf = (d, a = 0, b = a) => { const pos = off => { const s = starts(d.text); let line = 0; while (line + 1 < s.length && s[line + 1] <= off) line++; return {line, character: off - s[line]}; };
  return {document: d, selection: {start: pos(a), end: pos(b), isEmpty: a === b, active: pos(b), isEqual: () => true}}; };

// handler: the fake window of every rig answers a MODAL through `st.modal` (a string, a function of the message, or undefined = Esc); a non-modal
// message is recorded and its promise is settled by the test (notice.resolve).
function windowFor(st, ui) {
  return {
    get activeTextEditor() { return st.active; },
    showWarningMessage: (msg, ...rest) => {
      if (rest[0] && typeof rest[0] === 'object') {
        ui.modals.push({msg, detail: rest[0].detail, modal: rest[0].modal, buttons: rest.slice(1)}); ui.log.push('modal');
        return Promise.resolve(typeof st.modal === 'function' ? st.modal(msg) : st.modal);
      }
      return new Promise(resolve => { ui.notices.push({msg, buttons: rest, resolve}); ui.log.push('notice'); });
    },
    showInformationMessage: async msg => { ui.infos.push(msg); },
    showErrorMessage: async msg => { ui.errors.push(msg); },
    showQuickPick: async items => { ui.picks.push(items); return typeof st.pick === 'function' ? st.pick(items) : st.pick; }
  };
}
const newUi = () => ({modals: [], notices: [], infos: [], errors: [], picks: [], log: []});

// the gate alone: a fake clock, a fake file system (links: path -> real path, or an Error to fail), the live patterns of the setting
function gateRig({patterns = [], active = null} = {}) {
  const st = {patterns, modal: undefined, active, pick: undefined, clock: 1700000000000}, ui = newUi(), links = {}, calls = {realpath: [], realpathSync: []};
  const diag = new Diagnostics({now: () => st.clock});
  const resolve = p => { const v = links[p]; if (v instanceof Error) throw v; return v === undefined ? p : v; };
  const gate = createFileGate({
    vscode: {window: windowFor(st, ui)}, rt: {diagnostics: diag}, L, cfg: () => ({get: k => k === 'secretFilePatterns' ? st.patterns : undefined}), now: () => st.clock,
    realpath: async p => { calls.realpath.push(p); return resolve(p); }, realpathSync: p => { calls.realpathSync.push(p); return resolve(p); }
  });
  const changes = {n: 0}; gate.onChange(() => { changes.n++; });
  return {gate, st, ui, links, calls, diag, changes, advance: ms => { st.clock += ms; }};
}

// ---------------------------------------------------------------- the gate: SEC-30..39
test('SEC-30 a name that is not secret-class is let through at once: no dialog, no notification, nothing read, nothing recorded', async () => {
  const r = gateRig();
  for (const name of ['main.rs', 'README.md', 'tokens.json', '.env.example', 'credentials.ts', 'auth.json', 'notes.txt', 'src/lib.py']) {
    const d = doc(P(...name.split('/')), {throws: true});
    for (const interactive of [false, true]) assert.equal(await r.gate.ensure(d, {interactive, purpose: 'f1'}), true, name);
    assert.equal(d.reads, 0, name); assert.equal(r.gate.isLocked(d), false, name);
  }
  assert.equal(r.ui.modals.length, 0); assert.equal(r.ui.notices.length, 0); assert.deepEqual(r.diag.events, []); assert.deepEqual(r.gate.list(), []);
  assert.equal(await r.gate.ensure({}, {interactive: true}), true, 'a document without any name has nothing to classify');
  assert.equal(await r.gate.ensure(doc('Untitled-1', {scheme: 'untitled', throws: true}), {interactive: true}), true, 'an untitled buffer');
});

test('SEC-31 interactive: a modal names the file and its kind, says reading is local and sending is asked separately, offers Allow once / Allow for this session; Esc refuses this call and remembers nothing', async () => {
  const r = gateRig(); const d = doc(P('.env'), {throws: true});
  r.st.modal = undefined; // Esc
  assert.equal(await r.gate.ensure(d, {interactive: true, purpose: 'f1'}), false);
  assert.equal(r.ui.modals.length, 1);
  const m = r.ui.modals[0];
  assert.equal(m.modal, true); assert.match(m.msg, /"\.env"/); assert.match(m.msg, /environment variables file/);
  assert.match(m.detail, /Reading is local/); assert.match(m.detail, /AI provider is asked separately/);
  assert.deepEqual(m.buttons, [ALLOW_ONCE, ALLOW_SESSION]);
  assert.equal(r.ui.notices.length, 0, 'an explicit command never uses the notification');
  assert.deepEqual(r.gate.list(), [], 'Esc remembers nothing: not even a deny');
  assert.equal(await r.gate.ensure(d, {interactive: true, purpose: 'f2'}), false); assert.equal(r.ui.modals.length, 2, 'the next explicit command asks again');
  r.st.modal = () => 'something else'; // a button that is not ours is a refusal too
  assert.equal(await r.gate.ensure(d, {interactive: true}), false);
  r.st.modal = ALLOW_SESSION;
  assert.equal(await r.gate.ensure(d, {interactive: true, purpose: 'f1'}), true); assert.equal(r.ui.modals.length, 4);
  assert.equal(d.reads, 0, 'the gate never touches the content');
  // every kind has its own words
  for (const [name, re] of [['id_rsa', /private key/], ['server.pfx', /keystore/], ['credentials.json', /credentials file/], ['kubeconfig', /cloud credentials/], ['.npmrc', /token or login store/], ['terraform.tfstate', /infrastructure/], ['logins.kdbx', /password database/]]) {
    const x = gateRig(); x.st.modal = undefined; await x.gate.ensure(doc(P(name)), {interactive: true});
    assert.match(x.ui.modals[0].msg, re, name);
  }
});

test('SEC-32 "Allow once" is spent by exactly one call; "Allow for this session" stays until it expires', async () => {
  const r = gateRig(); const d = doc(P('.env'), {throws: true});
  r.st.modal = ALLOW_ONCE;
  assert.equal(await r.gate.ensure(d, {interactive: true, purpose: 'f1'}), true); assert.equal(r.ui.modals.length, 1);
  assert.deepEqual(r.gate.list(), [], 'spent by that very call');
  assert.equal(r.changes.n, 0, 'the state is as it was: nothing to redraw, and no re-trigger of the suggestions (they would show the typing-path notification right after the user said yes)');
  assert.equal(r.ui.notices.length, 0);
  assert.equal(await r.gate.ensure(d, {interactive: false, purpose: 'candidates'}), false, 'a typing call right after is not covered');
  assert.equal(await r.gate.ensure(d, {interactive: true, purpose: 'f1'}), true); assert.equal(r.ui.modals.length, 2, 'asked again');
  // a one-use grant that already exists (recorded in the store) is consumed by the first read and gone for the second
  const g = gateRig(); const e = doc(P('.env'));
  g.gate.gate.record(P('.env'), 'once');
  assert.equal(await g.gate.ensure(e, {interactive: false}), true); assert.equal(await g.gate.ensure(e, {interactive: false}), false);
  assert.equal(g.ui.modals.length, 0);
  // a session grant: no dialog again, typing and commands alike, until the TTL
  const s = gateRig(); s.st.modal = ALLOW_SESSION;
  assert.equal(await s.gate.ensure(e, {interactive: true}), true); assert.equal(s.ui.modals.length, 1);
  assert.equal(s.changes.n, 1, 'a session grant unlocks the file: the status and the suggestions follow');
  for (let i = 0; i < 5; i++) { assert.equal(await s.gate.ensure(e, {interactive: i % 2 === 0}), true); }
  assert.equal(s.ui.modals.length, 1); assert.equal(s.ui.notices.length, 0);
  assert.deepEqual(s.gate.list().map(x => [x.label, x.state]), [['.env', 'allow']]);
  assert.equal(s.gate.isLocked(e), false);
  s.advance(8 * 3600 * 1000 + 1);
  s.st.modal = undefined;
  assert.equal(await s.gate.ensure(e, {interactive: true}), false, 'the session grant expired'); assert.equal(s.ui.modals.length, 2);
});

test('SEC-33 typing path: never a modal; ONE non-modal notification per file and session; Keep off is remembered silently; Allow opens the file', async () => {
  const r = gateRig(); const d = doc(P('.env'), {throws: true});
  for (let i = 0; i < 10; i++) assert.equal(await r.gate.ensure(d, {interactive: false, purpose: 'candidates'}), false);
  assert.equal(r.ui.modals.length, 0, 'a modal would steal keystrokes');
  assert.equal(r.ui.notices.length, 1, 'one notification, however many keystrokes');
  assert.match(r.ui.notices[0].msg, /"\.env"/); assert.match(r.ui.notices[0].msg, /not reading it/);
  assert.deepEqual(r.ui.notices[0].buttons, [NOTE_ALLOW, NOTE_OFF]);
  assert.equal(d.reads, 0);
  // dismissed (never answered): still closed, still only the one notification
  assert.equal(await r.gate.ensure(d, {interactive: false}), false); assert.equal(r.ui.notices.length, 1);
  assert.equal(r.gate.isLocked(d), true);
  // the user clicks Allow: from now on the file is open, the status can redraw
  r.ui.notices[0].resolve(NOTE_ALLOW); await tick();
  assert.equal(r.changes.n, 1); assert.equal(r.gate.isLocked(d), false);
  assert.equal(await r.gate.ensure(d, {interactive: false}), true);
  // another file, Keep off: a deny is remembered, nothing nags, even an explicit command is refused without a dialog
  const k = doc(P('id_rsa'), {throws: true});
  for (let i = 0; i < 3; i++) await r.gate.ensure(k, {interactive: false});
  assert.equal(r.ui.notices.length, 2);
  r.ui.notices[1].resolve(NOTE_OFF); await tick();
  assert.deepEqual(r.gate.list().map(x => [x.label, x.state]).sort(), [['.env', 'allow'], ['id_rsa', 'deny']]);
  assert.equal(await r.gate.ensure(k, {interactive: false}), false); assert.equal(await r.gate.ensure(k, {interactive: true, purpose: 'f1'}), false);
  assert.equal(r.ui.notices.length, 2); assert.equal(r.ui.modals.length, 0);
  assert.equal(r.gate.lockInfo(k).state, 'deny');
  // a window that throws never breaks the call, and never opens the file
  const broken = createFileGate({vscode: {window: {showWarningMessage: () => { throw new Error('no window'); }}}, rt: {diagnostics: new Diagnostics()}, L});
  const hidden = doc(P('.env'), {throws: true});
  assert.equal(await broken.ensure(hidden, {interactive: false}), false); assert.equal(await broken.ensure(hidden, {interactive: true}), false); assert.equal(hidden.reads, 0);
});

test('SEC-34 the gate fails closed: an error while deciding is "do not read", never "read"', async () => {
  const r = gateRig();
  const evil = {uri: {scheme: 'file', get fsPath() { throw new Error('boom'); }}, fileName: P('main.rs')};
  assert.equal(await r.gate.ensure(evil, {interactive: true}), false);
  assert.equal(r.gate.isLocked(evil), true, 'the status and the language detection treat it as locked');
  assert.ok(r.diag.events.some(e => e.component === 'secretfile' && /^sec\.internal\./.test(e.request_type)));
});

test('SEC-35 real path: a name that hides a secret file behind a link is judged by where it points; the decision follows the real file; a failing realpath falls back to the plain path', async () => {
  const r = gateRig();
  r.links[P('notes.txt')] = P('.env');            // innocent name -> .env
  r.links[P('other.log')] = P('.env');            // a second link to the same file
  r.links[P('broken.txt')] = new Error('ENOENT'); // realpath fails
  r.links[P('.env.local')] = P('data', 'plain.dat'); // a secret NAME that points at a harmless file: still secret by its name
  const hidden = doc(P('notes.txt'), {throws: true});
  assert.equal(r.gate.lockInfo(hidden).label, '.env', 'sync: no ensure was ever called, the real path is resolved on the spot');
  r.st.modal = ALLOW_SESSION;
  assert.equal(await r.gate.ensure(hidden, {interactive: true, purpose: 'f1'}), true);
  assert.match(r.ui.modals[0].msg, /"\.env"/, 'the dialog names the real file');
  // one file, one decision: its own name and the second link are covered, without a dialog
  assert.equal(await r.gate.ensure(doc(P('.env'), {throws: true}), {interactive: true}), true);
  assert.equal(await r.gate.ensure(doc(P('other.log'), {throws: true}), {interactive: true}), true);
  assert.equal(r.ui.modals.length, 1);
  assert.deepEqual(r.gate.list().map(x => x.label), ['.env']);
  // a failing realpath: the plain path decides
  assert.equal(await r.gate.ensure(doc(P('broken.txt'), {throws: true}), {interactive: true}), true, 'innocent plain path');
  const dotenv = doc(P('.env.production'), {throws: true}); r.links[P('.env.production')] = new Error('EACCES');
  r.st.modal = undefined;
  assert.equal(await r.gate.ensure(dotenv, {interactive: true}), false, 'secret plain path'); assert.equal(r.ui.modals.length, 2);
  // the name is secret although the target is not
  assert.equal(r.gate.lockInfo(doc(P('.env.local'))).rule, 'env-file');
  assert.equal(await r.gate.ensure(doc(P('.env.local'), {throws: true}), {interactive: true}), false);
  // the case of the drive and of the name does not matter on Windows: the same key
  if (process.platform === 'win32') assert.equal(await r.gate.ensure(doc('c:\\PROJ\\.ENV', {throws: true}), {interactive: true}), true);
  // only a file-scheme document has a real path; the others are judged by their name, realpath is not even asked
  const before = r.calls.realpath.length + r.calls.realpathSync.length;
  r.links[P('x.txt')] = P('.env');
  assert.equal(await r.gate.ensure(doc(P('x.txt'), {scheme: 'vscode-remote', throws: true}), {interactive: true}), true);
  assert.equal(await r.gate.ensure(doc(P('x.txt'), {scheme: 'untitled', throws: true}), {interactive: true}), true);
  assert.equal(r.calls.realpath.length + r.calls.realpathSync.length, before);
  assert.equal(await r.gate.ensure(doc(P('.env'), {scheme: 'git', throws: true}), {interactive: true}), true, 'the key of git:/.env is the same file name: already allowed');
  // a path longer than 65536 characters keeps its tail: the name is still seen
  const long = ROOT + path.sep + 'a'.repeat(70000) + path.sep + 'id_ed25519';
  assert.equal(r.gate.isLocked(doc(long)), true);
});

test('SEC-36 isLocked / lockInfo are synchronous, open no dialog, read nothing; the real path is cached for a few seconds and shared with ensure', async () => {
  const r = gateRig(); r.links[P('plain.txt')] = P('private.pem');
  const d = doc(P('plain.txt'), {throws: true});
  assert.equal(r.gate.isLocked(d), true); assert.equal(r.calls.realpathSync.length, 1);
  assert.deepEqual(r.gate.lockInfo(d), {label: 'private.pem', kind: 'private-key', rule: 'key-extension', state: 'ask'});
  assert.equal(r.gate.isLocked(d), true); assert.equal(r.calls.realpathSync.length, 1, 'cached');
  assert.equal(await r.gate.ensure(d, {interactive: false}), false); assert.equal(r.calls.realpath.length, 0, 'ensure shares the cache');
  r.advance(6000);
  assert.equal(r.gate.isLocked(d), true); assert.equal(r.calls.realpathSync.length, 2, 'refreshed after the TTL: a link that moved is seen');
  r.links[P('plain.txt')] = P('plain.dat');
  r.advance(6000);
  assert.equal(r.gate.isLocked(d), false, 'the link now points at a harmless file');
  r.advance(6000);
  assert.equal(await r.gate.ensure(d, {interactive: true}), true); assert.equal(r.calls.realpath.length, 1);
  assert.equal(r.ui.modals.length, 0); assert.equal(r.ui.notices.length, 1, 'only the one notification of the earlier typing-path call'); assert.equal(d.reads, 0);
  // allowed -> not locked, kept off -> locked (state deny)
  const e = doc(P('.env')); assert.equal(r.gate.isLocked(e), true);
  r.gate.gate.record(P('.env'), 'session'); assert.equal(r.gate.isLocked(e), false);
  r.gate.gate.record(P('.env'), 'deny'); assert.equal(r.gate.lockInfo(e).state, 'deny');
});

test('SEC-37 the setting programmingIme.secretFilePatterns adds patterns (live); the built-in rules cannot be switched off', async () => {
  const r = gateRig({patterns: ['*.cred', 'config/private/**', 'secrets/']});
  const secret = [P('app.cred'), P('config', 'private', 'db.yml'), P('secrets', 'a.txt'), P('deep', 'config', 'private', 'x', 'y.txt')];
  const open = [P('app.credx'), P('config', 'public', 'db.yml'), P('main.rs')];
  for (const p of secret) { assert.equal(r.gate.isLocked(doc(p)), true, p); assert.equal(r.gate.isSecretPath(p), true, p); }
  for (const p of open) { assert.equal(r.gate.isLocked(doc(p)), false, p); assert.equal(r.gate.isSecretPath(p), false, p); }
  r.st.modal = undefined; await r.gate.ensure(doc(P('app.cred')), {interactive: true});
  assert.match(r.ui.modals[0].msg, /matches your secretFilePatterns setting/);
  r.st.patterns = []; assert.equal(r.gate.isLocked(doc(P('app.cred'))), false, 'the setting is read on every call: no reload');
  r.st.patterns = ['x.txt']; assert.equal(r.gate.isLocked(doc(P('x.txt'))), true);
  // nothing the setting says can exempt a built-in name
  for (const patterns of [[], ['!.env', '', 5, null, {}, '-.env'], 'nonsense', null, undefined, {not: 'an array'}, ['.env.example'], ['*', '!**']]) {
    r.st.patterns = patterns;
    assert.equal(r.gate.isLocked(doc(P('.env'))), true, JSON.stringify(patterns)); assert.equal(r.gate.isLocked(doc(P('id_rsa'))), true, JSON.stringify(patterns));
  }
  assert.equal(secretfile.classifyFile({path: P('.env'), userPatterns: ['!.env']}).secret, true);
  // a vscode whose settings throw never lowers the guard
  const t = createFileGate({vscode: {window: windowFor({}, newUi())}, rt: {diagnostics: new Diagnostics()}, L, cfg: () => { throw new Error('no settings'); }});
  assert.equal(t.isLocked(doc(P('.env'))), true);
  assert.equal(t.isLocked(doc(P('main.rs'))), false);
});

test('SEC-38 diagnostics carry the rule id and the event only: never a path, a file name or content; a decision is recorded once, not once per keystroke', async () => {
  const r = gateRig({patterns: ['diary.txt']});
  const files = [doc(P('.env'), {text: 'HUNTER2=sk-abcdefghijklmnopqrstuvwxyz0123\n', throws: true}), doc(P('secrets.yml'), {throws: true}), doc(P('diary.txt'), {throws: true})];
  r.st.modal = ALLOW_ONCE; await r.gate.ensure(files[0], {interactive: true, purpose: 'f1'});
  r.st.modal = undefined; await r.gate.ensure(files[1], {interactive: true, purpose: 'f2'});
  for (let i = 0; i < 50; i++) await r.gate.ensure(files[2], {interactive: false, purpose: 'candidates'});
  r.ui.notices[0].resolve(NOTE_OFF); await tick();
  const events = r.diag.events;
  assert.ok(events.length >= 5 && events.length <= 9, `few events: ${events.length}`);
  const rules = new Set(secretfile.DEFAULT_RULES.map(x => x.id).concat('user-pattern', 'OK'));
  for (const e of events) {
    assert.equal(e.component, 'secretfile'); assert.ok(rules.has(e.error_code), e.error_code);
    assert.match(e.request_type, /^sec\.(prompt|once|session|refused|notice|deny|manual|revoke|revokeAll)\.(f1|f2|candidates|annotate|notice|manual|other)$/, e.request_type);
  }
  const text = JSON.stringify(events);
  for (const leak of ['proj', '.env', 'secrets.yml', 'diary', 'HUNTER2', 'sk-abc', 'C:\\', '/proj']) assert.ok(!text.includes(leak), `leaked: ${leak}`);
  assert.deepEqual(events.map(e => e.request_type).filter(t => /^sec\.notice\./.test(t)), ['sec.notice.candidates'], 'one notice event for fifty keystrokes');
});

test('SEC-39 the command: a QuickPick of the decisions (label, state, expiry, folder) to forget them, and a way to allow the active file explicitly', async () => {
  const d = doc(P('.env'), {throws: true});
  const r = gateRig({active: {document: d}});
  // nothing decided, the active file is secret-class: the first row allows it
  r.st.pick = items => items.find(i => i.action === 'allow');
  await r.gate.manage();
  assert.equal(r.ui.picks[0].length, 1); assert.equal(r.ui.picks[0][0].label, '$(unlock) Allow ".env" for this session');
  assert.equal(r.gate.isLocked(d), false); assert.equal(r.changes.n, 1); assert.deepEqual(r.ui.infos, ['Allowed ".env" for this session.']);
  // now it is listed (allow), with the folder, and can be forgotten; Forget all is offered
  r.gate.gate.record(P('id_rsa'), 'deny');
  r.st.pick = undefined; await r.gate.manage();
  const rows = r.ui.picks[1];
  assert.deepEqual(rows.map(x => x.action), ['revoke', 'revoke', 'revokeAll']);
  const env = rows.find(x => x.entry && x.entry.label === '.env'), key = rows.find(x => x.entry && x.entry.label === 'id_rsa');
  assert.match(env.label, /\.env$/); assert.match(env.description, /^allowed until /); assert.match(key.description, /^kept off until /); assert.ok(env.detail.length > 0);
  assert.equal(rows[2].label, '$(trash) Forget all decisions');
  // forgetting the allowed file: it is locked again and the notification may be shown again
  await r.gate.ensure(d, {interactive: false}); assert.equal(r.ui.notices.length, 0, 'allowed: no notification');
  r.st.pick = items => items.find(i => i.entry && i.entry.label === '.env'); await r.gate.manage();
  assert.equal(r.gate.isLocked(d), true); assert.match(r.ui.infos[1], /^Forgot the decision for "\.env"/);
  await r.gate.ensure(d, {interactive: false}); assert.equal(r.ui.notices.length, 1);
  await r.gate.ensure(d, {interactive: false}); assert.equal(r.ui.notices.length, 1, 'once again, not per keystroke');
  r.st.pick = items => items.find(i => i.entry && i.entry.label === '.env'); // the .env row is gone now: only id_rsa and Forget all remain
  r.st.pick = items => items.find(i => i.action === 'revokeAll'); await r.gate.manage();
  assert.deepEqual(r.gate.list(), []); assert.equal(r.ui.infos[2], 'Forgot all decisions: secret-class files will be asked again.');
  // a notification that was shown before Forget all is announced again afterwards
  await r.gate.ensure(d, {interactive: false}); assert.equal(r.ui.notices.length, 2);
  // Esc on the pick changes nothing; with nothing to manage a message says so
  const e = gateRig({active: {document: doc(P('main.rs'))}});
  await e.gate.manage(); assert.deepEqual(e.ui.picks, []); assert.equal(e.ui.infos.length, 1); assert.match(e.ui.infos[0], /^Nothing to manage/);
  const s = gateRig({active: {document: d}}); s.st.pick = undefined; await s.gate.manage(); assert.equal(s.gate.isLocked(d), true);
  // a kept-off active file can be allowed from the same row
  const k = gateRig({active: {document: d}}); k.gate.gate.record(P('.env'), 'deny');
  k.st.pick = items => items.find(i => i.action === 'allow'); await k.gate.manage();
  assert.equal(k.gate.isLocked(d), false);
});

// ---------------------------------------------------------------- hook: F1 (SEC-40, SEC-41) and the tab fallback (ANN-86)
const RUST_SRC = 'use std::fmt;\n\nfn parse(input: &str) -> usize {\n    let n = input.len();\n    n\n}\n';
function f1Rig({text = RUST_SRC, fileName = P('src', 'main.rs'), language = 'rust', languageId = 'rust', aiEnabled = true, trusted = true, throws = false, config = {}, scheme = 'file', patterns = []} = {}) {
  const st = {aiEnabled, trusted, modal: undefined, active: null, patterns, language, config: {f1Mode: 'annotate', f1AnnotateOnOpen: false, autoComment: true, graphComment: false, ...config}};
  const ui = {...newUi(), requests: 0, panels: []};
  const d = doc(fileName, {text, languageId, throws, scheme, log: ui.log});
  const engine = createEngine('js', {globalState: store()});
  const diagnostics = new Diagnostics();
  const settings = settingsMod.effective({providers: {mock: {api: 'openai-responses', endpoint: 'https://api.a.example/v1/responses'}},
    scenarios: {f1Full: {provider: 'mock', model: 'm-full'}, f1Insert: {provider: 'mock', model: 'm-insert'}, realtime: {provider: 'mock', model: 'm-rt'}}});
  const rt = {engine, broker: engine.broker, coordinator: engine.coordinator, budget: engine.budget, diagnostics, settings, locale: 'en-US', ctx: {secrets: {get: async () => 'KEY-123456'}},
    ai: {infer: async () => { ui.requests++; throw new Error('no request expected'); }}};
  class Panel {
    constructor(vscode, h) { this.h = h; this.states = []; this.disposed = false; ui.panels.push(this); }
    setTitle() {} update(s) { this.states.push(s); } reveal() {}
    dispose() { if (!this.disposed) { this.disposed = true; this.h.onDispose(); } }
  }
  const vscode = {workspace: {get isTrusted() { return st.trusted; }, registerTextDocumentContentProvider: () => ({dispose() {}})}, window: {...windowFor(st, ui), visibleTextEditors: []}, commands: {executeCommand: async () => {}}, Range};
  const fileGate = createFileGate({vscode, rt, L, cfg: () => ({get: k => k === 'secretFilePatterns' ? st.patterns : undefined}), realpath: async p => p, realpathSync: p => p});
  const f1 = f1mod.createF1({vscode, rt, L, errorText: e => i18n.describeError('en-US', codeOf(e), e && e.message), enabled: () => true, aiEnabled: () => st.aiEnabled,
    workspaceId: () => engine.workspaceKey(['file:///w']), grantProviderId: sc => `${sc.providerId}:api.a.example`, cfg: () => ({get: k => st.config[k]}),
    infoFor: () => ({language: st.language, dialect: {}}), annotation: {wanted: () => ({comment: true, graph: false, scope: 'directory'}), directorySymbols: async () => []}, Panel, fileGate});
  const editor = editorOf(d, 0);
  st.active = editor; vscode.window.activeTextEditor = editor;
  return {f1, st, ui, d, rt, fileGate, editor, last: () => ui.panels[ui.panels.length - 1], state: () => ui.panels[ui.panels.length - 1].states.slice(-1)[0]};
}

test('SEC-40 F1: a secret-class file is not read until the user allows it; Esc opens no panel and says why; a non-secret file opens with no dialog', async () => {
  // not secret: no dialog, the document is read, the panel opens
  const n = f1Rig({fileName: P('src', 'main.rs')});
  await n.f1.open(); await n.f1.settled();
  assert.equal(n.ui.panels.length, 1); assert.equal(n.ui.modals.length, 0); assert.equal(n.ui.notices.length, 0); assert.equal(n.ui.log[0], 'read');
  // secret, Esc
  const r = f1Rig({fileName: P('.env'), text: 'API_KEY=placeholder\n', throws: true});
  await r.f1.open();
  assert.equal(r.ui.panels.length, 0); assert.equal(r.d.reads, 0, 'not one touch of the document');
  assert.equal(r.ui.modals.length, 1); assert.deepEqual(r.ui.modals[0].buttons, [ALLOW_ONCE, ALLOW_SESSION]); assert.match(r.ui.modals[0].msg, /"\.env"/);
  assert.equal(r.ui.infos.length, 1); assert.match(r.ui.infos[0], /^F1 did not start/); assert.match(r.ui.infos[0], /Secret Files/);
  assert.deepEqual(r.ui.errors, []); assert.equal(r.ui.requests, 0);
  // the opening lock was released: allow once, the panel opens and the first read comes AFTER the dialog
  const a = f1Rig({fileName: P('.env'), text: 'API_KEY=placeholder\n'}); a.st.modal = ALLOW_ONCE;
  await a.f1.open(); await a.f1.settled();
  assert.equal(a.ui.panels.length, 1); assert.deepEqual(a.ui.log.slice(0, 2), ['modal', 'read']);
  assert.deepEqual(a.fileGate.list(), [], 'the one-use grant is spent');
  await a.f1.open(); assert.equal(a.ui.modals.length, 2, 'the next F1 asks again');
  // session: asked once; Keep off (deny): no dialog, a short message
  const s = f1Rig({fileName: P('.env'), text: 'API_KEY=placeholder\n'}); s.st.modal = ALLOW_SESSION;
  await s.f1.open(); await s.f1.open(); assert.equal(s.ui.modals.length, 1); assert.equal(s.ui.panels.length, 2);
  const k = f1Rig({fileName: P('.env'), throws: true}); k.fileGate.gate.record(P('.env'), 'deny');
  await k.f1.open(); assert.equal(k.ui.modals.length, 0); assert.equal(k.ui.panels.length, 0); assert.equal(k.ui.infos.length, 1); assert.equal(k.d.reads, 0);
  // a user pattern is the same
  const u = f1Rig({fileName: P('notes', 'diary.txt'), throws: true, patterns: ['diary.txt']}); await u.f1.open();
  assert.equal(u.ui.modals.length, 1); assert.equal(u.ui.panels.length, 0);
});

test('SEC-41 F1: a refusal changes nothing - the open session on another file stays, and a stale session that is refused on reopen is not regenerated', async () => {
  const r = f1Rig({fileName: P('src', 'main.rs')});
  await r.f1.open(); await r.f1.settled();
  const sessionA = r.f1.current(); assert.ok(sessionA);
  const env = doc(P('.env'), {text: 'API_KEY=placeholder\n', throws: true, log: r.ui.log});
  r.st.active = editorOf(env, 0);
  await r.f1.open(r.st.active); await r.f1.settled();
  assert.equal(r.f1.current(), sessionA, 'the session on main.rs is still the current one'); assert.equal(r.ui.panels.length, 1); assert.equal(sessionA.panel.disposed, false);
  assert.equal(env.reads, 0);
  // reopen: allow once -> the session opens; the document changes (stale); Generate -> reopen -> the one-use grant is spent -> asked again -> Esc
  const o = f1Rig({fileName: P('.env'), text: 'API_KEY=placeholder\n'}); o.st.modal = ALLOW_ONCE;
  await o.f1.open(); await o.f1.settled();
  const first = o.f1.current(); assert.ok(first);
  o.d.version++; o.f1.onDocumentChanged({document: o.d, contentChanges: [{}]}); assert.equal(first.stale, true);
  o.st.modal = undefined;
  first.panel.h.onMessage({type: 'generate', intent: ''}); await o.f1.settled();
  assert.equal(o.ui.modals.length, 2, 'asked again'); assert.equal(o.f1.current(), first, 'the old session is still there'); assert.equal(first.stale, true, 'and still stale: nothing was regenerated');
  assert.equal(o.ui.requests, 0); assert.equal(o.ui.panels.length, 1);
});

test('ANN-86 F1 opens on the Whole file tab when the annotate tab is unavailable only because of the language or an empty document and AI is otherwise available', async () => {
  const open = async opts => { const r = f1Rig(opts); await r.f1.open(); await r.f1.settled(); return r; };
  const u = await open({language: 'unknown', languageId: 'plaintext', fileName: P('notes.txt'), text: 'plain words\n', config: {f1AnnotateOnOpen: true}});
  assert.equal(u.state().active, 'full'); assert.equal(u.state().canGenerate, true); assert.equal(u.state().mode, 'annotate');
  assert.equal(u.state().tabs.annotate.status, 'unavailable'); assert.match(u.state().tabs.annotate.message, /not available for the language "unknown"/, 'the annotate tab still says why');
  assert.equal(u.state().tabs.full.status, 'idle'); assert.equal(u.ui.requests, 0, 'no request starts on its own'); assert.equal(u.ui.modals.length, 0);
  const e = await open({text: ''});
  assert.equal(e.state().active, 'full'); assert.equal(e.state().canGenerate, true); assert.match(e.state().tabs.annotate.message, /empty/); assert.equal(e.ui.requests, 0);
  const w = await open({text: '  \n\n  '});
  assert.equal(w.state().active, 'full');
  // every other reason hits all tabs alike (or none of the others): the annotate tab stays on screen and says so
  const off = await open({language: 'unknown', languageId: 'plaintext', aiEnabled: false}); assert.equal(off.state().active, 'annotate'); assert.match(off.state().tabs.annotate.message, /AI off/);
  const untrusted = await open({language: 'unknown', languageId: 'plaintext', trusted: false}); assert.equal(untrusted.state().active, 'annotate');
  const git = await open({language: 'unknown', languageId: 'plaintext', scheme: 'git'}); assert.equal(git.state().active, 'annotate');
  const big = await open({language: 'unknown', languageId: 'plaintext', text: 'x'.repeat(2 * 1024 * 1024 + 1)}); assert.equal(big.state().active, 'annotate');
  // a supported language with code stays on the annotate tab; edit mode stays on the whole-file tab
  const ok = await open({}); assert.equal(ok.state().active, 'annotate'); assert.equal(ok.state().tabs.annotate.status, 'idle');
  const edit = await open({config: {f1Mode: 'edit'}, language: 'unknown', languageId: 'plaintext'}); assert.equal(edit.state().active, 'full'); assert.equal(edit.state().mode, 'edit');
  // the choice is free afterwards
  await u.last().h.onMessage({type: 'tab', tab: 'annotate'}); await u.f1.settled(); assert.equal(u.state().active, 'annotate'); assert.equal(u.state().canGenerate, false);
});

// ---------------------------------------------------------------- hook: F2 (SEC-42)
function f2Rig({patterns = [], aiEnabled = false, throws = false, fileName = P('src', 'main.rs'), text = RUST, languageId = 'rust'} = {}) {
  const st = {modal: undefined, patterns, active: null}, ui = {...newUi(), toasts: [], clip: [], dismissed: 0};
  const d = doc(fileName, {text, languageId, throws, log: ui.log});
  const engine = createEngine('js', {globalState: store()});
  const rt = {engine, broker: engine.broker, diagnostics: new Diagnostics(), settings: settingsMod.effective({}), locale: 'en-US', ctx: {secrets: {get: async () => undefined}}, ai: null};
  const vscode = {workspace: {isTrusted: true}, window: {...windowFor(st, ui), withProgress: (o, task) => Promise.resolve(task({report() {}}, {onCancellationRequested: () => ({dispose() {}})}))},
    env: {clipboard: {writeText: async t => { ui.log.push('clipboard'); ui.clip.push(t); }}}, ProgressLocation: {Notification: 15}};
  const fileGate = createFileGate({vscode, rt, L, cfg: () => ({get: k => k === 'secretFilePatterns' ? st.patterns : undefined}), realpath: async p => p, realpathSync: p => p});
  const notice = {show: x => { ui.log.push('notice'); ui.toasts.push(x); }, dismiss: () => { ui.dismissed++; }};
  const f2 = f2mod.createF2({vscode, rt, L, errorText: e => i18n.describeError('en-US', codeOf(e), e && e.message), enabled: () => true, aiEnabled: () => aiEnabled,
    workspaceId: () => engine.workspaceKey(['file:///w']), grantProviderId: sc => `${sc.providerId}:host`, infoFor: () => ({language: languageId === 'rust' ? 'rust' : 'unknown', dialect: {}}), notice, fileGate});
  const run = async (a, b) => { vscode.window.activeTextEditor = st.active = editorOf(d, a, b); await f2.run(); };
  return {f2, st, ui, d, run, fileGate};
}

test('SEC-42 F2: the selection and the file are not read before a secret-class file is allowed; a refusal is a notice and the clipboard is untouched; a non-secret file runs with no dialog', async () => {
  const n = f2Rig(); await n.run(0, 10);
  assert.equal(n.ui.modals.length, 0); assert.equal(n.ui.clip.length, 1, 'the offline template of a non-secret file'); assert.equal(n.ui.log[0], 'read');
  // secret, Esc: not a byte read, no clipboard write, a notice that says why
  const r = f2Rig({fileName: P('.env'), text: 'API_KEY=placeholder\nMODE=dev\n', throws: true}); await r.run(0, 12);
  assert.equal(r.d.reads, 0); assert.deepEqual(r.ui.clip, []); assert.equal(r.ui.modals.length, 1); assert.deepEqual(r.ui.modals[0].buttons, [ALLOW_ONCE, ALLOW_SESSION]);
  assert.equal(r.ui.toasts.length, 1); assert.match(r.ui.toasts[0].text, /^F2 did not run/); assert.match(r.ui.toasts[0].text, /clipboard was not changed/); assert.deepEqual(r.ui.errors, []);
  // no selection: nothing to read, so nothing to ask
  const e = f2Rig({fileName: P('.env'), throws: true}); await e.run(3, 3);
  assert.equal(e.ui.modals.length, 0); assert.match(e.ui.toasts[0].text, /Select some text/);
  // allowed once: the read comes after the dialog, the grant is spent, the next F2 asks again
  const o = f2Rig({fileName: P('notes', 'diary.txt'), patterns: ['diary.txt'], text: 'Remember to buy milk and call the bank before noon.\n', languageId: 'plaintext'}); o.st.modal = ALLOW_ONCE;
  await o.run(0, 20);
  assert.deepEqual(o.ui.log.slice(0, 2), ['modal', 'read']); assert.equal(o.ui.clip.length, 1);
  await o.run(0, 20); assert.equal(o.ui.modals.length, 2);
  // session: once; Keep off: no dialog, a notice
  const s = f2Rig({fileName: P('notes', 'diary.txt'), patterns: ['diary.txt'], text: 'Remember to buy milk and call the bank before noon.\n', languageId: 'plaintext'}); s.st.modal = ALLOW_SESSION;
  await s.run(0, 20); await s.run(0, 20); assert.equal(s.ui.modals.length, 1); assert.equal(s.ui.clip.length, 2);
  const k = f2Rig({fileName: P('.env'), throws: true}); k.fileGate.gate.record(P('.env'), 'deny'); await k.run(0, 5);
  assert.equal(k.ui.modals.length, 0); assert.equal(k.d.reads, 0); assert.deepEqual(k.ui.clip, []); assert.match(k.ui.toasts[0].text, /^F2 did not run/);
  // the OUTBOUND gate is still there: allowing the read of a .env does not let it be copied or sent
  const x = f2Rig({fileName: P('.env'), text: 'API_KEY=placeholder\nMODE=dev\n'}); x.st.modal = ALLOW_SESSION; await x.run(0, 12);
  assert.deepEqual(x.ui.clip, []); assert.match(x.ui.toasts[0].text, /Looks sensitive/);
});

// ---------------------------------------------------------------- hook: annotate.js (SEC-43, SEC-44)
const FileType = {File: 1, Directory: 2, SymbolicLink: 64};
function annRig({patterns = [], dirEntries = [], dirFiles = {}, links = {}, aiEnabled = true, openDocs = [], docFile = P('src', 'main.rs'), docText = RUST, throws = false} = {}) {
  const st = {modal: undefined, patterns, active: null}, ui = {...newUi(), warnings: [], reads: [], infer: 0};
  const d = doc(docFile, {text: docText, languageId: 'rust', throws, log: ui.log});
  const engine = createEngine('js', {globalState: store()});
  const rt = {engine, broker: engine.broker, diagnostics: new Diagnostics(), locale: 'en-US', ctx: {secrets: {get: async () => 'KEY-123456'}},
    settings: settingsMod.effective({providers: {mock: {api: 'openai-responses', endpoint: 'https://api.a.example/v1/responses'}}, scenarios: {f1Insert: {provider: 'mock', model: 'm-insert'}}}),
    ai: {infer: async () => { ui.infer++; return {text: 'this is not the JSON the engine asked for'}; }}};
  const uriOf = p => ({scheme: 'file', fsPath: p, toString: () => `file:///${p.replace(/\\/g, '/')}`});
  const vscode = {
    workspace: {isTrusted: true, textDocuments: openDocs, fs: {
      readDirectory: async () => dirEntries, stat: async () => ({size: 100}),
      readFile: async uri => { ui.reads.push(path.basename(uri.fsPath)); ui.log.push('dirread'); return Buffer.from(dirFiles[path.basename(uri.fsPath)] || 'fn zeta() {}\n'); }}},
    window: {...windowFor(st, ui), showWarningMessage: (msg, ...rest) => { if (!(rest[0] && typeof rest[0] === 'object')) { ui.warnings.push(msg); } return windowFor(st, ui).showWarningMessage(msg, ...rest); },
      withProgress: (o, task) => Promise.resolve(task({report() {}}, {onCancellationRequested: () => ({dispose() {}})}))},
    Uri: {file: uriOf, joinPath: (base, name) => uriOf(path.join(base.fsPath, name))}, FileType, ProgressLocation: {Notification: 15}
  };
  const fileGate = createFileGate({vscode, rt, L, cfg: () => ({get: k => k === 'secretFilePatterns' ? st.patterns : undefined}),
    realpath: async p => { const v = links[p]; if (v instanceof Error) throw v; return v === undefined ? p : v; }, realpathSync: p => links[p] || p});
  const workspaceId = () => engine.workspaceKey(['file:///w']);
  const cfgValues = {autoComment: true, graphComment: false, graphCommentScope: 'directory'};
  const ann = annMod.createAnnotation({vscode, rt, cfg: () => ({get: k => cfgValues[k]}), L, errorText: e => i18n.describeError('en-US', codeOf(e), e && e.message), aiEnabled: () => aiEnabled,
    workspaceId, grantProviderId: sc => `${sc.providerId}:api.a.example`, fileGate});
  const grant = () => engine.broker.authorize({providerId: 'mock:api.a.example', workspace: workspaceId(), scope: 'L3', ttlMs: 3600000});
  return {ann, st, ui, d, rt, fileGate, vscode, grant, cfgValues, insert: () => ann.annotateInsertion({doc: d, info: {language: 'rust', dialect: {}}, template: 'fn added() {}', position: {line: 1, character: 0}})};
}

test('SEC-43 annotate insertion: a protected file is not read and the snippet goes in unchanged; no AI configured means no read and no question at all', async () => {
  const r = annRig({docFile: P('.env'), throws: true}); await r.grant();
  assert.equal(await r.insert(), null);
  assert.equal(r.d.reads, 0); assert.equal(r.ui.infer, 0); assert.equal(r.ui.modals.length, 0, 'never a modal here');
  assert.deepEqual(r.ui.warnings.filter(w => !/not reading it/.test(w)), [], 'the insertion itself says nothing');
  // the gate may announce itself once (non-modal); after "Keep off" nothing at all
  r.fileGate.gate.record(P('.env'), 'deny'); const before = r.ui.notices.length;
  assert.equal(await r.insert(), null); assert.equal(r.ui.notices.length, before); assert.equal(r.d.reads, 0);
  // allowed for the session: the file is read; sending is a separate question and the content scan still refuses a .env (the downstream gate, unchanged)
  const a = annRig({docFile: P('.env')}); await a.grant(); a.fileGate.gate.record(P('.env'), 'session');
  assert.equal(await a.insert(), null); assert.ok(a.d.reads > 0); assert.equal(a.ui.infer, 0); assert.ok(a.ui.warnings.some(w => /looks sensitive/.test(w))); assert.equal(a.ui.modals.length, 0);
  // an allowed file that the content scan accepts goes on to the request (the fake provider answers with something unusable: the snippet stays as it is)
  const m = annRig({docFile: P('notes', 'diary.txt'), patterns: ['diary.txt']}); await m.grant(); m.fileGate.gate.record(P('notes', 'diary.txt'), 'session');
  assert.equal(await m.insert(), null); assert.ok(m.d.reads > 0); assert.equal(m.ui.infer, 1); assert.equal(m.ui.modals.length, 0);
  // non-secret: no gate dialog or notification
  const n = annRig(); await n.grant(); await n.insert(); assert.ok(n.d.reads > 0); assert.equal(n.ui.infer, 1); assert.equal(n.ui.notices.length, 0); assert.equal(n.ui.modals.length, 0);
  // no AI available: not read, not asked (nothing would be read)
  const off = annRig({docFile: P('.env'), aiEnabled: false, throws: true}); assert.equal(await off.insert(), null);
  assert.equal(off.d.reads, 0); assert.equal(off.ui.notices.length, 0); assert.equal(off.ui.modals.length, 0);
  // a one-use grant is spent by the read
  const o = annRig({docFile: P('notes', 'diary.txt'), patterns: ['diary.txt']}); await o.grant(); o.fileGate.gate.record(P('notes', 'diary.txt'), 'once');
  await o.insert(); assert.equal(o.ui.infer, 1); await o.insert(); assert.equal(o.ui.infer, 1, 'the second insertion is not covered');
});

test('SEC-44 directorySymbols: a secret-class sibling is never read (open in the editor or not, any engine answer), a symlink stays excluded, and a folder behind a link is judged by where it really is', async () => {
  const names = ['main.rs', '.env', 'id_rsa', 'secrets.yml', 'util.rs', 'notes.rs', 'link.rs', 'sub'];
  const entries = [['main.rs', FileType.File], ['.env', FileType.File], ['id_rsa', FileType.File], ['secrets.yml', FileType.File], ['util.rs', FileType.File], ['notes.rs', FileType.File],
    ['link.rs', FileType.File | FileType.SymbolicLink], ['sub', FileType.Directory]];
  const files = {'util.rs': 'pub fn util_one() {}\n', 'notes.rs': 'pub fn notes_one() {}\n', 'link.rs': 'pub fn linked() {}\n'};
  // the real engine: .env, id_rsa, secrets.yml are not candidates; the symlink is not a FileType.File; the user pattern removes notes.rs
  const r = annRig({dirEntries: entries, dirFiles: files, patterns: ['notes.rs']});
  const out = await r.ann.directorySymbols(r.d);
  assert.deepEqual(r.ui.reads, ['util.rs']); assert.deepEqual(out.map(x => x.file), ['util.rs']);
  assert.equal(r.ui.modals.length + r.ui.notices.length, 0, 'never a prompt here');
  // an engine that offers EVERYTHING: the names are still filtered (defence in depth), so .env / id_rsa / secrets.yml / notes.rs are not read
  const b = annRig({dirEntries: entries, dirFiles: files, patterns: ['notes.rs']});
  const offered = [];
  b.rt.engine.symbols.dirCandidates = async (current, list) => { offered.push(...list.map(x => x.name)); return names.filter(n => n !== 'sub').map(name => ({name, language: 'rust'})); };
  await b.ann.directorySymbols(b.d);
  assert.ok(offered.includes('util.rs') && offered.includes('link.rs'), 'the engine gets the other names'); assert.ok(!offered.some(n => ['.env', 'id_rsa', 'secrets.yml', 'notes.rs'].includes(n)), `a secret-class name never reaches the engine: ${offered}`);
  assert.deepEqual(b.ui.reads.sort(), ['link.rs', 'main.rs', 'util.rs'], 'only names that are not secret-class (the engine decides about the rest)');
  // a sibling that is open in the editor is not asked for its text either
  const envDoc = doc(P('src', '.env'), {throws: true});
  const o = annRig({dirEntries: entries, dirFiles: files, openDocs: [envDoc]});
  o.rt.engine.symbols.dirCandidates = async () => [{name: '.env', language: 'rust'}, {name: 'util.rs', language: 'rust'}];
  await o.ann.directorySymbols(o.d); assert.equal(envDoc.reads, 0); assert.deepEqual(o.ui.reads, ['util.rs']);
  // the folder is a link to ~/.ssh: every sibling is judged under the real folder, so nothing is read
  const l = annRig({dirEntries: entries, dirFiles: files, links: {[P('src')]: P('home', '.ssh')}});
  assert.deepEqual(await l.ann.directorySymbols(l.d), []); assert.deepEqual(l.ui.reads, []);
  // a failing realpath keeps the plain folder; a document that is not a file has no siblings
  const f = annRig({dirEntries: entries, dirFiles: files, links: {[P('src')]: new Error('EACCES')}}); await f.ann.directorySymbols(f.d); assert.deepEqual(f.ui.reads, ['util.rs', 'notes.rs']);
  const u = annRig({dirEntries: entries}); u.d.uri.scheme = 'untitled'; assert.deepEqual(await u.ann.directorySymbols(u.d), []);
});

// ---------------------------------------------------------------- the real extension.js / languageState.js against a fake `vscode` module (SEC-45)
function fakeVscode(st) {
  const ev = () => ({dispose() {}});
  const item = () => ({text: '', tooltip: '', command: '', name: '', show() {}, hide() {}, dispose() {}});
  const fake = {
    window: {
      ...windowFor(st, st.ui), get activeTextEditor() { return st.active; }, visibleTextEditors: [], // (the getter after the spread: a spread would freeze its value)
      createStatusBarItem: () => { const i = item(); st.items.push(i); return i; }, createOutputChannel: () => ({appendLine() {}, show() {}, dispose() {}}),
      setStatusBarMessage() {}, withProgress: (o, task) => Promise.resolve(task({report() {}}, {onCancellationRequested: () => ev()})),
      onDidChangeActiveTextEditor: ev, onDidChangeTextEditorSelection: ev, onDidChangeVisibleTextEditors: ev, onDidChangeWindowState: ev
    },
    workspace: {
      isTrusted: true, workspaceFolders: [], textDocuments: [],
      getConfiguration: () => ({get: k => st.config[k], update: async (k, v) => { st.config[k] = v; }}),
      registerTextDocumentContentProvider: () => ev(), onDidChangeTextDocument: ev, onDidCloseTextDocument: ev, onDidGrantWorkspaceTrust: ev, onDidChangeConfiguration: ev
    },
    commands: {registerCommand: (id, fn) => { st.commands.set(id, fn); return ev(); }, executeCommand: async (...a) => { st.executed.push(a); }},
    languages: {registerInlineCompletionItemProvider: (sel, p) => { st.provider = p; return ev(); }},
    env: {language: 'en'}, Range, StatusBarAlignment: {Right: 2}, ConfigurationTarget: {Global: 1}, QuickPickItemKind: {Separator: -1}, ProgressLocation: {Notification: 15},
    InlineCompletionItem: class { constructor(insertText, range) { this.insertText = insertText; this.range = range; } },
    SnippetString: class { constructor(value) { this.value = value; } }, FileType, Uri: {file: p => ({fsPath: p, scheme: 'file'}), joinPath: (b, n) => ({fsPath: path.join(b.fsPath, n)})}
  };
  return fake;
}
async function withExtension(fn) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'pime-sec-ext-'));
  const st = {modal: undefined, active: null, ui: {...newUi()}, items: [], commands: new Map(), executed: [], provider: null,
    config: {enabled: true, suggestMode: 'auto', aiEnabled: false, realtimeAI: false, debounceMs: 350, maxVisibleCandidates: 6, pinnedCandidates: [], f1Binding: 'f1', f1ExcludedLanguages: [], f2Binding: 'f2',
      f2ExcludedLanguages: [], uiLocale: 'en-US', engine: 'js', coreExecutable: '', providers: {}, scenarios: {}, budget: {}, dialects: {}, secretFilePatterns: []}};
  const fake = fakeVscode(st);
  const orig = Module._load;
  Module._load = function (request, ...rest) { return request === 'vscode' ? fake : orig.call(this, request, ...rest); };
  const names = ['runtime', 'languageState', 'extension', 'filegate'];
  for (const n of names) delete require.cache[require.resolve(path.join(SRC, n))];
  try {
    const ext = require(path.join(SRC, 'extension')), lang = require(path.join(SRC, 'languageState')), runtime = require(path.join(SRC, 'runtime'));
    const ctx = {subscriptions: [], globalState: store(), workspaceState: store(), secrets: {get: async () => undefined, store: async () => {}, delete: async () => {}}, globalStorageUri: {fsPath: tmp}};
    await ext.activate(ctx);
    await fn({st, ext, lang, runtime, ctx, fake, gate: require(path.join(SRC, 'filegate')).get()});
    ext.deactivate();
  } finally {
    Module._load = orig;
    for (const n of names) delete require.cache[require.resolve(path.join(SRC, n))];
    fs.rmSync(tmp, {recursive: true, force: true});
  }
}

test('SEC-45 extension.js: typing and Show Code Candidates read nothing of a protected file; language detection uses the name only; the status shows the lock and its click opens the command; a normal file is not affected', async () => {
  await withExtension(async ({st, lang, gate}) => {
    const call = (d, line, col) => st.provider.provideInlineCompletionItems(d, {line, character: col}, {}, {isCancellationRequested: false});
    const status = () => st.items[0];
    // ---- a normal Rust file: candidates come, nothing is asked, the status is the ordinary one
    const rs = doc(P('src', 'main.rs'), {text: 'fnm', languageId: 'rust', log: st.ui.log});
    st.active = editorOf(rs, 3);
    const items = await call(rs, 0, 3);
    assert.ok(items.length > 0, 'candidates for "fnm"'); assert.equal(st.ui.modals.length + st.ui.notices.length, 0);
    assert.ok(rs.reads > 0);
    // ---- a secret-class file: language detection (status), typing and the explicit command touch nothing
    const env = doc(P('.env'), {text: 'API_KEY=placeholder\nfnm', languageId: 'plaintext', throws: true, log: st.ui.log});
    st.active = editorOf(env, 0);
    assert.ok(lang.infoFor(env), 'the language of the file is still answered, from its name'); assert.equal(env.reads, 0, 'its head was not read');
    assert.deepEqual(await call(env, 1, 3), []); assert.equal(env.reads, 0);
    assert.equal(st.ui.modals.length, 0, 'never a modal while typing'); assert.equal(st.ui.notices.length, 1, 'one notification');
    for (let i = 0; i < 5; i++) await call(env, 1, 3);
    assert.equal(st.ui.notices.length, 1); assert.equal(env.reads, 0);
    await st.commands.get('programmingIme.showCandidates')(); assert.equal(env.reads, 0); assert.equal(st.ui.modals.length, 0);
    assert.match(st.ui.infos[st.ui.infos.length - 1], /^No candidates: Programming IME does not read this secret-class file/);
    // ---- the status: lock marker, explanation, click = the secret-file command. The gate's decision redraws it (onChange).
    st.ui.notices[0].resolve(NOTE_OFF); await tick();
    assert.match(status().text, /^\$\(lock\) PIME protected file$/); assert.match(status().tooltip, /"\.env" looks like a secret file/); assert.equal(status().command, 'programmingIme.secretFiles');
    assert.ok(st.executed.some(a => a[0] === 'editor.action.inlineSuggest.trigger'), 'the suggestions are asked again after a decision');
    // ---- the command allows it: the status returns to normal and the language is detected from the text now
    st.pick = items => items.find(i => i.action === 'allow');
    env.throws = false; // allowed from here on: reading it is legitimate
    await st.commands.get('programmingIme.secretFiles')();
    assert.doesNotMatch(status().text, /protected file/); assert.equal(status().command, 'programmingIme.chooseLanguage');
    assert.equal(gate.isLocked(env), false); assert.ok(env.reads > 0, 'after the allow, the head is read for the language like any other file');
    // ---- the menu has the entry
    st.pick = items => { st.menu = items.map(i => i.label); return undefined; };
    await st.commands.get('programmingIme.menu')();
    assert.ok(st.menu.includes('Secret files…'), st.menu.join(' | '));
    // ---- the language cache knows whether the head was read: the same document and version, allowed meanwhile, is detected again from its text
    let locked = true; const probe = doc(P('src', 'probe.rs'), {text: RUST, languageId: 'rust'});
    lang.setLockCheck(() => locked); lang.invalidate();
    lang.infoFor(probe); assert.equal(probe.reads, 0, 'locked: the name only');
    locked = false; lang.infoFor(probe); assert.ok(probe.reads > 0, 'allowed: the head is read now, not served from the name-only cache');
  });
});

// ---------------------------------------------------------------- manifest, texts, wiring (SEC-46)
test('SEC-46 manifest and texts: the command and the setting exist, every text exists in three locales with the same placeholders, no setting switches the gate off', () => {
  const pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
  assert.ok(pkg.contributes.commands.some(c => c.command === 'programmingIme.secretFiles' && c.title === '%cmd.secretFiles%'));
  const props = pkg.contributes.configuration.properties, p = props['programmingIme.secretFilePatterns'];
  assert.equal(p.type, 'array'); assert.deepEqual(p.items, {type: 'string'}); assert.deepEqual(p.default, []); assert.equal(p.description, '%cfg.secretFilePatterns%');
  assert.equal(p.scope, 'application', 'user settings only: a workspace cannot weaken the list');
  assert.deepEqual(Object.keys(props).filter(k => /secret|gate|protect/i.test(k)), ['programmingIme.secretFilePatterns'], 'no setting turns the gate off');
  for (const nls of ['package.nls.json', 'package.nls.zh-cn.json', 'package.nls.ja.json']) {
    const t = JSON.parse(fs.readFileSync(path.join(root, nls), 'utf8'));
    assert.ok(t['cmd.secretFiles'] && t['cfg.secretFilePatterns'], nls); assert.ok(/secretFilePatterns|\*\.cred/.test(t['cfg.secretFilePatterns']) || t['cfg.secretFilePatterns'].length > 40, nls);
  }
  const keys = Object.keys(i18n.S).filter(k => k.startsWith('sec.'));
  assert.ok(keys.length >= 29, `${keys.length} sec.* texts`);
  for (const k of keys) {
    for (let i = 0; i < 3; i++) assert.ok(typeof i18n.S[k][i] === 'string' && i18n.S[k][i].trim().length > 0, `${k} locale ${i}`);
    const holesOf = i => (i18n.S[k][i].match(/\{\d+\}/g) || []).sort().join(',');
    assert.equal(holesOf(1), holesOf(0), `${k}: zh-CN placeholders`); assert.equal(holesOf(2), holesOf(0), `${k}: ja-JP placeholders`);
  }
  assert.deepEqual(secretfile.KINDS.map(k => `sec.kind.${k}`).filter(k => !i18n.S[k]), [], 'every kind has a name');
  // the wiring: one shared gate, the hooks in the files that read documents
  const src = n => fs.readFileSync(path.join(SRC, n), 'utf8');
  assert.match(src('extension.js'), /const fileGate = require\('\.\/filegate'\)\.get\(\)/); assert.match(src('extension.js'), /reg\('programmingIme\.secretFiles', \(\) => fileGate\.manage\(\)\)/);
  assert.match(src('extension.js'), /fileGate\.ensure\(doc, \{interactive: false, purpose: 'candidates'\}\)/);
  assert.match(src('f1.js'), /fileGate\.ensure\(editor\.document, \{interactive: true, purpose: 'f1'\}\)/); assert.match(src('f2.js'), /fileGate\.ensure\(editor\.document, \{interactive: true, purpose: 'f2'\}\)/);
  assert.match(src('annotate.js'), /fileGate\.ensure\(doc, \{interactive: false, purpose: 'annotate'\}\)/);
  for (const f of ['f1.js', 'f2.js', 'annotate.js']) assert.match(src(f), /fileGate: require\('\.\/filegate'\)\.get\(\)/, `${f}: the real wiring passes the shared gate`);
  assert.match(src('languageState.js'), /require\('\.\/filegate'\)\.get\(\)\.isLocked\(doc\)/);
  // no document is read anywhere else in the bridge: every getText / lineAt of a document sits after a gate call or in code that only runs after one
  const readers = fs.readdirSync(SRC).filter(n => n.endsWith('.js')).filter(n => /\.getText\(|\.lineAt\(|readFile\(/.test(src(n))).sort();
  assert.deepEqual(readers, ['annotate.js', 'extension.js', 'f1.js', 'f2.js', 'languageState.js'], 'a new reader of document content must be wired to the gate (and listed here)');
});
