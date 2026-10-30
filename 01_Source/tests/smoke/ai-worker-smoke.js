'use strict';
// Smoke test of the real pime-ai-worker executable (plan phase P2): HTTPS through WinHTTP against the UAT mock provider,
// key injection from Windows Credential Manager, TLS rejection, 429/500 pass-through, timeout, cancel, key scrubbing.
//   cargo build --manifest-path crates/pime-ai-worker/Cargo.toml
//   PIME_WORKER_EXE=<path to pime-ai-worker.exe> node tests/smoke/ai-worker-smoke.js
//
// Side effects (the only ones): loopback HTTPS/HTTP servers, and Credential Manager entries whose target starts with
// PIME-TEST-<random> (session persistence), created, read and deleted here and removed again on every exit path. The
// test fails if one is left over. Nothing else on the machine is touched and nothing leaves the loopback interface.
const {spawn, spawnSync} = require('node:child_process');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const http = require('node:http');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');
const {startMock} = require('../uat/helpers/mock-provider.js');
const {parsePe} = require('../../scripts/pe-info.js');
// system DLLs present on every Windows 10/11 x64 installation (the list of tests/smoke/tsf-dll-check.js plus the two this worker is built for)
const ALLOWED_IMPORT = /^(kernel32|ntdll|ole32|oleaut32|msvcrt|propsys|userenv|ws2_32|bcryptprimitives|user32|gdi32|advapi32|msctf|winhttp|crypt32|api-ms-win-[a-z0-9-]+)\.dll$/;

const exe = process.env.PIME_WORKER_EXE || path.join(process.env.CARGO_TARGET_DIR || path.join(os.tmpdir(), 'pime-cargo-target'), 'debug', 'pime-ai-worker.exe');
if (!fs.existsSync(exe)) { console.error(`pime-ai-worker executable not found: ${exe} (build it with cargo build --manifest-path crates/pime-ai-worker/Cargo.toml, or set PIME_WORKER_EXE)`); process.exit(2); }

const RUN = crypto.randomBytes(4).toString('hex');
const NAMESPACE = `PIME-TEST-${RUN}`;
const KEY_A = `mock-key-${RUN}-aaaa1111`, KEY_B = `mock-key-${RUN}-bbbb2222`, KEY_C = `mock-key-${RUN}-cccc3333`;
const ALL_KEYS = [KEY_A, KEY_B, KEY_C];
const PROVIDERS = ['openai', 'anthropic', 'other', 'plainhttp'];
const sleep = ms => new Promise(r => setTimeout(r, ms));
const TEST_ENV = {PIME_WORKER_TEST_MODE: '1'};

const workers = [];
// Everything any worker ever wrote, for the final "the key appears nowhere" check.
const transcript = {stdout: [], stderr: []};

class Worker {
  constructor(args = [], env = TEST_ENV) {
    // the test-mode variable only ever comes from `env`, never from the environment this script runs in
    const base = {...process.env}; delete base.PIME_WORKER_TEST_MODE;
    this.child = spawn(exe, args, {stdio: ['pipe', 'pipe', 'pipe'], env: {...base, ...env}, windowsHide: true});
    this.args = args; this.lines = []; this.stderr = ''; this.buf = ''; this.pending = new Map(); this.nextId = 1;
    this.child.stdout.setEncoding('utf8'); this.child.stderr.setEncoding('utf8');
    this.child.stdout.on('data', d => this.onData(d));
    this.child.stderr.on('data', d => { this.stderr += d; transcript.stderr.push(d); });
    this.exited = new Promise(resolve => this.child.on('exit', (code, signal) => resolve({code, signal})));
    this.child.stdin.on('error', () => { /* the worker may already be gone */ });
    workers.push(this);
  }
  onData(chunk) {
    this.buf += chunk;
    for (let i = this.buf.indexOf('\n'); i >= 0; i = this.buf.indexOf('\n')) {
      const line = this.buf.slice(0, i); this.buf = this.buf.slice(i + 1);
      this.lines.push(line); transcript.stdout.push(line);
      let msg = null; try { msg = JSON.parse(line); } catch { /* reported by the JSON check at the end */ }
      if (msg && msg.id !== undefined && this.pending.has(msg.id)) { this.pending.get(msg.id).resolve(msg); this.pending.delete(msg.id); }
    }
  }
  id() { return this.nextId++; }
  // Sends a request and resolves with the reply carrying the same id.
  call(req, timeoutMs = 20000) {
    const id = req.id ?? this.id();
    const full = {...req, id};
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`no reply for ${full.op} id ${id} within ${timeoutMs} ms`)); }, timeoutMs);
      timer.unref(); // a request whose worker was shut down on purpose must not keep this script alive
      this.pending.set(id, {resolve: m => { clearTimeout(timer); resolve(m); }});
      this.child.stdin.write(JSON.stringify(full) + '\n');
    });
  }
  sendRaw(text) { this.child.stdin.write(text + '\n'); }
  async end() {
    this.child.stdin.end();
    const result = await Promise.race([this.exited, sleep(8000).then(() => null)]);
    if (!result) { this.child.kill(); throw new Error('the worker did not exit after its input ended'); }
    return result;
  }
  kill() { try { this.child.kill(); } catch { /* already gone */ } }
}

