'use strict';
// Optional locally built Rust engine over JSON Lines (stdin/stdout). Only an explicitly configured path is run.
// Each request carries an id; a slow or dead core never blocks input - callers fall back to the JS engine.
const {spawn} = require('node:child_process');

const MAX_BUFFER = 1048576;
class CoreProcess {
  constructor(binary, {timeoutMs = 100} = {}) {
    this.pending = new Map(); this.buffer = ''; this.dead = false; this.nextId = 1; this.timeoutMs = timeoutMs;
    this.child = spawn(binary, [], {stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true});
    this.child.stdout.on('data', bytes => {
      this.buffer += bytes.toString('utf8');
      if (this.buffer.length > MAX_BUFFER) { this.stop(); return; }
      let i;
      while ((i = this.buffer.indexOf('\n')) >= 0) {
        const line = this.buffer.slice(0, i); this.buffer = this.buffer.slice(i + 1);
        let msg; try { msg = JSON.parse(line); } catch { continue; }
        const p = this.pending.get(msg.id);
        if (p) { this.pending.delete(msg.id); p.resolve(msg); }
      }
    });
    this.child.on('error', () => this.stop());
    this.child.on('exit', () => this.stop());
    this.child.stderr.resume(); // diagnostics output of an untrusted binary is discarded, never logged
  }
  request(payload) {
    if (this.dead) return Promise.reject(new Error('CORE_UNAVAILABLE'));
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error('CORE_TIMEOUT')); }, this.timeoutMs);
      this.pending.set(id, {resolve: v => { clearTimeout(timer); resolve(v); }});
      try { this.child.stdin.write(JSON.stringify({...payload, id}) + '\n'); }
      catch (e) { clearTimeout(timer); this.pending.delete(id); reject(e); }
    });
  }
  stop() {
    if (this.dead) return;
    this.dead = true;
    for (const p of this.pending.values()) p.resolve({ok: false, error: 'CORE_UNAVAILABLE'});
    this.pending.clear();
    try { this.child.kill(); } catch { /* already gone */ }
  }
}
module.exports = {CoreProcess};
