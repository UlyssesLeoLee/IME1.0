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
//   engine.createAi / budget / buildPrompt / parseModelResult                  (steps 1.2: request logic; the HTTP call stays here in JS)
//   engine.privacy / annotate / symbols                                        (outbound gate, annotations, symbol index)
//   engine.createScheduler                                                     (step 1.3: the realtime scheduling decisions)
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
const aiMod = require('./core/ai');
const secretsMod = require('./core/secrets');
const annotateMod = require('./core/annotate');
const symbolsMod = require('./core/symbols');
const languagesMod = require('./core/languages');
const {RealtimeScheduler} = require('./core/scheduler');

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
    this.budget = new aiMod.Budget();
    this.privacy = {
      assess: (text, {fileName, scope}) => {
        const v = secretsMod.assessOutbound(text, {fileName, scope});
        return {action: v.action, scanned: {highest: v.scanned.highest, sensitiveFile: v.scanned.sensitiveFile, blocked: v.scanned.blocked, truncated: v.scanned.truncated}, summary: secretsMod.summarize(v.scanned.findings)};
      },
      redactionSet: () => ({map: [], count: 0}),
      redact: (text, set) => { const red = secretsMod.redact(text, secretsMod.scan(text, {fileName: ''}).findings); set.map.push(...red.map); set.count = set.map.length; return red.text; },
      restore: (text, set) => set.map.length ? secretsMod.restore(text, set.map) : {text, unresolved: 0},
      release: () => {}
    };
    this.annotate = {supports: profile => annotateMod.supportsAnnotation(profile), prompt: a => annotateMod.buildAnnotatePrompt(a), f1Addendum: a => annotateMod.f1Addendum(a), apply: (raw, a) => annotateMod.applyAnnotateReply(raw, a)};
    this.symbols = {
      extract: (profile, text, max) => symbolsMod.extractSymbols(profile, text, max),
      // entries: [{name, isFile}] -> [{name, language}]: the files of a folder worth reading (no credential-like names, a known extension)
      dirCandidates: (currentName, entries) => {
        const out = [];
        for (const {name, isFile} of entries) {
          if (!isFile || name === currentName || secretsMod.isSensitiveFileName(name)) continue;
          const decision = languagesMod.resolveLanguage({languageId: '', fileName: name});
          if (decision.reason === 'extension' || decision.reason === 'bridge-languageId') out.push({name, language: decision.language});
        }
        return out;
      }
    };
  }
  createAi({diagnostics, isEnabled, authorize}) { return new aiMod.AiClient({budget: this.budget, diagnostics, isEnabled, authorize}); }
  buildPrompt(task, payload) { return aiMod.buildPrompt(task, payload); }
  parseModelResult(raw, options) { return aiMod.parseModelResult(raw, options); }
  createScheduler(options) { return new RealtimeScheduler(options); }
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

// The budget lives in the core (ai.prepare reserves, ai.complete settles); the status bar and the usage message read it
// synchronously, so this view keeps the figures of the last reply (every AI reply carries them).
class RustBudgetView {
  constructor(engine) {
    this.engine = engine; this.cache = {requests: 0, tokens: 0, estimated: false, cost: null};
    this.maxRequests = Infinity; this.maxTokens = Infinity; this.price = 0; // the limits are configured in the core by applySettings
  }
  _sync(snapshot) { this.cache = snapshot; }
  snapshot() { return this.cache; }
  reset() {
    this.cache = {requests: 0, tokens: 0, estimated: false, cost: this.cache.cost === null ? null : 0};
    if (this.engine.available()) this.engine.rpc({op: 'budget.reset'}).catch(() => {});
  }
}

