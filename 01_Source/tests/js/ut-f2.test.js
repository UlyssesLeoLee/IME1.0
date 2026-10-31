'use strict';
// F2 prompt orchestration (src/f2.js): classification -> scenario / grant scope, consent, the outbound sensitivity gate, redaction, the review
// reply, the clipboard and the notices. The `vscode` module and the runtime are injected fakes (editor, dialogs, progress, clipboard, notice);
// the privacy gate, the permission broker and the AI client are the REAL JS ones, and the provider is a recording fake fetch, so "no secret
// reaches the provider" is checked on the actual request body. The pure logic is tested in ut-promptcraft.test.js.
// Nothing here ran inside a real VS Code window or touched the real clipboard (see the UAT spec f2-prompt.spec.js, which is a separate run).
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {core, makeFetch, okJson, responsesBody, sleep} = require('./helpers');
const {createEngine} = require('../../editor-bridges/vscode/src/engine');
const f2mod = require('../../editor-bridges/vscode/src/f2');
const {TransientNotice} = require('../../editor-bridges/vscode/src/ui/notice');
const settingsMod = core('settings'), aiMod = core('ai'), i18n = core('i18n'), promptcraft = core('promptcraft');
const {Diagnostics} = core('diagnostics');
const {fail, codeOf} = core('errors');

const root = path.join(__dirname, '..', '..', 'editor-bridges', 'vscode');
const SENTINEL = 'CLIPBOARD-BEFORE-F2';
const ONCE = 'Authorize this request', WORKSPACE = 'Authorize for this workspace (24 h)';
const HEADS = ['Who', 'What', 'When', 'Where', 'Why', 'How'];
const store = () => { const m = new Map(); return {get: k => m.get(k), update: (k, v) => { m.set(k, v); return Promise.resolve(); }}; };

// ---------------------------------------------------------------- fixtures
const RUST = 'fn read_port(s: &str) -> u16 {\n    s.parse::<u16>().unwrap()\n}\n\nfn main() {\n    println!("{}", read_port("80"));\n}\n';
const RUST_SELECTION = RUST.slice(0, RUST.indexOf('\n}\n') + 2); // the first function: lines 1-3
const FIELDS = {who: 'A Rust engineer.', what: 'Replace the unwrap with error handling.', when: 'Before the next release.', where: 'read_port in main.rs', why: 'A bad port string aborts the program.', how: 'Return a Result and propagate it with the question mark operator.'};
const CODE_REPLY = {needsOptimization: true, findings: [{line: 2, evidence: 'parse::<u16>().unwrap()', issue: 'unwrap panics on malformed input', impact: 'the process aborts', severity: 'high'}], w5h1: FIELDS};
const PROSE = 'We need a login page for our customers, and it should be fast.';
const PROSE_REPLY = {needsOptimization: true, findings: [{element: 'when', issue: 'no deadline is given'}], w5h1: {who: 'A web developer.', what: 'Build a fast customer login page.', when: 'Ask the user.', where: 'The customer portal.', why: 'Customers must sign in.', how: 'Describe the steps and the acceptance criteria.'}};
const NO_CHANGE = {needsOptimization: false, findings: []};
const SECRET_MEDIUM = 'fn connect() {\n    let password = "hunter2hunter2";\n    login(password);\n}\n';
const SECRET_HIGH = 'fn connect() {\n    let key = "sk-abcdefghijklmnopqrstuvwxyz0123";\n    login(key);\n}\n';

// ---------------------------------------------------------------- fakes
function makeEditor(text, a, b, {languageId = 'rust', fileName = 'C:\\proj\\src\\main.rs', untitled = false} = {}) {
  const pos = off => { const before = text.slice(0, off); return {line: before.split(/\r\n|\r|\n/).length - 1, character: off - (Math.max(before.lastIndexOf('\n'), before.lastIndexOf('\r')) + 1)}; };
  const offsetAt = p => { let off = 0; const lines = text.split(/(?<=\n)/); for (let i = 0; i < p.line; i++) off += lines[i].length; return off + p.character; };
  const doc = {languageId, fileName, isUntitled: untitled, uri: {scheme: 'file'}, getText: r => r ? text.slice(offsetAt(r.start), offsetAt(r.end)) : text, offsetAt};
  return {document: doc, selection: {start: pos(a), end: pos(b), isEmpty: a === b}};
}
const selectAll = (text, opts) => makeEditor(text, 0, text.length, opts);
const selectText = (file, part, opts) => { const a = file.indexOf(part); assert.ok(a >= 0, 'selection fixture'); return makeEditor(file, a, a + part.length, opts); };

