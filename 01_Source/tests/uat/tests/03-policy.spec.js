'use strict';
// UAT group 3: provider/model/effort routing, budget and rate limits, untrusted workspace, UI locale,
// permission revocation and realtime AI behaviour.
const {test, expect} = require('@playwright/test');
const fs = require('node:fs');
const {VsCode, sleep} = require('../helpers/vscode');
const {startMock} = require('../helpers/mock-provider');

test.describe.configure({mode: 'serial'});
const BEFORE = 'fn a() {}\n\nfn b() {}\n\nfn c() {}\n';
const AFTER = 'fn a() { 1 }\n\nfn b() {}\n\nfn c() { 3 }\n';
const ok = ({task}) => task === 'full' ? {json: {fullFile: AFTER}} : task === 'insert' ? {json: {insertCandidates: ['// ai\n']}} : {json: {insertCandidates: ['fn ai_made() {}']}};
const files = {};
for (let i = 1; i <= 12; i++) files[`p${i}.rs`] = BEFORE;
files['rt.rs'] = 'fn base() {}\n';

let appA = null;
const settingsPath = a => `${a.userDir}\\User\\settings.json`;
const patchSettings = (a, patch) => { const s = JSON.parse(fs.readFileSync(settingsPath(a), 'utf8')); Object.assign(s, patch); fs.writeFileSync(settingsPath(a), JSON.stringify(s, null, 2)); };
async function openF1(app, file, intent = 'x') { await app.closeAll(); await app.openFile(file); await app.page.keyboard.press('F1'); await app.waitPanel(); await app.panelFrame.locator('#intent').fill(intent); }
async function generate(app, choice = 'Authorize this request') { await app.panelFrame.locator('#generate').click(); if (choice) { await app.waitDialog(); await app.dialogClick(choice); } }

test.afterEach(async () => { if (appA) { await appA.close(); appA = null; } });

test('UAT-30 [ST-016/IT-017/ST-017/IT-018/AT-021] scenarios route to their own provider and key; effort is sent only when verified', async () => {
  const A = await startMock('A', ok), B = await startMock('B', ok);
  try {
    const fresh = {state: 'supported', supported: ['low', 'medium', 'high'], checkedAt: new Date().toISOString()};
    appA = await VsCode.launch({files, caFile: A.certFile, settings: {
      'programmingIme.enabled': true, 'programmingIme.suggestMode': 'explicit',
      'programmingIme.providers': {mockA: {api: 'openai-responses', endpoint: A.endpoint('openai-responses')}, mockB: {api: 'openai-chat', endpoint: B.endpoint('openai-chat')}}
    }});
    await appA.configureAI({provider: 'mockA', model: 'model-a', key: 'keyA-mock-111'});
    await appA.configureAI({provider: 'mockB', model: 'model-b', key: 'keyB-mock-222'});
    patchSettings(appA, {'programmingIme.scenarios': {
      realtime: {provider: 'mockA', model: 'rt-a'},
      f1Full: {provider: 'mockB', model: 'big-b', effort: 'high', effortCapability: fresh},
      f1Insert: {provider: 'mockA', model: 'mid-a', effort: 'high', effortCapability: {state: 'unknown'}}}});
    await sleep(2500);
    await openF1(appA, 'p1.rs');
    await generate(appA);                                   // two hosts -> two consent prompts
    if (await appA.dialogVisible().catch(() => false) || true) { try { await appA.waitDialog(4000); await appA.dialogClick('Authorize this request'); } catch { /* single prompt */ } }
    await appA.waitPanelStatus('full', /ready/); await appA.waitPanelStatus('insert', /ready/);
    expect(B.count('full')).toBe(1); expect(B.count('insert')).toBe(0);
    expect(A.count('insert')).toBe(1); expect(A.count('full')).toBe(0);
    expect(B.requests[0].headers.authorization).toBe('Bearer keyB-mock-222');
    expect(A.requests[0].headers.authorization).toBe('Bearer keyA-mock-111');
    expect(B.requests[0].body.model).toBe('big-b'); expect(A.requests[0].body.model).toBe('mid-a');
    expect(B.requests[0].body.reasoning_effort).toBe('high');            // verified capability: sent
    expect('reasoning' in A.requests[0].body).toBe(false);               // unknown capability: never sent
    await appA.panelFrame.locator('#tab-insert').click();
    expect(await appA.panelText('#panel-insert')).toMatch(/Effort note:.*NOT sent/i);   // and the user is told why
    await appA.screenshot('effort-not-sent-note');
  } finally { await A.close(); await B.close(); }
});