// AiClient over the core: ai.prepare does everything that has to happen before bytes leave (checks, authorisation, the
// sensitivity gate, the budget reservation, effort negotiation, the request), this class sends it - the one thing the core
// does not do - and reports the outcome to ai.complete, which maps errors, parses the reply and settles the budget.
class RustAiClient {
  constructor({engine, diagnostics = null, isEnabled = () => true, isTrusted = () => true, workspace, now = Date.now, fetchImpl = (...a) => fetch(...a)}) {
    Object.assign(this, {engine, diagnostics, isEnabled, isTrusted, workspace, now, fetchImpl});
    this.sentCount = 0; this.inFlight = 0; this.maxInFlight = 0;
  }
  // req: the same fields as AiClient.infer
  async infer(req) {
    const t0 = this.now();
    const record = (code, extra) => this.diagnostics && this.diagnostics.record({component: 'ai', error_code: code || 'OK', elapsed_ms: this.now() - t0, request_type: req.scenario, ...extra});
    try {
      const w = this.workspace();
      const prepared = await this.engine.rpc({
        op: 'ai.prepare', enabled: !!this.isEnabled(), trusted: !!this.isTrusted(), instructions: req.instructions, input: req.input, provider_id: req.providerId, grant_scope: req.grantScope,
        ...(typeof w === 'string' ? {workspace: w} : {workspace_uris: w.uris}), grant_id: req.grantId, allow_sensitive: !!req.allowSensitive,
        family: req.family, endpoint: req.endpoint, key: req.key, model: req.model, max_output_tokens: req.maxOutputTokens, timeout_ms: req.timeoutMs,
        effort: req.effort, capability: req.capability, api_version: req.apiVersion, max_tokens_param: req.maxTokensParam
      }, EDIT_TIMEOUT_MS);
      // own (ref'd) timer instead of AbortSignal.timeout: the deadline must hold even if nothing else keeps the loop alive
      const deadline = new AbortController();
      const timer = setTimeout(() => deadline.abort(Object.assign(new Error('request timed out'), {name: 'TimeoutError'})), prepared.timeout_ms);
      const signals = [deadline.signal];
      if (req.signal) signals.push(req.signal);
      this.sentCount++; this.inFlight++; this.maxInFlight = Math.max(this.maxInFlight, this.inFlight);
      let outcome;
      try {
        // redirect:'error' - the credential header must never follow a redirect to another host
        const response = await this.fetchImpl(prepared.http.url, {method: 'POST', headers: prepared.http.headers, body: JSON.stringify(prepared.http.body), signal: AbortSignal.any(signals), redirect: 'error'});
        if (!response.ok) outcome = {status: response.status, retry_after: response.headers?.get?.('retry-after')};
        else {
          const text = await response.text();
          outcome = text.length > prepared.max_response_units ? {status: response.status, oversize: true} : {status: response.status, body: text};
        }
      } catch (e) {
        outcome = {transport_error: {name: e && e.name ? String(e.name) : '', code: String((e && (e.code || (e.cause && e.cause.code))) || ''), user_aborted: !!(req.signal && req.signal.aborted)}};
      } finally { this.inFlight--; clearTimeout(timer); }
      const done = await this.engine.rpc({op: 'ai.complete', request: prepared.request, ...outcome}, EDIT_TIMEOUT_MS);
      record('OK', {provider_id: req.providerId});
      return {text: done.text, usage: done.usage, estimated: done.estimated, effort: done.effort, elapsedMs: done.elapsed_ms};
    } catch (e) {
      record(e.code || 'INTERNAL', {provider_id: req.providerId, error_detail: e.detail});
      throw e;
    }
  }
}

