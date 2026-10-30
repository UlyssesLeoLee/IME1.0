'use strict';
// Unit tests for the seven-language engine. Test titles start with the UT/IT id of the design documents.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const {core} = require('./helpers');
const L = core('languages');
const V = core('validators');
const {InputSession} = core('scheduler');
const {parseModelResult} = core('ai');
const ctxMod = core('context');
const {PimeError} = core('errors');

const ids = list => list.map(c => c.id);
const predict = (profile, prefix, before = '', dialect = {}) => L.predict({profile, prefix, before, dialect});

test('UT-006 candidate kind/source/validation round-trip and invalid kind rejected', () => {
  for (const kind of L.KINDS) for (const source of L.SOURCES) for (const validation of L.VALIDATIONS) {
    const c = L.makeCandidate({id: `x.${kind}.${source}.${validation}`, kind, source, validation, label: 'l', text: 'abc'});
    assert.deepEqual(L.candidateFromJSON(L.candidateToJSON(c)), c);
  }
  assert.throws(() => L.candidateFromJSON({id: 'a', kind: 'bogus', source: 'rule', validation: 'valid'}), e => e instanceof PimeError && e.code === 'OUTPUT_INVALID');
  assert.throws(() => L.candidateFromJSON('{not json'), /not JSON|candidate/);
});

test('UT-007 ranking: pin and deterministic rules first, stable across repeats, AI last', () => {
  const mk = (id, extra) => L.makeCandidate({id, kind: 'syntax', source: 'rule', validation: 'valid', label: id, text: id, match: 'prefix', weight: 50, ...extra});
  const list = [mk('b', {}), mk('a', {}), mk('ai1', {source: 'ai', kind: 'ai_inline', validation: 'unchecked', ai_score: 99}), mk('pinned', {weight: 1}), mk('user', {source: 'user_rule', weight: 1})];
  const first = ids(L.rankCandidates(list, {pins: new Set(['pinned'])}));
  assert.deepEqual(first.slice(0, 2), ['pinned', 'user']);
  assert.equal(first.at(-1), 'ai1');
  assert.ok(first.indexOf('a') < first.indexOf('b'), 'equal scores fall back to the stable id');
  for (let i = 0; i < 10; i++) assert.deepEqual(ids(L.rankCandidates([...list].reverse(), {pins: new Set(['pinned'])})), first);
});

test('UT-008 de-duplication keeps one row and every source', () => {
  const rule = L.makeCandidate({id: 'r', kind: 'syntax', source: 'rule', validation: 'valid', label: 'x', text: 'FN main()  {}', profile_id: 'rust'});
  const ai = L.makeCandidate({id: 'a', kind: 'ai_inline', source: 'ai', validation: 'unchecked', label: 'x', text: 'fn main() {}', profile_id: 'rust'});
  const out = L.dedupe([rule, ai], 'rust');
  assert.equal(out.length, 1);
  assert.deepEqual(out[0].sources, ['rule', 'ai']);
});

test('UT-010 placeholders become snippet tab stops, or plain text without a bridge', () => {
  const template = 'fn ${name}(${args}) {\n    ${cursor}\n}';
  assert.deepEqual(L.placeholdersOf(template), ['name', 'args']);
  assert.equal(L.toSnippet(template), 'fn ${1:name}(${2:args}) {\n    $0\n}');
  assert.equal(L.toSnippet('for ${i} in 0..${n} { ${i} }'), 'for ${1:i} in 0..${2:n} { $1 }');
  assert.equal(L.toSnippet('$items | % { $_ }'), '\\$items | % { \\$_ }', 'literal $ must be escaped');
  const plain = L.renderPlain(template);
  assert.ok(!plain.includes('${'));
  assert.equal(plain, 'fn name(args) {\n    \n}');
});

test('UT-011 / IT-009 seven packs load through one interface; duplicate or corrupt packs are rejected', () => {
  const reg = L.loadRegistry();
  assert.deepEqual([...reg.packs.keys()].sort(), [...L.LANGUAGES].sort());
  for (const lang of L.LANGUAGES) {
    const prefix = {rust: 'fnm', python: 'def', typescript: 'fn', powershell: 'func', c: 'main', assembly: 'mov', 'machine-code': 'nop'}[lang];
    const out = L.predict({profile: lang, prefix, before: ''});
    assert.ok(out.length >= 1, `${lang} produced no candidate`);
    for (const c of out) assert.ok(L.KINDS.includes(c.kind) && L.SOURCES.includes(c.source) && L.VALIDATIONS.includes(c.validation));
    assert.ok(out.some(c => c.validation === 'valid' || c.validation === 'verified'), `${lang}: no valid candidate`);
  }
  assert.throws(() => reg.add(reg.get('rust')), e => e.code === 'RULEPACK_INVALID');
  assert.throws(() => new L.Registry().add({schema_version: 1, profile_id: 'rust'}), e => e.code === 'RULEPACK_INVALID');
  assert.throws(() => new L.Registry().add({...reg.get('rust'), schema_version: 7}), e => e.code === 'RULEPACK_INVALID');
});

