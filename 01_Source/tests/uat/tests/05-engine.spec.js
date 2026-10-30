'use strict';
// UAT group 5: the engine switch (migration step 1.0). With programmingIme.engine = rust the Rust core is REQUIRED:
// when it is missing or dies the IME features are switched off with a clear status and notification, there is no silent
// fallback to the JavaScript engine, and typing in VS Code itself is never affected.
const {test, expect} = require('@playwright/test');
const fs = require('node:fs');
const path = require('node:path');
const {spawnSync} = require('node:child_process');
const {VsCode, sleep} = require('../helpers/vscode');

test.describe.configure({mode: 'serial'});
const FILES = {'a.rs': 'fn existing() {}\n'};
const BASE = {'programmingIme.enabled': true, 'programmingIme.suggestMode': 'explicit'};
const CORE = process.env.PIME_CORE_EXE && fs.existsSync(process.env.PIME_CORE_EXE) ? process.env.PIME_CORE_EXE : null;
let app = null;
test.afterEach(async () => { if (app) { await app.close(); app = null; } });

async function typingStillWorks(file) {
  await app.openFile(file);
  await app.end(); await app.type('\nlet typed = 1;');
  expect(await app.editorText()).toContain('let typed = 1;');
  expect(await app.isActiveTabDirty()).toBe(true);
}

test('ENG-01 [step 1.0] engine=rust without a usable core: clear status and notification, IME features off, no JS fallback, typing unaffected', async () => {
  app = await VsCode.launch({files: FILES, settings: {...BASE, 'programmingIme.engine': 'rust', 'programmingIme.coreExecutable': 'C:\\pime-does-not-exist\\pime-core.exe'}});
  expect(await app.waitStatus(/PIME .*core unavailable/)).toMatch(/core unavailable/);
  const note = await app.waitNotification(/core is not available/i, 15000);
  expect(note).toMatch(/programmingIme\.coreExecutable/); expect(note).toMatch(/typing in VS Code is not affected/i);
  await app.screenshot('engine-rust-core-missing');
  await typingStillWorks('a.rs');
  // no candidates: the JavaScript engine must NOT answer instead
  await app.type('\nfnm');
  await app.clearNotifications();
  await app.cmd('Programming IME: Show Code Candidates');
  await sleep(1200);
  expect(await app.quickVisible()).toBe(false);
  expect((await app.editorText())).not.toContain('fn main()');
  // F1 is not taken over: it opens the Command Palette
  await app.focusEditor();
  await app.page.keyboard.press('F1');
  await app.waitQuick();
  expect(await app.page.locator('.quick-input-box input').inputValue()).toMatch(/^>/);
  await app.escapeQuick();
});

test('ENG-02 [step 1.0] engine=rust: the core is killed while VS Code runs: status switches to "core unavailable", candidates stop, typing continues', async () => {
  test.skip(!CORE, 'needs PIME_CORE_EXE (a built pime-core)');
  app = await VsCode.launch({files: FILES, settings: {...BASE, 'programmingIme.engine': 'rust', 'programmingIme.coreExecutable': CORE}});
  await app.openFile('a.rs');
  await app.waitStatus(/PIME rust/);
  await app.end(); await app.type('\nfnm');
  await app.cmd('Programming IME: Show Code Candidates'); await app.waitQuick();
  expect((await app.quickRows()).join('|')).toMatch(/main function/);      // the Rust core answers
  await app.escapeQuick();
  spawnSync('taskkill', ['/F', '/IM', path.basename(CORE)], {stdio: 'ignore'});   // the core dies
  await app.waitStatus(/core unavailable/, 15000);
  await app.waitNotification(/core is not available/i, 15000);
  await app.screenshot('engine-rust-core-killed');
  await app.clearNotifications();
  await app.cmd('Programming IME: Show Code Candidates');
  await sleep(1000);
  expect(await app.quickVisible()).toBe(false);
  await app.focusEditor(); await app.end(); await app.type(' // still typing');
  expect(await app.editorText()).toContain('// still typing');
});