// handler(call, n) -> response (or a promise); default: the reply of the test as a Responses API body
function rig({reply = CODE_REPLY, language = 'rust', handler = null, noticeImpl = null, realNotice = false} = {}) {
  const st = {enabled: true, aiEnabled: true, trusted: true, locale: 'en-US', key: {'pime-key:mock': 'KEY-123456'}, language};
  const ui = {warnings: [], errors: [], infos: [], dialogs: [], progress: [], toasts: [], dismissed: 0, handlers: [], choose: {consent: ONCE, sensitive: 'Redact and send'}, log: []};
  const clip = {text: SENTINEL, writes: []};
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
  const fetchImpl = makeFetch(handler || ((call, n) => okJson(responsesBody(JSON.stringify(typeof reply === 'function' ? reply(call, n) : reply)))));
  const ai = new aiMod.AiClient({fetchImpl, budget: engine.budget, diagnostics, isEnabled: () => st.aiEnabled, authorize});
  const settings = settingsMod.effective({providers: {mock: {api: 'openai-responses', endpoint: 'https://api.a.example/v1/responses'}},
    scenarios: {f1Full: {provider: 'mock', model: 'm-full'}, f1Insert: {provider: 'mock', model: 'm-insert'}, realtime: {provider: 'mock', model: 'm-rt'}}});
  const rt = {engine, broker: engine.broker, ai, diagnostics, settings, get locale() { return st.locale; }, ctx: {secrets: {get: async id => st.key[id]}}};
  const vscode = {
    workspace: {get isTrusted() { return st.trusted; }},
    window: {
      activeTextEditor: null,
      showWarningMessage: async (msg, opts, ...buttons) => {
        if (!(opts && opts.modal)) { ui.warnings.push(msg); return undefined; }
        ui.dialogs.push({msg, detail: opts.detail, buttons});
        const pick = ui.choose[/Possible secrets/.test(msg) ? 'sensitive' : 'consent'];
        return typeof pick === 'function' ? pick(msg) : pick;
      },
      showErrorMessage: async msg => { ui.errors.push(msg); },
      showInformationMessage: async msg => { ui.infos.push(msg); },
      withProgress: (options, task) => {
        ui.progress.push(options);
        const token = {isCancellationRequested: false, onCancellationRequested: fn => { ui.handlers.push(fn); return {dispose() {}}; }};
        return Promise.resolve(task({report() {}}, token));
      }
    },
    env: {clipboard: {writeText: async text => { ui.log.push('clipboard'); clip.writes.push(text); clip.text = text; }}},
    ProgressLocation: {Notification: 15}
  };
  const notice = noticeImpl || (realNotice ? new TransientNotice(vscode) : {
    show: x => { ui.log.push('notice'); ui.toasts.push(x); return {dismissed: Promise.resolve('fake')}; },
    dismiss: () => { ui.dismissed++; }
  });
  const f2 = f2mod.createF2({vscode, rt, L: (k, ...a) => i18n.t(st.locale, k, ...a), errorText: e => i18n.describeError(st.locale, codeOf(e), e && e.message), enabled: () => st.enabled,
    aiEnabled: () => st.aiEnabled, workspaceId, grantProviderId: sc => `${sc.providerId}:${new URL(sc.endpoint).host}`.slice(0, 64), infoFor: () => ({language: st.language, dialect: {}}), notice});
  const run = async editor => { vscode.window.activeTextEditor = editor; await f2.run(); };
  return {f2, st, ui, clip, rt, fetch: fetchImpl, run, notice, vscode};
}
const wire = r => r.fetch.calls.map(c => c.init.body).join('\n');
const sentInput = (r, n = 0) => JSON.parse(r.fetch.calls[n].body.input);
const headingOrder = (prompt, heads = HEADS) => heads.map(h => prompt.indexOf(`## ${h}\n`));
const increasing = xs => xs.every((x, i) => x >= 0 && (i === 0 || x > xs[i - 1]));
const until = async (fn, ms = 3000) => { const end = Date.now() + ms; while (!fn()) { if (Date.now() > end) throw new Error('timed out waiting'); await sleep(5); } };
const hang = (call, n, then) => n === 1 ? new Promise((_, rej) => call.init.signal.addEventListener('abort', () => rej(Object.assign(new Error('aborted'), {name: 'AbortError'})))) : then();
const diagText = r => JSON.stringify(r.rt.diagnostics.events);

