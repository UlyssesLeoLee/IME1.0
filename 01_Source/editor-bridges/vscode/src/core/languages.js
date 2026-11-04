'use strict';
// Seven-language local engine (FR-009/010/011/012/025/026/029, DD §4-§5): declarative packs, dialects, language
// resolution, context-aware deterministic candidates, ranking and de-duplication. No network, no execution.
const fs = require('node:fs');
const path = require('node:path');
const {fail} = require('./errors');
const validators = require('./validators');
const {maskSource, validateSource} = validators;
const x86 = require('./x86');

const LANGUAGES = Object.freeze(['rust', 'python', 'typescript', 'powershell', 'c', 'assembly', 'machine-code']);
const KINDS = Object.freeze(['keyword', 'syntax', 'prefix', 'identifier', 'ai_inline', 'full_file_patch', 'byte_hex', 'explain']);
const SOURCES = Object.freeze(['rule', 'user_rule', 'context', 'ai']);
const VALIDATIONS = Object.freeze(['valid', 'verified', 'unchecked', 'invalid', 'context_missing']);
const CONTEXTS = Object.freeze(['statement', 'expression', 'type', 'unknown', 'literal']);
const MASK_STYLE = Object.freeze({rust: 'rust', c: 'c', typescript: 'typescript', python: 'python', powershell: 'powershell', sql: 'sql'});
// Plugin languages (see docs/LANGUAGE_PLUGIN_SPEC.md): a plugin pack adds a whole new language profile,
// not only triggers for an existing one. The built-in seven always stay on; plugin profiles are toggled by the user.
const PLUGIN_LANGUAGES = Object.freeze(['cypher', 'plsql', 'tsql']);
// The plugin languages that are on out of the box. T-SQL shares the `.sql` extension with PL/SQL, so it stays off
// until the user asks for it; PL/SQL owns `.sql` by default.
const DEFAULT_PLUGIN_LANGUAGES = Object.freeze(['cypher', 'plsql']);
const PLACEHOLDER = /\$\{([A-Za-z_][A-Za-z0-9_.-]*)\}/g;

// ---------------------------------------------------------------- pack validation (UT-011, UT-058)
const PACK_KEYS = ['schema_version', 'profile_id', 'version', 'display_name', 'language_ids', 'accepted_extensions', 'validator', 'dialects', 'keywords', 'triggers', 'generated'];
const USER_PACK_KEYS = ['schema_version', 'profile_id', 'version', 'keywords', 'triggers'];
const TRIGGER_KEYS = ['id', 'pattern', 'kind', 'weight', 'when', 'contexts', 'in', 'not_in', 'candidates'];
const CANDIDATE_KEYS = ['label', 'explain', 'text', 'stmt_semicolon', 'assumptions', 'when'];
const LIMITS = Object.freeze({packBytes: 262144, triggers: 2000, candidates: 8, text: 4000, label: 200, pattern: 64, keywords: 2000});
// A plugin profile adds a whole new language: its id is its own and never a built-in one.
const PLUGIN_PROFILE_ID = /^[a-z][a-z0-9-]{0,31}$/;

