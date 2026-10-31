'use strict';
// UAT group 6: the F2 prompt (docs/F2_PROMPT.md; requirement added by the project owner after the design baseline, test ids PRM-xx).
// Select text or code, press F2: a complete 5W1H prompt lands on the clipboard; nothing to optimise -> a bottom-right notice that goes away
// on the next input and leaves the clipboard alone; AI not available -> the UNREVIEWED template; secrets never leave and are never copied.
//
// STATUS: written against the proven patterns of 02-ai-f1 / 04-annotate, NOT YET RUN on a real VS Code (the work item had no memory for it).
// Run it with:  node tests/run-all.js --uat --only-uat "UAT-3[6-9]|UAT-4[01]"
//
// The clipboard is the REAL operating-system clipboard (Windows PowerShell Get-Clipboard / Set-Clipboard): VS Code writes it through Electron, so
// reading it back from the test process is the honest check. The spec saves the clipboard text at the start and restores it at the end;
// it needs an interactive Windows desktop session (the same one that runs VS Code). PowerShell is started with windowsHide: a console window that
// took the focus would also dismiss the notice (it closes when VS Code loses focus).
const {test, expect} = require('@playwright/test');
const fs = require('node:fs');
const {spawnSync} = require('node:child_process');
const {VsCode, sleep} = require('../helpers/vscode');
const {startMock} = require('../helpers/mock-provider');

test.describe.configure({mode: 'serial'});
const KEY = 'mock-key-test-123';
const SENTINEL = 'CLIPBOARD-BEFORE-F2';
const WORKSPACE_BUTTON = 'Authorize for this workspace';
const HEADS = ['Who', 'What', 'When', 'Where', 'Why', 'How'];
let app, mock, behaviour, savedClipboard = '';

// ---------------------------------------------------------------- clipboard (OS level, Windows PowerShell 5.1)
const PS = ['-NoProfile', '-NonInteractive', '-STA'];
function readClipboard() {
  const r = spawnSync('powershell.exe', [...PS, '-Command', '[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false; $t = Get-Clipboard -Raw; if ($null -ne $t) { [Console]::Out.Write($t) }'], {encoding: 'buffer', timeout: 20000, windowsHide: true});
  return r.stdout.toString('utf8').split(String.fromCharCode(0xfeff)).join('').replace(/\r\n/g, '\n');
}
function writeClipboard(text) {
  spawnSync('powershell.exe', [...PS, '-Command', 'Set-Clipboard -Value $env:PIME_UAT_CLIP'], {env: {...process.env, PIME_UAT_CLIP: text}, timeout: 20000, windowsHide: true});
}
async function waitClipboard(pred, timeout = 20000) {
  const end = Date.now() + timeout; let last = '';
  while (Date.now() < end) { last = readClipboard(); if (pred(last)) return last; await sleep(400); }
  throw new Error(`the clipboard never matched; last: ${JSON.stringify(last.slice(0, 200))}`);
}

// ---------------------------------------------------------------- fixtures
const RUST = 'fn read_port(s: &str) -> u16 {\n    s.parse::<u16>().unwrap()\n}\n\nfn main() {\n    println!("{}", read_port("80"));\n}\n';
const PROSE = 'We need a login page for our customers, and it should be fast.';
const FILES = {
  'req1.md': `${PROSE}\n`, 'req2.md': `${PROSE}\n`, 'req3.md': `${PROSE}\n`, 'req4.md': `${PROSE}\n`,
  'code1.rs': RUST, 'code2.rs': RUST, 'code3.rs': RUST, 'code4.rs': RUST,
  'secret1.rs': 'fn connect() {\n    let key = "sk-abcdefghijklmnopqrstuvwxyz0123";\n    login(key);\n}\n',
  'secret2.rs': 'fn connect() {\n    let key = "sk-abcdefghijklmnopqrstuvwxyz0123";\n    login(key);\n}\n',
  'secret3.rs': 'fn connect() {\n    let key = "sk-abcdefghijklmnopqrstuvwxyz0123";\n    login(key);\n}\n',
  'medium1.rs': 'fn connect() {\n    let password = "hunter2hunter2";\n    login(password);\n}\n',
  'rename1.ts': 'function alpha() {\n  return 1;\n}\nalpha();\n'
};
const FIELDS = {who: 'A Rust engineer.', what: 'Replace the unwrap with error handling.', when: 'Before the next release.', where: 'read_port in main.rs', why: 'A bad port string aborts the program.', how: 'Return a Result and propagate it with the question mark operator.'};
const CODE_REPLY = {needsOptimization: true, findings: [{line: 2, evidence: 'parse::<u16>().unwrap()', issue: 'unwrap panics on malformed input', impact: 'the process aborts', severity: 'high'}], w5h1: FIELDS};
const PROSE_REPLY = {needsOptimization: true, findings: [{element: 'when', issue: 'no deadline is given'}], w5h1: {who: 'A web developer.', what: 'Build a fast customer login page.', when: 'Ask the user.', where: 'The customer portal.', why: 'Customers must sign in.', how: 'Describe the steps and the acceptance criteria.'}};
const NO_CHANGE = {needsOptimization: false, findings: []};
const f2Requests = () => mock.requests.filter(r => r.input && (r.input.kind === 'code' || r.input.kind === 'prose')); // the mock labels them task "unknown"
const settingsFile = () => `${app.userDir}\\User\\settings.json`;
const patchSettings = patch => { const s = JSON.parse(fs.readFileSync(settingsFile(), 'utf8')); Object.assign(s, patch); fs.writeFileSync(settingsFile(), JSON.stringify(s, null, 2)); };
const toast = text => app.page.locator('.notifications-toasts .notification-toast', {hasText: text}).first();
const increasing = xs => xs.every((x, i) => x >= 0 && (i === 0 || x > xs[i - 1]));

