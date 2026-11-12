'use strict';
// One entry point for every stored test script. It only RUNS them: fixtures, golden data, certificates and reports are
// never regenerated unless you ask for it explicitly (gen-fixtures.js --write, --report).
//
//   node tests/run-all.js                 fast, no GUI: --syntax --fixtures --golden --js --rust --smoke --tsf --installer
//   node tests/run-all.js --all           the above + --uat + --uat-rust
//   node tests/run-all.js --tsf           crates/pime-tsf: its unit tests, then build the DLL (GNU target) and inspect it (x64, exports, imports); never registers it
//   node tests/run-all.js --agent         crates/pime-agent: the front-ends the AI clients call (the MCP server, the CLI, the local server)
//   node tests/run-all.js --tsf-live      crates/pime-tsf: the live host test (the real msctf, a real context; it opens the candidate window on the desktop)
//   node tests/run-all.js --tsf-ui         crates/pime-tsf: the native windows (the probe opens the candidate window, the floating bar and the settings window; a picture of the desktop is taken and checked)
//   node tests/run-all.js --worker        crates/pime-ai-worker: unit tests + the smoke test against the HTTPS mock (needs OpenSSL; writes PIME-TEST session credentials and removes them)
//   node tests/run-all.js --ipc           crates/pime-ipc: unit + loopback tests on real named pipes (unique test pipe names)
//   node tests/run-all.js --setup         crates/pime-setup: unit + temp-folder integration tests (never touches Program Files, HKLM or regsvr32)
//   node tests/run-all.js --installer     the numbers quoted in 02_Verification/IME_FRAMEWORK_MATRIX.md against the built installer and the logs it cites, the UAT numbers against the Playwright report in 02_Verification/uat, and the build log / the sidecar / the prose against the same artifact
//   node tests/run-all.js --js            only the JavaScript unit/integration suite (add PIME_CORE_EXE to also run the engine differential tests)
//   node tests/run-all.js --rust          cargo test --workspace (builds the debug core exe, which --smoke and --uat-rust reuse)
//   node tests/run-all.js --uat           Playwright on the real VS Code, JavaScript engine
//   node tests/run-all.js --uat-rust      Playwright with programmingIme.engine=rust (needs the built core; evidence goes to a temp folder)
//   node tests/run-all.js --report        rewrite 02_Verification/test-execution-report.* from the last results (explicit, never implicit)
//   options: --only-uat "UAT-23"          forwarded to playwright as -g
// Exit code 0 only when every selected step passed.
const {spawnSync, spawn} = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const tests = __dirname, src = path.resolve(tests, '..'), root = path.resolve(src, '..');
const verification = path.join(root, '02_Verification');
const args = process.argv.slice(2);
const has = f => args.includes(f);
const all = has('--all');
const fast = !args.some(a => /^--(syntax|fixtures|golden|js|rust|smoke|tsf|tsf-live|tsf-ui|worker|ipc|setup|agent|tase|bench|mutation|installer|uat|uat-rust|report)$/.test(a)) && !all;
// no flags = the fast set (no GUI); --all adds the two UAT runs; single flags select single steps
const FAST = ['syntax', 'fixtures', 'golden', 'rust', 'js', 'smoke', 'tsf', 'agent', 'tase', 'bench', 'mutation', 'installer'];
const want = k => all || (fast && FAST.includes(k)) || has(`--${k}`);
const onlyUat = (() => { const i = args.indexOf('--only-uat'); return i >= 0 ? args[i + 1] : null; })();
const targetDir = process.env.CARGO_TARGET_DIR || path.join(os.tmpdir(), 'pime-cargo-target');
const coreExe = process.env.PIME_CORE_EXE || path.join(targetDir, 'debug', 'pime-core.exe');
const isWin = process.platform === 'win32';
const npx = isWin ? 'npx.cmd' : 'npx';
const results = [];