function checkKeys(obj, allowed, where) {
  for (const k of Object.keys(obj)) if (!allowed.includes(k)) throw fail('RULEPACK_INVALID', `${where}: unknown field "${k}" (rule packs are declarative data, never scripts)`);
}
function validatePack(pack, {builtin = true, plugin = false, bytes = 0} = {}) {
  if (!pack || typeof pack !== 'object' || Array.isArray(pack)) throw fail('RULEPACK_INVALID', 'pack must be an object');
  if (bytes > LIMITS.packBytes) throw fail('RULEPACK_INVALID', 'pack exceeds size limit');
  checkKeys(pack, builtin ? PACK_KEYS : USER_PACK_KEYS, 'pack');
  if (pack.schema_version !== 1) throw fail('RULEPACK_INVALID', 'unsupported schema_version (migration required)');
  if (plugin) {
    // a plugin pack has the shape of a built-in pack, but it may not reuse a built-in profile id
    if (typeof pack.profile_id !== 'string' || !PLUGIN_PROFILE_ID.test(pack.profile_id)) throw fail('RULEPACK_INVALID', 'invalid plugin profile_id');
    if (LANGUAGES.includes(pack.profile_id)) throw fail('RULEPACK_INVALID', 'a plugin profile must not be a built-in profile_id');
  } else if (!LANGUAGES.includes(pack.profile_id)) throw fail('RULEPACK_INVALID', 'unknown profile_id');
  if (typeof pack.version !== 'string' || !/^\d+\.\d+\.\d+$/.test(pack.version)) throw fail('RULEPACK_INVALID', 'version must be semver');
  const triggers = pack.triggers ?? [], keywords = pack.keywords ?? [];
  if (!Array.isArray(triggers) || triggers.length > LIMITS.triggers) throw fail('RULEPACK_INVALID', 'too many triggers');
  if (!Array.isArray(keywords) || keywords.length > LIMITS.keywords || keywords.some(k => typeof k !== 'string' || !/^[\w#.$-]{1,64}$/.test(k))) throw fail('RULEPACK_INVALID', 'invalid keywords');
  const ids = new Set();
  for (const t of triggers) {
    if (!t || typeof t !== 'object') throw fail('RULEPACK_INVALID', 'trigger must be an object');
    checkKeys(t, TRIGGER_KEYS, 'trigger');
    if (typeof t.id !== 'string' || !/^[\w.-]{1,80}$/.test(t.id) || ids.has(t.id)) throw fail('RULEPACK_INVALID', `trigger id missing or duplicate: ${t.id}`);
    ids.add(t.id);
    if (typeof t.pattern !== 'string' || !/^[A-Za-z0-9_#.-]{1,64}$/.test(t.pattern)) throw fail('RULEPACK_INVALID', `invalid pattern in ${t.id}`);
    if (!['syntax', 'prefix'].includes(t.kind || 'syntax')) throw fail('RULEPACK_INVALID', `invalid kind in ${t.id}`);
    if (t.weight !== undefined && (!Number.isInteger(t.weight) || t.weight < 0 || t.weight > 100)) throw fail('RULEPACK_INVALID', `invalid weight in ${t.id}`);
    for (const f of ['contexts']) if (t[f] !== undefined && (!Array.isArray(t[f]) || t[f].some(x => !CONTEXTS.includes(x)))) throw fail('RULEPACK_INVALID', `invalid ${f} in ${t.id}`);
    for (const f of ['in', 'not_in']) if (t[f] !== undefined && (!Array.isArray(t[f]) || t[f].some(x => typeof x !== 'string'))) throw fail('RULEPACK_INVALID', `invalid ${f} in ${t.id}`);
    if (t.when !== undefined && (typeof t.when !== 'object' || Array.isArray(t.when) || Object.values(t.when).some(v => !Array.isArray(v)))) throw fail('RULEPACK_INVALID', `invalid when in ${t.id}`);
    if (!Array.isArray(t.candidates) || t.candidates.length < 1 || t.candidates.length > LIMITS.candidates) throw fail('RULEPACK_INVALID', `candidates of ${t.id} out of bounds`);
    for (const c of t.candidates) {
      if (!c || typeof c !== 'object') throw fail('RULEPACK_INVALID', 'candidate must be an object');
      checkKeys(c, CANDIDATE_KEYS, 'candidate');
      if (typeof c.text !== 'string' || c.text.length > LIMITS.text || c.text.length === 0) throw fail('RULEPACK_INVALID', `invalid text in ${t.id}`);
      if (typeof c.label !== 'string' || c.label.length > LIMITS.label) throw fail('RULEPACK_INVALID', `invalid label in ${t.id}`);
      // "${" may only open a well-formed placeholder: no template/command substitution syntax is accepted.
      if (c.text.replace(PLACEHOLDER, '').includes('${')) throw fail('RULEPACK_INVALID', `malformed placeholder in ${t.id}`);
      if (c.assumptions !== undefined && (!Array.isArray(c.assumptions) || c.assumptions.some(a => typeof a !== 'string'))) throw fail('RULEPACK_INVALID', `invalid assumptions in ${t.id}`);
    }
  }
  if (builtin) {
    if (!Array.isArray(pack.language_ids) || !Array.isArray(pack.accepted_extensions) || typeof pack.dialects !== 'object') throw fail('RULEPACK_INVALID', 'incomplete built-in pack');
    for (const [axis, def] of Object.entries(pack.dialects)) {
      if (!Array.isArray(def.values) || !def.values.includes(def.default)) throw fail('RULEPACK_INVALID', `dialect axis ${axis} needs a default among its values`);
    }
  }
  return pack;
}

// ---------------------------------------------------------------- registry
// The built-in seven are always active. A plugin pack adds a whole new language profile; it stays DISCOVERABLE
// while it is off, but then it contributes nothing: no language id, no extension, no trigger, no keyword.
class Registry {
  constructor() {
    this.packs = new Map();       // the built-in profiles
    this.plugins = new Map();     // every discovered plugin pack (on or off)
    this.user = new Map();        // user rule packs per profile
    this.userKeywords = new Map();
    this.byLanguageId = new Map();// language id -> profile id (built-ins and ACTIVE plugins)
    this.byExt = new Map();       // extension -> the ACTIVE plugin profiles that own it
    this.enabledPlugins = new Set();
    this.pluginErrors = [];       // the plugin packs that were found but could not be loaded
  }
  add(pack, bytes = 0) {
    validatePack(pack, {builtin: true, bytes});
    if (this.packs.has(pack.profile_id)) throw fail('RULEPACK_INVALID', `duplicate profile ${pack.profile_id}`);
    this.packs.set(pack.profile_id, pack);
    this.rebuild();
    return this;
  }
  // A plugin pack is validated like a built-in pack and may not claim a language id or an extension of a built-in.
  addPlugin(pack, bytes = 0) {
    validatePack(pack, {builtin: true, plugin: true, bytes});
    if (this.packs.has(pack.profile_id)) throw fail('RULEPACK_INVALID', `duplicate profile ${pack.profile_id}`);
    if (this.plugins.has(pack.profile_id)) throw fail('RULEPACK_INVALID', `duplicate plugin profile ${pack.profile_id}`);
    this.assertPluginClaims(pack);
    this.plugins.set(pack.profile_id, pack);
    this.rebuild();
    return this;
  }
  assertPluginClaims(pack) {
    for (const [id, builtin] of this.packs) {
      for (const lid of pack.language_ids || []) if ((builtin.language_ids || []).includes(lid)) throw fail('RULEPACK_INVALID', `language id "${lid}" belongs to the built-in profile ${id}`);
      for (const ext of pack.accepted_extensions || []) if ((builtin.accepted_extensions || []).includes(ext)) throw fail('RULEPACK_INVALID', `extension "${ext}" belongs to the built-in profile ${id}`);
    }
  }
  // The enabled set: the plugin profiles that are on. The built-ins are not part of it (they are always on).
  setEnabledPlugins(ids) {
    this.enabledPlugins = new Set([...(ids || [])].filter(id => typeof id === 'string' && id));
    this.rebuild();
    return this;
  }
  enablePlugin(id) { if (!this.plugins.has(id)) return false; this.enabledPlugins.add(id); this.rebuild(); return true; }
  disablePlugin(id) { if (!this.enabledPlugins.delete(id)) return false; this.rebuild(); return true; }
  // The active maps are rebuilt from the packs: a plugin that is off contributes nothing.
  rebuild() {
    this.byLanguageId.clear(); this.byExt.clear();
    for (const pack of this.packs.values()) for (const id of pack.language_ids || []) if (!this.byLanguageId.has(id)) this.byLanguageId.set(id, pack.profile_id);
    for (const pack of this.plugins.values()) {
      if (!this.enabledPlugins.has(pack.profile_id)) continue;
      for (const id of pack.language_ids || []) if (!this.byLanguageId.has(id)) this.byLanguageId.set(id, pack.profile_id);
      for (const e of pack.accepted_extensions || []) { const list = this.byExt.get(e) || []; if (!list.includes(pack.profile_id)) list.push(pack.profile_id); this.byExt.set(e, list); }
    }
  }
  // User packs only add triggers/keywords; they cannot replace or remove built-in behaviour.
  addUser(pack, bytes = 0) {
    validatePack(pack, {builtin: false, bytes});
    const base = this.packs.get(pack.profile_id) || this.plugins.get(pack.profile_id);
    if (!base) throw fail('RULEPACK_INVALID', 'user pack targets an unloaded profile');
    const list = this.user.get(pack.profile_id) || [];
    const taken = new Set([...(base.triggers || []).map(t => t.id), ...list.map(t => t.id)]);
    for (const t of pack.triggers || []) if (taken.has(t.id)) throw fail('RULEPACK_INVALID', `trigger id collides with an existing rule: ${t.id}`);
    this.user.set(pack.profile_id, list.concat((pack.triggers || []).map(t => ({...t, user: true}))));
    const kw = this.userKeywords || (this.userKeywords = new Map());
    kw.set(pack.profile_id, (kw.get(pack.profile_id) || []).concat(pack.keywords || []));
    return this;
  }
  clearUser() { this.user.clear(); this.userKeywords = new Map(); }
  // The pack of an ACTIVE profile (a built-in or an enabled plugin); a plugin that is off has no active pack.
  get(profile) { return this.packs.get(profile) || (this.enabledPlugins.has(profile) ? this.plugins.get(profile) : undefined); }
  // The pack of a discovered plugin, on or off (the UI lists them).
  plugin(id) { return this.plugins.get(id); }
  triggers(profile) { return (this.get(profile)?.triggers || []).concat(this.user.get(profile) || []); }
  keywords(profile) { return (this.get(profile)?.keywords || []).concat(this.userKeywords?.get(profile) || []); }
  // An ACTIVE profile: a built-in one or an enabled plugin one.
  hasProfile(id) { return this.packs.has(id) || this.enabledPlugins.has(id); }
  // A discovered plugin pack, on or off.
  hasPlugin(id) { return this.plugins.has(id); }
  // The ACTIVE owners of an extension (an extension may belong to more than one plugin profile).
  extensionOwners(ext) { return this.byExt.get(ext) || []; }
  // Every discovered plugin profile, on or off.
  pluginProfiles() { return [...this.plugins.keys()]; }
  // The plugin profiles that are on.
  enabledPluginProfiles() { return [...this.plugins.keys()].filter(id => this.enabledPlugins.has(id)); }
  // The validator family of every active plugin profile (it comes from the pack, never from the profile id).
  families() { const out = new Map(); for (const [id, pack] of this.plugins) if (this.enabledPlugins.has(id)) out.set(id, pack.validator); return out; }
  familyOf(id) { return this.enabledPlugins.has(id) ? this.plugins.get(id)?.validator : undefined; }
}
// Check a plugin pack before it is imported: it must be a valid plugin pack and must not claim a built-in
// language id or extension.
function checkPluginPack(pack, bytes = 0, reg = registry()) {
  validatePack(pack, {builtin: true, plugin: true, bytes});
  if (reg.packs.has(pack.profile_id)) throw fail('RULEPACK_INVALID', `duplicate profile ${pack.profile_id}`);
  if (reg.hasPlugin(pack.profile_id)) throw fail('RULEPACK_INVALID', `duplicate plugin profile ${pack.profile_id}`);
  reg.assertPluginClaims(pack);
  return pack;
}
function findPackDir() {
  const candidates = [process.env.PIME_LANGUAGE_PACKS, path.join(__dirname, '..', '..', 'language-packs'), path.join(__dirname, '..', '..', '..', '..', 'language-packs')];
  for (const c of candidates) if (c && fs.existsSync(path.join(c, 'rust.json'))) return c;
  throw fail('RULEPACK_INVALID', 'language-packs directory not found');
}
// The shipped plugin packs live next to the built-in ones (both are in the extension root of the .vsix).
function findPluginDir(packDir = findPackDir()) {
  if (process.env.PIME_LANGUAGE_PLUGINS) return process.env.PIME_LANGUAGE_PLUGINS;
  return path.join(path.dirname(packDir), 'language-plugins');
}
// loadRegistry(dir) keeps working; the options add the plugin directories and the enabled set.
function loadRegistry(dir = findPackDir(), options = {}) {
  const {pluginDirs = [findPluginDir(dir)], enabled = DEFAULT_PLUGIN_LANGUAGES} = options;
  const reg = new Registry();
  for (const id of LANGUAGES) {
    const file = path.join(dir, `${id}.json`);
    const raw = fs.readFileSync(file, 'utf8');
    let pack;
    try { pack = JSON.parse(raw); } catch { throw fail('RULEPACK_INVALID', `${id}.json is not valid JSON`); }
    if (pack.profile_id !== id) throw fail('RULEPACK_INVALID', `${id}.json declares profile ${pack.profile_id}`);
    reg.add(pack, Buffer.byteLength(raw));
  }
  reg.setEnabledPlugins(enabled);
  for (const pdir of pluginDirs) {
    if (!pdir || !fs.existsSync(pdir)) continue; // no plugin packs: the built-in seven still work
    for (const name of fs.readdirSync(pdir).filter(n => n.endsWith('.json')).sort()) {
      const raw = fs.readFileSync(path.join(pdir, name), 'utf8');
      try {
        let pack; try { pack = JSON.parse(raw); } catch { throw fail('RULEPACK_INVALID', 'not valid JSON'); }
        reg.addPlugin(pack, Buffer.byteLength(raw));
      } catch (e) { reg.pluginErrors.push({name, code: e.code || 'RULEPACK_INVALID', message: e.message}); }
    }
  }
  return reg;
}
let defaultRegistry = null;
const registry = () => defaultRegistry || (defaultRegistry = loadRegistry());
function setRegistry(reg) { defaultRegistry = reg; validators.setFamilies(reg.families()); }

// ---------------------------------------------------------------- dialects (FR-012)
function defaultDialect(profile, reg = registry()) {
  const out = {};
  for (const [axis, def] of Object.entries(reg.get(profile)?.dialects || {})) out[axis] = def.default;
  return out;
}
function resolveDialect(profile, requested = {}, reg = registry()) {
  const values = {}, unsupported = [];
  for (const [axis, def] of Object.entries(reg.get(profile)?.dialects || {})) {
    const want = requested[axis];
    if (want === undefined || want === null || want === '') values[axis] = def.default;
    else if (def.values.includes(want)) values[axis] = want;
    else { values[axis] = def.default; unsupported.push({axis, value: want}); }
  }
  return {values, unsupported};
}
function dialectMatches(when, dialect) {
  if (!when) return true;
  return Object.entries(when).every(([axis, allowed]) => allowed.includes(dialect[axis]));
}

// ---------------------------------------------------------------- language resolution (FR-010/011, DD §5.3)
const STRONG_EXT = {'.rs': 'rust', '.py': 'python', '.pyw': 'python', '.ts': 'typescript', '.tsx': 'typescript', '.mts': 'typescript', '.cts': 'typescript',
  '.ps1': 'powershell', '.psm1': 'powershell', '.psd1': 'powershell', '.c': 'c'};
const AMBIGUOUS_EXT = {'.h': 'c', '.asm': 'assembly', '.s': 'assembly', '.nasm': 'assembly', '.bin': 'machine-code', '.hex': 'machine-code'};
const LEXICAL = [
  ['rust', ['fn main()', 'impl ', 'let mut ', 'println!', 'use std::']],
  ['python', ['def ', 'import ', 'self.', 'elif ', '__name__']],
  ['typescript', ['interface ', 'export const ', ': string', 'Promise<', '=> {']],
  ['powershell', ['Get-', 'Write-Host', 'Where-Object', '$PSVersionTable', 'param(']],
  ['c', ['#include', 'printf(', 'typedef struct', 'int main(']],
  ['assembly', ['section .text', 'mov rax', 'global _start', '[rbp', 'push rbp']]
];
const extOf = name => { const m = /(\.[^.\\/]+)$/.exec(name || ''); return m ? m[1].toLowerCase() : ''; };
function lexicalGuess(text) {
  if (typeof text !== 'string' || !text) return null;
  const sample = text.slice(0, 20000), scored = [];
  for (const [lang, tokens] of LEXICAL) scored.push([lang, tokens.filter(t => sample.includes(t)).length]);
  scored.sort((a, b) => b[1] - a[1]);
  // one stray `if`/`for` proves nothing: need two independent hints and no competing language
  return scored[0][1] >= 2 && scored[1][1] === 0 ? scored[0][0] : null;
}
// ctx: {languageId?, fileName?, text?}  current: language kept when nothing reliable is known
function resolveLanguage(ctx, current = 'unknown', lock = null, reg = registry()) {
  const ext = extOf(ctx.fileName), hints = {};
  if (ctx.languageId === 'typescriptreact' || ext === '.tsx') hints.mode = 'tsx';
  else if (ctx.languageId === 'typescript' || ext === '.ts') hints.mode = 'ts';
  if (lock && reg.hasProfile(lock)) return {language: lock, dialectHints: lock === 'typescript' ? hints : {}, confidence: 100, reason: 'manual-lock', changed: lock !== current, locked: true};
  const byId = reg.byLanguageId.get(ctx.languageId);
  if (byId) return {language: byId, dialectHints: byId === 'typescript' ? hints : {}, confidence: 95, reason: 'bridge-languageId', changed: byId !== current};
  if (STRONG_EXT[ext]) return {language: STRONG_EXT[ext], dialectHints: STRONG_EXT[ext] === 'typescript' ? hints : {}, confidence: 85, reason: 'extension', changed: STRONG_EXT[ext] !== current};
  const out = {language: current, dialectHints: {}, confidence: 0, reason: 'kept', changed: false};
  // the registry decides for every other extension: one active owner is a strong hint, several are ambiguous
  const owners = reg.extensionOwners(ext);
  if (owners.length === 1) return {language: owners[0], dialectHints: {}, confidence: 85, reason: 'extension', changed: owners[0] !== current};
  if (owners.length > 1) { out.reason = 'ambiguous-extension'; out.uncertain = true; out.suggestion = owners[0]; }
  else if (AMBIGUOUS_EXT[ext] || !ext) {
    out.reason = 'ambiguous-extension'; out.uncertain = true;
    if (AMBIGUOUS_EXT[ext]) out.suggestion = AMBIGUOUS_EXT[ext];
  }
  const guess = lexicalGuess(ctx.text);
  if (guess && guess !== current) { out.suggestion = out.suggestion || guess; out.reason = out.reason === 'kept' ? 'lexical-suggestion' : out.reason; out.uncertain = true; }
  return out; // never switches automatically on weak evidence
}

// ---------------------------------------------------------------- context analysis (FR-025/030)
const BRACE_HEADERS = {rust: [['fn', /\bfn\b/], ['impl', /\bimpl\b/], ['trait', /\btrait\b/], ['mod', /\bmod\b/]], c: [['fn', /\)\s*$/]], typescript: [['class', /\bclass\b/], ['interface', /\binterface\b/], ['fn', /=>\s*$|\)\s*(?::\s*[\w<>\[\] |]+)?\s*$/]]};
// The mask style of a profile: the built-in ones have their own, a plugin pack is masked like its validator family.
function maskStyleFor(profile, reg = registry()) {
  const direct = MASK_STYLE[profile];
  if (direct) return direct;
  const pack = reg.get(profile);
  if (pack && pack.validator === 'sql') return 'sql';
  return null;
}
function classifyContext(profile, before, reg = registry()) {
  if (profile === 'machine-code') return 'unknown';
  const tail = before.slice(-6000);
  const style = maskStyleFor(profile, reg);
  if (style) { const m = maskSource(tail + 'Q', style); if (m[m.length - 1] === ' ') return 'literal'; }
  if (profile === 'assembly' && /[;#]/.test(tail.slice(tail.lastIndexOf('\n') + 1))) return 'literal';
  const masked = style ? maskSource(tail, style) : tail;
  const lines = masked.split(/\r\n|\r|\n/);
  const cur = lines[lines.length - 1];
  const prev = [...lines.slice(0, -1)].reverse().find(l => l.trim() !== '');
  const probe = (cur.trim() === '' ? prev : cur);
  if (probe === undefined) return 'statement';
  const t = probe.trimEnd();
  if (profile === 'assembly') return 'statement';
  if (profile === 'python') {
    let depth = 0; for (const c of masked) { if ('([{'.includes(c)) depth++; else if (')]}'.includes(c)) depth = Math.max(0, depth - 1); }
    if (depth > 0) return 'expression';
    if (cur.trim() === '') return t.endsWith('\\') ? 'expression' : 'statement';
    return /(?:[=(,[{+\-*/%<>!&|^~]|\b(?:return|yield|await|in|and|or|not|is|if|elif|while|lambda))$/.test(t) ? 'expression' : 'unknown';
  }
  const generic = profile === 'rust' || profile === 'typescript';
  if (style === 'sql') {
    // A SQL statement is line oriented, so the statement position is the normal case. An expression position is a
    // line that clearly goes on (an operator, a comparison or an open bracket).
    if (/[,(=<>+*/-]$/.test(t)) return 'expression';
    return 'statement';
  }
  if (/(?:->|\bas|\bextends|\bimplements|(?<!:):)$/.test(t) || (generic && /<$/.test(t))) return cur.trim() === '' ? 'unknown' : 'type';
  if (/(?:=>|[=(,[+\-*/%>!&|^~?]|\breturn)$/.test(t)) return 'expression';
  if (/[;{}]$/.test(t) || t === '') return 'statement';
  // TypeScript and PowerShell statements need no terminator: a new line that does not continue an expression starts a statement
  return (profile === 'typescript' || profile === 'powershell') && cur.trim() === '' ? 'statement' : 'unknown';
}
function enclosingBlock(profile, before, reg = registry()) {
  const style = maskStyleFor(profile, reg);
  const tail = before.slice(-12000);
  if (profile === 'python') {
    const lines = tail.split(/\r\n|\r|\n/);
    const curIndent = /^[ \t]*/.exec(lines[lines.length - 1])[0].length;
    let want = curIndent;
    for (let i = lines.length - 2; i >= 0 && want > 0; i--) {
      if (lines[i].trim() === '' || /^\s*#/.test(lines[i])) continue;
      const indent = /^[ \t]*/.exec(lines[i])[0].length;
      if (indent < want) {
        if (/^\s*class\b/.test(lines[i])) return 'class';
        if (/^\s*(?:async\s+)?def\b/.test(lines[i])) return 'def';
        want = indent;
      }
    }
    return 'module';
  }
  if (!style || !BRACE_HEADERS[profile]) return 'module';
  const masked = maskSource(tail, style);
  const stack = [];
  for (let i = 0; i < masked.length; i++) {
    if (masked[i] === '{') stack.push(i);
    else if (masked[i] === '}') stack.pop();
  }
  if (!stack.length) return 'module';
  const open = stack[stack.length - 1];
  const headerStart = Math.max(masked.lastIndexOf(';', open), masked.lastIndexOf('}', open), masked.lastIndexOf('{', open - 1)) + 1;
  const header = masked.slice(headerStart, open);
  for (const [kind, re] of BRACE_HEADERS[profile]) if (re.test(header)) return kind;
  return 'block';
}
function nearbyIdentifiers(profile, text, key, max = 3, reg = registry()) {
  const kw = new Set(reg.keywords(profile).map(k => k.toLowerCase()));
  const counts = new Map(), lower = key.toLowerCase();
  const re = /[A-Za-z_$][\w$]*/g;
  let m;
  const sample = text.slice(-4000);
  while ((m = re.exec(sample))) {
    const word = m[0];
    if (word.length <= key.length || kw.has(word.toLowerCase()) || !word.toLowerCase().startsWith(lower)) continue;
    counts.set(word, (counts.get(word) || 0) + 1);
  }
  return [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).slice(0, max).map(([w]) => w);
}

// ---------------------------------------------------------------- rendering helpers (FR-008, UT-010/018)
function placeholdersOf(text) {
  const seen = []; let m; const re = new RegExp(PLACEHOLDER.source, 'g');
  while ((m = re.exec(text))) if (m[1] !== 'cursor' && !seen.includes(m[1])) seen.push(m[1]);
  return seen;
}
const renderPlain = (text, cursorText = '') => text.replace(PLACEHOLDER, (_, name) => name === 'cursor' ? cursorText : name);
function toSnippet(text) {
  const order = placeholdersOf(text), esc = s => s.replace(/[\\$]/g, '\\$&');
  let out = '', last = 0, m; const seenNames = new Set();
  const re = new RegExp(PLACEHOLDER.source, 'g');
  while ((m = re.exec(text))) {
    out += esc(text.slice(last, m.index)); last = m.index + m[0].length;
    if (m[1] === 'cursor') out += '$0';
    else if (seenNames.has(m[1])) out += `$${order.indexOf(m[1]) + 1}`;
    else { seenNames.add(m[1]); out += `\${${order.indexOf(m[1]) + 1}:${m[1]}}`; }
  }
  return out + esc(text.slice(last));
}
// Plain-text fallback that follows the host's indentation and EOL (no tab/space mixing).
function renderForHost(profile, text, host = {}) {
  const {tabSize = 4, insertSpaces = true, eol = '\n', baseIndent = ''} = host;
  const unit = profile === 'typescript' ? 2 : 4;
  const indentUnit = insertSpaces ? ' '.repeat(tabSize) : '\t';
  const lines = renderPlain(text).split('\n').map((line, i) => {
    const lead = /^ */.exec(line)[0].length;
    const body = line.slice(lead);
    const level = Math.floor(lead / unit);
    return (i === 0 ? '' : baseIndent) + indentUnit.repeat(level) + ' '.repeat(lead % unit) + body;
  });
  return lines.join(eol);
}

// ---------------------------------------------------------------- candidates (FR-006, UT-006)
function makeCandidate(fields) {
  const c = {kind: 'syntax', source: 'rule', validation: 'unchecked', assumptions: [], score: 0, ...fields};
  if (!KINDS.includes(c.kind)) throw fail('OUTPUT_INVALID', `invalid candidate kind: ${c.kind}`);
  if (!SOURCES.includes(c.source)) throw fail('OUTPUT_INVALID', `invalid candidate source: ${c.source}`);
  if (!VALIDATIONS.includes(c.validation)) throw fail('OUTPUT_INVALID', `invalid candidate validation: ${c.validation}`);
  return c;
}
const candidateToJSON = c => JSON.stringify(c);
function candidateFromJSON(json) {
  let o; try { o = typeof json === 'string' ? JSON.parse(json) : json; } catch { throw fail('OUTPUT_INVALID', 'candidate is not JSON'); }
  if (!o || typeof o !== 'object' || typeof o.id !== 'string') throw fail('OUTPUT_INVALID', 'candidate shape');
  return makeCandidate(o);
}
const MATCH_RANK = {exact: 0, prefix: 1, keyword: 2, fuzzy: 3, context: 4};
function matchPattern(pattern, key, label) {
  const p = pattern.toLowerCase();
  if (p === key) return 'exact';
  if (p.startsWith(key)) return 'prefix';
  if (key.length >= 2 && (label || '').toLowerCase().split(/[^a-z0-9]+/).some(w => w.length > 0 && w.startsWith(key))) return 'keyword';
  if (key.length >= 2 && key.length < p.length && key[0] === p[0]) {
    let i = 0; for (const ch of p) if (ch === key[i]) i++;
    if (i === key.length) return 'fuzzy';
  }
  return null;
}
const validationCache = new Map();
function validateTemplate(profile, text, dialect, family) {
  const cacheKey = `${profile}|${JSON.stringify(dialect)}|${text}`;
  let hit = validationCache.get(cacheKey);
  if (!hit) {
    const body = profile === 'python' ? 'pass' : '';
    const r = validateSource(profile, renderPlain(text, body), dialect, family ? {family} : undefined);
    hit = r.ok ? 'valid' : 'invalid';
    if (validationCache.size > 5000) validationCache.clear();
    validationCache.set(cacheKey, hit);
  }
  return hit;
}
const fingerprint = (profile, text) => `${profile}|${renderPlain(text).trim().toLowerCase().replace(/\s+/g, ' ')}`;

// Candidate ordering (DD §4.2): pin > explicit user rule > syntax-valid > match quality > frequency > weight > AI > id.
function rankCandidates(list, {pins = new Set(), usage = new Map()} = {}) {
  const valid = c => c.validation === 'valid' || c.validation === 'verified' ? 2 : c.validation === 'unchecked' ? 1 : 0;
  return [...list].sort((a, b) =>
    (pins.has(b.id) - pins.has(a.id)) ||
    ((b.source === 'user_rule') - (a.source === 'user_rule')) ||
    (valid(b) - valid(a)) ||
    ((MATCH_RANK[a.match] ?? 9) - (MATCH_RANK[b.match] ?? 9)) ||
    ((usage.get(b.id) || 0) - (usage.get(a.id) || 0)) ||
    ((b.weight || 0) - (a.weight || 0)) ||
    ((b.ai_score || 0) - (a.ai_score || 0)) ||
    (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}
// One row per normalised content; every contributing source stays visible in `sources`.
function dedupe(list, profile) {
  const seen = new Map(), out = [];
  for (const c of list) {
    if (c.text == null) { out.push(c); continue; }
    const key = fingerprint(profile || c.profile_id, c.text);
    const hit = seen.get(key);
    if (hit) { for (const s of c.sources || [c.source]) if (!hit.sources.includes(s)) hit.sources.push(s); continue; }
    c.sources = [...(c.sources || [c.source])];
    seen.set(key, c); out.push(c);
  }
  return out;
}
const MAX_PREFIX = 128;
function extractPrefix(profile, linePrefix) {
  if (profile === 'machine-code') return linePrefix.replace(/^\s+/, '').slice(-60);
  const re = profile === 'powershell' ? /[A-Za-z_][\w-]{0,63}$/ : profile === 'c' ? /#?[A-Za-z_]\w{0,63}$/ : /[A-Za-z_$][\w$]{0,63}$/;
  return (re.exec(linePrefix) || [''])[0];
}

function predictMachine(prefix, dialect, limit) {
  const key = x86.normalize(prefix);
  if (!key) return [];
  const fmt = dialect.format === 'compact' ? h => h.replace(/ /g, '') : h => h;
  const out = [];
  for (const e of x86.listEncodable()) {
    if (!e.asm.startsWith(key)) continue;
    out.push(makeCandidate({id: 'mc.' + e.asm, kind: 'byte_hex', source: 'rule', label: `${e.asm}  →  ${e.hex}`, text: fmt(e.hex), explain: 'x86-64 long mode; verified against the encoding table; text only, never executed',
      validation: 'verified', profile_id: 'machine-code', match: e.asm === key ? 'exact' : 'prefix', weight: 100 - Math.min(90, e.asm.length), assumptions: ['x86-64 long mode']}));
    if (out.length >= 60) break;
  }
  if (!out.length && !/^(?:[0-9a-f]{2}\s*)+$/i.test(key)) {
    const r = x86.encode(key);
    if (!r.ok) out.push(makeCandidate({id: 'mc.unresolved:' + key, kind: 'explain', source: 'rule', label: `${key}: ${r.code}`, text: null, explain: r.reason, validation: 'context_missing', profile_id: 'machine-code', match: 'exact', assumptions: [r.reason]}));
  }
  const hexKey = /^(?:[0-9a-f]{2}\s*)+$/i.test(key) ? x86.disassembleKnown(key) : null;
  if (hexKey) out.push(makeCandidate({id: 'mc.disasm:' + key, kind: 'explain', source: 'rule', label: `${key.toUpperCase()}  =  ${hexKey}`, text: null, explain: 'Known-form disassembly preview', validation: 'verified', profile_id: 'machine-code', match: 'exact'}));
  return out.slice(0, limit);
}

// predict({profile, prefix, before?, after?, dialect?, limit?, pins?, usage?}) -> ranked candidates (UT-037, FR-025/026)
// The profile may be a built-in one or an ACTIVE plugin profile; a plugin that is off predicts nothing.
function predict(opts, reg = registry()) {
  const {profile, prefix, before = '', after = '', limit = 6, pins, usage} = opts;
  const key = typeof prefix === 'string' ? prefix.trim().toLowerCase() : '';
  if (!key || key.length > MAX_PREFIX || !reg.hasProfile(profile)) return [];
  const {values: dialect, unsupported} = resolveDialect(profile, opts.dialect, reg);
  if (unsupported.length) { // unknown ISA/dialect: say so instead of guessing bytes or syntax
    const bad = unsupported[0];
    return [makeCandidate({id: `${profile}.dialect`, kind: 'explain', source: 'rule', label: `${bad.axis} "${bad.value}" is not supported`, text: null, explain: 'Pick a supported value (for example isa x86-64) to get candidates', validation: 'context_missing', profile_id: profile, match: 'exact', assumptions: ['dialect context missing']})];
  }
  if (profile === 'machine-code') return rankCandidates(predictMachine(prefix, dialect, limit * 2), {pins, usage}).slice(0, limit);
  const family = reg.familyOf(profile); // the validator family of the pack (the SQL family for cypher / plsql / tsql)
  const ctx = opts.context || classifyContext(profile, before, reg);
  if (ctx === 'literal') return []; // inside a string or comment nothing is proposed
  const block = enclosingBlock(profile, before, reg);
  const out = [];
  for (const t of reg.triggers(profile)) {
    if (!dialectMatches(t.when, dialect)) continue;
    if (t.contexts && !t.contexts.includes(ctx)) continue;
    if (t.in && !t.in.includes(block)) continue;
    if (t.not_in && t.not_in.includes(block)) continue;
    for (let i = 0; i < t.candidates.length; i++) {
      const c = t.candidates[i];
      if (!dialectMatches(c.when, dialect)) continue;
      const match = matchPattern(t.pattern, key, c.label);
      if (!match) continue;
      let text = c.text; const assumptions = [...(c.assumptions || [])];
      if (c.stmt_semicolon && ctx !== 'statement' && text.endsWith(';')) {
        text = text.slice(0, -1);
        assumptions.push(ctx === 'unknown' ? 'No semicolon added: statement position not provable' : 'No semicolon: expression position');
      }
      out.push(makeCandidate({id: `${t.id}#${i}`, kind: t.kind || 'syntax', source: t.user ? 'user_rule' : 'rule', profile_id: profile, label: c.label, text, explain: c.explain || '',
        match, weight: t.weight ?? 50, assumptions, validation: validateTemplate(profile, text, dialect, family), context: ctx, block}));
    }
  }
  for (const kw of reg.keywords(profile)) {
    const lower = kw.toLowerCase();
    if (!lower.startsWith(key)) continue;
    out.push(makeCandidate({id: `${profile}.kw.${kw}`, kind: 'keyword', source: 'rule', profile_id: profile, label: kw, text: kw, match: lower === key ? 'exact' : 'prefix', weight: 20, validation: 'valid', context: ctx}));
  }
  if (before || after) {
    for (const [i, word] of nearbyIdentifiers(profile, before + after, prefix.trim(), 3, reg).entries())
      out.push(makeCandidate({id: `${profile}.id.${word}`, kind: 'identifier', source: 'context', profile_id: profile, label: word, text: word, explain: 'Seen in nearby code', match: 'context', weight: 30 - i, validation: 'unchecked', assumptions: ['Only nearby text was read; no project symbol table']}));
  }
  const kept = out.filter(c => c.validation !== 'invalid');
  return dedupe(rankCandidates(kept, {pins, usage}), profile).slice(0, limit);
}

module.exports = {
  LANGUAGES, PLUGIN_LANGUAGES, DEFAULT_PLUGIN_LANGUAGES, KINDS, SOURCES, VALIDATIONS, CONTEXTS, LIMITS, Registry, validatePack, checkPluginPack, loadRegistry, registry, setRegistry, findPackDir, findPluginDir,
  defaultDialect, resolveDialect, resolveLanguage, extOf, classifyContext, enclosingBlock, nearbyIdentifiers, extractPrefix, maskStyleFor,
  placeholdersOf, renderPlain, toSnippet, renderForHost, makeCandidate, candidateToJSON, candidateFromJSON, rankCandidates, dedupe, fingerprint, predict
};
