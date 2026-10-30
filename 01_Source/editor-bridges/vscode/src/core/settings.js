'use strict';
// Versioned settings, v0.1 -> v1 migration, atomic persistence and user rule loading (FR-045/046, NFR-401,
// DD §11). Settings never contain keys; keys live in SecretStorage and are referenced by provider id only.
const fs = require('node:fs');
const {fail} = require('./errors');
const secrets = require('./secrets');
const ai = require('./ai');
const {loadRegistry, LIMITS} = require('./languages');

const SCHEMA_VERSION = 1;
const SCENARIOS = Object.freeze(['realtime', 'f1Full', 'f1Insert']);
const DEFAULT_PROVIDERS = Object.freeze({
  openai: Object.freeze({api: 'openai-responses', endpoint: 'https://api.openai.com/v1/responses'}),
  'openai-compatible': Object.freeze({api: 'openai-chat', endpoint: 'https://api.example.com/v1/chat/completions'}),
  anthropic: Object.freeze({api: 'anthropic-messages', endpoint: 'https://api.anthropic.com/v1/messages'})
});
const baseScenario = {provider: 'openai', model: '', effort: 'auto', effortCapability: {state: 'unknown'}};
const DEFAULT_SCENARIOS = Object.freeze({
  realtime: Object.freeze({...baseScenario, maxOutputTokens: 250, timeoutMs: 8000}),
  f1Full: Object.freeze({...baseScenario, maxOutputTokens: 12000, timeoutMs: 60000}),
  f1Insert: Object.freeze({...baseScenario, maxOutputTokens: 1500, timeoutMs: 30000})
});
const DEFAULT_BUDGET = Object.freeze({maxRequests: 200, maxTokens: 400000, pricePer1kTokens: 0});

// user values override defaults field by field; unknown scenarios/providers are ignored, not trusted.
function effective({providers = {}, scenarios = {}, budget = {}} = {}) {
  const outProviders = {};
  for (const [id, def] of Object.entries({...DEFAULT_PROVIDERS, ...providers})) if (/^[\w.-]{1,40}$/.test(id) && def && typeof def === 'object') outProviders[id] = {...(DEFAULT_PROVIDERS[id] || {}), ...def};
  const outScenarios = {};
  for (const name of SCENARIOS) outScenarios[name] = {...DEFAULT_SCENARIOS[name], ...(scenarios[name] || {})};
  return {providers: outProviders, scenarios: outScenarios, budget: {...DEFAULT_BUDGET, ...budget}};
}
// Resolve the concrete request settings for one scenario (FR-021: scenarios are fully independent).
function resolveScenario(settings, name) {
  const s = settings.scenarios[name];
  if (!s) throw fail('SETTINGS_INVALID', `unknown scenario ${name}`);
  const p = settings.providers[s.provider];
  if (!p) throw fail('MODEL_UNSUPPORTED', `provider "${s.provider}" is not defined`);
  return {scenario: name, providerId: s.provider, family: p.api, endpoint: p.endpoint, model: s.model, effort: s.effort, capability: s.effortCapability, maxOutputTokens: s.maxOutputTokens, timeoutMs: s.timeoutMs};
}
function validateSettings(settings) {
  const issues = [];
  for (const [id, p] of Object.entries(settings.providers || {})) {
    if (!ai.FAMILIES[p.api]) issues.push({path: `providers.${id}.api`, message: 'unknown API family'});
    try { ai.checkEndpoint(p.endpoint); } catch { issues.push({path: `providers.${id}.endpoint`, message: 'HTTPS endpoint required'}); }
  }
  for (const [name, s] of Object.entries(settings.scenarios || {})) {
    if (!settings.providers[s.provider]) issues.push({path: `scenarios.${name}.provider`, message: 'undefined provider'});
    if (!ai.EFFORT_LEVELS.includes(s.effort)) issues.push({path: `scenarios.${name}.effort`, message: 'invalid effort level'});
    if (!Number.isInteger(s.maxOutputTokens) || s.maxOutputTokens < 1 || s.maxOutputTokens > 64000) issues.push({path: `scenarios.${name}.maxOutputTokens`, message: 'out of range'});
    if (!Number.isInteger(s.timeoutMs) || s.timeoutMs < 1000 || s.timeoutMs > 300000) issues.push({path: `scenarios.${name}.timeoutMs`, message: 'out of range (1 s - 300 s)'});
  }
  return issues;
}

