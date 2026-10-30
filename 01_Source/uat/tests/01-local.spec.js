'use strict';
// UAT group 1: local candidates, language handling, pause, F1 default, AI-off zero requests. No real AI involved.
// Each test title carries the acceptance ids of the design documents (AT-xxx / ST-xxx / IT-xxx) it gives evidence for.
const {test, expect} = require('@playwright/test');
const {VsCode, sleep} = require('../helpers/vscode');
const {startMock} = require('../helpers/mock-provider');

test.describe.configure({mode: 'serial'});
let app, mock;
const FILES = {
  'a.rs': 'fn existing() {}\n', 'b.py': 'x = 1\n', 'App.tsx': 'const a = 1;\n', 'run.ps1': '$a = 1\n', 'c.c': 'int a;\n',
  'u5.rs': 'fn base() {}\n', 'notes.asm': '; asm\n', 'bytes.hex': '', 'plain.txt': 'hello\n', 'lock.py': 'y = 2\n', 'ui.rs': '\n'
};
test.beforeAll(async () => {
  mock = await startMock('A', () => ({json: {insertCandidates: ['should-not-be-called']}}));
  app = await VsCode.launch({
    files: FILES, caFile: null,
    settings: {
      'programmingIme.enabled': true, 'programmingIme.suggestMode': 'explicit',
      'programmingIme.providers': {mockA: {api: 'openai-responses', endpoint: mock.endpoint('openai-responses')}},
      'programmingIme.scenarios': {realtime: {provider: 'mockA', model: 'm'}, f1Full: {provider: 'mockA', model: 'm'}, f1Insert: {provider: 'mockA', model: 'm'}}
    }
  });
});
test.afterAll(async () => { await app.screenshot('end-of-group-1'); await app.close(); await mock.close(); });

async function candidatesFor(prefix) {
  await app.type(prefix);
  await app.cmd('Programming IME: Show Code Candidates');   // same command as Ctrl+Alt+Space
  try { await app.waitQuick(); } catch (e) {
    await app.screenshot('candidates-missing');
    throw new Error(`no candidate list; status=${await app.statusText()}; notifications=${JSON.stringify(await app.notifications())}; editor=${JSON.stringify((await app.editorText()).slice(-80))}`);
  }
  await expect.poll(async () => (await app.quickRows()).length, {timeout: 8000}).toBeGreaterThan(0);
  return app.quickRows();
}

test('UAT-01 [FR-031/IT-026 F1 default] with the IME enabled the status bar shows the profile; F1 is taken by Programming IME only when AI is usable', async () => {
  await app.openFile('a.rs');
  const s = await app.waitStatus(/PIME rust/);
  expect(s).toMatch(/PIME rust/);
  await app.screenshot('status-rust');
});

test('UAT-02 [AT-010/ST-010] recognises .rs .py .tsx .ps1 .c; ambiguous .asm/.txt keep the current language and only hint', async () => {
  for (const [file, lang] of [['a.rs', 'rust'], ['b.py', 'python'], ['App.tsx', 'typescript'], ['run.ps1', 'powershell'], ['c.c', 'c']]) {
    await app.openFile(file);
    await app.waitStatus(new RegExp(`PIME ${lang}`));
  }
  await app.openFile('notes.asm');
  const s = await app.statusText();
  expect(s).toMatch(/PIME c\b/);            // not switched to assembly: .asm alone is not a reliable signal
  expect(s).toMatch(/PIME c \?/);           // ...but the uncertainty is shown
  await app.openFile('plain.txt');
  expect(await app.statusText()).toMatch(/PIME c/);
  await app.screenshot('ambiguous-keeps-language');
});

test('UAT-03 [AT-011/ST-010/IT-011] manual lock beats auto detection and is removable', async () => {
  await app.openFile('lock.py');
  await app.waitStatus(/PIME python/);
  await app.cmd('Programming IME: Choose Programming Language');
  await app.waitQuick();
  await app.pickRow('typescript');          // first group: "lock for this file"
  await app.waitStatus(/PIME typescript/);
  expect(await app.statusText()).toMatch(/PIME typescript/);
  await app.end();
  const rows = await candidatesFor('fori');
  expect(rows.join(' | ')).toMatch(/for-of loop/);        // TypeScript rule although the file is .py
  await app.escapeQuick();
  await app.cmd('Programming IME: Choose Programming Language');
  await app.waitQuick();
  await app.pickRow('Auto');
  await app.waitStatus(/PIME python/);
});

