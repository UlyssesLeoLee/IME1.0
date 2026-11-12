'use strict';
// UAT group 08a: the local candidate overlay (IT-006), the syntax context of the engine per language (ST-008)
// and the language drop-down of the seven built-in languages (ST-031).
// Everything here runs on the local rule engine: no AI provider, no network, no request.
const {test, expect} = require('@playwright/test');
const path = require('node:path');
const {VsCode, sleep} = require('../helpers/vscode');
const {quickLabels, quickDescriptions, indentOf, openCandidates, pickLanguage} = require('../helpers/08-support');
// The engine module the extension itself runs (src/core/languages.js). The expected order of the candidates is
// computed with the SAME code, so "the overlay shows the order of the engine" is a real comparison and not a
// restatement of the expectation. (With PIME_ENGINE=rust the core answers instead; the order check is skipped then.)
const engine = require(path.resolve(__dirname, '..', '..', '..', 'editor-bridges', 'vscode', 'src', 'core', 'languages.js'));
const RUST_ENGINE = process.env.PIME_ENGINE === 'rust';

test.describe.configure({mode: 'serial'});
let app;
const FILES = {
  'ctx.rs': 'fn existing() {}\n',                                                // statement position
  'expr.rs': 'fn existing() {}\nlet value = ',                                    // expression position (the line ends with '=')
  'App.tsx': 'const a = 1;\n',                                                    // TSX (JSX) mode
  'plain.ts': 'const a = 1;\n',                                                   // TS mode
  'cls.py': 'class Widget:\n    def __init__(self):\n        pass\n    ',           // inside a class body (4 spaces)
  'mod.py': 'x = 1\n',                                                            // module level
  'lang.rs': 'fn existing() {}\n'                                                 // only for the language drop-down
};
const BASE = {'programmingIme.enabled': true, 'programmingIme.suggestMode': 'explicit'};

test.beforeAll(async () => { app = await VsCode.launch({files: FILES, settings: BASE}); });
test.afterAll(async () => { await app.screenshot('end-of-group-08a'); await app.close(); });
test.beforeEach(async () => { await app.closeAll(); });

// The candidate overlay of the bridge: the command is the same one as Ctrl+Alt+Space.

test('UAT-56 [IT-006] the rule candidates reach the candidate overlay: source, language and order are the engine order, the text stays unchanged', async () => {
  await app.openFile('ctx.rs');
  await app.end();
  await app.type('\nfn');
  const before = await app.editorText();
  const labels = await openCandidates(app);
  // the language of the overlay and the prefix it belongs to are in its title
  const title = await app.quickTitle();
  expect(title).toMatch(/rust/);
  expect(title).toMatch(/fn/);
  // every row names its source and its validation state; with the AI off no row is an AI row
  const descriptions = await quickDescriptions(app);
  expect(descriptions.length).toBe(labels.length);
  for (const d of descriptions) expect(d).toMatch(/^\[(local rule|nearby code|your rule|AI)\] /);
  expect(descriptions.join(' | ')).not.toMatch(/\[AI\]/);
  // the order is the order of the engine (the same module the extension runs)
  if (!RUST_ENGINE) {
    const expected = engine.predict({profile: 'rust', prefix: 'fn', before: 'fn existing() {}\n', after: '', dialect: engine.defaultDialect('rust'), limit: 6});
    expect(labels).toEqual(expected.map(c => c.label));
  }
  expect(labels.join(' | ')).toMatch(/function/);
  // the document is not touched while the overlay is open, and Esc writes nothing
  expect(await app.editorText()).toBe(before);
  await app.screenshot('it-006-candidate-overlay');
  await app.escapeQuick();
  await sleep(300);
  expect(await app.editorText()).toBe(before);
});

