'use strict';
// Privacy-preserving diagnostics (FR-050, NFR-301, DD §12.2). Only whitelisted metadata is recorded; every value
// must be a short token-like string or a number, so source code, prompts, paths and keys cannot slip in.
const crypto = require('node:crypto');

const ALLOWED = Object.freeze(['timestamp', 'component', 'error_code', 'error_detail', 'elapsed_ms', 'request_type', 'anonymized_session', 'provider_id', 'counters']);
const TOKEN = /^[A-Za-z0-9_.:-]{0,64}$/;
function sanitize(event, session) {
  const out = {};
  for (const k of ALLOWED) {
    if (!(k in event) && k !== 'anonymized_session' && k !== 'timestamp') continue;
    let v = event[k];
    if (k === 'anonymized_session') v = session;
    else if (k === 'timestamp') v = event.timestamp ?? Date.now();
    else if (k === 'counters') {
      v = Object.fromEntries(Object.entries(v || {}).filter(([ck, cv]) => TOKEN.test(ck) && Number.isFinite(cv)));
    } else if (typeof v === 'number') v = Number.isFinite(v) ? Math.round(v) : 0;
    else v = typeof v === 'string' && TOKEN.test(v) ? v : '[filtered]';
    out[k] = v;
  }
  return out;
}
class Diagnostics {
  constructor({now = Date.now, sessionSeed = crypto.randomBytes(8).toString('hex'), max = 500, sink = null} = {}) {
    this.now = now; this.max = max; this.sink = sink; this.events = [];
    this.session = crypto.createHash('sha256').update(sessionSeed).digest('hex').slice(0, 12); // anonymised, not reversible
  }
  record(event) {
    const clean = sanitize({timestamp: this.now(), ...event}, this.session);
    this.events.push(clean);
    if (this.events.length > this.max) this.events.shift();
    if (this.sink) this.sink(JSON.stringify(clean));
    return clean;
  }
  summary() {
    const byCode = {}, lastByComponent = {};
    for (const e of this.events) {
      const c = e.error_code || 'OK';
      byCode[c] = (byCode[c] || 0) + 1;
      lastByComponent[e.component || 'unknown'] = {error_code: c, elapsed_ms: e.elapsed_ms ?? null};
    }
    return {total: this.events.length, byCode, lastByComponent};
  }
  toText() { return this.events.map(e => JSON.stringify(e)).join('\n'); }
}
module.exports = {Diagnostics, sanitize, ALLOWED};
