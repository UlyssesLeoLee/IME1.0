'use strict';
// Step 1.3 (P1B) differential tests: the real pime-core over JSON Lines must give the same answers as the JavaScript
// modules for settings (validation, migration, export/import, user rule packs), diagnostics, interface texts and the UI
// models, and the engine adapter's presentation methods must behave like the JS engine's. These tests need a built core:
// set PIME_CORE_EXE; without it they are SKIPPED (not passed). The bulk of the parity is in tests/rust/parity_cfg.rs
// (fixtures generated from the JS modules); this file closes the loop through the real process and the adapter.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const {createEngine} = require('../../editor-bridges/vscode/src/engine');
const {core} = require('./helpers');
const S = core('settings'), i18n = core('i18n'), ui = core('uimodel'), ai = core('ai'), {Diagnostics, sanitize} = core('diagnostics');

const EXE = process.env.PIME_CORE_EXE;
const skip = EXE && fs.existsSync(EXE) ? false : 'set PIME_CORE_EXE to a built pime-core to run the Rust engine tests';
const store = () => { const m = new Map(); return {get: k => m.get(k), update: (k, v) => { m.set(k, v); return Promise.resolve(); }}; };

async function rustEngine(values = {}) {
  const rust = createEngine('rust', {globalState: store()});
  assert.equal(await rust.start({binary: EXE, locale: 'en-US', values}), true, rust.reason);
  return rust;
}
const attempt = fn => { try { return {value: fn()}; } catch (e) { return {error: {code: e.code, message: e.message}}; } };
const attemptRpc = async (rust, body) => { try { return {value: await rust.rpc(body)}; } catch (e) { return {error: {code: e.code, message: e.message}}; } };

test('CFG engine: every interface text, in every locale, is the JS text (FR-044, UT-044/056)', {skip}, async () => {
  const rust = await rustEngine();
  try {
    for (const locale of [...i18n.LOCALES, 'fr-FR']) {
      const raw = (await rust.rpc({op: 'ui.bundle', locale, raw: true})).bundle;
      const idx = locale === 'zh-CN' ? 1 : locale === 'ja-JP' ? 2 : 0;
      assert.deepEqual(Object.keys(raw), Object.keys(i18n.S), `${locale}: the key set and order`);
      for (const key of Object.keys(i18n.S)) assert.equal(raw[key], i18n.S[key][idx] || i18n.S[key][0], `${locale} ${key}`);
      const errors = (await rust.rpc({op: 'ui.errors', locale})).errors;
      for (const code of Object.keys(i18n.ERR)) assert.equal(errors[code], i18n.describeError(locale, code), `${locale} ${code}`);
      // filled texts, with arguments of several types
      for (const [key, args] of [['f1.hunk', [2, 7, 1, 3]], ['consent.sensitive', ['aws, github']], ['f1.usage', [12, 3456, ' (estimated)']], ['cand.pick', ['rust', 'fnm']], ['f1.target', [3, 4, null]], ['x {0} {1}', ['{1}', [1, 2]]]]) {
        const got = (await rust.rpc({op: 'ui.text', locale, key, args})).text;
        assert.equal(got, i18n.t(locale, key, ...args), `${locale} ${key}`);
      }
      assert.deepEqual((await rust.rpc({op: 'ui.bundle', locale, keys: ['f1.hunk', 'status.off', '10', '2', 'nope']})).bundle, i18n.bundle(locale, ['f1.hunk', 'status.off', '10', '2', 'nope']));
    }
    for (const [setting, language] of [['auto', 'ja-JP'], ['auto', 'zh-cn'], ['zh-CN', 'ja'], ['', 'fr'], ['en-US', 'zh'], [null, 'JA'], ['auto', undefined]]) {
      const got = (await rust.rpc({op: 'ui.resolve_locale', setting, ...(language === undefined ? {} : {language})})).locale;
      assert.equal(got, i18n.resolveLocale(setting, language), `${setting}/${language}`);
    }
    for (const [code, message] of [['STALE_DOCUMENT'], ['NOPE', 'raw message'], ['NOPE'], ['AI_DISABLED', 'ignored']]) {
      assert.equal((await rust.rpc({op: 'ui.error_text', locale: 'zh-CN', code, ...(message === undefined ? {} : {message})})).text, i18n.describeError('zh-CN', code, message), code);
    }
  } finally { rust.stop(); }
});

