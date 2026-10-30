'use strict';
// Locally built Rust engine over JSON Lines (stdin/stdout). Only an explicitly configured path is run.
// Each request carries an id; a slow or dead core never blocks input. With the JS engine callers fall back to the
// JS implementation; with the Rust engine there is no fallback and the IME features are disabled instead.
const {spawn} = require('node:child_process');
const {StringDecoder} = require('node:string_decoder');

const MAX_BUFFER = 1048576;
class CoreProcess {
  // maxBuffer: the longest response line accepted (document-sized replies of the Rust engine need more than 1 MiB)
  constructor(binary, {timeoutMs = 100, maxBuffer = MAX_BUFFER, onStop = null} = {}) {
    this.pending = new Map(); this.buffer = ''; this.dead = false; this.nextId = 1; this.timeoutMs = timeoutMs; this.maxBuffer = maxBuffer; this.onStop = onStop;
    this.decoder = new StringDecoder('utf8'); // a multi-byte character can be split between two chunks
    this.child = spawn(binary, [], {stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true});
    this.child.stdout.on('data', bytes => {
      this.buffer += this.decoder.write(bytes);
      let i;
      while ((i = this.buffer.indexOf('\n')) >= 0) {
        const line = this.buffer.slice(0, i); this.buffer = this.buffer.slice(i + 1);
        let msg; try { msg = JSON.parse(line); } catch { continue; }
        const p = this.pending.get(msg.id);
        if (p) { this.pending.delete(msg.id); p.resolve(msg); }
      }
      if (this.buffer.length > this.maxBuffer) this.stop();
    });
    this.child.stdin.on('error', () => this.stop()); // EPIPE after the core died
    this.child.on('error', () => this.stop());
    this.child.on('exit', () => this.stop());
    this.child.stderr.resume(); // diagnostics output of an untrusted binary is discarded, never logged
  }
  // options.timeoutMs overrides the default for this one request (edit operations on large documents need longer)
  request(payload, options = {}) {
    if (this.dead) return Promise.reject(new Error('CORE_UNAVAILABLE'));
    const timeoutMs = options.timeoutMs || this.timeoutMs;
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error('CORE_TIMEOUT')); }, timeoutMs);
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
    if (this.onStop) { try { this.onStop(); } catch { /* the observer must not break shutdown */ } }
  }
}
module.exports = {CoreProcess};
