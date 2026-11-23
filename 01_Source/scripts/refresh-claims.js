'use strict';
// Writes the numbers of the documents from the products on the disk. Nothing is typed in by hand.
//
//   node scripts/refresh-claims.js           # refresh the numbers
//   node scripts/refresh-claims.js --check   # only report what is out of date
//
// The numbers that are written:
//   - the size and the SHA-256 of the installer (the build is NOT byte-reproducible: the digest moves on every
//     build, so the documents have to follow the artefact);
//   - the test counts of the evidence logs (the core, the TSF crate, the agent crate, the TASE crates, the
//     JavaScript suite and the installer crate).
//
// Every number is read from a product: the artefact on the disk and the logs of the last gate round. A pattern
// that is not found is an error, not a silent skip: a renamed section must not leave a stale number behind.
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');

const root = path.resolve(__dirname, '..', '..');
const checkOnly = process.argv.includes('--check');

const FILES = {
  matrix: path.join(root, '02_Verification', 'IME_FRAMEWORK_MATRIX.md'),
  readme: path.join(root, 'README_交付说明.md'),
  installer: path.join(root, '01_Source', 'docs', 'INSTALLER.md'),
  checklist: path.join(root, '02_Verification', 'REAL_MACHINE_CHECKLIST.md'),
};

// ---- the claims block says which artefact and which logs are meant ---------------------------------------------
const text = {};
for (const [key, file] of Object.entries(FILES)) text[key] = fs.readFileSync(file, 'utf8');
const block = text.matrix.match(/<!--\s*installer-claims:begin\s*-->([\s\S]*?)<!--\s*installer-claims:end\s*-->/);
if (!block) {
  console.error('the framework matrix has no machine-readable claims block');
  process.exit(1);
}
const claims = {};
for (const raw of block[1].split(/\r?\n/)) {
  const m = /^\s*([a-z0-9_]+)\s*=\s*(.+)$/.exec(raw);
  if (m) claims[m[1]] = m[2].trim();
}

// ---- the artefact on the disk --------------------------------------------------------------------------------
const installerPath = path.join(root, '01_Source', 'dist', claims.installer_file || '');
if (!claims.installer_file || !fs.existsSync(installerPath)) {
  console.error(`the installer is not on the disk: ${installerPath}`);
  console.error('build it first:  node scripts/build-installer.js');
  process.exit(1);
}
const bytes = fs.readFileSync(installerPath);
const digest = crypto.createHash('sha256').update(bytes).digest('hex');
const size = bytes.length;
const sizeText = size.toLocaleString('en-US');

// ---- the counts of the last gate round ------------------------------------------------------------------------
const readLog = name => {
  const file = path.join(root, '02_Verification', name);
  if (!fs.existsSync(file)) {
    console.error(`the evidence log is missing: 02_Verification/${name}`);
    process.exit(1);
  }
  const raw = fs.readFileSync(file);
  if (raw[0] === 0xFF && raw[1] === 0xFE) return raw.subarray(2).toString('utf16le');
  return raw.toString('utf8');
};
const runAll = readLog(claims.evidence_run_all_log);
const setup = readLog(claims.evidence_setup_log);
const last = (text, re) => {
  const found = [...text.matchAll(re)];
  return found.length ? found[found.length - 1][1] : null;
};
const counts = {
  rust_core_tests: last(runAll, /Rust: cargo test --workspace.*?(\d+) tests/gm),
  rust_tsf_tests: last(runAll, /TSF crate \(crates\/pime-tsf\): cargo test.*?(\d+) tests/gm),
  rust_agent_tests: last(runAll, /AI agent front-ends \(crates\/pime-agent\): cargo test.*?(\d+) tests/gm),
  rust_tase_tests: last(runAll, /TASE crates \(the template-aware semantic execution system\): cargo test.*?(\d+) tests/gm),
  js_tests: last(runAll, /^\S+ tests (\d+)\s*$/gm),
};
// the installer crate: the unit tests first, then the real file system tests, then the real link tests.
// The order of the log is the order of the three targets - the counts are **not** sorted: the numbers belong to
// the targets, and sorting them would put the number of the link tests into the claim of the unit tests.
const setupCounts = [...setup.matchAll(/test result: ok\. (\d+) passed/g)]
  .map(m => Number(m[1])).filter(n => n > 0);
if (setupCounts.length === 3) {
  counts.rust_setup_unit_tests = String(setupCounts[0]);
  counts.rust_setup_real_fs_tests = String(setupCounts[1]);
  counts.rust_setup_real_links_tests = String(setupCounts[2]);
}
for (const [key, value] of Object.entries(counts)) {
  if (value === null || value === undefined) {
    console.error(`the log does not carry the number of ${key}: the gate round is not complete`);
    process.exit(1);
  }
}