test('UAT-57 [ST-008] the engine reads the syntax context: Rust statement vs expression, TSX vs TS, Python class body vs module level', async () => {
  // Rust: the same prefix in an expression position must not offer the statement candidate
  await app.openFile('ctx.rs'); await app.end(); await app.type('\nfnm');
  const rustStatement = await openCandidates(app);
  expect(rustStatement.join(' | ')).toMatch(/main function/);
  await app.escapeQuick();
  await app.closeAll();
  await app.openFile('expr.rs'); await app.end(); await app.type('fnm');
  await app.cmd('Programming IME: Show Code Candidates');
  await sleep(1200);
  expect(await app.quickVisible()).toBe(false);          // no statement candidate in an expression position
  expect(await app.editorText()).toContain('let value = fnm');
  await app.screenshot('st-008-rust-expression');
  // TypeScript: the TSX mode uses the JSX-safe form <T,>, the TS mode the bare <T> that TSX would read as JSX
  await app.closeAll();
  await app.openFile('App.tsx'); await app.end(); await app.type('\ngarrow');
  const tsxLabels = await openCandidates(app);
  expect(tsxLabels.join(' | ')).toMatch(/TSX/);
  await app.pickRow('generic arrow function (TSX: trailing comma)');
  const tsxText = await app.waitEditorText('<T,>');
  expect(tsxText).toContain('= <T,>(value: T): T => value;');
  expect(tsxText).not.toMatch(/<T>\(value/);
  await app.closeAll();
  await app.openFile('plain.ts'); await app.end(); await app.type('\ngarrow');
  const tsLabels = await openCandidates(app);
  expect(tsLabels.join(' | ')).not.toMatch(/TSX/);
  await app.pickRow('generic arrow function');
  const tsText = await app.waitEditorText('<T>');
  expect(tsText).toContain('= <T>(value: T): T => value;');
  expect(tsText).not.toContain('<T,>');
  // Python: inside a class body the method (with self) is offered, at module level the free function
  await app.closeAll();
  await app.openFile('cls.py'); await app.end(); await app.type('def');
  const classLabels = await openCandidates(app);
  expect(classLabels.join(' | ')).toMatch(/method/);
  expect(classLabels.join(' | ')).not.toMatch(/\bfunction\b/);
  await app.pickRow('method');
  const clsText = await app.waitEditorText('def name(');
  const clsLines = clsText.split('\n');
  const defLine = clsLines.find(l => l.includes('def name('));
  const bodyLine = clsLines[clsLines.indexOf(defLine) + 1];
  expect(indentOf(defLine)).toBe(4);                      // the block indentation of the class body
  expect(indentOf(bodyLine)).toBeGreaterThan(indentOf(defLine));   // the body is one level deeper: valid Python
  await app.screenshot('st-008-python-class-body');
  await app.closeAll();
  await app.openFile('mod.py'); await app.end(); await app.type('\ndef');
  const modLabels = await openCandidates(app);
  expect(modLabels.join(' | ')).toMatch(/\bfunction\b/);
  expect(modLabels.join(' | ')).not.toMatch(/\bmethod\b/);
});

test('UAT-58 [ST-031] the language drop-down of the floating candidate list offers the seven languages, the switch takes effect at once and the old candidates are not inserted', async () => {
  await app.openFile('ctx.rs'); await app.end(); await app.type('\nfnm');
  const rustLabels = await openCandidates(app);
  expect(rustLabels.join(' | ')).toMatch(/main function/);        // the Rust rules are active
  await app.escapeQuick();
  const before = await app.editorText();
  expect(before).toContain('fnm');
  // the drop-down lists the seven built-in languages
  await app.cmd('Programming IME: Choose Programming Language');
  await app.waitQuick();
  const languageLabels = await quickLabels(app);
  for (const id of ['rust', 'python', 'typescript', 'powershell', 'c', 'assembly', 'machine-code'])
    expect(languageLabels, `the drop-down must offer ${id}`).toContain(id);
  await app.screenshot('st-031-language-dropdown');
  await app.page.locator('.quick-input-list .monaco-list-row .label-name').filter({hasText: /^python$/}).first().click();
  await app.waitStatus(/PIME python/);
  // the new rules are active at once: the same prefix now follows the Python rules
  await app.focusEditor(); await app.end();
  await app.page.keyboard.press('Backspace'); await app.page.keyboard.press('Backspace'); await app.page.keyboard.press('Backspace');
  await app.type('def');
  const pythonLabels = await openCandidates(app);
  expect(pythonLabels.join(' | ')).toMatch(/\bfunction\b/);
  expect(pythonLabels.join(' | ')).not.toMatch(/main function/);   // the candidate of the old language is gone
  await app.pickRow('function');
  const after = await app.waitEditorText('def name(');
  expect(after).not.toContain('fn main()');                       // the old candidate was not inserted
  expect(after).toContain('fn existing() {}');
  // every one of the seven languages can be chosen and takes effect
  await app.closeAll();
  await app.openFile('lang.rs');
  for (const id of ['rust', 'python', 'typescript', 'powershell', 'c', 'assembly', 'machine-code']) {
    await pickLanguage(app, id);
    await expect.poll(async () => await app.statusText(), {timeout: 8000}).toMatch(new RegExp(`PIME ${id}(?![a-z-])`));
  }
});
