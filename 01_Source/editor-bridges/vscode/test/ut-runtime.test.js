'use strict';
// Settings migration, atomic persistence, UI model, i18n, fault handling, performance and static contract checks.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {core, makeFs, FakeTimers} = require('./helpers');
const S = core('settings');
const i18n = core('i18n');
const ui = core('uimodel');
const L = core('languages');
const {CoreSupervisor, InputSession} = core('scheduler');

const root = path.join(__dirname, '..');
const pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));

test('UT-043 / IT-026 / ST-023 F1 never steals the key unless enabled, bound and not excluded', () => {
  const f1 = pkg.contributes.keybindings.filter(k => k.command === 'programmingIme.f1Edit');
  assert.deepEqual(f1.map(k => k.key).sort(), ['f1', 'shift+f1']);
  for (const k of f1) assert.match(k.when, /editorTextFocus && programmingIme\.f1Key\./, 'only with editor focus and an explicit context key');
  const props = pkg.contributes.configuration.properties;
  assert.equal(props['programmingIme.enabled'].default, false, 'off by default: F1 keeps opening the Command Palette');
  assert.deepEqual(props['programmingIme.f1Binding'].enum, ['f1', 'shift+f1', 'none']);
  assert.equal(props['programmingIme.aiEnabled'].default, false); assert.equal(props['programmingIme.realtimeAI'].default, false);
  assert.deepEqual(props['programmingIme.f1ExcludedLanguages'].default, []);
  const ext = fs.readFileSync(path.join(root, 'src', 'extension.js'), 'utf8');
  assert.match(ext, /f1Key\.f1', on && binding === 'f1' && !excluded/);
  assert.match(ext, /f1Key\.shiftF1', on && binding === 'shift\+f1' && !excluded/);
});

test('UT-056 / UT-044 UI locale is independent of the programming profile and of the OS language', () => {
  assert.equal(i18n.resolveLocale('zh-CN', 'ja'), 'zh-CN', 'explicit UI locale wins over the VS Code/OS language');
  assert.equal(i18n.resolveLocale('auto', 'ja-JP'), 'ja-JP'); assert.equal(i18n.resolveLocale('auto', 'zh-cn'), 'zh-CN'); assert.equal(i18n.resolveLocale('auto', 'fr'), 'en-US');
  const s = new InputSession({language: 'rust'});
  s.setLanguage('c');
  assert.equal(s.language, 'c');
  assert.match(i18n.t('zh-CN', 'f1.tab.full'), /整文件/); assert.match(i18n.t('ja-JP', 'f1.tab.full'), /ファイル全体/); assert.match(i18n.t('en-US', 'f1.tab.full'), /Whole file/);
  const code = L.predict({profile: 'c', prefix: 'main'})[0].text;
  for (const loc of i18n.LOCALES) { i18n.t(loc, 'status.off'); assert.equal(L.predict({profile: 'c', prefix: 'main'})[0].text, code, 'code is never translated'); }
  for (const key of Object.keys(i18n.S)) for (let i = 0; i < 3; i++) assert.ok(i18n.S[key][i] && i18n.S[key][i].length, `${key} missing in locale ${i}`);
  for (const key of Object.keys(i18n.ERR)) for (let i = 0; i < 3; i++) assert.ok(i18n.ERR[key][i], `error ${key} missing in locale ${i}`);
  assert.equal(i18n.t('en-US', 'f1.hunk', 2, 7, 1, 3), 'Block 2 (old lines 7, 1 removed, 3 added)');
  assert.equal(i18n.describeError('zh-CN', 'NOPE', 'raw message'), 'raw message');
  assert.equal(i18n.t('en-US', 'does.not.exist'), 'does.not.exist');
  for (const nls of ['package.nls.json', 'package.nls.zh-cn.json', 'package.nls.ja.json']) {
    const t = JSON.parse(fs.readFileSync(path.join(root, nls), 'utf8'));
    for (const c of pkg.contributes.commands) assert.ok(t[c.title.replace(/%/g, '')], `${nls}: ${c.title}`);
  }
});

test('UT-057 every candidate row is described by text (source, validation, position, action), not colour', () => {
  const cands = [
    L.makeCandidate({id: 'a', kind: 'syntax', source: 'rule', validation: 'valid', label: 'main function', text: 'fn main() {}'}),
    L.makeCandidate({id: 'b', kind: 'ai_inline', source: 'ai', validation: 'unchecked', label: 'AI idea', text: 'x'}),
    L.makeCandidate({id: 'c', kind: 'explain', source: 'rule', validation: 'context_missing', label: 'jmp foo', text: null})
  ];
  for (const loc of i18n.LOCALES) {
    cands.forEach((c, i) => {
      const row = ui.candidateRow(loc, c, i + 1, cands.length);
      assert.ok(row.ariaLabel.startsWith(`${i + 1} / 3:`)); assert.ok(row.ariaLabel.includes(c.label));
      assert.ok(row.description.includes(i18n.t(loc, 'src.' + c.source))); assert.ok(row.description.includes(i18n.t(loc, 'val.' + c.validation)));
    });
  }
  assert.equal(ui.candidateRow('en-US', cands[2], 3, 3).insertable, false); assert.match(ui.candidateRow('en-US', cands[2], 3, 3).ariaLabel, /not insertable/);
  const merged = {...cands[0], sources: ['rule', 'ai']};
  assert.match(ui.candidateRow('en-US', merged, 1, 1).description, /\+1/);
});

test('UT-055 overlay placement stays on the caret monitor and hides when it cannot be located', () => {
  const monitors = [{x: -1920, y: 0, w: 1920, h: 1080}, {x: 0, y: 0, w: 2560, h: 1440}];
  const size = {w: 320, h: 200};
  const edge = ui.placeOverlay({caret: {x: -30, y: 500}, size, monitors, dpi: 100});
  assert.ok(edge.x >= -1920 && edge.x + edge.w <= 0, 'clamped inside the left monitor');
  const bottom = ui.placeOverlay({caret: {x: 100, y: 1430}, size, monitors, dpi: 250});
  assert.ok(bottom.y + bottom.h <= 1440 && bottom.y < 1430, 'flips above the caret at the bottom edge');
  assert.equal(bottom.w, 800); assert.equal(bottom.h, 500, '250% DPI scales the window');
  assert.equal(ui.placeOverlay({caret: {x: 99999, y: 5}, size, monitors}), null, 'caret on no monitor: hide, do not steal focus');
  assert.equal(ui.placeOverlay({caret: {x: 10, y: 10}, size: {w: 4000, h: 10}, monitors}), null, 'larger than the monitor');
});

test('UT-077 / IT-048 environment check: Windows 11 x64, DPI 100-250%, others are explained', () => {
  assert.deepEqual(ui.checkEnvironment({platform: 'win32', arch: 'x64', build: 22631, dpi: 150}), {supported: true, reasons: []});
  for (const dpi of [100, 150, 200, 250]) assert.equal(ui.checkEnvironment({platform: 'win32', arch: 'x64', build: 26100, dpi}).supported, true);
  assert.equal(ui.checkEnvironment({platform: 'win32', arch: 'x64', build: 19045}).supported, false);
  assert.equal(ui.checkEnvironment({platform: 'win32', arch: 'arm64', build: 26100}).reasons[0], 'x64 is required');
  assert.ok(ui.checkEnvironment({platform: 'linux', arch: 'x64', build: 0}).reasons.length >= 2);
  assert.equal(ui.checkEnvironment({platform: 'win32', arch: 'x64', build: 26100, dpi: 300}).supported, false);
  assert.equal(ui.checkEnvironment({platform: 'win32', arch: 'x64', build: 26100}).supported, true, 'unknown dpi is skipped, not guessed');
});

test('UT-059 / UT-073 / IT-039 / ST-033 migration is atomic: a failure keeps the previous settings and rules', () => {
  const v0 = {provider: 'openai-compatible', endpoint: 'https://api.example.com/v1/chat/completions', model: 'm1', effort: 'high', effortCapability: 'supported', userRules: [{id: 'mine'}], theme: 'x'};
  const m = S.migrateV0(v0, Date.parse('2026-10-30T00:00:00Z'));
  assert.equal(m.schema_version, 1); assert.equal(m.scenarios.f1Full.provider, 'openai-compatible'); assert.equal(m.scenarios.realtime.model, 'm1');
  assert.deepEqual(m.scenarios.f1Insert.effortCapability, {state: 'supported', supported: ['low', 'medium', 'high'], checkedAt: '2026-10-30T00:00:00.000Z'});
  assert.equal(m.providers['openai-compatible'].endpoint, 'https://api.example.com/v1/chat/completions');
  const insecure = S.migrateV0({endpoint: 'http://x', effort: 'turbo'});
  assert.deepEqual(insecure.conflicts.map(c => c.key).sort(), ['effort', 'endpoint']);
  assert.throws(() => S.migrate({schema_version: 9}), e => e.code === 'SETTINGS_INVALID', 'downgrade is refused with a clear error');
  let stored = {...v0}, failWrite = true;
  const store = {read: () => stored, write: o => { if (failWrite) throw Object.assign(new Error('ENOSPC'), {code: 'ENOSPC'}); stored = o; }};
  assert.throws(() => S.migrateStore(store), e => e.code === 'SETTINGS_INVALID' && /previous settings are intact/.test(e.message));
  assert.deepEqual(stored, v0, 'old configuration still complete after the failed migration');
  failWrite = false;
  const done = S.migrateStore(store);
  assert.equal(stored.schema_version, 1); assert.deepEqual(stored.userRules, [{id: 'mine'}]); assert.equal(stored.theme, 'x');
  assert.ok(!('model' in stored), 'legacy flat keys are retired'); assert.ok(done.backup.model === 'm1', 'backup retains the old values');
  assert.equal(S.migrate(stored).schema_version, 1);
});

test('UT-073 atomic file writes: ENOSPC or a crash before rename leave the last valid file; .bak recovers corruption', () => {
  const good = JSON.stringify({schema_version: 1, x: 'old'});
  for (const failOn of ['write', 'rename']) {
    const fsx = makeFs({'/s.json': good}, failOn);
    assert.throws(() => S.writeFileAtomic('/s.json', JSON.stringify({schema_version: 1, x: 'new'}), fsx), /failed/);
    assert.equal(fsx.files.get('/s.json'), good, `${failOn}: old file untouched`);
    assert.ok(![...fsx.files.keys()].some(k => k.includes('.tmp-')), 'temp file removed');
  }
  const fsx = makeFs({'/s.json': good});
  S.writeFileAtomic('/s.json', JSON.stringify({schema_version: 1, x: 'new'}), fsx);
  assert.equal(JSON.parse(fsx.files.get('/s.json')).x, 'new'); assert.equal(fsx.files.get('/s.json.bak'), good);
  fsx.files.set('/s.json', '{"schema_ver');          // power loss mid-write
  const rec = S.readJsonRecovering('/s.json', v => { if (v.schema_version !== 1) throw new Error('bad'); }, fsx);
  assert.equal(rec.recovered, true); assert.equal(rec.value.x, 'old');
  assert.equal(S.readJsonRecovering('/missing.json', () => {}, fsx).value, null);
  const exported = S.exportSettings(S.effective({}));
  assert.equal(S.importSettings(exported).settings.scenarios.realtime.timeoutMs, 8000);
  assert.throws(() => S.importSettings('{bad'), e => e.code === 'SETTINGS_INVALID');
  assert.throws(() => S.importSettings(JSON.stringify({schema_version: 1, providers: {X: {api: 'openai-chat', endpoint: 'http://x'}}})), e => e.code === 'SETTINGS_INVALID');
});

test('UT-058 / ST-033 user rule packs: bad packs are rejected and skipped, valid ones keep working', () => {
  const good = JSON.stringify({schema_version: 1, profile_id: 'python', version: '1.0.0', triggers: [{id: 'my.log', pattern: 'logd', candidates: [{label: 'debug log', text: 'logging.debug("${msg}")'}]}]});
  const evil = JSON.stringify({schema_version: 1, profile_id: 'python', version: '1.0.0', triggers: [{id: 'my.evil', pattern: 'evil', candidates: [{label: 'x', text: 'x', shell: 'calc.exe'}]}]});
  const huge = JSON.stringify({schema_version: 1, profile_id: 'python', version: '1.0.0', keywords: [], triggers: [], pad: 'x'.repeat(300000)});
  const built = S.buildRegistryWithUserRules([{name: 'good.json', text: good}, {name: 'evil.json', text: evil}, {name: 'huge.json', text: huge}, {name: 'junk.json', text: '{{'}]);
  assert.deepEqual(built.accepted, ['good.json']); assert.deepEqual(built.rejected.map(r => r.name).sort(), ['evil.json', 'huge.json', 'junk.json']);
  const out = L.predict({profile: 'python', prefix: 'logd', before: ''}, built.registry);
  assert.equal(out[0].id, 'my.log#0'); assert.equal(out[0].source, 'user_rule');
  assert.deepEqual(L.predict({profile: 'python', prefix: 'evil', before: ''}, built.registry), []);
  assert.equal(L.predict({profile: 'python', prefix: 'def', before: ''}, built.registry)[0].source, 'rule', 'built-in rules still load');
});

test('UT-064 / UT-067 / IT-004 / ST-037 a failing core means Direct within the 2 s target; JS candidates keep flowing', () => {
  let t = 0;
  const sup = new CoreSupervisor({now: () => t});
  assert.equal(sup.isDirect, false);
  t = 1234; sup.reportFailure();
  assert.equal(sup.isDirect, true); assert.ok(sup.timeToDirect(1234) <= 2000);
  t = 1300; sup.reportFailure(); assert.equal(sup.directSince, 1234, 'repeated failures do not move the switch point');
  assert.equal(sup.failures, 2);
  for (let i = 0; i < 100; i++) assert.ok(L.predict({profile: 'rust', prefix: 'fn', before: ''}).length >= 1, 'typing continues without the core');
  sup.reportRecovered(); assert.equal(sup.isDirect, false);
  const cp = fs.readFileSync(path.join(root, 'src', 'coreprocess.js'), 'utf8');
  assert.match(cp, /timeoutMs = 100/, 'a slow core is abandoned after 100 ms');
  assert.match(fs.readFileSync(path.join(root, 'src', 'extension.js'), 'utf8'), /reportFailure\(\); \/\* Direct fallback/);
});

test('UT-066 / IT-044 / ST-038 offline: every network call fails, local candidates and Direct keep working', async () => {
  const ai = core('ai');
  const client = new ai.AiClient({fetchImpl: async () => { throw Object.assign(new Error('fetch failed'), {cause: {code: 'ECONNREFUSED'}}); }});
  const langs = {rust: 'fn', python: 'de', typescript: 'in', powershell: 'Ge', c: 'ma', assembly: 'mo', 'machine-code': 'no'};
  for (let round = 0; round < 50; round++) {
    for (const [profile, prefix] of Object.entries(langs)) assert.ok(L.predict({profile, prefix, before: ''}).length >= 1, `${profile} round ${round}`);
    try { await client.infer({scenario: 'realtime', family: 'openai-chat', endpoint: 'https://x.example/v1', key: 'k', model: 'm', instructions: 'i', input: 'x', timeoutMs: 100}); assert.fail('must fail'); }
    catch (e) { assert.equal(e.code, 'AI_NETWORK'); }
  }
  assert.equal(client.inFlight, 0);
});

test('UT-068 / IT-022 / ST-039 local candidate computation: P95 of 1000 events <= 20 ms (even with 2000 lines of context)', () => {
  const profiles = ['rust', 'python', 'typescript', 'powershell', 'c'], prefixes = ['f', 'fn', 'de', 'Ge', 'ma', 'str', 'imp'];
  const before = 'let value = compute(items);\n'.repeat(2000);
  const times = [];
  for (let i = 0; i < 1000; i++) {
    const t0 = process.hrtime.bigint();
    L.predict({profile: profiles[i % 5], prefix: prefixes[i % 7], before});
    times.push(Number(process.hrtime.bigint() - t0) / 1e6);
  }
  times.sort((a, b) => a - b);
  const p95 = times[949];
  console.log(`# UT-068 P95=${p95.toFixed(3)}ms max=${times[999].toFixed(3)}ms node=${process.version} platform=${process.platform}/${process.arch}`);
  assert.ok(p95 <= 20, `P95 ${p95} ms`);
  const t0 = process.hrtime.bigint(); L.predict({profile: 'rust', prefix: 'fn', before}); // frozen provider: local result needs no network at all
  assert.ok(Number(process.hrtime.bigint() - t0) / 1e6 < 50);
});

test('UT-074 static: nothing in the source writes a key to settings, files or logs', () => {
  const ext = fs.readFileSync(path.join(root, 'src', 'extension.js'), 'utf8');
  assert.match(ext, /secrets\.store\('pime-key:' \+ id, key\)/);
  assert.doesNotMatch(ext, /update\('[^']*[kK]ey[^']*'/, 'no configuration key holds an API key');
  for (const f of fs.readdirSync(path.join(root, 'src')).filter(n => n.endsWith('.js'))) {
    const src = fs.readFileSync(path.join(root, 'src', f), 'utf8');
    assert.doesNotMatch(src, /console\.log|appendLine\([^)]*key\b/, `${f}: no console/log of keys`);
    assert.doesNotMatch(src, /fs\.(?:write|append)File/, `${f}: no direct file writes of runtime data`);
  }
});

test('FR-029 / NFR-302 the package exposes provider/model/effort/budget as separate, key-free settings', () => {
  const p = pkg.contributes.configuration.properties;
  for (const k of ['providers', 'scenarios', 'budget', 'debounceMs', 'aiEnabled', 'realtimeAI']) assert.ok(p['programmingIme.' + k], k);
  assert.equal(p['programmingIme.debounceMs'].minimum, 150); assert.equal(p['programmingIme.debounceMs'].maximum, 1000); assert.equal(p['programmingIme.debounceMs'].default, 350);
  assert.ok(!Object.keys(p).some(k => /apiKey|secret|token/i.test(k)));
  assert.equal(S.DEFAULT_BUDGET.maxRequests, 200);
});