// ---------------------------------------------------------------- tests
test('PRM-20 routing: code goes to f1Full with the L3 consent and the numbered file; prose goes to f1Insert with the L1 consent and the selection only', async () => {
  const code = rig({reply: CODE_REPLY}); code.ui.choose.consent = WORKSPACE;
  await code.run(selectText(RUST, RUST_SELECTION));
  assert.equal(code.fetch.calls.length, 1);
  assert.equal(code.fetch.calls[0].body.model, 'm-full', 'scenario f1Full');
  assert.match(code.ui.dialogs[0].msg, /Send the COMPLETE current document/); assert.match(code.ui.dialogs[0].msg, /api\.a\.example/); assert.match(code.ui.dialogs[0].detail, /secrets/i);
  assert.deepEqual(code.rt.broker.list().map(g => g.scope), ['L3']);
  const ci = sentInput(code);
  assert.equal(ci.kind, 'code'); assert.match(ci.numbered, /^1\| fn read_port/m); assert.match(ci.numbered, /^2\|     s\.parse::<u16>\(\)\.unwrap\(\)/m);
  assert.equal(ci.selection.startLine, 1); assert.equal(ci.selection.endLine, 3); assert.equal(ci.fileName, 'main.rs', 'only the base name leaves the machine');
  assert.ok(!code.fetch.calls[0].init.body.includes('C:\\\\proj'), 'no local path in the request');
  assert.ok(code.fetch.calls[0].body.max_output_tokens >= 3000, 'room for the JSON reply');

  const prose = rig({reply: PROSE_REPLY, language: 'unknown'}); prose.ui.choose.consent = WORKSPACE;
  await prose.run(selectAll(PROSE, {languageId: 'markdown', fileName: 'C:\\proj\\notes.md'}));
  assert.equal(prose.fetch.calls[0].body.model, 'm-insert', 'scenario f1Insert');
  assert.match(prose.ui.dialogs[0].msg, /Send a nearby code excerpt/);
  assert.deepEqual(prose.rt.broker.list().map(g => g.scope), ['L1']);
  const pi = sentInput(prose);
  assert.equal(pi.kind, 'prose'); assert.equal(pi.selection.text, PROSE); assert.equal(pi.numbered, undefined, 'a prose request carries no file'); assert.equal(pi.file, undefined);
});

