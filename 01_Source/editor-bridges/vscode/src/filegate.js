'use strict';
// The secret-file read gate of the VS Code bridge. BEFORE the extension reads the content of a document for any purpose (candidates while
// typing, language detection, F1, F2, the insertion annotation, the sibling symbol index) it asks this module, which looks at the file NAME
// and PATH only (core/secretfile.js: .env, private keys, keystores, credentials ...). A secret-class file is read only after the user
// confirmed it. Reading (local) and sending (to an AI provider) are separate questions: this prompt only decides whether Programming IME
// may read the file at all; the content scan / redaction / AI consent downstream stay as they are (docs/SECRET_FILES.md).
//
//   ensure(doc, {interactive, purpose}) -> Promise<boolean>   true: read it, false: do not read anything
//     not secret                       true, no dialog, nothing read
//     secret, the gate allows it       the grant is spent (a one-use grant is gone after this call), true
//     secret, the user said "Keep off" false, silently (the state is remembered for the session)
//     secret, nothing decided yet      interactive (an explicit command: F1, F2): a MODAL with "Allow once" / "Allow for this session";
//                                      Esc or Cancel refuses THIS call and remembers nothing, the next command asks again.
//                                      not interactive (typing, candidates): never a modal (it would steal keystrokes); ONE non-modal
//                                      notification per file and session ("Allow this file for this session" / "Keep off"); false meanwhile.
//   isLocked(doc) / lockInfo(doc)     sync, no dialog, no read: the status bar and the language detection use it
//   manage()                          the command programmingIme.secretFiles: forget decisions, allow the active file explicitly
//
// A link, a junction or a hard link hides its target behind an innocent name, so the REAL path of a file-scheme document is resolved
// (fs.realpath, the plain path when that fails) and BOTH paths are classified: the file is secret if either is. The decision is kept under
// the real path, so every name of one file shares it. A hard link cannot be seen by name or by realpath and stays uncovered.
// `vscode`, the runtime, the settings and the file system are injected (createFileGate), so the flow is tested against fakes; get() wires
// the real ones once, and every module of the bridge shares that one instance (a session grant given while typing also covers F1).
const fs = require('node:fs');
const path = require('node:path');
const {classifyFile, SecretFileGate, KINDS} = require('./core/secretfile');

const REAL_TTL_MS = 5000, REAL_MAX = 200, NOTIFIED_MAX = 500, VERDICTS_MAX = 500; // real-path cache: the typing path asks on every keystroke
const PURPOSES = Object.freeze(['f1', 'f2', 'candidates', 'annotate']);
const KIND_SET = new Set(KINDS);

const realpathAsync = p => new Promise((resolve, reject) => { fs.realpath.native(p, (e, r) => e ? reject(e) : resolve(r)); }); // native: also expands 8.3 short names
const realpathSync = p => fs.realpathSync.native(p);

