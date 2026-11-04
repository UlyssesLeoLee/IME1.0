'use strict';
// Cross-checks the evidence set: every number that appears in more than one place must agree.
//
//   node tests/smoke/evidence-cross-check.js
//
// The framework matrix quotes the size, the SHA-256 and the payload entry sizes of the built installer, and the
// test counts of the two evidence logs.  installer-claims-check.js binds the matrix to the artifact and to the logs.
// This script closes the remaining gap: it also binds
//   - the .sha256 sidecar next to the installer,
//   - 02_Verification/installer-build.log (the build transcript), and
//   - the numbers written in the prose of the matrix, of README_交付说明.md and of INSTALLER.md,
// to the artifact that is actually on disk.  A build log that describes an older build than the one that ships is
// exactly the kind of stale evidence this project must not have.
//
// If the installer has not been built on this machine the artifact checks are skipped, loudly: the numbers are then
// unverified, not verified.
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..', '..', '..');       // the repository root
const read = file => fs.readFileSync(path.join(root, file), 'utf8');
const sha256 = file => crypto.createHash('sha256').update(fs.readFileSync(path.join(root, file))).digest('hex');

let failed = 0;
const check = (ok, text) => { console.log(`${ok ? 'ok  ' : 'FAIL'} ${text}`); if (!ok) failed++; };
const eq = (label, actual, expected) => {
  const same = String(actual) === String(expected);
  check(same, `${label}: ${same ? actual : `${actual} != ${expected}`}`);
};

// ---- the claims block of the matrix -------------------------------------------------------------------------------------------------
const doc = read('02_Verification/IME_FRAMEWORK_MATRIX.md');
const block = doc.match(/<!--\s*installer-claims:begin\s*-->([\s\S]*?)<!--\s*installer-claims:end\s*-->/);
if (!block) { console.log('FAIL the framework matrix has no machine-readable claims block'); process.exit(1); }
const claims = {};
for (const raw of block[1].split(/\r?\n/)) {
  const m = /^\s*([a-z0-9_]+)\s*=\s*(.+)$/.exec(raw);
  if (m) claims[m[1]] = m[2].trim();
}
const total = k => Number(claims.rust_setup_unit_tests) + Number(claims.rust_setup_real_fs_tests) + Number(claims.rust_setup_real_links_tests);

// ---- the built installer --------------------------------------------------------------------------------------------------------------
const exe = `01_Source/dist/${claims.installer_file}`;
if (!fs.existsSync(path.join(root, exe))) {
  console.log(`note SKIPPED the installer is not built on this machine (${exe} does not exist).`);
  console.log('      The artifact numbers below are UNVERIFIED, not verified.');
  console.log('      Build it with: cd 01_Source && node scripts/build-installer.js');
} else {
  const bytes = fs.statSync(path.join(root, exe)).size;
  const digest = sha256(exe);
  eq('the installer size vs the claims block', bytes, claims.installer_bytes);
  eq('the installer SHA-256 vs the claims block', digest, claims.installer_sha256);
  const sidecar = read(`${exe}.sha256`);
  check(sidecar.includes(digest) && sidecar.includes(claims.installer_file),
    'the .sha256 sidecar names the installer and carries the same digest');

  // the build transcript must describe THIS artifact, not an older one
  const buildLogPath = '02_Verification/installer-build.log';
  if (!fs.existsSync(path.join(root, buildLogPath))) {
    check(false, `the build log is missing: ${buildLogPath}`);
  } else {
    const buildLog = read(buildLogPath);
    const m = buildLog.match(/(\d+) bytes\s+sha256 ([0-9a-f]{64})/);
    if (!m) check(false, 'the build log does not record the size and the SHA-256 of the installer');
    else {
      eq('the build log size vs the installer on disk', m[1], bytes);
      eq('the build log SHA-256 vs the installer on disk', m[2], digest);
    }
    const stub = buildLog.match(/stub\s+(\d+) bytes/);
    if (stub) eq('the build log stub size vs the claims block', stub[1], claims.stub_bytes);
    else check(false, 'the build log does not record the size of the stub');
  }

  // the prose of the matrix repeats the same numbers
  eq('the matrix prose size vs the installer on disk', doc.match(/\| 大小 \| ([\d,]+) 字节 \|/)[1].replace(/,/g, ''), bytes);
  eq('the matrix prose SHA-256 vs the installer on disk', doc.match(/\| SHA-256 \| `([0-9a-f]{64})` \|/)[1], digest);
  eq('the matrix prose stub size vs the claims block', doc.match(/`pime-setup\.exe`，([\d,]+) 字节/)[1].replace(/,/g, ''), claims.stub_bytes);
  eq('the matrix prose pime.ico size vs the claims block', doc.match(/`pime\.ico` ([\d,]+) B/)[1].replace(/,/g, ''), claims.payload_pime_ico_bytes);
}