// The raw output of a test run, kept as evidence: the traceability report maps the design case ids by the ids
// in the test titles, and it reads these files. Without them it would map against a stale run.
function saveEvidence(name, text) {
  try {
    fs.writeFileSync(path.join(verification, `${name}-test-result.txt`), text.replace(/\r\n/g, '\n'), 'utf8');
  } catch (e) {
    console.log(`note: the evidence file of ${name} was not written: ${e.message}`);
  }
}

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
      saveEvidence('rust', `${r.stdout || ''}${r.stderr || ''}`);
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
// cargo in one of the crates that live outside the main workspace; a freshly linked exe can fail once with os error 5
const cargoRetry = (crate, cargoArgs, extraEnv = {}, evidenceName = '') => {
  for (let attempt = 1; attempt <= 3; attempt++) {
    const r = spawnSync('cargo', cargoArgs, {cwd: path.join(src, 'crates', crate), encoding: 'utf8', env: {...process.env, CARGO_TARGET_DIR: targetDir, ...extraEnv}, shell: isWin});
    process.stdout.write(r.stdout || ''); process.stderr.write(r.stderr || '');
    if (evidenceName) saveEvidence(evidenceName, `${r.stdout || ''}${r.stderr || ''}`);
    if (r.status === 0) return {ok: true, note: ((r.stdout || '').match(/test result: ok\. (\d+) passed/g) || []).map(x => Number(x.match(/(\d+)/)[1])).reduce((a, b) => a + b, 0) + ' tests'};
    if (!/os error 5/.test((r.stdout || '') + (r.stderr || ''))) break;
  }
  return {ok: false, note: `cargo ${cargoArgs[0]} failed in crates/${crate}`};
};
if (want('tsf')) {
  // the raw output is kept as evidence: the traceability report reads it (the TSF crate is outside the main
  // workspace, so `cargo test --workspace` never sees it)
  step('TSF crate (crates/pime-tsf): cargo test (the unit tests, the IME scenario suite and the function ledger)', () => cargoRetry('pime-tsf', ['test'], {}, 'tsf'));
  step('TSF DLL (crates/pime-tsf): builds on the GNU target, x64 PE, only system DLL imports (not registered or loaded)', () => sh(process.execPath, [path.join(tests, 'smoke', 'tsf-dll-check.js')], {env: {...process.env, CARGO_TARGET_DIR: targetDir}}));
}
// The live part drives the real msctf and opens the candidate window of the text service on the desktop: it is
// opt-in (--tsf-live), because a test run must not take over the screen of the user.
if (has('--tsf-live')) {
  step('TSF live host test (the real msctf, a real document context; it opens the candidate window)', () =>
    cargoRetry('pime-tsf', ['test', '--test', 'ime_host', '--', '--nocapture'], {PIME_IME_LIVE: '1'}));
}
// The native windows (the candidate window, the floating bar, the settings window): the probe opens them, a picture of
// the desktop is taken while they are open and the picture is checked (it must not be a flat rectangle).
if (has('--tsf-ui')) {
  step('native windows of the IME (the probe opens them, a picture is taken and checked)', () => {
    const env = {...process.env, CARGO_TARGET_DIR: targetDir};
    const crate = path.join(src, 'crates', 'pime-tsf');
    const built = sh('cargo', ['build', '--target', 'x86_64-pc-windows-gnu', '--example', 'ime_ui_probe'], {cwd: crate, env});
    if (!built.ok) return built;
    const exe = path.join(targetDir, 'x86_64-pc-windows-gnu', 'debug', 'examples', 'ime_ui_probe.exe');
    if (!fs.existsSync(exe)) return {ok: false, note: 'the probe was not built'};
    const picture = path.join(root, '02_Verification', 'ime-ui', 'ime-windows.png');
    fs.mkdirSync(path.dirname(picture), {recursive: true});
    const probe = spawn(exe, ['12'], {cwd: crate, env, stdio: 'ignore'});
    try {
      // let the windows appear, then take the picture while they are open
      spawnSync('powershell', ['-ExecutionPolicy', 'Bypass', '-File', path.join(tests, 'smoke', 'ime-ui-capture.ps1'), '-Out', picture], {stdio: 'inherit', shell: isWin});
    } finally { probe.kill(); }
    return sh(process.execPath, [path.join(tests, 'smoke', 'ime-ui-check.js'), picture]);
  });
}
// The front-ends the AI clients call (the MCP server, the command line and the local server). They live in a
// workspace of their own, and their tests include the real WM_COPYDATA round trip against the bridge.
// The TASE crates (the template-aware semantic execution system): every one of them is a crate of its own, with
// its own Cargo.lock, so each gets its own build directory (the shared one would rebuild the windows crate for
// every new feature set, and that is what runs the machine out of memory).
const TASE_CRATES = [
  'tase-contract',
  'tase-registry',
  'tase-catalog',
  'tase-planner',
  'tase-metrics',
  'tase-semantic',
  'tase-executor',
  'tase-local-ipc',
  'tase-local-store',
  'tase-agent-scheduler',
  'tase-local-hub',
  'tase-worker',
  'tase-catalog-mcp',
  'tase-ime-core',
  'tase-impls',
];
if (want('tase')) {
  step('TASE crates (the template-aware semantic execution system): cargo test (every crate of its own)', () => {
    let tests = 0;
    let done = 0;
    for (const crate of TASE_CRATES) {
      const dir = path.join(src, 'crates', crate);
      if (!fs.existsSync(dir)) continue;
      const r = cargoRetry(crate, ['test'], {CARGO_TARGET_DIR: path.join(targetDir, crate)});
      if (!r.ok) return r;
      tests += Number((r.note.match(/(\d+) tests/) || [0, 0])[1]);
      done++;
    }
    return {ok: done > 0, note: `${done} crates, ${tests} tests`};
  });
  // The catalogue and the code must agree: the implementations of the catalogue carry the real digest of the file
  // they live in, so a change in the code that is not registered turns this step red.
  step('TASE catalog: the implementations of the code are registered with their digest', () =>
    sh(process.execPath, [path.join(src, 'scripts', 'collect-implementations.js'), '--check']));
  step('TASE catalog: the 93 knowledge cards and the registered implementations agree', () =>
    sh(process.execPath, [path.join(src, 'scripts', 'validate-tase-catalog.js')]));
}
if (want('agent')) {
  step('AI agent front-ends (crates/pime-agent): cargo test (the MCP server, the CLI and the local server)', () => cargoRetry('pime-agent', ['test']));
}
if (want('worker')) {
  step('AI worker (crates/pime-ai-worker): cargo test', () => cargoRetry('pime-ai-worker', ['test']));
  step('AI worker smoke test against the HTTPS mock (PIME-TEST session credentials, removed afterwards)', () => {
    const b = cargoRetry('pime-ai-worker', ['build']);
    return b.ok ? sh(process.execPath, [path.join(tests, 'smoke', 'ai-worker-smoke.js')], {env: {...process.env, CARGO_TARGET_DIR: targetDir}}) : b;
  });
}
if (want('bench')) {
  // The cost and the latency benchmark: the local candidate path has to stay inside its budget (DD-IME §20.2:
  // p95 <= 50 ms). The evidence is written into 02_Verification/cost-benchmark.json.
  step('cost benchmark: the local candidate path stays inside its budget (DD-IME 20.2)', () =>
    sh(process.execPath, [path.join(src, 'scripts', 'bench-cost.js')]));
}
if (want('ipc')) step('IPC (crates/pime-ipc): unit + loopback tests on real named pipes', () => sh(process.execPath, [path.join(tests, 'smoke', 'ipc-smoke.js')], {env: {...process.env, CARGO_TARGET_DIR: targetDir}}));
// The red-green check: the implementation is broken on purpose, one place at a time, and the matching suite has to
// notice it. It is slow (every mutation rebuilds its crate), so it runs after the other steps. The report is
// 02_Verification/mutation-report.txt.
if (want('mutation')) step('red-green check: the implementation is broken on purpose and the suite has to notice it', () =>
  sh(process.execPath, [path.join(src, 'scripts', 'mutation-check.js')], {env: {...process.env, CARGO_TARGET_DIR: targetDir}}));
if (want('setup')) step('Installer (crates/pime-setup): cargo test (temp-folder integration tests only)', () => cargoRetry('pime-setup', ['test']));
if (want('installer')) step('framework matrix: the quoted numbers match the built installer and the cited logs', () => sh(process.execPath, [path.join(tests, 'smoke', 'installer-claims-check.js')]));
if (want('installer')) step('UAT claims: the quoted UAT numbers match the Playwright report in 02_Verification/uat', () => sh(process.execPath, [path.join(tests, 'smoke', 'uat-claims-check.js')]));
if (want('installer')) step('evidence cross-check: the build log, the sidecar and the prose agree with the built installer', () => sh(process.execPath, [path.join(tests, 'smoke', 'evidence-cross-check.js')]));
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
