'use strict';
// Plugin languages: a plugin pack adds a whole new language profile (its own rules, dialects and validator).
// The built-in seven are always on; a plugin profile is on or off and the set lives in programmingIme.pluginLanguages.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {core} = require('./helpers');
const L = core('languages');
const V = core('validators');
const S = core('settings');
const {InputSession} = core('scheduler');

const root = path.join(__dirname, '..', '..', 'editor-bridges', 'vscode');
const ids = list => list.map(c => c.id);
const packFile = (name, pack) => ({name, text: JSON.stringify(pack)});
// a plugin pack that adds a language of its own (the SQL family, like the shipped ones)
const mysqlPack = over => ({
  schema_version: 1, profile_id: 'mysql', version: '1.0.0', display_name: 'MySQL',
  language_ids: ['mysql'], accepted_extensions: ['.mysql'], validator: 'sql',
  dialects: {version: {values: ['8'], default: '8'}}, keywords: ['select', 'insert'],
  triggers: [{id: 'mysql.select', pattern: 'select', kind: 'syntax', weight: 80, contexts: ['statement'], candidates: [{label: 'SELECT ... FROM', text: 'SELECT ${columns}\nFROM ${table};'}]}],
  ...over
});

test('UT-PLG-01 the three shipped plugin packs load next to the seven built-in packs', () => {
  const reg = L.loadRegistry();
  assert.deepEqual([...reg.packs.keys()].sort(), [...L.LANGUAGES].sort(), 'the built-in seven stay seven');
  assert.deepEqual(reg.pluginProfiles(), ['cypher', 'plsql', 'tsql'], 'every discovered plugin pack is listed, on or off');
  assert.deepEqual(reg.enabledPluginProfiles(), ['cypher', 'plsql'], 'T-SQL is off and PL/SQL is on out of the box');
  assert.deepEqual([...L.PLUGIN_LANGUAGES], ['cypher', 'plsql', 'tsql']);
  assert.deepEqual([...L.DEFAULT_PLUGIN_LANGUAGES], ['cypher', 'plsql']);
  assert.deepEqual(reg.pluginErrors, [], 'the shipped packs are valid');
});

test('UT-PLG-02 a plugin profile predicts with its own rules and is validated with its own family', () => {
  const reg = L.loadRegistry();
  const out = L.predict({profile: 'cypher', prefix: 'match', before: ''}, reg);
  assert.ok(out.length >= 1, 'cypher produces candidates');
  assert.ok(ids(out).includes('cypher.match#0'));
  assert.ok(out.every(c => c.profile_id === 'cypher'));
  assert.ok(out.some(c => c.validation === 'valid'), 'the SQL family validates the candidates');
  const plsql = L.predict({profile: 'plsql', prefix: 'select', before: ''}, reg);
  assert.ok(ids(plsql).includes('plsql.select#0'));
  // the trigger only fires in a statement position, not inside a string
  assert.deepEqual(L.predict({profile: 'plsql', prefix: 'select', before: "WHERE a = 'x"}, reg), []);
});

test('UT-PLG-03 a disabled plugin profile predicts nothing and cannot be locked', () => {
  const reg = L.loadRegistry(undefined, {enabled: ['cypher']});
  assert.deepEqual(L.predict({profile: 'tsql', prefix: 'select', before: ''}, reg), [], 'a disabled plugin predicts nothing');
  assert.equal(reg.hasProfile('tsql'), false);
  assert.equal(reg.hasPlugin('tsql'), true, 'it stays discoverable');
  assert.deepEqual(reg.triggers('tsql'), [], 'a disabled plugin contributes no trigger');
  assert.deepEqual(reg.keywords('tsql'), []);
  assert.deepEqual(reg.byLanguageId.get('tsql'), undefined, 'no language id');
  assert.deepEqual(reg.extensionOwners('.tsql'), [], 'no extension');
  const locked = L.resolveLanguage({languageId: 'plaintext', fileName: 'x.tsql'}, 'unknown', 'tsql', reg);
  assert.notEqual(locked.reason, 'manual-lock', 'a disabled profile cannot be locked');
  const session = new InputSession({language: 'unknown'});
  assert.equal(session.lock('tsql', 'file', 'f1'), false, 'the scheduler refuses a disabled profile');
  assert.equal(session.lock('cypher', 'file', 'f1'), true, 'an enabled plugin profile can be locked');
});

