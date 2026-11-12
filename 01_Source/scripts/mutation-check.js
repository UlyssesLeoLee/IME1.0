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
      restored++;
    } catch {
      // a file that cannot be restored is reported by the restore check at the end of the run
    }
  }
  return restored;
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

let caught = 0;
let missed = 0;
let stale = 0;
const misses = [];
const stales = [];
const errors = [];

for (const item of selected) {
  const file = path.join(root, item.file);
  const original = fs.readFileSync(file, 'utf8');
  if (!original.includes(item.from)) {
    write(`STALE  ${item.id} ${item.what}: the text to break is not in the file any more`);
    stale++;
    stales.push(item.id);
    continue;
  }
  const mutated = original.replace(item.from, item.to);
  if (mutated === original) {
    write(`STALE  ${item.id} ${item.what}: the replacement changed nothing`);
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
    write(`MISSED ${item.id} ${item.what}  <-- the suite does not notice this`);
  } else if (failedAssertion) {
    caught++;
    write(`CAUGHT ${item.id} ${item.what}`);
  } else {
    errors.push(item.id);
    write(`ERROR ${item.id} ${item.what}  <-- the suite did not run (exit ${status}): the result is unknown`);
    const lines = out.split(/\r?\n/).filter(line => /error|warning: unused/.test(line)).slice(0, 3);
    for (const line of lines) console.log(`         ${line}`);
  }
}

// ---- the restore is verified, not trusted ---------------------------------------------------------------------
const damaged = [];
for (const [file, state] of before) {
  if (digest(file) !== state.digest) damaged.push(path.relative(root, file));
}
write('');
write(`the run checked ${caught + missed + errors} mutations: ${caught} caught, ${missed} missed, ${errors.length} unknown (the suite did not run), ${stale} stale`);
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
write('every mutation was caught: the suite really pins these behaviours');