test('CFG engine: settings validation accepts every effort level and reports the same problems as JS (UT-029/031)', {skip}, async () => {
  const rust = await rustEngine();
  try {
    for (const effort of [...ai.EFFORT_LEVELS, 'MAX', 'turbo', '']) {
      const settings = S.effective({scenarios: {realtime: {effort}, f1Full: {effort}}});
      assert.deepEqual((await rust.rpc({op: 'settings.validate', settings})).issues, S.validateSettings(settings), effort);
    }
    const messy = S.effective({providers: {p: {api: 'nope', endpoint: 'http://x', apiVersion: 'latest', maxTokensParam: 'x'}}, scenarios: {realtime: {provider: 'ghost', effort: 'turbo', maxOutputTokens: 0, timeoutMs: 1}}});
    assert.deepEqual((await rust.rpc({op: 'settings.validate', settings: messy})).issues, S.validateSettings(messy));
    // the core applies the same effective settings
    const values = {providers: {mockA: {api: 'openai-chat', endpoint: 'https://a.example/v1'}}, scenarios: {f1Full: {provider: 'mockA', model: 'm', effort: 'max'}}, budget: {maxRequests: 3}};
    assert.deepEqual((await rust.rpc({op: 'settings.apply', values})).settings, S.effective(values));
    assert.equal(JSON.stringify((await rust.rpc({op: 'settings.resolve', scenario: 'f1Full'})).scenario), JSON.stringify(S.resolveScenario(S.effective(values), 'f1Full')));
  } finally { rust.stop(); }
});

test('CFG engine: migration, export/import and rule packs give the JS answers (UT-058/059/073, FR-045/046)', {skip}, async () => {
  const rust = await rustEngine();
  try {
    const docs = [
      {provider: 'openai-compatible', endpoint: 'https://api.example.com/v1/chat/completions', model: 'm1', effort: 'high', effortCapability: 'unsupported', userRules: [{id: 'mine'}], theme: 'x'},
      {endpoint: 'http://x', effort: 'turbo'}, {effort: 'max'}, {}, {schema_version: 1, providers: {a: {api: 'openai-chat', endpoint: 'https://a.example/'}}, budget: {maxRequests: 2}, extra: 1},
      {schema_version: 2}, {schema_version: '1'}, {schema_version: null}, {schema_version: [1, [2, null]]}, {schema_version: 1e21}
    ];
    for (const doc of docs) {
      const js = attempt(() => S.migrate(doc, 0));
      const got = await attemptRpc(rust, {op: 'settings.migrate', doc});
      if (js.error) assert.deepEqual(got.error, js.error, JSON.stringify(doc));
      else {
        assert.deepEqual(got.value.result, JSON.parse(JSON.stringify(js.value)), JSON.stringify(doc));
        let written;
        S.migrateStore({read: () => doc, write: o => { written = JSON.parse(JSON.stringify(o)); }}, 0);
        assert.deepEqual(got.value.document, written, `${JSON.stringify(doc)}: the document a store keeps`);
      }
    }
    // the timestamp of a supported capability comes from the core's clock
    const before = Date.now();
    const m = (await rust.rpc({op: 'settings.migrate', doc: {effortCapability: 'supported'}})).result;
    const stamp = Date.parse(m.scenarios.realtime.effortCapability.checkedAt);
    assert.ok(stamp >= before - 1000 && stamp <= Date.now() + 1000, 'checkedAt is now');
    assert.deepEqual(m.scenarios.realtime.effortCapability.supported, ['low', 'medium', 'high']);
    // export and import
    const settings = S.effective({providers: {mockA: {api: 'openai-chat', endpoint: 'https://a.example/v1'}}, scenarios: {f1Full: {provider: 'mockA', model: 'モデル', effort: 'max'}}, budget: {pricePer1kTokens: 1e-7}});
    const text = (await rust.rpc({op: 'settings.export', settings})).text;
    assert.equal(text, S.exportSettings(settings));
    assert.deepEqual((await rust.rpc({op: 'settings.import', text})).settings, S.importSettings(text).settings);
    const leaky = S.effective({scenarios: {realtime: {model: 'AKIAIOSFODNN7EXAMPLE'}}});
    assert.deepEqual((await attemptRpc(rust, {op: 'settings.export', settings: leaky})).error, attempt(() => S.exportSettings(leaky)).error);
    for (const bad of ['{bad', '{"schema_version":3}', '{"schema_version":1,"scenarios":{"realtime":{"effort":"turbo","maxOutputTokens":0}}}']) {
      assert.deepEqual((await attemptRpc(rust, {op: 'settings.import', text: bad})).error, attempt(() => S.importSettings(bad)).error, bad);
    }
    // user rule packs
    const good = {schema_version: 1, profile_id: 'python', version: '1.0.0', triggers: [{id: 'my.log', pattern: 'logd', candidates: [{label: 'debug log', text: 'logging.debug("${msg}")'}]}]};
    const files = [
      {name: 'good.json', text: JSON.stringify(good)}, {name: 'again.json', text: JSON.stringify(good)}, {name: 'evil.json', text: JSON.stringify({...good, triggers: [{...good.triggers[0], id: 'e', shell: 'calc.exe'}]})},
      {name: 'bad.json', text: '{'}, {name: 'cobol.json', text: JSON.stringify({...good, profile_id: 'cobol'})}, {name: 'huge.json', text: JSON.stringify({...good, pad: 'x'.repeat(300000)})}
    ];
    const js = S.buildRegistryWithUserRules(files);
    const got = await rust.rpc({op: 'settings.rules_check', files});
    assert.deepEqual(got.accepted, js.accepted);
    assert.deepEqual(got.rejected, js.rejected.map(r => ({name: r.name, code: r.code, message: r.message})));
  } finally { rust.stop(); }
});

