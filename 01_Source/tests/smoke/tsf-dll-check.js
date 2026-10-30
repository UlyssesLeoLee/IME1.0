'use strict';
// Builds crates/pime-tsf (release, x86_64-pc-windows-gnu) and checks the resulting DLL as a PE file:
// x64, a DLL, exactly the four COM entry points, and only Windows system DLLs imported (no MinGW runtime DLL a user would
// not have). It does NOT register or load the DLL: nothing here proves that Windows accepts it as an input method.
const {spawnSync} = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {parsePe} = require('../../scripts/pe-info.js');

const crate = path.resolve(__dirname, '..', '..', 'crates', 'pime-tsf');
const targetDir = process.env.CARGO_TARGET_DIR || path.join(os.tmpdir(), 'pime-cargo-target');
const TARGET = 'x86_64-pc-windows-gnu';
const REQUIRED_EXPORTS = ['DllCanUnloadNow', 'DllGetClassObject', 'DllRegisterServer', 'DllUnregisterServer'];
// system DLLs present on every Windows 10/11 x64 installation
const ALLOWED_IMPORT = /^(kernel32|ntdll|ole32|oleaut32|msvcrt|propsys|userenv|ws2_32|bcryptprimitives|user32|gdi32|advapi32|msctf|api-ms-win-[a-z0-9-]+)\.dll$/;

let failed = 0;
const check = (ok, text) => { console.log(`${ok ? 'ok  ' : 'FAIL'} ${text}`); if (!ok) failed++; };

let built = false;
for (let attempt = 1; attempt <= 3 && !built; attempt++) { // a freshly linked file can fail once while Defender scans it
  const r = spawnSync('cargo', ['build', '--release', '--target', TARGET], {cwd: crate, encoding: 'utf8', env: {...process.env, CARGO_TARGET_DIR: targetDir}, shell: process.platform === 'win32'});
  built = r.status === 0;
  if (!built) {
    process.stderr.write((r.stdout || '') + (r.stderr || ''));
    if (!/os error 5/.test((r.stdout || '') + (r.stderr || ''))) break;
  }
}
check(built, `cargo build --release --target ${TARGET} (crates/pime-tsf)`);
if (built) {
  const dll = path.join(targetDir, TARGET, 'release', 'pime_tsf.dll');
  check(fs.existsSync(dll), `output exists: ${dll}`);
  const info = parsePe(fs.readFileSync(dll));
  check(info.machineCode === 0x8664, `machine is x64 (0x8664), found ${info.machine}`);
  check(info.isDll, 'file is a DLL');
  check(JSON.stringify(info.exports) === JSON.stringify(REQUIRED_EXPORTS), `exports exactly ${REQUIRED_EXPORTS.join(', ')} (found: ${info.exports.join(', ')})`);
  const bad = info.imports.filter(n => !ALLOWED_IMPORT.test(n));
  check(bad.length === 0, `imports only Windows system DLLs (${info.imports.length} imported${bad.length ? '; unexpected: ' + bad.join(', ') : ''})`);
  console.log(`size: ${fs.statSync(dll).size} bytes`);
}
console.log(failed ? `tsf-dll-check: ${failed} check(s) FAILED` : 'tsf-dll-check: all checks passed (build + PE inspection only; the DLL was not registered or loaded)');
process.exit(failed ? 1 : 0);
