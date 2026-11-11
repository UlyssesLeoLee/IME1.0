'use strict';
// Recompute MANIFEST_SHA256.txt: the SHA-256 of every file the manifest lists, plus the new files of this round.
//
// The rule (measured, not guessed):
//   * a text file is hashed as its LF-normalised content, which is exactly the bytes of its git blob. A checkout
//     with CRLF line endings therefore produces the same hash on every platform.
//   * a binary file (.xlsx, .png, .ico) is hashed as it is: normalising it would change its bytes.
// The date logs (02_Verification/*.log) and the manifest itself are deliberately not listed: they are rewritten on
// every run.
//
//   node scripts/update-manifest.js          # rewrite the manifest
//   node scripts/update-manifest.js --check  # only report what is stale (exit 1 when something is)
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..', '..');
const manifestPath = path.join(root, 'MANIFEST_SHA256.txt');
const check = process.argv.includes('--check');

// The files that are never listed: the manifest itself and the logs that every run rewrites.
const EXCLUDED = new Set([
  'MANIFEST_SHA256.txt',
  '02_Verification/run-all-2026-11-02.log',
  '02_Verification/setup-tests-2026-11-02.log',
  '02_Verification/run-all-2026-11-03.log',
  '02_Verification/setup-tests-2026-11-03.log',
  '02_Verification/run-all-2026-11-04.log',
  '02_Verification/setup-tests-2026-11-04.log',
  '02_Verification/installer-build.log',
  // The benchmark is rewritten on every run (the numbers are a measurement of this machine).
  '02_Verification/cost-benchmark.json',
]);

// The directories whose files this round added to the repository.
//
// A directory is listed here instead of every file of it when the round brings a whole tree (the TASE design
// documents, the catalogue of the standard assets, the new crates). The files of the tree are read from the disk, so
// the list cannot rot.
const ADDITION_DIRECTORIES = [
  '00_Documents',
  '01_Source/assets/tase-catalog',
  '01_Source/crates/tase-contract',
  '01_Source/crates/tase-registry',
  '01_Source/crates/tase-planner',
  '01_Source/crates/tase-metrics',
  '01_Source/crates/tase-local-ipc',
  '01_Source/crates/tase-ime-core',
  '01_Source/crates/tase-local-store',
  '01_Source/crates/tase-semantic',
  '01_Source/crates/tase-catalog',
  '01_Source/crates/tase-catalog-mcp',
  '01_Source/crates/tase-local-hub',
  '01_Source/crates/tase-executor',
  '01_Source/crates/tase-agent-scheduler',
  '01_Source/crates/tase-worker',
  '01_Source/crates/tase-impls',
];

// The files of a directory, in the order the walk finds them.
function filesUnder(directory) {
  const found = [];
  const walk = (current) => {
    for (const entry of fs.readdirSync(current, {withFileTypes: true})) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) walk(full);
      else found.push(path.relative(root, full).split(path.sep).join('/'));
    }
  };
  walk(path.join(root, directory));
  return found.sort();
}