test('UAT-04 [ST-006/IT-007/AT-026] candidates: Esc writes nothing, accept inserts exactly at the prefix, Ctrl+Z restores', async () => {
  await app.openFile('a.rs');
  await app.end();
  const before = await app.editorText();
  const rows = await candidatesFor('fnm');
  expect(rows.join(' | ')).toMatch(/main function/);
  expect(rows[0]).toMatch(/local rule/);
  await app.screenshot('candidates-rust-fnm');
  await app.page.keyboard.press('Escape');
  await sleep(300);
  expect(await app.editorText()).toBe(before + 'fnm');                 // only what the user typed
  await app.cmd('Programming IME: Show Code Candidates'); await app.waitQuick();
  await app.pickRow('main function');
  const after = await app.waitEditorText('fn main() {');
  expect(after).toContain('fn existing() {}');
  expect(after).not.toContain('fnm');
  await app.undo();
  expect(await app.editorText()).toContain('fnm');
});

test('UAT-05 [ST-007/IT-009] every profile returns its own candidates', async () => {
  const cases = [['u5.rs', 'fori', /iterator loop/], ['b.py', 'def', /function|method/], ['App.tsx', 'interface', /interface/], ['run.ps1', 'func', /function/], ['c.c', 'main', /main function/]];
  for (const [file, prefix, re] of cases) {
    await app.openFile(file); await app.end(); await app.type('\n');
    const rows = await candidatesFor(prefix);
    expect(rows.join(' | ')).toMatch(re);
    await app.escapeQuick();
  }
  // assembly and machine code via the language dropdown
  await app.openFile('notes.asm'); await app.end();
  await app.cmd('Programming IME: Choose Programming Language'); await app.waitQuick(); await app.pickRow('assembly'); await app.waitStatus(/PIME assembly/);
  expect((await candidatesFor('mov')).join(' | ')).toMatch(/Intel/);
  await app.escapeQuick();
});

test('UAT-06 [ST-043/AT-053/UT-024] machine code: verified bytes for known forms, no bytes for unresolved symbols, nothing executed', async () => {
  await app.openFile('bytes.hex');
  await app.cmd('Programming IME: Choose Programming Language'); await app.waitQuick(); await app.pickRow('machine-code'); await app.waitStatus(/PIME machine-code/);
  await app.focusEditor();
  const rows = await candidatesFor('push rbp');
  expect(rows[0]).toMatch(/push rbp\s+→\s+55/);
  expect(rows[0]).toMatch(/verified/);
  await app.pickRow('push rbp');
  await app.waitEditorText('55');
  await app.type('\n');
  const unresolved = await candidatesFor('jmp external_symbol');
  expect(unresolved.join(' | ')).toMatch(/BYTE_ENCODING_UNRESOLVED/);
  expect(unresolved.join(' | ')).toMatch(/context missing/);
  const before = await app.editorText();
  await app.pickRow('BYTE_ENCODING_UNRESOLVED');            // selecting the explanation must not insert bytes
  await sleep(500);
  expect(await app.editorText()).toBe(before);
  expect(await app.page.locator('.terminal-wrapper').count()).toBe(0);   // no terminal was opened by the IME
  await app.screenshot('machine-code-unresolved');
});

test('UAT-07 [FR-028/ST-021/IT-024] pause: no candidates, typed text never altered', async () => {
  await app.openFile('ui.rs'); await app.end();
  await app.cmd('Programming IME: Pause / Resume Suggestions');
  await app.waitStatus(/paused/);
  await app.type('fnm fn let x = 1;', 8);
  await app.cmd('Programming IME: Show Code Candidates');
  await app.waitNotification(/paused/i);
  expect(await app.editorText()).toContain('fnm fn let x = 1;');
  await app.cmd('Programming IME: Pause / Resume Suggestions');
  await expect.poll(async () => !/paused/.test(await app.statusText())).toBeTruthy();
});

test('UAT-08 [AT-023/IT-019/FR-022] with AI off, typing 100+ characters and pressing F1 sends zero requests', async () => {
  await app.openFile('ui.rs'); await app.end();
  await app.type('\nlet value = compute(items); // keep typing to prove nothing is sent while AI is off ........................', 5);
  await sleep(1200);
  await app.page.keyboard.press('F1');                      // IME enabled -> F1 opens the Programming IME panel
  await app.waitPanel();
  await expect(app.panelFrame.locator('#panel-full .status')).toHaveAttribute('data-status', 'unavailable');
  await expect(app.panelFrame.locator('#panel-insert .status')).toHaveAttribute('data-status', 'unavailable');
  expect(await app.panelText('#panel-full .status')).toMatch(/AI off/i);
  await app.screenshot('f1-ai-off');
  expect(mock.count()).toBe(0);
});
