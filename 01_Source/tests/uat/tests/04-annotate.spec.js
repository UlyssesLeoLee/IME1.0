'use strict';
// UAT group 4: automatic detailed comments (setting autoComment, default ON) and the Cypher node/edge comment
// (setting graphComment). Requirements added by the project owner after the design baseline (ids ANN-xx).
const {test, expect} = require('@playwright/test');
const fs = require('node:fs');
const {VsCode, sleep} = require('../helpers/vscode');
const {startMock} = require('../helpers/mock-provider');

test.describe.configure({mode: 'serial'});
const KEY = 'mock-key-test-123';
let app, mock, behaviour;
const settingsFile = () => `${app.userDir}\\User\\settings.json`;
const patchSettings = patch => { const s = JSON.parse(fs.readFileSync(settingsFile(), 'utf8')); Object.assign(s, patch); fs.writeFileSync(settingsFile(), JSON.stringify(s, null, 2)); };

// a model that behaves: comments only
const goodReply = ({input}) => ({json: {
  commented: `// The entry point: the operating system starts your program here.\n${input.snippet.replace('${cursor}', '// Write your own steps below this line, one per line.\n    ${cursor}')}`,
  cypher: ["MERGE (f:File {name:'an1.rs'})", "MERGE (m:Function {name:'main', file:'an1.rs'})", 'MERGE (f)-[:DEFINES]->(m)', "MATCH (n) DETACH DELETE n"]
}});
const FILES = {
  'an1.rs': 'fn existing() {}\n', 'an2.rs': 'fn existing() {}\n', 'an3.rs': 'fn existing() {}\n', 'an4.rs': 'fn existing() {}\n', 'an5.rs': 'fn existing() {}\n', 'an6.rs': 'fn existing() {}\n',
  'secret1.rs': 'const K: &str = "sk-abcdefghijklmnopqrstuvwxyz0123";\nfn a() {}\n',
  'util.rs': 'pub fn helper_one() { let secret_body = 42; }\npub struct Widget;\n', 'notes.env': 'X=1\n', '.env': 'API_KEY=abcdefghijklmnop12345\n'
};
test.beforeAll(async () => {
  mock = await startMock('A', args => behaviour(args));
  behaviour = goodReply;
  app = await VsCode.launch({files: FILES, caFile: mock.certFile, settings: {
    'programmingIme.enabled': true, 'programmingIme.suggestMode': 'explicit',   // NOTE: autoComment is deliberately NOT set: the default must be ON
    'programmingIme.providers': {mockA: {api: 'openai-responses', endpoint: mock.endpoint('openai-responses')}}}});
});
test.afterAll(async () => { await app.screenshot('end-of-group-4'); await app.close(); await mock.close(); });
test.beforeEach(async () => { behaviour = goodReply; mock.reset(); await app.closeAll(); });

async function pickMain(file, typed = '\nfnm') {
  await app.openFile(file); await app.end(); await app.type(typed);
  await app.cmd('Programming IME: Show Code Candidates'); await app.waitQuick();
  await app.pickRow('main function');
}

test('ANN-10 with AI off nothing is asked, nothing is sent: the candidate is inserted unchanged (autoComment on by default)', async () => {
  const props = JSON.parse(fs.readFileSync(require('node:path').join(__dirname, '..', '..', '..', 'editor-bridges', 'vscode', 'package.json'), 'utf8')).contributes.configuration.properties;
  expect(props['programmingIme.autoComment'].default).toBe(true);        // the checkbox is ticked by default
  expect(props['programmingIme.graphComment'].default).toBe(false);
  await pickMain('an1.rs');
  const text = await app.waitEditorText('fn main() {');
  expect(text).not.toContain('operating system');
  expect(await app.dialogVisible()).toBe(false);
  expect(mock.count()).toBe(0);
});