test.beforeAll(async () => {
  savedClipboard = readClipboard();
  mock = await startMock('F2', args => behaviour(args));
  behaviour = () => ({json: NO_CHANGE});
  app = await VsCode.launch({files: FILES, caFile: mock.certFile, settings: {
    'programmingIme.enabled': true, 'programmingIme.suggestMode': 'explicit',
    'programmingIme.providers': {mockA: {api: 'openai-responses', endpoint: mock.endpoint('openai-responses')}}}});
  await app.configureAI({provider: 'mockA', model: 'm-1', key: KEY});
});
test.afterAll(async () => { await app.screenshot('end-of-group-6'); await app.close(); await mock.close(); writeClipboard(savedClipboard); });
test.beforeEach(async () => { behaviour = () => ({json: NO_CHANGE}); mock.reset(); writeClipboard(SENTINEL); await app.closeAll(); });

async function selectAllIn(file) { await app.openFile(file); await app.page.keyboard.press('Control+A'); }
// the first F2 of a scope asks for the consent; later ones find the workspace grant. Returns the dialog (or null when none was asked).
// Only a CONSENT dialog is answered here; any other dialog (the sensitive-data question) is left open for the caller's own waitDialog().
async function consentIfAsked(re) {
  let d = null;
  try { d = await app.waitDialog(3000); } catch { return null; }
  if (!/Send the COMPLETE|Send a nearby/.test(d.message)) return null;
  if (re) expect(d.message).toMatch(re);
  await app.dialogClick(WORKSPACE_BUTTON);
  return d;
}

test('UAT-36 [PRM-40] a prose selection + F2: the clipboard gets the improved 5W1H prompt (six headings in order) after a consent that names the receiver', async () => {
  behaviour = () => ({json: PROSE_REPLY});
  await selectAllIn('req1.md');
  await app.page.keyboard.press('F2');
  const d = await app.waitDialog();
  expect(d.message).toMatch(/nearby code excerpt/); expect(d.message).toContain('127.0.0.1');     // prose: only the selection (scope L1)
  await app.screenshot('f2-prose-consent');
  await app.dialogClick(WORKSPACE_BUTTON);
  const clip = await waitClipboard(t => t !== SENTINEL);
  const at = HEADS.map(h => clip.indexOf(`## ${h}\n`));
  expect(increasing(at), clip).toBe(true);
  expect(clip).toContain('Build a fast customer login page.'); expect(clip).toContain('no deadline is given'); expect(clip).toContain(PROSE);
  expect(clip).toMatch(/^Carry out the task described by the 5W1H below/);
  await app.waitNotification(/Copied \d+ characters: paste it into your AI assistant/);
  const reqs = f2Requests();
  expect(reqs.length).toBe(1); expect(mock.count()).toBe(1);
  expect(reqs[0].input.kind).toBe('prose'); expect(reqs[0].input.selection.text.trim()).toBe(PROSE);
  expect(reqs[0].input.numbered).toBeUndefined();                                                   // no file part in a prose request
  await app.screenshot('f2-prose-copied');
});

