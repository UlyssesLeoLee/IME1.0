'use strict';
// Permission broker (FR-016, DD §15): grants are bound to (provider, workspace, scope, expiry) and are revocable.
// Per-request grants are consumed on use. Nothing here stores file contents or keys.
const crypto = require('node:crypto');
const {fail} = require('./errors');

const SCOPES = Object.freeze(['L1', 'L2', 'L3', 'L4']);
const STORE_KEY = 'pime.grants.v1';
const workspaceToken = uris => crypto.createHash('sha256').update([...uris].sort().join('\n')).digest('hex').slice(0, 24);

class PermissionBroker {
  constructor({now = Date.now, store = null} = {}) {
    this.now = now; this.store = store; this.grants = new Map();
    const saved = store && store.get(STORE_KEY);
    if (Array.isArray(saved)) for (const g of saved) if (g && typeof g.id === 'string' && SCOPES.includes(g.scope) && !g.once) this.grants.set(g.id, g);
  }
  _save() {
    if (this.store) this.store.update(STORE_KEY, [...this.grants.values()].filter(g => !g.once && !g.revoked));
  }
  authorize({providerId, workspace, scope, ttlMs = 24 * 3600 * 1000, once = false}) {
    if (!providerId || !workspace || !SCOPES.includes(scope)) throw fail('GRANT_DENIED', 'invalid grant request');
    const grant = {id: crypto.randomBytes(12).toString('hex'), providerId, workspace, scope, once, used: false, revoked: false, createdAt: this.now(), expiresAt: this.now() + ttlMs};
    this.grants.set(grant.id, grant);
    this._save();
    return grant;
  }
  // Any mismatch is a denial; the reason never contains workspace paths or provider secrets.
  check(grantId, {providerId, workspace, scope}) {
    const g = this.grants.get(grantId);
    if (!g) return {ok: false, reason: 'unknown-grant'};
    if (g.revoked) return {ok: false, reason: 'revoked'};
    if (g.expiresAt <= this.now()) return {ok: false, reason: 'expired'};
    if (g.once && g.used) return {ok: false, reason: 'already-used'};
    if (g.providerId !== providerId) return {ok: false, reason: 'provider-mismatch'};
    if (g.workspace !== workspace) return {ok: false, reason: 'workspace-mismatch'};
    if (g.scope !== scope) return {ok: false, reason: 'scope-mismatch'};
    return {ok: true, reason: 'ok'};
  }
  // Finds a reusable (workspace-wide) grant for the criteria.
  find({providerId, workspace, scope}) {
    for (const g of this.grants.values()) if (!g.once && this.check(g.id, {providerId, workspace, scope}).ok) return g;
    return null;
  }
  require(grantId, criteria) {
    const r = this.check(grantId, criteria);
    if (!r.ok) throw fail('GRANT_DENIED', `authorisation denied (${r.reason})`, {userAction: 'Review and re-authorise the data transfer'});
    return this.grants.get(grantId);
  }
  // Still honoured at Apply time: a one-use grant is already "used" by then, but a revoked/expired one is not.
  stillValid(grantId) {
    const g = this.grants.get(grantId);
    return !!g && !g.revoked && g.expiresAt > this.now();
  }
  consume(grantId) {
    const g = this.grants.get(grantId);
    if (g && g.once) { g.used = true; }
  }
  revoke(grantId) { const g = this.grants.get(grantId); if (g) { g.revoked = true; this._save(); } return !!g; }
  revokeProvider(providerId) { let n = 0; for (const g of this.grants.values()) if (g.providerId === providerId && !g.revoked) { g.revoked = true; n++; } this._save(); return n; }
  revokeAll() { let n = 0; for (const g of this.grants.values()) if (!g.revoked) { g.revoked = true; n++; } this._save(); return n; }
  list() { return [...this.grants.values()].filter(g => !g.revoked && g.expiresAt > this.now() && !(g.once && g.used)).map(({id, providerId, scope, once, expiresAt}) => ({id, providerId, scope, once, expiresAt})); }
}
module.exports = {PermissionBroker, workspaceToken, SCOPES};
