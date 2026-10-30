'use strict';
// Shared mutable runtime for the VS Code bridge (settings, session, broker, AI client, coordinator).
// Pure logic lives in ./core; this file is the only place that wires it to the VS Code API.
const vscode = require('vscode');
const {InputSession, CoreSupervisor} = require('./core/scheduler');
const {createEngine} = require('./engine');
const {EditCoordinator} = require('./core/transaction');
const {Diagnostics} = require('./core/diagnostics');
const {t, describeError, resolveLocale} = require('./core/i18n');
const {codeOf, fail} = require('./core/errors');
const settingsMod = require('./core/settings');

const cfg = () => vscode.workspace.getConfiguration('programmingIme');
const rt = {
  ctx: null, settings: settingsMod.effective(), locale: 'en-US', session: new InputSession(), broker: null,
  budget: null, diagnostics: new Diagnostics(), coordinator: new EditCoordinator(), ai: null, scheduler: null, engine: null, onEngineChange: () => {},
  supervisor: new CoreSupervisor(), output: null, aiCache: new Map(), cfg
};
const L = (key, ...args) => t(rt.locale, key, ...args);
const errorText = e => describeError(rt.locale, codeOf(e), e && e.message);
// with the Rust engine the IME is off while its core is unavailable (typing in VS Code itself is never affected)
const enabled = () => cfg().get('enabled') === true && (!rt.engine || rt.engine.available());
const aiEnabled = () => cfg().get('aiEnabled') === true;
const workspaceId = () => {
  const folders = vscode.workspace.workspaceFolders || [];
  return rt.engine.workspaceKey(folders.length ? folders.map(f => f.uri.toString()) : ['no-folder']);
};
function refreshSettings() {
  const c = cfg();
  rt.settings = settingsMod.effective({providers: c.get('providers'), scenarios: c.get('scenarios'), budget: c.get('budget')});
  rt.locale = resolveLocale(c.get('uiLocale'), vscode.env.language);
  if (rt.engine) rt.engine.applySettings({providers: c.get('providers'), scenarios: c.get('scenarios'), budget: c.get('budget')}, rt.locale);
  const b = rt.settings.budget;
  rt.budget.maxRequests = b.maxRequests > 0 ? b.maxRequests : Infinity;
  rt.budget.maxTokens = b.maxTokens > 0 ? b.maxTokens : Infinity;
  rt.budget.price = b.pricePer1kTokens || 0;
  if (rt.scheduler) rt.scheduler.setDebounce(c.get('debounceMs'));
}
// grants are bound to provider AND target host, so changing the endpoint never inherits an old authorisation
const grantProviderId = sc => {
  let host = 'invalid';
  try { host = new URL(sc.endpoint).host; } catch { /* keeps "invalid" */ }
  return `${sc.providerId}:${host}`.slice(0, 64);
};
async function authorizeRequest(req) {
  if (!vscode.workspace.isTrusted) throw fail('CONSENT_REQUIRED', 'workspace is not trusted');
  const criteria = {providerId: req.providerId, workspace: workspaceId(), scope: req.grantScope};
  if (req.grantId) { await rt.broker.require(req.grantId, criteria); await rt.broker.consume(req.grantId); return true; }
  if (!await rt.broker.find(criteria)) throw fail('CONSENT_REQUIRED', 'no authorisation for this provider and scope');
  return true;
}
function init(ctx) {
  rt.ctx = ctx;
  rt.engine = createEngine(cfg().get('engine') === 'rust' ? 'rust' : 'js', {globalState: ctx.globalState, onChange: () => rt.onEngineChange()});
  rt.broker = rt.engine.broker; rt.coordinator = rt.engine.coordinator; rt.budget = rt.engine.budget;
  rt.output = vscode.window.createOutputChannel('Programming IME');
  ctx.subscriptions.push(rt.output);
  rt.diagnostics.sink = line => rt.output.appendLine(line);
  rt.ai = rt.engine.createAi({diagnostics: rt.diagnostics, isEnabled: aiEnabled, authorize: authorizeRequest, isTrusted: () => vscode.workspace.isTrusted, workspace: workspaceId});
  refreshSettings();
}
module.exports = {rt, cfg, L, errorText, enabled, aiEnabled, workspaceId, refreshSettings, grantProviderId, init};
