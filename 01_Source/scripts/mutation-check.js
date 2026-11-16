'use strict';
// The red-green check: a test suite that stays green while the implementation is broken proves nothing.
//
// This script breaks the implementation on purpose, one place at a time, and asks the matching suite whether
// it notices. A mutation that is CAUGHT means the suite really pins that behaviour; a mutation that is MISSED is a
// hole in the test suite (it is reported, not hidden).
//
//   node scripts/mutation-check.js                 # every case
//   node scripts/mutation-check.js --only nema     # the cases whose id starts with "nema"
//   node scripts/mutation-check.js --list          # only list the cases
//
// The original file is restored after every single mutation, and the run verifies at the end that every file is
// byte for byte the one it found (the restore is not taken on trust).
//
// The cases live in `scripts/mutation-cases.js`: the file, the text that is replaced, the text that replaces it and
// the suite that has to notice.
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const root = path.resolve(__dirname, '..', '..');
const cases = require('./mutation-cases.js');

const args = process.argv.slice(2);
const only = args.includes('--only') ? args[args.indexOf('--only') + 1] : '';
const list = args.includes('--list');
// The report is written line by line: a run that is killed by the machine (this one runs out of virtual memory
// now and then) still leaves the cases it has finished.
const REPORT = path.join(root, '02_Verification', 'mutation-report.txt');
const write = line => {
  fs.appendFileSync(REPORT, `${line}\n`, 'utf8');
  console.log(line);
};

// ---- the suites that have to notice -------------------------------------------------------------------------
const TARGET_DIR = process.env.CARGO_TARGET_DIR || 'C:\\cargo-target-ime';
const RUST_ENV = {
  ...process.env,
  CARGO_TARGET_DIR: TARGET_DIR,
  RUSTUP_TOOLCHAIN: 'stable-x86_64-pc-windows-gnu',
  CARGO_BUILD_JOBS: '1',
  CARGO_PROFILE_DEV_DEBUG: '0',
  CARGO_INCREMENTAL: '0',
};

function runSuite(suite) {
  // A run that is killed by the machine (the file of the DLL is locked by the scanner, the cargo cannot get the
  // lock, the memory is gone) is not a result: it is retried, like the gate does with its own cargo runs.
  let last = {status: 0, out: ''};
  for (let attempt = 1; attempt <= 3; attempt++) {
    last = runSuiteOnce(suite);
    const broke = last.status !== 0 && !/test result: FAILED/.test(last.out) && !/^not ok \d+/m.test(last.out);
    if (!broke) return last;
    if (attempt < 3) {
      const wait = attempt * 2000;
      const until = Date.now() + wait;
      while (Date.now() < until) {} // the file lock needs a moment
    }
  }
  return last;
}

function runSuiteOnce(suite) {
  if (suite.kind === 'js') {
    const file = path.join(root, '01_Source', 'tests', 'js', suite.file);
    const result = spawnSync(process.execPath, ['--test', '--test-reporter=tap', suite.file], {
      cwd: path.join(root, '01_Source', 'tests', 'js'),
      encoding: 'utf8',
      env: {...process.env, ...(suite.env || {})},
      maxBuffer: 256 * 1024 * 1024,
    });
    return {status: result.status === null ? -1 : result.status, out: `${result.stdout || ''}${result.stderr || ''}`};
  }
  if (suite.kind === 'rust') {
    const cwd = path.join(root, '01_Source', 'crates', suite.crate);
    const result = spawnSync('cargo', ['test', '--quiet'], {
      cwd,
      encoding: 'utf8',
      env: {...RUST_ENV, ...(suite.env || {})},
      shell: process.platform === 'win32',
      maxBuffer: 256 * 1024 * 1024,
    });
    return {status: result.status === null ? -1 : result.status, out: `${result.stdout || ''}${result.stderr || ''}`};
  }
  throw new Error(`unknown suite kind: ${suite.kind}`);
}

// ---- the run ------------------------------------------------------------------------------------------------
const digest = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const before = new Map();
for (const item of cases) {
  const file = path.join(root, item.file);
  if (!before.has(file)) before.set(file, {bytes: fs.readFileSync(file), digest: digest(file)});
}

if (list) {
  for (const item of cases) console.log(`${item.id}\t${item.what}\t${item.file}\t${item.suite.kind}:${item.suite.crate || item.suite.file}`);
  process.exit(0);
}

