'use strict';
const vscode = require('vscode');
const fs = require('node:fs');
const path = require('node:path');
const {rt, cfg, L, errorText, enabled, aiEnabled, workspaceId, refreshSettings, grantProviderId, init} = require('./runtime');
const lang = require('./languageState');
const f1 = require('./f1');
const {RealtimeScheduler} = require('./core/scheduler');
const languages = require('./core/languages');
const aiMod = require('./core/ai');
const secrets = require('./core/secrets');
const settingsMod = require('./core/settings');
const {fail, codeOf} = require('./core/errors');
const {sha256} = require('./core/context');
const {candidateRow, checkEnvironment} = require('./core/uimodel');
const {annotateInsertion} = require('./annotate');

const status = {lang: null, ai: null};
let lastScheduled = null;
let engineNotified = false;

// ------------------------------------------------------------------ status bar (SCR-01: language + AI state + usage entry)
function updateStatus() {
  if (!status.lang) return;
  const editor = vscode.window.activeTextEditor;
  const info = editor ? lang.infoFor(editor.document) : null;
  const on = enabled();
  const coreDown = cfg().get('enabled') === true && !rt.engine.available(); // Rust engine without a core: IME features are off
  const name = coreDown ? L('engine.status') : !on ? L('status.off') : (info ? info.language : rt.session.language);
  status.lang.text = `${coreDown ? '$(error)' : '$(code)'} PIME ${name}${on && info && info.locked ? ' $(lock)' : ''}${on && info && info.uncertain && info.suggestion ? ' ?' : ''}${on && rt.session.paused ? ` (${L('status.paused')})` : ''}`;
  status.lang.tooltip = coreDown ? L('engine.unavailable', rt.engine.reason) : info && info.uncertain && info.suggestion ? L('lang.uncertain', info.suggestion, info.language) : L('lang.pick');
  const sc = safeScenario('realtime');
  const b = rt.budget.snapshot();
  const aiState = !aiEnabled() ? L('ai.off') : (rt.session.paused ? L('ai.paused') : L('ai.on'));
  status.ai.text = `$(sparkle) ${aiState}`;
  status.ai.tooltip = `${L('status.tooltip')}\n${sc ? `${sc.model || '-'} · effort ${sc.effort}` : ''}\n${L('f1.usage', b.requests, b.tokens, b.estimated ? ' (estimated)' : '')}`;
  status.lang.show(); status.ai.show();
}
function safeScenario(name) { try { return settingsMod.resolveScenario(rt.settings, name); } catch { return null; } }
async function updateContextKeys() {
  const on = enabled();
  const editor = vscode.window.activeTextEditor;
  const excluded = editor ? (cfg().get('f1ExcludedLanguages') || []).includes(editor.document.languageId) : false;
  const binding = cfg().get('f1Binding');
  await vscode.commands.executeCommand('setContext', 'programmingIme.enabled', on);
  await vscode.commands.executeCommand('setContext', 'programmingIme.f1Key.f1', on && binding === 'f1' && !excluded);
  await vscode.commands.executeCommand('setContext', 'programmingIme.f1Key.shiftF1', on && binding === 'shift+f1' && !excluded);
}