test('ANN-11 default settings + AI configured: the whole file is sent after consent and the snippet comes back with detailed comments, code unchanged', async () => {
  await app.configureAI({provider: 'mockA', model: 'm-1', key: KEY});
  await app.openFile('an2.rs'); await app.end(); await app.type('\n// unsaved note\nfnm');
  await app.cmd('Programming IME: Show Code Candidates'); await app.waitQuick(); await app.pickRow('main function');
  const d = await app.waitDialog();
  expect(d.message).toMatch(/COMPLETE current file/);
  expect(d.message).not.toMatch(/NAMES/);                                   // graph comment is off: no directory data
  await app.screenshot('annotate-consent');
  await app.dialogClick('Authorize this request');
  const text = await app.waitEditorText('operating system starts your program');
  expect(text).toContain('fn main() {'); expect(text).toContain('// Write your own steps below this line');
  expect(mock.count('annotate')).toBe(1);
  const req = mock.requests.find(r => r.task === 'annotate');
  expect(req.input.requested).toEqual({comment: true, graph: false});
  expect(req.input.file).toContain('// unsaved note');                       // the AI saw the whole (unsaved) file
  expect(req.input.snippet).toMatch(/^fn main\(\) \{/);
  expect(req.rawBody).not.toContain('sk-');
  expect(await app.page.locator('.terminal-wrapper').count()).toBe(0);
  await app.screenshot('annotate-comments-inserted');
  await app.undo();                                                          // one Ctrl+Z removes the whole insertion
  expect(await app.editorText()).not.toContain('operating system');
});

test('ANN-12 if the AI changes the code, its comments are discarded and the original snippet is inserted', async () => {
  behaviour = () => ({json: {commented: 'fn main() {\n    std::process::exit(1); // sneaky\n}'}});
  await pickMain('an3.rs');
  await app.waitDialog(); await app.dialogClick('Authorize this request');
  await app.waitNotification(/changed the code itself/);
  const text = await app.waitEditorText('fn main() {');
  expect(text).not.toContain('process::exit'); expect(text).not.toContain('sneaky');
  expect(mock.count('annotate')).toBe(1);
});

test('ANN-13 refusing the consent, or turning autoComment off, inserts the snippet unchanged with zero requests', async () => {
  await pickMain('an4.rs');
  await app.waitDialog(); await app.dialogClick('Cancel');
  await app.waitEditorText('fn main() {');
  expect(mock.count()).toBe(0);
  await app.closeAll();
  patchSettings({'programmingIme.autoComment': false}); await sleep(2500);
  await pickMain('an5.rs');
  await app.waitEditorText('fn main() {');
  expect(await app.dialogVisible()).toBe(false);
  expect(mock.count()).toBe(0);
  patchSettings({'programmingIme.autoComment': true}); await sleep(2500);
});

test('ANN-14 a file with secrets is never sent for annotation', async () => {
  await app.openFile('secret1.rs'); await app.end(); await app.type('\nfnm');
  await app.cmd('Programming IME: Show Code Candidates'); await app.waitQuick(); await app.pickRow('main function');
  await app.waitNotification(/looks sensitive/);
  await app.waitEditorText('fn main() {');
  expect(mock.count()).toBe(0);
});

test('ANN-15 graph comment: Cypher block above the snippet, built from symbol NAMES of the folder (never their code), destructive statements dropped', async () => {
  patchSettings({'programmingIme.autoComment': false, 'programmingIme.graphComment': true, 'programmingIme.graphCommentScope': 'directory'}); await sleep(2500);
  await pickMain('an6.rs');
  const d = await app.waitDialog();
  expect(d.message).toMatch(/COMPLETE current file/); expect(d.message).toMatch(/NAMES \(not the code\)/);
  await app.screenshot('annotate-graph-consent');
  await app.dialogClick('Authorize this request');
  const text = await app.waitEditorText('[graph] Cypher');
  expect(text).toContain("// MERGE (m:Function {name:'main', file:'an1.rs'})"); expect(text).toContain('// MERGE (f)-[:DEFINES]->(m)');
  expect(text).not.toContain('DETACH DELETE');
  expect(text).toContain('fn main() {'); expect(text).not.toContain('operating system');          // autoComment is off
  const req = mock.requests.find(r => r.task === 'annotate');
  expect(req.input.requested).toEqual({comment: false, graph: true});
  const files = req.input.directorySymbols.map(f => f.file);
  expect(files).toContain('util.rs'); expect(files).not.toContain('.env'); expect(files).not.toContain('an6.rs');
  expect(JSON.stringify(req.input.directorySymbols)).toContain('helper_one');
  expect(req.rawBody).not.toContain('secret_body');                                              // names only, not the code of util.rs
  expect(req.rawBody).not.toContain('API_KEY');
  await app.screenshot('annotate-graph-inserted');
});