test('CFG engine: diagnostics keep the same whitelisted, filtered record and the same summary (UT-063/071)', {skip}, async () => {
  const rust = await rustEngine();
  try {
    const events = [
      {timestamp: 1000, component: 'ai', error_code: 'AI_TIMEOUT', elapsed_ms: 12.6, request_type: 'f1Full', provider_id: 'openai:api.openai.com', counters: {requests: 2, 'bad key': 1, s: 'x'}},
      {timestamp: 1001, component: 'edit', error_code: 'EDIT_UNKNOWN'}, {timestamp: 1002, component: 'C:\\Users\\me\\x.rs', error_code: 'a b', elapsed_ms: -3.5},
      {timestamp: 1003, input_text: 'fn secret() {}', api_key: 'sk-live-123', prompt: 'p', document_uri: 'file:///x', component: 'ai'}, {timestamp: 1004, error_code: 404}, {timestamp: 1005, counters: [1, 2]}
    ];
    const js = new Diagnostics({now: () => 1, sessionSeed: 'x'});
    for (const event of events) {
      const clean = (await rust.rpc({op: 'diag.record', event})).event;
      const want = js.record(event);
      assert.deepEqual({...clean, anonymized_session: 0}, {...want, anonymized_session: 0}, JSON.stringify(event));
      assert.equal(clean.anonymized_session.length, 12);
    }
    assert.deepEqual((await rust.rpc({op: 'diag.summary'})).summary, js.summary());
    const lines = (await rust.rpc({op: 'diag.text'})).text.split('\n').map(l => JSON.parse(l));
    assert.deepEqual(lines.map(l => ({...l, anonymized_session: 0})), js.events.map(l => ({...l, anonymized_session: 0})));
    const all = JSON.stringify(lines);
    for (const leak of ['secret', 'sk-live', 'file:///', 'Users']) assert.ok(!all.includes(leak), leak);
    // a timestamp that is not a number is replaced by the clock here (the JS module would record it as given)
    const odd = (await rust.rpc({op: 'diag.record', event: {timestamp: '/home/me/secret.rs'}})).event;
    assert.equal(typeof odd.timestamp, 'number');
    assert.deepEqual(sanitize({timestamp: 5}, 's'), {timestamp: 5, anonymized_session: 's'});
  } finally { rust.stop(); }
});