test('UT-PLG-04 the enabled set decides what is active', () => {
  const off = L.loadRegistry(undefined, {enabled: []});
  assert.deepEqual(off.enabledPluginProfiles(), []);
  assert.equal(off.hasProfile('cypher'), false);
  assert.equal(off.hasProfile('rust'), true, 'the built-in profiles are always active');
  assert.deepEqual(off.extensionOwners('.sql'), []);
  assert.deepEqual(L.predict({profile: 'plsql', prefix: 'select', before: ''}, off), []);
  const on = L.loadRegistry(undefined, {enabled: ['cypher', 'plsql', 'tsql']});
  assert.deepEqual(on.enabledPluginProfiles(), ['cypher', 'plsql', 'tsql']);
  assert.equal(on.hasProfile('tsql'), true);
  // the same registry can be switched at runtime
  const reg = L.loadRegistry();
  reg.disablePlugin('plsql');
  assert.equal(reg.hasProfile('plsql'), false);
  assert.deepEqual(reg.extensionOwners('.sql'), [], 'a disabled plugin owns no extension');
  reg.enablePlugin('plsql');
  assert.equal(reg.hasProfile('plsql'), true);
  assert.deepEqual(reg.extensionOwners('.sql'), ['plsql']);
});

test('UT-PLG-05 .sql belongs to PL/SQL by default and becomes ambiguous when T-SQL is on as well', () => {
  const reg = L.loadRegistry();
  const sql = L.resolveLanguage({languageId: 'plaintext', fileName: 'x.sql'}, 'unknown', null, reg);
  assert.equal(sql.language, 'plsql');
  assert.equal(sql.reason, 'extension');
  assert.equal(sql.confidence, 85);
  assert.equal(sql.uncertain, undefined);
  const cypher = L.resolveLanguage({languageId: 'plaintext', fileName: 'x.cypher'}, 'unknown', null, reg);
  assert.equal(cypher.language, 'cypher');
  assert.equal(cypher.reason, 'extension');
  assert.equal(L.resolveLanguage({languageId: 'plaintext', fileName: 'x.cql'}, 'unknown', null, reg).language, 'cypher');
  assert.equal(L.resolveLanguage({languageId: 'plaintext', fileName: 'x.tsql'}, 'unknown', null, reg).language, 'unknown', 'T-SQL is off');
  const both = L.loadRegistry(undefined, {enabled: ['cypher', 'plsql', 'tsql']});
  const ambiguous = L.resolveLanguage({languageId: 'plaintext', fileName: 'x.sql'}, 'unknown', null, both);
  assert.equal(ambiguous.language, 'unknown', 'the current language is kept');
  assert.equal(ambiguous.uncertain, true);
  assert.equal(ambiguous.suggestion, 'plsql', 'the first owner is suggested');
  assert.equal(ambiguous.reason, 'ambiguous-extension');
  assert.deepEqual(both.extensionOwners('.sql'), ['plsql', 'tsql'], 'the extension maps to the list of active owners');
});

test('UT-PLG-06 a user-imported plugin pack adds a whole new language', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pime-plugins-'));
  try {
    const files = [packFile('mysql.json', mysqlPack())];
    const built = S.buildRegistryWithPlugins(files, {enabled: ['mysql'], ruleFiles: []});
    assert.deepEqual(built.accepted, ['mysql.json']);
    assert.deepEqual(built.rejected, []);
    const reg = built.registry;
    assert.equal(reg.hasProfile('mysql'), true);
    assert.deepEqual(reg.byLanguageId.get('mysql'), 'mysql');
    assert.deepEqual(reg.extensionOwners('.mysql'), ['mysql']);
    const out = L.predict({profile: 'mysql', prefix: 'select', before: ''}, reg);
    assert.ok(ids(out).includes('mysql.select#0'), 'the imported pack brings its own rules');
    const resolved = L.resolveLanguage({languageId: 'plaintext', fileName: 'x.mysql'}, 'unknown', null, reg);
    assert.equal(resolved.language, 'mysql');
    // the family of the pack decides the validator, not the profile id
    assert.equal(V.validateSource('mysql', 'SELECT 1;', {version: '8'}, {family: 'sql'}).level, 'valid');
  } finally { fs.rmSync(dir, {recursive: true, force: true}); }
});

