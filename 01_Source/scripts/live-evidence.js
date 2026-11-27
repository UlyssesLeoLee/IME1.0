'use strict';
// Runs the live part of the suite: the real text service, a real keyboard, a real window.
//
//   node scripts/live-evidence.js [--copy-dll]
//
// The live part is opt-in for two reasons: it takes the focus of the desktop (the laser interface of the input
// method comes on the screen), and it needs the profile of this product to be registered on this machine. The report
// of a real run is written by the test itself into 02_Verification/ime-live-report.txt - this script only runs it and
// says what it measured (the two invariants of the user: the pinyin must not stand next to the character, the code
// must go into the document as it was typed).
//
// --copy-dll: also copies the text service of the installed product over the **per-user COM override**
//             (HKCU\Software\Classes\CLSID\{...}\InProcServer32 -> %LOCALAPPDATA%\IME1.0-dev16\pime_tsf.dll), which
//             wins over the installed one. Without that step the old DLL keeps answering and the run measures the
//             wrong build.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {spawnSync} = require('node:child_process');
const {runCargoSuite} = require('./cargo-suite.js');

const root = path.resolve(__dirname, '..', '..');
const src = path.join(root, '01_Source');
const targetDir = process.env.CARGO_TARGET_DIR || path.join(root, '01_Source', 'target');
const reportPath = path.join(root, '02_Verification', 'ime-live-report.txt');
const copyDll = process.argv.includes('--copy-dll');

const CLSID = '{43F95ED1-95B4-421D-8F5A-837128C2D854}';
const profileKey = `HKLM\\SOFTWARE\\Microsoft\\CTF\\TIP\\${CLSID}\\LanguageProfile\\0x00000804`;

const say = text => console.log(text);

// ---- 1. is the product installed on this machine? ------------------------------------------------------------------
const profile = spawnSync('reg', ['query', profileKey], {encoding: 'utf8', timeout: 30000});
if (profile.status !== 0) {
  say('the live step cannot run: the zh-CN profile of this product is not registered on this machine.');
  say('install it first (the installer needs administrator rights):');
  say('  01_Source\\dist\\ProgrammingIME-<version>-win-x64-setup.exe');
  say('then log out or restart, and run this script again.');
  process.exit(1);
}
say('ok  the zh-CN profile of this product is registered');

// ---- 2. the per-user COM override (it wins over the installed DLL) -------------------------------------------------
const override = path.join(os.homedir(), 'AppData', 'Local', 'IME1.0-dev16', 'pime_tsf.dll');
const installed = path.join(process.env.ProgramFiles || 'C:\\Program Files', 'IME1.0', 'pime_tsf.dll');
if (copyDll) {
  if (!fs.existsSync(installed)) {
    say(`FAIL the text service of the installed product is not there: ${installed}`);
    process.exit(1);
  }
  fs.mkdirSync(path.dirname(override), {recursive: true});
  fs.copyFileSync(installed, override);
  say(`ok  the text service of the installed product is copied over the per-user override: ${override}`);
  say('    (the applications that are already open keep the old DLL - open them again)');
} else if (fs.existsSync(override)) {
  say(`note the per-user COM override is there: ${override}`);
  say('     it wins over the installed DLL - run with --copy-dll to put the installed one over it');
}

// ---- 3. the run itself -------------------------------------------------------------------------------------------
say('\nrunning the live part (it takes the focus of the desktop for a few seconds - do not touch the mouse or the keyboard)');
const run = runCargoSuite({
  args: ['test', '--test', 'ime_live', '--', '--nocapture', '--test-threads=1'],
  cwd: path.join(src, 'crates', 'pime-tsf'),
  env: {...process.env, PIME_IME_LIVE: '1', CARGO_TARGET_DIR: targetDir},
});
process.stdout.write(run.out || '');
if (run.note) say(`note: ${run.note}`);

// ---- 4. what it measured -----------------------------------------------------------------------------------------
say('');
if (fs.existsSync(reportPath)) {
  const text = fs.readFileSync(reportPath, 'utf8');
  const field = name => {
    const m = text.match(new RegExp(`${name}=(\\S*)`));
    return m ? m[1] : '<the report has no such field>';
  };
  say(`the report of the run: ${path.relative(root, reportPath)}`);
  say(`  doc_before_click = ${field('doc_before_click')}`);
  say(`  doc_after_click  = ${field('doc_after_click')}   <- the candidate alone, the pinyin must not stand next to it`);
  say(`  doc_code         = ${field('doc_code')}`);
  say(`  doc_code_delta   = ${field('doc_code_delta')}   <- the code as it was typed`);
  say(`  code_phase       = ${field('code_phase')}`);
} else {
  say('note: the run wrote no report (the test skips itself when the profile is not registered)');
}
process.exit(run.status === 0 ? 0 : 1);