// The files this run breaks are backed up outside the repository: a run that is killed (the machine runs out of
// memory now and then) must not leave a broken implementation in the working tree. The next run puts the files
// back before it touches anything.
const BACKUP_DIR = path.join(os.tmpdir(), 'ime-mutation-backups');
const backupName = relative => relative.split('/').join('__');

function restoreLeftovers() {
  if (!fs.existsSync(BACKUP_DIR)) return 0;
  let restored = 0;
  for (const name of fs.readdirSync(BACKUP_DIR)) {
    const backup = path.join(BACKUP_DIR, name);
    const target = path.join(root, name.split('__').join('/'));
    try {
      fs.copyFileSync(backup, target);
      fs.unlinkSync(backup);
      touch(target);
      restored++;
    } catch {
      // a file that cannot be restored is reported by the restore check at the end of the run
    }
  }
  return restored;
}

// The mtime of a restored file has to move FORWARD.
//
// Cargo decides by the mtime whether an artefact is out of date, and the artefact that was built from the broken
// source is the newer one: if the restore leaves the old timestamp, cargo keeps the broken binary and the next run
// tests the broken code (D-133). Writing the bytes back is not enough - the timestamp has to follow.
function touch(file) {
  const now = new Date();
  try { fs.utimesSync(file, now, now); } catch { /* a file that cannot be touched is reported by the restore check */ }
}

function backup(relative, bytes) {
  fs.mkdirSync(BACKUP_DIR, {recursive: true});
  fs.writeFileSync(path.join(BACKUP_DIR, backupName(relative)), bytes);
}

function undoBackup(relative) {
  const file = path.join(BACKUP_DIR, backupName(relative));
  if (!fs.existsSync(file)) return;
  fs.copyFileSync(file, path.join(root, relative));
  fs.unlinkSync(file);
}

const restoredAtStart = restoreLeftovers();
if (restoredAtStart) {
  console.log(`note: ${restoredAtStart} file(s) of a killed run were put back before this run started`);
}
if (!args.includes('--append')) {
  fs.writeFileSync(REPORT, `# The red-green check of ${new Date().toISOString()}\n# ${cases.length} mutations of the implementation; every one of them is restored right after it is applied.\n\n`, 'utf8');
}

const wanted = only ? only.split(',').map(prefix => prefix.trim()).filter(Boolean) : [];
const selected = wanted.length ? cases.filter(item => wanted.some(prefix => item.id.startsWith(prefix))) : cases;
if (wanted.length && selected.length !== cases.length) {
  console.log(`note: ${selected.length} of the ${cases.length} cases are selected`);
}

// Every mutation rebuilds a crate, so the run needs room on the disk of the target directory. When the disk is
// (nearly) full the builds fail and every case would end as "the suite did not run": say it before the round
// instead of after it (D-135).
try {
  const drive = path.parse(process.env.CARGO_TARGET_DIR || path.join(root, 'target')).root;
  const free = fs.statfsSync ? fs.statfsSync(drive).bavail * fs.statfsSync(drive).bsize : null;
  if (free !== null && free < 2 * 1024 * 1024 * 1024) {
    console.log(`note: only ${(free / 1024 / 1024 / 1024).toFixed(1)} GB are free on ${drive}: the mutations rebuild crates, and a full disk makes every case end as "the suite did not run"`);
  }
} catch { /* the disk space cannot be read: the round simply runs */ }

let caught = 0;

// The design entry a case protects: a requirement of the SRS, a case of the test design, or the ruling of the user
// when the feature is not in the design documents at all. The report quotes it, so the evidence of the round is
// about requirements and not about the lines of code it happens to break.
const anchor = item => (item.req ? '[' + item.req + '] ' : '');
// The cases per design entry: what the round says about each requirement.
const byAnchor = new Map();
const noteAnchor = (item, kind) => {
  const key = item.req || '(no design entry)';
  if (!byAnchor.has(key)) byAnchor.set(key, {caught: 0, missed: 0, unknown: 0, stale: 0, ids: []});
  const entry = byAnchor.get(key);
  entry[kind]++;
  entry.ids.push(item.id);
};

let missed = 0;
let stale = 0;
const misses = [];
const stales = [];
const errors = [];

