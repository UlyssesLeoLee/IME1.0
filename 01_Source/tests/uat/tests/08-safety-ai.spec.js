'use strict';
// UAT group 08b: the text-only profiles (Assembly, Machine Code, PowerShell) never execute anything, and the F1
// path edits the document at the caret with correct UTF-16 / CRLF and refuses an invalid proposal.
// IT-043 (Assembly / Machine Code are not executed), ST-009 (PowerShell and machine code: text only),
// ST-026 (insert / replace Unicode at the caret), ST-029 (a syntax error is marked and never executed).
const {test, expect} = require('@playwright/test');
const {VsCode, sleep} = require('../helpers/vscode');
const {startMock} = require('../helpers/mock-provider');
const {quickLabels, openCandidates, pickLanguage, processTree, newExecutables, workspaceFiles} = require('../helpers/08-support');

test.describe.configure({mode: 'serial'});
const KEY = 'mock-key-test-456';
const FILES = {
  'asm.asm': '; asm\n',
  'bytes.hex': '',
  'run.ps1': '$a = 1\n',
  'uni.rs': 'fn a() {}\r\nfn b() {}\r\nlet s = "\u{1F600}";\r\n',   // CRLF and an emoji (a surrogate pair) on line 3
  'bad.rs': 'fn a() {}\n',
  'keep.txt': 'sentinel - this file must not change\n'
};
let app, mock, behaviour;

test.beforeAll(async () => {
  behaviour = ({task, input}) => task === 'full' ? {json: {fullFile: input.currentFile}} : {json: {insertCandidates: ['AI_TEXT']}};
  mock = await startMock('A', args => behaviour(args));
  app = await VsCode.launch({files: FILES, caFile: mock.certFile, settings: {
    'programmingIme.enabled': true, 'programmingIme.suggestMode': 'explicit',
    'programmingIme.providers': {mockA: {api: 'openai-responses', endpoint: mock.endpoint('openai-responses')}}
  }});
});
test.afterAll(async () => { await app.screenshot('end-of-group-08b'); await app.close(); await mock.close(); });
test.beforeEach(async () => { mock.reset(); await app.closeAll(); });

// The F1 panel: open it, generate, answer the consent dialog.
async function generatePanel() {
  await app.page.keyboard.press('Control+F1');
  await app.waitPanel();
  await app.panelFrame.locator('#generate').click();
  await app.waitDialog();
  await app.dialogClick('Authorize this request');
}
// The provider is configured through the command palette; the palette sometimes needs a second try when a
// quick input was still open before (the helper of the harness cannot change).
async function configureAI() {
  for (let attempt = 1; attempt <= 3; attempt++) {
    try { await app.escapeQuick(); await app.configureAI({provider: 'mockA', model: 'm-1', key: KEY}); return; }
    catch (e) { if (attempt === 3) throw e; await sleep(1000); }
  }
}

