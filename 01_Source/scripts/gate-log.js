'use strict';
// Run the gate with a clean build cache.
//
// The gate writes its own evidence log (`02_Verification/run-all-2026-11-04.log`, see tests/run-all.js): it is
// written while the gate runs, so the claims checks inside the gate read the round that is running (D-134).
//
// The cache is cleaned first because the red-green step leaves the BROKEN binary in the shared target directory:
// the restore puts the backup's mtime back on the source, the artefact built from the broken source is the newer
// one, and cargo would keep it - so the next round would test the broken code (D-133). Cleaning it takes a minute
// and makes the run trustworthy.
//
// Usage:
//   node scripts/gate-log.js              # the fast set of the gate (it includes the UAT now, D-137)
//   node scripts/gate-log.js --all        # + the UAT with the Rust engine
//   node scripts/gate-log.js --tsf --rust # single steps
//   node scripts/gate-log.js --no-clean   # do not clean the build cache first
const {spawn, spawnSync} = require('node:child_process');
const path = require('node:path');

const root = path.resolve(__dirname, '..', '..');
const src = path.join(root, '01_Source');

const argv = process.argv.slice(2);
let clean = true;
let pass = [];
for (let i = 0; i < argv.length; i++) {
  const a = argv[i];
  if (a === '--no-clean') clean = false;
  else if (a === '--') { pass = argv.slice(i + 1); break; }
  else pass.push(a);
}

const env = process.env;
const say = text => console.log(text);

// ---- the cache the red-green step poisons -------------------------------------------------------------------
if (clean) {
  const pimeCore = spawnSync('cargo', ['clean', '-p', 'pime-core'], {cwd: src, env, encoding: 'utf8', shell: true});
  say(`clean pime-core: ${pimeCore.status === 0 ? 'ok' : 'FAILED (' + (pimeCore.stderr || '').trim() + ')'}`);
  const pimeTsf = spawnSync('cargo', ['clean', '-p', 'pime-tsf'], {cwd: path.join(src, 'crates', 'pime-tsf'), env, encoding: 'utf8', shell: true});
  say(`clean pime-tsf: ${pimeTsf.status === 0 ? 'ok' : 'FAILED (' + (pimeTsf.stderr || '').trim() + ')'}`);
}

// ---- the gate ------------------------------------------------------------------------------------------------
const child = spawn(process.execPath, [path.join(src, 'tests', 'run-all.js'), ...pass], {cwd: root, env, stdio: 'inherit'});
child.on('error', err => { console.error(err); process.exit(2); });
child.on('close', code => {
  console.log(code === 0 ? 'the gate is green' : `the gate is red (exit ${code})`);
  process.exit(code === 0 ? 0 : 1);
});
