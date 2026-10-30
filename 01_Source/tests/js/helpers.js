'use strict';
// Test doubles (DD-12 "testkit"): fake provider/fetch, virtual clock, fake editor document, fault-injecting fs.
const path = require('node:path');
const core = name => require(path.join(__dirname, '..', '..', 'editor-bridges', 'vscode', 'src', 'core', name));

// Fake provider: records every call; handler decides the answer.
function makeFetch(handler) {
  const calls = [];
  const f = async (url, init) => {
    const call = {url, init, body: JSON.parse(init.body), headers: init.headers};
    calls.push(call);
    return handler(call, calls.length);
  };
  f.calls = calls;
  return f;
}
const headers = map => ({get: k => map[String(k).toLowerCase()] ?? null});
const okJson = (obj, h = {}) => ({ok: true, status: 200, headers: headers(h), text: async () => JSON.stringify(obj)});
const httpError = (status, h = {}) => ({ok: false, status, headers: headers(h), text: async () => ''});
const responsesBody = (text, usage = {input_tokens: 10, output_tokens: 5}) => ({output_text: text, usage});
const chatBody = (text, usage = {prompt_tokens: 10, completion_tokens: 5}) => ({choices: [{message: {content: text}}], usage});

// Virtual clock: timers fire only when advance() moves time forward.
class FakeTimers {
  constructor() { this.now = 0; this.q = []; this.id = 1; }
  setTimeout(fn, ms) { const t = {id: this.id++, at: this.now + ms, fn}; this.q.push(t); return t.id; }
  clearTimeout(id) { this.q = this.q.filter(t => t.id !== id); }
  async advance(ms) {
    const target = this.now + ms;
    for (;;) {
      const next = this.q.filter(t => t.at <= target).sort((a, b) => a.at - b.at || a.id - b.id)[0];
      if (!next) break;
      this.q = this.q.filter(t => t !== next);
      this.now = next.at; next.fn();
      await tick();
    }
    this.now = target;
    await tick();
  }
}
const tick = () => new Promise(r => setImmediate(r));
const sleep = ms => new Promise(r => setTimeout(r, ms));

// Fake editor buffer with version counter, undo stack and failure modes for apply.
class FakeDoc {
  constructor(uri, text, version = 42) { this.uri = uri; this.text = text; this.version = version; this.history = []; this.applyCalls = 0; this.mode = 'ok'; this.closed = false; this.selection = null; }
  userEdit(text) { this.history.push(this.text); this.text = text; this.version++; }
  undo() { if (this.history.length) { this.text = this.history.pop(); this.version++; } }
  state() { return {uri: this.uri, version: this.version, text: this.text, isClosed: this.closed, editable: true, selection: this.selection}; }
  adapter() {
    return {
      getState: () => this.state(),
      applyEdit: async edits => {
        this.applyCalls++;
        if (this.mode === 'throw') throw new Error('pipe closed');
        if (this.mode === 'hang') return new Promise(() => {});
        if (this.mode === 'reject') return false;
        const {applyEdits} = core('textrange');
        if (this.mode === 'wrong-text') { this.userEdit(applyEdits(this.text, edits) + '\n// extra'); return true; }
        this.userEdit(applyEdits(this.text, edits));
        return true;
      }
    };
  }
}
// fs double: fail on a chosen operation, otherwise delegate to a memory map.
function makeFs(initial = {}, failOn = null) {
  const files = new Map(Object.entries(initial));
  const err = op => Object.assign(new Error(`${op} failed`), {code: 'ENOSPC'});
  return {
    files,
    writeFileSync: (p, t) => { if (failOn === 'write') throw err('write'); files.set(p, String(t)); },
    existsSync: p => files.has(p),
    copyFileSync: (a, b) => { files.set(b, files.get(a)); },
    renameSync: (a, b) => { if (failOn === 'rename') throw err('rename'); files.set(b, files.get(a)); files.delete(a); },
    rmSync: p => { files.delete(p); },
    readFileSync: p => { if (!files.has(p)) throw Object.assign(new Error('ENOENT'), {code: 'ENOENT'}); return files.get(p); }
  };
}
const crypto = require('node:crypto');
const sha = t => crypto.createHash('sha256').update(t, 'utf8').digest('hex');

module.exports = {core, makeFetch, okJson, httpError, responsesBody, chatBody, FakeTimers, FakeDoc, makeFs, tick, sleep, sha};