test('UT-012 bridge languageId beats extension and lexical hints', () => {
  const d = L.resolveLanguage({languageId: 'rust', fileName: 'x.py', text: 'def f(self):\n    import os\n'});
  assert.equal(d.language, 'rust'); assert.equal(d.reason, 'bridge-languageId');
  assert.equal(L.resolveLanguage({languageId: 'plaintext', fileName: 'x.py'}).language, 'python');
});

test('UT-013 / IT-010 ambiguous signals keep the current language and only suggest', () => {
  for (const fileName of ['a.h', 'a.asm', 'a.bin', 'Makefile']) {
    const d = L.resolveLanguage({languageId: 'plaintext', fileName}, 'rust');
    assert.equal(d.language, 'rust'); assert.equal(d.changed, false); assert.equal(d.uncertain, true);
  }
  assert.equal(L.resolveLanguage({languageId: 'plaintext', fileName: 'a.asm'}, 'rust').suggestion, 'assembly');
  const weak = L.resolveLanguage({languageId: 'plaintext', fileName: 'n.txt', text: 'if x then for y'}, 'python');
  assert.equal(weak.language, 'python'); assert.equal(weak.suggestion, undefined);
  const lexical = L.resolveLanguage({languageId: 'plaintext', fileName: 'Untitled-1', text: 'fn main() {\n    let mut x = 1;\n}\n'}, 'python');
  assert.equal(lexical.language, 'python'); assert.equal(lexical.suggestion, 'rust'); assert.equal(lexical.changed, false);
  const conflict = L.resolveLanguage({languageId: 'plaintext', fileName: 'mix.txt', text: 'fn main() { } def f(self):'}, 'c');
  assert.equal(conflict.suggestion, undefined, 'competing languages: no suggestion');
});

test('UT-014 / IT-011 manual lock outranks automatic detection until unlocked', () => {
  const s = new InputSession({language: 'unknown'});
  s.lock('typescript', 'file', 'f1');
  const py = {languageId: 'python', fileName: 'x.py', fileKey: 'f1'};
  assert.equal(s.resolve(py).language, 'typescript');
  assert.equal(s.resolve({...py, fileKey: 'other-file'}).language, 'python', 'a file lock does not leak to other files');
  s.unlock('file', 'f1');
  assert.equal(s.resolve(py).language, 'python');
  s.lock('rust', 'global');
  assert.equal(s.resolve(py).language, 'rust', 'global lock');
  assert.equal(s.lock('klingon', 'file', 'x'), false);
});

test('UT-015 / IT-008 switching language bumps profile_epoch and invalidates older keys', () => {
  const s = new InputSession({language: 'rust'});
  s.profileEpoch = 4; s.noteInput(); s.noteInput();
  const oldKey = s.key();
  assert.equal(s.isCurrent(oldKey), true);
  assert.equal(s.setLanguage('python'), true);
  assert.equal(s.profileEpoch, 5);
  assert.equal(s.isCurrent(oldKey), false, 'a result computed for Rust must not be applied to Python');
  assert.equal(s.setLanguage('python'), false);
});

test('UT-016 / IT-012 dialects: rules of another standard are not offered, invalid syntax is flagged', () => {
  assert.ok(ids(predict('c', 'constexpr', '', {std: 'c23'})).includes('c.constexpr.23#0'));
  assert.deepEqual(predict('c', 'constexpr', '', {std: 'c17'}), []);
  assert.ok(ids(predict('c', 'static', '', {std: 'c11'})).includes('c.static_assert.11#0'));
  assert.ok(!ids(predict('c', 'static', '', {std: 'c23'})).includes('c.static_assert.11#0'));
  assert.equal(V.validateSource('c', 'constexpr int a = 1;', {std: 'c11'}).ok, false);
  assert.equal(V.validateSource('c', 'constexpr int a = 1;', {std: 'c23'}).ok, true);
  assert.ok(ids(predict('powershell', 'tern', '$x = ', {version: '7'})).includes('ps.ternary.7#0'));
  assert.ok(ids(predict('powershell', 'tern', '$x = ', {version: '5.1'})).includes('ps.ifelse.5#0'));
  assert.equal(V.validateSource('powershell', '$a ?? 1', {version: '5.1'}).ok, false);
  assert.equal(V.validateSource('powershell', '$a ?? 1', {version: '7'}).ok, true);
  assert.ok(ids(predict('rust', 'ext', '', {edition: '2021'})).includes('rust.extern.2021#0'));
  assert.ok(ids(predict('rust', 'ext', '', {edition: '2024'})).includes('rust.unsafe_extern.2024#0'));
  const unsupported = predict('rust', 'fn', '', {edition: '2018'});
  assert.equal(unsupported.length, 1); assert.equal(unsupported[0].validation, 'context_missing'); assert.equal(unsupported[0].text, null);
});