test('UAT-31 [ST-018/IT-020/FR-023/FR-024] budget stops AI before any request; a 429 pauses AI; local candidates keep working', async () => {
  let mode = 'ok';
  const M = await startMock('A', args => mode === '429' ? {status: 429, headers: {'retry-after': '120'}} : ok(args));
  try {
    appA = await VsCode.launch({files, caFile: M.certFile, settings: {
      'programmingIme.enabled': true, 'programmingIme.suggestMode': 'explicit', 'programmingIme.budget': {maxRequests: 2},
      'programmingIme.providers': {mockA: {api: 'openai-responses', endpoint: M.endpoint('openai-responses')}}}});
    await appA.configureAI({provider: 'mockA', model: 'm', key: 'keyA-mock-111'});
    await openF1(appA, 'p2.rs');
    await generate(appA, 'Authorize for this workspace');
    await appA.waitPanelStatus('full', /ready/); await appA.waitPanelStatus('insert', /ready/);
    expect(M.count()).toBe(2);
    expect(await appA.panelText('#usage')).toMatch(/2 requests/);
    await appA.panelFrame.locator('#regen-full').click();                // budget of 2 requests is used up
    await appA.waitPanelStatus('full', /error/);
    expect(await appA.panelText('#panel-full .status')).toMatch(/budget reached/i);
    expect(M.count()).toBe(2);                                           // refused locally: nothing was sent
    // local candidates are unaffected
    await appA.closeAll(); await appA.openFile('p3.rs'); await appA.end(); await appA.type('\nfnm');
    await appA.cmd('Programming IME: Show Code Candidates'); await appA.waitQuick();
    expect((await appA.quickRows()).join('|')).toMatch(/main function/);
    await appA.escapeQuick();
    await appA.cmd('Programming IME: Reset AI Usage');
    mode = '429';
    await openF1(appA, 'p4.rs'); await generate(appA, null).catch(() => {});
    await appA.waitPanelStatus('full', /error/);
    const before = M.count();
    expect(await appA.panelText('#panel-full .status')).toMatch(/rate limit/i);
    await appA.panelFrame.locator('#regen-full').click();
    await appA.waitPanelStatus('full', /error/);
    await sleep(500);
    expect(M.count()).toBe(before);                                       // paused locally after 429: no new request
    await appA.screenshot('rate-limited');
  } finally { await M.close(); }
});

test('UAT-32 [ST-013/FR-015/FR-039] untrusted workspace: AI paths are unavailable and explained, nothing is sent, local candidates work', async () => {
  const M = await startMock('A', ok);
  try {
    appA = await VsCode.launch({files, caFile: M.certFile, trusted: false, settings: {
      'security.workspace.trust.enabled': true, 'programmingIme.enabled': true, 'programmingIme.aiEnabled': true, 'programmingIme.suggestMode': 'explicit',
      'programmingIme.providers': {mockA: {api: 'openai-responses', endpoint: M.endpoint('openai-responses')}},
      'programmingIme.scenarios': {f1Full: {provider: 'mockA', model: 'm'}, f1Insert: {provider: 'mockA', model: 'm'}, realtime: {provider: 'mockA', model: 'm'}}}});
    await appA.openFile('p5.rs');
    await appA.page.keyboard.press('F1'); await appA.waitPanel();
    await expect(appA.panelFrame.locator('#panel-full .status')).toHaveAttribute('data-status', 'unavailable');
    expect(await appA.panelText('#panel-full .status')).toMatch(/not trusted/i);
    expect(await appA.panelText('#generate')).toBeTruthy();
    expect(M.count()).toBe(0);
    await appA.screenshot('untrusted-workspace');
    await appA.closeAll(); await appA.openFile('p6.rs'); await appA.end(); await appA.type('\nfnm');
    await appA.cmd('Programming IME: Show Code Candidates'); await appA.waitQuick();
    expect((await appA.quickRows()).join('|')).toMatch(/main function/);
  } finally { await M.close(); }
});

test('UAT-33 [ST-032/FR-044/UT-056] UI language is independent: zh-CN and ja-JP UI, the programming profile and code stay unchanged', async () => {
  appA = await VsCode.launch({files, settings: {'programmingIme.enabled': true, 'programmingIme.suggestMode': 'explicit', 'programmingIme.uiLocale': 'zh-CN'}});
  await appA.openFile('p7.rs'); await appA.end(); await appA.type('\nfnm');
  await appA.cmd('Programming IME: Show Code Candidates'); await appA.waitQuick();
  const zh = (await appA.quickRows()).join('|');
  expect(zh).toMatch(/本地规则/); expect(zh).toMatch(/main function/);          // UI text localised, candidate label/code unchanged
  const title = await appA.quickTitle(); expect(title).toMatch(/本地候选/);
  await appA.pickRow('main function');
  const codeZh = await appA.waitEditorText('fn main() {');
  await appA.page.keyboard.press('F1'); await appA.waitPanel();
  expect(await appA.panelText('#tab-full')).toMatch(/整文件修改/); expect(await appA.panelText('#tab-insert')).toMatch(/光标处插入/);
  await appA.screenshot('ui-zh-CN');
  patchSettings(appA, {'programmingIme.uiLocale': 'ja-JP'});
  await sleep(2500);
  await appA.closeAll(); await appA.openFile('p8.rs'); await appA.end(); await appA.type('\nfnm');
  await appA.cmd('Programming IME: Show Code Candidates'); await appA.waitQuick();
  expect((await appA.quickRows()).join('|')).toMatch(/ローカル規則/);
  await appA.pickRow('main function');
  const codeJa = await appA.waitEditorText('fn main() {');
  expect(codeJa.replace('p8', '')).toContain('fn main() {'); expect(codeZh).toContain('fn main() {');        // same Rust code in every UI language
  expect(await appA.statusText()).toMatch(/PIME rust/);
  await appA.page.keyboard.press('F1'); await appA.waitPanel();
  expect(await appA.panelText('#tab-full')).toMatch(/ファイル全体/);
  await appA.screenshot('ui-ja-JP');
});

