'use strict';
// UAT group 7: F1 annotate (docs/F1_ANNOTATE.md; requirement added by the project owner after the design baseline, ids UAT-42..UAT-49).
// F1 with no selection annotates the declaration around the cursor, F1 with a selection annotates exactly the selected lines; the model only
// says "above scope line N put this comment", our code inserts the lines; the proposal is reviewed as hunks, applied as one undo step.
//
// STATUS: written against the proven patterns of 02-ai-f1 / 04-annotate / f2-prompt, NOT RUN on a real VS Code (the work item had no memory
// for it: no Playwright, no VS Code, no UAT run). Syntax-checked only (node tests/run-all.js --syntax). The same behaviour is covered against
// fakes by tests/js/ut-f1-annotate.test.js (ANN-60..ANN-85). The part most likely to need maintenance is highlightedLines(): it reads Monaco's
// own overlay DOM, which is not an API.
// Run it with:  node tests/run-all.js --uat --only-uat "UAT-4[2-9]"
const {test, expect} = require('@playwright/test');
const fs = require('node:fs');
const {VsCode, sleep} = require('../helpers/vscode');
const {startMock} = require('../helpers/mock-provider');

test.describe.configure({mode: 'serial'});
const KEY = 'mock-key-test-123';
let app, mock, behaviour;

// 1-based lines: parse_header is 12-22, the for loop 14-19, impl Parser 7-23 (no comments anywhere: the code fingerprint is "every line that is not a // line")
const RUST = [
  'use std::fmt;', '', 'struct Parser {', '    pos: usize,', '}', '', 'impl Parser {', '    fn new() -> Parser {', '        Parser { pos: 0 }', '    }', '',
  '    fn parse_header(&mut self, input: &str) -> usize {', '        let mut n = 0;', '        for c in input.chars() {', "            if c == ':' {", '                break;', '            }',
  '            n += 1;', '        }', '        self.pos = n;', '        n', '    }', '}', ''].join('\n');
const FILES = {};
for (let i = 1; i <= 9; i++) FILES[`sc${i}.rs`] = RUST;
FILES['secret1.rs'] = 'fn connect() {\n    let password = "hunter2hunter2";\n    login(password);\n}\n';

// a model that behaves: a comment above every second anchorable line of the scope (numbered from 1 within the scope)
const goodReply = ({input}) => ({json: {comments: (input.anchorable || []).filter((_, i) => i % 2 === 0).map(line => ({line, text: `Explains line ${line} of this scope for a beginner.`}))}});
const settingsFile = () => `${app.userDir}\\User\\settings.json`;
const patchSettings = patch => { const s = JSON.parse(fs.readFileSync(settingsFile(), 'utf8')); Object.assign(s, patch); fs.writeFileSync(settingsFile(), JSON.stringify(s, null, 2)); };
const scopeRequests = () => mock.requests.filter(r => r.task === 'scope');

test.beforeAll(async () => {
  mock = await startMock('A', args => behaviour(args));
  behaviour = goodReply;
  app = await VsCode.launch({files: FILES, caFile: mock.certFile, settings: {
    'programmingIme.enabled': true, 'programmingIme.suggestMode': 'explicit', 'programmingIme.f1Mode': 'annotate',   // helpers/vscode.js defaults to "edit"
    'programmingIme.providers': {mockA: {api: 'openai-responses', endpoint: mock.endpoint('openai-responses')}}}});
});
test.afterAll(async () => { await app.screenshot('end-of-group-7'); await app.close(); await mock.close(); });
test.beforeEach(async () => { behaviour = goodReply; mock.reset(); await app.closeAll(); });