test('UAT-37 [PRM-41] a code selection + F2: the request carries the whole file, the clipboard has the findings with their checked evidence', async () => {
  behaviour = () => ({json: CODE_REPLY});
  await app.openFile('code1.rs');
  await app.goto(1, 1);
  for (let i = 0; i < 3; i++) await app.page.keyboard.press('Shift+ArrowDown');                     // lines 1-3: the first function
  await app.page.keyboard.press('F2');
  const d = await app.waitDialog();
  expect(d.message).toMatch(/COMPLETE current document/);                                            // code: the file goes to the provider (scope L3)
  await app.screenshot('f2-code-consent');
  await app.dialogClick(WORKSPACE_BUTTON);
  const clip = await waitClipboard(t => t !== SENTINEL);
  expect(increasing(HEADS.map(h => clip.indexOf(`## ${h}\n`))), clip).toBe(true);
  expect(clip).toContain('line 2 [high]: unwrap panics on malformed input'); expect(clip).toContain('Evidence: `parse::<u16>().unwrap()`');
  expect(clip).toContain('```rust'); expect(clip).toContain('Return a Result and propagate it'); expect(clip).toMatch(/Selected lines: 1-3 of 8/);
  const req = f2Requests()[0];
  expect(req.input.kind).toBe('code');
  expect(req.input.file).toContain('fn main()');                                                     // the file around the selection, not just the selection
  expect(req.input.numbered).toMatch(/^5\| fn main\(\) \{/m);
  expect(req.input.selection.startLine).toBe(1); expect(req.input.selection.endLine).toBe(3);
  expect(req.input.fileName).toBe('code1.rs');                                                       // the base name only, no local path
  expect(req.rawBody).not.toContain(app.ws.replace(/\\/g, '\\\\'));
  expect(mock.count()).toBe(1);
  await app.waitNotification(/Copied \d+ characters/);
});

test('UAT-38 [PRM-42] nothing to optimise: a bottom-right notice with the right text, the clipboard unchanged, gone after the next typed character', async () => {
  behaviour = () => ({json: NO_CHANGE});
  await app.openFile('code2.rs');
  await app.page.keyboard.press('Control+A');
  await app.page.keyboard.press('F2');
  await consentIfAsked(/COMPLETE current document/);
  const t = toast(/No optimisation needed/);
  await t.waitFor({state: 'visible', timeout: 20000});
  expect((await t.innerText()).replace(/\s+/g, ' ')).toMatch(/No optimisation needed: the AI found no verified problem in the selected code\. The clipboard was not changed\./);
  // bottom-right of the VS Code window (box in page coordinates against the window size)
  const box = await t.boundingBox();
  const win = await app.page.evaluate(() => ({w: window.innerWidth, h: window.innerHeight}));
  expect(box.x).toBeGreaterThan(win.w * 0.5); expect(box.x + box.width).toBeGreaterThan(win.w - 90); expect(box.x + box.width).toBeLessThanOrEqual(win.w + 1);
  expect(box.y).toBeGreaterThan(win.h * 0.45); expect(box.y + box.height).toBeLessThanOrEqual(win.h + 1);
  await app.screenshot('f2-no-optimisation-toast');
  expect(f2Requests().length).toBe(1);                                                               // (the clipboard is read after the toast is gone: no process is started while it is on screen)
  await sleep(1300);                                                                                 // past the 600 ms grace period and VS Code's minimum display time
  await expect(t).toBeVisible();
  await app.page.keyboard.type('x');                                                                 // the next input
  await expect(t).toBeHidden({timeout: 8000});
  expect(readClipboard()).toBe(SENTINEL);                                                            // never overwritten by "nothing to do"
  expect((await app.notifications()).some(n => /No optimisation needed/.test(n))).toBe(false);
});

test('UAT-39 [PRM-43] AI off: the UNREVIEWED template is copied with the reason in the notice and not one request is sent', async () => {
  patchSettings({'programmingIme.aiEnabled': false}); await sleep(2500);
  try {
    await selectAllIn('code3.rs');
    await app.page.keyboard.press('F2');
    const clip = await waitClipboard(t => t !== SENTINEL);
    expect(clip.split('\n')[0]).toBe('UNREVIEWED TEMPLATE: no AI looked at this selection and nothing was analysed. Answer the bracketed questions below, then send it.');
    expect(increasing(HEADS.map(h => clip.indexOf(`## ${h}\n`))), clip).toBe(true);
    expect(clip).toContain('fn read_port'); expect(clip).toContain('<state who should answer');
    const n = await app.waitNotification(/UNREVIEWED template/);
    expect(n).toMatch(/AI is off/); expect(n).toMatch(/Nothing was analysed/);
    expect(await app.dialogVisible()).toBe(false);                                                   // nothing is sent, so nothing to authorise
    expect(mock.count()).toBe(0);
    await app.screenshot('f2-template-notice');
  } finally { patchSettings({'programmingIme.aiEnabled': true}); await sleep(2500); }
});

test('UAT-40 [PRM-44] F2 without a selection is still Rename: our command does not run (no notice, no dialog, no request, clipboard unchanged)', async () => {
  await app.openFile('rename1.ts');
  await app.goto(1, 12);                                                                             // inside "alpha", empty selection
  let renamed = false;
  for (let i = 0; i < 12 && !renamed; i++) {                                                         // the TypeScript server needs a few seconds before it offers Rename
    await app.page.keyboard.press('F2');
    renamed = await app.page.locator('.rename-box').first().waitFor({state: 'visible', timeout: 4000}).then(() => true, () => false);
  }
  expect(await app.dialogVisible()).toBe(false);
  expect(mock.count()).toBe(0); expect(readClipboard()).toBe(SENTINEL);
  expect((await app.notifications()).some(n => /Select some text or code first/.test(n))).toBe(false);   // our command would have said this
  if (renamed) { await app.screenshot('f2-rename-box'); await app.page.keyboard.press('Escape'); }
  test.skip(!renamed, 'the TypeScript language server offered no Rename within 48 s on this machine (the no-selection checks above did run)');
});

test('UAT-41 [PRM-45] secrets: a high-risk file is never sent or copied without the user, "Redact and send" sends no secret byte, medium risk is redacted silently', async () => {
  // the user closes the sensitive-data dialog: nothing is sent, nothing is copied
  await selectAllIn('secret1.rs');
  await app.page.keyboard.press('F2');
  await consentIfAsked(/COMPLETE current document/);
  const d = await app.waitDialog();
  expect(d.message).toMatch(/Possible secrets found/); expect(d.buttons.join('|')).toMatch(/Redact and send/);
  await app.dialogClick('Cancel');
  await app.waitNotification(/Looks sensitive.*nothing was sent to an AI and nothing was copied/);
  expect(mock.count()).toBe(0); expect(readClipboard()).toBe(SENTINEL);

  // "Redact and send": the provider sees markers only, the clipboard prompt keeps them
  await app.closeAll(); mock.reset();
  behaviour = () => ({json: {needsOptimization: true, findings: [{line: 2, evidence: 'let key = "PIME_REDACTED_1";', issue: 'a key is hard-coded', impact: 'anyone with the source has it', severity: 'high'}], w5h1: FIELDS}});
  await selectAllIn('secret2.rs');
  await app.page.keyboard.press('F2');
  await consentIfAsked();
  await app.waitDialog(); await app.dialogClick('Redact and send');
  const clip = await waitClipboard(t => t !== SENTINEL);
  const wire = mock.requests.map(r => r.rawBody).join('\n');
  expect(wire).not.toContain('sk-abcdefghijklmnopqrstuvwxyz0123'); expect(wire).toContain('PIME_REDACTED_1');
  expect(clip).not.toContain('sk-abcdef'); expect(clip).toContain('let key = "PIME_REDACTED_1";');
  expect(clip).toMatch(/Scope: 1 secret-like value\(s\) replaced by PIME_REDACTED_n markers \(not restored\)/);
  await app.screenshot('f2-redacted');

  // medium risk (a password assignment) is redacted without a question, as in F1
  await app.closeAll(); mock.reset(); writeClipboard(SENTINEL);
  behaviour = () => ({json: NO_CHANGE});
  await selectAllIn('medium1.rs');
  await app.page.keyboard.press('F2');
  await consentIfAsked();
  await app.waitNotification(/No optimisation needed/);
  expect(await app.dialogVisible()).toBe(false);
  expect(mock.requests.map(r => r.rawBody).join('\n')).not.toContain('hunter2hunter2');

  // AI off: the offline template is a clipboard write too, so the high-risk selection is refused there as well
  await app.closeAll(); mock.reset(); writeClipboard(SENTINEL);
  patchSettings({'programmingIme.aiEnabled': false}); await sleep(2500);
  try {
    await selectAllIn('secret3.rs');
    await app.page.keyboard.press('F2');
    await app.waitNotification(/nothing was sent to an AI and nothing was copied/);
    expect(mock.count()).toBe(0); expect(readClipboard()).toBe(SENTINEL); expect(await app.dialogVisible()).toBe(false);
  } finally { patchSettings({'programmingIme.aiEnabled': true}); await sleep(2500); }
});
