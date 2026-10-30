'use strict';
// Engine adapter (migration steps 1.0/1.1). `programmingIme.engine` selects who owns the pure logic:
//   js   - the JavaScript modules in ./core (the v0.2 behaviour); a configured Rust core is only an optional accelerator
//          for local candidates and the JS implementation is the fallback.
//   rust - the Rust core (`pime-core`, protocol 0.3) is REQUIRED. There is no silent fallback: when it is missing or
//          dies the IME features are switched off and the reason is shown; typing in VS Code itself is never affected.
// The rest of the extension only talks to this adapter, so it contains no `if (rust)` branches. The adapter provides
//   engine.coordinator  same methods as core/transaction.js EditCoordinator   (createPlan, prepare, apply, invalidate, purge)
//   engine.broker       same methods as core/grants.js PermissionBroker       (+ findNow, revokeProviderPrefix)
//   engine.snapshot / excerpt / validateSnapshot / propose / select / resultText / hunkEdits   (the F1 edit path)
// Every method may return a Promise (Rust) or a plain value (JS): callers always `await`.
const fs = require('node:fs');
const crypto = require('node:crypto');
const {CoreProcess} = require('./coreprocess');
const {PermissionBroker, workspaceToken} = require('./core/grants');
const {EditCoordinator} = require('./core/transaction');
const ctxMod = require('./core/context');
const diffMod = require('./core/diff');
const {validateSource} = require('./core/validators');
const {applyEdits} = require('./core/textrange');
const {fail} = require('./core/errors');

const PROTOCOL_ANY = /^0\.[23]/;   // an optional core (js engine) may speak 0.2 or 0.3
const PROTOCOL_RUST = /^0\.3/;     // session operations need 0.3
const START_TIMEOUT_MS = 5000;
const EDIT_TIMEOUT_MS = 30000;     // document-sized requests
const MAX_REPLY_BYTES = 32 * 1024 * 1024;
const GRANTS_KEY = 'pime.grants.v1';
const PREVIEW_UNITS = 4000;
const GATE_TTL_MS = 5000;

// ------------------------------------------------------------------------------------------------ js engine
class JsBroker extends PermissionBroker {
  findNow(criteria) { return this.find(criteria); } // synchronous gate of the realtime scheduler
  revokeProviderPrefix(prefix) { let n = 0; for (const g of this.grants.values()) if (g.providerId.startsWith(prefix)) { this.revoke(g.id); n++; } return n; }
}
const resultOf = (text, language, dialect) => ({
  length: text.length, hash: ctxMod.sha256(text), preview: text.slice(0, PREVIEW_UNITS), truncated: text.length > PREVIEW_UNITS,
  validation: validateSource(language, text, dialect)
});

class JsEngine {
  constructor({globalState}) {
    this.kind = 'js'; this.core = null; this.reason = '';
    this.broker = new JsBroker({store: globalState});
    this.coordinator = new EditCoordinator();
    this.fallbackAllowed = true; this.predictTimeoutMs = 0; // 0 = the CoreProcess default (100 ms)
  }
  available() { return true; }
  predictClient() { return this.core && !this.core.dead ? this.core : null; }
  describe() { return this.core && !this.core.dead ? 'rust' : 'js-fallback'; }
  // Optional accelerator: spawn it when configured, forget it when it does not answer a health check.
  start({binary}) {
    this.stop();
    if (!binary || !fs.existsSync(binary)) return Promise.resolve();
    const drop = () => { if (this.core) { this.core.stop(); this.core = null; } };
    try {
      this.core = new CoreProcess(binary);
      // the health check runs in the background: activation never waits for an optional accelerator
      this.core.request({op: 'health'}).then(r => { if (!r.ok || !PROTOCOL_ANY.test(String(r.protocol))) drop(); }).catch(drop);
    } catch { this.core = null; }
    return Promise.resolve();
  }
  stop() { if (this.core) { this.core.stop(); this.core = null; } }
  applySettings() {}
  noteFailure() {}
  workspaceKey(uris) { return workspaceToken(uris); }
  // ---- the edit path (the JS snapshot keeps its text; the core never sees it)
  snapshot(args) { return ctxMod.makeSnapshot(args); }
  release() {}
  excerpt(snapshot, offset, bounds) { return ctxMod.excerpt(snapshot.content, offset, bounds); }
  validateSnapshot(snapshot, language, dialect, edits) { return validateSource(language, edits ? applyEdits(snapshot.content, edits) : snapshot.content, dialect); }
  // -> {review, hunks: [{id, old/new geometry, header, lines}], result}
  propose(snapshot, proposed, {language, dialect}) {
    const hunks = diffMod.buildHunks(snapshot.content, proposed);
    const view = hunks.map(h => ({id: h.id, oldStartLine: h.oldStartLine, oldLineCount: h.oldLineCount, newStartLine: h.newStartLine, newLineCount: h.newLineCount, startOffset: h.startOffset, endOffset: h.endOffset, ...diffMod.renderHunk(snapshot.content, h)}));
    const review = {hunks};
    return {review, hunks: view, result: this.select(snapshot, review, hunks.map(h => h.id), {language, dialect})};
  }
  select(snapshot, review, ids, {language, dialect}) { return resultOf(diffMod.applyHunks(snapshot.content, review.hunks, ids), language, dialect); }
  resultText(snapshot, review, ids) { return diffMod.applyHunks(snapshot.content, review.hunks, ids); }
  hunkEdits(review, ids) { return diffMod.hunkEdits(review.hunks, ids); }
}

