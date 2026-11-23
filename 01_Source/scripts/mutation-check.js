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
const { runCargoSuite } = require('./cargo-suite.js');

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
// The environment of the suite is the environment of the developer **minus the switches that would make the suite
// fail for a reason that has nothing to do with the mutation**: the live tests drive the installed text service and
// take the focus of the desktop, so a stray `PIME_IME_LIVE` in the shell that starts the gate would make every case
// fail for an environment reason - the round would be green for the wrong reason.
const RUST_ENV = {
  ...process.env,
  CARGO_TARGET_DIR: TARGET_DIR,
  RUSTUP_TOOLCHAIN: 'stable-x86_64-pc-windows-gnu',
  CARGO_BUILD_JOBS: '1',
  CARGO_PROFILE_DEV_DEBUG: '0',
  CARGO_INCREMENTAL: '0',
};
delete RUST_ENV.PIME_IME_LIVE;
delete RUST_ENV.PIME_AGENT_BRIDGE_HWND;

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
    const env = {...process.env, ...(suite.env || {})};
    // The live switch must not reach the JavaScript suite either: a stray `PIME_IME_LIVE` would make the suite fail
    // for a reason that has nothing to do with the mutation.
    delete env.PIME_IME_LIVE;
    delete env.PIME_AGENT_BRIDGE_HWND;
    const result = spawnSync(process.execPath, ['--test', '--test-reporter=tap', suite.file], {
      cwd: path.join(root, '01_Source', 'tests', 'js'),
      encoding: 'utf8',
      env,
      maxBuffer: 256 * 1024 * 1024,
    });
    return {status: result.status === null ? -1 : result.status, out: `${result.stdout || ''}${result.stderr || ''}`};
  }
  if (suite.kind === 'rust') {
    const cwd = path.join(root, '01_Source', 'crates', suite.crate);
    // The helper survives the scanner of the machine: it restores the write access of a binary the scanner
    // locked and runs the test binaries from a copy when the scanner refuses to execute them (D-138). Without
    // it, every case of a crate whose binary is quarantined ends as "the suite did not run" - the round would
    // say nothing about the code at all.
    const result = runCargoSuite({
      args: ['test', '--quiet'],
      cwd,
      env: {...RUST_ENV, ...(suite.env || {})},
    });
    return {status: result.status, out: result.out, note: result.note};
  }
  throw new Error(`unknown suite kind: ${suite.kind}`);
}

// ---- the run ------------------------------------------------------------------------------------------------
const digest = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');

// The files this run breaks are backed up outside the repository: a run that is killed (the machine runs out of
// memory now and then) must not leave a broken implementation in the working tree. The next run puts the files
// back before it touches anything.
const BACKUP_DIR = path.join(os.tmpdir(), 'ime-mutation-backups');
const backupName = relative => relative.split('/').join('__');

