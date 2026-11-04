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
  '02_Verification/installer-build.log',
]);

// The files that this round added to the repository.
const ADDITIONS = [
  '01_Source/crates/pime-setup/assets/ime1.0.ico',
  '01_Source/crates/pime-setup/build.rs',
  '01_Source/crates/pime-setup/src/paint.rs',
  '01_Source/crates/pime-setup/src/wizard.rs',
  '01_Source/tests/smoke/evidence-cross-check.js',
  '01_Source/crates/pime-tsf/src/floating.rs',
  '01_Source/crates/pime-tsf/src/imesettings.rs',
  '01_Source/crates/pime-tsf/src/menus.rs',
  '01_Source/crates/pime-tsf/src/settings_window.rs',
  '01_Source/scripts/manifest-rule-probe.js',
  '01_Source/scripts/update-manifest.js',
  // the in-process TSF host (the diagnostic tool that drives the real msctf against the real DLL)
  '01_Source/crates/pime-tsf/examples/tsf_host.rs',
  // the gate that binds the UAT numbers in the documents to the Playwright report
  '01_Source/tests/smoke/uat-claims-check.js',
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
  '01_Source/crates/pime-tsf/src/bar_skin.rs',
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

if (check) {
  console.log(stale || gone ? `${stale} stale, ${gone} gone` : `the manifest matches all ${listed.size} files`);
  process.exit(stale || gone ? 1 : 0);
}

const sorted = [...listed.entries()].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
const text = sorted.map(([relative, digest]) => `${digest}  ${relative}`).join('\n') + '\n';
fs.writeFileSync(manifestPath, text, 'utf8');
console.log(`MANIFEST_SHA256.txt: ${sorted.length} entries (${stale} refreshed, ${ADDITIONS.length} candidates)`);
