'use strict';
// Runs the tests of crates/pime-ipc (PIME-IPC/1, named pipes) and checks the crate's structural promises:
//   * `windows` is pinned to 0.58.0 in the crate's own Cargo.lock (same pin as crates/pime-tsf, see docs/VERSIONS.md)
//   * `unsafe` appears only in the two FFI modules (the crate also denies it everywhere else at compile time)
//   * cargo test: unit tests + loopback integration tests, each on its own randomly named pipe
// The loopback tests create and close named pipes of this process only. They never touch a real PIME pipe, the registry, a
// COM server or the network. Cases that need a second Windows user, a remote machine or another session are NOT RUN here
// (listed in docs/IPC.md).
//   node tests/smoke/ipc-smoke.js [--repeat=N]     (default 1; use 3 to look for flakiness)
const {spawnSync} = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const crate = path.resolve(__dirname, '..', '..', 'crates', 'pime-ipc');
const targetDir = process.env.CARGO_TARGET_DIR || path.join(os.tmpdir(), 'pime-cargo-target');
const repeat = Math.max(1, Number((process.argv.find(a => /^--repeat=\d+$/.test(a)) || '--repeat=1').split('=')[1]));
const ALLOWED_UNSAFE = new Set(['pipe.rs', 'winsec.rs']);

let failed = 0;
const check = (ok, text) => { console.log(`${ok ? 'ok  ' : 'FAIL'} ${text}`); if (!ok) failed++; };

// 1. the pin
const lock = fs.readFileSync(path.join(crate, 'Cargo.lock'), 'utf8');
const win = /name = "windows"\r?\nversion = "([^"]+)"/.exec(lock);
check(win && win[1] === '0.58.0', `Cargo.lock pins windows 0.58.0 (found ${win ? win[1] : 'none'})`);

// 2. unsafe only where allowed (comments and doc comments are ignored)
const stray = [];
for (const f of fs.readdirSync(path.join(crate, 'src')).filter(n => n.endsWith('.rs'))) {
  if (ALLOWED_UNSAFE.has(f)) continue;
  const code = fs.readFileSync(path.join(crate, 'src', f), 'utf8').split(/\r?\n/).filter(l => !/^\s*\/\//.test(l)).join('\n');
  if (/\bunsafe\b/.test(code.replace(/#!\[deny\([^)]*\)\]/g, '').replace(/allow\(unsafe_code\)/g, ''))) stray.push(f);
}
check(stray.length === 0, `no unsafe outside ${[...ALLOWED_UNSAFE].join(', ')}${stray.length ? ' (found in ' + stray.join(', ') + ')' : ''}`);

// 3. cargo test (a freshly linked test exe can fail once with "os error 5" while Defender scans it: retry)
function cargoTest() {
  for (let attempt = 1; attempt <= 3; attempt++) {
    const r = spawnSync('cargo', ['test'], {cwd: crate, encoding: 'utf8', env: {...process.env, CARGO_TARGET_DIR: targetDir}, shell: process.platform === 'win32', maxBuffer: 64 * 1024 * 1024});
    const out = (r.stdout || '') + (r.stderr || '');
    if (r.status === 0 || !/os error 5/.test(out)) return {status: r.status, out};
  }
  return {status: 1, out: 'os error 5 on every attempt'};
}

const counts = [];
for (let i = 1; i <= repeat; i++) {
  const {status, out} = cargoTest();
  const results = [...out.matchAll(/test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored/g)];
  const passed = results.reduce((n, m) => n + Number(m[2]), 0);
  const bad = results.reduce((n, m) => n + Number(m[3]), 0);
  check(status === 0 && bad === 0 && passed > 0, `cargo test run ${i}/${repeat} (crates/pime-ipc): ${passed} passed, ${bad} failed`);
  if (status !== 0) process.stderr.write(out.split('\n').filter(l => /FAILED|panicked|error|warning/.test(l)).slice(0, 40).join('\n') + '\n');
  counts.push(passed);
}
check(counts.every(n => n === counts[0]), `same number of tests in every run (${counts.join(', ')})`);
console.log(failed ? `ipc-smoke: ${failed} check(s) FAILED` : 'ipc-smoke: all checks passed (named pipes of this process only; second-user, remote-client and cross-session cases are NOT RUN here)');
process.exit(failed ? 1 : 0);
