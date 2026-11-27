'use strict';
// One entry point for every stored test script. It only RUNS them: fixtures, golden data, certificates and reports are
// never regenerated unless you ask for it explicitly (gen-fixtures.js --write, --report).
//
//   node tests/run-all.js                 fast, no GUI: --syntax --fixtures --golden --js --rust --smoke --tsf --installer
//   node tests/run-all.js --all           the above + --uat + --uat-rust
//   node tests/run-all.js --tsf           crates/pime-tsf: its unit tests, then build the DLL (GNU target) and inspect it (x64, exports, imports); never registers it
//   node tests/run-all.js --agent         crates/pime-agent: the front-ends the AI clients call (the MCP server, the CLI, the local server)
//   node tests/run-all.js --tsf-live      crates/pime-tsf: the live host test (the real msctf, a real context; it opens the candidate window on the desktop)
//   node tests/run-all.js --ime-live      crates/pime-tsf: the regression test of the REAL input method (the installed DLL, a real window with the keyboard focus, the real keyboard; it takes the focus of the desktop)
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
const {runCargoSuite} = require('../scripts/cargo-suite.js');

const tests = __dirname, src = path.resolve(tests, '..'), root = path.resolve(src, '..');
const verification = path.join(root, '02_Verification');
const args = process.argv.slice(2);
const has = f => args.includes(f);
const all = has('--all');
const fast = !args.some(a => /^--(syntax|fixtures|golden|js|rust|smoke|tsf|tsf-live|ime-live|tsf-ui|worker|ipc|setup|agent|tase|bench|mutation|installer|tray|uat|uat-rust|report)$/.test(a)) && !all;
// no flags = the fast set (no GUI); --all adds the two UAT runs; single flags select single steps
// The UAT is NOT part of the fast set any more (the user, 2026-11-19): it drives the VS Code **bridge**, not the
// input method itself, and it was the most fragile step of the gate - about 2.2% of its scenarios fail per run and
// the failure reads like a product defect (D-150). It stays available as an explicit step
// (`node tests/run-all.js --uat`) and its evidence stays in 02_Verification/uat, but it is no longer a gate.
// What watches the input method itself is the step that drives the real TSF DLL through the real msctf
// (`--ime-live`: the installed DLL, a real window with the keyboard focus, the real keyboard).
const FAST = ['syntax', 'fixtures', 'golden', 'rust', 'js', 'smoke', 'tsf', 'agent', 'tase', 'tray', 'bench', 'mutation', 'setup', 'installer'];
const want = k => all || (fast && FAST.includes(k)) || has(`--${k}`);
const onlyUat = (() => { const i = args.indexOf('--only-uat'); return i >= 0 ? args[i + 1] : null; })();
const targetDir = process.env.CARGO_TARGET_DIR || path.join(os.tmpdir(), 'pime-cargo-target');
const coreExe = process.env.PIME_CORE_EXE || path.join(targetDir, 'debug', 'pime-core.exe');
const isWin = process.platform === 'win32';
const npx = isWin ? 'npx.cmd' : 'npx';
const results = [];

// The gate writes its own evidence log, while it runs.
//
// The log is a committed artefact (the three claims checks read it, the delivery quotes its numbers), and those
// checks run INSIDE the gate. If the log were only written after the run, they would read the log of the previous
// round: a log that ever carried a failing step could then never be green again (D-134). Writing it here - every
// line that goes to the console goes into the file as well - means the checks read the round that is running.
const LOG_PATH = path.join(verification, 'run-all-2026-11-04.log');
let logFd = null;
try {
  // opened synchronously and truncated at once: an asynchronous stream would still be unopened when the process
  // exits, and the log would keep the content of the previous round
  logFd = fs.openSync(LOG_PATH, 'w');
  const writeOut = process.stdout.write.bind(process.stdout);
  const writeErr = process.stderr.write.bind(process.stderr);
  process.stdout.write = (chunk, ...rest) => {
    try { fs.writeSync(logFd, chunk); } catch { /* the console still gets the line */ }
    return writeOut(chunk, ...rest);
  };
  process.stderr.write = (chunk, ...rest) => {
    try { fs.writeSync(logFd, chunk); } catch { /* the console still gets the line */ }
    return writeErr(chunk, ...rest);
  };
  process.on('exit', () => { try { fs.closeSync(logFd); } catch { /* the file is closed anyway */ } });
} catch (e) {
  console.log(`note: the evidence log was not opened: ${e.message}`);
}