// RealtimeScheduler whose decisions (is this input still the newest, may the request start, is the result stale, how long
// to debounce) come from the core. The bridge keeps what only it can do: the timer, the abort signal and the chain that
// lets one request run at a time. Replies are applied in the order of the requests (one pipe), and while the core is
// unavailable nothing is scheduled (the AI path is optional, typing is never affected).
class RustScheduler {
  constructor({engine, debounceMs, timers = {setTimeout, clearTimeout}, run, onResult = () => {}, onDiscard = () => {}, isAllowed = () => true}) {
    Object.assign(this, {engine, timers, run, onResult, onDiscard, isAllowed});
    this.debounceMs = 350; this.timer = null; this.controller = null; this.chain = Promise.resolve();
    this.stats = {scheduled: 0, started: 0, delivered: 0, discarded: 0, inFlight: 0, maxInFlight: 0};
    this.wanted = debounceMs; this.configuredFor = null;
    this._configure();
  }
  _ask(body) { return this.engine.rpc(body).then(r => { if (r.stats) this.stats = r.stats; return r; }, () => null); }
  // The window lives in the core's session: it is (re)sent whenever the setting changes or the core has a new session
  // (the scheduler is created before the core has started).
  setDebounce(ms) { this.wanted = ms; this.configuredFor = null; this._configure(); }
  _configure() {
    if (!this.engine.available() || this.configuredFor === this.engine.session) return;
    this.configuredFor = this.engine.session;
    this._ask({op: 'sched.configure', debounce_ms: this.wanted}).then(r => { if (r) this.debounceMs = r.debounce_ms; });
  }
  _stop() {
    if (this.timer) { this.timers.clearTimeout(this.timer); this.timer = null; }
    if (this.controller) this.controller.abort();
  }
  // Local state is invalidated at once; the network abort is best effort (already-sent data cannot be recalled).
  schedule(key, payload) {
    const allowed = !!this.isAllowed(); // a synchronous gate, asked now like the reference does
    this._configure();
    this._ask({op: 'sched.schedule', key, allowed}).then(r => {
      if (!r) return;
      if (r.effects.clear_timer && this.timer) { this.timers.clearTimeout(this.timer); this.timer = null; }
      if (r.effects.abort && this.controller) this.controller.abort();
      if (r.effects.arm_ms !== null) {
        this.timer = this.timers.setTimeout(() => { this.timer = null; this.chain = this.chain.then(() => this._fire(key, payload)).catch(() => {}); }, r.effects.arm_ms);
      }
    });
  }
  async _fire(key, payload) {
    const started = await this._ask({op: 'sched.fire', key, allowed: !!this.isAllowed()});
    if (!started || !started.start) return; // superseded while waiting for the previous request
    const controller = this.controller = new AbortController();
    let result, failure = null;
    try { result = await this.run(payload, controller.signal, key); } catch (e) { failure = e || new Error('failed'); }
    const verdict = await this._ask({op: 'sched.finish', key, ok: !failure, code: failure && failure.code ? failure.code : undefined});
    if (!verdict) return;
    if (verdict.action === 'deliver') this.onResult(result, key); else this.onDiscard(verdict.code, key);
  }
  cancel() { this._stop(); this._ask({op: 'sched.cancel'}); }
}