test('UT-PLG-07 a plugin pack that claims a built-in language id or extension is rejected', () => {
  const reg = L.loadRegistry();
  // the profile id of a plugin is its own: it may not be a built-in one
  assert.throws(() => L.validatePack(mysqlPack({profile_id: 'rust'}), {builtin: true, plugin: true}), e => e.code === 'RULEPACK_INVALID');
  assert.throws(() => L.validatePack(mysqlPack({profile_id: 'MySQL'}), {builtin: true, plugin: true}), e => e.code === 'RULEPACK_INVALID');
  // a plugin may not claim a language id or an extension of a built-in pack
  assert.throws(() => L.checkPluginPack(mysqlPack({language_ids: ['rust']}), 0, reg), e => e.code === 'RULEPACK_INVALID');
  assert.throws(() => L.checkPluginPack(mysqlPack({accepted_extensions: ['.rs']}), 0, reg), e => e.code === 'RULEPACK_INVALID');
  assert.throws(() => reg.addPlugin(mysqlPack({language_ids: ['python']})), e => e.code === 'RULEPACK_INVALID');
  assert.throws(() => reg.addPlugin(mysqlPack({accepted_extensions: ['.py']})), e => e.code === 'RULEPACK_INVALID');
  assert.doesNotThrow(() => L.checkPluginPack(mysqlPack(), 0, reg));
  assert.equal(reg.hasPlugin('mysql'), false, 'the rejected pack was not loaded');
  // the built-in extensions stay in the hardcoded tables: byExt holds the plugin owners only
  assert.deepEqual(reg.extensionOwners('.rs'), []);
  assert.deepEqual(reg.extensionOwners('.py'), []);
  assert.equal(L.resolveLanguage({languageId: 'plaintext', fileName: 'x.rs'}, 'unknown', null, reg).language, 'rust');
});

test('UT-PLG-08 a bad plugin pack is reported and skipped; the previous registry is not degraded', () => {
  const good = packFile('mysql.json', mysqlPack());
  const bad = {name: 'broken.json', text: '{not json'};
  const built = S.buildRegistryWithPlugins([good, bad], {enabled: ['mysql'], ruleFiles: []});
  assert.deepEqual(built.accepted, ['mysql.json']);
  assert.equal(built.rejected.length, 1);
  assert.equal(built.rejected[0].name, 'broken.json');
  assert.equal(built.registry.hasProfile('mysql'), true, 'the good pack is loaded');
  assert.equal(built.registry.hasProfile('rust'), true, 'the built-in packs are untouched');
});

test('UT-PLG-09 the SQL validator reports the dialect codes and validates well-formed SQL', () => {
  const sql = (profile, text, dialect) => V.validateSource(profile, text, dialect, {family: 'sql'});
  const codes = r => r.issues.map(i => i.code);
  assert.deepEqual(codes(sql('cypher', 'MATCH (n)\nWHERE EXISTS { (n) }', {version: '4'})), ['CYPHER5_EXISTS_SUBQUERY']);
  assert.deepEqual(codes(sql('cypher', 'MATCH (n) IN TRANSACTIONS', {version: '4'})), ['CYPHER5_IN_TRANSACTIONS']);
  assert.deepEqual(codes(sql('cypher', 'MATCH (n)\nWHERE EXISTS { (n) }', {version: '5'})), []);
  assert.deepEqual(codes(sql('plsql', 'SELECT LISTAGG(a) FROM t', {version: '12c'})), ['PLSQL19C_LISTAGG']);
  assert.deepEqual(codes(sql('plsql', 'SELECT LISTAGG(a) FROM t', {version: '19c'})), []);
  assert.deepEqual(codes(sql('tsql', 'CREATE OR ALTER VIEW v AS SELECT 1', {version: '2012'})), ['TSQL2016_CREATE_OR_ALTER']);
  assert.deepEqual(codes(sql('tsql', 'CREATE OR ALTER VIEW v AS SELECT 1', {version: '2016'})), []);
  assert.deepEqual(codes(sql('tsql', 'SELECT STRING_AGG(a) FROM t', {version: '2016'})), ['TSQL2017_STRING_AGG']);
  assert.deepEqual(codes(sql('tsql', 'SELECT STRING_AGG(a) FROM t', {version: '2017'})), []);
  // an unterminated string is reported with the line of the opening quote
  const open = sql('plsql', "SELECT 1\nWHERE a = 'x", {version: '21c'});
  assert.deepEqual(codes(open), ['SQL_UNTERMINATED_STRING']);
  assert.equal(open.issues[0].line, 2);
  assert.equal(open.ok, false);
  assert.equal(open.level, 'invalid');
  // the bracket issues are the ones of the other validators
  assert.deepEqual(codes(sql('plsql', 'SELECT (1', {version: '21c'})), ['BRACKET_UNCLOSED']);
  assert.deepEqual(codes(sql('plsql', 'SELECT 1)', {version: '21c'})), ['BRACKET_UNEXPECTED']);
  assert.deepEqual(codes(sql('plsql', 'SELECT (1]', {version: '21c'})), ['BRACKET_MISMATCH']);
  // well-formed SQL of every dialect is valid
  for (const [profile, text, dialect] of [['cypher', 'MATCH (n) RETURN n;', {version: '5'}], ['plsql', 'SELECT a FROM t WHERE b = 1;', {version: '21c'}], ['tsql', 'SELECT a FROM t;', {version: '2022'}]]) {
    const r = sql(profile, text, dialect);
    assert.equal(r.ok, true, `${profile}: ${JSON.stringify(r.issues)}`);
    assert.equal(r.level, 'valid');
  }
  // a comment and a string are masked: their content is not a dialect issue
  assert.deepEqual(codes(sql('plsql', '-- LISTAGG\nSELECT 1;', {version: '12c'})), []);
  assert.deepEqual(codes(sql('plsql', '/* LISTAGG */ SELECT 1;', {version: '12c'})), []);
});