// ------------------------------------------------------------------ local candidates (JS engine; optional Rust core)
const toRange = (position, prefix) => new vscode.Range(position.line, position.character - prefix.length, position.line, position.character);
function usageMap() { return new Map(Object.entries(rt.ctx.globalState.get('pime.usage') || {})); }
function bumpUsage(id) {
  const u = rt.ctx.globalState.get('pime.usage') || {};
  u[id] = (u[id] || 0) + 1;
  rt.ctx.globalState.update('pime.usage', u); // counts per candidate id only - never the inserted code
}
async function localCandidates(doc, position, info) {
  const linePrefix = doc.lineAt(position.line).text.slice(0, position.character);
  const prefix = languages.extractPrefix(info.language, linePrefix);
  if (!prefix) return {prefix, range: toRange(position, ''), candidates: []};
  const offset = doc.offsetAt(position);
  const before = doc.getText(new vscode.Range(doc.positionAt(Math.max(0, offset - 6000)), position));
  const beforePrefix = before.slice(0, before.length - prefix.length);
  const after = doc.getText(new vscode.Range(position, doc.positionAt(Math.min(doc.getText().length, offset + 2000))));
  const limit = cfg().get('maxVisibleCandidates') || 6;
  const request = {profile: info.language, prefix, before: beforePrefix, after, dialect: info.dialect, limit, pins: new Set(cfg().get('pinnedCandidates') || []), usage: usageMap()};
  const client = rt.engine.predictClient();
  if (client) {
    try {
      const out = await client.request({op: 'predict', language: info.language, prefix, before: beforePrefix, after, dialect: info.dialect, limit, pins: [...request.pins], usage: Object.fromEntries(request.usage)}, {timeoutMs: rt.engine.predictTimeoutMs});
      if (out.ok && Array.isArray(out.candidates)) return {prefix, range: toRange(position, prefix), candidates: out.candidates};
      throw new Error('CORE_BAD_REPLY');
    } catch { rt.supervisor.reportFailure(); /* Direct fallback: the JS engine below always works */ }
  }
  // the Rust engine never falls back to the JS implementation: no candidates for this keystroke (a dead core is reported by the engine)
  if (!rt.engine.fallbackAllowed) return {prefix, range: toRange(position, ''), candidates: []};
  return {prefix, range: toRange(position, prefix), candidates: languages.predict(request)};
}
const snippetFor = c => new vscode.SnippetString(languages.toSnippet(c.text));
const escapePlain = s => s.replace(/[\\$}]/g, '\\$&');

// ------------------------------------------------------------------ realtime AI (FR-027, async, never blocks typing)
function realtimeAllowed() {
  if (!enabled() || rt.session.paused || !aiEnabled() || cfg().get('realtimeAI') !== true || !vscode.workspace.isTrusted) return false;
  const sc = safeScenario('realtime');
  return !!sc && !!sc.model && !!rt.broker.findNow({providerId: grantProviderId(sc), workspace: workspaceId(), scope: 'L1'});
}
async function runRealtime(payload, signal) {
  const sc = settingsMod.resolveScenario(rt.settings, 'realtime');
  const key = await rt.ctx.secrets.get('pime-key:' + sc.providerId);
  const verdict = secrets.assessOutbound(payload.before + payload.after, {fileName: payload.fileName, scope: 'realtime'});
  if (verdict.action === 'block') throw fail('SENSITIVE_BLOCKED', 'nearby code contains high-risk secrets');
  let {before, after} = payload;
  const map = [];
  if (verdict.action === 'redact') {
    for (const part of ['before', 'after']) {
      const text = part === 'before' ? before : after;
      const red = secrets.redact(text, secrets.scan(text).findings);
      map.push(...red.map); if (part === 'before') before = red.text; else after = red.text;
    }
  }
  const prompt = aiMod.buildPrompt('realtime', {language: payload.language, dialect: payload.dialect, before, after, typedPrefix: payload.prefix});
  const res = await rt.ai.infer({...sc, providerId: grantProviderId(sc), key, ...prompt, signal, grantScope: 'L1'});
  const parsed = aiMod.parseModelResult(res.text, {task: 'realtime'});
  return parsed.insertCandidates.slice(0, 2).map(raw => map.length ? secrets.restore(raw, map) : {text: raw, unresolved: 0}).filter(r => !r.unresolved && r.text.trim()).map((r, i) => ({
    id: `ai.${sha256(r.text).slice(0, 10)}`, kind: 'ai_inline', source: 'ai', profile_id: payload.language, label: r.text.split('\n')[0].slice(0, 80), text: r.text, plain: true,
    explain: 'AI suggestion - not verified by the local syntax rules', validation: 'unchecked', assumptions: ['Model output; review before accepting'], match: 'context', weight: 10 - i
  }));
}
function setupScheduler() {
  rt.scheduler = new RealtimeScheduler({
    debounceMs: cfg().get('debounceMs'), run: runRealtime, isAllowed: realtimeAllowed,
    onResult: (items, key) => {
      const editor = vscode.window.activeTextEditor;
      if (!editor || !rt.session.isCurrent(key)) return;
      rt.aiCache.set(key.seq + ':' + editor.document.uri.toString(), {key, version: editor.document.version, items});
      if (rt.aiCache.size > 20) rt.aiCache.delete(rt.aiCache.keys().next().value);
      vscode.commands.executeCommand('editor.action.inlineSuggest.trigger'); // re-ask the provider: the cache now holds AI items
    },
    onDiscard: () => {}
  });
}
function cachedAi(doc) {
  const hit = rt.aiCache.get(rt.session.inputSeq + ':' + doc.uri.toString());
  return hit && hit.version === doc.version && rt.session.isCurrent(hit.key) ? hit.items : [];
}