// v0.1 flat keys {provider, endpoint, model, effort, effortCapability} -> v1 structure (FR-046).
function migrateV0(old = {}, now = Date.now()) {
  const conflicts = [], notes = [];
  const providers = JSON.parse(JSON.stringify(DEFAULT_PROVIDERS));
  const provider = old.provider === 'openai-compatible' ? 'openai-compatible' : 'openai';
  if (old.endpoint) {
    try { ai.checkEndpoint(old.endpoint); providers[provider].endpoint = old.endpoint; }
    catch { conflicts.push({key: 'endpoint', reason: 'not a valid HTTPS endpoint; the default was kept'}); }
  }
  const cap = old.effortCapability === 'supported' ? {state: 'supported', supported: ['low', 'medium', 'high'], checkedAt: new Date(now).toISOString()}
    : old.effortCapability === 'unsupported' ? {state: 'unsupported'} : {state: 'unknown'};
  const effort = ai.EFFORT_LEVELS.includes(old.effort) ? old.effort : 'auto';
  if (old.effort && effort === 'auto' && old.effort !== 'auto') conflicts.push({key: 'effort', reason: `unknown level "${old.effort}" reset to auto`});
  const base = {provider, model: typeof old.model === 'string' ? old.model : '', effort, effortCapability: cap};
  const scenarios = {};
  for (const name of SCENARIOS) scenarios[name] = {...DEFAULT_SCENARIOS[name], ...base};
  notes.push('Authorisations from v0.1 are not carried over; authorise again on first use.');
  return {schema_version: SCHEMA_VERSION, providers, scenarios, conflicts, notes};
}
function migrate(doc, now) {
  const v = doc && doc.schema_version;
  if (v === SCHEMA_VERSION) return {...doc, conflicts: [], notes: []};
  if (v === undefined || v === 0) return migrateV0(doc, now);
  throw fail('SETTINGS_INVALID', `settings schema_version ${v} is newer than this build (${SCHEMA_VERSION}); downgrade is not supported`);
}

// Migration through a store: the previous state is preserved if anything fails (UT-059/073).
function migrateStore(store, now) {
  const backup = store.read();
  const next = migrate(backup, now);
  // fields we do not own (user rules, preferences) pass through untouched; only the legacy flat keys are retired
  const {provider, endpoint, model, effort, effortCapability, ...kept} = backup;
  try { store.write({...kept, schema_version: next.schema_version, providers: next.providers, scenarios: next.scenarios}); }
  catch (e) { throw fail('SETTINGS_INVALID', `migration aborted, previous settings are intact (${e.code || e.message})`); }
  return {settings: next, backup, conflicts: next.conflicts, notes: next.notes};
}

// write temp -> keep .bak -> rename: a crash or ENOSPC before the rename leaves the old file untouched.
function writeFileAtomic(file, text, fsImpl = fs) {
  const tmp = `${file}.tmp-${process.pid}`;
  try {
    fsImpl.writeFileSync(tmp, text);
    if (fsImpl.existsSync(file)) fsImpl.copyFileSync(file, `${file}.bak`);
    fsImpl.renameSync(tmp, file);
  } catch (e) {
    try { fsImpl.rmSync(tmp, {force: true}); } catch { /* best effort */ }
    throw e;
  }
}
function readJsonRecovering(file, validate = () => {}, fsImpl = fs) {
  for (const [candidate, recovered] of [[file, false], [`${file}.bak`, true]]) {
    try { const value = JSON.parse(fsImpl.readFileSync(candidate, 'utf8')); validate(value); return {value, recovered}; } catch { /* try next */ }
  }
  return {value: null, recovered: false};
}

function exportSettings(settings) {
  const text = JSON.stringify({schema_version: SCHEMA_VERSION, providers: settings.providers, scenarios: settings.scenarios, budget: settings.budget}, null, 2);
  if (secrets.scan(text).blocked) throw fail('SETTINGS_INVALID', 'export refused: it would contain secret-like data');
  return text;
}
function importSettings(text) {
  let doc;
  try { doc = JSON.parse(text); } catch { throw fail('SETTINGS_INVALID', 'settings file is not valid JSON'); }
  const migrated = migrate(doc);
  const settings = effective(migrated);
  const issues = validateSettings(settings);
  if (issues.length) throw fail('SETTINGS_INVALID', `invalid settings: ${issues.map(i => i.path).join(', ')}`);
  return {settings, conflicts: migrated.conflicts, notes: migrated.notes};
}

// User rule packs (FR-045, UT-058): validated as data in a fresh registry; a bad pack is reported and skipped,
// and the active registry is only replaced by the returned one, so a failure never degrades the old rules.
function buildRegistryWithUserRules(files, makeBase = () => loadRegistry()) {
  const reg = makeBase(), accepted = [], rejected = [];
  for (const f of files) {
    try {
      if (Buffer.byteLength(f.text) > LIMITS.packBytes) throw fail('RULEPACK_INVALID', 'pack exceeds size limit');
      let pack; try { pack = JSON.parse(f.text); } catch { throw fail('RULEPACK_INVALID', 'not valid JSON'); }
      reg.addUser(pack, Buffer.byteLength(f.text));
      accepted.push(f.name);
    } catch (e) { rejected.push({name: f.name, code: e.code || 'RULEPACK_INVALID', message: e.message}); }
  }
  return {registry: reg, accepted, rejected};
}
module.exports = {
  SCHEMA_VERSION, SCENARIOS, DEFAULT_PROVIDERS, DEFAULT_SCENARIOS, DEFAULT_BUDGET, effective, resolveScenario, validateSettings,
  migrateV0, migrate, migrateStore, writeFileAtomic, readJsonRecovering, exportSettings, importSettings, buildRegistryWithUserRules
};