// The files that this round added to the repository.
const ADDITIONS = [

  '01_Source/crates/pime-setup/assets/ime1.0.ico',
  '01_Source/crates/pime-setup/build.rs',
  '01_Source/crates/pime-setup/src/paint.rs',
  '01_Source/crates/pime-setup/src/wizard.rs',
  '01_Source/tests/smoke/evidence-cross-check.js',
  '01_Source/crates/pime-tsf/src/skin.rs',
  '01_Source/crates/pime-tsf/src/imesettings.rs',
  '01_Source/crates/pime-tsf/src/menus.rs',
  '01_Source/crates/pime-tsf/src/settings_window.rs',
  '01_Source/scripts/manifest-rule-probe.js',
  '01_Source/scripts/update-manifest.js',
  // the in-process TSF host (the diagnostic tool that drives the real msctf against the real DLL)
  '01_Source/crates/pime-tsf/examples/tsf_host.rs',
  // the gate that binds the UAT numbers in the documents to the Playwright report
  '01_Source/tests/smoke/uat-claims-check.js',
  // the mouths of light (one per source of input) and the catalogue validator
  '01_Source/scripts/validate-tase-catalog.js',
  // the mouths of light (one per source of input) and the catalogue validator
  '01_Source/crates/pime-tsf/src/mouths.rs',
  // the plugin language packs (shipped with the distribution) and their specification
  '01_Source/language-plugins/cypher.json',
  '01_Source/language-plugins/plsql.json',
  '01_Source/language-plugins/tsql.json',
  '01_Source/docs/LANGUAGE_PLUGIN_SPEC.md',
  // the IME test scripts: the scenario suite, the function ledger and the host-level test
  '01_Source/tests/tsf/common/mod.rs',
  '01_Source/tests/tsf/common/scenarios.rs',
  '01_Source/tests/tsf/ime_scenarios.rs',
  '01_Source/tests/tsf/ime_functions.rs',
  '01_Source/tests/tsf/ime_host.rs',
  // the native window probe and the scripts that take and check its picture
  '01_Source/crates/pime-tsf/examples/ime_ui_probe.rs',
  '01_Source/tests/smoke/ime-ui-capture.ps1',
  '01_Source/tests/smoke/ime-ui-check.js',
  // the matrix of the IME functions and their tests
  '01_Source/docs/IME_TEST_MATRIX.md',
  '01_Source/docs/HOTKEYS.md',
  // the plugin language on both engines: the JavaScript suite, the shared fixtures and the Rust core test
  '01_Source/tests/js/ut-plugin-language.test.js',
  '01_Source/tests/fixtures/plugin-cases.json',
  '01_Source/tests/rust/plugin_languages.rs',
  // the L4 evidence: the desktop before the probe opened the windows, and the desktop with them open
  '02_Verification/ime-ui/desktop-baseline.png',
  '02_Verification/ime-ui/ime-windows.png',

  '01_Source/crates/pime-core/src/base64.rs',
  '01_Source/crates/pime-core/src/imeswitch.rs',
  '01_Source/crates/pime-core/src/style.rs',
  '01_Source/crates/pime-tray/Cargo.toml',
  '01_Source/crates/pime-tray/src/icon.rs',
  '01_Source/crates/pime-tray/src/main.rs',
  '01_Source/crates/pime-tray/src/tray.rs',
  '01_Source/crates/pime-tray/src/voice.rs',
  '01_Source/crates/pime-tsf/src/glass.rs',
  '01_Source/crates/pime-tsf/src/ime_switch.rs',
  '01_Source/crates/pime-tsf/src/laser.rs',
  '01_Source/crates/pime-tsf/src/laser_ui.rs',
  '01_Source/crates/pime-tsf/src/paint.rs',
  '01_Source/tests/tsf/ime_selection.rs',

  '01_Source/crates/pime-tray/src/autostart.rs',
  '01_Source/crates/pime-tsf/src/candidate_skin.rs',

  '02_Verification/ime-ui/candidate-list-zoom.png',
  '02_Verification/ime-ui/floating-bar-zoom.png',
  '02_Verification/ime-ui/settings-panel-zoom.png',
  '02_Verification/ime-ui/laser-beam-zoom.png',
  // the probe that keeps the beam on the screen, so that a picture can show it
  '01_Source/crates/pime-tsf/examples/laser_beam_probe.rs',
  // the detection of an existing installation (the page that offers repair / reinstall / uninstall)
  '01_Source/crates/pime-setup/src/detect.rs',
  // the probe that drags the mouth of the glow to the top edge, and the picture that shows it there
  '01_Source/crates/pime-tsf/examples/laser_mouth_probe.rs',
  '02_Verification/ime-ui/laser-mouth-top.png',
  // the dictionary the IME carries itself (no other IME is called) and the data it is built from
  '01_Source/crates/pime-core/src/cjk.rs',
  '01_Source/crates/pime-core/src/cjk/english-words.txt',
  '01_Source/crates/pime-core/src/cjk/japanese-words.txt',
  '01_Source/crates/pime-core/src/cjk/pinyin-phrases.txt',
  '01_Source/crates/pime-core/src/cjk/pinyin-syllables.txt',
  '01_Source/crates/pime-core/src/cjk/romaji-kana.txt',
  // the reserved interfaces for the word bank management (AI curation, remote update, sign in)
  '01_Source/crates/pime-core/src/services.rs',
  // the candidate list lives in the laser panel now, and the plugin predictor bridges the plugin languages
  '01_Source/crates/pime-tsf/src/laser_candidates.rs',
  '01_Source/crates/pime-tsf/src/predictor.rs',
  // the machine readable report of the settings window probe (the picture alone cannot prove the tabs)
  '02_Verification/ime-ui/probe-report.json',
  // the wire format the AI clients and the text service share (one contract, two sides)
  '01_Source/crates/pime-core/src/agent_proto.rs',
  // the bridge window of the text service: it receives the requests of the AI clients
  '01_Source/crates/pime-tsf/src/agent_bridge.rs',
  // the front-ends an AI agent calls: the MCP server, the command line and the local server
  '01_Source/crates/pime-agent/Cargo.toml',
  '01_Source/crates/pime-agent/Cargo.lock',
  '01_Source/crates/pime-agent/src/lib.rs',
  '01_Source/crates/pime-agent/src/bridge.rs',
  '01_Source/crates/pime-agent/src/core.rs',
  '01_Source/crates/pime-agent/src/mcp.rs',
  // the link of the agent to the Hub of TASE (the pipe of the logon session) and the tools that use it
  '01_Source/crates/pime-agent/src/hub_client.rs',
  '01_Source/crates/pime-agent/src/hub_tools.rs',
  '01_Source/crates/pime-agent/src/cli.rs',
  '01_Source/crates/pime-agent/src/serve.rs',
  '01_Source/crates/pime-agent/src/bin/pime-mcp.rs',
  '01_Source/crates/pime-agent/src/bin/pime-ime.rs',
  // how the mainstream AI clients call the input method
  '01_Source/docs/AI_AGENT_API.md',
  // the vertical writing mode (the words sideways, the characters one per line)
  '01_Source/crates/pime-tsf/src/vertical.rs',
  // the catalogue adapter of the AI agent (it finds the TASE catalogue on disk)
  '01_Source/crates/pime-agent/src/catalog.rs',
  '01_Source/crates/pime-tray/Cargo.lock',
  '01_Source/crates/pime-tsf/examples/profile_probe.rs',
  // the one ranking rule on both sides: the Rust core and the extension mirror it
  '01_Source/editor-bridges/vscode/src/core/candidates.js',
  '01_Source/tests/fixtures/tase-ranking-cases.json',
  '01_Source/tests/js/ut-tase-candidates.test.js',
  '01_Source/tests/rust/parity_tase_ime.rs',
  // the shared privacy and safety rules of TASE, used by the text service
  '01_Source/crates/pime-tsf/src/tase_policy.rs',
  // the registration of the implementations of the catalogue
  '01_Source/scripts/collect-implementations.js',
  '01_Source/scripts/validate-tase-catalog.js',
  // the map of the fifteen TASE crates of this repository
  '01_Source/docs/TASE_CRATES.md',
  '01_Source/docs/TASE_CHAIN.md',
  // the .NEMA v1.2 rules (an external reference: the interpreter and the syntax of the domains)
  '01_Source/docs/NEMA_SPEC_v1.2.md',
  // the rules of the domains (.NEMA) and the demonstration of the laser interface (ten agents)
  '01_Source/crates/pime-core/src/nema.rs',
  '01_Source/crates/pime-tsf/src/demo.rs',
  // the cost and the latency benchmark of the product
  '01_Source/scripts/bench-cost.js',
  '01_Source/crates/tase-metrics/examples/cost_bench.rs',
  '01_Source/assets/pricing/sample-price-table.json',
  '02_Verification/COST_BENCHMARK.md',
  // the defect submission sheet (the items the user has to decide)
  '02_Verification/DEFECT_SUBMISSION_2026-11-07.md',
  // the checklist the user walks through after the new DLL is in place
  '02_Verification/REAL_MACHINE_CHECKLIST.md',
];