function createFileGate(env) {
  const {vscode, rt, L} = env;
  const cfg = env.cfg || (() => ({get: () => undefined}));
  const now = typeof env.now === 'function' ? env.now : Date.now;
  const realpath = env.realpath || realpathAsync, realpathSyncFn = env.realpathSync || realpathSync;
  const gate = env.gate || new SecretFileGate({now});
  const realCache = new Map();   // path -> {real, at}
  const notified = new Set();    // gate keys that already got their one non-modal notification this session
  const listeners = [];

  // diagnostics hold the rule id and the event only: never a path, a file name or content (the whitelist would filter them anyway)
  const note = (event, a, purpose) => {
    try { rt.diagnostics.record({component: 'secretfile', error_code: a && a.c ? a.c.rule : 'OK', request_type: `sec.${event}.${purpose}`}); } catch { /* diagnostics never break the gate */ }
  };
  const emit = () => { for (const fn of listeners) { try { fn(); } catch { /* a listener must not break a decision */ } } };
  const userPatterns = () => { try { const v = cfg().get('secretFilePatterns'); return Array.isArray(v) ? v : []; } catch { return []; } };
  const kindText = kind => KIND_SET.has(kind) ? L(`sec.kind.${kind}`) : String(kind);
  const remember = (set, key, max) => { set.add(key); while (set.size > max) set.delete(set.values().next().value); };

  // ---------------------------------------------------------------- paths
  const givenPaths = doc => {
    const u = doc && doc.uri, out = [];
    for (const p of [u && u.fsPath, doc && doc.fileName]) if (typeof p === 'string' && p !== '' && !out.includes(p)) out.push(p);
    return out;
  };
  const toResolve = doc => doc && doc.uri && doc.uri.scheme === 'file' ? givenPaths(doc)[0] || '' : ''; // other schemes have no real path to resolve
  const fresh = hit => hit && now() - hit.at < REAL_TTL_MS;
  const keep = (p, real) => { realCache.delete(p); realCache.set(p, {real, at: now()}); while (realCache.size > REAL_MAX) realCache.delete(realCache.keys().next().value); return real; };
  async function realPath(p) {
    const hit = realCache.get(p);
    if (fresh(hit)) return hit.real;
    let real = p;
    try { const r = await realpath(p); if (typeof r === 'string' && r !== '') real = r; } catch { /* the plain path */ }
    return keep(p, real);
  }
  function realPathSync(p) {
    const hit = realCache.get(p);
    if (fresh(hit)) return hit.real;
    let real = p;
    try { const r = realpathSyncFn(p); if (typeof r === 'string' && r !== '') real = r; } catch { /* the plain path */ }
    return keep(p, real);
  }
  // The classification is pure and costs a few hundred microseconds, and the typing path asks on every keystroke: remember the answers (keyed by path AND patterns).
  const verdicts = new Map();
  function classify(p, patterns) {
    if (p.length > 1024) return classifyFile({path: p, userPatterns: patterns}); // not worth remembering (and it would hold a lot of memory)
    const key = JSON.stringify([p, patterns]);
    let c = verdicts.get(key);
    if (!c) { c = classifyFile({path: p, userPatterns: patterns}); verdicts.set(key, c); while (verdicts.size > VERDICTS_MAX) verdicts.delete(verdicts.keys().next().value); }
    return c;
  }
  // -> null (not secret) | {path: the key path of the decision, c: the classification}. Nothing here catches: a bug must not become "not secret".
  function assess(doc, real) {
    const paths = givenPaths(doc);
    if (real && !paths.includes(real)) paths.push(real);
    if (!paths.length) return null;
    const patterns = userPatterns();
    let hit = null;
    for (const p of paths) { const c = classify(p, patterns); if (c.secret && !hit) hit = c; }
    return hit ? {path: real || paths[0], c: hit} : null;
  }
  const spend = a => gate.consume(a.path); // a one-use grant is gone after this; a session grant stays; false: expired meanwhile -> do not read

  // ---------------------------------------------------------------- the decision
  async function askModal(a, purpose) {
    note('prompt', a, purpose);
    const once = L('sec.allowOnce'), session = L('sec.allowSession');
    let choice;
    try { choice = await vscode.window.showWarningMessage(L('sec.confirm', a.c.label, kindText(a.c.kind)), {modal: true, detail: L('sec.confirm.detail')}, once, session); } catch { choice = undefined; }
    if (choice !== once && choice !== session) { note('refused', a, purpose); return false; } // Esc: this call only, nothing is remembered
    gate.record(a.path, choice === once ? 'once' : 'session');
    const ok = spend(a); // right after the record, no await in between
    note(choice === once ? 'once' : 'session', a, purpose);
    emit();
    return ok;
  }
  function notifyOnce(a, purpose) {
    const key = gate.key(a.path);
    if (notified.has(key)) return;
    remember(notified, key, NOTIFIED_MAX);
    note('notice', a, purpose);
    const allow = L('sec.notice.allow'), keepOff = L('sec.notice.keepOff');
    let shown;
    try { shown = vscode.window.showWarningMessage(L('sec.notice', a.c.label, kindText(a.c.kind)), allow, keepOff); } catch { return; }
    Promise.resolve(shown).then(choice => { // never awaited by the typing path
      if (choice === allow) { gate.record(a.path, 'session'); note('session', a, 'notice'); emit(); }
      else if (choice === keepOff) { gate.record(a.path, 'deny'); note('deny', a, 'notice'); emit(); }
    }, () => {});
  }
  async function ensure(doc, opts) {
    const interactive = !!opts && opts.interactive === true, purpose = opts && PURPOSES.includes(opts.purpose) ? opts.purpose : 'other';
    let a;
    try {
      const p = toResolve(doc);
      a = assess(doc, p ? await realPath(p) : null);
    } catch { note('internal', null, purpose); return false; } // fail closed: an error must not turn into "read it"
    if (!a) return true; // not a secret-class name: no dialog, nothing read here
    // from here on there is no await before the state is read, so concurrent callers cannot both show a notification
    const state = gate.state(a.path);
    if (state === 'deny') return false;
    if (state === 'allow') return spend(a);
    if (interactive) return askModal(a, purpose);
    notifyOnce(a, purpose);
    return false;
  }

  // ---------------------------------------------------------------- status bar and language detection (sync: no dialog, no read of content)
  function lockInfo(doc) {
    try {
      const p = toResolve(doc);
      const a = assess(doc, p ? realPathSync(p) : null);
      if (!a || gate.state(a.path) === 'allow') return null;
      return {label: a.c.label, kind: a.c.kind, rule: a.c.rule, state: gate.state(a.path)};
    } catch { return {label: '', kind: null, rule: 'internal', state: 'ask'}; } // fail closed
  }
  const isLocked = doc => lockInfo(doc) !== null;
  const isSecretPath = p => classify(p, userPatterns()).secret; // names only (the siblings of a folder)

  // ---------------------------------------------------------------- the command
  const forget = key => { notified.delete(key); }; // a forgotten decision may be announced again
  async function manage() {
    const editor = vscode.window.activeTextEditor, items = [];
    let active = null;
    if (editor) { try { const p = toResolve(editor.document); active = assess(editor.document, p ? realPathSync(p) : null); } catch { active = null; } }
    if (active && gate.state(active.path) !== 'allow') items.push({label: `$(unlock) ${L('sec.pick.allowActive', active.c.label)}`, action: 'allow', active});
    const list = gate.list();
    for (const e of list) {
      items.push({label: `$(${e.state === 'allow' ? 'check' : 'circle-slash'}) ${e.label}`, description: L(`sec.pick.${e.state}`, new Date(e.expiresAt).toLocaleString()), detail: path.dirname(e.key), action: 'revoke', entry: e});
    }
    if (list.length) items.push({label: `$(trash) ${L('sec.pick.revokeAll')}`, action: 'revokeAll'});
    if (!items.length) { vscode.window.showInformationMessage(L('sec.pick.none')); return; }
    const pick = await vscode.window.showQuickPick(items, {title: L('sec.pick.title')});
    if (!pick) return;
    if (pick.action === 'allow') {
      gate.record(pick.active.path, 'session'); note('manual', pick.active, 'manual'); emit();
      vscode.window.showInformationMessage(L('sec.allowed', pick.active.c.label));
    } else if (pick.action === 'revoke') {
      gate.revoke(pick.entry.key); forget(pick.entry.key); note('revoke', null, 'manual'); emit();
      vscode.window.showInformationMessage(L('sec.revoked', pick.entry.label));
    } else if (pick.action === 'revokeAll') {
      gate.clear(); notified.clear(); note('revokeAll', null, 'manual'); emit();
      vscode.window.showInformationMessage(L('sec.revokedAll'));
    }
  }

  return {
    ensure, isLocked, lockInfo, isSecretPath, manage, realPath, gate,
    list: () => gate.list(),
    onChange: fn => { if (typeof fn === 'function') listeners.push(fn); }
  };
}

// The gate a module uses when none was injected (the fakes of the tests): a real one with the names of the built-in rules, no user
// patterns, and dialogs through the injected `vscode`. A file that is not secret-class never touches any of it.
const gateFor = env => env.fileGate || createFileGate({vscode: env.vscode, rt: env.rt, L: env.L, cfg: env.cfg});

let shared = null;
function get() { // the one gate of the extension, created on first use
  if (shared) return shared;
  const vscode = require('vscode'), runtime = require('./runtime');
  shared = createFileGate({vscode, rt: runtime.rt, L: runtime.L, cfg: runtime.cfg});
  return shared;
}

module.exports = {createFileGate, gateFor, get};