test('UT-017 Rust: no semicolon is forced when the position is not provably a statement', () => {
  const stmt = predict('rust', 'println', 'fn main() {\n    ');
  assert.ok(stmt[0].text.endsWith(';'));
  const expr = predict('rust', 'println', 'let x = ');
  assert.ok(!expr[0].text.endsWith(';'));
  const unknown = predict('rust', 'println', 'foo.bar baz ');
  assert.ok(!unknown[0].text.endsWith(';')); assert.ok(unknown[0].assumptions.some(a => /semicolon/i.test(a)));
  const match = predict('rust', 'mat', 'let v = ').find(c => c.id === 'rust.match#0');
  assert.ok(match && !match.text.trimEnd().endsWith(';'));
  assert.equal(L.classifyContext('rust', 'fn main() {\n    '), 'statement');
  assert.equal(L.classifyContext('rust', 'let x = '), 'expression');
  assert.equal(L.classifyContext('rust', 'fn f() -> '), 'type');
});

test('UT-018 Python templates follow the host indentation and EOL without mixing', () => {
  const template = 'def ${name}():\n    ${cursor}\n    pass';
  const two = L.renderForHost('python', template, {tabSize: 2, insertSpaces: true, eol: '\n'});
  assert.deepEqual(two.split('\n').map(l => /^ */.exec(l)[0].length), [0, 2, 2]);
  const tabs = L.renderForHost('python', template, {insertSpaces: false, eol: '\r\n'});
  assert.ok(!/(?<!\r)\n/.test(tabs), 'only CRLF');
  for (const line of tabs.split('\r\n').slice(1)) assert.match(line, /^\t*[^\t ]*$/, 'tabs only, no spaces in the indent');
  assert.equal(L.renderForHost('python', template, {tabSize: 4, baseIndent: '    '}).split('\n')[2], '        pass');
  assert.equal(V.validateSource('python', L.renderPlain(template, 'pass')).ok, true);
});