test('UAT-59 [IT-043] Assembly and Machine Code produce text only: nothing is executed, injected or compiled', async () => {
  const procsBefore = processTree(app.proc.pid);
  const filesBefore = workspaceFiles(app.ws);
  // the Assembly profile: an instruction is TEXT
  await app.openFile('asm.asm');
  await pickLanguage(app, 'assembly');
  await app.waitStatus(/PIME assembly/);
  await app.focusEditor(); await app.end();
  await app.type('mov');
  const asmLabels = await openCandidates(app);
  expect(asmLabels.join(' | ')).toMatch(/mov \(Intel/);
  await app.pickRow('mov (Intel: destination, source)');
  const asmText = await app.waitEditorText('mov rax, rbx');
  expect(asmText).toContain('mov rax, rbx');                 // instruction text, nothing else
  expect(asmText).not.toMatch(/[A-Za-z]:\\|\.exe|\.com/);     // no path, no program
  // the Machine Code profile: the bytes are HEX TEXT
  await app.closeAll();
  await app.openFile('bytes.hex');
  await pickLanguage(app, 'machine-code');
  await app.waitStatus(/PIME machine-code/);
  await app.focusEditor(); await app.end();
  await app.type('push rbp');
  const mcLabels = await openCandidates(app);
  expect(mcLabels.join(' | ')).toMatch(/push rbp/);
  await app.pickRow('push rbp');
  const mcText = await app.waitEditorText('55');
  expect(mcText).toContain('55');                             // hex text, not an executable
  expect(mcText).not.toMatch(/[A-Za-z]:\\|\.exe|\.bin/);
  // no side effect: no terminal, no new process under VS Code, no file created, changed or deleted
  expect(await app.page.locator('.terminal-wrapper').count()).toBe(0);
  expect(newExecutables(procsBefore, processTree(app.proc.pid))).toEqual([]);
  expect(workspaceFiles(app.ws)).toEqual(filesBefore);
  await app.screenshot('it-043-text-only');
});

test('UAT-60 [ST-009] PowerShell and machine code: only text or hex, zero execution and zero process injection', async () => {
  const procsBefore = processTree(app.proc.pid);
  const filesBefore = workspaceFiles(app.ws);
  // the local PowerShell rule says it in the row: text only, never executed
  await app.openFile('run.ps1');
  await app.end();
  await app.type('gci');
  await app.cmd('Programming IME: Show Code Candidates');
  await app.waitQuick();
  const rows = (await app.page.locator('.quick-input-list .monaco-list-row').allInnerTexts()).map(t => t.replace(/\s+/g, ' ').trim());
  expect(rows.join(' | ')).toMatch(/Get-ChildItem \(text only, never executed\)/);
  await app.pickRow('Get-ChildItem (text only, never executed)');
  const psText = await app.waitEditorText('Get-ChildItem');
  expect(psText).toContain('Get-ChildItem -Path path -Recurse');   // the command as text in the buffer
  // a dangerous command from the AI is text as well: it is shown, never run
  behaviour = ({task, input}) => task === 'full' ? {json: {fullFile: input.currentFile}}
    : {json: {insertCandidates: ['Remove-Item -Recurse -Force C:\\pime-should-not-exist']}};
  await app.closeAll();
  await app.openFile('run.ps1');
  await configureAI();
  await generatePanel();
  await app.waitPanelStatus('insert', /ready/);
  expect(await app.panelText('#panel-insert')).toMatch(/Remove-Item -Recurse -Force/);
  // still nothing happened on the machine
  expect(newExecutables(procsBefore, processTree(app.proc.pid))).toEqual([]);
  expect(workspaceFiles(app.ws)).toEqual(filesBefore);
  expect(app.readUser('keep.txt')).toContain('sentinel');
  expect(await app.page.locator('.terminal-wrapper').count()).toBe(0);
  await app.screenshot('st-009-powershell-text-only');
});

test('UAT-61 [ST-026] inserting/replacing Unicode at the caret: only the selection changes, UTF-16 and CRLF stay correct', async () => {
  behaviour = ({task, input}) => task === 'full' ? {json: {fullFile: input.currentFile}} : {json: {insertCandidates: ['let x = 1;\nlet y = 2;']}};
  await app.openFile('uni.rs');
  await app.goto(3, 14);                                   // the end of line 3, behind the emoji
  await app.page.keyboard.press('Shift+ArrowLeft');        // the selection is the two characters behind the emoji
  await app.page.keyboard.press('Shift+ArrowLeft');
  const before = await app.editorText();
  expect(before.split('\n')[2]).toContain('\u{1F600}');
  await configureAI();
  await generatePanel();
  await app.waitPanelStatus('insert', /ready/);
  await app.panelFrame.locator('#tab-insert').click();     // the insert tab holds the candidate for the selection
  await expect(app.panelFrame.locator('#panel-insert .cand')).toHaveCount(1);
  await app.panelFrame.locator('#panel-insert .cand button.apply').click();
  const after = await app.waitEditorText('let x = 1;');
  const afterLines = after.split('\n');
  expect(afterLines[0]).toBe(before.split('\n')[0]);       // nothing before the selection changed
  expect(afterLines[1]).toBe(before.split('\n')[1]);
  expect(afterLines[2]).toBe('let s = "\u{1F600}let x = 1;');   // only the selection was replaced
  expect(afterLines[3]).toBe('let y = 2;');
  expect(after).toContain('\u{1F600}');                     // the surrogate pair survived
  // the line endings of the file are still CRLF - also for the line the model brought with it
  await app.page.locator('.tab', {hasText: 'uni.rs'}).first().click();   // the focus is in the webview: back to the document
  await sleep(400);
  await app.save();
  await sleep(600);
  const onDisk = app.readUser('uni.rs');
  expect(onDisk).toContain('let x = 1;\r\nlet y = 2;');
  expect(onDisk.replace(/\r\n/g, '')).not.toContain('\n');
  expect(onDisk).toContain('\u{1F600}let x = 1;');
  await app.screenshot('st-026-unicode-replace');
});

test('UAT-62 [ST-029] a syntax error in the AI output is marked, the direct Apply is disabled and nothing is executed', async () => {
  const BAD = 'fn a() {}\nfn main() {\n    let x = ;\n';   // the brace is never closed
  behaviour = ({task}) => task === 'full' ? {json: {fullFile: BAD}} : {json: {insertCandidates: ['ok']}};
  const procsBefore = processTree(app.proc.pid);
  await app.openFile('bad.rs');
  const before = await app.editorText();
  await configureAI();
  await generatePanel();
  await app.waitPanelStatus('full', /ready/);
  // the proposal is marked as invalid
  await expect(app.panelFrame.locator('#validation')).toHaveAttribute('data-level', 'bad');
  expect(await app.panelText('#validation')).toMatch(/Syntax check failed/i);
  // the direct Apply of the proposal is disabled
  await expect(app.panelFrame.locator('#apply-full')).toBeDisabled();
  // nothing was written and nothing was executed
  expect(await app.editorText()).toBe(before);
  expect(await app.page.locator('.terminal-wrapper').count()).toBe(0);
  expect(newExecutables(procsBefore, processTree(app.proc.pid))).toEqual([]);
  await app.screenshot('st-029-validation-bad');
});