// ------------------------------------------------------------------ inline suggestions
const inlineProvider = {
  async provideInlineCompletionItems(doc, position, _ictx, token) {
    if (!enabled() || rt.session.paused || cfg().get('suggestMode') !== 'auto') return [];
    const info = lang.infoFor(doc);
    if (info.language === 'unknown') return [];
    const {prefix, range, candidates} = await localCandidates(doc, position, info);
    if (!prefix || token.isCancellationRequested) return [];
    const items = [];
    for (const c of candidates.filter(x => x.text != null).slice(0, 3)) {
      const item = new vscode.InlineCompletionItem(snippetFor(c), range);
      item.filterText = prefix; // ghost text is shown although the typed prefix is only an abbreviation
      items.push(item);
    }
    // local suggestions are returned at once; AI results arrive later through the cache + re-trigger
    if (realtimeAllowed()) {
      const key = rt.session.key();
      for (const c of cachedAi(doc).slice(0, 2)) {
        const item = new vscode.InlineCompletionItem(c.text, range); item.filterText = prefix; items.push(item);
      }
      const marker = `${key.seq}:${key.profileEpoch}:${key.documentEpoch}`;
      if (lastScheduled !== marker) {
        lastScheduled = marker;
        const offset = doc.offsetAt(position);
        rt.scheduler.schedule(key, {
          language: info.language, dialect: info.dialect, prefix, fileName: doc.fileName,
          before: doc.getText(new vscode.Range(doc.positionAt(Math.max(0, offset - 1800)), position)),
          after: doc.getText(new vscode.Range(position, doc.positionAt(Math.min(doc.getText().length, offset + 900))))
        });
      }
    }
    return items;
  }
};