test('UT-019 TSX: a generic arrow is never offered as a bare <T>', () => {
  const tsx = predict('typescript', 'garrow', '', {mode: 'tsx'})[0];
  assert.match(tsx.text, /<T,>\(/); assert.doesNotMatch(tsx.text, /<T>\(/);
  assert.match(predict('typescript', 'garrow', '', {mode: 'ts'})[0].text, /<T>\(/);
  assert.equal(V.validateSource('typescript', 'const f = <T>(v: T) => v;', {mode: 'tsx'}).ok, false);
  assert.equal(V.validateSource('typescript', 'const f = <T,>(v: T) => v;', {mode: 'tsx'}).ok, true);
  assert.deepEqual(ids(predict('typescript', 'comp', '', {mode: 'ts'})), [], 'JSX component only in TSX');
  assert.ok(ids(predict('typescript', 'comp', '', {mode: 'tsx'})).includes('ts.component.tsx#0'));
  assert.equal(L.resolveLanguage({languageId: 'typescriptreact', fileName: 'A.tsx'}).dialectHints.mode, 'tsx');
});

test('UT-037 / AT-026 any letter prefix yields valid local candidates without typing the full trigger', () => {
  const cases = [['rust', 'f'], ['rust', 'fn'], ['python', 'd'], ['python', 'de'], ['typescript', 'f'], ['typescript', 'as'], ['powershell', 'Ge'], ['powershell', 'p'], ['c', 'ma'], ['c', 'in']];
  for (const [profile, prefix] of cases) {
    const out = predict(profile, prefix);
    assert.ok(out.length >= 1, `${profile}/${prefix}`);
    assert.ok(out.every(c => c.validation !== 'invalid'));
  }
  assert.ok(ids(predict('rust', 'f')).includes('rust.fn#0'));
  assert.deepEqual(predict('rust', 'zzzzzz'), []);
  assert.deepEqual(predict('rust', ''), []);
  assert.deepEqual(L.predict({profile: 'rust', prefix: 'f'.repeat(129)}), []);
  const fuzzy = predict('rust', 'pfn');
  assert.ok(ids(fuzzy).includes('rust.pubfn#0'), 'non-fixed abbreviation: pfn -> pubfn');
});

test('UT-037 nearby code feeds identifier candidates (no project-wide claims)', () => {
  const out = L.predict({profile: 'rust', prefix: 'cou', before: 'let counter = 1;\nlet count_total = counter + 2;\n'});
  const idc = out.filter(c => c.kind === 'identifier');
  assert.deepEqual(idc.map(c => c.label), ['counter', 'count_total']);
  assert.ok(idc.every(c => c.source === 'context' && c.validation === 'unchecked' && c.assumptions.length));
  assert.deepEqual(predict('rust', 'fn', 'let s = "abc '), [], 'nothing is proposed inside a string');
  assert.deepEqual(predict('rust', 'fn', '// hello '), [], 'nothing is proposed inside a comment');
});

test('UT-041 candidates carry source, language and explanation; unverified AI never looks verified', () => {
  const rule = predict('rust', 'fnm')[0];
  assert.equal(rule.source, 'rule'); assert.equal(rule.profile_id, 'rust'); assert.ok(rule.explain);
  const ai = L.makeCandidate({id: 'ai.1', kind: 'ai_inline', source: 'ai', label: 'x', text: 'fn x() {}', ai_score: 1, assumptions: ['Model output']});
  assert.equal(ai.validation, 'unchecked');
  assert.notEqual(ai.validation, 'verified');
  const missing = predict('machine-code', 'jmp external_symbol')[0];
  assert.equal(missing.validation, 'context_missing'); assert.equal(missing.text, null);
});

test('UT-042 / IT-025 without full context a whole-file or cross-file proposal is not applicable', () => {
  const l1 = ctxMod.makeSnapshot({uri: 'file:///a.rs', version: 1, text: 'fn a() {}\n', level: 'L1'});
  assert.throws(() => ctxMod.assertFullFileWritable(l1), e => e.code === 'FULL_CONTEXT_REQUIRED');
  const raw = JSON.stringify({files: [{path: 'a.rs', content: 'fn a() { 1 }\n'}, {path: '../other.rs', content: 'pwn'}, {uri: 'file:///elsewhere/b.rs', content: 'x'}], explanation: 'e'});
  const out = parseModelResult(raw, {task: 'full', baseline: 'fn a() {}\n', currentFile: 'C:\\w\\a.rs'});
  assert.equal(out.proposedText, 'fn a() { 1 }\n'); assert.equal(out.rejectedTargets, 2);
  const onlyOther = parseModelResult(JSON.stringify({files: [{path: 'b.rs', content: 'x'}]}), {task: 'full', baseline: 'fn a() {}\n', currentFile: 'a.rs'});
  assert.equal(onlyOther.proposedText, undefined);
});

test('UT-058 rule packs are validated as data: size, unknown fields, placeholders, collisions', () => {
  const base = {schema_version: 1, profile_id: 'rust', version: '1.0.0', triggers: [{id: 'u.x', pattern: 'abc', candidates: [{label: 'l', text: 'abc ${cursor}'}]}]};
  assert.doesNotThrow(() => L.validatePack(base, {builtin: false}));
  const bad = (mutate, re) => { const p = JSON.parse(JSON.stringify(base)); mutate(p); assert.throws(() => L.validatePack(p, {builtin: false}), e => e.code === 'RULEPACK_INVALID' && (!re || re.test(e.message))); };
  bad(p => { p.triggers[0].candidates[0].shell = 'exec'; }, /unknown field/);
  bad(p => { p.exec = 'rm -rf /'; }, /unknown field/);
  bad(p => { p.accepted_extensions = ['.x']; }, /unknown field/);
  bad(p => { p.triggers[0].candidates[0].text = 'x ${$(whoami)}'; }, /placeholder/);
  bad(p => { p.triggers[0].candidates[0].text = 'x'.repeat(4001); });
  bad(p => { p.triggers = Array.from({length: 2001}, (_, i) => ({id: 't' + i, pattern: 'a', candidates: [{label: 'l', text: 'a'}]})); });
  bad(p => { p.triggers[0].pattern = 'a b;rm'; });
  bad(p => { p.profile_id = 'cobol'; });
  assert.throws(() => L.validatePack(base, {builtin: false, bytes: 262145}), e => e.code === 'RULEPACK_INVALID');
  const reg = L.loadRegistry();
  assert.throws(() => reg.addUser({...base, triggers: [{...base.triggers[0], id: 'rust.main'}]}), /collides/);
  reg.addUser(base);
  const out = L.predict({profile: 'rust', prefix: 'abc', before: ''}, reg);
  assert.equal(out[0].source, 'user_rule', 'a user rule ranks above built-in rules');
});