function restoreLeftovers() {
  if (!fs.existsSync(BACKUP_DIR)) return {restored: 0, kept: []};
  let restored = 0;
  const kept = [];
  for (const name of fs.readdirSync(BACKUP_DIR)) {
    if (name.endsWith('.mutated') || name.endsWith('.kept')) continue;
    const backup = path.join(BACKUP_DIR, name);
    const target = path.join(root, name.split('__').join('/'));
    try {
      // Two cases have to be told apart, because they need the opposite decision:
      //
      // * the file on the disk is **the mutation** (the run was killed before it could put the original back) => the
      //   original **must** go back, or the tree keeps a broken implementation and the round runs against it;
      // * the file on the disk is something else (the developer is working in the tree) => their version is the newer
      //   one, so it stays; the copy of the run is written aside instead of being copied over it (a restore that quietly
      //   throws away the work of the developer is worse than a broken file).
      //
      // The backup carries **both** versions (see `backup`): without the second one the two cases look the same.
      let original = null;
      let mutated = null;
      try {
        const stored = JSON.parse(fs.readFileSync(backup, 'utf8'));
        original = stored.original;
        mutated = stored.mutated;
      } catch {
        // a backup of an older run (the plain original): the mutation is unknown, so the only safe reading is
        // "the file was changed by somebody else" - their version stays.
        try { original = fs.readFileSync(backup, 'utf8'); } catch { /* unreadable: reported below */ }
      }
      let current = null;
      try { current = fs.readFileSync(target, 'utf8'); } catch { /* the target is gone: the backup is the only copy left */ }
      if (original === null) {
        // the backup cannot be read: leave everything as it is and say so
        kept.push({target, aside: backup});
        continue;
      }
      if (current !== null && mutated !== null && current === mutated) {
        // the run was killed before the restore: the original goes back.
        fs.writeFileSync(target, original, 'utf8');
        fs.unlinkSync(backup);
        touch(target);
        restored++;
        continue;
      }
      if (current === null || current !== original) {
        // the file on the disk is neither the original nor the mutation: the developer is working in the tree and
        // their version stays.
        const aside = `${backup}.kept`;
        fs.renameSync(backup, aside);
        kept.push({target, aside});
        continue;
      }
      fs.writeFileSync(target, original, 'utf8');
      fs.unlinkSync(backup);
      touch(target);
      restored++;
    } catch {
      // a file that cannot be restored is reported by the restore check at the end of the run
    }
  }
  return {restored, kept};
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

function backup(relative, original, mutated) {
  fs.mkdirSync(BACKUP_DIR, {recursive: true});
  // **Both** versions go into one file and it is renamed **once**: two renames would leave a backup without its
  // second version if the run is killed in between, and the next run could not tell the two cases apart
  // (a killed run from a mutation against a file the developer edited). One atomic rename, no window (D-178).
  const name = path.join(BACKUP_DIR, backupName(relative));
  const temporary = `${name}.tmp`;
  fs.writeFileSync(temporary, JSON.stringify({original: original.toString('utf8'), mutated: mutated.toString('utf8')}));
  fs.renameSync(temporary, name);
}

function undoBackup(relative) {
  const file = path.join(BACKUP_DIR, backupName(relative));
  if (!fs.existsSync(file)) return;
  try {
    const stored = JSON.parse(fs.readFileSync(file, 'utf8'));
    fs.writeFileSync(path.join(root, relative), stored.original, 'utf8');
  } catch {
    fs.copyFileSync(file, path.join(root, relative));
  }
  fs.unlinkSync(file);
}

// The leftovers of a killed run are put back **before anything else**, even before a listing: a run that only lists
// the cases must not leave a broken implementation in the tree.
const restoredAtStart = restoreLeftovers();
if (restoredAtStart.restored) {
  console.log(`note: ${restoredAtStart.restored} file(s) of a killed run were put back before this run started`);
}
for (const kept of restoredAtStart.kept) {
  console.log(`note: ${kept.target} was changed after a killed run: your version was kept, the copy of the run is in ${kept.aside}`);
}

// A listing is a read of the cases - but it comes **after** the restore above: even a listing must not leave a broken
// implementation in the tree.
if (list) {
  for (const item of cases) console.log(`${item.id}\t${item.what}\t${item.file}\t${item.suite.kind}:${item.suite.crate || item.suite.file}`);
  process.exit(0);
}

// The snapshot of the files this run breaks is taken **after** the leftovers were put back: it has to hold the bytes
// the tree is supposed to have, not the broken ones a killed run left behind. The snapshot taken before the restore
// made every run after a killed one report "these files were not restored" although they were (D-178).
const before = new Map();
for (const item of cases) {
  const file = path.join(root, item.file);
  if (!before.has(file)) before.set(file, {bytes: fs.readFileSync(file), digest: digest(file)});
}

const wanted = only ? only.split(',').map(prefix => prefix.trim()).filter(Boolean) : [];
const selected = wanted.length ? cases.filter(item => wanted.some(prefix => item.id.startsWith(prefix))) : cases;
// A prefix that matches **no** case is a mistake, not a round that checks nothing: the report must not be overwritten
// with an empty round and the exit code must not be 0 (a round that checked nothing is not a green round).
if (selected.length === 0) {
  console.error(`no case matches ${wanted.join(', ')}: nothing was checked`);
  process.exit(5);
}
if (!args.includes('--append')) {
  // The header names the **selected** count: a partial round must not look like a whole one in the file alone (the
  // note used to go to the console only, so a reader of the committed report could not tell).
  const scope = selected.length === cases.length
    ? `all ${cases.length} mutations of the implementation were checked in this round`
    : `${selected.length} of the ${cases.length} mutations of the implementation were checked in this round`;
  fs.writeFileSync(REPORT, `# The red-green check of ${new Date().toISOString()}\n# ${scope}; every one of them is restored right after it is applied.\n`, 'utf8');
}
if (wanted.length && selected.length !== cases.length) {
  const note = `note: ${selected.length} of the ${cases.length} cases are selected`;
  console.log(note);
  fs.appendFileSync(REPORT, `# ${note}\n`, 'utf8');
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
// The files that somebody else changed while a case was running: they are not damage, they are the developer's work.
const changedElsewhere = [];

// The anchor of a case is written with LF in the cases file, but the files in the repository do not all use the
// same line endings (the working tree has CRLF in some of them) => a plain comparison never matches and the case
// ends as "stale", which means it verifies nothing. The pattern therefore accepts both line endings, and the
// replacement is written with the line endings of the file it goes into.
function anchorPattern(text) {
  const escaped = text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&').replace(/\r?\n/g, '\\r?\\n');
  return new RegExp(escaped);
}

for (const item of selected) {
  const file = path.join(root, item.file);
  const original = fs.readFileSync(file, 'utf8');
  const pattern = anchorPattern(item.from);
  if (!pattern.test(original)) {
    write(`STALE  ${item.id} ${anchor(item)}${item.what}: the text to break is not in the file any more`);
    noteAnchor(item, 'stale');
    stale++;
    stales.push(item.id);
    continue;
  }
  const endings = original.includes('\r\n') ? '\r\n' : '\n';
  const replacement = item.to.replace(/\r?\n/g, endings);
  const mutated = original.replace(pattern, () => replacement);
  if (mutated === original) {
    write(`STALE  ${item.id} ${anchor(item)}${item.what}: the replacement changed nothing`);
    noteAnchor(item, 'stale');
    stale++;
    stales.push(item.id);
    continue;
  }
  // The backup comes **before** the mutation: a run that is killed between the two would leave a broken file in the
  // tree **and no backup at all**, and the next run would take the broken bytes for the original, report that every
  // file was restored and end green with a broken implementation in the tree (D-178).
  backup(item.file, Buffer.from(original, 'utf8'), Buffer.from(mutated, 'utf8'));
  fs.writeFileSync(file, mutated, 'utf8');
  let status = 0;
  let out = '';
  let note = '';
  try {
    const run = runSuite(item.suite);
    status = run.status;
    out = run.out;
    note = run.note || '';
  } catch (error) {
    status = -1;
    out = String(error.message || error);
  } finally {
    // The original file comes back whatever happened - **unless** somebody else changed it while the case was running
    // (the developer is editing the tree): restoring then would throw their work away. Their version stays, the backup
    // is dropped (it must not come back at the next run either) and the round says so.
    const current = fs.readFileSync(file, 'utf8');
    if (current === mutated) {
      fs.writeFileSync(file, original, 'utf8');
      undoBackup(item.file);
      touch(file); // the artefact of the broken source is the newer one: cargo has to rebuild it (D-133)
    } else {
      const leftover = path.join(BACKUP_DIR, backupName(item.file));
      if (fs.existsSync(leftover)) fs.unlinkSync(leftover);
      changedElsewhere.push(file);
      write(`note: ${item.file} was changed by somebody else while the case ran: it was left as it is`);
    }
  }
  // The suite has to **fail on an assertion**, not merely to stop: a build that does not compile, a test binary
  // that crashes or a cargo that cannot get the lock are all "the suite did not run", not "the suite noticed".
  //
  // A test binary that could **not be executed** is the same thing and must not be counted as a catch: the security
  // scanner of the machine quarantines a freshly linked binary (D-138) and cargo prints "error: test failed" for it
  // too ⇒ the round would be green for the wrong reason (the suite "noticed" the break because the test could not
  // run at all).
  const blockedBinary = /could not execute process|never executed|os error 5/i.test(out);
  const failedAssertion =
    !blockedBinary &&
    (/test result: FAILED/.test(out) ||
      /^failures:/m.test(out) ||
      /^not ok \d+/m.test(out) ||
      /^# fail [1-9]/m.test(out) ||
      /error: test failed/.test(out));
  if (status === 0) {
    missed++;
    misses.push(item.id);
    write(`MISSED ${item.id} ${anchor(item)}${item.what}  <-- the suite does not notice this`);
    noteAnchor(item, 'missed');
  } else if (failedAssertion) {
    caught++;
    write(`CAUGHT ${item.id} ${anchor(item)}${item.what}`);
    if (note) write(`note: ${item.id}: ${note}`);
    noteAnchor(item, 'caught');
  } else {
    errors.push(item.id);
    // the reason matters: "the suite did not run" is what the report says, but the reader has to know whether the
    // machine was the reason (a full disk, a cargo that is not on the PATH, the scanner that holds a fresh test
    // binary - D-138) or the code (D-135, D-147)
    const diskFull = /no space left on device|os error 112|disk full/i.test(out);
    const noCargo = /is not recognized|not found/i.test(out) && /cargo/i.test(out);
    const scanner = blockedBinary;
    const cause = scanner
      ? 'the scanner of the machine held the test binary (D-138): run the target by hand before the gate'
      : diskFull
        ? 'the disk was full'
        : noCargo
          ? 'cargo was not on the PATH'
          : '';
    write(`ERROR ${item.id} ${anchor(item)}${item.what}  <-- the suite did not run (exit ${status}): the result is unknown${cause ? ' (' + cause + ')' : ''}`);
    if (note) write(`note: ${item.id}: ${note}`);
    noteAnchor(item, 'unknown');
    const lines = out.split(/\r?\n/).filter(line => /error|warning: unused|no space left|os error 1\d\d|is not recognized/i.test(line)).slice(0, 3);
    for (const line of lines) console.log(`         ${line}`);
  }
}

// ---- the restore is verified, not trusted ---------------------------------------------------------------------
const damaged = [];
for (const [file, state] of before) {
  // A file that somebody else changed while a case was running is not damage: it is the developer's work.
  if (changedElsewhere.includes(file)) continue;
  if (digest(file) !== state.digest) damaged.push(path.relative(root, file));
}
write('');
write(`the run checked ${caught + missed + errors.length + stale} mutations: ${caught} caught, ${missed} missed, ${errors.length} unknown (the suite did not run), ${stale} stale`);
// Every failure is written down and the exit code is the most severe one. The code used to leave at the first
// failure, so a round with a missed **and** a stale case lost the evidence of the stale one (D-178).
//
// The exit codes (every one of them means "the round is red", and the report says which one it was):
//   1  the suite does not notice a mutation (a hole in the test suite)
//   2  a file was not restored (the tree is not the one the round found)
//   3  the result of a mutation is unknown (the suite did not run: the machine, not the code)
//   4  a mutation is stale (the text to break is not in the file any more: it verified nothing - D-111)
// The order below is the order of severity: a damaged tree first (nothing else can be believed), then the unknown
// results, then the missed ones, then the stale ones.
let exitCode = 0;
if (damaged.length) {
  write(`FAIL: these files were not restored: ${damaged.join(', ')}`);
  exitCode = 2;
}
if (errors.length) {
  write(`the result of ${errors.length} mutation(s) is unknown: ${errors.join(', ')}`);
  if (!exitCode) exitCode = 3;
}
if (missed) {
  write(`the suite does not notice ${missed} mutation(s): ${misses.join(', ')}`);
  if (!exitCode) exitCode = 1;
}
if (stale) {
  write(`the suite did not verify ${stale} mutation(s): the text to break is not in the file any more (${stales.join(', ')})`);
  write('a stale case is a failure: a case that is written wrong or whose anchor is gone has verified nothing (D-111)');
  if (!exitCode) exitCode = 4;
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
if (exitCode) {
  write('');
  write(`the round is RED (exit ${exitCode})`);
  process.exit(exitCode);
}
write('every mutation was caught: the suite really pins these behaviours');