const send = (worker, id, url, extra = {}) => worker.call({op: 'http.send', id, method: 'POST', url, ...extra});
const bearer = ref => ({credentialRef: ref, authHeader: 'Authorization: Bearer {key}'});
const jsonHeaders = {'Content-Type': 'application/json'};
const cmdkeyList = () => {
  const r = spawnSync('cmdkey', [`/list:${NAMESPACE}*`], {encoding: 'utf8', windowsHide: true});
  return `${r.stdout || ''}${r.stderr || ''}`;
};
// cmdkey echoes its filter in a localized header line, so only `<namespace>/` (which that header never contains) means "an entry exists"
const leftovers = () => cmdkeyList().includes(`${NAMESPACE}/`);
const cleanupCredentials = () => {
  for (const p of PROVIDERS) spawnSync('cmdkey', [`/delete:${NAMESPACE}/${p}`], {encoding: 'utf8', windowsHide: true});
};

let step = 0;
async function check(name, fn) {
  const t0 = Date.now();
  try { await fn(); } catch (e) { console.error(`FAIL  ${name}\n${e.stack || e}`); throw e; }
  step++; console.log(`ok ${String(step).padStart(2)}  ${name} (${Date.now() - t0} ms)`);
}

(async () => {
  const mock = await startMock('ai-worker-smoke', async ({record}) => {
    const c = new URL(record.url, 'https://mock.invalid').searchParams.get('case');
    switch (c) {
      case 'hang': return {hang: true};
      case 'slow': return {delayMs: 700, text: 'slow'};
      case 'echo': return {text: `echo:${record.headers.authorization || record.headers['x-api-key'] || 'no-credential-header'}`};
      case 'e429': return {status: 429, headers: {'retry-after': '7', 'set-cookie': 'session=abc', 'x-ratelimit-remaining-requests': '0'}};
      case 'e500': return {status: 500};
      case 'redirect': return {status: 302, headers: {location: 'https://127.0.0.1:9/stolen'}};
      case 'huge': return {text: 'x'.repeat(5 * 1024 * 1024)};
      default: return {text: 'hello from the mock'};
    }
  });
  const origin = `127.0.0.1:${mock.port}`;
  const url = (family, query = '') => `${mock.endpoint(family)}${query ? `?case=${query}` : ''}`;
  const insecureArgs = ['--insecure-test-ca', '--test-credential-namespace', NAMESPACE];

  try {
    await check('the executable is x64 and imports only Windows system DLLs (WinHTTP, crypt32, advapi32 ...), no MinGW runtime', async () => {
      const info = parsePe(fs.readFileSync(exe));
      assert.equal(info.machineCode, 0x8664); assert.equal(info.isDll, false);
      const bad = info.imports.filter(n => !ALLOWED_IMPORT.test(n));
      assert.deepEqual(bad, [], `unexpected imports: ${bad.join(', ')}`);
      for (const needed of ['winhttp.dll', 'advapi32.dll']) assert.ok(info.imports.includes(needed), `${needed} must be imported`);
    });

    // ---------------------------------------------------------------------------------------------------- gating
    await check('test-only flags are refused without PIME_WORKER_TEST_MODE=1 (exit 2, fixed message, nothing on stdout)', async () => {
      for (const args of [['--insecure-test-ca'], ['--test-allow-loopback-http'], ['--test-credential-namespace', NAMESPACE]]) {
        for (const env of [{}, {PIME_WORKER_TEST_MODE: '0'}, {PIME_WORKER_TEST_MODE: 'true'}]) {
          const w = new Worker(args, env);
          const {code} = await w.exited;
          assert.equal(code, 2, `${args.join(' ')} with ${JSON.stringify(env)} must be refused`);
          assert.match(w.stderr, /refusing to start/);
          assert.equal(w.lines.length, 0, 'no protocol output');
        }
      }
      const bad = new Worker(['--test-credential-namespace', 'ProgrammingIME']);
      assert.equal((await bad.exited).code, 2, 'a test namespace must start with PIME-TEST-');
      const unknown = new Worker(['--no-such-option']);
      assert.equal((await unknown.exited).code, 2);
    });

    await check('the product start (no options) relaxes nothing', async () => {
      const w = new Worker([], {}); // not even the test-mode variable
      const h = await w.call({op: 'health'});
      assert.equal(h.ok, true); assert.equal(h.name, 'pime-ai-worker'); assert.equal(h.transport, 'winhttp'); assert.equal(h.protocol, '0.3');
      assert.deepEqual([h.testMode, h.insecureTestCa, h.allowLoopbackHttp, h.namespace], [false, false, false, 'ProgrammingIME']);
      assert.deepEqual(h.ops, ['health', 'http.send', 'http.cancel', 'cred.set', 'cred.exists', 'cred.delete']);
      // the real credential namespace is deliberately not touched by this test, not even to read it
      await w.end();
    });

    // ---------------------------------------------------------------------------- TLS: the product default rejects
    await check('TLS: without --insecure-test-ca a self-signed certificate is REJECTED and the key never leaves (test mode alone relaxes nothing)', async () => {
      const w = new Worker(['--test-credential-namespace', NAMESPACE]);
      const h = await w.call({op: 'health'});
      assert.deepEqual([h.testMode, h.insecureTestCa], [true, false]);
      assert.equal((await w.call({op: 'cred.set', credentialRef: 'openai', host: origin, secret: KEY_A})).ok, true);
      const before = mock.count();
      const r = await send(w, 100, url('openai-responses'), {...bearer('openai'), headers: jsonHeaders, body: '{}'});
      assert.equal(r.ok, false); assert.equal(r.error, 'DEPTH_ZERO_SELF_SIGNED_CERT', `unexpected reply ${JSON.stringify(r)}`);
      assert.equal(mock.count(), before, 'the handshake failed: not a single HTTP request (and so no key) reached the server');
      assert.ok(!JSON.stringify(r).includes(KEY_A));
      assert.equal((await w.call({op: 'cred.delete', credentialRef: 'openai'})).deleted, true);
      await w.end();
    });

    // --------------------------------------------------------------------------- the main worker (test CA accepted)
    const w = new Worker(insecureArgs);
    await check('health reports the test configuration', async () => {
      const h = await w.call({op: 'health'});
      assert.deepEqual([h.testMode, h.insecureTestCa, h.allowLoopbackHttp, h.namespace], [true, true, false, NAMESPACE]);
    });

    await check('IT-016 Credential Manager round trip: set, exists (origin only), listed by the OS, delete, gone', async () => {
      assert.deepEqual((await w.call({op: 'cred.exists', credentialRef: 'openai'})).exists, false);
      assert.ok(!leftovers(), 'no entry before the test');
      const set = await w.call({op: 'cred.set', credentialRef: 'openai', host: origin, secret: KEY_A});
      assert.deepEqual([set.ok, set.stored, set.host], [true, true, origin]);
      const ex = await w.call({op: 'cred.exists', credentialRef: 'openai'});
      assert.deepEqual([ex.ok, ex.exists, ex.host], [true, true, origin]);
      assert.ok(!('secret' in ex) && !JSON.stringify(ex).includes(KEY_A), 'there is no operation that returns a key');
      assert.ok(cmdkeyList().includes(`${NAMESPACE}/openai`), `the OS must list the entry (positive control): ${cmdkeyList()}`);
      const del = await w.call({op: 'cred.delete', credentialRef: 'openai'});
      assert.deepEqual([del.ok, del.deleted], [true, true]);
      assert.equal((await w.call({op: 'cred.exists', credentialRef: 'openai'})).exists, false);
      assert.equal((await w.call({op: 'cred.delete', credentialRef: 'openai'})).deleted, false);
      assert.ok(!cmdkeyList().includes(`${NAMESPACE}/openai`), 'the OS must no longer list it');
    });

    await check('credential operations refuse bad input without echoing the key', async () => {
      const bad = await w.call({op: 'cred.set', credentialRef: 'openai', host: origin, secret: 'has spaces in the pasted key'});
      assert.deepEqual([bad.ok, bad.error, bad.field], [false, 'INVALID_REQUEST', 'secret']);
      assert.ok(!JSON.stringify(bad).includes('spaces'));
      assert.equal((await w.call({op: 'cred.exists', credentialRef: 'openai'})).exists, false, 'nothing was stored');
      assert.equal((await w.call({op: 'cred.set', credentialRef: '../evil', host: origin, secret: KEY_A})).field, 'credentialRef');
      assert.equal((await w.call({op: 'cred.set', credentialRef: 'openai', host: 'https://api.openai.com', secret: KEY_A})).field, 'host');
    });

    await w.call({op: 'cred.set', credentialRef: 'openai', host: origin, secret: KEY_A});
    await w.call({op: 'cred.set', credentialRef: 'anthropic', host: origin, secret: KEY_B});
    await w.call({op: 'cred.set', credentialRef: 'other', host: 'api.example.com', secret: KEY_C});

    await check('success path: Bearer key injected at send time, the mock saw it, no worker line has it', async () => {
      mock.reset();
      const r = await send(w, 1000, url('openai-responses'), {...bearer('openai'), headers: jsonHeaders, body: JSON.stringify({model: 'm', input: 'hi'})});
      assert.equal(r.ok, true, JSON.stringify(r)); assert.equal(r.status, 200);
      assert.equal(JSON.parse(r.body).output_text, 'hello from the mock');
      assert.match(r.headers['content-type'], /application\/json/);
      assert.equal(mock.requests.length, 1);
      const seen = mock.requests[0];
      assert.equal(seen.headers.authorization, `Bearer ${KEY_A}`, 'positive control: the key reached the server');
      assert.equal(seen.method, 'POST'); assert.equal(seen.headers['content-type'], 'application/json');
      assert.deepEqual(JSON.parse(seen.rawBody), {model: 'm', input: 'hi'}, 'the body arrived intact');
      assert.ok(typeof r.elapsedMs === 'number' && r.bodyBytes > 0 && r.redactions === 0);
    });

    await check('success path: x-api-key form plus an allow-listed anthropic-version header', async () => {
      mock.reset();
      const r = await send(w, 1001, url('anthropic-messages'), {credentialRef: 'anthropic', authHeader: 'x-api-key: {key}', headers: {...jsonHeaders, 'anthropic-version': '2026-10-21'}, body: '{}'});
      assert.equal(r.ok, true, JSON.stringify(r));
      assert.equal(mock.requests[0].headers['x-api-key'], KEY_B);
      assert.equal(mock.requests[0].headers['anthropic-version'], '2026-10-21');
      assert.equal(mock.requests[0].headers.authorization, undefined, 'only the requested credential header is sent');
    });

    await check('a request without a credential reference sends no credential header at all', async () => {
      mock.reset();
      const r = await send(w, 1002, url('openai-responses'), {headers: jsonHeaders, body: '{}'});
      assert.equal(r.ok, true);
      assert.equal(mock.requests[0].headers.authorization, undefined);
      assert.equal(mock.requests[0].headers['x-api-key'], undefined);
    });

    await check('UT-036 429 and 500 pass through with status, retry-after and rate-limit headers; cookies are dropped', async () => {
      const r = await send(w, 1010, url('openai-responses', 'e429'), {...bearer('openai'), body: '{}'});
      assert.deepEqual([r.ok, r.status], [true, 429]);
      assert.equal(r.headers['retry-after'], '7'); assert.equal(r.headers['x-ratelimit-remaining-requests'], '0');
      assert.equal(r.headers['set-cookie'], undefined, 'headers are an allow-list');
      assert.match(r.body, /mock-error/);
      const s = await send(w, 1011, url('openai-responses', 'e500'), {...bearer('openai'), body: '{}'});
      assert.deepEqual([s.ok, s.status], [true, 500]);
    });

    await check('timeout: a server that never answers ends with TIMEOUT at the deadline, and the worker keeps working', async () => {
      const t0 = Date.now();
      const r = await send(w, 1020, url('openai-responses', 'hang'), {...bearer('openai'), body: '{}', timeoutMs: 600});
      const took = Date.now() - t0;
      assert.deepEqual([r.ok, r.error], [false, 'TIMEOUT'], JSON.stringify(r));
      assert.ok(took >= 500 && took < 4000, `the deadline held (${took} ms)`);
      assert.equal((await send(w, 1021, url('openai-responses'), {...bearer('openai'), body: '{}'})).status, 200);
    });

    await check('cancel mid-flight: http.cancel aborts the WinHTTP request, the send replies CANCELLED quickly, the worker keeps working', async () => {
      mock.reset();
      const pending = send(w, 1030, url('openai-responses', 'hang'), {...bearer('openai'), body: '{}', timeoutMs: 30000});
      for (let i = 0; i < 200 && mock.requests.length < 1; i++) await sleep(25);
      assert.equal(mock.requests.length, 1, 'the request reached the server and is now in flight');
      const t0 = Date.now();
      const cancel = await w.call({op: 'http.cancel', request: 1030});
      assert.deepEqual([cancel.ok, cancel.cancelled], [true, true]);
      const r = await pending;
      assert.deepEqual([r.ok, r.error], [false, 'CANCELLED'], JSON.stringify(r));
      assert.ok(Date.now() - t0 < 3000, `cancellation was prompt (${Date.now() - t0} ms)`);
      assert.equal((await w.call({op: 'http.cancel', request: 1030})).cancelled, false, 'already finished');
      assert.equal((await w.call({op: 'http.cancel', request: 424242})).cancelled, false, 'unknown id');
      assert.equal((await send(w, 1031, url('openai-responses'), {...bearer('openai'), body: '{}'})).status, 200);
    });

    await check('replies are matched by id: a fast request overtakes a slow one', async () => {
      const order = [];
      const slow = send(w, 1040, url('openai-responses', 'slow'), {...bearer('openai'), body: '{}'}).then(r => { order.push(1040); return r; });
      await sleep(100);
      const fast = send(w, 1041, url('openai-responses'), {...bearer('openai'), body: '{}'}).then(r => { order.push(1041); return r; });
      const [a, b] = await Promise.all([slow, fast]);
      assert.deepEqual([a.status, b.status], [200, 200]);
      assert.deepEqual(order, [1041, 1040]);
    });

    await check('a provider that echoes the key back gets it scrubbed (positive control: the mock really received it)', async () => {
      mock.reset();
      const r = await send(w, 1050, url('openai-responses', 'echo'), {...bearer('openai'), body: '{}'});
      assert.equal(mock.requests[0].headers.authorization, `Bearer ${KEY_A}`);
      assert.equal(r.ok, true);
      assert.ok(r.redactions >= 1, 'the worker reports that it scrubbed something');
      assert.ok(!r.body.includes(KEY_A), 'the key is gone from the body');
      assert.match(r.body, /echo:Bearer \[redacted\]/);
    });

    await check('redirects are never followed: 302 is REDIRECT_BLOCKED and exactly one request was made', async () => {
      mock.reset();
      const r = await send(w, 1060, url('openai-responses', 'redirect'), {...bearer('openai'), body: '{}'});
      assert.deepEqual([r.ok, r.error], [false, 'REDIRECT_BLOCKED']);
      assert.equal(mock.requests.length, 1);
      assert.ok(!JSON.stringify(r).includes('stolen'), 'the redirect target is not reported');
    });

    await check('responses over the 4 MiB cap are refused (RESPONSE_TOO_LARGE)', async () => {
      const r = await send(w, 1070, url('openai-responses', 'huge'), {...bearer('openai'), body: '{}', timeoutMs: 20000});
      assert.deepEqual([r.ok, r.error], [false, 'RESPONSE_TOO_LARGE'], JSON.stringify(r).slice(0, 200));
      assert.equal((await send(w, 1071, url('openai-responses'), {...bearer('openai'), body: '{}'})).status, 200, 'still serving');
    });

    await check('a key is never sent to an origin it was not stored for; missing credentials send nothing', async () => {
      mock.reset();
      const a = await send(w, 1080, url('openai-responses'), {...bearer('other'), body: '{}'});
      assert.deepEqual([a.ok, a.error], [false, 'CREDENTIAL_ORIGIN_MISMATCH']);
      const b = await send(w, 1081, url('openai-responses'), {...bearer('never-stored'), body: '{}'});
      assert.deepEqual([b.ok, b.error], [false, 'NO_CREDENTIAL']);
      assert.equal(mock.requests.length, 0, 'nothing reached the server');
    });

    await check('policy: http, user info, forbidden headers and malformed requests are refused before anything is sent', async () => {
      mock.reset();
      const cases = [
        [send(w, 1090, `http://${origin}/v1/responses`, {body: '{}'}), 'URL_NOT_ALLOWED'],
        [send(w, 1091, `https://user:pw@127.0.0.1:${mock.port}/`, {body: '{}'}), 'URL_NOT_ALLOWED'],
        [send(w, 1092, url('openai-responses'), {headers: {Authorization: 'Bearer stolen'}, body: '{}'}), 'HEADER_NOT_ALLOWED'],
        [send(w, 1093, url('openai-responses'), {credentialRef: 'openai', body: '{}'}), 'INVALID_REQUEST'],
        [send(w, 1094, url('openai-responses'), {timeoutMs: 0, body: '{}'}), 'INVALID_REQUEST'],
        [send(w, 1095, url('openai-responses'), {surprise: 1, body: '{}'}), 'INVALID_REQUEST']
      ];
      for (const [promise, error] of cases) { const r = await promise; assert.deepEqual([r.ok, r.error], [false, error], JSON.stringify(r)); }
      assert.equal(mock.requests.length, 0);
      w.sendRaw('this is not json');
      w.sendRaw(JSON.stringify({op: 'nope', id: 1099}));
      assert.equal((await w.call({op: 'nope', id: 1098})).error, 'UNKNOWN_OPERATION');
      for (let i = 0; i < 100 && !w.lines.some(l => l.includes('INVALID_JSON')); i++) await sleep(20);
      assert.ok(w.lines.some(l => l.includes('INVALID_JSON')));
    });

    await check('a refused connection is CONNECT_FAILED', async () => {
      const closed = await new Promise(resolve => { const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const {port} = s.address(); s.close(() => resolve(port)); }); });
      const r = await send(w, 1100, `https://127.0.0.1:${closed}/v1`, {body: '{}', timeoutMs: 8000});
      assert.deepEqual([r.ok, r.error], [false, 'CONNECT_FAILED'], JSON.stringify(r));
    });

    // ------------------------------------------------------------------------------------------ plain http, test mode
    await check('--test-allow-loopback-http: plain http works for loopback only, and still injects the key', async () => {
      const seen = [];
      const server = http.createServer((req, res) => { seen.push(req.headers); req.resume(); req.on('end', () => { res.writeHead(200, {'content-type': 'application/json'}); res.end('{"plain":true}'); }); });
      await new Promise(r => server.listen(0, '127.0.0.1', r));
      const port = server.address().port;
      const hw = new Worker(['--test-allow-loopback-http', '--test-credential-namespace', NAMESPACE]);
      try {
        assert.equal((await hw.call({op: 'health'})).allowLoopbackHttp, true);
        assert.equal((await hw.call({op: 'cred.set', credentialRef: 'plainhttp', host: `127.0.0.1:${port}`, secret: KEY_A})).ok, true);
        const r = await send(hw, 1, `http://127.0.0.1:${port}/v1/x`, {...bearer('plainhttp'), body: '{}'});
        assert.deepEqual([r.ok, r.status, r.body], [true, 200, '{"plain":true}'], JSON.stringify(r));
        assert.equal(seen[0].authorization, `Bearer ${KEY_A}`);
        const off = await send(hw, 2, 'http://example.com/v1', {body: '{}'});
        assert.deepEqual([off.ok, off.error], [false, 'URL_NOT_ALLOWED'], 'a non-loopback host is refused before any lookup');
        assert.equal((await hw.call({op: 'cred.delete', credentialRef: 'plainhttp'})).deleted, true);
      } finally { hw.kill(); await new Promise(r => server.close(r)); }
    });

    // ---------------------------------------------------------------------------------------------- cleanup and EOF
    await check('end of input aborts a request that is still in flight and the worker exits promptly', async () => {
      const t0 = Date.now();
      const pending = send(w, 1200, url('openai-responses', 'hang'), {...bearer('openai'), body: '{}', timeoutMs: 30000}).catch(e => e);
      for (let i = 0; i < 200 && mock.requests.length < 1; i++) await sleep(25);
      mock.reset();
      for (const p of ['openai', 'anthropic', 'other']) assert.equal((await w.call({op: 'cred.delete', credentialRef: p})).deleted, true);
      const {code} = await w.end();
      assert.equal(code, 0);
      assert.ok(Date.now() - t0 < 6000, 'the 30 s deadline was not waited for');
      void pending; // the reply may or may not have been written before the pipe closed; nothing depends on it
    });

    await check('nothing is left in Credential Manager, also seen by a fresh worker', async () => {
      assert.ok(!leftovers(), `leftover entries: ${cmdkeyList()}`);
      const fresh = new Worker(['--test-credential-namespace', NAMESPACE]);
      for (const p of PROVIDERS) assert.equal((await fresh.call({op: 'cred.exists', credentialRef: p})).exists, false, p);
      await fresh.end();
    });

    await check('no key appears in any line any worker wrote (stdout or stderr), every stdout line is JSON, stderr stayed empty', async () => {
      const out = transcript.stdout.join('\n'), err = transcript.stderr.join('');
      for (const key of ALL_KEYS) {
        assert.ok(!out.includes(key), 'a key appeared on stdout');
        assert.ok(!err.includes(key), 'a key appeared on stderr');
      }
      assert.ok(transcript.stdout.length > 40, 'the check looked at real traffic');
      for (const line of transcript.stdout) JSON.parse(line);
      const valid = workers.filter(x => !x.args.includes('--no-such-option') && x.lines.length > 0);
      for (const x of valid) assert.equal(x.stderr, '', `stderr of a running worker must stay empty (${x.args.join(' ')})`);
    });

    console.log(`ai-worker-smoke: all checks passed (${exe})`);
  } finally {
    for (const x of workers) x.kill();
    await mock.close().catch(() => {});
    cleanupCredentials();
    if (leftovers()) { console.error('Credential Manager entries of this run could not be removed'); process.exitCode = 1; }
  }
})().catch(e => { console.error(e && e.stack || e); cleanupCredentials(); process.exit(1); });

// the whole run is bounded: a hung worker must fail the test, not stall the pipeline
setTimeout(() => { console.error('ai-worker-smoke: overall time limit (180 s) exceeded'); for (const x of workers) x.kill(); cleanupCredentials(); process.exit(1); }, 180000).unref();