const isBinary = bytes => bytes.includes(0);
const hashOf = file => {
  const bytes = fs.readFileSync(file);
  if (isBinary(bytes)) return crypto.createHash('sha256').update(bytes).digest('hex');
  const text = bytes.toString('utf8').replace(/\r\n/g, '\n');
  return crypto.createHash('sha256').update(Buffer.from(text, 'utf8')).digest('hex');
};

const listed = new Map();
for (const line of fs.readFileSync(manifestPath, 'utf8').split(/\r?\n/)) {
  const trimmed = line.trim();
  if (!trimmed) continue;
  const match = /^([0-9a-f]{64})\s+(.+)$/.exec(trimmed);
  if (!match) continue;
  listed.set(match[2].replace(/\\/g, '/'), match[1]);
}

let stale = 0;
let gone = 0;
for (const [relative, expected] of [...listed]) {
  if (EXCLUDED.has(relative)) {
    listed.delete(relative);
    continue;
  }
  const file = path.join(root, relative);
  if (!fs.existsSync(file)) {
    console.log(`gone   ${relative}`);
    gone++;
    if (!check) listed.delete(relative);
    continue;
  }
  const actual = hashOf(file);
  if (actual !== expected) {
    stale++;
    console.log(`stale  ${relative}`);
    console.log(`       expected ${expected}`);
    console.log(`       actual   ${actual}`);
    listed.set(relative, actual);
  }
}

for (const relative of ADDITIONS) {
  const file = path.join(root, relative);
  if (!fs.existsSync(file)) {
    console.log(`skip   ${relative} (not on disk)`);
    continue;
  }
  if (listed.has(relative)) continue;
  listed.set(relative, hashOf(file));
  console.log(`new    ${relative}`);
}

// the whole trees of this round
for (const directory of ADDITION_DIRECTORIES) {
  if (!fs.existsSync(path.join(root, directory))) {
    console.log(`skip   ${directory}/ (not on disk)`);
    continue;
  }
  for (const relative of filesUnder(directory)) {
    if (listed.has(relative)) continue;
    listed.set(relative, hashOf(path.join(root, relative)));
    console.log(`new    ${relative}`);
  }
}

if (check) {
  console.log(stale || gone ? `${stale} stale, ${gone} gone` : `the manifest matches all ${listed.size} files`);
  process.exit(stale || gone ? 1 : 0);
}

const sorted = [...listed.entries()].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
const text = sorted.map(([relative, digest]) => `${digest}  ${relative}`).join('\n') + '\n';
fs.writeFileSync(manifestPath, text, 'utf8');
console.log(`MANIFEST_SHA256.txt: ${sorted.length} entries (${stale} refreshed, ${ADDITIONS.length} candidates)`);