// ---- the two evidence logs --------------------------------------------------------------------------------------------------------------
const runAll = read(`02_Verification/${claims.evidence_run_all_log}`);
const setup = read(`02_Verification/${claims.evidence_setup_log}`);
const last = (text, re) => { const found = [...text.matchAll(re)]; return found.length ? found[found.length - 1][1] : null; };
eq('the run-all log core tests vs the claims block', last(runAll, /Rust: cargo test --workspace.*?(\d+) tests/gm), claims.rust_core_tests);
eq('the run-all log tsf tests vs the claims block', last(runAll, /TSF crate \(crates\/pime-tsf\): cargo test.*?(\d+) tests/gm), claims.rust_tsf_tests);
eq('the run-all log js tests vs the claims block', last(runAll, /^\S+ tests (\d+)\s*$/gm), claims.js_tests);
eq('the run-all log has no failing step', (runAll.match(/^FAIL\s/gm) || []).length, 0);
const setupCounts = [...setup.matchAll(/test result: ok\. (\d+) passed/g)].map(m => Number(m[1])).filter(n => n > 0).sort((a, b) => a - b);
eq('the setup log test counts vs the claims block', setupCounts.join('+'),
  [claims.rust_setup_unit_tests, claims.rust_setup_real_fs_tests, claims.rust_setup_real_links_tests].map(Number).sort((a, b) => a - b).join('+'));
eq('the setup log has no failing target', [...setup.matchAll(/test result: FAILED/g)].length, 0);

// ---- the test counts repeated in the prose ----------------------------------------------------------------------------------------------
eq('the README installer test count vs the claims block', read('README_交付说明.md').match(/安装器 crate \*\*(\d+) \/ \d+\*\*/)[1], total());
eq('the INSTALLER.md installer test count vs the claims block', read('01_Source/docs/INSTALLER.md').match(/安装器 \*\*(\d+) 项\*\*/)[1], total());
eq('the matrix §5 installer test count vs the claims block', doc.match(/\| 安装器 `cargo test`.*?\*\*(\d+) \/ \d+ 通过\*\*/)[1], total());

// The other counts are quoted in the prose as well. They used to be free text: a stale number in the README or in the
// matrix was invisible to every gate. They are bound to the claims block here, so a number that drifts away from the
// log it comes from turns the gate red instead of quietly lying in the document.
{
  const readme = read('README_交付说明.md');
  const prose = (label, text, re) => {
    const found = text.match(re);
    if (!found) { check(false, `the ${label} count is missing from the document`); return; }
    eq(`the ${label} count vs the claims block`, found[1], claims[label]);
  };
  prose('js_tests', readme, /JavaScript \*\*(\d+) \/ \d+\*\*/);
  prose('rust_core_tests', readme, /Rust 核心 \*\*(\d+) \/ \d+\*\*/);
  prose('rust_tsf_tests', readme, /Rust TSF crate \*\*(\d+) \/ \d+\*\*/);
  prose('rust_core_tests', doc, /\| Rust 核心 `cargo test --workspace`.*?\*\*(\d+) \/ \d+ 通过\*\*/);
  prose('rust_tsf_tests', doc, /\| Rust TSF `cargo test`.*?\*\*(\d+) \/ \d+ 通过\*\*/);
  prose('js_tests', doc, /\| JavaScript 单元 \/ 集成 \+ 引擎差分.*?\*\*(\d+) \/ \d+ 通过\*\*/);
}

// ---- the integrity manifest -------------------------------------------------------------------------------------------------------------
// The manifest itself is not in the manifest, but its entry count is quoted in two documents; that number has to
// match the file that is actually on disk, or the documents describe a list that no longer exists.
const manifestPath = path.join(root, 'MANIFEST_SHA256.txt');
if (!fs.existsSync(manifestPath)) {
  check(false, 'MANIFEST_SHA256.txt is missing');
} else {
  const entries = read('MANIFEST_SHA256.txt').split(/\r?\n/).filter(l => /^[0-9a-f]{64}\s/.test(l)).length;
  const matrixRow = doc.match(/\*\*(\d+) \/ \d+ 对得上\*\*/);
  eq('the manifest entry count vs the matrix §5 row', matrixRow ? matrixRow[1] : 'none', entries);
  const readmeRow = read('README_交付说明.md').match(/全量刷新，(\d+) 条全部对得上/);
  eq('the manifest entry count vs the README', readmeRow ? readmeRow[1] : 'none', entries);
}

console.log(failed
  ? `evidence-cross-check: ${failed} check(s) FAILED - the logs, the documents and the built installer disagree`
  : 'evidence-cross-check: all checks passed (the logs, the documents and the built installer agree)');
process.exit(failed ? 1 : 0);