for (const item of selected) {
  const file = path.join(root, item.file);
  const original = fs.readFileSync(file, 'utf8');
  if (!original.includes(item.from)) {
    write(`STALE  ${item.id} ${anchor(item)}${item.what}: the text to break is not in the file any more`);
    noteAnchor(item, 'stale');
    stale++;
    stales.push(item.id);
    continue;
  }
  const mutated = original.replace(item.from, item.to);
  if (mutated === original) {
    write(`STALE  ${item.id} ${anchor(item)}${item.what}: the replacement changed nothing`);
    noteAnchor(item, 'stale');
    stale++;
    stales.push(item.id);
    continue;
  }
  fs.writeFileSync(file, mutated, 'utf8');
  backup(item.file, Buffer.from(original, 'utf8'));
  let status = 0;
  let out = '';
  try {
    const run = runSuite(item.suite);
    status = run.status;
    out = run.out;
  } catch (error) {
    status = -1;
    out = String(error.message || error);
  } finally {
    // the original file comes back whatever happened
    fs.writeFileSync(file, original, 'utf8');
    undoBackup(item.file);
    touch(file); // the artefact of the broken source is the newer one: cargo has to rebuild it (D-133)
  }
  // The suite has to **fail on an assertion**, not merely to stop: a build that does not compile, a test binary
  // that crashes or a cargo that cannot get the lock are all "the suite did not run", not "the suite noticed".
  const failedAssertion =
    /test result: FAILED/.test(out) ||
    /^failures:/m.test(out) ||
    /^not ok \d+/m.test(out) ||
    /^# fail [1-9]/m.test(out) ||
    /error: test failed/.test(out);
  if (status === 0) {
    missed++;
    misses.push(item.id);
    write(`MISSED ${item.id} ${anchor(item)}${item.what}  <-- the suite does not notice this`);
    noteAnchor(item, 'missed');
  } else if (failedAssertion) {
    caught++;
    write(`CAUGHT ${item.id} ${anchor(item)}${item.what}`);
    noteAnchor(item, 'caught');
  } else {
    errors.push(item.id);
    // the reason matters: "the suite did not run" is what the report says, but the reader has to know whether the
    // machine was the reason (a full disk, a cargo that is not on the PATH) or the code (D-135, D-147)
    const diskFull = /no space left on device|os error 112|disk full/i.test(out);
    const noCargo = /is not recognized|not found/i.test(out) && /cargo/i.test(out);
    const cause = diskFull ? 'the disk was full' : noCargo ? 'cargo was not on the PATH' : '';
    write(`ERROR ${item.id} ${anchor(item)}${item.what}  <-- the suite did not run (exit ${status}): the result is unknown${cause ? ' (' + cause + ')' : ''}`);
    noteAnchor(item, 'unknown');
    const lines = out.split(/\r?\n/).filter(line => /error|warning: unused|no space left|os error 1\d\d|is not recognized/i.test(line)).slice(0, 3);
    for (const line of lines) console.log(`         ${line}`);
  }
}

// ---- the restore is verified, not trusted ---------------------------------------------------------------------
const damaged = [];
for (const [file, state] of before) {
  if (digest(file) !== state.digest) damaged.push(path.relative(root, file));
}
write('');
write(`the run checked ${caught + missed + errors.length} mutations: ${caught} caught, ${missed} missed, ${errors.length} unknown (the suite did not run), ${stale} stale`);
if (damaged.length) {
  write(`FAIL: these files were not restored: ${damaged.join(', ')}`);
  process.exit(2);
}
if (errors.length) {
  write(`the result of ${errors.length} mutation(s) is unknown: ${errors.join(', ')}`);
  process.exit(3);
}
if (missed) {
  write(`the suite does not notice ${missed} mutation(s): ${misses.join(', ')}`);
  process.exit(1);
}
write('');
write('by the design entry (what the round says about each requirement):');
for (const [key, entry] of [...byAnchor.entries()].sort((a, b) => a[0].localeCompare(b[0]))) {
  const parts = [];
  if (entry.caught) parts.push(entry.caught + ' caught');
  if (entry.missed) parts.push(entry.missed + ' missed');
  if (entry.unknown) parts.push(entry.unknown + ' unknown');
  if (entry.stale) parts.push(entry.stale + ' stale');
  write(`  ${key}: ${parts.join(', ')}`);
}
write('every mutation was caught: the suite really pins these behaviours');
