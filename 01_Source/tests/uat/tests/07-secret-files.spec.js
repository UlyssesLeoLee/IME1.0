'use strict';
// UAT group 8: the secret-file read gate (docs/SECRET_FILES.md; requirement added by the project owner after the design baseline, test ids SEC-xx, UAT ids UAT-50..UAT-55).
// Original wording (Chinese): "通过获取.env等文件名规则，首先分析该文件是否属于密钥类，如果有，弹出确认窗口，用户确认可以读取，才读取".
// Before Programming IME reads a file it looks at the NAME only; a secret-class file (.env, keys, credentials ...) is read only after the user confirmed it:
// a MODAL for an explicit command (F1, F2), ONE non-modal notification per file and session while typing, and a status bar lock + the command
// "Programming IME: Secret Files" to allow or forget a file.
//
// STATUS: written against the proven patterns of 02-ai-f1 / 04-annotate / f2-prompt / 06-annotate-scope, NOT RUN on a real VS Code (the work item had no memory
// for it: no Playwright, no VS Code, no UAT run). Syntax-checked only (node tests/run-all.js --syntax). The same behaviour is covered against fakes by
// tests/js/ut-secret-wiring.test.js (SEC-30..SEC-46). The parts most likely to need maintenance are the selectors of the notification BUTTONS
// (notificationButton) and of the status bar text: both are VS Code DOM, not an API. No link or junction is created here (symlink creation is blocked on
// the development machine), so the real-path resolution is NOT covered by this spec.
// Run it with:  node tests/run-all.js --uat --only-uat "UAT-5[0-5]"
// The secret-class files exist only in this spec's own throw-away workspace (helpers/vscode.js writes `files` there); the base profile is untouched.
const {test, expect} = require('@playwright/test');
const {spawnSync} = require('node:child_process');
const {VsCode, sleep} = require('../helpers/vscode');
const {startMock} = require('../helpers/mock-provider');

test.describe.configure({mode: 'serial'});
const SENTINEL = 'CLIPBOARD-BEFORE-SEC';
let app, mock, savedClipboard = '';

// ---------------------------------------------------------------- clipboard (OS level, Windows PowerShell 5.1; the same helpers as f2-prompt.spec.js)
const PS = ['-NoProfile', '-NonInteractive', '-STA'];
function readClipboard() {
  const r = spawnSync('powershell.exe', [...PS, '-Command', '[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false; $t = Get-Clipboard -Raw; if ($null -ne $t) { [Console]::Out.Write($t) }'], {encoding: 'buffer', timeout: 20000, windowsHide: true});
  return r.stdout.toString('utf8').split(String.fromCharCode(0xfeff)).join('').replace(/\r\n/g, '\n');
}
function writeClipboard(text) {
  spawnSync('powershell.exe', [...PS, '-Command', 'Set-Clipboard -Value $env:PIME_UAT_CLIP'], {env: {...process.env, PIME_UAT_CLIP: text}, timeout: 20000, windowsHide: true});
}

// ---------------------------------------------------------------- fixtures
// One secret-class file per test: the tests share ONE extension host and a decision (allow for the session, keep off, the "already announced" memory) lives in memory
// for the whole session, so a file used by an earlier test would not ask again. Each of these names is an `env-file` of core/secretfile.js.
const SECRET = 'API_KEY=placeholder-value-for-uat\nMODE=dev\n';
const FILES = {
  '.env.type': SECRET, '.env.f1': SECRET, '.env.f2': SECRET, '.env.cmd': SECRET,
  '.env.example': 'API_KEY=\nMODE=dev\n',                       // a template: not secret-class
  'main.rs': 'fn main() {\n    println!("hi");\n}\n',
  'diary.txt': 'Remember to buy milk and call the bank before noon.\n'   // secret-class only through the user setting below
};
const ALLOW_ONCE = 'Allow once', ALLOW_SESSION = 'Allow for this session';
const NOTE_ALLOW = 'Allow this file for this session', NOTE_OFF = 'Keep off';
const notificationCount = async re => (await app.notifications()).filter(t => re.test(t)).length;
// a button of a notification toast (VS Code DOM: a guess at the structure; if it is wrong the spec fails here, not in the product)
const notificationButton = label => app.page.locator('.notifications-toasts .notification-list-item-buttons-container .monaco-button', {hasText: label}).first();
const protectedStatus = /PIME protected file/;
const pressF1 = async file => { await app.openFile(file); await app.page.keyboard.press('F1'); };
const panelCount = () => app.page.locator('iframe.webview').count();

test.beforeAll(async () => {
  savedClipboard = readClipboard();
  mock = await startMock('S', () => ({json: {}}));
  app = await VsCode.launch({files: FILES, caFile: mock.certFile, settings: {
    'programmingIme.enabled': true, 'programmingIme.suggestMode': 'auto',     // the typing path needs the automatic suggestions
    'programmingIme.secretFilePatterns': ['diary.txt'],
    'programmingIme.providers': {mockA: {api: 'openai-responses', endpoint: mock.endpoint('openai-responses')}}}});   // AI stays off: nothing is sent in this group
});
test.afterAll(async () => { await app.screenshot('end-of-group-8'); await app.close(); await mock.close(); writeClipboard(savedClipboard); });
test.beforeEach(async () => { mock.reset(); writeClipboard(SENTINEL); await app.closeAll(); await app.clearNotifications(); });

