'use strict';
// Smoke test of the real pime-core executable over JSON Lines: protocol 0.3 health, a prediction that must equal the JS engine's,
// the verified x86 encoder, the secret scanner (findings never echo values), and protocol error replies.
//   PIME_CORE_EXE=<path to pime-core.exe> node tests/smoke/core-binary-smoke.js
const {spawnSync} = require('node:child_process');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const exe = process.env.PIME_CORE_EXE || path.join(process.env.CARGO_TARGET_DIR || path.join(os.tmpdir(), 'pime-cargo-target'), 'debug', 'pime-core.exe');
if (!fs.existsSync(exe)) { console.error(`pime-core executable not found: ${exe} (build it with cargo build, or set PIME_CORE_EXE)`); process.exit(2); }
const jsLanguages = require(path.join(__dirname, '..', '..', 'editor-bridges', 'vscode', 'src', 'core', 'languages.js'));

const requests = [
  {op: 'health', id: 1},
  {op: 'predict', id: 2, language: 'rust', prefix: 'fnm', before: '', after: '', dialect: {edition: '2024'}, limit: 3},
  {op: 'encode', id: 3, instruction: 'mov rax, rbx'},
  {op: 'encode', id: 4, instruction: 'jmp external_symbol'},
  {op: 'scan', id: 5, text: 'api_key = "mock-key-test-123"', fileName: ''},
  {op: 'nope', id: 6}
];
const input = requests.map(r => JSON.stringify(r)).join('\n') + '\nnot json\n';
const run = spawnSync(exe, [], {input, encoding: 'utf8'});
assert.equal(run.status, 0, 'the core must exit cleanly at end of input');
const out = run.stdout.trim().split('\n').map(l => JSON.parse(l));

assert.equal(out[0].ok, true); assert.equal(out[0].protocol, '0.3'); assert.equal(out[0].languages.length, 7);
assert.equal(out[1].ok, true); assert.equal(out[1].id, 2, 'ids are echoed');
const js = jsLanguages.predict({profile: 'rust', prefix: 'fnm', before: '', limit: 3, dialect: {edition: '2024'}});
assert.equal(out[1].candidates[0].id, js[0].id, 'same first candidate as the JavaScript engine');
assert.equal(out[1].candidates[0].text, js[0].text);
assert.deepEqual([out[2].ok, out[2].hex], [true, '48 89 D8']);
assert.deepEqual([out[3].ok, out[3].code], [false, 'BYTE_ENCODING_UNRESOLVED']);
assert.equal(out[4].ok, true); assert.equal(out[4].result.findings[0].kind, 'credential-assignment');
assert.ok(!run.stdout.includes('mock-key-test-123'), 'the scanner must never echo a secret value');
assert.deepEqual([out[5].ok, out[5].error], [false, 'UNKNOWN_OPERATION']);
assert.deepEqual([out[6].ok, out[6].error], [false, 'INVALID_JSON']);
console.log(`core-binary-smoke: all checks passed (${exe})`);