// ------------------------------------------------------------------ commands
async function showCandidates() {
  const editor = vscode.window.activeTextEditor;
  if (!editor) return;
  if (!enabled()) { vscode.window.showWarningMessage(L('f1.disabledHere')); return; }
  if (rt.session.paused) { vscode.window.showInformationMessage(L('cand.paused')); return; }
  const doc = editor.document, info = lang.infoFor(doc);
  const version0 = doc.version, selection0 = editor.selection;
  const {prefix, range, candidates} = await localCandidates(doc, editor.selection.active, info);
  const all = [...candidates, ...cachedAi(doc)];
  if (!all.length) { vscode.window.showInformationMessage(L('cand.none')); return; }
  const items = all.map((c, i) => { const row = candidateRow(rt.locale, c, i + 1, all.length); return {label: row.label, description: row.description, detail: row.detail, accessibilityInformation: {label: row.ariaLabel}, c}; });
  const pick = await vscode.window.showQuickPick(items, {title: L('cand.pick', info.language, prefix), matchOnDescription: true, matchOnDetail: true});
  if (!pick) return; // Esc / dismiss: zero writes
  const c = pick.c;
  if (c.text == null) { vscode.window.showInformationMessage(`${c.label}: ${c.explain || ''}`); return; }
  // autoComment / graphComment: the user's AI adds comments to the chosen snippet; null = insert it unchanged
  const annotated = (c.kind === 'syntax' || c.kind === 'ai_inline') ? await annotateInsertion({doc, info, template: c.text, position: editor.selection.active}) : null;
  const text = annotated ? annotated.text : c.text;
  if (vscode.window.activeTextEditor !== editor || doc.version !== version0 || !editor.selection.isEqual(selection0)) {
    vscode.window.showWarningMessage(L('cand.stale')); return; // never insert at an old position (also after waiting for the AI)
  }
  const ok = await editor.insertSnippet(c.plain ? new vscode.SnippetString(escapePlain(text)) : new vscode.SnippetString(languages.toSnippet(text)), range);
  if (ok) {
    bumpUsage(c.id);
    vscode.window.setStatusBarMessage(annotated ? L('annotate.done', annotated.kinds.join(' + ')) : L('cand.inserted'), 4000);
  }
}
async function chooseLanguage() {
  const editor = vscode.window.activeTextEditor;
  const items = [{label: L('lang.auto'), value: 'auto'}, {label: L('lang.fileLock'), kind: vscode.QuickPickItemKind.Separator}];
  for (const id of languages.LANGUAGES) items.push({label: id, description: L('lang.fileLock'), value: id, scope: 'file'});
  items.push({label: L('lang.globalLock'), kind: vscode.QuickPickItemKind.Separator});
  for (const id of languages.LANGUAGES) items.push({label: `${id} (all files)`, description: L('lang.globalLock'), value: id, scope: 'global'});
  const pick = await vscode.window.showQuickPick(items, {title: L('lang.pick'), matchOnDescription: true});
  if (!pick || !editor) return;
  if (pick.value === 'auto') lang.unlock(editor.document); else lang.lock(editor.document, pick.value, pick.scope);
  updateStatus();
}
async function menu() {
  const items = [
    {label: L('menu.language'), run: chooseLanguage},
    {label: rt.session.paused ? L('menu.resume') : L('menu.pause'), run: async () => togglePause()},
    {label: aiEnabled() ? 'AI: off' : 'AI: on', run: toggleAI},
    {label: L('menu.usage'), run: showUsage},
    {label: L('menu.permissions'), run: managePermissions},
    {label: L('menu.diagnostics'), run: showDiagnostics},
    {label: L('menu.f1'), run: () => f1.open()}
  ];
  const pick = await vscode.window.showQuickPick(items, {title: L('menu.title')});
  if (pick) await pick.run();
}
function togglePause() {
  rt.session.paused = !rt.session.paused;
  if (rt.session.paused && rt.scheduler) rt.scheduler.cancel();
  updateStatus();
}
async function toggleAI() {
  const next = !aiEnabled();
  await cfg().update('aiEnabled', next, vscode.ConfigurationTarget.Global);
  if (!next && rt.scheduler) rt.scheduler.cancel();
  updateStatus();
}
function showUsage() {
  const b = rt.budget.snapshot();
  vscode.window.showInformationMessage(L('f1.usage', b.requests, b.tokens, b.estimated ? ' (estimated)' : '') + (b.cost !== null ? ` ≈ ${b.cost}` : ''));
}
function showDiagnostics() {
  rt.output.appendLine(L('diag.title'));
  rt.output.appendLine(JSON.stringify(rt.diagnostics.summary()));
  const build = Number((require('node:os').release().split('.')[2]) || 0);
  rt.output.appendLine(JSON.stringify({environment: checkEnvironment({platform: process.platform, arch: process.arch, build}), core: rt.engine.describe(), engine: rt.engine.kind, direct: rt.supervisor.isDirect}));
  rt.output.show(true);
}
async function managePermissions() {
  const grants = await rt.broker.list();
  if (!grants.length) { vscode.window.showInformationMessage(L('perm.none')); return; }
  const items = [{label: L('perm.revokeAll'), all: true}, ...grants.map(g => ({label: g.providerId, description: `${g.scope} · ${g.once ? 'this request' : 'workspace'} · until ${new Date(g.expiresAt).toLocaleString()}`, id: g.id}))];
  const pick = await vscode.window.showQuickPick(items, {title: L('perm.title')});
  if (!pick) return;
  if (pick.all) { await rt.broker.revokeAll(); vscode.window.showInformationMessage(L('perm.revokedAll')); } else await rt.broker.revoke(pick.id);
  if (rt.scheduler) rt.scheduler.cancel();
  updateStatus();
}
async function setObject(key, mutate) {
  const current = JSON.parse(JSON.stringify(cfg().get(key) || {}));
  mutate(current);
  await cfg().update(key, current, vscode.ConfigurationTarget.Global);
  refreshSettings();
}
async function configureAI() {
  const providers = rt.settings.providers;
  const pick = await vscode.window.showQuickPick([...Object.keys(providers).map(id => ({label: id, description: `${providers[id].api} · ${providers[id].endpoint}`, id})), {label: '$(add) new provider…', id: null}], {title: 'Provider'});
  if (!pick) return;
  let id = pick.id, api;
  if (id === null) {
    id = await vscode.window.showInputBox({prompt: 'Provider id (letters, digits, - _ .)', validateInput: v => /^[\w.-]{1,40}$/.test(v || '') ? undefined : 'invalid id'});
    if (!id) return;
    const fam = await vscode.window.showQuickPick(Object.keys(aiMod.FAMILIES), {title: 'API family'});
    if (!fam) return; api = fam;
  } else api = providers[id].api;
  const endpoint = await vscode.window.showInputBox({prompt: 'HTTPS endpoint', value: providers[id] ? providers[id].endpoint : '', ignoreFocusOut: true,
    validateInput: v => { try { aiMod.checkEndpoint(v); return undefined; } catch { return 'HTTPS endpoint without credentials required'; } }});
  if (!endpoint) return;
  const model = await vscode.window.showInputBox({prompt: 'Exact model id', value: safeScenario('f1Full')?.model || '', ignoreFocusOut: true});
  if (!model) return;
  const key = await vscode.window.showInputBox({prompt: `API key for "${id}" - stored in VS Code SecretStorage`, password: true, ignoreFocusOut: true});
  if (!key) return;
  await rt.ctx.secrets.store('pime-key:' + id, key);
  await setObject('providers', o => { o[id] = {api, endpoint}; });
  await setObject('scenarios', o => { for (const n of settingsMod.SCENARIOS) o[n] = {...(o[n] || {}), provider: id, model}; });
  await cfg().update('aiEnabled', true, vscode.ConfigurationTarget.Global);
  await rt.broker.revokeProvider(`${id}:${new URL(endpoint).host}`);
  vscode.window.showInformationMessage(L('ai.configured'));
  updateStatus();
}
async function deleteKey() {
  const id = await vscode.window.showQuickPick(Object.keys(rt.settings.providers), {title: 'Delete API key of provider'});
  if (!id) return;
  await rt.ctx.secrets.delete('pime-key:' + id);
  await rt.broker.revokeProviderPrefix(id + ':'); // key gone -> no authorisation survives
  if (rt.scheduler) rt.scheduler.cancel();
  vscode.window.showInformationMessage(L('ai.keyDeleted'));
}
async function chooseModel() {
  const scope = await vscode.window.showQuickPick([{label: 'Realtime completion', v: ['realtime']}, {label: 'F1 whole file', v: ['f1Full']}, {label: 'F1 insert', v: ['f1Insert']}, {label: 'All scenarios', v: settingsMod.SCENARIOS}], {title: 'Scenario'});
  if (!scope) return;
  const first = safeScenario(scope.v[0]);
  const model = await vscode.window.showInputBox({prompt: 'Exact model id', value: first?.model || ''});
  if (!model) return;
  const effort = await vscode.window.showQuickPick(aiMod.EFFORT_LEVELS, {title: 'Effort (never sent unless the capability below says it is supported)'});
  if (!effort) return;
  const cap = await vscode.window.showQuickPick([
    {label: 'Unknown - do not send effort', v: {state: 'unknown'}},
    {label: 'Supported: low, medium, high (verified against provider docs)', v: {state: 'supported', supported: ['low', 'medium', 'high'], checkedAt: new Date().toISOString()}},
    {label: 'Supported: none, minimal, low, medium, high, xhigh, max (OpenAI list; real support varies by model - verify yours)', v: {state: 'supported', supported: ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'], checkedAt: new Date().toISOString()}},
    {label: 'Not supported', v: {state: 'unsupported'}}], {title: 'Model capability - do not guess'});
  if (!cap) return;
  await setObject('scenarios', o => { for (const n of scope.v) o[n] = {...(o[n] || {}), model, effort, effortCapability: cap.v}; });
  updateStatus();
}
async function toggleRealtimeAI() {
  const next = cfg().get('realtimeAI') !== true;
  if (next) {
    if (!vscode.workspace.isTrusted) { vscode.window.showWarningMessage(L('consent.untrusted')); return; }
    const sc = safeScenario('realtime');
    if (!aiEnabled() || !sc || !sc.model || !await rt.ctx.secrets.get('pime-key:' + sc.providerId)) { vscode.window.showWarningMessage(L('consent.noKey')); return; }
    const gk = grantProviderId(sc);
    if (!await rt.broker.find({providerId: gk, workspace: workspaceId(), scope: 'L1'})) {
      const choice = await vscode.window.showWarningMessage(L('consent.nearby', gk.slice(gk.indexOf(':') + 1)), {modal: true, detail: L('consent.risk')}, L('consent.workspace'));
      if (!choice) return;
      await rt.broker.authorize({providerId: gk, workspace: workspaceId(), scope: 'L1', ttlMs: 24 * 3600 * 1000});
    }
  } else if (rt.scheduler) rt.scheduler.cancel();
  await cfg().update('realtimeAI', next, vscode.ConfigurationTarget.Global);
  vscode.window.showInformationMessage(next ? L('ai.realtimeOn') : L('ai.realtimeOff'));
}
async function reloadRules() {
  const dir = path.join(rt.ctx.globalStorageUri.fsPath, 'rules');
  fs.mkdirSync(dir, {recursive: true});
  const files = fs.readdirSync(dir).filter(n => n.endsWith('.json')).map(name => ({name, text: fs.readFileSync(path.join(dir, name), 'utf8')}));
  const built = settingsMod.buildRegistryWithUserRules(files);
  languages.setRegistry(built.registry);
  if (built.rejected.length) vscode.window.showWarningMessage(`Programming IME rules: ${built.rejected.length} pack(s) rejected - ${built.rejected.map(r => `${r.name}: ${r.message}`).join('; ')}`);
  else if (files.length) vscode.window.showInformationMessage(`Programming IME rules loaded: ${built.accepted.join(', ')}`);
}
async function migrateOldSettings() {
  if (rt.ctx.globalState.get('pime.settingsSchema') === settingsMod.SCHEMA_VERSION) return;
  const c = cfg();
  const old = {provider: c.get('provider'), endpoint: c.get('endpoint'), model: c.get('model'), effort: c.get('effort'), effortCapability: c.get('effortCapability')};
  const hasOld = old.model || (old.provider && old.provider !== 'openai') || (old.effort && old.effort !== 'auto');
  try {
    if (hasOld && !Object.keys(c.get('scenarios') || {}).length) {
      const m = settingsMod.migrateV0(old);
      await c.update('providers', m.providers, vscode.ConfigurationTarget.Global);
      await c.update('scenarios', m.scenarios, vscode.ConfigurationTarget.Global);
      rt.ctx.globalState.update('pime.settingsBackup', old); // previous values stay recoverable
      if (m.conflicts.length) vscode.window.showWarningMessage(`Programming IME settings migrated with ${m.conflicts.length} conflict(s): ${m.conflicts.map(x => x.key).join(', ')}`);
    }
    await rt.ctx.globalState.update('pime.settingsSchema', settingsMod.SCHEMA_VERSION);
  } catch { /* the old keys are untouched: migration is retried at the next start */ }
}
// js engine: the configured core is an optional accelerator; rust engine: it is required (see engine.js)
function startCore() {
  const binary = cfg().get('coreExecutable');
  const c = cfg();
  return rt.engine.start({binary, locale: rt.locale, values: {providers: c.get('providers'), scenarios: c.get('scenarios'), budget: c.get('budget')}});
}
// the Rust engine lost (or never had) its core: say so once, switch the IME features off, leave typing alone
function onEngineChange() {
  updateStatus(); updateContextKeys();
  if (rt.engine.available() || engineNotified) return;
  engineNotified = true;
  vscode.window.showErrorMessage(L('engine.unavailable', rt.engine.reason));
}

// ------------------------------------------------------------------ activation
async function activate(ctx) {
  init(ctx);
  lang.initLocks();
  await migrateOldSettings();
  refreshSettings();
  rt.session.paused = false;
  setupScheduler();
  status.lang = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
  status.lang.command = 'programmingIme.chooseLanguage'; status.lang.name = 'Programming IME language';
  status.ai = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 99);
  status.ai.command = 'programmingIme.menu'; status.ai.name = 'Programming IME AI';
  ctx.subscriptions.push(status.lang, status.ai);
  f1.registerPreviewProvider(ctx);
  rt.onEngineChange = onEngineChange;
  await startCore();
  await reloadRules().catch(() => {});
  await updateContextKeys();
  updateStatus();
  const reg = (id, fn) => ctx.subscriptions.push(vscode.commands.registerCommand(id, fn));
  reg('programmingIme.enable', async () => { await cfg().update('enabled', !enabled(), vscode.ConfigurationTarget.Global); await updateContextKeys(); updateStatus(); });
  reg('programmingIme.chooseLanguage', chooseLanguage);
  reg('programmingIme.showCandidates', showCandidates);
  reg('programmingIme.menu', menu);
  reg('programmingIme.togglePause', () => togglePause());
  reg('programmingIme.toggleAI', toggleAI);
  reg('programmingIme.configureAI', configureAI);
  reg('programmingIme.deleteKey', deleteKey);
  reg('programmingIme.chooseModel', chooseModel);
  reg('programmingIme.toggleRealtimeAI', toggleRealtimeAI);
  reg('programmingIme.managePermissions', managePermissions);
  reg('programmingIme.showUsage', showUsage);
  reg('programmingIme.resetUsage', () => { rt.budget.reset(); updateStatus(); });
  reg('programmingIme.showDiagnostics', showDiagnostics);
  reg('programmingIme.reloadRules', reloadRules);
  reg('programmingIme.openRulesFolder', async () => { const dir = path.join(rt.ctx.globalStorageUri.fsPath, 'rules'); fs.mkdirSync(dir, {recursive: true}); await vscode.commands.executeCommand('revealFileInOS', vscode.Uri.file(dir)); });
  reg('programmingIme.f1Edit', () => f1.open());
  ctx.subscriptions.push(vscode.languages.registerInlineCompletionItemProvider([{scheme: 'file'}, {scheme: 'untitled'}], inlineProvider));
  ctx.subscriptions.push(
    vscode.window.onDidChangeActiveTextEditor(() => { rt.session.noteDocumentChange(); lastScheduled = null; updateStatus(); updateContextKeys(); }),
    vscode.workspace.onDidChangeTextDocument(e => {
      if (e.contentChanges.length && vscode.window.activeTextEditor && e.document === vscode.window.activeTextEditor.document) { rt.session.noteInput(); lang.invalidate(); }
      f1.onDocumentChanged(e);
    }),
    vscode.window.onDidChangeTextEditorSelection(e => { f1.onSelectionChanged(e); }),
    vscode.workspace.onDidCloseTextDocument(d => f1.onDocumentClosed(d)),
    vscode.workspace.onDidGrantWorkspaceTrust(() => updateStatus()),
    vscode.workspace.onDidChangeConfiguration(e => {
      if (!e.affectsConfiguration('programmingIme')) return;
      refreshSettings(); lang.invalidate();
      if (e.affectsConfiguration('programmingIme.engine')) vscode.window.showInformationMessage(L('engine.reload'));
      else if (e.affectsConfiguration('programmingIme.coreExecutable')) { engineNotified = false; startCore(); }
      if (!aiEnabled() || cfg().get('realtimeAI') !== true) rt.scheduler.cancel();
      updateContextKeys(); updateStatus();
    })
  );
}
function deactivate() {
  if (rt.scheduler) rt.scheduler.cancel();
  f1.closeSession();
  rt.engine.stop();
}
module.exports = {activate, deactivate};