test('CFG engine: candidate rows, overlay placement, environment check, status bar and menu texts match JS (UT-055/057/077)', {skip}, async () => {
  const rust = await rustEngine();
  try {
    const cands = [
      {label: 'main function', source: 'rule', validation: 'valid', explain: 'Entry point', assumptions: ['a', '', 'b'], text: 'fn main() {}'}, {label: 'AI idea', source: 'ai', validation: 'unchecked', text: 'x'},
      {label: 'jmp foo', source: 'rule', validation: 'context_missing', text: null}, {label: 'merged', source: 'rule', sources: ['rule', 'ai'], validation: 'verified', text: ''}, {label: 'user', source: 'user_rule', sources: ['user_rule', 'rule', 'ai', 'context'], validation: 'invalid', text: 'y'}
    ];
    for (const locale of [...i18n.LOCALES, 'fr-FR']) {
      const rows = (await rust.rpc({op: 'ui.candidate_rows', locale, candidates: cands.map(c => ({...c, text: c.text == null ? null : ''}))})).rows;
      assert.deepEqual(rows, cands.map((c, i) => ui.candidateRow(locale, c, i + 1, cands.length)), locale);
    }
    const monitors = [{x: -1920, y: 0, w: 1920, h: 1080}, {x: 0, y: 0, w: 2560, h: 1440}];
    for (const [caret, size, dpi] of [[{x: -30, y: 500}, {w: 320, h: 200}, 100], [{x: 100, y: 1430}, {w: 320, h: 200}, 250], [{x: 99999, y: 5}, {w: 320, h: 200}], [{x: 10, y: 10}, {w: 2, h: 2}, 125], [{x: 2500, y: 10.5}, {w: 321, h: 201}, 150], [{x: 5, y: 5}, {w: 4000, h: 10}]]) {
      const got = (await rust.rpc({op: 'ui.place_overlay', caret, size, monitors, ...(dpi === undefined ? {} : {dpi})})).placement;
      assert.deepEqual(got, ui.placeOverlay({caret, size, monitors, ...(dpi === undefined ? {} : {dpi})}) ?? null);
    }
    for (const env of [{platform: 'win32', arch: 'x64', build: 22631, dpi: 150}, {platform: 'win32', arch: 'x64', build: 19045}, {platform: 'linux', arch: 'arm64', build: 0, dpi: 300}, {platform: 'win32', arch: 'x64', build: 26100, dpi: null}]) {
      const got = await rust.rpc({op: 'ui.check_env', ...env});
      assert.deepEqual({supported: got.supported, reasons: got.reasons}, ui.checkEnvironment(env), JSON.stringify(env));
    }
    const state = {coreDown: false, reason: '', enabled: true, info: {language: 'c', locked: true, uncertain: true, suggestion: 'cpp'}, sessionLanguage: 'typescript', paused: true, aiEnabled: true, scenario: {model: 'gpt-x', effort: 'max'}, budget: {requests: 12, tokens: 3456, estimated: true}};
    for (const locale of i18n.LOCALES) for (const over of [{}, {coreDown: true, reason: 'the core process ended'}, {enabled: false, info: null, scenario: null}, {paused: false, aiEnabled: false, info: {language: 'rust'}}]) {
      const s = {...state, ...over};
      const got = (await rust.rpc({op: 'ui.status_model', locale, core_down: s.coreDown, reason: s.reason, enabled: s.enabled, info: s.info, session_language: s.sessionLanguage, paused: s.paused, ai_enabled: s.aiEnabled, scenario: s.scenario, budget: s.budget})).model;
      assert.deepEqual(got, ui.statusModel(locale, s), `${locale} ${JSON.stringify(over)}`);
    }
    for (const locale of i18n.LOCALES) for (const paused of [false, true]) for (const aiEnabled of [false, true]) {
      assert.deepEqual((await rust.rpc({op: 'ui.menu_model', locale, paused, ai_enabled: aiEnabled})).model, ui.menuModel(locale, {paused, aiEnabled}));
    }
  } finally { rust.stop(); }
});

test('CFG engine adapter: the presentation methods answer like the JS engine, and still answer when the core is down', {skip}, async () => {
  const js = createEngine('js', {globalState: store()});
  const rust = await rustEngine();
  const cands = [{label: 'a', source: 'rule', validation: 'valid', text: 'x', explain: 'e'}, {label: 'b', source: 'ai', sources: ['ai', 'rule'], validation: 'unchecked', text: null}, {label: 'c', validation: 'verified', assumptions: ['z']}];
  const state = {paused: true, aiEnabled: false};
  const env = {platform: 'win32', arch: 'x64', build: 26100};
  try {
    for (const locale of i18n.LOCALES) {
      assert.deepEqual(await rust.candidateRows(locale, cands), await js.candidateRows(locale, cands), locale);
      assert.deepEqual(await rust.menuModel(locale, state), await js.menuModel(locale, state), locale);
    }
    assert.deepEqual(await rust.checkEnvironment(env), await js.checkEnvironment(env));
    // the real core was asked (a wrong answer from it would not be hidden by the fallback)
    assert.equal((await rust.rpc({op: 'ui.menu_model', paused: true, ai_enabled: false})).model.items[1].label, i18n.t('en-US', 'menu.resume'));
    // the core goes away: the menu and the diagnostics still work, with the same texts
    rust.noteFailure('test: the core stopped');
    assert.equal(rust.available(), false);
    assert.deepEqual(await rust.candidateRows('en-US', cands), await js.candidateRows('en-US', cands));
    assert.deepEqual(await rust.menuModel('ja-JP', state), await js.menuModel('ja-JP', state));
    assert.deepEqual(await rust.checkEnvironment(env), await js.checkEnvironment(env));
  } finally { rust.stop(); }
});