// A freshly linked binary can be refused once (the scanner of the machine, or a locked file). That is not a failure
// of the code, so the run is repeated (D-138: the old check only knew "os error 5" and did not see the other forms).
const CANNOT_EXECUTE = /os error 5|could not execute process|never executed|no such file or directory|could not be started/i;
// The machine ran out of memory while compiling ("rustc-LLVM ERROR: out of memory", "Allocation failed", the
// Win32 error 1455 "the paging file is too small"): that is the machine, not the code (D-180). The other
// applications of the machine come and go, so the step is tried again after a longer pause instead of turning
// the whole round red.
const OUT_OF_MEMORY = /out of memory|allocation failed|os error 1455|paging file is too small|unable to commit/i;
// The machine refused to START the program at all ("spawn EPERM", the commit limit is exhausted): that is the
// machine, not the code (D-180, 2026-11-25 的门禁实录里出现过一次). It is retried like the other machine faults,
// otherwise one hiccup of the machine turns the whole round red.
const SPAWN_REFUSED = /the command could not be started/i;
const RETRY_PAUSE_MS = 45000;
const RETRY_PAUSE_MEMORY_MS = 60000;

// The raw output of a test run, kept as evidence: the traceability report maps the design case ids by the ids
// in the test titles, and it reads these files. Without them it would map against a stale run.
function saveEvidence(name, text) {
  try {
    fs.writeFileSync(path.join(verification, `${name}-test-result.txt`), text.replace(/\r\n/g, '\n'), 'utf8');
  } catch (e) {
    console.log(`note: the evidence file of ${name} was not written: ${e.message}`);
  }
}

// The output of a step goes through the console AND into the evidence log.
//
// The output has to be CAPTURED, not inherited: a child that inherits the file descriptor writes straight to it and
// never passes through this process, so its lines would be missing from the log - and the claims checks read the
// numbers of the suites out of that log (the count of the JavaScript suite comes from there).
const sh = (cmd, cmdArgs, opts = {}) => {
  const r = spawnSync(cmd, cmdArgs, {
    stdio: ['inherit', 'pipe', 'pipe'],
    encoding: 'utf8',
    maxBuffer: 256 * 1024 * 1024,
    shell: isWin && /\.cmd$/i.test(cmd),
    ...opts,
  });
  if (r.stdout) process.stdout.write(r.stdout);
  if (r.stderr) process.stderr.write(r.stderr);
  return {ok: r.status === 0, note: r.status === 0 ? '' : `exit ${r.status}`, out: `${r.stdout || ''}${r.stderr || ''}`};
};

// A freshly linked binary can be refused by the scanner of the machine for a moment ("could not execute process").
// The run is repeated then - and the retries WAIT a little: without the pause the scanner catches every new binary
// as well, because the three attempts follow each other within seconds (D-138).
const sleepSync = ms => { try { Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms); } catch { /* the pause is a nicety */ } };

// A step is tried again when its binary could not be executed: the scanner of the machine quarantines a freshly
// linked binary for a moment, and a step that builds one (the cost benchmark, the DLL check) has no retry of its own.
function step(name, fn) {
  const t0 = Date.now();
  console.log(`\n=== ${name} ===`);
  let ok = false, note = '', out = '';
  for (let attempt = 1; attempt <= 3; attempt++) {
    try { const r = fn(); ok = r.ok; note = r.note || ''; out = r.out || ''; } catch (e) { note = String(e.message || e); }
    if (ok) break;
    const memory = OUT_OF_MEMORY.test(out) || OUT_OF_MEMORY.test(note);
    if ((!CANNOT_EXECUTE.test(out) && !memory) || attempt === 3) break;
    const pause = memory ? RETRY_PAUSE_MEMORY_MS : RETRY_PAUSE_MS;
    console.log(`note: the step could not ${memory ? 'get the memory it needs' : 'execute its binary (the scanner of the machine?)'}; it is tried again in ${pause / 1000} s`);
    sleepSync(pause);
  }
  results.push({name, ok, note, seconds: ((Date.now() - t0) / 1000).toFixed(1)});
  console.log(`--- ${name}: ${ok ? 'PASS' : 'FAIL'} ${note}`);
}

