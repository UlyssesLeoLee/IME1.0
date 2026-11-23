'use strict';
// The preflight of the release: what is still missing before v0.3.3 can be published.
//
//   node scripts/release-preflight.js
//
// The gate of this repository is red on some machines for reasons that have nothing to do with the product: the
// **commit limit** of Windows (the commit charge of the machine against its limit) and the **antivirus scanner** that
// holds a freshly written executable. Both make the gate red in a way that looks like a product defect and is not.
//
// This script measures the three things that decide whether the release can go out, and says what to do about each
// one. It changes nothing: it only reads (the one thing it opens for writing is the installer, and it writes no byte).
//
// Exit code 0 when everything is in place, 1 when something is missing (the text says what).
const fs = require('fs');
const os = require('os');
const path = require('path');
const crypto = require('node:crypto');
const {spawnSync} = require('node:child_process');

const root = path.resolve(__dirname, '..', '..');
const read = relative => fs.readFileSync(path.join(root, relative), 'utf8');

let problems = 0;
const check = (ok, what, how = '') => {
  console.log(`${ok ? 'ok  ' : 'FAIL'}  ${what}${how ? ' -> ' + how : ''}`);
  if (!ok) problems++;
};

// ---- 1. the commit limit of the machine ------------------------------------------------------------------------
const counter = (name) => {
  const r = spawnSync('powershell', ['-NoProfile', '-Command', `(Get-Counter '${name}').CounterSamples[0].CookedValue`], {
    encoding: 'utf8',
  });
  const text = (r.stdout || '').trim();
  // An empty or non-numeric answer is **not** a number: `Number('')` is 0, and a limit of 0 MB would say "0 MB free of
  // 0 MB" instead of "the counter could not be read" (D-178).
  if (!/^-?\d+(\.\d+)?$/.test(text)) return null;
  const value = Number(text);
  return Number.isFinite(value) ? value : null;
};
const limit = counter('\\Memory\\Commit Limit');
const charged = counter('\\Memory\\Committed Bytes');
if (limit === null || charged === null) {
  console.log('note: the commit limit could not be read (the counters need Windows) - check it by hand');
} else {
  const free = (limit - charged) / (1024 * 1024);
  check(
    free >= 2048,
    'the machine has room for the gate (the commit limit needs about 2 GB)',
    `${Math.round(free)} MB free of ${Math.round(limit / (1024 * 1024))} MB`,
  );
}

// ---- 2. the antivirus scanner and the installer ------------------------------------------------------------------
// The scanner of this machine (360) kept a handle on the installer path for hours: the build could not write it and
// could not rename it. A file that cannot be opened for writing is such a handle.
const claims = read('02_Verification/IME_FRAMEWORK_MATRIX.md').match(
  /installer_file = ([^\r\n]+)/,
);
const installerName = claims ? claims[1].trim() : 'ProgrammingIME-0.3.3-win-x64-setup.exe';
const installer = path.join(root, '01_Source', 'dist', installerName);
if (!fs.existsSync(installer)) {
  check(false, `the installer is on the disk: ${installerName}`);
} else {
  let writable = true;
  try {
    const fd = fs.openSync(installer, 'r+');
    fs.closeSync(fd);
  } catch (e) {
    writable = false;
    check(false, 'the installer can be written (the scanner holds its path)', e.code || e.message);
  }
  if (writable) check(true, 'the installer can be written (the scanner holds no handle on it)');
}

// ---- 3. the build transcript and the artefact -------------------------------------------------------------------
// The transcript has to describe the artefact that is on the disk (the evidence cross-check compares the two).
const logPath = path.join(root, '02_Verification', 'installer-build.log');
if (!fs.existsSync(logPath)) {
  check(false, 'the build transcript exists');
} else if (!fs.existsSync(installer)) {
  check(false, 'the build transcript describes the artefact that is on the disk', 'the installer is not on the disk');
} else {
  const log = fs.readFileSync(logPath, 'utf8');
  const size = fs.statSync(installer).size;
  const digest = crypto.createHash('sha256').update(fs.readFileSync(installer)).digest('hex');
  const recorded = log.match(/(\d+) bytes\s+sha256 ([0-9a-f]{64})/);
  check(
    !!recorded && Number(recorded[1]) === size && recorded[2] === digest,
    'the build transcript describes the artefact that is on the disk',
    recorded ? `transcript ${recorded[1]} B / ${recorded[2].slice(0, 12)}…` : 'the transcript records no size and digest',
  );
}

// ---- 4. the claims and the gate log ------------------------------------------------------------------------------
for (const script of ['tests/smoke/installer-claims-check.js', 'tests/smoke/evidence-cross-check.js']) {
  const r = spawnSync(process.execPath, [path.join(root, '01_Source', script)], {encoding: 'utf8'});
  const text = `${r.stdout || ''}${r.stderr || ''}`;
  const failures = text.split('\n').filter(line => line.startsWith('FAIL'));
  check(r.status === 0, `the numbers of the documents match the products: ${script}`, failures.join(' | '));
}

// ---- what to do --------------------------------------------------------------------------------------------------
console.log(`
If the commit limit is short: close the browsers and the other big applications, or let Windows manage the paging
file (administrator PowerShell):  wmic computersystem set AutomaticManagedPagefile=True
If the scanner holds the installer: quit it from its tray (the process is 360tray) or restart the machine.
Then, from the root of the repository:
      node 01_Source/scripts/build-installer.js   (the transcript must end with the size and the digest)
      node 01_Source/scripts/gate-log.js          (the whole gate, about 25 minutes)
      node 01_Source/scripts/release-preflight.js (this script again - everything must be ok)`);
console.log(problems ? `\n${problems} thing(s) are missing before the release can go out` : '\neverything is in place');
process.exit(problems ? 1 : 0);
