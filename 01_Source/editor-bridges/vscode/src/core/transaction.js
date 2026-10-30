'use strict';
// Edit plans and the Prepare/Apply transaction (FR-035/036/037/040, NFR-502, DD §9.4).
// Rules enforced here: single current file; explicit confirmation; baseline version+hash re-check; one-use token
// marked used BEFORE the write; read-back verification; EDIT_UNKNOWN is final (no automatic retry).
const crypto = require('node:crypto');
const {fail} = require('./errors');
const {validateOffsets, applyEdits} = require('./textrange');
const {sha256, assertFullFileWritable} = require('./context');

const sameSelection = (a, b) => !!a && !!b && a.start.line === b.start.line && a.start.character === b.start.character && a.end.line === b.end.line && a.end.character === b.end.character;

class EditCoordinator {
  constructor({now = Date.now, planTtlMs = 10 * 60 * 1000, tokenTtlMs = 60 * 1000, applyTimeoutMs = 5000, random = () => crypto.randomBytes(16).toString('hex')} = {}) {
    Object.assign(this, {now, planTtlMs, tokenTtlMs, applyTimeoutMs, random});
    this.plans = new Map(); this.tokens = new Map(); this.queue = Promise.resolve();
    this.stats = {plans: 0, prepared: 0, applyCalls: 0, writes: 0};
  }
  // snapshot: from context.makeSnapshot; edits: UTF-16 offsets into snapshot.content; targets: every document the proposal touches.
  createPlan({kind, snapshot, edits, targets = [], selection}) {
    if (kind !== 'full' && kind !== 'insert') throw fail('PATCH_INVALID', 'unknown plan kind');
    if (targets.some(t => t !== snapshot.document_uri)) throw fail('CROSS_FILE_DENIED', 'v1 edits only the current file', {userAction: 'Only changes to the open file can be applied'});
    if (kind === 'full') assertFullFileWritable(snapshot);
    if (!Array.isArray(edits) || edits.length === 0) throw fail('PATCH_INVALID', 'plan has no edits');
    const sorted = validateOffsets(snapshot.content, edits);
    const proposedText = applyEdits(snapshot.content, sorted);
    const plan = {
      id: this.random(), kind, uri: snapshot.document_uri, editorInstance: snapshot.editor_instance_id,
      baseline: {version: snapshot.document_version, hash: snapshot.sha256}, edits: sorted, selection: kind === 'insert' ? (selection || snapshot.selection) : null,
      proposedHash: sha256(proposedText), expiresAt: this.now() + this.planTtlMs, status: 'REVIEW_READY'
    };
    this.plans.set(plan.id, plan); this.stats.plans++;
    return plan;
  }
  // The document changed, closed or lost focus: nothing prepared from the old baseline may be applied.
  invalidate(uri) {
    let n = 0;
    for (const p of this.plans.values()) if (p.uri === uri && (p.status === 'REVIEW_READY' || p.status === 'PREPARED')) { p.status = 'STALE'; n++; }
    return n;
  }
  _check(plan, state) {
    if (plan.status === 'STALE') throw fail('STALE_DOCUMENT', 'the document changed after the preview', {userAction: 'Generate the proposal again'});
    if (plan.expiresAt <= this.now()) throw fail('STALE_RESULT', 'proposal expired');
    if (!state || state.isClosed) throw fail('DOCUMENT_CLOSED', 'document is closed');
    if (state.uri !== plan.uri || state.version !== plan.baseline.version || sha256(state.text) !== plan.baseline.hash) {
      plan.status = 'STALE';
      throw fail('STALE_DOCUMENT', 'the document version or content differs from the reviewed baseline', {userAction: 'Review the new Diff and confirm again'});
    }
    if (state.editable === false) throw fail('EDIT_REJECTED', 'document is read-only');
    if (plan.kind === 'insert' && !sameSelection(plan.selection, state.selection)) { plan.status = 'STALE'; throw fail('STALE_SELECTION', 'the cursor or selection moved since the proposal was made'); }
  }
  // confirmed must come from a deliberate user action; grantOk from the permission broker; validation from the validators.
  prepare(planId, {state, confirmed, grantOk, validationOk = true}) {
    const plan = this.plans.get(planId);
    if (!plan) throw fail('TOKEN_INVALID', 'unknown plan');
    if (confirmed !== true) throw fail('USER_CONFIRMATION_REQUIRED', 'explicit confirmation is required before anything is written');
    if (grantOk !== true) throw fail('GRANT_DENIED', 'authorisation is missing or revoked');
    this._check(plan, state);
    if (validationOk === false) throw fail('VALIDATION_FAILED', 'syntax check failed; default apply is disabled');
    const token = {id: this.random(), planId, used: false, expiresAt: this.now() + this.tokenTtlMs};
    this.tokens.set(token.id, token); plan.status = 'PREPARED'; this.stats.prepared++;
    return token.id;
  }
  // adapter: {getState(), applyEdit(edits) -> Promise<boolean>}. Serialised so our own applies never interleave.
  apply(tokenId, adapter) {
    const run = this.queue.then(() => this._apply(tokenId, adapter));
    this.queue = run.catch(() => {});
    return run;
  }
  async _apply(tokenId, adapter) {
    this.stats.applyCalls++;
    const token = this.tokens.get(tokenId);
    if (!token || token.used || token.expiresAt <= this.now()) throw fail('TOKEN_INVALID', 'apply token is unknown, already used or expired');
    token.used = true; // consumed before the first write: a replay can never write twice
    const plan = this.plans.get(token.planId);
    this._check(plan, adapter.getState());
    let accepted, timer;
    this.stats.writes++;
    const deadline = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('APPLY_TIMEOUT')), this.applyTimeoutMs); });
    try {
      accepted = await Promise.race([Promise.resolve(adapter.applyEdit(plan.edits)), deadline]);
    } catch {
      plan.status = 'UNKNOWN';
      throw fail('EDIT_UNKNOWN', 'the editor did not confirm the edit; check the document manually', {severity: 'error', userAction: 'Inspect the file; the edit is not retried automatically'});
    } finally { clearTimeout(timer); }
    const after = adapter.getState();
    if (accepted === false) {
      if (after && sha256(after.text) === plan.baseline.hash) { plan.status = 'STALE'; throw fail('EDIT_REJECTED', 'the editor rejected the edit; nothing was written'); }
      plan.status = 'UNKNOWN'; throw fail('EDIT_UNKNOWN', 'the editor rejected the edit but the document changed');
    }
    if (!after || sha256(after.text) !== plan.proposedHash) { plan.status = 'UNKNOWN'; throw fail('EDIT_UNKNOWN', 'read-back does not match the proposed text; the edit is not retried', {userAction: 'Inspect the file and use Undo if needed'}); }
    plan.status = 'APPLIED';
    return {state: 'APPLIED', newVersion: after.version, newHash: plan.proposedHash};
  }
  discard(planId) { this.plans.delete(planId); }
  statusOf(planId) { return this.plans.get(planId)?.status || 'UNKNOWN_PLAN'; }
  purge() { // drop expired plans and tokens (source fragments must not linger, DD §11)
    const t = this.now();
    for (const [id, p] of this.plans) if (p.expiresAt <= t || p.status === 'APPLIED') this.plans.delete(id);
    for (const [id, k] of this.tokens) if (k.expiresAt <= t || k.used) this.tokens.delete(id);
  }
}
module.exports = {EditCoordinator, sameSelection};
