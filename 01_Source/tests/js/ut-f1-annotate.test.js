'use strict';
// F1 annotate (src/f1.js, src/ui/f1panel.js, docs/F1_ANNOTATE.md): the scope under the cursor (or the selection) gets comments, built by OUR code
// from the model's "above line N put this comment" answers, and reviewed as hunks before anything is written.
// The `vscode` module, the editor, the panel and the runtime are injected fakes (document with undo, editor with decorations, dialogs, webview);
// the permission broker, the privacy gate, the edit coordinator, the scope resolver, the annotation functions and the AI client are the REAL
// JS ones, and the provider is a recording fake fetch, so "no secret reaches the provider" is checked on the actual request body.
// The webview page itself is run against a tiny fake DOM (ANN-79/80). Nothing here ran inside a real VS Code window (see the UAT spec
// 06-annotate-scope.spec.js, which is a separate run). The Rust engine has no ops for scope/anchored comments yet (docs/F1_ANNOTATE.md).
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const {core, makeFetch, okJson, responsesBody, sleep} = require('./helpers');
const {createEngine} = require('../../editor-bridges/vscode/src/engine');
const f1mod = require('../../editor-bridges/vscode/src/f1');
const panelMod = require('../../editor-bridges/vscode/src/ui/f1panel');
const settingsMod = core('settings'), aiMod = core('ai'), i18n = core('i18n'), annotateMod = core('annotate'), scopeMod = core('scope');
const {Diagnostics} = core('diagnostics');
const {fail, codeOf} = core('errors');

const SRC = path.join(__dirname, '..', '..');
const ONCE = 'Authorize this request', WORKSPACE = 'Authorize for this workspace (24 h)';
const store = () => { const m = new Map(); return {get: k => m.get(k), update: (k, v) => { m.set(k, v); return Promise.resolve(); }}; };
const EOLRE = /\r\n|\r|\n/g;

// ---------------------------------------------------------------- fixtures
const RUST = [
  'use std::fmt;', '', 'struct Parser {', '    pos: usize,', '}', '', 'impl Parser {', '    fn new() -> Parser {', '        Parser { pos: 0 }', '    }', '',
  '    fn parse_header(&mut self, input: &str) -> usize {', '        let mut n = 0;', '        for c in input.chars() {', "            if c == ':' {", '                break;', '            }',
  '            n += 1;', '        }', '        self.pos = n;', '        n', '    }', '}', ''].join('\n');
// 0-based lines of RUST: parse_header is lines 11-21, the for loop 13-18, impl Parser 6-22
const PARSE_HEADER = {startLine: 11, endLine: 21};
const atText = (text, part, extra = 0) => { const i = text.indexOf(part); assert.ok(i >= 0, `fixture text: ${part}`); return i + extra; };
const SECRET_FILE = 'fn connect() {\n    let password = "hunter2hunter2";\n    login(password);\n}\n\nfn other() {\n    other_call();\n}\n';
const SECRET_HIGH_FILE = 'fn connect() {\n    let key = "sk-abcdefghijklmnopqrstuvwxyz0123";\n    login(key);\n}\n';

// ---------------------------------------------------------------- fakes
function lineStarts(text) { const s = [0]; EOLRE.lastIndex = 0; for (let m; (m = EOLRE.exec(text));) s.push(m.index + m[0].length); return s; }
class FakeDoc {
  constructor(text, {languageId = 'rust', fileName = 'C:\\proj\\src\\main.rs', scheme = 'file'} = {}) {
    Object.assign(this, {text, version: 1, languageId, fileName, isDirty: false, isClosed: false, undoStack: [], writes: 0});
    this.uri = {scheme, toString: () => `${scheme}:///proj/${fileName.split(/[\\/]/).pop()}`};
    this.eol = /\r\n/.test(text) ? '\r\n' : '\n';
  }
  getText(range) { return range ? this.text.slice(this.offsetAt(range.start), this.offsetAt(range.end)) : this.text; }
  offsetAt(p) { return lineStarts(this.text)[p.line] + p.character; }
  positionAt(off) { const s = lineStarts(this.text); let line = 0; while (line + 1 < s.length && s[line + 1] <= off) line++; return {line, character: off - s[line]}; }
  edit(text) { this.undoStack.push(this.text); this.text = text; this.version++; }  // what the user typing does
  undo() { if (this.undoStack.length) { this.text = this.undoStack.pop(); this.version++; } }
}
class Range {
  constructor(a, b, c, d) { if (typeof a === 'object') { this.start = a; this.end = b; } else { this.start = {line: a, character: b}; this.end = {line: c, character: d}; } }
}
function makeEditor(doc, a, b = a) {
  const e = {document: doc, selection: {start: doc.positionAt(a), end: doc.positionAt(b), isEmpty: a === b}, visibleRanges: [new Range(0, 0, 100000, 0)], reveals: [], decor: new Map(),
    setDecorations(type, ranges) { if (type.disposed) throw new Error('decoration type used after dispose'); type.editors.add(e); e.decor.set(type, ranges); },
    revealRange(range, how) { e.reveals.push({range, how}); }};
  return e;
}

// handler(call, n) -> response; reply(input, n) -> object (JSON) | string (raw model text), input = the parsed prompt of the request
function rig({language = 'rust', text = RUST, fileName, languageId = 'rust', config = {}, reply, handler, key = {'pime-key:mock': 'KEY-123456'}, dirSymbols = [], scheme = 'file'} = {}) {
  const st = {enabled: true, aiEnabled: true, trusted: true, locale: 'en-US', key, language,
    config: {f1Mode: 'annotate', f1AnnotateOnOpen: true, autoComment: true, graphComment: false, graphCommentScope: 'directory', ...config}};
  const ui = {warnings: [], errors: [], infos: [], dialogs: [], panels: [], types: [], commands: [], applyCalls: 0, choose: {consent: ONCE, sensitive: 'Redact and send'}, providers: new Map()};
  const doc = new FakeDoc(text, {languageId, scheme, ...(fileName ? {fileName} : {})});
  const engine = createEngine('js', {globalState: store()});
  const diagnostics = new Diagnostics();
  const workspaceId = () => engine.workspaceKey(['file:///w']);
  const authorize = async req => {
    if (!st.trusted) throw fail('CONSENT_REQUIRED', 'workspace is not trusted');
    const criteria = {providerId: req.providerId, workspace: workspaceId(), scope: req.grantScope};
    if (req.grantId) { await engine.broker.require(req.grantId, criteria); await engine.broker.consume(req.grantId); return true; }
    if (!await engine.broker.find(criteria)) throw fail('CONSENT_REQUIRED', 'no authorisation');
    return true;
  };
  const defaultReply = input => ({comments: input.anchorable.slice(0, 1).map(line => ({line, text: 'Explains this line.'}))});
  const fetchImpl = makeFetch(handler || ((call, n) => {
    const r = (reply || defaultReply)(JSON.parse(call.body.input), n);
    return okJson(responsesBody(typeof r === 'string' ? r : JSON.stringify(r)));
  }));
  const ai = new aiMod.AiClient({fetchImpl, budget: engine.budget, diagnostics, isEnabled: () => st.aiEnabled, authorize});
  const settings = settingsMod.effective({providers: {mock: {api: 'openai-responses', endpoint: 'https://api.a.example/v1/responses'}},
    scenarios: {f1Full: {provider: 'mock', model: 'm-full'}, f1Insert: {provider: 'mock', model: 'm-insert'}, realtime: {provider: 'mock', model: 'm-rt'}}});
  const rt = {engine, broker: engine.broker, coordinator: engine.coordinator, budget: engine.budget, ai, diagnostics, settings, get locale() { return st.locale; }, ctx: {secrets: {get: async id => st.key[id]}}};
  const L = (k, ...a) => i18n.t(st.locale, k, ...a);
  const calls = {directory: 0};
  let f1 = null;
  const vscode = {
    workspace: {
      get isTrusted() { return st.trusted; },
      registerTextDocumentContentProvider: (scheme, provider) => { ui.providers.set(scheme, provider); return {dispose() {}}; },
      // one WorkspaceEdit = one undo step; like the editor, the inserted text takes the line ending of the document
      applyEdit: async we => {
        ui.applyCalls++;
        const before = doc.text;
        let text = before;
        const edits = we.edits.map(e => ({a: doc.offsetAt(e.range.start), b: doc.offsetAt(e.range.end), text: e.text.replace(EOLRE, doc.eol)})).sort((x, y) => y.a - x.a);
        for (const e of edits) text = text.slice(0, e.a) + e.text + text.slice(e.b);
        doc.undoStack.push(before); doc.text = text; doc.version++; doc.writes++;
        f1.onDocumentChanged({document: doc, contentChanges: [{}]}); // the editor reports its own edits too: f1 must ignore them while applying
        return true;
      }
    },
    window: {
      activeTextEditor: null, visibleTextEditors: [],
      showWarningMessage: async (msg, opts, ...buttons) => {
        if (!(opts && opts.modal)) { ui.warnings.push(msg); return undefined; }
        ui.dialogs.push({msg, detail: opts.detail, buttons});
        const pick = ui.choose[/Possible secrets/.test(msg) ? 'sensitive' : 'consent'];
        return typeof pick === 'function' ? pick(msg) : pick;
      },
      showErrorMessage: async msg => { ui.errors.push(msg); },
      showInformationMessage: async msg => { ui.infos.push(msg); },
      createWebviewPanel: (type, title) => {
        const panel = {type, title, posted: [], disposed: false, handler: null, onDispose: null,
          webview: {html: '', cspSource: 'csp:', onDidReceiveMessage(fn) { panel.handler = fn; }, postMessage(m) { panel.posted.push(m); return Promise.resolve(true); }},
          onDidDispose(fn) { panel.onDispose = fn; }, dispose() { if (!panel.disposed) { panel.disposed = true; if (panel.onDispose) panel.onDispose(); } }, reveal() {}};
        ui.panels.push(panel);
        Promise.resolve().then(() => { if (panel.handler) panel.handler({type: 'ready'}); }); // the page announces itself as soon as it has loaded
        return panel;
      },
      createTextEditorDecorationType: opts => {
        const t = {opts, disposed: false, editors: new Set(), dispose() { t.disposed = true; for (const e of t.editors) e.decor.delete(t); }};
        ui.types.push(t);
        return t;
      }
    },
    commands: {executeCommand: async (...args) => { ui.commands.push(args); }},
    Range, WorkspaceEdit: class { constructor() { this.edits = []; } replace(uri, range, text) { this.edits.push({uri, range, text}); } },
    Uri: {from: o => ({...o, toString() { return `${o.scheme}:${o.path}`; }})}, ViewColumn: {Beside: -2},
    ThemeColor: class { constructor(id) { this.id = id; } }, OverviewRulerLane: {Center: 2}, TextEditorRevealType: {InCenterIfOutsideViewport: 2}
  };
  const cfg = () => ({get: k => st.config[k]});
  const annotation = {
    wanted: () => ({comment: st.config.autoComment !== false, graph: st.config.graphComment === true, scope: st.config.graphCommentScope === 'file' ? 'file' : 'directory'}),
    directorySymbols: async () => { calls.directory++; return dirSymbols; }
  };
  f1 = f1mod.createF1({vscode, rt, L, errorText: e => i18n.describeError(st.locale, codeOf(e), e && e.message), enabled: () => st.enabled, aiEnabled: () => st.aiEnabled, workspaceId,
    grantProviderId: sc => `${sc.providerId}:${new URL(sc.endpoint).host}`.slice(0, 64), cfg, infoFor: () => ({language: st.language, dialect: {}}), annotation});
  const panel = () => ui.panels[ui.panels.length - 1];
  const h = {
    f1, st, ui, rt, doc, vscode, fetch: fetchImpl, calls, L, editor: null,
    state: () => panel().posted[panel().posted.length - 1],
    tab: (name = 'annotate') => h.state().tabs[name],
    requests: () => fetchImpl.calls.length,
    input: (n = 0) => JSON.parse(fetchImpl.calls[n].body.input),
    wire: () => fetchImpl.calls.map(c => c.init.body).join('\n'),
    // opens F1 at [a, b] (a === b: just a cursor); the page is "ready" at once; the automatic request (if any) has finished when this returns
    async open(a, b = a, editor = null) {
      h.editor = editor || makeEditor(doc, a, b);
      vscode.window.activeTextEditor = h.editor; vscode.window.visibleTextEditors = [h.editor];
      await f1.open();
      h.ready();
      await f1.settled();
      return h;
    },
    ready() { panel().handler({type: 'ready'}); },
    async send(m) { panel().handler(m); await f1.settled(); },
    types: () => ui.types.filter(t => !t.disposed),
    highlight: () => { const t = h.types()[0]; const r = t && h.editor.decor.get(t); return r ? r.map(x => ({startLine: x.start.line, endLine: x.end.line})) : null; }
  };
  return h;
}
const until = async (fn, ms = 3000) => { const end = Date.now() + ms; while (!fn()) { if (Date.now() > end) throw new Error('timed out waiting'); await sleep(5); } };
// a model reply from [[scope line (1-based, as numbered), text], ...]
const replyOf = (...items) => ({comments: items.map(([line, text]) => ({line, text}))});
const added = (before, after) => after.split('\n').filter(l => !before.split('\n').includes(l));
const fingerprint = (profile, t) => annotateMod.codeFingerprint(profile, t);