// ------------------------------------------------------------------------------------------------ rust engine
// EditCoordinator over the core: createPlan/prepare map to edit.plan/edit.prepare, and apply is split into
// edit.begin_apply -> the editor writes -> edit.finish_apply (the core marks the token used BEFORE the write).
class RustCoordinator {
  constructor(engine) { this.engine = engine; this.queue = Promise.resolve(); }
  // edits: [{start, end, text}] or the selection object returned by engine.hunkEdits (proposal + hunk ids)
  async createPlan({kind, snapshot, edits, targets = [], selection}) {
    const body = {op: 'edit.plan', kind, targets, selection};
    if (Array.isArray(edits)) { body.snapshot_id = snapshot.snapshot_id; body.edits = edits; } else { body.proposal = edits.proposal; body.hunk_ids = edits.hunkIds; }
    return (await this.engine.rpc(body)).plan;
  }
  async prepare(planId, {state, confirmed, grantId, grantOk, validationOk}) {
    return (await this.engine.rpc({op: 'edit.prepare', plan: planId, state, confirmed, grant_id: grantId, grant_ok: grantOk, validation_ok: validationOk}, EDIT_TIMEOUT_MS)).token;
  }
  // adapter: {getState(), applyEdit(edits) -> Promise<boolean>}. Serialised so our own applies never interleave.
  apply(tokenId, adapter) {
    const run = this.queue.then(() => this._apply(tokenId, adapter));
    this.queue = run.catch(() => {});
    return run;
  }
  async _apply(tokenId, adapter) {
    const begin = await this.engine.rpc({op: 'edit.begin_apply', token: tokenId, state: adapter.getState()}, EDIT_TIMEOUT_MS);
    let outcome, timer;
    const deadline = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('APPLY_TIMEOUT')), begin.apply_timeout_ms); });
    try { outcome = (await Promise.race([Promise.resolve(adapter.applyEdit(begin.edits)), deadline])) === false ? 'rejected' : 'accepted'; }
    catch { outcome = 'error'; } finally { clearTimeout(timer); }
    const done = await this.engine.rpc({op: 'edit.finish_apply', token: tokenId, outcome, state: adapter.getState()}, EDIT_TIMEOUT_MS);
    return {state: done.state, newVersion: done.newVersion, newHash: done.newHash};
  }
  // fire and forget: the pipe keeps the order, so any later request sees the invalidation
  invalidate(uri) { return this.engine.rpc({op: 'edit.invalidate', uri}).then(r => r.invalidated, () => 0); }
  purge() { return this.engine.rpc({op: 'edit.purge'}).then(() => {}, () => {}); }
  discard(planId) { return this.engine.rpc({op: 'edit.discard', plan: planId}).then(() => {}, () => {}); }
  async statusOf(planId) { return (await this.engine.rpc({op: 'edit.status', plan: planId})).status; }
}