test('UT-PLG-10 the validator family comes from the pack, an unknown family has no validator', () => {
  assert.deepEqual(Object.keys(V.VALIDATOR_FAMILIES), ['sql']);
  const unknown = V.validateSource('mysql', 'SELECT 1;', {version: '8'}, {family: 'c-like'});
  assert.equal(unknown.ok, false);
  assert.equal(unknown.level, 'unchecked');
  assert.equal(unknown.issues[0].code, 'NO_VALIDATOR');
  // the built-in profiles keep their own per-profile dispatch
  for (const [profile, text] of [['rust', 'fn main() {}'], ['python', 'x = 1'], ['typescript', 'const x = 1;'], ['powershell', 'Get-ChildItem'], ['c', 'int main(void){return 0;}'], ['assembly', 'mov rax, rbx'], ['machine-code', '48 89 D8']])
    assert.equal(V.validateSource(profile, text, {}).level, 'valid', profile);
});

test('UT-PLG-11 the built-in profiles behave exactly as before, also with plugins loaded', () => {
  const withPlugins = L.loadRegistry();
  const onlyBuiltins = L.loadRegistry(undefined, {enabled: []});
  for (const fileName of ['x.rs', 'x.py', 'x.ts', 'x.ps1', 'x.c', 'a.h', 'a.asm', 'a.bin']) {
    const a = L.resolveLanguage({languageId: 'plaintext', fileName}, 'unknown', null, withPlugins);
    const b = L.resolveLanguage({languageId: 'plaintext', fileName}, 'unknown', null, onlyBuiltins);
    assert.deepEqual(a, b, fileName);
  }
  assert.equal(L.resolveLanguage({languageId: 'python', fileName: 'x.py'}, 'unknown', null, withPlugins).language, 'python');
  assert.equal(L.resolveLanguage({languageId: 'plaintext', fileName: 'x.sql'}, 'unknown', null, withPlugins).reason, 'extension');
  for (const lang of L.LANGUAGES) {
    const prefix = {rust: 'fnm', python: 'def', typescript: 'fn', powershell: 'func', c: 'main', assembly: 'mov', 'machine-code': 'nop'}[lang];
    const a = L.predict({profile: lang, prefix, before: ''}, withPlugins);
    const b = L.predict({profile: lang, prefix, before: ''}, onlyBuiltins);
    assert.deepEqual(ids(a), ids(b), lang);
  }
});

test('UT-PLG-12 the setting and the commands are declared in the manifest (English, Japanese, Simplified Chinese)', () => {
  const pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
  const prop = pkg.contributes.configuration.properties['programmingIme.pluginLanguages'];
  assert.ok(prop, 'the setting exists');
  assert.equal(prop.type, 'array');
  assert.deepEqual(prop.default, ['cypher', 'plsql'], 'T-SQL is off and PL/SQL is on by default');
  const commands = pkg.contributes.commands.map(c => c.command);
  for (const c of ['programmingIme.pluginLanguages', 'programmingIme.importPluginPacks', 'programmingIme.openPluginFolder']) assert.ok(commands.includes(c), c);
  for (const file of ['package.nls.json', 'package.nls.ja.json', 'package.nls.zh-cn.json']) {
    const nls = JSON.parse(fs.readFileSync(path.join(root, file), 'utf8'));
    for (const key of ['cmd.pluginLanguages', 'cmd.importPluginPacks', 'cmd.openPluginFolder', 'cfg.pluginLanguages']) assert.ok(nls[key], `${file}: ${key}`);
  }
  const ext = fs.readFileSync(path.join(root, 'src', 'extension.js'), 'utf8');
  assert.match(ext, /op: 'setPluginLanguages'/, 'the core is told the enabled set');
  assert.match(ext, /programmingIme\.pluginLanguages/, 'the setting is watched');
});