// F1 at a line / column, optionally with `select` whole lines selected downwards from there (the selection then ends at column 0 of the next line)
async function openF1(file, {line = 18, col = 1, select = 0} = {}) {
  await app.openFile(file);
  await app.goto(line, col);
  for (let i = 0; i < select; i++) await app.page.keyboard.press('Shift+ArrowDown');
  await app.page.keyboard.press('F1');
  await app.waitPanel();
}
const authorise = async (choice = 'Authorize this request') => { await app.waitDialog(); await app.dialogClick(choice); };
const scopeText = () => app.panelText('#ann-scope');
const hunkLines = async () => app.panelFrame.locator('#panel-annotate fieldset.hunk .diff span.l').evaluateAll(els => els.map(e => ({add: e.classList.contains('add'), del: e.classList.contains('del'), text: e.textContent})));
// the 1-based document lines that carry our highlight decoration (best effort: Monaco renders a whole-line decoration as a .cdr child of the line's overlay row)
async function highlightedLines() {
  return app.page.evaluate(() => {
    const groups = [...document.querySelectorAll('.editor-group-container')];
    const g = groups.find(x => x.querySelector('.monaco-editor .view-lines') && !x.querySelector('iframe.webview')) || groups.find(x => x.querySelector('.monaco-editor .view-lines'));
    if (!g) return [];
    const tops = [...g.querySelectorAll('.monaco-editor .view-lines .view-line')].map(l => parseFloat(l.style.top)).sort((a, b) => a - b);
    const lit = [...g.querySelectorAll('.monaco-editor .view-overlays .cdr')].map(d => parseFloat(d.parentElement.style.top));
    return [...new Set(lit)].map(t => tops.findIndex(x => Math.abs(x - t) < 1) + 1).filter(n => n > 0).sort((a, b) => a - b);
  });
}
const range = (a, b) => Array.from({length: b - a + 1}, (_, i) => a + i);
const withoutComments = text => text.split('\n').filter(l => !/^\s*\/\//.test(l)).join('\n');

test('UAT-42 [ANN-77] AI off: the annotate tab says so, nothing is asked, nothing is sent', async () => {
  await openF1('sc1.rs');
  await app.waitPanelStatus('annotate', /unavailable/);
  expect(await app.panelText('#panel-annotate .status')).toMatch(/AI off/);
  expect(await app.panelFrame.locator('#tab-annotate').getAttribute('aria-selected')).toBe('true');   // annotate is the default tab
  await expect(app.panelFrame.locator('#generate')).toBeDisabled();
  expect(await app.dialogVisible()).toBe(false);
  expect(mock.count()).toBe(0);
  await app.screenshot('annotate-ai-off');
});

test('UAT-43 [ANN-60] no selection inside a function: that function is the scope, the highlight shows it, and the hunks only add comment lines', async () => {
  await app.configureAI({provider: 'mockA', model: 'm-1', key: KEY});
  await openF1('sc2.rs', {line: 18});                                         // inside parse_header, no selection
  const d = await app.waitDialog();                                           // F1 starts the request on its own: the consent appears first
  expect(d.message).toMatch(/COMPLETE current file/); expect(d.message).toMatch(/comments for the chosen scope/);
  await app.screenshot('annotate-consent');
  await app.dialogClick('Authorize this request');
  await app.waitPanelStatus('annotate', /ready/);
  expect(await scopeText()).toBe('Scope: fn parse_header (lines 12-22, 11 lines)');
  expect(await app.panelText('#ann-crumbs')).toBe('fn parse_header > impl Parser > file');
  expect(scopeRequests().length).toBe(1); expect(mock.count()).toBe(1);        // one F1 = one request
  const req = scopeRequests()[0];
  expect(req.input.scope).toEqual({kind: 'declaration', label: 'fn parse_header', startLine: 12, endLine: 22});
  expect(req.input.numbered.split('\n').length).toBe(11); expect(req.input.file).toBe(RUST);
  expect(req.body.max_output_tokens).toBeGreaterThanOrEqual(3000);
  expect(await highlightedLines()).toEqual(range(12, 22));
  // the proposal: comment lines only, nothing removed, nothing written yet
  const lines = await hunkLines();
  expect(lines.filter(l => l.del).length).toBe(0);
  expect(lines.filter(l => l.add).length).toBeGreaterThan(0);
  for (const l of lines.filter(x => x.add)) expect(l.text).toMatch(/^\+ \s*\/\/ Explains line/);
  expect(await app.editorText()).toBe(RUST);
  expect(await app.isActiveTabDirty()).toBe(false);
  await app.screenshot('annotate-proposal');
});

test('UAT-44 [ANN-61] a selection: exactly the selected lines are the scope, whatever is around them', async () => {
  await openF1('sc3.rs', {line: 14, col: 1, select: 5});                      // lines 14-18 (the selection ends at column 0 of line 19)
  await authorise();
  await app.waitPanelStatus('annotate', /ready/);
  expect(await scopeText()).toBe('Selection: lines 14-18 (5 lines)');
  expect(await app.panelText('#ann-crumbs')).toBe('selection > fn parse_header > impl Parser > file');
  const req = scopeRequests()[0];
  expect(req.input.scope).toEqual({kind: 'selection', label: 'selection', startLine: 14, endLine: 18});
  expect(req.input.numbered.split('\n').length).toBe(5);
  expect(await highlightedLines()).toEqual(range(14, 18));
  await app.panelFrame.locator('#apply-annotate').click();
  await expect(app.panelFrame.locator('#banner')).toContainText(/Applied to the editor buffer \(not saved\)/);
  await app.openFile('sc3.rs');
  const afterText = await app.editorText();                                   // (the editor shows the final empty line too: it ends with "\n" like RUST)
  const after = afterText.replace(/\n$/, '').split('\n'), before = RUST.replace(/\n$/, '').split('\n');
  expect(after.slice(0, 13)).toEqual(before.slice(0, 13));                    // everything above the selection is untouched
  expect(after.slice(after.length - (before.length - 18))).toEqual(before.slice(18)); // and everything below it
  expect(after.length).toBeGreaterThan(before.length);
  expect(withoutComments(afterText)).toBe(RUST);
});

test('UAT-45 [ANN-62] Wider moves up the chain without sending anything; Generate then runs the wider scope', async () => {
  await openF1('sc4.rs', {line: 18});
  await authorise();
  await app.waitPanelStatus('annotate', /ready/);
  expect(scopeRequests().length).toBe(1);
  await expect(app.panelFrame.locator('#ann-narrower')).toBeDisabled();
  await app.panelFrame.locator('#ann-wider').click();
  await app.waitPanelStatus('annotate', /idle/);
  expect(await scopeText()).toBe('Scope: impl Parser (lines 7-23, 17 lines)');
  expect(scopeRequests().length).toBe(1);                                      // changing the scope sends nothing
  expect(await highlightedLines()).toEqual(range(7, 23));
  await app.panelFrame.locator('#ann-wider').click();
  expect(await scopeText()).toBe('Whole file: lines 1-23 (23 lines)');
  await expect(app.panelFrame.locator('#ann-wider')).toBeDisabled();           // the end of the chain
  await app.panelFrame.locator('#ann-narrower').click();
  expect(await scopeText()).toBe('Scope: impl Parser (lines 7-23, 17 lines)');
  await app.panelFrame.locator('#generate').click();
  await app.waitPanelStatus('annotate', /ready/);
  expect(scopeRequests().length).toBe(2);
  expect(scopeRequests()[1].input.scope).toEqual({kind: 'declaration', label: 'impl Parser', startLine: 7, endLine: 23});
  await app.screenshot('annotate-wider');
});

test('UAT-46 [ANN-69] Apply writes the selected blocks as ONE undo step: the code is unchanged, Ctrl+Z restores the original, the highlight is gone', async () => {
  await openF1('sc5.rs', {line: 18});
  await authorise();
  await app.waitPanelStatus('annotate', /ready/);
  const ids = await app.panelFrame.locator('#panel-annotate fieldset.hunk').evaluateAll(els => els.map(e => e.getAttribute('data-hunk')));
  expect(ids.length).toBeGreaterThan(1);
  await app.panelFrame.locator(`#cb-annotate-${ids[0]}`).uncheck();            // leave the first block out
  await app.panelFrame.locator('#apply-annotate').click();
  await expect(app.panelFrame.locator('#banner')).toContainText(/Applied to the editor buffer \(not saved\)/);
  expect(await highlightedLines()).toEqual([]);                                // the highlight is cleared after Apply
  await app.openFile('sc5.rs');
  const text = await app.editorText();
  expect(text).toContain('// Explains line 3 of this scope for a beginner.');
  expect(text).not.toContain('// Explains line 1 of this scope');
  expect(withoutComments(text)).toBe(RUST);                                    // the code is the same
  expect(await app.isActiveTabDirty()).toBe(true);
  expect(fs.readFileSync(app.userFile('sc5.rs'), 'utf8')).toBe(RUST);          // disk untouched
  await app.undo();                                                            // ONE Ctrl+Z
  expect(await app.editorText()).toBe(RUST);
  await app.screenshot('annotate-after-undo');
});

test('UAT-47 [ANN-65] declining the consent sends zero requests; the tab waits and Generate asks again', async () => {
  await openF1('sc6.rs', {line: 18});
  await app.waitDialog(); await app.dialogClick('Cancel');
  await sleep(800);
  expect(mock.count()).toBe(0);
  await app.waitPanelStatus('annotate', /idle/);
  expect(await app.dialogVisible()).toBe(false);
  await expect(app.panelFrame.locator('#generate')).toBeEnabled();
  await app.panelFrame.locator('#generate').click();
  await authorise();
  await app.waitPanelStatus('annotate', /ready/);
  expect(scopeRequests().length).toBe(1);
});

test('UAT-48 [ANN-66] secrets: the request carries no secret byte, the comments read <redacted>, the secret stays where it was', async () => {
  behaviour = ({input}) => ({json: {comments: [{line: 1, text: 'Connects with PIME_REDACTED_1 as the credential.'}, {line: 2, text: 'Hands the PIME_REDACTED_1 value to login.'}]}});
  await app.openFile('secret1.rs');
  await app.goto(3, 1);
  await app.page.keyboard.press('F1'); await app.waitPanel();
  await authorise();
  await app.waitPanelStatus('annotate', /ready/);
  const wire = mock.requests.map(r => r.rawBody).join('\n');
  expect(wire).not.toContain('hunter2hunter2'); expect(wire).toContain('PIME_REDACTED_1');
  expect(await app.panelText('#panel-annotate')).toMatch(/redacted before sending/);
  await expect(app.panelFrame.locator('#preview-annotate')).toContainText('<redacted>');
  await expect(app.panelFrame.locator('#preview-annotate')).not.toContainText('PIME_REDACTED_');
  await expect(app.panelFrame.locator('#preview-annotate')).toContainText('let password = "hunter2hunter2";');   // the local text is the original
  await app.panelFrame.locator('#apply-annotate').click();
  await expect(app.panelFrame.locator('#banner')).toContainText(/Applied to the editor buffer/);
  await app.openFile('secret1.rs');
  const text = await app.editorText();
  expect(text).toContain('// Connects with <redacted> as the credential.'); expect(text).not.toContain('PIME_REDACTED_');
  expect((text.match(/hunter2hunter2/g) || []).length).toBe(1);
});

test('UAT-49 [ANN-74] f1Mode=edit is the old F1: whole-file tab first, nothing starts on its own, Generate asks for both requests', async () => {
  patchSettings({'programmingIme.f1Mode': 'edit'}); await sleep(2500);
  await openF1('sc7.rs', {line: 18});
  expect(await app.panelFrame.locator('#tab-full').getAttribute('aria-selected')).toBe('true');
  await sleep(800);
  expect(mock.count()).toBe(0); expect(await app.dialogVisible()).toBe(false);
  expect(await app.panelText('#panel-annotate .status')).toMatch(/Nothing is written|Tick/);
  await app.panelFrame.locator('#tab-annotate').click();
  expect(await scopeText()).toBe('Scope: fn parse_header (lines 12-22, 11 lines)');   // the annotate tab is there, idle
  expect(mock.count()).toBe(0);
  await app.panelFrame.locator('#tab-full').click();
  behaviour = ({task}) => task === 'full' ? {json: {fullFile: RUST.replace('let mut n = 0;', 'let mut n = 0; // counter')}} : {json: {insertCandidates: []}};
  await app.panelFrame.locator('#generate').click();
  const d = await app.waitDialog(); expect(d.message).toMatch(/complete document and a nearby excerpt/); await app.dialogClick('Authorize this request');
  await app.waitPanelStatus('full', /ready/);
  expect(mock.count('full')).toBe(1); expect(mock.count('insert')).toBe(1); expect(scopeRequests().length).toBe(0);
  patchSettings({'programmingIme.f1Mode': 'annotate'}); await sleep(2500);
});