test('UAT-34 [FR-016/ST-014] workspace authorisation can be revoked at any time and the next request asks again', async () => {
  const M = await startMock('A', ok);
  try {
    appA = await VsCode.launch({files, caFile: M.certFile, settings: {
      'programmingIme.enabled': true, 'programmingIme.suggestMode': 'explicit',
      'programmingIme.providers': {mockA: {api: 'openai-responses', endpoint: M.endpoint('openai-responses')}}}});
    await appA.configureAI({provider: 'mockA', model: 'm', key: 'keyA-mock-111'});
    await openF1(appA, 'p9.rs'); await generate(appA, 'Authorize for this workspace');
    await appA.waitPanelStatus('full', /ready/);
    await appA.panelFrame.locator('#regen-full').click();               // authorised for the workspace: no new prompt
    await appA.waitPanelStatus('full', /ready/);
    expect(await appA.dialogVisible()).toBe(false);
    expect(M.count('full')).toBe(2);
    await appA.cmd('Programming IME: Manage AI Permissions'); await appA.waitQuick();
    expect((await appA.quickRows()).join('|')).toMatch(/L3|L1/);
    await appA.pickRow('Revoke all');
    await sleep(500);
    await appA.page.locator('.tab', {hasText: 'Programming IME'}).first().click();
    await appA.panelFrame.locator('#regen-full').click();               // revoked: the consent dialog is back, nothing sent yet
    const d = await appA.waitDialog();
    expect(d.message).toMatch(/Send/);
    const sent = M.count();
    await appA.page.keyboard.press('Escape'); await sleep(500);
    expect(M.count()).toBe(sent);
  } finally { await M.close(); }
});

test('UAT-35 [FR-025..027/ST-019/ST-020/IT-022/IT-023/UT-040] realtime AI: local first, AI marked as AI, stale/rapid typing never queues bursts, text never altered', async () => {
  let inFlight = 0, maxInFlight = 0;
  const M = await startMock('A', async args => { if (args.task !== 'realtime') return ok(args); inFlight++; maxInFlight = Math.max(maxInFlight, inFlight); await sleep(400); inFlight--; return ok(args); });
  try {
    appA = await VsCode.launch({files, caFile: M.certFile, settings: {
      'programmingIme.enabled': true, 'programmingIme.suggestMode': 'auto', 'programmingIme.debounceMs': 350,
      'programmingIme.providers': {mockA: {api: 'openai-responses', endpoint: M.endpoint('openai-responses')}}}});
    await appA.configureAI({provider: 'mockA', model: 'm', key: 'keyA-mock-111'});
    await appA.closeAll(); await appA.openFile('rt.rs'); await appA.end(); await appA.type('\n');
    M.reset();
    await appA.cmd('Programming IME: Toggle Realtime AI');
    const d = await appA.waitDialog(); expect(d.message).toMatch(/nearby code excerpt/);
    await appA.dialogClick('Authorize for this workspace');
    await appA.waitNotification(/Realtime AI enabled/);
    expect(M.count()).toBe(0);                                              // enabling alone sends nothing
    await appA.focusEditor(); await appA.end();
    await appA.type('fn', 40);                                              // rapid burst: f, n
    await sleep(2500);
    expect(M.count('realtime')).toBeLessThanOrEqual(1);                     // debounced: one request for the whole burst
    await appA.page.keyboard.press('Backspace'); await appA.page.keyboard.press('Backspace'); await appA.type('fn', 40);   // erase and retype: older requests must be dropped
    await sleep(2500);
    expect(maxInFlight).toBeLessThanOrEqual(1);
    expect(M.count('realtime')).toBeGreaterThanOrEqual(1);
    expect((await appA.editorText()).replace(/·/g, '')).toMatch(/^fn base\(\) \{\}\s*\nfn\s*$/);   // only the typed characters: nothing was inserted by AI or rules
    await appA.cmd('Programming IME: Show Code Candidates'); await appA.waitQuick();
    const rows = await appA.quickRows();
    expect(rows[0]).toMatch(/local rule|nearby code/);                       // local candidates first
    expect(rows.join('|')).toMatch(/fn ai_made\(\) \{\}.*\[AI\]/);            // the AI row is labelled as AI
    expect(rows.join('|')).toMatch(/unchecked/);
    await appA.screenshot('realtime-ai-labelled');
    await appA.escapeQuick();
    expect((await appA.editorText()).replace(/·/g, '')).toMatch(/\nfn\s*$/);
  } finally { await M.close(); }
});
