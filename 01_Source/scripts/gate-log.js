'use strict';
// Regenerate the evidence log of the gate: `02_Verification/run-all-2026-11-04.log`.
//
// Why this script exists. The log is a committed artefact - the three claims checks read it, and the delivery
// quotes the numbers in it - but nothing in the repository could produce it. And the obvious way to produce it
// (run the gate, then write the log) is what D-134 is about: the log is only on disk AFTER the run has finished,
// so the checks inside the gate read the log of the PREVIOUS round. A log that ever carried a failing step
// could therefore never be green again.
//
// This script writes the log WHILE the gate runs: the file is truncated at the start and filled as the steps
// print, so the checks inside the gate read the round that is running - its numbers, and no failing step yet.
// The gate script itself is not touched; that decision (whether to move this into tests/run-all.js) is the
// user's, see D-134.
//
// Usage:
//   node scripts/gate-log.js                     # the fast set of the gate (the 12 steps of its FAST array)
//   node scripts/gate-log.js --all               # + the two UAT runs
//   node scripts/gate-log.js --tsf --rust        # single steps
//   node scripts/gate-log.js --no-clean          # do not clean the build cache first (see below)
//   node scripts/gate-log.js --log <path>        # write the log somewhere else
//
// The build cache is cleaned before the run by default, because the red-green step leaves the BROKEN binary in
// the shared target directory: the restore puts the backup's mtime back on the source, the artefact built from
// the broken source is the newer one, so cargo does not rebuild it and the next run would test the broken code
// (D-133). pime-tsf has its own workspace, so it is cleaned in its own crate directory.
//
// The cargo environment (the GNU toolchain first on the PATH) has to be set by the caller; see
// `01_Source/docs/IME_TEST_MATRIX.md` and `01_Source/docs/TASE_CRATES.md` for the one-liner.
const {spawn, spawnSync} = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..', '..');
const src = path.join(root, '01_Source');
const defaultLog = path.join(root, '02_Verification', 'run-all-2026-11-04.log');

const argv = process.argv.slice(2);
let clean = true;
let logPath = defaultLog;
let pass = [];
for (let i = 0; i < argv.length; i++) {
  const a = argv[i];
  if (a === '--no-clean') clean = false;
  else if (a === '--log') { logPath = path.resolve(argv[++i]); }
  else if (a === '--') { pass = argv.slice(i + 1); break; }
  else pass.push(a);
}

const env = process.env;
const say = text => console.log(text);

// ---- the cache the red-green step poisons ------------------------------------------------------------------
if (clean) {
  const pimeCore = spawnSync('cargo', ['clean', '-p', 'pime-core'], {cwd: src, env, encoding: 'utf8', shell: true});
  say(`clean pime-core: ${pimeCore.status === 0 ? 'ok' : 'FAILED (' + (pimeCore.stderr || '').trim() + ')'}`);
  const pimeTsf = spawnSync('cargo', ['clean', '-p', 'pime-tsf'], {cwd: path.join(src, 'crates', 'pime-tsf'), env, encoding: 'utf8', shell: true});
  say(`clean pime-tsf: ${pimeTsf.status === 0 ? 'ok' : 'FAILED (' + (pimeTsf.stderr || '').trim() + ')'}`);
}

// ---- the gate, with the log written while it runs -------------------------------------------------------------
fs.mkdirSync(path.dirname(logPath), {recursive: true});
const fd = fs.openSync(logPath, 'w'); // truncated now: the checks inside the gate have to read THIS round
const child = spawn(process.execPath, [path.join(src, 'tests', 'run-all.js'), ...pass], {
  cwd: root,
  env,
  stdio: ['ignore', 'pipe', 'pipe'],
});
let stderr = '';
child.stdout.on('data', chunk => { fs.writeSync(fd, chunk); process.stdout.write(chunk); });
child.stderr.on('data', chunk => { stderr += chunk.toString(); });
child.on('error', err => { console.error(err); fs.closeSync(fd); process.exit(2); });
child.on('close', code => {
  fs.closeSync(fd);
  // the cargo chatter goes in where it always went: right before the first step header
  const text = fs.readFileSync(logPath, 'utf8');
  const marker = text.indexOf('\n--- ');
  const merged = marker >= 0 ? text.slice(0, marker) + '\n' + stderr.replace(/\n$/, '') + text.slice(marker) : text + stderr;
  fs.writeFileSync(logPath, merged, 'utf8');
  console.log(`the log is ${Buffer.byteLength(merged, 'utf8')} bytes -> ${logPath}`);
  console.log(code === 0 ? 'the gate is green' : `the gate is red (exit ${code})`);
  process.exit(code === 0 ? 0 : 1);
});