test('UAT-50 [SEC-33] typing in a .env: never a dialog, ONE notification however much is typed, the status bar shows the lock; Allow this file opens it', async () => {
  await app.openFile('.env.type');
  await app.end();
  await app.type('\nEXTRA=1', 25);
  const note = await app.waitNotification(/looks like a secret file/);
  expect(note).toContain('".env.type"'); expect(note).toMatch(/not reading it/);
  expect(await app.dialogVisible()).toBe(false);
  await app.type('23456789', 25);                                           // more keystrokes: still the one notification
  await sleep(1500);
  expect(await notificationCount(/looks like a secret file/)).toBe(1);
  expect(await app.statusText()).toMatch(protectedStatus);
  await app.screenshot('secret-typing-notice');
  await notificationButton(NOTE_ALLOW).click();
  await app.waitStatus(/PIME (?!protected)/, 8000);
  expect(await app.statusText()).not.toMatch(protectedStatus);
  expect(mock.count()).toBe(0);
});

test('UAT-51 [SEC-40] F1 on a .env: a modal with exactly Allow once / Allow for this session; Esc opens no panel and remembers nothing; Allow once is spent by one F1', async () => {
  await pressF1('.env.f1');
  const d = await app.waitDialog();
  expect(d.message).toContain('.env.f1'); expect(d.message).toMatch(/environment variables file/);
  expect(d.detail).toMatch(/Reading is local/); expect(d.detail).toMatch(/AI provider is asked separately/);
  expect(d.buttons).toEqual(expect.arrayContaining([ALLOW_ONCE, ALLOW_SESSION]));
  await app.screenshot('secret-f1-modal');
  await app.page.keyboard.press('Escape');                                  // Esc: refuse this call
  await app.dialog.waitFor({state: 'hidden', timeout: 5000});
  await app.waitNotification(/F1 did not start/);
  expect(await panelCount()).toBe(0);
  await app.page.keyboard.press('F1');                                      // nothing was remembered: it asks again
  await app.waitDialog(); await app.dialogClick(ALLOW_ONCE);
  await app.waitPanel();
  expect(mock.count()).toBe(0);
  await app.closeAll();
  await app.openFile('.env.f1'); await app.page.keyboard.press('F1');      // the one-use grant is gone: asked again
  const again = await app.waitDialog();
  expect(again.buttons).toEqual(expect.arrayContaining([ALLOW_ONCE, ALLOW_SESSION]));
  await app.dialogClick(ALLOW_SESSION);
  await app.waitPanel();
  await app.closeAll();
  await app.openFile('.env.f1'); await app.page.keyboard.press('F1');      // a session grant: no dialog any more
  await app.waitPanel();
  expect(await app.dialogVisible()).toBe(false);
});

test('UAT-52 [SEC-42] F2 on a .env selection: a refusal is a notice and the clipboard is untouched; allowing the READ does not allow the COPY (the content gate still refuses it)', async () => {
  await app.openFile('.env.f2');
  await app.page.keyboard.press('Control+A');
  await app.page.keyboard.press('F2');
  const d = await app.waitDialog();
  expect(d.buttons).toEqual(expect.arrayContaining([ALLOW_ONCE, ALLOW_SESSION]));
  await app.page.keyboard.press('Escape');
  await app.dialog.waitFor({state: 'hidden', timeout: 5000});
  await app.waitNotification(/F2 did not run/);
  expect(readClipboard()).toBe(SENTINEL);
  await app.closeAll();
  await app.openFile('.env.f2'); await app.page.keyboard.press('Control+A'); await app.page.keyboard.press('F2');   // Esc remembered nothing: it asks again
  await app.waitDialog(); await app.dialogClick(ALLOW_ONCE);
  await app.waitNotification(/Looks sensitive/);                           // read, but never copied: reading and sending/copying are separate questions
  expect(readClipboard()).toBe(SENTINEL); expect(mock.count()).toBe(0);
});

test('UAT-53 [SEC-39] the command "Programming IME: Secret Files": allow the active file, see the decision, forget it, the status bar follows', async () => {
  await app.openFile('.env.cmd');
  await app.waitStatus(protectedStatus, 8000);                              // the lock is on screen as soon as the file is the active one (no read, no keystroke)
  await app.cmd('Programming IME: Secret Files');
  await app.waitQuick();
  const rows = await app.quickRows();
  expect(rows[0]).toMatch(/Allow ".env.cmd" for this session/);
  await app.pickRow('Allow ".env.cmd" for this session');
  await app.waitNotification(/Allowed ".env.cmd" for this session/);
  await app.waitStatus(/PIME (?!protected)/, 8000);
  await app.cmd('Programming IME: Secret Files');
  await app.waitQuick();
  const listed = await app.quickRows();
  expect(listed.some(r => /\.env\.cmd/.test(r) && /allowed until/.test(r))).toBe(true);
  expect(listed.some(r => /Forget all decisions/.test(r))).toBe(true);
  await app.screenshot('secret-files-list');
  await app.pickRow('allowed until');
  await app.waitNotification(/Forgot the decision for ".env.cmd"/);
  await app.waitStatus(protectedStatus, 8000);
});

test('UAT-54 [SEC-37] a name from programmingIme.secretFilePatterns is asked like a built-in one; the dialog says why', async () => {
  await pressF1('diary.txt');
  const d = await app.waitDialog();
  expect(d.message).toContain('diary.txt'); expect(d.message).toMatch(/matches your secretFilePatterns setting/);
  await app.page.keyboard.press('Escape');
  await app.dialog.waitFor({state: 'hidden', timeout: 5000});
  expect(await panelCount()).toBe(0);
});

test('UAT-55 [SEC-30] a template and an ordinary source file are not asked: F1 opens at once, the status bar has no lock', async () => {
  for (const file of ['.env.example', 'main.rs']) {
    await app.closeAll();
    await pressF1(file);
    await app.waitPanel();
    expect(await app.dialogVisible()).toBe(false);
    expect(await app.statusText()).not.toMatch(protectedStatus);
  }
});