// PermissionBroker over the core. The core owns every grant; this class only forwards and persists what the core
// reports (`persist`) in globalState, in the shape the JS broker always stored.
class RustBroker {
  constructor(engine, store) { this.engine = engine; this.store = store; this.gate = new Map(); }
  _workspace(w) { return typeof w === 'string' ? {workspace: w} : {workspace_uris: w.uris}; }
  _criteria(c) { return {provider_id: c.providerId, ...this._workspace(c.workspace), scope: c.scope}; }
  _key(c) { return `${c.providerId}|${typeof c.workspace === 'string' ? c.workspace : JSON.stringify(c.workspace.uris)}|${c.scope}`; }
  _changed(r) { this.gate.clear(); if (this.store && r.persist) this.store.update(GRANTS_KEY, r.persist); return r; }
  async load() { // persisted grants -> core (at session start)
    const saved = this.store && this.store.get(GRANTS_KEY);
    if (Array.isArray(saved) && saved.length) await this.engine.rpc({op: 'grant.import', grants: saved});
  }
  async authorize({providerId, workspace, scope, ttlMs, once = false}) {
    const r = this._changed(await this.engine.rpc({op: 'grant.authorize', provider_id: providerId, ...this._workspace(workspace), scope, ttl_ms: ttlMs, once}));
    if (!once) this.gate.set(this._key({providerId, workspace, scope}), {grant: r.grant, at: Date.now()});
    return r.grant;
  }
  _id(grantId) { return typeof grantId === 'string' ? grantId : ''; } // a missing id is simply an unknown grant, as in the JS broker
  async check(grantId, c) { return (await this.engine.rpc({op: 'grant.check', grant_id: this._id(grantId), ...this._criteria(c)})).result; }
  async find(c) {
    const grant = (await this.engine.rpc({op: 'grant.find', ...this._criteria(c)})).grant;
    this.gate.set(this._key(c), {grant, at: Date.now()});
    return grant;
  }
  // Synchronous gate for the realtime scheduler: the last answer of find(), refreshed in the background. It only decides
  // whether to try; the request itself is authorised by require() just before anything is sent.
  findNow(c) {
    const key = this._key(c), hit = this.gate.get(key);
    if (!hit || Date.now() - hit.at > GATE_TTL_MS) this.find(c).catch(() => {});
    return hit ? hit.grant : null;
  }
  async require(grantId, c) { return (await this.engine.rpc({op: 'grant.require', grant_id: this._id(grantId), ...this._criteria(c)})).grant; }
  async stillValid(grantId) { return (await this.engine.rpc({op: 'grant.still_valid', grant_id: this._id(grantId)})).valid; }
  async consume(grantId) { await this.engine.rpc({op: 'grant.consume', grant_id: this._id(grantId)}); }
  async revoke(grantId) { return this._changed(await this.engine.rpc({op: 'grant.revoke', grant_id: this._id(grantId)})).revoked; }
  async revokeProvider(providerId) { return this._changed(await this.engine.rpc({op: 'grant.revoke_provider', provider_id: providerId})).count; }
  async revokeProviderPrefix(prefix) { return this._changed(await this.engine.rpc({op: 'grant.revoke_provider_prefix', prefix})).count; }
  async revokeAll() { return this._changed(await this.engine.rpc({op: 'grant.revoke_all'})).count; }
  async list() { return (await this.engine.rpc({op: 'grant.list'})).grants; }
}

