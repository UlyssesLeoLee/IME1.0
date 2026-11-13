'use strict';
// The F1 test rig of ut-f1-annotate.test.js: the fakes (document with undo, editor with decorations, dialogs,
// webview panel), the runtime (the REAL permission broker, privacy gate, edit coordinator, scope resolver,
// annotation functions and AI client; the provider is a recording fake fetch), the fixtures and the helpers.
// The lines were moved here byte for byte, so the behaviour is the same; a second test file (the F1 latency
// measurement, ST-040) uses the same doubles.
const path = require('node:path');
const assert = require('node:assert/strict');
const {core, makeFetch, okJson, responsesBody, sleep} = require('./helpers');
const {createEngine} = require('../../editor-bridges/vscode/src/engine');
const f1mod = require('../../editor-bridges/vscode/src/f1');
const settingsMod = core('settings'), aiMod = core('ai'), i18n = core('i18n');
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
function rig({language = 'rust', text = RUST, fileName, languageId = 'rust', config = {}, reply, handler, key = {'pime-key:mock': 'KEY-123456'}, dirSymbols = [], scheme = 'file', engine: given = null} = {}) {
  const st = {enabled: true, aiEnabled: true, trusted: true, locale: 'en-US', key, language,
    config: {f1Mode: 'annotate', f1AnnotateOnOpen: true, autoComment: true, graphComment: false, graphCommentScope: 'directory', ...config}};
  const ui = {warnings: [], errors: [], infos: [], dialogs: [], panels: [], types: [], commands: [], applyCalls: 0, choose: {consent: ONCE, sensitive: 'Redact and send'}, providers: new Map()};
  const doc = new FakeDoc(text, {languageId, scheme, ...(fileName ? {fileName} : {})});
  const engine = given || createEngine('js', {globalState: store()});   // `given`: a started Rust engine (ANN-85)
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
  const ai = new aiMod.AiClient({fetchImpl, budget: given ? new aiMod.Budget() : engine.budget, diagnostics, isEnabled: () => st.aiEnabled, authorize}); // (the HTTP call stays in JS with either engine)
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

module.exports = {
  SRC, ONCE, WORKSPACE, store, EOLRE,
  RUST, PARSE_HEADER, atText, SECRET_FILE, SECRET_HIGH_FILE,
  FakeDoc, Range, makeEditor, rig,
  until, replyOf, added,
};