class RustEngine {
  constructor({globalState, onChange = () => {}}) {
    this.kind = 'rust'; this.core = null; this.session = null; this.state = 'starting'; this.reason = ''; this.onChange = onChange;
    this.broker = new RustBroker(this, globalState);
    this.coordinator = new RustCoordinator(this);
    this.fallbackAllowed = false; this.predictTimeoutMs = 2000; // no fallback: a slow answer means "no candidates this keystroke"
    this.budget = new RustBudgetView(this);
    this.privacy = {
      assess: async (text, {fileName, scope}) => { const r = await this.rpc({op: 'privacy.assess', text, file_name: fileName, scope}, EDIT_TIMEOUT_MS); return {action: r.action, scanned: r.scanned, summary: r.summary}; },
      redactionSet: () => ({id: null, count: 0}), // the originals stay in the core; a set only holds the id of its map
      redact: async (text, set) => { const r = await this.rpc({op: 'privacy.redact', text, redaction: set.id || undefined}, EDIT_TIMEOUT_MS); set.id = r.redaction; set.count = r.total; return r.text; },
      restore: async (text, set) => {
        if (!set.count) return {text, unresolved: 0};
        const r = await this.rpc({op: 'privacy.restore', text, redaction: set.id}, EDIT_TIMEOUT_MS);
        return {text: r.text, unresolved: r.unresolved};
      },
      release: async set => { const id = set.id; set.id = null; set.count = 0; if (id && this.available()) await this.rpc({op: 'privacy.release', redaction: id}).catch(() => {}); }
    };
    this.annotate = {
      supports: async (profile, dialect) => (await this.rpc({op: 'annotate.supports', language: profile, dialect})).supported,
      prompt: async a => {
        const r = await this.rpc({op: 'annotate.prompt', language: a.language, dialect: a.dialect, locale: a.locale, comment: !!a.comment, graph: !!a.graph, file_name: a.fileName, file_text: a.fileText, snippet: a.snippet, line: a.line, current_symbols: a.currentSymbols, directory_symbols: a.directorySymbols}, EDIT_TIMEOUT_MS);
        return {instructions: r.instructions, input: r.input};
      },
      f1Addendum: async a => (await this.rpc({op: 'annotate.f1_addendum', task: a.task, comment: !!a.comment, graph: !!a.graph, locale: a.locale, language: a.language, dialect: a.dialect})).text,
      apply: async (raw, a) => {
        const r = await this.rpc({op: 'annotate.apply', raw, profile: a.profile, original: a.original, dialect: a.dialect, comment: !!a.comment, graph: !!a.graph}, EDIT_TIMEOUT_MS);
        return r.commented === undefined ? {text: r.text, notes: r.notes} : {text: r.text, notes: r.notes, commented: r.commented, graph: r.graph};
      }
    };
    this.symbols = {
      extract: async (profile, text, max) => (await this.rpc({op: 'symbols.extract', language: profile, text, max}, EDIT_TIMEOUT_MS)).symbols,
      dirCandidates: async (currentName, entries) => (await this.rpc({op: 'symbols.dir_candidates', current_name: currentName, entries: entries.map(e => ({name: e.name, is_file: !!e.isFile}))})).candidates
    };
  }
  createAi({diagnostics, isEnabled, isTrusted, workspace}) { return new RustAiClient({engine: this, diagnostics, isEnabled, isTrusted, workspace}); }
  async buildPrompt(task, payload) { const r = await this.rpc({op: 'ai.prompt', task, payload}, EDIT_TIMEOUT_MS); return {instructions: r.instructions, input: r.input}; }
  async parseModelResult(raw, {task = 'full', baseline = '', currentFile = ''} = {}) { return (await this.rpc({op: 'ai.parse', raw, task, baseline, current_file: currentFile}, EDIT_TIMEOUT_MS)).result; }
  createScheduler(options) { return new RustScheduler({engine: this, ...options}); }
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
    // the effective budget limits go to the core's budget (0 = unlimited), in the same order on the pipe
    this.core.request({op: 'settings.apply', session: this.session, values, locale}, {timeoutMs: START_TIMEOUT_MS}).then(r => {
      const b = r.ok && r.settings && r.settings.budget;
      if (b && this.available()) return this.core.request({op: 'budget.configure', session: this.session, max_requests: b.maxRequests, max_tokens: b.maxTokens, price_per_1k_tokens: b.pricePer1kTokens}, {timeoutMs: START_TIMEOUT_MS});
    }).catch(() => {});
  }
  // One request to the core's session. A failure becomes a PimeError with the core's error code; a dead core switches
  // the IME features off (reported through onChange), a slow one only fails this request.
  async rpc(body, timeoutMs = START_TIMEOUT_MS) {
    if (!this.available()) throw fail('IPC_TIMEOUT', 'the Programming IME core is not available');
    let r;
    try { r = await this.core.request({...body, session: this.session}, {timeoutMs}); }
    catch { if (this.core && this.core.dead) this.noteFailure(); throw fail('IPC_TIMEOUT', 'the Programming IME core did not answer in time'); }
    if (r.budget) this.budget._sync(r.budget); // every AI reply, failures included, carries the budget after it
    if (!r.ok) {
      if (r.error === 'CORE_UNAVAILABLE' || r.error === 'NO_SESSION') this.noteFailure(r.error === 'NO_SESSION' ? 'the core lost the session' : 'the core stopped');
      // an AI failure also carries the metadata of the reference error
      const err = fail(r.error || 'INTERNAL', r.message || r.error, {retryable: r.retryable === true, userAction: r.user_action || ''});
      if (r.retry_after_sec !== undefined) err.retryAfterSec = r.retry_after_sec;
      if (r.detail !== undefined) err.detail = r.detail;
      throw err;
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