test('PRM-21 code review with findings: the clipboard gets the 5W1H prompt with the checked evidence, the notice comes AFTER the copy, one progress with Cancel', async () => {
  const r = rig({reply: CODE_REPLY});
  await r.run(selectText(RUST, RUST_SELECTION));
  assert.equal(r.clip.writes.length, 1);
  const prompt = r.clip.writes[0];
  assert.ok(increasing(headingOrder(prompt)), 'the six headings, in order');
  assert.equal(promptcraft.verify5W1H(prompt, 'en-US').ok, true);
  assert.match(prompt, /1\. line 2 \[high\]: unwrap panics on malformed input/); assert.match(prompt, /Evidence: `parse::<u16>\(\)\.unwrap\(\)`/);
  assert.match(prompt, /```rust\nfn read_port/); assert.match(prompt, /Return a Result and propagate it/);
  assert.match(prompt, /File: `main\.rs`/); assert.match(prompt, /Selected lines: 1-3 of 8/);
  assert.deepEqual(r.ui.log, ['clipboard', 'notice'], 'show() is the last step');
  assert.equal(r.ui.toasts[0].kind, 'info'); assert.equal(r.ui.toasts[0].text, `Copied ${prompt.length} characters: paste it into your AI assistant.`);
  assert.equal(r.ui.progress.length, 1); assert.equal(r.ui.progress[0].cancellable, true); assert.equal(r.ui.progress[0].location, 15);
  assert.deepEqual(r.ui.errors, []); assert.deepEqual(r.ui.warnings, []);
  assert.ok(r.rt.diagnostics.events.some(e => e.component === 'f2' && e.error_code === 'OK' && e.request_type === 'f2.code'));
  assert.equal(r.rt.engine.budget.snapshot().requests, 1, 'exactly one request');
});

test('PRM-22 prose: the language of the text picks the prompt language (Chinese text -> Chinese headings), a code file with prose in a comment stays prose', async () => {
  const zh = '我们需要为客户做一个登录页面，并且要尽快完成。';
  const r = rig({reply: PROSE_REPLY, language: 'unknown'});
  await r.run(selectAll(zh, {languageId: 'markdown', fileName: 'C:\\proj\\notes.md'}));
  const prompt = r.clip.writes[0];
  assert.ok(increasing(headingOrder(prompt, ['谁', '做什么', '何时', '何处', '为什么', '怎么做'])), prompt);
  assert.equal(promptcraft.verify5W1H(prompt, 'zh-CN').ok, true);
  assert.match(sentInput(r).locale, /^zh-CN$/);
  assert.match(r.fetch.calls[0].body.instructions, /Simplified Chinese/);
  assert.equal(r.ui.toasts[0].text, `Copied ${prompt.length} characters: paste it into your AI assistant.`, 'the toast follows the UI language (en-US here), the prompt the text');
  // a sentence selected inside a comment of a Rust file is prose, not code
  const file = 'fn main() {\n    // We should check every port number before we parse it, and report the error to the user.\n    run();\n}\n';
  const c = rig({reply: PROSE_REPLY, language: 'rust'});
  await c.run(selectText(file, 'We should check every port number before we parse it, and report the error to the user.'));
  assert.equal(sentInput(c).kind, 'prose'); assert.equal(c.fetch.calls[0].body.model, 'm-insert');
});

test('PRM-23 AI path not available: the UNREVIEWED template is copied, the notice says why, and not a single request is sent', async () => {
  const cases = [
    ['AI off', r => { r.st.aiEnabled = false; }, 'AI is off'],
    ['untrusted workspace', r => { r.st.trusted = false; }, 'this workspace is not trusted'],
    ['no key', r => { r.st.key = {}; }, 'no AI provider, model and key are configured'],
    ['consent declined', r => { r.ui.choose.consent = undefined; }, 'sending was not authorised']
  ];
  for (const [label, setup, reason] of cases) {
    for (const [kind, editor, language] of [['code', selectText(RUST, RUST_SELECTION), 'rust'], ['prose', selectAll(PROSE, {languageId: 'markdown', fileName: 'C:\\proj\\a.md'}), 'unknown']]) {
      const r = rig({language}); setup(r);
      await r.run(editor);
      assert.equal(r.fetch.calls.length, 0, `${label}/${kind}: zero requests`);
      assert.equal(r.clip.writes.length, 1, `${label}/${kind}`);
      assert.match(r.clip.writes[0].split('\n')[0], /^UNREVIEWED TEMPLATE: no AI looked at this selection/, `${label}/${kind}: first line`);
      assert.equal(promptcraft.verify5W1H(r.clip.writes[0], 'en-US').ok, true);
      assert.ok(r.clip.writes[0].includes(kind === 'code' ? 'fn read_port' : PROSE), 'the selection is in the template');
      assert.deepEqual(r.ui.log, ['clipboard', 'notice']);
      assert.equal(r.ui.toasts[0].kind, 'warn'); assert.match(r.ui.toasts[0].text, /UNREVIEWED template/); assert.ok(r.ui.toasts[0].text.includes(reason), `${label}: ${r.ui.toasts[0].text}`);
      assert.deepEqual(r.ui.errors, []);
    }
  }
  // a file over 2 MiB is not sent for review (nothing is truncated silently): template, reason named
  const big = RUST + '// ' + 'x'.repeat(2 * 1024 * 1024) + '\n';
  const r = rig(); await r.run(selectText(big, RUST_SELECTION));
  assert.equal(r.fetch.calls.length, 0); assert.match(r.clip.writes[0], /^UNREVIEWED TEMPLATE/); assert.match(r.ui.toasts[0].text, /larger than 2 MiB/);
  // declining leaves no grant behind
  const d = rig(); d.ui.choose.consent = undefined; await d.run(selectText(RUST, RUST_SELECTION));
  assert.deepEqual(d.rt.broker.list(), []);
});

test('PRM-24 redact: the secret never reaches the provider or the clipboard, the markers stay in the prompt, invented markers are dropped', async () => {
  // medium risk (credential assignment): redacted without a dialog
  const reply = {needsOptimization: true, findings: [{line: 2, evidence: 'let password = "PIME_REDACTED_1";', issue: 'a password is hard-coded', impact: 'anyone with the source has it', severity: 'high'}],
    w5h1: {...FIELDS, why: 'Because PIME_REDACTED_9 is exposed.', how: 'Move PIME_REDACTED_1 to a secret store and read it at start-up.'}};
  const r = rig({reply});
  await r.run(selectAll(SECRET_MEDIUM));
  assert.equal(r.ui.dialogs.filter(d => /Possible secrets/.test(d.msg)).length, 0, 'medium risk is redacted silently (as in F1)');
  assert.ok(!wire(r).includes('hunter2hunter2'), 'the secret is not in the request body');
  assert.ok(wire(r).includes('PIME_REDACTED_1'));
  const sent = sentInput(r);
  assert.ok(sent.numbered.includes('PIME_REDACTED_1') && sent.selection.text.includes('PIME_REDACTED_1'));
  const prompt = r.clip.writes[0];
  assert.ok(!prompt.includes('hunter2hunter2'), 'never on the clipboard');
  assert.ok(prompt.includes('let password = "PIME_REDACTED_1";'), 'the material and the evidence keep the marker');
  assert.ok(!prompt.includes('PIME_REDACTED_9'), 'a marker the model made up is dropped with its field');
  assert.match(prompt, /Move PIME_REDACTED_1 to a secret store/, 'a known marker is kept as a marker (it is not restored)');
  assert.match(prompt, /Scope: 1 secret-like value\(s\) replaced by PIME_REDACTED_n markers \(not restored\)/);
  assert.match(r.ui.toasts[0].text, /^Copied \d+ characters: paste it into your AI assistant\. 1 secret-like value\(s\) replaced by PIME_REDACTED_n markers/);
  assert.equal(promptcraft.verify5W1H(prompt, 'en-US').ok, true);

  // high risk: the user decides; "Redact and send" behaves the same
  const h = rig({reply: {needsOptimization: true, findings: [{line: 2, evidence: 'let key = "PIME_REDACTED_1";', issue: 'a key is hard-coded', impact: 'leak', severity: 'high'}], w5h1: FIELDS}});
  await h.run(selectAll(SECRET_HIGH));
  const d = h.ui.dialogs.find(x => /Possible secrets found \(api-key×1\)/.test(x.msg)); assert.ok(d, h.ui.dialogs.map(x => x.msg).join('|'));
  assert.deepEqual(d.buttons, ['Redact and send', 'Send unredacted (I accept the risk)']);
  assert.ok(!wire(h).includes('sk-abcdefghijklmnopqrstuvwxyz0123')); assert.ok(!h.clip.writes[0].includes('sk-abcdef'));

  // "Send unredacted" is the user's explicit choice: only then do the bytes leave (and the last gate in the client is told so)
  const raw = rig({reply: {needsOptimization: true, findings: [{line: 2, evidence: 'let key = "sk-abcdefghijklmnopqrstuvwxyz0123";', issue: 'a key is hard-coded', impact: 'leak', severity: 'high'}], w5h1: FIELDS}});
  raw.ui.choose.sensitive = 'Send unredacted (I accept the risk)';
  await raw.run(selectAll(SECRET_HIGH));
  assert.ok(wire(raw).includes('sk-abcdefghijklmnopqrstuvwxyz0123')); assert.ok(raw.clip.writes[0].includes('sk-abcdef'));
});

test('PRM-25 needsOptimization:false is the genuine "nothing to fix" answer: a notice (own wording for code and prose), the clipboard UNTOUCHED', async () => {
  for (const [kind, editor, language, re] of [['code', selectText(RUST, RUST_SELECTION), 'rust', /^No optimisation needed: the AI found no verified problem in the selected code\./],
    ['prose', selectAll(PROSE, {languageId: 'markdown', fileName: 'C:\\p\\a.md'}), 'unknown', /^No optimisation needed: the text already answers who, what, when, where, why and how\./]]) {
    const r = rig({reply: NO_CHANGE, language});
    await r.run(editor);
    assert.equal(r.fetch.calls.length, 1, kind);
    assert.equal(r.clip.writes.length, 0, `${kind}: nothing written`); assert.equal(r.clip.text, SENTINEL);
    assert.equal(r.ui.toasts.length, 1); assert.match(r.ui.toasts[0].text, re); assert.equal(r.ui.toasts[0].kind, 'info');
    assert.deepEqual(r.ui.errors, []); assert.deepEqual(r.ui.warnings, []);
    assert.ok(r.rt.diagnostics.events.some(e => e.component === 'f2' && e.request_type === `f2.${kind}.noopt`));
  }
  // with the real TransientNotice: the toast is the LAST progress notification, after the request's own progress
  const real = rig({reply: NO_CHANGE, realNotice: true});
  await real.run(selectText(RUST, RUST_SELECTION));
  assert.equal(real.ui.progress.length, 2); assert.equal(real.ui.progress[0].cancellable, true); assert.match(real.ui.progress[0].title, /5W1H/);
  assert.equal(real.ui.progress[1].cancellable, false); assert.match(real.ui.progress[1].title, /No optimisation needed/); assert.equal(real.notice.active, true);
  real.notice.dispose();
});

test('PRM-26 an unusable reply is an ERROR (warning, clipboard untouched), never "no optimisation needed"', async () => {
  for (const text of ['this is not json at all', '{"findings": []}', '{"needsOptimization": "no"}', '{"needsOptimization": false, "findings": "x"}']) {
    const r = rig({handler: () => okJson(responsesBody(text))});
    await r.run(selectText(RUST, RUST_SELECTION));
    assert.equal(r.clip.writes.length, 0); assert.equal(r.ui.toasts.length, 0, 'no notice, in particular not the "nothing to optimise" one');
    assert.deepEqual(r.ui.warnings, ['The AI did not return the requested JSON object, so nothing was copied. Try again or choose another model.']);
    assert.ok(r.rt.diagnostics.events.some(e => e.component === 'f2' && e.error_code === 'OUTPUT_INVALID'));
  }
});

test('PRM-27 findings the file does not contain are dropped: none left -> nothing copied; some left -> only those', async () => {
  const fake = {line: 2, evidence: 'this text is not in the file', issue: 'invented problem', impact: 'none', severity: 'high'};
  const none = rig({reply: {needsOptimization: true, findings: [fake, {...fake, line: 99}], w5h1: FIELDS}});
  await none.run(selectText(RUST, RUST_SELECTION));
  assert.equal(none.clip.writes.length, 0); assert.equal(none.ui.toasts.length, 1);
  assert.equal(none.ui.toasts[0].kind, 'warn'); assert.match(none.ui.toasts[0].text, /none of them could be found in your file, so nothing was copied/);
  assert.doesNotMatch(none.ui.toasts[0].text, /No optimisation needed/, 'not reported as "nothing to optimise"');
  const some = rig({reply: {needsOptimization: true, findings: [fake, CODE_REPLY.findings[0]], w5h1: FIELDS}});
  await some.run(selectText(RUST, RUST_SELECTION));
  assert.equal(some.clip.writes.length, 1); assert.ok(!some.clip.writes[0].includes('invented problem')); assert.ok(some.clip.writes[0].includes('unwrap panics'));
});

test('PRM-28 a sensitive selection or file is never copied and never sent when the user does not accept it', async () => {
  // AI on: the user closes the sensitive-data dialog
  const a = rig(); a.ui.choose.sensitive = undefined;
  await a.run(selectAll(SECRET_HIGH));
  assert.equal(a.fetch.calls.length, 0); assert.equal(a.clip.writes.length, 0);
  assert.equal(a.ui.toasts[0].kind, 'warn'); assert.match(a.ui.toasts[0].text, /^Looks sensitive \(api-key×1\): nothing was sent to an AI and nothing was copied\.$/);
  // a credential file by name, harmless text. The read gate (src/filegate.js) asks first and the user allows reading; the OUTBOUND gate still refuses
  const b = rig({language: 'unknown'}); b.ui.choose.sensitive = undefined; b.ui.choose.consent = msg => /secret file/.test(msg) ? 'Allow once' : ONCE;
  await b.run(selectAll('API_KEY=placeholder\nMODE=dev\n', {languageId: 'properties', fileName: 'C:\\proj\\.env'}));
  assert.equal(b.fetch.calls.length, 0); assert.equal(b.clip.writes.length, 0); assert.match(b.ui.toasts[0].text, /sensitive file name/);
  // AI off: the offline template is a clipboard write too, so a high-risk selection is refused there as well (no dialog)
  const c = rig(); c.st.aiEnabled = false;
  await c.run(selectAll(SECRET_HIGH));
  assert.equal(c.clip.writes.length, 0); assert.equal(c.ui.dialogs.length, 0); assert.match(c.ui.toasts[0].text, /nothing was copied/);
  // AI off + medium risk: the template is copied with the value replaced by a marker
  const d = rig(); d.st.aiEnabled = false;
  await d.run(selectAll(SECRET_MEDIUM));
  assert.equal(d.clip.writes.length, 1); assert.ok(!d.clip.writes[0].includes('hunter2hunter2')); assert.ok(d.clip.writes[0].includes('PIME_REDACTED_1'));
  assert.match(d.clip.writes[0], /^UNREVIEWED TEMPLATE/); assert.match(d.ui.toasts[0].text, /PIME_REDACTED_n markers/);
  assert.equal(d.fetch.calls.length, 0);
});

test('PRM-29 a second F2 cancels the first: only the newer answer reaches the clipboard, the older one ends silently', async () => {
  const r = rig({handler: (call, n) => hang(call, n, () => okJson(responsesBody(JSON.stringify(CODE_REPLY))))});
  r.ui.choose.consent = WORKSPACE;
  const editor = selectText(RUST, RUST_SELECTION);
  const first = r.run(editor);
  await until(() => r.fetch.calls.length === 1);
  const second = r.run(editor);
  await Promise.all([first, second]);
  assert.equal(r.fetch.calls.length, 2); assert.equal(r.clip.writes.length, 1, 'exactly one copy');
  assert.deepEqual(r.ui.errors, [], 'the cancelled request is not an error'); assert.deepEqual(r.ui.warnings, []);
  assert.equal(r.ui.toasts.length, 1); assert.match(r.ui.toasts[0].text, /^Copied \d+ characters/);
  assert.equal(r.f2.busy, false);
});

test('PRM-30 Cancel on the progress notification ends the request silently with the clipboard untouched', async () => {
  const r = rig({handler: (call, n) => hang(call, n, () => okJson(responsesBody('{}')))});
  const done = r.run(selectText(RUST, RUST_SELECTION));
  await until(() => r.fetch.calls.length === 1 && r.ui.handlers.length === 1);
  r.ui.handlers[0]();
  await done;
  assert.equal(r.clip.writes.length, 0); assert.deepEqual(r.ui.errors, []); assert.deepEqual(r.ui.warnings, []); assert.equal(r.ui.toasts.length, 0);
  assert.equal(r.f2.busy, false);
});

test('PRM-31 gates: disabled, no selection (palette), blank selection and an over-large selection stop before anything is classified or sent', async () => {
  const none = rig();
  await none.run(makeEditor(RUST, 3, 3));
  assert.equal(none.ui.toasts.length, 1); assert.equal(none.ui.toasts[0].kind, 'info'); assert.match(none.ui.toasts[0].text, /^Select some text or code first/);
  await none.run(null);
  assert.equal(none.ui.toasts.length, 2, 'no editor: same advice');
  const blank = rig(); await blank.run(makeEditor('a  \n   \nb', 3, 8));
  assert.match(blank.ui.toasts[0].text, /^Select some text or code first/);
  const off = rig(); off.st.enabled = false; await off.run(selectAll(RUST));
  assert.equal(off.ui.warnings.length, 1); assert.match(off.ui.warnings[0], /Programming IME is disabled/);
  const huge = 'x'.repeat(f2mod.LIMITS.selectionChars + 1);
  const big = rig(); await big.run(selectAll(huge, {languageId: 'plaintext'}));
  assert.match(big.ui.toasts[0].text, /^The selection has 200001 characters; the limit is 200000\./); assert.equal(big.ui.toasts[0].kind, 'warn');
  for (const r of [none, blank, off, big]) { assert.equal(r.fetch.calls.length, 0); assert.equal(r.clip.writes.length, 0); assert.equal(r.ui.dialogs.length, 0); }
  // exactly at the limit is accepted
  const ok = rig({language: 'unknown'}); ok.st.aiEnabled = false; await ok.run(selectAll('word '.repeat(f2mod.LIMITS.selectionChars / 5), {languageId: 'plaintext'}));
  assert.equal(ok.clip.writes.length, 1);
});

test('PRM-32 context keys and manifest: F2 only with enabled + the chosen binding + a language that is not excluded, and only with a selection', () => {
  const {contextKeys} = f2mod;
  for (const enabled of [false, true]) for (const binding of ['f2', 'shift+f2', 'none', undefined]) for (const excluded of [false, true]) {
    const k = contextKeys({enabled, binding, excluded});
    assert.equal(k.f2, enabled && binding === 'f2' && !excluded, `${enabled}/${binding}/${excluded}`);
    assert.equal(k.shiftF2, enabled && binding === 'shift+f2' && !excluded, `${enabled}/${binding}/${excluded}`);
  }
  const pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
  const keys = pkg.contributes.keybindings.filter(k => k.command === 'programmingIme.f2Prompt');
  assert.deepEqual(keys.map(k => k.key).sort(), ['f2', 'shift+f2']);
  assert.equal(keys.find(k => k.key === 'f2').when, 'editorTextFocus && editorHasSelection && programmingIme.f2Key.f2', 'without a selection F2 keeps renaming');
  assert.equal(keys.find(k => k.key === 'shift+f2').when, 'editorTextFocus && editorHasSelection && programmingIme.f2Key.shiftF2');
  const p = pkg.contributes.configuration.properties;
  assert.deepEqual(p['programmingIme.f2Binding'].enum, ['f2', 'shift+f2', 'none']); assert.equal(p['programmingIme.f2Binding'].default, 'f2');
  assert.match(p['programmingIme.f2Binding'].description, /F2 normally renames a symbol/); assert.match(p['programmingIme.f2Binding'].description, /non-empty selection/); assert.match(p['programmingIme.f2Binding'].description, /Programming IME is enabled/);
  assert.deepEqual(p['programmingIme.f2ExcludedLanguages'].default, []); assert.equal(p['programmingIme.f2ExcludedLanguages'].type, 'array');
  assert.equal(p['programmingIme.enabled'].default, false, 'off by default: F2 keeps renaming');
  assert.ok(pkg.contributes.commands.some(c => c.command === 'programmingIme.f2Prompt' && c.title === '%cmd.f2Prompt%'));
  for (const nls of ['package.nls.json', 'package.nls.zh-cn.json', 'package.nls.ja.json']) assert.ok(JSON.parse(fs.readFileSync(path.join(root, nls), 'utf8'))['cmd.f2Prompt'], nls);
  // extension.js: the keys are refreshed by updateContextKeys, which runs on configuration change, enable toggle and editor change
  const ext = fs.readFileSync(path.join(root, 'src', 'extension.js'), 'utf8');
  assert.match(ext, /setContext', 'programmingIme\.f2Key\.f2', f2Keys\.f2/); assert.match(ext, /setContext', 'programmingIme\.f2Key\.shiftF2', f2Keys\.shiftF2/);
  assert.match(ext, /f2\.contextKeys\(\{enabled: on, binding: cfg\(\)\.get\('f2Binding'\), excluded: f2Excluded\}\)/); assert.match(ext, /cfg\(\)\.get\('f2ExcludedLanguages'\)/);
  assert.match(ext, /reg\('programmingIme\.f2Prompt', \(\) => f2\.run\(\)\)/); assert.match(ext, /f2\.init\(ctx\)/);
  assert.match(ext, /reg\('programmingIme\.enable', async \(\) => \{[^\n]*updateContextKeys\(\)/);
  assert.match(ext, /onDidChangeActiveTextEditor\([^\n]*updateContextKeys\(\)/); assert.match(ext, /onDidChangeConfiguration[\s\S]*?updateContextKeys\(\); updateStatus\(\)/);
  // the notice is owned by the extension: disposed with it
  assert.match(fs.readFileSync(path.join(root, 'src', 'f2.js'), 'utf8'), /ctx\.subscriptions\.push\(notice,/);
});

test('PRM-33 diagnostics and error text never carry the selection, the code, the prompt or a secret; one error message per failure', async () => {
  const MARK = 'zebra_unique_token';
  const file = `fn ${MARK}() {\n    let password = "hunter2hunter2";\n    s.parse::<u16>().unwrap()\n}\n`;
  const reply = {needsOptimization: true, findings: [{line: 3, evidence: 'parse::<u16>().unwrap()', issue: `issue about ${MARK}`, impact: 'x', severity: 'low'}], w5h1: FIELDS};
  for (const label of ['ok', 'http 500']) {
    const r = rig({reply, handler: label === 'ok' ? undefined : () => ({ok: false, status: 500, headers: {get: () => null}, text: async () => ''})});
    await r.run(selectAll(file));
    const text = diagText(r);
    for (const needle of [MARK, 'hunter2', 'unwrap', 'password', 'PIME_REDACTED', 'Evidence']) assert.ok(!text.includes(needle), `${label}: diagnostics contain "${needle}"`);
    for (const e of r.rt.diagnostics.events) assert.ok(['f2', 'ai'].includes(e.component) && /^[A-Za-z0-9_.:-]{0,64}$/.test(e.error_code), JSON.stringify(e));
    if (label === 'http 500') {
      assert.equal(r.ui.errors.length, 1, 'one error message');
      assert.equal(r.ui.errors[0], 'F2 prompt failed, the clipboard was not changed: The provider returned a server error.');
      assert.equal(r.clip.writes.length, 0); assert.ok(r.rt.diagnostics.events.some(e => e.component === 'f2' && e.error_code === 'AI_SERVER_ERROR'));
    }
  }
  // an unexpected exception: the text of an INTERNAL error may quote content, so it is not shown; only the code
  const r = rig({reply});
  r.rt.engine.privacy.assess = () => { throw new Error(`boom ${MARK} hunter2hunter2`); };
  await r.run(selectAll(file));
  assert.equal(r.ui.errors.length, 1); assert.ok(!r.ui.errors[0].includes(MARK) && !r.ui.errors[0].includes('hunter2')); assert.equal(r.ui.errors[0], 'F2 prompt failed, the clipboard was not changed: INTERNAL');
  assert.ok(!diagText(r).includes(MARK)); assert.equal(r.clip.writes.length, 0); assert.equal(r.fetch.calls.length, 0);
});

test('PRM-34 selectionInRedacted: the selection is cut out of the redacted file; a changed edge falls back to whole lines', () => {
  const {selectionInRedacted: cut} = f2mod;
  const orig = ['let a = 1;', 'let password = "hunter2hunter2"; // x', 'let b = 2;'];
  const red = ['let a = 1;', 'let password = "PIME_REDACTED_1"; // x', 'let b = 2;'];
  assert.equal(cut(orig, red, 0, 0, 2, 10), red.join('\n'), 'whole lines');
  assert.equal(cut(orig, red, 0, 4, 2, 5), 'a = 1;\nlet password = "PIME_REDACTED_1"; // x\nlet b', 'columns kept on the first and the last line');
  assert.equal(cut(orig, red, 1, 4, 1, 12), red[1], 'the text after the end column changed (a secret follows): the whole redacted line');
  assert.equal(cut(orig, red, 1, 0, 1, orig[1].length), red[1]);
  assert.equal(cut(orig, red, 1, 17, 1, 37), red[1], 'the start column lies inside the secret: the whole redacted line');
  assert.equal(cut(orig, red, 0, 4, 0, 9), 'a = 1', 'a single unchanged line keeps its columns');
  assert.equal(cut(orig, red, 0, 0, 1, 0), 'let a = 1;\n', 'a selection that ends at column 0 of the next line takes nothing of that line, redacted or not');
  assert.equal(cut(orig, red, 1, orig[1].length, 2, 5), '\nlet b', 'a selection that starts at the end of a redacted line takes nothing of it');
  const same = ['one', 'two', 'three'];
  assert.equal(cut(same, same, 0, 1, 2, 2), 'ne\ntwo\nth');
});

test('PRM-35 every f2 text exists in all three locales with the same placeholders; f2.js writes no files and logs nothing', () => {
  const keys = Object.keys(i18n.S).filter(k => k.startsWith('f2.'));
  assert.equal(keys.length, 18);
  const holes = s => (s.match(/\{\d+\}/g) || []).sort().join('');
  for (const k of keys) { const row = i18n.S[k]; for (let i = 0; i < 3; i++) assert.ok(row[i] && row[i].length, `${k}[${i}]`); assert.equal(holes(row[1]), holes(row[0]), `${k}: zh placeholders`); assert.equal(holes(row[2]), holes(row[0]), `${k}: ja placeholders`); }
  assert.match(i18n.t('zh-CN', 'f2.copied', 12), /已复制 12 个字符/); assert.match(i18n.t('ja-JP', 'f2.noOptCode'), /最適化は不要/); assert.equal(i18n.t('en-US', 'f2.selectionTooLarge', 5, 7), 'The selection has 5 characters; the limit is 7. Select less.');
  const src = fs.readFileSync(path.join(root, 'src', 'f2.js'), 'utf8');
  assert.doesNotMatch(src, /console\.|fs\.(?:write|append)File|appendLine/); assert.doesNotMatch(src, /globalState|workspaceState|\.update\(/, 'F2 keeps no state');
  // nothing invisible in the sources of this work item
  const invisible = new RegExp('[\\u200b-\\u200f\\u2060-\\u2064\\u202a-\\u202e\\ufeff]');
  for (const f of [path.join(root, 'src', 'f2.js'), path.join(root, 'src', 'core', 'i18n.js'), __filename]) assert.doesNotMatch(fs.readFileSync(f, 'utf8'), invisible, path.basename(f));
});
