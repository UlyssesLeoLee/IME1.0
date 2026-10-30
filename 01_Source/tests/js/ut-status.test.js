'use strict';
// SCR-01 status bar items and the status menu are plain-text models (core/uimodel.js), so that the JS engine and the Rust core
// render the same text. The functions were extracted from extension.js without a change of behaviour: this test keeps the
// original inline code as the reference and compares the model with it over every combination of the state.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const {core} = require('./helpers');
const {t} = core('i18n');
const ui = core('uimodel');

// the code of extension.js updateStatus() / menu() before the extraction, with `on`, `rt.session.paused` etc. as parameters
function legacyStatus(L, {coreDown, reason, enabled: on, info, sessionLanguage, paused, aiEnabled, scenario: sc, budget: b}) {
  const name = coreDown ? L('engine.status') : !on ? L('status.off') : (info ? info.language : sessionLanguage);
  const lang = {
    text: `${coreDown ? '$(error)' : '$(code)'} PIME ${name}${on && info && info.locked ? ' $(lock)' : ''}${on && info && info.uncertain && info.suggestion ? ' ?' : ''}${on && paused ? ` (${L('status.paused')})` : ''}`,
    tooltip: coreDown ? L('engine.unavailable', reason) : info && info.uncertain && info.suggestion ? L('lang.uncertain', info.suggestion, info.language) : L('lang.pick')
  };
  const aiState = !aiEnabled ? L('ai.off') : (paused ? L('ai.paused') : L('ai.on'));
  const ai = {text: `$(sparkle) ${aiState}`, tooltip: `${L('status.tooltip')}\n${sc ? `${sc.model || '-'} · effort ${sc.effort}` : ''}\n${L('f1.usage', b.requests, b.tokens, b.estimated ? ' (estimated)' : '')}`};
  return {lang, ai};
}
const legacyMenu = (L, {paused, aiEnabled}) => [
  L('menu.language'), paused ? L('menu.resume') : L('menu.pause'), aiEnabled ? 'AI: off' : 'AI: on', L('menu.usage'), L('menu.permissions'), L('menu.diagnostics'), L('menu.f1')
];

test('SCR-01 the status bar model renders exactly what the extension rendered inline, in every state and locale', () => {
  const infos = [null, {language: 'rust', locked: false, uncertain: false}, {language: 'c', locked: true, uncertain: true, suggestion: 'cpp'}, {language: 'python', uncertain: true}, {language: 'rust', locked: true, uncertain: true, suggestion: ''}];
  const scenarios = [null, {model: '', effort: 'auto'}, {model: 'gpt-x', effort: 'max'}];
  const budgets = [{requests: 0, tokens: 0, estimated: false}, {requests: 12, tokens: 3456, estimated: true}];
  let n = 0;
  for (const locale of ['zh-CN', 'en-US', 'ja-JP']) {
    const L = (key, ...args) => t(locale, key, ...args);
    for (const coreDown of [false, true]) for (const enabled of [true, false]) for (const info of infos) for (const paused of [false, true]) for (const aiEnabled of [false, true]) for (const scenario of scenarios) for (const budget of budgets) {
      const state = {coreDown, reason: 'the core process ended', enabled, info, sessionLanguage: 'typescript', paused, aiEnabled, scenario, budget};
      assert.deepEqual(ui.statusModel(locale, state), legacyStatus(L, state), `${locale} ${JSON.stringify(state)}`);
      n++;
    }
  }
  assert.equal(n, 2 * 2 * 5 * 2 * 2 * 3 * 2 * 3);
  // spot checks of the text itself
  const base = {coreDown: false, reason: '', enabled: true, info: {language: 'c', locked: true, uncertain: true, suggestion: 'cpp'}, sessionLanguage: 'rust', paused: true, aiEnabled: true, scenario: {model: 'm', effort: 'high'}, budget: {requests: 1, tokens: 2, estimated: false}};
  assert.equal(ui.statusModel('en-US', base).lang.text, '$(code) PIME c $(lock) ? (paused)');
  assert.equal(ui.statusModel('en-US', base).lang.tooltip, 'Detected cpp (uncertain) - keeping c');
  assert.equal(ui.statusModel('en-US', {...base, enabled: false, info: null}).lang.text, '$(code) PIME OFF');
  assert.equal(ui.statusModel('zh-CN', {...base, coreDown: true, reason: 'r'}).lang.text, '$(error) PIME 核心不可用 $(lock) ? (已暂停)');
  assert.equal(ui.statusModel('en-US', base).ai.text, '$(sparkle) AI paused');
});

test('SCR-01 the status menu lists the actions in order; the AI item names the action and is English in every locale', () => {
  for (const locale of ['zh-CN', 'en-US', 'ja-JP', 'fr-FR']) {
    const L = (key, ...args) => t(locale, key, ...args);
    for (const paused of [false, true]) for (const aiEnabled of [false, true]) {
      const m = ui.menuModel(locale, {paused, aiEnabled});
      assert.deepEqual(m.items.map(i => i.id), ['language', 'pause', 'ai', 'usage', 'permissions', 'diagnostics', 'f1']);
      assert.deepEqual(m.items.map(i => i.label), legacyMenu(L, {paused, aiEnabled}));
      assert.equal(m.title, L('menu.title'));
    }
  }
});