// ---------------------------------------------------------------- routing: cursor or selection
test('ANN-60 no selection: the declaration around the cursor is the scope, the request is the scope numbered plus the whole file, the highlight shows exactly it', async () => {
  const h = rig({reply: () => replyOf([1, 'Reads the header of the input.'], [3, 'Walks over every character.']), dirSymbols: []});
  await h.open(atText(RUST, 'n += 1', 3));
  const t = h.tab();
  assert.equal(t.status, 'ready', JSON.stringify(t));
  assert.equal(t.scope.text, 'Scope: fn parse_header (lines 12-22, 11 lines)');
  assert.deepEqual(t.scope.crumbs.map(c => c.label), ['fn parse_header', 'impl Parser', 'file']); assert.equal(t.scope.index, 0);
  assert.equal(t.scope.canNarrower, false); assert.equal(t.scope.canWider, true);
  assert.deepEqual(h.highlight(), [PARSE_HEADER]);
  // exactly one request, scenario f1Insert, the whole file as context, the scope numbered from 1, the reply has room (>= 3000 tokens)
  assert.equal(h.requests(), 1);
  const call = h.fetch.calls[0], inp = h.input();
  assert.equal(call.body.model, 'm-insert'); assert.ok(call.body.max_output_tokens >= 3000);
  assert.equal(inp.file, RUST); assert.equal(inp.fileName, 'main.rs', 'only the base name leaves the machine');
  assert.ok(!call.init.body.includes('C:\\\\proj'), 'no local path in the request');
  assert.deepEqual(inp.scope, {kind: 'declaration', label: 'fn parse_header', startLine: 12, endLine: 22});
  assert.match(inp.numbered, /^1\|     fn parse_header\(&mut self, input: &str\) -> usize \{\n2\|         let mut n = 0;/);
  assert.equal(inp.numbered.split('\n').length, 11); assert.deepEqual(inp.requested, {comment: true, graph: false});
  assert.deepEqual(h.rt.broker.list(), [], 'a one-use grant was consumed by the request');
  assert.match(h.ui.dialogs[0].msg, /Send the COMPLETE current file to api\.a\.example/);
  // the proposal: two hunks, each a pure insertion, the code is the same
  assert.equal(t.hunks.length, 2); assert.ok(t.hunks.every(x => x.selected));
  assert.match(t.message, /^Proposed: 2 comment\(s\)\./);
  const s = h.f1.current(), proposed = s.tabs.annotate.proposed;
  assert.ok(s.tabs.annotate.review.hunks.every(x => x.oldLineCount === 0), 'only lines are added');
  assert.equal(fingerprint('rust', proposed), fingerprint('rust', RUST));
  assert.deepEqual(added(RUST, proposed), ['    // Reads the header of the input.', '        // Walks over every character.']);
  assert.equal(h.doc.text, RUST, 'nothing is written before Apply');
  assert.equal(h.tab('full').status, 'idle'); assert.equal(h.tab('insert').status, 'idle', 'the other tabs did not run');
  assert.equal(h.state().active, 'annotate'); assert.equal(h.state().mode, 'annotate');
});

test('ANN-61 a selection is the scope: exactly the selected lines (snapped to whole lines), whatever declaration contains it', async () => {
  const a = atText(RUST, 'for c in', 4), b = atText(RUST, 'n += 1', 2);          // starts and ends in the middle of lines
  const h = rig({reply: () => replyOf([1, 'Loop over the characters.'])});
  await h.open(a, b);
  const t = h.tab();
  assert.equal(t.status, 'ready');
  assert.equal(t.scope.text, 'Selection: lines 14-18 (5 lines)');
  assert.deepEqual(t.scope.crumbs.map(c => c.label), ['selection', 'fn parse_header', 'impl Parser', 'file']);
  assert.deepEqual(h.highlight(), [{startLine: 13, endLine: 17}]);
  const inp = h.input();
  assert.deepEqual(inp.scope, {kind: 'selection', label: 'selection', startLine: 14, endLine: 18});
  assert.equal(inp.numbered.split('\n').length, 5); assert.match(inp.numbered, /^1\|         for c in input\.chars\(\) \{/);
  assert.equal(inp.file, RUST, 'the whole file is the context, the selection only says what to comment');
  assert.deepEqual(added(RUST, h.f1.current().tabs.annotate.proposed), ['        // Loop over the characters.']);
  // a selection that ends at column 0 of the next line does not include that line (dragging down whole lines)
  const c = rig({reply: () => replyOf([1, 'x'])});
  await c.open(atText(RUST, '        let mut n = 0;'), atText(RUST, '        for c in input'));
  assert.equal(c.tab().scope.text, 'Selection: line 13 (1 line)');
  // selecting only blanks is the same as a cursor there
  const blank = rig({reply: () => replyOf([1, 'x'])});
  await blank.open(atText(RUST, '    }\n\n    fn parse_header', 6), atText(RUST, '    }\n\n    fn parse_header', 7));
  assert.notEqual(blank.tab().scope.text.startsWith('Selection'), true);
});

// ---------------------------------------------------------------- widen / narrow
test('ANN-62 Wider / Narrower walk the chain, reset the tab to idle (no request on their own), move the highlight and disable at the ends', async () => {
  const h = rig({reply: () => replyOf([1, 'Explains it.'])});
  await h.open(atText(RUST, 'n += 1', 3));
  assert.equal(h.requests(), 1);
  const wider = () => h.send({type: 'scope', move: 'wider'}), narrower = () => h.send({type: 'scope', move: 'narrower'});
  await wider();
  let t = h.tab();
  assert.equal(t.scope.text, 'Scope: impl Parser (lines 7-23, 17 lines)'); assert.equal(t.scope.index, 1);
  assert.equal(t.status, 'idle'); assert.deepEqual(t.hunks, []); assert.equal(h.requests(), 1, 'changing the scope never sends anything');
  assert.deepEqual(h.highlight(), [{startLine: 6, endLine: 22}]);
  assert.equal(t.scope.canWider, true); assert.equal(t.scope.canNarrower, true);
  await wider();
  t = h.tab();
  assert.equal(t.scope.text, 'Whole file: lines 1-23 (23 lines)'); assert.equal(t.scope.canWider, false, 'the end of the chain'); assert.equal(t.scope.canNarrower, true);
  assert.deepEqual(h.highlight(), [{startLine: 0, endLine: 22}]);
  await wider();                                           // a stray message at the end changes nothing
  assert.equal(h.tab().scope.index, 2);
  // Generate now runs the annotate tab on the WIDE scope: one more request, numbered against the file
  await h.send({type: 'generate', intent: ''});
  assert.equal(h.requests(), 2); assert.deepEqual(h.input(1).scope, {kind: 'file', label: 'file', startLine: 1, endLine: 23});
  assert.equal(h.input(1).numbered.split('\n').length, 23);
  await narrower(); await narrower();
  t = h.tab();
  assert.equal(t.scope.text, 'Scope: fn parse_header (lines 12-22, 11 lines)'); assert.equal(t.scope.canNarrower, false);
  assert.equal(t.status, 'idle'); assert.equal(h.requests(), 2);
  await narrower();
  assert.equal(h.tab().scope.index, 0);
});

test('ANN-63 Wider stops where the next scope is too large: the entry is shown crossed out with the reason, and it is never selectable', async () => {
  const filler = Array.from({length: 600}, (_, i) => `    // filler line number ${i} padding the impl so that it is too big to send`).join('\n');
  const big = `impl Big {\n    fn small() {\n        let x = 1;\n    }\n${filler}\n}\n`;
  assert.ok(big.length > f1mod.MAX_SCOPE_CHARS);
  const h = rig({text: big, reply: () => replyOf([1, 'x'])});
  await h.open(atText(big, 'let x', 2));
  const t = h.tab();
  assert.equal(t.status, 'ready'); assert.equal(t.scope.text, 'Scope: fn small (lines 2-4, 3 lines)');
  assert.deepEqual(t.scope.crumbs.map(c => [c.label, !!c.disabled]), [['fn small', false], ['impl Big', true]]);
  assert.equal(t.scope.canWider, false);
  assert.match(t.scope.hint, /^The next wider scope \(impl Big\) is too large \(\d+ characters, 605 lines; the limits are 20000 characters and 1200 lines\)\.$/);
  assert.equal(t.scope.crumbs[1].title, t.scope.hint, 'the reason is on the crossed-out entry too');
  await h.send({type: 'scope', move: 'wider'});
  assert.equal(h.tab().scope.index, 0); assert.equal(h.tab().status, 'ready', 'a refused move does not reset the tab');
  // a scope that is itself too large: nothing is sent, the message says what to do (there is nothing narrower in the chain)
  const body = Array.from({length: 700}, (_, i) => `    let v${i} = ${i}; // a line that makes this function far too long to send`).join('\n');
  const huge = `fn long() {\n${body}\n}\n`;
  const g = rig({text: huge, reply: () => replyOf([1, 'x'])});
  await g.open(atText(huge, 'let v5 ', 3));
  const u = g.tab();
  assert.equal(u.status, 'unavailable'); assert.match(u.message, /^The scope fn long is too large \(\d+ characters, 702 lines; the limits are 20000 characters and 1200 lines\)\. Select a smaller part of the code and press F1 again\.$/);
  assert.equal(u.scope.canWider, false); assert.equal(u.scope.canNarrower, false); assert.equal(g.requests(), 0);
  assert.equal(g.state().canGenerate, false, 'Generate has nothing to run');
  assert.deepEqual(g.highlight(), [{startLine: 0, endLine: 701}], 'the too large range is still shown so the user sees why');
  // a selection of the small part works inside the same file
  const s = rig({text: huge, reply: () => replyOf([1, 'x'])});
  await s.open(atText(huge, 'let v5 '), atText(huge, 'let v8 ', 12));
  assert.equal(s.tab().status, 'ready'); assert.equal(s.tab().scope.text, 'Selection: lines 7-10 (4 lines)');
  // the line limit: short lines keep a scope under the character limit, but the review diff of core/diff.js would degrade into ONE block
  // (a "removed 1979 lines" replace) past ~2000 lines, so the scope is refused far earlier
  const shortLines = n => `fn many() {\n${Array.from({length: n}, (_, i) => (i % 3 === 2 ? '' : `  x${i % 10}();`)).join('\n')}\n}\n`;
  const long = shortLines(1300);
  assert.ok(long.length < f1mod.MAX_SCOPE_CHARS);
  const l = rig({text: long, reply: () => replyOf([1, 'x'])});
  await l.open(atText(long, 'x5', 1));
  assert.equal(l.tab().status, 'unavailable'); assert.match(l.tab().message, /^The scope fn many is too large \(\d+ characters, 1302 lines; the limits are 20000 characters and 1200 lines\)\./);
  assert.equal(l.requests(), 0);
  // just under the limit: every comment is its own selectable block
  const ok = shortLines(1190);
  const k = rig({text: ok, reply: input => ({comments: input.anchorable.filter((_, i) => i % 20 === 0).map(line => ({line, text: 'c'}))})});
  await k.open(atText(ok, 'x5', 1));
  assert.equal(k.tab().status, 'ready');
  const comments = k.input().anchorable.filter((_, i) => i % 20 === 0).length;
  assert.ok(comments > 30); assert.equal(k.tab().hunks.length, comments, 'one block per comment');
  assert.ok(k.f1.current().tabs.annotate.review.hunks.every(x => x.oldLineCount === 0));
});

// ---------------------------------------------------------------- options
test('ANN-64 options start from autoComment / graphComment, change for this session only, and say what to enable when both are off', async () => {
  const h = rig({config: {f1AnnotateOnOpen: false}, reply: () => replyOf([1, 'x'])});
  await h.open(atText(RUST, 'n += 1', 3));
  let t = h.tab();
  assert.deepEqual([t.options.comment, t.options.graph], [true, false]); assert.equal(t.status, 'idle'); assert.equal(h.requests(), 0, 'f1AnnotateOnOpen=false: nothing starts');
  await h.send({type: 'option', name: 'graph', on: true});
  await h.send({type: 'option', name: 'comment', on: false});
  t = h.tab();
  assert.deepEqual([t.options.comment, t.options.graph], [false, true]);
  assert.deepEqual(h.st.config, {f1Mode: 'annotate', f1AnnotateOnOpen: false, autoComment: true, graphComment: false, graphCommentScope: 'directory'}, 'the settings were not written');
  assert.equal(h.tab().status, 'idle');
  await h.send({type: 'option', name: 'graph', on: false});
  t = h.tab();
  assert.equal(t.message, 'Tick Comments or Cypher graph to say what to add.'); assert.equal(t.canRun, false); assert.equal(h.state().canGenerate, false);
  await h.send({type: 'generate', intent: ''}); await h.send({type: 'regenerate', tab: 'annotate'});
  assert.equal(h.requests(), 0, 'with nothing to add, nothing is sent'); assert.equal(h.ui.dialogs.length, 0, 'and no consent is asked');
  await h.send({type: 'option', name: 'bogus', on: true});
  assert.deepEqual(Object.keys(h.f1.current().tabs.annotate.options), ['comment', 'graph']);
  // the request carries what was ticked
  await h.send({type: 'option', name: 'comment', on: true});
  await h.send({type: 'generate', intent: 'explain the loop for a beginner'});
  assert.equal(h.requests(), 1);
  assert.deepEqual(h.input().requested, {comment: true, graph: false}); assert.equal(h.input().intent, 'explain the loop for a beginner');
  // a graph-only request carries the Cypher wish and is applied above the first line of the scope
  const g = rig({config: {f1AnnotateOnOpen: false, autoComment: false, graphComment: true, graphCommentScope: 'file'},
    reply: () => ({cypher: ["MERGE (f:Function {name:'parse_header'})", "MERGE (p:Struct {name:'Parser'})", 'MERGE (p)-[:DEFINES]->(f)']})});
  await g.open(atText(RUST, 'n += 1', 3));
  assert.deepEqual([g.tab().options.comment, g.tab().options.graph], [false, true]);
  await g.send({type: 'generate', intent: ''});
  assert.deepEqual(g.input().requested, {comment: false, graph: true});
  assert.equal(g.tab().status, 'ready'); assert.match(g.tab().message, /a Cypher relation comment/);
  const proposed = g.f1.current().tabs.annotate.proposed;
  assert.deepEqual(added(RUST, proposed), ['    // [graph] Cypher - node/edge relations of this block:', "    // MERGE (f:Function {name:'parse_header'})", "    // MERGE (p:Struct {name:'Parser'})", '    // MERGE (p)-[:DEFINES]->(f)']);
  assert.equal(proposed.indexOf('// [graph]'), RUST.indexOf('    fn parse_header') + 4, 'above the first line of the scope');
  assert.equal(g.calls.directory, 0, 'graphCommentScope=file never reads the folder');
});

test('ANN-65 consent: the whole file (L3) is asked first; the directory graph adds the symbol NAMES (L4) to the same modal; declining sends nothing', async () => {
  // plain comments: L3 only, one modal, workspace grant is kept
  const a = rig(); a.ui.choose.consent = WORKSPACE;
  await a.open(atText(RUST, 'n += 1', 3));
  assert.equal(a.ui.dialogs.length, 1); assert.deepEqual(a.ui.dialogs[0].buttons, [ONCE, WORKSPACE]);
  assert.match(a.ui.dialogs[0].msg, /Send the COMPLETE current file to api\.a\.example so the AI can write comments for the chosen scope\? Only comment lines are added; your code is never rewritten\.$/);
  assert.match(a.ui.dialogs[0].detail, /secrets/i);
  assert.deepEqual(a.rt.broker.list().map(g => g.scope), ['L3']);
  assert.equal(a.requests(), 1);
  await a.send({type: 'regenerate', tab: 'annotate'});
  assert.equal(a.ui.dialogs.length, 1, 'the workspace grant is reused: no second modal'); assert.equal(a.requests(), 2);
  // the directory graph: the same modal says the NAMES of other files are sent; L4 is granted too and the names are in the request
  const dir = [{file: 'util.rs', symbols: [{kind: 'fn', name: 'helper'}]}];
  const g = rig({config: {graphComment: true}, dirSymbols: dir, reply: () => ({comments: [{line: 1, text: 'c'}], cypher: ["MERGE (f:Function {name:'parse_header'})"]})}); g.ui.choose.consent = WORKSPACE;
  await g.open(atText(RUST, 'n += 1', 3));
  assert.equal(g.ui.dialogs.length, 1); assert.match(g.ui.dialogs[0].msg, /COMPLETE current file[^]*\n\nAlso send the NAMES \(not the code\) of symbols found in the other files of this folder/);
  assert.deepEqual(g.rt.broker.list().map(x => x.scope).sort(), ['L3', 'L4']);
  assert.equal(g.calls.directory, 1); assert.deepEqual(g.input().directorySymbols, dir);
  assert.equal(g.tab().status, 'ready'); assert.match(g.tab().message, /1 comment\(s\), a Cypher relation comment/);
  // once: the L4 grant is consumed with the request, nothing stays behind
  const o = rig({config: {graphComment: true}, dirSymbols: dir}); await o.open(atText(RUST, 'n += 1', 3));
  assert.deepEqual(o.rt.broker.list(), []); assert.equal(o.requests(), 1);
  // the file scope needs no L4 and never reads the folder
  const f = rig({config: {graphComment: true, graphCommentScope: 'file'}, dirSymbols: dir}); f.ui.choose.consent = WORKSPACE;
  await f.open(atText(RUST, 'n += 1', 3));
  assert.deepEqual(f.rt.broker.list().map(x => x.scope), ['L3']); assert.equal(f.calls.directory, 0); assert.deepEqual(f.input().directorySymbols, []);
  assert.doesNotMatch(f.ui.dialogs[0].msg, /NAMES/);
  // declining: no request, no grant, the tab waits (idle), and the user can try again
  const d = rig(); d.ui.choose.consent = undefined;
  await d.open(atText(RUST, 'n += 1', 3));
  assert.equal(d.requests(), 0); assert.deepEqual(d.rt.broker.list(), []); assert.equal(d.tab().status, 'idle'); assert.equal(d.state().busy, false);
  assert.equal(d.state().canGenerate, true); assert.equal(d.calls.directory, 0);
  d.ui.choose.consent = ONCE; await d.send({type: 'generate', intent: ''});
  assert.equal(d.requests(), 1); assert.equal(d.tab().status, 'ready');
  // an existing workspace grant for L3 (and L4) means no modal at all
  const e = rig(); await e.rt.broker.authorize({providerId: 'mock:api.a.example', workspace: e.rt.engine.workspaceKey(['file:///w']), scope: 'L3', ttlMs: 3600000});
  await e.open(atText(RUST, 'n += 1', 3));
  assert.equal(e.ui.dialogs.length, 0); assert.equal(e.requests(), 1);
});

// ---------------------------------------------------------------- privacy
test('ANN-66 redaction: one redaction of the whole file, the request is built from it, the comments are applied to the ORIGINAL text, markers read <redacted>', async () => {
  const reply = input => { assert.ok(input.file.includes('PIME_REDACTED_1') && input.numbered.includes('PIME_REDACTED_1')); return replyOf([1, 'Connects using PIME_REDACTED_1 as the credential.'], [2, 'Hands the PIME_REDACTED_1 value to login.']); };
  const h = rig({text: SECRET_FILE, reply});
  await h.open(atText(SECRET_FILE, 'login(', 2));
  assert.equal(h.ui.dialogs.filter(d => /Possible secrets/.test(d.msg)).length, 0, 'medium risk is redacted silently (as everywhere in F1)');
  assert.ok(!h.wire().includes('hunter2hunter2'), 'the secret is not in the request body');
  const inp = h.input();
  assert.ok(inp.file.includes('let password = "PIME_REDACTED_1";')); assert.equal(inp.numbered.split('\n').length, 4);
  assert.ok(inp.file.includes('fn other()'), 'the rest of the file is still context');
  const t = h.tab(), proposed = h.f1.current().tabs.annotate.proposed;
  assert.equal(t.status, 'ready'); assert.equal(t.hunks.length, 2);
  assert.ok(proposed.includes('let password = "hunter2hunter2";'), 'the code of the document is the original');
  assert.ok(!proposed.includes('PIME_REDACTED'), 'no marker reaches the document');
  assert.deepEqual(added(SECRET_FILE, proposed), ['// Connects using <redacted> as the credential.', '    // Hands the <redacted> value to login.']);
  assert.ok(t.notes.some(n => /^1 secret-like value\(s\) were redacted before sending; a marker in a comment reads <redacted> and is never restored\.$/.test(n)), t.notes.join('|'));
  // the invariants hold when applied too: the secret stays exactly where it was, nothing new carries it
  await h.send({type: 'applyAnnotate'});
  assert.equal(h.doc.text, proposed); assert.equal((h.doc.text.match(/hunter2hunter2/g) || []).length, 1);
  // high risk: the user decides; both answers behave as in the other tabs
  const hi = rig({text: SECRET_HIGH_FILE, reply: () => replyOf([1, 'The key PIME_REDACTED_1 is used here.'])});
  await hi.open(atText(SECRET_HIGH_FILE, 'login', 2));
  const d = hi.ui.dialogs.find(x => /Possible secrets found \(api-key×1\)/.test(x.msg)); assert.ok(d, hi.ui.dialogs.map(x => x.msg).join('|'));
  assert.deepEqual(d.buttons, ['Redact and send', 'Send unredacted (I accept the risk)']);
  assert.ok(!hi.wire().includes('sk-abcdefghijklmnopqrstuvwxyz0123')); assert.ok(!hi.f1.current().tabs.annotate.proposed.includes('PIME_REDACTED'));
  const raw = rig({text: SECRET_HIGH_FILE, reply: () => replyOf([1, 'x'])}); raw.ui.choose.sensitive = 'Send unredacted (I accept the risk)';
  await raw.open(atText(SECRET_HIGH_FILE, 'login', 2));
  assert.ok(raw.wire().includes('sk-abcdefghijklmnopqrstuvwxyz0123'), 'only the explicit choice sends the bytes'); assert.equal(raw.tab().status, 'ready');
  const closed = rig({text: SECRET_HIGH_FILE}); closed.ui.choose.sensitive = undefined;
  await closed.open(atText(SECRET_HIGH_FILE, 'login', 2));
  assert.equal(closed.requests(), 0, 'closing the sensitive-data dialog is "no"'); assert.equal(closed.tab().status, 'idle');
});

test('ANN-67 redaction that changes the number of lines is refused before anything is sent', async () => {
  const h = rig({text: SECRET_FILE, config: {f1AnnotateOnOpen: false}});
  const real = h.rt.engine.privacy.redact;
  h.rt.engine.privacy.redact = async (text, set) => (await real(text, set)).replace('\n    login', ' login'); // joins two lines
  await h.open(atText(SECRET_FILE, 'login(', 2));
  await h.send({type: 'generate', intent: ''});
  assert.equal(h.requests(), 0);
  assert.equal(h.tab().status, 'error'); assert.match(h.tab().message, /^Failed: The payload looks sensitive; nothing was sent\.$/);
  assert.equal(h.state().busy, false);
});

// ---------------------------------------------------------------- replies
test('ANN-68 a reply with nothing to add is "ready" with a clear message (and why comments were skipped); an unusable reply is an error that can be retried', async () => {
  const none = rig({reply: () => ({comments: []})});
  await none.open(atText(RUST, 'n += 1', 3));
  let t = none.tab();
  assert.equal(t.status, 'ready'); assert.equal(t.message, 'Nothing to add: no new comment was proposed for this scope.'); assert.deepEqual(t.hunks, []);
  assert.equal(none.state().banner, null); assert.equal(t.canApply, false);
  assert.ok(none.tab().notes.every(n => !/Failed/.test(n)));
  // everything the model named was not a safe position / an existing explanation / unusable: the counts say so
  const skipped = rig({reply: () => ({comments: [{line: 99, text: 'beyond the scope'}, {line: 'x', text: 'malformed'}, {line: 2, text: '   '}, {line: 4, text: 'dup'}, {line: 4, text: 'second one for the same line'}]})});
  await skipped.open(atText(RUST, 'n += 1', 3));
  t = skipped.tab();
  assert.equal(t.status, 'ready'); assert.equal(t.hunks.length, 1, 'the one good comment is proposed');
  assert.deepEqual(t.notes.filter(n => /skipped/.test(n)), ['1 comment(s) skipped: not a safe position.', '1 comment(s) skipped: duplicates of a comment that is already there or already proposed.', '2 comment(s) skipped: empty, malformed or over the size limits.'],
    'beyond the scope = not a safe position; a second comment for one line is a duplicate; malformed and empty are counted together');
  // an existing comment that says the same is not repeated
  const withComment = RUST.replace('        for c in', '        // Loop over every character of the input.\n        for c in');
  const dup = rig({text: withComment, reply: () => replyOf([4, 'loop over every character of the input.'])});
  await dup.open(atText(withComment, 'n += 1', 3));
  assert.equal(dup.tab().status, 'ready'); assert.deepEqual(dup.tab().hunks, []); assert.ok(dup.tab().notes.includes('1 comment(s) skipped: duplicates of a comment that is already there or already proposed.'), dup.tab().notes.join('|'));
  // unusable replies: an error with its own wording, Regenerate works, nothing changed
  let n = 0;
  const bad = rig({reply: (_, k) => (n = k) === 1 ? 'I am sorry, I cannot do that' : replyOf([1, 'Now it works.'])});
  await bad.open(atText(RUST, 'n += 1', 3));
  t = bad.tab();
  assert.equal(t.status, 'error'); assert.equal(t.message, 'The model reply could not be used (it is not the expected JSON). Nothing was changed. Try Regenerate or another model.');
  assert.equal(bad.doc.text, RUST); assert.deepEqual(t.hunks, []);
  await bad.send({type: 'regenerate', tab: 'annotate'});
  assert.equal(bad.tab().status, 'ready'); assert.equal(bad.tab().hunks.length, 1); assert.equal(bad.requests(), 2);
  for (const text of ['', '[]', '"x"', '{"comments": "no"}']) {
    const r = rig({reply: () => text}); await r.open(atText(RUST, 'n += 1', 3));
    assert.ok(['error', 'ready'].includes(r.tab().status), text); assert.deepEqual(r.tab().hunks, []);
    assert.equal(r.doc.text, RUST);
  }
  // a provider failure is an error with the provider's reason
  const down = rig({handler: () => ({ok: false, status: 401, headers: {get: () => null}, text: async () => ''})});
  await down.open(atText(RUST, 'n += 1', 3));
  assert.equal(down.tab().status, 'error'); assert.match(down.tab().message, /^Failed: The provider rejected the API key/);
});

// ---------------------------------------------------------------- apply, undo, stale
test('ANN-69 Apply writes the selected hunks as ONE WorkspaceEdit through the coordinator: one undo step, code unchanged, highlight cleared', async () => {
  const h = rig({reply: () => replyOf([1, 'Header.'], [3, 'Loop.'], [10, 'Return value.']), config: {}});
  await h.open(atText(RUST, 'n += 1', 3));
  let t = h.tab();
  assert.equal(t.hunks.length, 3); assert.ok(t.canApply);
  // deselect the middle block: the preview and the result follow
  await h.send({type: 'toggleHunk', tab: 'annotate', id: t.hunks[1].id, on: false});
  t = h.tab();
  assert.deepEqual(t.hunks.map(x => x.selected), [true, false, true]);
  assert.equal(t.validation.level, 'ok'); assert.ok(!t.preview.text.includes('// Loop.')); assert.ok(t.preview.text.includes('// Header.'));
  assert.match(t.preview.title, /^Result preview \(\d+ chars, sha256 [0-9a-f]{12}\)$/);
  assert.equal(h.tab('full').status, 'idle', 'toggling an annotate block did not touch the full tab');
  assert.equal(h.highlight() !== null, true);
  const before = h.doc.text;
  await h.send({type: 'applyAnnotate'});
  assert.equal(h.ui.applyCalls, 1, 'one WorkspaceEdit'); assert.equal(h.doc.undoStack.length, 1, 'one undo step');
  assert.deepEqual(added(before, h.doc.text), ['    // Header.', '        // Return value.']);
  assert.equal(fingerprint('rust', h.doc.text), fingerprint('rust', before));
  assert.equal(h.state().banner.text, 'Applied to the editor buffer (not saved). Ctrl+Z undoes it.'); assert.equal(h.state().banner.kind, 'info');
  assert.equal(h.highlight(), null, 'the highlight is cleared after Apply'); assert.equal(h.types().length, 0);
  assert.equal(h.tab().canApply, false); assert.equal(h.state().canGenerate, false);
  assert.ok(h.rt.diagnostics.events.some(e => e.component === 'edit' && e.error_code === 'OK' && e.request_type === 'full'));
  h.doc.undo();
  assert.equal(h.doc.text, before, 'one undo restores the original text');
  await h.send({type: 'applyAnnotate'});                                 // a second Apply of the same proposal is refused
  assert.equal(h.ui.applyCalls, 1);
  // Generate after Apply starts over from the live buffer (a fresh session)
  const panels = h.ui.panels.length;
  await h.send({type: 'regenerate', tab: 'annotate'});
  assert.equal(h.ui.panels.length, panels + 1); assert.equal(h.tab().status, 'ready'); assert.equal(h.requests(), 2);
});

test('ANN-70 apply with every hunk deselected does nothing; an invalid selection cannot be applied', async () => {
  const h = rig({reply: () => replyOf([1, 'A.'], [3, 'B.'])});
  await h.open(atText(RUST, 'n += 1', 3));
  for (const x of h.tab().hunks) await h.send({type: 'toggleHunk', tab: 'annotate', id: x.id, on: false});
  assert.equal(h.tab().canApply, false);
  await h.send({type: 'applyAnnotate'});
  assert.equal(h.ui.applyCalls, 0); assert.equal(h.doc.text, RUST);
});

test('ANN-71 stale: a change to the document after the proposal disables Apply and drops the highlight; Generate starts over from a fresh snapshot', async () => {
  const h = rig({reply: () => replyOf([1, 'Header.'])});
  await h.open(atText(RUST, 'n += 1', 3));
  assert.equal(h.tab().canApply, true); assert.ok(h.highlight());
  h.doc.edit(RUST.replace('let mut n = 0;', 'let mut n = 1;')); h.f1.onDocumentChanged({document: h.doc, contentChanges: [{}]});
  assert.equal(h.state().stale, true); assert.equal(h.state().banner.text, 'The document changed after this proposal. Nothing was written. Generate again.');
  assert.equal(h.tab().canApply, false); assert.equal(h.highlight(), null);
  await h.send({type: 'applyAnnotate'});
  assert.equal(h.ui.applyCalls, 0); assert.equal(h.doc.text, RUST.replace('let mut n = 0;', 'let mut n = 1;'));
  const panels = h.ui.panels.length;
  await h.send({type: 'generate', intent: 'again'});
  assert.equal(h.ui.panels.length, panels + 1, 'a new session'); assert.equal(h.ui.panels[panels - 1].disposed, true, 'the stale panel is gone');
  assert.equal(h.requests(), 2); assert.equal(h.input(1).intent, 'again');
  assert.ok(h.input(1).file.includes('let mut n = 1;'), 'the fresh buffer is the context');
  assert.equal(h.tab().status, 'ready'); assert.equal(h.tab().canApply, true); assert.equal(h.state().stale, false);
  assert.deepEqual(h.highlight(), [PARSE_HEADER]);
  await h.send({type: 'applyAnnotate'});
  assert.equal(h.ui.applyCalls, 1);
  // a document closed under the session: the same
  const c = rig({reply: () => replyOf([1, 'x'])}); await c.open(atText(RUST, 'n += 1', 3));
  c.f1.onDocumentClosed(c.doc);
  assert.equal(c.state().stale, true); assert.equal(c.tab().canApply, false);
  // the editor reports its own edit while we apply: that is not a stale document (covered by ANN-69), and a selection move does not invalidate an annotate proposal
  const m = rig({reply: () => replyOf([1, 'x'])}); await m.open(atText(RUST, 'n += 1', 3));
  m.f1.onSelectionChanged({textEditor: m.editor, selections: [{start: {line: 0, character: 0}, end: {line: 0, character: 0}}]});
  assert.equal(m.tab().canApply, true);
});

test('ANN-72 a one-line scope in a CRLF file: the inserted comment uses CRLF too, so the coordinator read-back of the edit holds', async () => {
  const crlf = 'const a = 1;\r\nfunction one() { return 2; }\r\nconst b = 3;\r\n';
  const h = rig({text: crlf, language: 'typescript', languageId: 'typescript', fileName: 'C:\\p\\a.ts', reply: () => replyOf([1, 'Returns two.'])});
  await h.open(atText(crlf, 'one()', 2));
  assert.equal(h.tab().scope.text, 'Scope: function one (line 2, 1 line)');
  const proposed = h.f1.current().tabs.annotate.proposed;
  assert.equal(proposed, 'const a = 1;\r\n// Returns two.\r\nfunction one() { return 2; }\r\nconst b = 3;\r\n', 'every line break is CRLF');
  await h.send({type: 'applyAnnotate'});
  assert.equal(h.doc.text, proposed); assert.equal(h.state().banner.kind, 'info', h.state().banner && h.state().banner.text);
  // a multi-line scope copies the break of the scope itself
  const crlf2 = 'function a() {\r\n  let x = 1;\r\n  return x;\r\n}\r\n';
  const g = rig({text: crlf2, language: 'typescript', languageId: 'typescript', fileName: 'C:\\p\\b.ts', reply: () => replyOf([2, 'Starts at one.'])});
  await g.open(atText(crlf2, 'let x', 2));
  assert.equal(g.f1.current().tabs.annotate.proposed, 'function a() {\r\n  // Starts at one.\r\n  let x = 1;\r\n  return x;\r\n}\r\n');
  await g.send({type: 'applyAnnotate'}); assert.equal(g.state().banner.kind, 'info');
});

test('ANN-73 Open Diff shows the document next to the result of the SELECTED blocks, read-only', async () => {
  const h = rig({reply: () => replyOf([1, 'Header.'], [3, 'Loop.'])});
  h.f1.registerPreviewProvider({subscriptions: []});
  await h.open(atText(RUST, 'n += 1', 3));
  await h.send({type: 'toggleHunk', tab: 'annotate', id: h.tab().hunks[1].id, on: false});
  await h.send({type: 'openDiff', tab: 'annotate'});
  const [cmd, left, right, title] = h.ui.commands[0];
  assert.equal(cmd, 'vscode.diff'); assert.equal(left, h.doc.uri); assert.equal(right.scheme, 'pime-preview'); assert.match(title, /READ-ONLY preview$/);
  const shown = h.ui.providers.get('pime-preview').provideTextDocumentContent(right);
  assert.ok(shown.includes('// Header.') && !shown.includes('// Loop.'));
  // not before there is a proposal
  const idle = rig({config: {f1AnnotateOnOpen: false}}); await idle.open(atText(RUST, 'n += 1', 3));
  await idle.send({type: 'openDiff', tab: 'annotate'});
  assert.equal(idle.ui.commands.length, 0);
});

// ---------------------------------------------------------------- modes and auto-start
test('ANN-74 edit mode is the old F1: opens on the whole-file tab, starts nothing, Generate runs the whole-file and the insert request, the annotate tab stays idle', async () => {
  const AFTER = RUST.replace('let mut n = 0;', 'let mut n = 0; // counter');
  const h = rig({config: {f1Mode: 'edit'}, handler: call => {
    const m = call.body.model;
    return okJson(responsesBody(JSON.stringify(m === 'm-full' ? {fullFile: AFTER, explanation: 'comment added'} : {insertCandidates: ['// hi\n'], explanation: 'one comment'})));
  }});
  h.ui.choose.consent = WORKSPACE;
  await h.open(atText(RUST, 'n += 1', 3));
  assert.equal(h.state().active, 'full'); assert.equal(h.state().mode, 'edit');
  assert.equal(h.requests(), 0, 'no automatic request'); assert.equal(h.ui.dialogs.length, 0);
  assert.equal(h.tab().status, 'idle'); assert.equal(h.highlight(), null, 'no highlight while the other tabs are on screen');
  assert.equal(h.tab().scope.text, 'Scope: fn parse_header (lines 12-22, 11 lines)', 'the annotate tab is ready to be used');
  await h.send({type: 'generate', intent: 'fill'});
  assert.equal(h.requests(), 2); assert.deepEqual(h.fetch.calls.map(c => c.body.model).sort(), ['m-full', 'm-insert']);
  assert.equal(h.ui.dialogs.length, 1); assert.match(h.ui.dialogs[0].msg, /complete document and a nearby excerpt/);
  assert.equal(h.tab('full').status, 'ready'); assert.equal(h.tab('full').hunks.length, 1); assert.equal(h.tab('insert').status, 'ready'); assert.equal(h.tab('annotate').status, 'idle');
  // the whole-file tab keeps its element-level contract: hunk ids, preview, validation, Apply
  assert.deepEqual(Object.keys(h.tab('full')).sort(), ['canApply', 'hunks', 'hunksText', 'message', 'notes', 'preview', 'scopeText', 'status', 'validation']);
  assert.equal(h.tab('full').canApply, true);
  await h.send({type: 'applyFull'});
  assert.equal(h.doc.text, AFTER); assert.equal(h.ui.applyCalls, 1); assert.equal(h.doc.undoStack.length, 1);
  // the annotate tab can still be used on request in edit mode (no automatic start, the user presses the button)
  const e = rig({config: {f1Mode: 'edit'}}); e.ui.choose.consent = WORKSPACE;
  await e.open(atText(RUST, 'n += 1', 3));
  await e.send({type: 'tab', tab: 'annotate'});
  assert.equal(e.state().active, 'annotate'); assert.ok(e.highlight(), 'highlighted once the annotate tab is on screen');
  await e.send({type: 'regenerate', tab: 'annotate'});
  assert.equal(e.requests(), 1); assert.equal(e.tab().status, 'ready');
  // anything but "edit" is the new default
  const bogus = rig({config: {f1Mode: 'whatever', f1AnnotateOnOpen: false}}); await bogus.open(atText(RUST, 'n += 1', 3));
  assert.equal(bogus.state().mode, 'annotate'); assert.equal(bogus.state().active, 'annotate');
});

test('ANN-75 annotate mode: Generate and Enter run the ACTIVE tab only, so one F1 never spends three requests', async () => {
  const h = rig({handler: call => {
    const m = call.body.model;
    const inp = JSON.parse(call.body.input);
    return okJson(responsesBody(JSON.stringify(m === 'm-full' ? {fullFile: RUST.replace('let mut n = 0;', 'let mut n = 0; // c'), explanation: 'x'} : inp.anchorable ? replyOf([1, 'Header.']) : {insertCandidates: ['// hi\n']})));
  }});
  h.ui.choose.consent = WORKSPACE;
  await h.open(atText(RUST, 'n += 1', 3));
  assert.equal(h.requests(), 1, 'F1 = one request'); assert.equal(h.fetch.calls[0].body.model, 'm-insert'); assert.equal(h.tab().status, 'ready');
  await h.send({type: 'tab', tab: 'full'});
  assert.equal(h.highlight(), null, 'leaving the annotate tab removes the highlight');
  await h.send({type: 'generate', intent: ''});
  assert.equal(h.requests(), 2); assert.equal(h.fetch.calls[1].body.model, 'm-full'); assert.equal(h.tab('full').status, 'ready'); assert.equal(h.tab('insert').status, 'idle');
  await h.send({type: 'tab', tab: 'insert'});
  await h.send({type: 'generate', intent: ''});
  assert.equal(h.requests(), 3); assert.equal(h.fetch.calls[2].body.model, 'm-insert'); assert.equal(h.tab('insert').status, 'ready');
  assert.ok(h.tab('insert').candidates.length === 1);
  await h.send({type: 'tab', tab: 'annotate'});
  assert.ok(h.highlight(), 'back on the annotate tab: highlighted again'); assert.equal(h.types().length, 1);
  await h.send({type: 'generate', intent: ''});
  assert.equal(h.requests(), 4);
  assert.equal(h.fetch.calls[3].body.model, 'm-insert'); assert.ok(h.input(3).anchorable);
});

test('ANN-76 auto-start only in annotate mode, with f1AnnotateOnOpen, AI available, something to add and a configured provider', async () => {
  const text = atText(RUST, 'n += 1', 3);
  const cases = [
    ['annotate + on-open + AI ready', {}, r => {}, 1, 'ready'],
    ['f1AnnotateOnOpen=false', {f1AnnotateOnOpen: false}, r => {}, 0, 'idle'],
    ['edit mode', {f1Mode: 'edit'}, r => {}, 0, 'idle'],
    ['AI off', {}, r => { r.st.aiEnabled = false; }, 0, 'unavailable'],
    ['untrusted workspace', {}, r => { r.st.trusted = false; }, 0, 'unavailable'],
    ['no key', {}, r => { r.st.key = {}; }, 0, 'idle'],
    ['nothing ticked', {autoComment: false}, r => {}, 0, 'idle']
  ];
  for (const [label, config, setup, requests, status] of cases) {
    const r = rig({config}); setup(r);
    await r.open(text);
    assert.equal(r.requests(), requests, label); assert.equal(r.tab().status, status, label);
    assert.equal(r.ui.dialogs.length, requests ? 1 : 0, `${label}: the consent modal only when a request is about to go out`);
  }
  // the first F1 does not block the next one while the request is running: a second F1 opens a fresh session
  let release;
  const slow = rig({handler: call => new Promise(res => { release = () => res(okJson(responsesBody(JSON.stringify(replyOf([1, 'late']))))); })});
  slow.vscode.window.activeTextEditor = makeEditor(slow.doc, text); slow.vscode.window.visibleTextEditors = [slow.vscode.window.activeTextEditor];
  await slow.f1.open();
  slow.ready();
  await until(() => slow.fetch.calls.length === 1);
  assert.equal(slow.state().busy, true); assert.equal(slow.tab().status, 'loading');
  const second = slow.f1.open(); await second;
  assert.equal(slow.ui.panels.length, 2, 'a second F1 during the request opens a new session'); assert.equal(slow.ui.panels[0].disposed, true);
  release();
  await until(() => slow.fetch.calls.length === 2).catch(() => {}); // the second session starts its own request, answer it too
  release && release();
  // cancelling ends the request quietly
  const c = rig({handler: call => new Promise((_, rej) => call.init.signal.addEventListener('abort', () => rej(Object.assign(new Error('aborted'), {name: 'AbortError'}))))});
  c.vscode.window.activeTextEditor = makeEditor(c.doc, text); c.vscode.window.visibleTextEditors = [c.vscode.window.activeTextEditor];
  await c.f1.open(); c.ready();
  await until(() => c.fetch.calls.length === 1);
  await c.send({type: 'cancel'});
  assert.equal(c.tab().status, 'idle'); assert.match(c.tab().message, /cancelled/i); assert.equal(c.state().busy, false);
});

test('ANN-77 the annotate tab is unavailable, with a clear message, wherever it cannot work; no request is sent', async () => {
  const cases = [
    ['unsupported language', {language: 'unknown', languageId: 'plaintext'}, /^Adding comments to a scope is not available for the language "unknown"\.$/],
    ['empty document', {text: ''}, /^The document is empty: there is nothing to comment\.$/],
    ['whitespace only', {text: '  \n\n  '}, /^The document is empty/],
    ['AI off', {setup: r => { r.st.aiEnabled = false; }}, /^AI off - Programming IME: Toggle AI$/],
    ['untrusted', {setup: r => { r.st.trusted = false; }}, /^This workspace is not trusted, so AI is disabled\.$/],
    ['not a file or an untitled buffer', {scheme: 'git'}, /^Whole-file edits need the full editor buffer/],
    ['file over 2 MiB', {text: RUST + '// ' + 'x'.repeat(2 * 1024 * 1024) + '\n'}, /^The file exceeds 2 MiB; adding comments to a scope is disabled \(nothing is truncated\)\.$/]
  ];
  for (const [label, {setup, ...opts}, re] of cases) {
    const r = rig(opts); if (setup) setup(r);
    await r.open(Math.min(opts.text === undefined ? atText(RUST, 'n += 1', 3) : 0, (opts.text === undefined ? RUST : opts.text).length));
    const t = r.tab();
    assert.equal(t.status, 'unavailable', label); assert.match(t.message, re, label);
    assert.equal(r.requests(), 0, label); assert.equal(r.highlight(), null, `${label}: nothing to highlight`);
    assert.equal(t.scope, null, label); assert.equal(r.state().canGenerate === false || label === 'file over 2 MiB' || label === 'not a file or an untitled buffer', true, label);
    assert.equal(r.ui.dialogs.length, 0, label);
  }
  // an unavailable annotate tab does not take the other tabs down (language unknown, AI on): they say their own thing
  const u = rig({language: 'unknown', languageId: 'plaintext'}); await u.open(0);
  assert.equal(u.tab('full').status, 'idle'); assert.equal(u.tab('insert').status, 'idle'); assert.equal(u.state().active, 'annotate');
  await u.send({type: 'tab', tab: 'full'}); assert.equal(u.state().canGenerate, true);
});

test('ANN-78 every supported language: scope + a reply + apply gives comment-only hunks and the same code, on snippets and on real files of this repository', () => {
  const engine = createEngine('js', {globalState: store()});
  const run = (profile, text, at, {dialect = {}, label} = {}) => {
    const [s, e] = Array.isArray(at) ? at : [at, at];
    const t = f1mod.resolveTarget({profile, dialect, text, start: s, end: e});
    assert.ok(t.ok, `${label}: ${JSON.stringify(t)}`); assert.ok(t.allowed.length > 0, `${label}: the scope fits`);
    const sc = t.allowed[0], scopeText = text.slice(sc.start, sc.end), startLine = sc.startLine + 1;
    const prompt = annotateMod.buildScopePrompt({language: profile, dialect, comment: true, graph: false, fileName: 'x', fileText: text, scope: {startLine, endLine: sc.endLine + 1, label: sc.label, kind: sc.kind, text: scopeText}});
    const inp = JSON.parse(prompt.input);
    assert.ok(inp.anchorable.length > 0, `${label}: something can be commented`);
    const pick = inp.anchorable.filter((_, i) => i % 3 === 0).map(line => ({line, text: `Comment for line ${line}.\nSecond line.`}));
    const out = annotateMod.applyScopeReply(JSON.stringify({comments: pick}), {profile, dialect, scopeText, fileText: text, startLine, comment: true, graph: false});
    assert.ok(out.ok, `${label}: ${JSON.stringify(out)}`); assert.equal(out.added.comments, pick.length, label);
    const proposed = text.slice(0, sc.start) + f1mod.fitEol(out.text, scopeText, text) + text.slice(sc.end);
    const prop = engine.propose({content: text}, proposed, {language: profile, dialect});
    assert.equal(prop.hunks.length, pick.length, `${label}: one hunk per comment`);
    assert.ok(prop.review.hunks.every(x => x.oldLineCount === 0), `${label}: pure insertions`);
    assert.equal(fingerprint(profile, proposed), fingerprint(profile, text), `${label}: same code`);
    assert.ok(!/\r\n/.test(text) || !/(?<!\r)\n/.test(proposed), `${label}: line breaks stay CRLF`);
    assert.equal(prop.result.validation.ok, true, `${label}: still valid`);
    return {sc, prop};
  };
  const snippets = [
    ['rust', RUST, atText(RUST, 'n += 1', 3), 'fn parse_header'],
    ['typescript', 'import x from "y";\n\nexport function total(items: number[]): number {\n  let sum = 0;\n  for (const i of items) {\n    sum += i;\n  }\n  return sum;\n}\n', 90, 'function total'],
    ['python', 'import os\n\n\nclass Repo:\n    def load(self, path):\n        """Load it."""\n        if not os.path.exists(path):\n            return None\n        with open(path) as fh:\n            return fh.read()\n', 120, 'def load'],
    ['powershell', 'function Get-Total {\r\n    param([int[]]$Items)\r\n    $sum = 0\r\n    foreach ($i in $Items) {\r\n        $sum += $i\r\n    }\r\n    return $sum\r\n}\r\n', 80, 'Get-Total'],
    ['c', '#include <stdio.h>\n\nstatic int add(int a, int b) {\n    int s = a + b;\n    if (s > 10) {\n        s = 10;\n    }\n    return s;\n}\n', 75, 'add'],
    ['assembly', 'section .text\nglobal _start\n_start:\n    mov eax, 1\n    xor ebx, ebx\n    int 0x80\n', 55, '_start']
  ];
  for (const [profile, text, at, name] of snippets) {
    const r = run(profile, text, at, {dialect: profile === 'assembly' ? {assembler: 'nasm'} : {}, label: profile});
    assert.ok(r.sc.label.includes(name) || r.sc.kind !== 'declaration', `${profile}: ${r.sc.label}`);
  }
  // real files of this repository (CRLF on a Windows checkout): a function, a selection, a Rust function, a script
  const read = p => fs.readFileSync(path.join(SRC, p), 'utf8');
  const diff = read('editor-bridges/vscode/src/core/diff.js');
  run('typescript', diff, diff.indexOf('function diffMiddle') + 40, {label: 'diff.js function'});
  const a = diff.indexOf('const hunks = [];'), b = diff.indexOf('flush();', a);
  run('typescript', diff, [a, b + 3], {label: 'diff.js selection'});
  const budget = read('crates/pime-core/src/budget.rs');
  run('rust', budget, budget.indexOf('fn ') + 20, {label: 'budget.rs fn'});
  const build = read('scripts/build.ps1');
  run('powershell', build, Math.floor(build.length / 2), {label: 'build.ps1 block'});
});

// ---------------------------------------------------------------- highlight
test('ANN-79 the highlight is a whole-line decoration in theme colours that follows the scope, never steals the cursor, and is disposed with the session', async () => {
  const h = rig({reply: () => replyOf([1, 'x'])});
  const ed = makeEditor(h.doc, atText(RUST, 'n += 1', 3)); const selBefore = ed.selection;
  await h.open(0, 0, ed);
  assert.equal(h.ui.types.length, 1); const type = h.ui.types[0];
  assert.equal(type.opts.isWholeLine, true); assert.equal(type.opts.backgroundColor.id, 'editor.rangeHighlightBackground');
  assert.equal(type.opts.overviewRulerColor.id, 'editorOverviewRuler.rangeHighlightForeground');
  assert.deepEqual(h.highlight(), [PARSE_HEADER]);
  assert.deepEqual(ed.reveals, [], 'the scope contains the cursor and is visible: no scrolling'); assert.equal(ed.selection, selBefore, 'the selection was never touched');
  // re-rendering (a hunk toggle) does not create decorations again
  await h.send({type: 'toggleHunk', tab: 'annotate', id: 'h1', on: false}); assert.equal(h.ui.types.length, 1);
  // Wider: the highlight moves; the editor scrolls only when the new scope is entirely out of view
  ed.visibleRanges = [new Range(10, 0, 20, 0)];
  await h.send({type: 'scope', move: 'wider'});
  assert.deepEqual(h.highlight(), [{startLine: 6, endLine: 22}]); assert.deepEqual(ed.reveals, [], 'partly visible: left alone');
  ed.visibleRanges = [new Range(40, 0, 50, 0)];
  await h.send({type: 'scope', move: 'narrower'});
  assert.equal(ed.reveals.length, 1); assert.equal(ed.reveals[0].how, 2); assert.equal(ed.reveals[0].range.start.line, 11);
  assert.equal(ed.selection, selBefore);
  assert.equal(h.ui.types.length, 1, 'the same decoration type is reused');
  // a new editor object for the same document (tab hidden and shown again): the highlight follows
  const ed2 = makeEditor(h.doc, 0); h.vscode.window.visibleTextEditors = [ed2]; h.editor = ed2;
  h.f1.onVisibleEditorsChanged();
  assert.deepEqual(h.highlight(), [PARSE_HEADER]); assert.deepEqual(ed.decor.get(h.ui.types[0]), [], 'removed from the old editor');
  // not visible at all: nothing to decorate, nothing thrown
  h.vscode.window.visibleTextEditors = []; h.f1.onVisibleEditorsChanged();
  // closing the panel disposes it
  h.vscode.window.visibleTextEditors = [ed2];
  h.ui.panels[0].dispose();
  assert.equal(h.f1.current(), null); assert.equal(h.types().length, 0); assert.ok(h.ui.types.every(t => t.disposed));
  // closeSession (a second F1 / deactivate) too, and an unavailable tab never creates one
  const c = rig({reply: () => replyOf([1, 'x'])}); await c.open(atText(RUST, 'n += 1', 3));
  assert.equal(c.types().length, 1); c.f1.closeSession(); assert.equal(c.types().length, 0);
  const off = rig(); off.st.aiEnabled = false; await off.open(atText(RUST, 'n += 1', 3)); assert.equal(off.ui.types.length, 0);
});

// ---------------------------------------------------------------- reopen carries the session choices
test('ANN-80 Generate on a stale session reopens with the same tab and the same options; the settings are still untouched', async () => {
  const h = rig({config: {f1AnnotateOnOpen: false}, reply: () => replyOf([1, 'x'])});
  await h.open(atText(RUST, 'n += 1', 3));
  await h.send({type: 'option', name: 'graph', on: true}); await h.send({type: 'option', name: 'comment', on: false});
  await h.send({type: 'tab', tab: 'full'});
  h.doc.edit(RUST + '// more\n'); h.f1.onDocumentChanged({document: h.doc, contentChanges: [{}]});
  const key = JSON.stringify(h.st.config);
  await h.send({type: 'regenerate', tab: 'annotate'});
  assert.equal(h.state().active, 'full', 'the tab the user was on'); assert.deepEqual([h.tab().options.comment, h.tab().options.graph], [false, true]);
  assert.equal(h.requests(), 1, 'one request, from the regenerate (no second one from an automatic start)'); assert.deepEqual(h.input().requested, {comment: false, graph: true});
  assert.equal(JSON.stringify(h.st.config), key);
});

// ---------------------------------------------------------------- pure helpers
test('ANN-81 helpers: line counting and cutting keep the line breaks, markers become <redacted>, a one-line scope takes the document line break', () => {
  assert.equal(f1mod.lineCount('a'), 1); assert.equal(f1mod.lineCount('a\n'), 2); assert.equal(f1mod.lineCount('a\r\nb\rc\nd'), 4);
  const t = 'one\r\ntwo\r\nthree\r\nfour';
  assert.deepEqual(f1mod.lineSpan(t, 1, 2), {start: 5, end: 15}); assert.equal(t.slice(5, 15), 'two\r\nthree');
  assert.deepEqual(f1mod.lineSpan(t, 3, 3), {start: 17, end: 21}); assert.equal(f1mod.lineSpan(t, 0, 4), null); assert.equal(f1mod.lineSpan(t, 2, 1), null); assert.equal(f1mod.lineSpan(t, -1, 1), null);
  assert.equal(f1mod.maskMarkers('a PIME_REDACTED_1 b PIME_REDACTED_12 c PIME_REDACTED_'), 'a <redacted> b <redacted> c PIME_REDACTED_');
  assert.equal(f1mod.maskMarkers('{"text":"x PIME_REDACTED_3"}'), '{"text":"x <redacted>"}', 'a JSON reply stays valid JSON');
  assert.equal(f1mod.fitEol('// c\nline', 'line', 'a\r\nb'), '// c\r\nline'); assert.equal(f1mod.fitEol('// c\nline', 'line', 'a\nb'), '// c\nline');
  assert.equal(f1mod.fitEol('// c\nline', 'line', 'no break at all'), '// c\nline');
  assert.equal(f1mod.fitEol('x\r\n// c\nl1\nl2', 'l1\nl2', 'a\r\nb'), 'x\r\n// c\nl1\nl2', 'a multi-line scope copies its own break: untouched');
  const r = f1mod.resolveTarget({profile: 'rust', dialect: {}, text: RUST, start: atText(RUST, 'n += 1', 3), end: atText(RUST, 'n += 1', 3)});
  assert.equal(r.ok, true); assert.equal(r.tooLarge, false); assert.deepEqual(r.allowed.map(e => e.label), ['fn parse_header', 'impl Parser', 'file']); assert.equal(r.blocked, null);
  assert.deepEqual(f1mod.resolveTarget({profile: 'cobol', text: 'x', start: 0, end: 0}), {ok: false, reason: 'unsupported-language'});
  assert.deepEqual(f1mod.resolveTarget({profile: 'rust', text: ' \n', start: 0, end: 0}), {ok: false, reason: 'empty-document'});
  const small = f1mod.resolveTarget({profile: 'rust', dialect: {}, text: RUST, start: atText(RUST, 'n += 1', 3), end: atText(RUST, 'n += 1', 3)}, 30);
  assert.equal(small.tooLarge, true); assert.equal(small.allowed.length, 0);
});

// ---------------------------------------------------------------- the page (fake DOM)
class El {
  constructor(tag, root) {
    Object.assign(this, {tag, root, nodeType: 1, children: [], attrs: {}, listeners: {}, parent: null, id: '', className: '', value: '', disabled: false, tabIndex: 0, text: null, lang: ''});
    this.classList = {toggle: (c, on) => { const set = new Set(this.className.split(/\s+/).filter(Boolean)); if (on) set.add(c); else set.delete(c); this.className = [...set].join(' '); }, contains: c => this.className.split(/\s+/).includes(c)};
  }
  setAttribute(k, v) { this.attrs[k] = String(v); if (k === 'id') this.id = String(v); if (k === 'class') this.className = String(v); if (k === 'disabled') this.disabled = true; if (k === 'tabindex') this.tabIndex = Number(v); }
  getAttribute(k) { return k in this.attrs ? this.attrs[k] : null; }
  append(...kids) { for (const k of kids.flat()) { if (k === null || k === undefined) continue; if (typeof k === 'object') { k.parent = this; this.children.push(k); } else this.children.push({isText: true, text: String(k), parent: this}); } }
  replaceChildren(...kids) { this.children = []; this.text = null; this.append(...kids); }
  addEventListener(t, fn) { (this.listeners[t] = this.listeners[t] || []).push(fn); }
  get textContent() { return this.text !== null ? this.text : this.children.map(c => c.isText ? c.text : c.textContent).join(''); }
  set textContent(v) { this.children = []; this.text = String(v); }
  closest(sel) { const role = /\[role=(\w+)\]/.exec(sel)[1]; for (let n = this; n; n = n.parent) if (n.attrs && n.attrs.role === role) return n; return null; }
  focus() { this.root.activeElement = this; }
  find(pred, out = []) { if (pred(this)) out.push(this); for (const c of this.children) if (!c.isText) c.find(pred, out); return out; }
  fire(type, ev = {}) { for (const fn of this.listeners[type] || []) fn({target: this, key: ev.key, ...ev}); }
}
// Runs the real page script of getHtml() against a minimal DOM; `post` collects the messages the page sends to the extension.
function loadPage() {
  const html = panelMod.getHtml('csp:', 'NONCE');
  const body = new El('body', null); const root = {activeElement: null};
  body.root = root;
  for (const m of html.matchAll(/<(\w+)([^>]*?)\sid="([^"]+)"([^>]*)>/g)) {   // the static elements of the page
    const el = new El(m[1], root), attrs = `${m[2]} ${m[4]}`;
    el.setAttribute('id', m[3]);
    for (const a of attrs.matchAll(/([\w-]+)="([^"]*)"/g)) el.setAttribute(a[1], a[2]);
    body.append(el);
  }
  const all = () => body.find(() => true);
  const doc = {documentElement: {lang: ''}, get activeElement() { return root.activeElement; }, getElementById: id => all().find(e => e.id === id) || null, createElement: tag => new El(tag, root), createTextNode: t => ({isText: true, nodeType: 3, text: String(t)})};
  const posted = [], handlers = {};
  const win = {addEventListener: (t, fn) => { handlers[t] = fn; }};
  const script = /<script nonce="NONCE">([\s\S]*?)<\/script>/.exec(html)[1];
  vm.runInNewContext(script, {document: doc, window: win, acquireVsCodeApi: () => ({postMessage: m => posted.push(JSON.parse(JSON.stringify(m)))})}); // (objects of the page's own realm: copied)
  const page = {posted, body, $: id => doc.getElementById(id), feed: state => handlers.message({data: state}), text: id => doc.getElementById(id) ? doc.getElementById(id).textContent : null,
    ids: root2 => body.find(e => e.id && (!root2 || e.closest && true)).map(e => e.id), panel: name => doc.getElementById('panel-' + name)};
  return page;
}

test('ANN-82 the page script: static markup, tab ids, i18n keys in every locale, and the whole-file element ids the existing UAT specs rely on', () => {
  const html = panelMod.getHtml('csp:', 'NONCE');
  for (const id of ['tab-full', 'tab-insert', 'tab-annotate', 'panel-full', 'panel-insert', 'panel-annotate', 'intent', 'generate', 'cancel', 'banner', 'usage']) assert.ok(html.includes(`id="${id}"`), id);
  assert.match(html, /Content-Security-Policy" content="default-src 'none'; style-src 'nonce-NONCE'; script-src 'nonce-NONCE';"/);
  const script = /<script nonce="NONCE">([\s\S]*?)<\/script>/.exec(html)[1];
  assert.doesNotThrow(() => new vm.Script(script), 'the page script parses');
  assert.ok(!/innerHTML|insertAdjacentHTML|eval\(|document\.write/.test(script), 'model text only ever reaches textContent');
  for (const key of panelMod.I18N_KEYS) for (const loc of i18n.LOCALES) assert.notEqual(i18n.t(loc, key), key, `${loc} ${key}`);
  // keys the page needs and every f1.ann.* key have all three languages
  for (const key of Object.keys(i18n.S).filter(k => k.startsWith('f1.ann.'))) for (const idx of [0, 1, 2]) assert.ok(i18n.S[key][idx] && i18n.S[key][idx].trim(), `${key}[${idx}]`);
});

test('ANN-83 the page renders the annotate tab from a real session state, posts the right messages, and keeps the whole-file ids', async () => {
  const h = rig({reply: () => replyOf([1, 'Header.'], [3, 'Loop.'])});
  await h.open(atText(RUST, 'n += 1', 3));
  const page = loadPage();
  page.feed(h.state());
  assert.equal(page.$('panel-annotate').classList.contains('hidden'), false); assert.equal(page.$('panel-full').classList.contains('hidden'), true);
  assert.equal(page.$('tab-annotate').getAttribute('aria-selected'), 'true'); assert.equal(page.text('tab-annotate'), 'C. Annotate scope');
  assert.equal(page.text('ann-scope'), 'Scope: fn parse_header (lines 12-22, 11 lines)');
  assert.equal(page.text('ann-crumbs'), 'fn parse_header > impl Parser > file');
  assert.equal(page.$('ann-crumbs').getAttribute('aria-label'), 'Scope path, innermost first');
  assert.equal(page.$('ann-narrower').disabled, true); assert.equal(page.$('ann-wider').disabled, false);
  assert.equal(page.$('ann-opt-comment').getAttribute('checked'), ''); assert.equal(page.$('ann-opt-graph').getAttribute('checked'), null);
  const status = page.$('panel-annotate').find(e => e.className === 'status')[0];
  assert.equal(status.getAttribute('data-status'), 'ready'); assert.match(status.textContent, /^Proposed: 2 comment\(s\)\./);
  const hunks = page.$('panel-annotate').find(e => e.tag === 'fieldset');
  assert.equal(hunks.length, 2); assert.deepEqual(hunks.map(x => x.getAttribute('data-hunk')), ['h1', 'h2']);
  assert.ok(page.$('cb-annotate-h1') && page.$('cb-annotate-h2')); assert.equal(page.$('cb-h1'), null, 'no whole-file ids when the full tab has no hunks');
  assert.ok(page.$('preview-annotate') && page.$('validation-annotate') && page.$('open-diff-annotate') && page.$('preview-title-annotate'));
  assert.equal(page.$('validation-annotate').getAttribute('data-level'), 'ok');
  assert.equal(page.$('apply-annotate').disabled, false); assert.equal(page.text('regen-annotate'), 'Regenerate');
  assert.equal(page.$('generate').disabled, false);
  // clicks
  page.$('ann-wider').fire('click'); page.$('ann-opt-graph').fire('change', {target: {checked: true}}); page.$('cb-annotate-h2').fire('change', {target: {checked: false}});
  page.$('open-diff-annotate').fire('click'); page.$('apply-annotate').fire('click'); page.$('regen-annotate').fire('click');
  page.$('intent').value = 'why'; page.$('generate').fire('click'); page.$('intent').fire('keydown', {key: 'Enter'});
  page.$('tab-annotate').fire('click'); page.$('tab-full').fire('click');
  assert.deepEqual(page.posted, [{type: 'ready'}, {type: 'scope', move: 'wider'}, {type: 'option', name: 'graph', on: true}, {type: 'toggleHunk', tab: 'annotate', id: 'h2', on: false},
    {type: 'openDiff', tab: 'annotate'}, {type: 'applyAnnotate'}, {type: 'regenerate', tab: 'annotate'}, {type: 'generate', intent: 'why'}, {type: 'generate', intent: 'why'},
    {type: 'tab', tab: 'annotate'}, {type: 'tab', tab: 'full'}]);
  // after Wider: idle with the "Add comments" button; at the end of the chain Wider is off; a blocked entry is crossed out
  await h.send({type: 'scope', move: 'wider'}); await h.send({type: 'scope', move: 'wider'});
  page.feed(h.state());
  assert.equal(page.$('ann-wider').disabled, true); assert.equal(page.text('regen-annotate'), 'Add comments');
  assert.equal(page.$('panel-annotate').find(e => e.tag === 'fieldset').length, 0);
  // both options off: the page shows what to tick and the run button is off
  await h.send({type: 'option', name: 'comment', on: false}); page.feed(h.state());
  assert.equal(page.$('regen-annotate').disabled, true); assert.equal(page.$('generate').disabled, true);
  assert.equal(page.$('panel-annotate').find(e => e.className === 'status')[0].textContent, 'Tick Comments or Cypher graph to say what to add.');
  // keyboard focus survives the rebuild of the panels
  page.$('ann-opt-graph').focus(); page.feed(h.state()); assert.equal(page.$('ann-opt-graph').id, 'ann-opt-graph');
});

test('ANN-84 the page: unavailable tab (message only), edit mode default tab, and the whole-file tab keeps #cb-h1 #preview #validation #open-diff #apply-full #regen-full', async () => {
  const u = rig({language: 'unknown', languageId: 'plaintext'}); await u.open(0);
  const page = loadPage(); page.feed(u.state());
  assert.equal(page.$('ann-scope'), null); assert.equal(page.$('ann-opt-comment'), null); assert.equal(page.$('regen-annotate'), null);
  assert.equal(page.$('panel-annotate').find(e => e.className === 'status')[0].getAttribute('data-status'), 'unavailable');
  assert.match(page.text('panel-annotate'), /not available for the language "unknown"/);
  // edit mode: whole-file tab first, full proposal ids unchanged
  const AFTER = RUST.replace('let mut n = 0;', 'let mut n = 0; // counter');
  const h = rig({config: {f1Mode: 'edit'}, handler: call => okJson(responsesBody(JSON.stringify(call.body.model === 'm-full' ? {fullFile: AFTER, explanation: 'x'} : {insertCandidates: ['// hi\n']})))});
  h.ui.choose.consent = WORKSPACE;
  await h.open(atText(RUST, 'n += 1', 3)); await h.send({type: 'generate', intent: ''});
  const p = loadPage(); p.feed(h.state());
  assert.equal(p.$('panel-full').classList.contains('hidden'), false); assert.equal(p.$('panel-annotate').classList.contains('hidden'), true);
  for (const id of ['cb-h1', 'preview', 'preview-title', 'validation', 'open-diff', 'apply-full', 'regen-full']) assert.ok(p.$(id), id);
  p.$('cb-h1').fire('change', {target: {checked: false}}); p.$('open-diff').fire('click'); p.$('apply-full').fire('click');
  assert.deepEqual(p.posted.slice(1), [{type: 'toggleHunk', tab: 'full', id: 'h1', on: false}, {type: 'openDiff', tab: 'full'}, {type: 'applyFull'}]);
  assert.equal(p.text('regen-annotate'), 'Add comments', 'idle and reachable in edit mode');
  assert.equal(p.$('generate').disabled, false);
});
