'use strict';
// Binds the numbers the IME framework matrix quotes to the machine products they claim to describe.
//
//   node tests/smoke/installer-claims-check.js
//
// 02_Verification/IME_FRAMEWORK_MATRIX.md carries a machine-readable claims block. This script compares it with
//   - the built installer in 01_Source/dist/ (file size, SHA-256, the payload entry sizes, the size of the stub), and
//   - the raw test logs the document cites (the test counts).
// A claim that does not match the artifact FAILS. A cited evidence log that is missing FAILS: the document would be
// quoting evidence that no longer exists. The only skip is "the installer has not been built on this machine"; that
// case says so loudly, because the claims are then unverified, not verified.
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..', '..', '..'); // the repository root
const docPath = path.join(root, '02_Verification', 'IME_FRAMEWORK_MATRIX.md');
const distDir = path.join(root, '01_Source', 'dist');
const MAGIC = Buffer.from('PIME-PAYLOAD-V1\0', 'latin1');
const sha256 = data => crypto.createHash('sha256').update(data).digest();

let failed = 0;
const check = (ok, text) => { console.log(`${ok ? 'ok  ' : 'FAIL'} ${text}`); if (!ok) failed++; };
const note = text => console.log(`note ${text}`);

// ---- the claims block ----------------------------------------------------------------------------------------------------------------
if (!fs.existsSync(docPath)) {
  console.log(`FAIL the framework matrix is missing: ${docPath}`);
  process.exit(1);
}
const doc = fs.readFileSync(docPath, 'utf8');
const block = doc.match(/<!--\s*installer-claims:begin\s*-->([\s\S]*?)<!--\s*installer-claims:end\s*-->/);
if (!block) {
  console.log(`FAIL ${docPath} has no machine-readable claims block (<!-- installer-claims:begin --> ... end)`);
  process.exit(1);
}
const claims = {};
for (const raw of block[1].split(/\r?\n/)) {
  const line = raw.trim();
  if (!line || line.startsWith('#')) continue;
  const eq = line.indexOf('=');
  if (eq < 0) continue;
  claims[line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
}
const claim = key => {
  if (!(key in claims)) { check(false, `the claims block has no "${key}"`); return null; }
  return claims[key];
};
note(`claims read from ${path.relative(root, docPath)}: ${Object.keys(claims).length} entries`);

// ---- the built installer -------------------------------------------------------------------------------------------------------------
// An independent reader of the payload format (crates/pime-setup/src/payload.rs): a packing bug cannot verify itself.
function readPayload(exe) {
  const footer = exe.length - 56;
  if (footer < 0) throw new Error('the file is too short to carry a payload');
  if (!exe.subarray(footer, footer + 16).equals(MAGIC)) throw new Error('no payload footer at the end of the file');
  const blobLength = Number(exe.readBigUInt64LE(footer + 16));
  const blob = exe.subarray(footer - blobLength, footer);
  if (!sha256(blob).equals(exe.subarray(footer + 24, footer + 56))) throw new Error('the payload hash does not match the footer');
  const entries = [];
  let at = 4;
  for (let i = 0; i < blob.readUInt32LE(0); i++) {
    const nameLength = blob.readUInt16LE(at); at += 2;
    const name = blob.toString('utf8', at, at + nameLength); at += nameLength;
    const size = Number(blob.readBigUInt64LE(at)); at += 8;
    const hash = blob.subarray(at, at + 32); at += 32;
    const data = blob.subarray(at, at + size); at += size;
    if (!sha256(data).equals(hash)) throw new Error(`the hash of ${name} does not match`);
    entries.push({name, size});
  }
  if (at !== blob.length) throw new Error('bytes remain after the last payload entry');
  return {entries, blobLength};
}

const installerName = claim('installer_file');
const installerPath = installerName ? path.join(distDir, installerName) : null;
if (!installerPath || !fs.existsSync(installerPath)) {
  note(`SKIPPED the installer is not built on this machine (${installerPath || '?'} does not exist).`);
  note('        The artifact claims below are UNVERIFIED, not verified.');
  note('        Build it with: cd 01_Source && node scripts/build-installer.js');
} else {
  const bytes = fs.readFileSync(installerPath);
  check(String(bytes.length) === claim('installer_bytes'), `the installer is ${bytes.length} bytes`);
  const digest = sha256(bytes).toString('hex');
  check(digest === claim('installer_sha256'), `the installer SHA-256 is ${digest}`);

  const sidecar = `${installerPath}.sha256`;
  if (fs.existsSync(sidecar)) {
    const sidecarText = fs.readFileSync(sidecar, 'utf8');
    check(sidecarText.includes(digest) && sidecarText.includes(installerName),
      'the .sha256 sidecar names the installer and carries the same digest');
  } else {
    check(false, `the .sha256 sidecar is missing: ${sidecar}`);
  }

  let payload = null;
  try {
    payload = readPayload(bytes);
    check(payload.entries.length === Number(claim('payload_entry_count')), `the payload carries ${payload.entries.length} entries`);
    for (const entry of payload.entries) {
      const key = `payload_${entry.name.replace(/[^A-Za-z0-9]+/g, '_').toLowerCase()}_bytes`;
      if (key in claims) check(String(entry.size) === claims[key], `the payload entry ${entry.name} is ${entry.size} bytes`);
      else note(`the claims block has no "${key}" (the payload entry ${entry.name})`);
    }
    // the stub is everything in front of the payload blob: file = stub | blob | footer(56 bytes)
    const stubBytes = bytes.length - payload.blobLength - 56;
    check(String(stubBytes) === claim('stub_bytes'), `the installer stub is ${stubBytes} bytes`);
  } catch (e) {
    check(false, `the payload of the installer does not read back: ${e.message}`);
  }
}

// ---- the test counts the document quotes come from the logs it cites ----------------------------------------------------------------
// The logs in 02_Verification are UTF-8, but a log written by PowerShell's Tee-Object is UTF-16LE with a BOM.
// Decode both, so the evidence is readable whichever way it was captured.
function readText(file) {
  const bytes = fs.readFileSync(file);
  if (bytes[0] === 0xFF && bytes[1] === 0xFE) return bytes.subarray(2).toString('utf16le');
  if (bytes[0] === 0xEF && bytes[1] === 0xBB && bytes[2] === 0xBF) return bytes.subarray(3).toString('utf8');
  return bytes.toString('utf8');
}

function readLog(key) {
  const name = claim(key);
  if (!name) return null;
  const file = path.join(root, '02_Verification', name);
  if (!fs.existsSync(file)) { check(false, `the cited evidence log is missing: 02_Verification/${name}`); return null; }
  return readText(file);
}
const lastMatch = (text, re) => {
  const found = [...text.matchAll(re)];
  return found.length ? found[found.length - 1][1] : null;
};

const runAll = readLog('evidence_run_all_log');
if (runAll !== null) {
  const pairs = [
    ['rust_core_tests', /Rust: cargo test --workspace.*?(\d+) tests/gm],
    ['rust_tsf_tests', /TSF crate \(crates\/pime-tsf\): cargo test.*?(\d+) tests/gm],
    ['rust_agent_tests', /AI agent front-ends \(crates\/pime-agent\): cargo test.*?(\d+) tests/gm],
    ['js_tests', /^\S+ tests (\d+)\s*$/gm],
  ];
  for (const [key, re] of pairs) {
    const found = lastMatch(runAll, re);
    const claimed = claim(key);
    check(found !== null && found === claimed, `${key}: the log says ${found === null ? 'nothing' : found}, the document says ${claimed}`);
  }
  const jsFail = lastMatch(runAll, /^\S+ fail (\d+)\s*$/gm);
  check(jsFail === '0', `the JavaScript suite in the log has ${jsFail} failures`);
}

const setupLog = readLog('evidence_setup_log');
if (setupLog !== null) {
  const found = [...setupLog.matchAll(/test result: ok\. (\d+) passed/g)].map(m => Number(m[1])).filter(n => n > 0).sort((a, b) => a - b);
  const claimed = ['rust_setup_unit_tests', 'rust_setup_real_fs_tests', 'rust_setup_real_links_tests']
    .map(claim).filter(v => v !== null).map(Number).sort((a, b) => a - b);
  check(found.length === claimed.length && found.every((n, i) => n === claimed[i]),
    `the installer crate test counts: the log says ${found.join(' + ')}, the document says ${claimed.join(' + ')}`);
  const failures = [...setupLog.matchAll(/test result: FAILED/g)].length;
  check(failures === 0, `the installer crate suite in the log has ${failures} failing test targets`);
}

console.log(failed
  ? `installer-claims-check: ${failed} check(s) FAILED - the document and the machine products disagree`
  : 'installer-claims-check: all checks passed (the document matches the built installer and the cited logs)');
process.exit(failed ? 1 : 0);
