'use strict';
// One entry point for every stored test script. It only RUNS them: fixtures, golden data, certificates and reports are
// never regenerated unless you ask for it explicitly (gen-fixtures.js --write, --report).
//
//   node tests/run-all.js                 fast, no GUI: --syntax --fixtures --golden --js --rust --smoke --tsf
//   node tests/run-all.js --all           the above + --uat + --uat-rust
//   node tests/run-all.js --tsf           build crates/pime-tsf (GNU target) and inspect the DLL (x64, exports, imports); never registers it
//   node tests/run-all.js --js            only the JavaScript unit/integration suite (add PIME_CORE_EXE to also run the engine differential tests)
//   node tests/run-all.js --rust          cargo test --workspace (builds the debug core exe, which --smoke and --uat-rust reuse)
//   node tests/run-all.js --uat           Playwright on the real VS Code, JavaScript engine
//   node tests/run-all.js --uat-rust      Playwright with programmingIme.engine=rust (needs the built core; evidence goes to a temp folder)
//   node tests/run-all.js --report        rewrite 02_Verification/test-execution-report.* from the last results (explicit, never implicit)
//   options: --only-uat "UAT-23"          forwarded to playwright as -g
// Exit code 0 only when every selected step passed.
const {spawnSync} = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const tests = __dirname, src = path.resolve(tests, '..'), root = path.resolve(src, '..');
const args = process.argv.slice(2);
const has = f => args.includes(f);
const all = has('--all');
const fast = !args.some(a => /^--(syntax|fixtures|golden|js|rust|smoke|tsf|uat|uat-rust|report)$/.test(a)) && !all;
// no flags = the fast set (no GUI); --all adds the two UAT runs; single flags select single steps
const FAST = ['syntax', 'fixtures', 'golden', 'rust', 'js', 'smoke', 'tsf'];
const want = k => all || (fast && FAST.includes(k)) || has(`--${k}`);
const onlyUat = (() => { const i = args.indexOf('--only-uat'); return i >= 0 ? args[i + 1] : null; })();
const targetDir = process.env.CARGO_TARGET_DIR || path.join(os.tmpdir(), 'pime-cargo-target');
const coreExe = process.env.PIME_CORE_EXE || path.join(targetDir, 'debug', 'pime-core.exe');
const isWin = process.platform === 'win32';
const npx = isWin ? 'npx.cmd' : 'npx';
const results = [];

function step(name, fn) {
  const t0 = Date.now();
  console.log(`\n=== ${name} ===`);
  let ok = false, note = '';
  try { const r = fn(); ok = r.ok; note = r.note || ''; } catch (e) { note = String(e.message || e); }
  results.push({name, ok, note, seconds: ((Date.now() - t0) / 1000).toFixed(1)});
  console.log(`--- ${name}: ${ok ? 'PASS' : 'FAIL'} ${note}`);
}
const sh = (cmd, cmdArgs, opts = {}) => {
  const r = spawnSync(cmd, cmdArgs, {stdio: 'inherit', shell: isWin && /\.cmd$/i.test(cmd), ...opts});
  return {ok: r.status === 0, note: r.status === 0 ? '' : `exit ${r.status}`};
};

if (want('syntax')) step('syntax check (extension sources + all test scripts)', () => sh(process.execPath, [path.join(src, 'editor-bridges', 'vscode', 'scripts', 'check-syntax.js')]));
if (want('fixtures')) step('golden fixtures are unchanged (compare only)', () => sh(process.execPath, [path.join(tests, 'gen-fixtures.js')]));
if (want('golden')) step('golden-v0.2 cases are still present in the live fixtures', () => sh(process.execPath, [path.join(src, 'scripts', 'glue-check.js'), '--golden']));
if (want('rust')) {
  step('Rust: cargo test --workspace', () => {
    for (let attempt = 1; attempt <= 3; attempt++) { // a freshly linked exe can fail once while Defender scans it
      const r = spawnSync('cargo', ['test', '--workspace'], {cwd: src, encoding: 'utf8', env: {...process.env, CARGO_TARGET_DIR: targetDir}});
      process.stdout.write(r.stdout || ''); process.stderr.write(r.stderr || '');
      if (r.status === 0) return {ok: true, note: ((r.stdout || '').match(/test result: ok\. (\d+) passed/g) || []).map(x => Number(x.match(/(\d+)/)[1])).reduce((a, b) => a + b, 0) + ' tests'};
      if (!/os error 5/.test((r.stdout || '') + (r.stderr || ''))) break;
    }
    return {ok: false, note: 'cargo test failed'};
  });
} else if ((want('smoke') || want('uat-rust')) && !fs.existsSync(coreExe)) {
  step('Rust: cargo build (the core exe is needed by the selected steps)', () => sh('cargo', ['build'], {cwd: src, env: {...process.env, CARGO_TARGET_DIR: targetDir}}));
}
// with a built core the JS suite also runs its engine differential tests (they skip without it)
if (want('js')) step('JavaScript tests' + (fs.existsSync(coreExe) ? ' (+ engine differential tests, core exe found)' : ''), () =>
  sh(process.execPath, ['--test', '*.test.js'], {cwd: path.join(tests, 'js'), env: {...process.env, ...(fs.existsSync(coreExe) ? {PIME_CORE_EXE: coreExe} : {})}}));
if (want('smoke')) step('real pime-core.exe smoke test (JSON Lines)', () => sh(process.execPath, [path.join(tests, 'smoke', 'core-binary-smoke.js')], {env: {...process.env, PIME_CORE_EXE: coreExe}}));
if (want('tsf')) step('TSF DLL (crates/pime-tsf): builds on the GNU target, x64 PE, only system DLL imports (not registered or loaded)', () => sh(process.execPath, [path.join(tests, 'smoke', 'tsf-dll-check.js')], {env: {...process.env, CARGO_TARGET_DIR: targetDir}}));
const uat = engine => {
  const env = {...process.env};
  const uatArgs = ['playwright', 'test', ...(onlyUat ? ['-g', onlyUat] : [])];
  if (engine === 'rust') {
    env.PIME_ENGINE = 'rust'; env.PIME_CORE_EXE = coreExe;
    env.UAT_EVIDENCE_DIR = env.UAT_EVIDENCE_DIR || fs.mkdtempSync(path.join(os.tmpdir(), 'pime-uat-rust-'));
  }
  return sh(npx, uatArgs, {cwd: path.join(tests, 'uat'), env});
};
if (want('uat')) step('UAT on real VS Code (JavaScript engine)', () => uat('js'));
if (want('uat-rust')) step('UAT on real VS Code (Rust engine)', () => uat('rust'));
if (has('--report')) step('rewrite the traceability report (explicit)', () => sh(process.execPath, [path.join(src, 'scripts', 'traceability-report.js')]));

console.log('\n==== summary ====');
for (const r of results) console.log(`${r.ok ? 'PASS' : 'FAIL'}  ${r.name}  (${r.seconds}s) ${r.note}`);
process.exit(results.every(r => r.ok) ? 0 : 1);