if (want('syntax')) step('syntax check (extension sources + all test scripts)', () => sh(process.execPath, [path.join(src, 'editor-bridges', 'vscode', 'scripts', 'check-syntax.js')]));
if (want('fixtures')) step('golden fixtures are unchanged (compare only)', () => sh(process.execPath, [path.join(tests, 'gen-fixtures.js')]));
if (want('golden')) step('golden-v0.2 cases are still present in the live fixtures', () => sh(process.execPath, [path.join(src, 'scripts', 'glue-check.js'), '--golden']));
if (want('rust')) {
  step('Rust: cargo test --workspace', () => {
    let last = null;
    for (let attempt = 1; attempt <= 3; attempt++) { // a freshly linked exe can fail once while Defender scans it
      // The helper restores the write access of a binary the scanner locked and runs the test binaries from a
      // copy when the scanner refuses to execute them (D-138). The tests are the same tests either way.
      const r = runCargoSuite({args: ['test', '--workspace'], cwd: src, env: {...process.env, CARGO_TARGET_DIR: targetDir}});
      last = r;
      process.stdout.write(r.out || '');
      if (r.note) console.log(`note: ${r.note}`);
      saveEvidence('rust', r.out || '');
      if (r.status === 0) return {ok: true, note: ((r.out || '').match(/test result: ok\. (\d+) passed/g) || []).map(x => Number(x.match(/(\d+)/)[1])).reduce((a, b) => a + b, 0) + ' tests'};
      if (!CANNOT_EXECUTE.test(r.out || '') && !OUT_OF_MEMORY.test(r.out || '') && !SPAWN_REFUSED.test(r.out || '')) break;
      if (attempt < 3) sleepSync(OUT_OF_MEMORY.test(r.out || '') || SPAWN_REFUSED.test(r.out || '') ? RETRY_PAUSE_MEMORY_MS : RETRY_PAUSE_MS);
    }
    // A test binary that could not be started at all is the machine, not the code: the step says so.
    if (last && last.status === -1) return {ok: false, note: `the scanner of the machine blocked a test binary (${last.note || 'the binary could not be started'})`};
    return {ok: false, note: 'cargo test failed'};
  });
} else if ((want('smoke') || want('uat-rust')) && !fs.existsSync(coreExe)) {
  step('Rust: cargo build (the core exe is needed by the selected steps)', () => sh('cargo', ['build'], {cwd: src, env: {...process.env, CARGO_TARGET_DIR: targetDir}}));
}
// with a built core the JS suite also runs its engine differential tests (they skip without it)
if (want('js')) step('JavaScript tests' + (fs.existsSync(coreExe) ? ' (+ engine differential tests, core exe found)' : ''), () =>
  sh(process.execPath, ['--test', '*.test.js'], {cwd: path.join(tests, 'js'), env: {...process.env, ...(fs.existsSync(coreExe) ? {PIME_CORE_EXE: coreExe} : {})}}));