class RustEngine {
  constructor({globalState, onChange = () => {}}) {
    this.kind = 'rust'; this.core = null; this.session = null; this.state = 'starting'; this.reason = ''; this.onChange = onChange;
    this.broker = new RustBroker(this, globalState);
    this.coordinator = new RustCoordinator(this);
    this.fallbackAllowed = false; this.predictTimeoutMs = 2000; // no fallback: a slow answer means "no candidates this keystroke"
  }
  available() { return this.state === 'ready' && !!this.core && !this.core.dead; }
  predictClient() { return this.available() ? this.core : null; }
  describe() { return this.available() ? 'rust' : 'unavailable'; }
  _unavailable(reason) {
    if (this.state === 'unavailable') return false;
    this.state = 'unavailable'; this.reason = reason; this.session = null;
    const core = this.core; this.core = null;
    if (core) core.stop();
    this.onChange(); return false;
  }
  noteFailure(reason = 'the core stopped responding') { return this._unavailable(reason); }
  // Spawns the core, checks health (protocol 0.3), opens the session and applies the settings. Never throws.
  async start({binary, locale, values}) {
    this.stop(); this.state = 'starting'; this.reason = '';
    if (!binary) return this._unavailable('programmingIme.coreExecutable is empty');
    if (!fs.existsSync(binary)) return this._unavailable('programmingIme.coreExecutable does not exist');
    let core;
    try { core = this.core = new CoreProcess(binary, {timeoutMs: START_TIMEOUT_MS, maxBuffer: MAX_REPLY_BYTES, onStop: () => { if (this.core === core) this._unavailable('the core process ended'); }}); }
    catch { return this._unavailable('the core process could not be started'); }
    try {
      const health = await core.request({op: 'health'});
      if (!health.ok) return this._unavailable('the core did not answer the health check');
      if (!PROTOCOL_RUST.test(String(health.protocol))) return this._unavailable(`the core speaks protocol ${health.protocol}, 0.3 is required`);
      const opened = await core.request({op: 'session.open', locale, entropy: crypto.randomBytes(32).toString('hex'), settings: values});
      if (!opened.ok) return this._unavailable(`session.open failed (${opened.error})`);
      this.session = opened.session; this.state = 'ready';
      this.applySettings(values, locale); // the bridge's settings follow the session, as they do on every later change
      await this.broker.load();
      this.onChange();
      return true;
    } catch { return this._unavailable('the core did not answer in time'); }
  }
  stop() { const core = this.core; this.core = null; this.session = null; if (core) core.stop(); }
  // Settings reach the core in order with every later request (one pipe), so this does not need to be awaited.
  applySettings(values, locale) {
    if (!this.available()) return;
    this.core.request({op: 'settings.apply', session: this.session, values, locale}, {timeoutMs: START_TIMEOUT_MS}).catch(() => {});
  }
  // One request to the core's session. A failure becomes a PimeError with the core's error code; a dead core switches
  // the IME features off (reported through onChange), a slow one only fails this request.
  async rpc(body, timeoutMs = START_TIMEOUT_MS) {
    if (!this.available()) throw fail('IPC_TIMEOUT', 'the Programming IME core is not available');
    let r;
    try { r = await this.core.request({...body, session: this.session}, {timeoutMs}); }
    catch { if (this.core && this.core.dead) this.noteFailure(); throw fail('IPC_TIMEOUT', 'the Programming IME core did not answer in time'); }
    if (!r.ok) {
      if (r.error === 'CORE_UNAVAILABLE' || r.error === 'NO_SESSION') this.noteFailure(r.error === 'NO_SESSION' ? 'the core lost the session' : 'the core stopped');
      throw fail(r.error || 'INTERNAL', r.message || r.error);
    }
    return r;
  }
  workspaceKey(uris) { return {uris}; }
  // ---- the edit path: the core keeps the snapshot text; the JS snapshot also keeps it because the AI request (still built
  // in JS until step 1.2) needs it, and it is sent to the core exactly once
  async snapshot({uri, version, text, languageId, selection, isDirty, fileName, level, editorInstanceId}) {
    const r = await this.rpc({op: 'ctx.snapshot', uri, version, text, languageId, selection, isDirty, fileName, level, editorInstanceId}, EDIT_TIMEOUT_MS);
    return {...r.snapshot, content: text};
  }
  release(snapshot) { if (snapshot.snapshot_id) this.rpc({op: 'ctx.release', snapshot_id: snapshot.snapshot_id}).catch(() => {}); }
  async excerpt(snapshot, offset, {before, after}) { return (await this.rpc({op: 'ctx.excerpt', snapshot_id: snapshot.snapshot_id, cursor: offset, before, after})).excerpt; }
  async validateSnapshot(snapshot, language, dialect, edits) { return (await this.rpc({op: 'ctx.validate', snapshot_id: snapshot.snapshot_id, language, dialect, edits}, EDIT_TIMEOUT_MS)).result; }
  async propose(snapshot, proposed, {language, dialect}) {
    const r = await this.rpc({op: 'edit.propose', snapshot_id: snapshot.snapshot_id, proposed_text: proposed, language, dialect}, EDIT_TIMEOUT_MS);
    return {review: {proposal: r.proposal}, hunks: r.hunks, result: r.result};
  }
  async select(snapshot, review, ids, {language, dialect}) { return (await this.rpc({op: 'edit.select', proposal: review.proposal, hunk_ids: ids, language, dialect}, EDIT_TIMEOUT_MS)).result; }
  async resultText(snapshot, review, ids) { return (await this.rpc({op: 'edit.result', proposal: review.proposal, hunk_ids: ids}, EDIT_TIMEOUT_MS)).text; }
  hunkEdits(review, ids) { return {proposal: review.proposal, hunkIds: ids}; }
}

function createEngine(kind, deps) { return kind === 'rust' ? new RustEngine(deps) : new JsEngine(deps); }
module.exports = {createEngine, JsEngine, RustEngine};