// ---- the numbers that are written ------------------------------------------------------------------------------
const problems = [];
const changed = [];
// a number in the claims block
const claimLine = (key, value) => {
  const re = new RegExp(`^(\\s*${key}\\s*=\\s*)(.+)$`, 'm');
  const m = text.matrix.match(re);
  if (!m) {
    problems.push(`the claims block has no "${key}"`);
    return;
  }
  if (m[2].trim() !== String(value)) changed.push(`claims ${key}: ${m[2].trim()} -> ${value}`);
  text.matrix = text.matrix.replace(re, `$1${value}`);
};
// a number in a prose line: the regex captures the number in the group `group` (default 1)
const prose = (key, re, value, label, group = 1) => {
  const m = text[key].match(re);
  if (!m) {
    problems.push(`${label}: the pattern is not in ${key}`);
    return;
  }
  const before = m[group];
  if (before === undefined) {
    problems.push(`${label}: the pattern has no group ${group} in ${key}`);
    return;
  }
  if (before !== String(value)) changed.push(`${label}: ${before} -> ${value}`);
  text[key] = text[key].replace(m[0], m[0].replace(before, String(value)));
};

claimLine('installer_bytes', size);
claimLine('installer_sha256', digest);
for (const [key, value] of Object.entries(counts)) claimLine(key, value);

// the matrix: the prose row of the artefact
prose('matrix', /\| 大小 \| ([\d,]+) 字节 \|/, sizeText, 'matrix prose size');
prose('matrix', /\| SHA-256 \| `([0-9a-f]{64})` \|/, digest, 'matrix prose sha');
// the matrix: the prose rows of the test counts
prose('matrix', /\| Rust 核心 `cargo test --workspace`[^\n]*?\*\*(\d+)( \/ \d+ 通过\*\*)/, counts.rust_core_tests, 'matrix core count');
prose('matrix', /\| Rust TSF `cargo test`[^\n]*?\*\*(\d+)( \/ \d+ 通过\*\*)/, counts.rust_tsf_tests, 'matrix tsf count');
prose('matrix', /\| Rust agent `cargo test`[^\n]*?\*\*(\d+)( \/ \d+ 通过\*\*)/, counts.rust_agent_tests, 'matrix agent count');
prose('matrix', /\| 安装器 `cargo test`[^\n]*?\*\*(\d+)( \/ \d+ 通过\*\*)/, totalSetup(), 'matrix installer count');
prose('matrix', /\| JavaScript 单元 \/ 集成 \+ 引擎差分[^\n]*?\*\*(\d+)( \/ \d+ 通过\*\*)/, counts.js_tests, 'matrix js count');

// the README: the counts of the suites
prose('readme', /JavaScript \*\*(\d+)( \/ \d+\*\*)/, counts.js_tests, 'README js count');
prose('readme', /Rust 核心 \*\*(\d+)( \/ \d+\*\*)/, counts.rust_core_tests, 'README core count');
prose('readme', /Rust TSF crate \*\*(\d+)( \/ \d+\*\*)/, counts.rust_tsf_tests, 'README tsf count');
prose('readme', /Rust agent crate \*\*(\d+)( \/ \d+\*\*)/, counts.rust_agent_tests, 'README agent count');
prose('readme', /安装器 crate \*\*(\d+)( \/ \d+\*\*)/, totalSetup(), 'README installer count');

// INSTALLER.md: the count of the installer crate
prose('installer', /安装器 \*\*(\d+)( 项\*\*)/, totalSetup(), 'INSTALLER.md installer count');

// the checklist: the digest the human checks before installing
prose('checklist', /(（)([\d,]+)( 字节，sha256 `)[0-9a-f]{64}(`；)/, sizeText, 'checklist size', 2);
text.checklist = text.checklist.replace(/(sha256 `)[0-9a-f]{64}(`；)/, `$1${digest}$2`);

function totalSetup() {
  return String(setupCounts.reduce((a, b) => a + b, 0));
}

if (problems.length) {
  console.error('ABORTED - the documents do not have the expected shape:');
  for (const problem of problems) console.error(`  - ${problem}`);
  process.exit(1);
}

console.log(`the artefact: ${sizeText} bytes, sha256 ${digest}`);
console.log(`the counts of the log: core ${counts.rust_core_tests}, tsf ${counts.rust_tsf_tests}, agent ${counts.rust_agent_tests}, tase ${counts.rust_tase_tests}, js ${counts.js_tests}, installer ${totalSetup()}`);
if (!changed.length) {
  console.log('the documents already carry these numbers');
  process.exit(0);
}
for (const line of changed) console.log(`  ${line}`);
if (checkOnly) {
  console.log('the documents are out of date (run the script without --check)');
  process.exit(1);
}
for (const [key, file] of Object.entries(FILES)) fs.writeFileSync(file, text[key], 'utf8');
console.log('OK: the documents carry the numbers of the products on the disk');