if (want('smoke')) step('real pime-core.exe smoke test (JSON Lines)', () => sh(process.execPath, [path.join(tests, 'smoke', 'core-binary-smoke.js')], {env: {...process.env, PIME_CORE_EXE: coreExe}}));
const uat = engine => {
  const env = {...process.env};
  const uatArgs = ['playwright', 'test', ...(onlyUat ? ['-g', onlyUat] : [])];
  if (engine === 'rust') {
    env.PIME_ENGINE = 'rust'; env.PIME_CORE_EXE = coreExe;
    env.UAT_EVIDENCE_DIR = env.UAT_EVIDENCE_DIR || fs.mkdtempSync(path.join(os.tmpdir(), 'pime-uat-rust-'));
  }
  return sh(npx, uatArgs, {cwd: path.join(tests, 'uat'), env});
};
// The UAT is the most timing-sensitive step of the gate (it drives the real VS Code), so it runs EARLY, while the
// machine is still idle, and it gets a second try: a scenario that times out under load must not turn the gate red.
// Both tries stay in the log, so nothing is hidden (D-148).
if (want('uat')) step('UAT on real VS Code (JavaScript engine)', () => {
  const first = uat('js');
  if (first.ok) return first;
  console.log('note: the UAT failed on the first try; it runs once more (its scenarios drive the real VS Code and time out under load)');
  return uat('js');
});
if (want('uat-rust')) step('UAT on real VS Code (Rust engine)', () => uat('rust'));
// cargo in one of the crates that live outside the main workspace; a freshly linked exe can fail once with os error 5
const cargoRetry = (crate, cargoArgs, extraEnv = {}, evidenceName = '') => {
  let last = null;
  for (let attempt = 1; attempt <= 3; attempt++) {
    const r = runCargoSuite({args: cargoArgs, cwd: path.join(src, 'crates', crate), env: {...process.env, CARGO_TARGET_DIR: targetDir, ...extraEnv}});
    last = r;
    process.stdout.write(r.out || '');
    if (r.note) console.log(`note: ${r.note}`);
    if (evidenceName) saveEvidence(evidenceName, r.out || '');
    if (r.status === 0) return {ok: true, note: ((r.out || '').match(/test result: ok\. (\d+) passed/g) || []).map(x => Number(x.match(/(\d+)/)[1])).reduce((a, b) => a + b, 0) + ' tests'};
    if (!CANNOT_EXECUTE.test(r.out || '') && !OUT_OF_MEMORY.test(r.out || '') && !SPAWN_REFUSED.test(r.out || '')) break;
    if (attempt < 3) sleepSync(OUT_OF_MEMORY.test(r.out || '') || SPAWN_REFUSED.test(r.out || '') ? RETRY_PAUSE_MEMORY_MS : RETRY_PAUSE_MS);
  }
  if (last && last.status === -1) return {ok: false, note: `the scanner of the machine blocked a test binary in crates/${crate} (${last.note || 'the binary could not be started'})`};
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
// The regression test of the REAL input method: the installed DLL, a real window that owns the keyboard focus, the
// real keyboard. It takes the focus of the desktop for a moment (the laser interface comes on the screen), so it is
// opt-in (--ime-live). Two of its four cases fail today (D-170: the key events do not reach the text service).
if (has('--ime-live')) {
  step('the real input method (the installed DLL, a focused window, the real keyboard; it takes the focus of the desktop)', () =>
    cargoRetry('pime-tsf', ['test', '--test', 'ime_live', '--', '--nocapture', '--test-threads=1'], {PIME_IME_LIVE: '1'}));
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
  // pime-ipc is the IPC layer the TASE crates talk through (tase-local-ipc uses it): it is a crate of its own with
  // its own Cargo.lock, and its own tests (the named pipe round trip) were **not** run by the gate until now -
  // a change in it could pass the gate without a single one of its own tests running.
  'pime-ipc',
];
if (want('tase')) {
  step('TASE crates (the template-aware semantic execution system) and pime-ipc: cargo test (every crate of its own)', () => {
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
if (want('tray')) {
  // The resident tray: the logo it draws at run time, the autostart switch and the voice input. The tray is its own
  // workspace (it needs unsafe), so it is built in its own directory, like the TSF crate.
  step('resident tray (crates/pime-tray): cargo test (the logo at run time, the autostart and the voice input)',
    () => cargoRetry('pime-tray', ['test']));
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
// The installer crate is part of the gate: its tests are the evidence for the numbers the documents quote about the
// installer (the claims block reads `02_Verification/setup-test-result.txt`, which this step writes) - a log from an
// older build would be evidence for code that is not the code under test (the verifier's point, 2026-11-25).
if (want('setup')) step('Installer (crates/pime-setup): cargo test (temp-folder integration tests only)', () =>
  cargoRetry('pime-setup', ['test'], {}, 'setup'));
// The claims checks run AFTER the UAT (it runs early, see above): they read the evidence of the acceptance run (the
// numbers of the scenarios, their result and the time the report was written), so they have to see the run of this
// round and not the one of the previous round.
if (want('installer')) step('framework matrix: the quoted numbers match the built installer and the cited logs', () => sh(process.execPath, [path.join(tests, 'smoke', 'installer-claims-check.js')]));
if (want('installer')) step('UAT claims: the quoted UAT numbers match the Playwright report in 02_Verification/uat', () => sh(process.execPath, [path.join(tests, 'smoke', 'uat-claims-check.js')]));
if (want('installer')) step('evidence cross-check: the build log, the sidecar and the prose agree with the built installer', () => sh(process.execPath, [path.join(tests, 'smoke', 'evidence-cross-check.js')]));
if (has('--report')) step('rewrite the traceability report (explicit)', () => sh(process.execPath, [path.join(src, 'scripts', 'traceability-report.js')]));

console.log('\n==== summary ====');
for (const r of results) console.log(`${r.ok ? 'PASS' : 'FAIL'}  ${r.name}  (${r.seconds}s) ${r.note}`);
process.exit(results.every(r => r.ok) ? 0 : 1);
