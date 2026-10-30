'use strict';
// Regenerates the shared golden fixtures from the JavaScript engine. The JS unit tests re-check them on every run
// (so a behaviour change must be a deliberate fixture update), and the Rust core must reproduce them exactly.
// Usage: node tests/gen-fixtures.js
const fs = require('node:fs');
const path = require('node:path');
const core = path.join(__dirname, '..', 'editor-bridges', 'vscode', 'src', 'core');
const languages = require(path.join(core, 'languages'));
const validators = require(path.join(core, 'validators'));
const x86 = require(path.join(core, 'x86'));
const secrets = require(path.join(core, 'secrets'));

const out = path.join(__dirname, 'fixtures');
fs.mkdirSync(out, {recursive: true});
// --check compares instead of writing, so tests can detect drift without touching files other tools are reading.
const CHECK = process.argv.includes('--check');
let drift = 0;
const write = (name, data) => {
  const text = JSON.stringify(data, null, 1) + '\n', file = path.join(out, name);
  if (!CHECK) { fs.writeFileSync(file, text); return; }
  const current = fs.existsSync(file) ? fs.readFileSync(file, 'utf8').replace(/\r\n/g, '\n') : null;
  if (current !== text) { drift++; console.error(`fixture drift: ${name}`); }
};

// ---------------------------------------------------------------- predict
const P = (profile, prefix, before = '', dialect = {}, after = '') => ({profile, prefix, before, after, dialect});
const predictInputs = [
  P('rust', 'fnm'), P('rust', 'f'), P('rust', 'fn', 'impl Foo {\n    '), P('rust', 'fn', 'trait T {\n    '), P('rust', 'fn', 'struct A;\n\n'),
  P('rust', 'fn', 'impl Foo {\n    fn a() {\n        '), P('rust', 'println', 'fn main() {\n    '), P('rust', 'println', 'let x = '), P('rust', 'println', 'foo.bar baz '),
  P('rust', 'fn', 'let s = "abc '), P('rust', 'fn', '// hello '), P('rust', 'fn', '/* block '), P('rust', 'mat', 'let v = '), P('rust', 'res', 'fn f() -> '),
  P('rust', 'cou', 'let counter = 1;\nlet count_total = counter + 2;\n'), P('rust', 'ext', '', {edition: '2021'}), P('rust', 'ext', '', {edition: '2024'}),
  P('rust', 'Fn'), P('rust', 'zzzz'), P('rust', 'imp', 'fn main() {\n    '), P('rust', 'impl', 'impl A {\n    '), P('rust', 'opt', 'let x: '), P('rust', 'tes', ''),
  P('rust', 'fn', '', {edition: '2018'}),
  P('python', 'd'), P('python', 'def', 'class A:\n    '), P('python', 'def', 'x = 1\n'), P('python', 'def', 'class A:\n    def g(self):\n        '), P('python', 'fori', 'def f():\n    '),
  P('python', 'lc', 'x = '), P('python', 'cl'), P('python', 'pri'), P('python', 't'), P('python', 'if', '# comment '), P('python', 'wi'), P('python', 'ma', 'x = 1\n'),
  P('typescript', 'garrow', '', {mode: 'ts'}), P('typescript', 'garrow', '', {mode: 'tsx'}), P('typescript', 'comp', '', {mode: 'tsx'}), P('typescript', 'comp', '', {mode: 'ts'}),
  P('powershell', 'func', '$a = 1\n'), P('powershell', 'func', '$a = 1 +\n'), P('typescript', 'fori', 'y = 2\n'), P('typescript', 'fori', 'const a = 1\nconst b = 2\n'), P('python', 'fori', 'y = 2\n'), P('rust', 'fori', 'y = 2\n'),
  P('typescript', 'int'), P('typescript', 'fn', 'const x = '), P('typescript', 'pro', 'let p: '), P('typescript', 'as'), P('typescript', 'en'), P('typescript', 'cla'),
  P('powershell', 'Ge'), P('powershell', 'p'), P('powershell', 'tern', '$x = ', {version: '7'}), P('powershell', 'tern', '$x = ', {version: '5.1'}), P('powershell', 'func'),
  P('powershell', 'foreach'), P('powershell', 'para', '', {version: '7'}), P('powershell', 'para', '', {version: '5.1'}), P('powershell', 'Wh'), P('powershell', 'gci'),
  P('c', 'static', '', {std: 'c11'}), P('c', 'static', '', {std: 'c23'}), P('c', 'constexpr', '', {std: 'c17'}), P('c', 'nullptr', 'x = ', {std: 'c23'}), P('c', 'nullptr', 'x = ', {std: 'c11'}),
  P('c', '#inc'), P('c', 'inc'), P('c', 'main'), P('c', 'fnptr'), P('c', 'str'), P('c', 'ty'), P('c', 'if', '// c '),
  P('assembly', 'mov', '', {syntax: 'intel'}), P('assembly', 'mov', '', {syntax: 'att', assembler: 'gas'}), P('assembly', 'prol', '', {syntax: 'intel'}), P('assembly', 'prol', '', {syntax: 'att', assembler: 'gas'}),
  P('assembly', 'sec', '', {assembler: 'nasm'}), P('assembly', 'sec', '', {assembler: 'gas', syntax: 'intel'}), P('assembly', 'sec', '', {assembler: 'masm'}), P('assembly', 'ret'), P('assembly', 'sys'),
  P('assembly', 'mov', '', {isa: 'arm64'}), P('assembly', 'mo', 'mov rax, rbx ; copy '),
  P('machine-code', 'nop'), P('machine-code', 'push r'), P('machine-code', 'mov rax, rbx'), P('machine-code', 'jmp foo'), P('machine-code', '48 89 d8'), P('machine-code', 'xor eax'),
  P('machine-code', 'mov r8'), P('machine-code', 'nop', '', {isa: 'arm64'}), P('machine-code', 'sub rsp, 0x20'), P('machine-code', 'mov rax, 1'), P('machine-code', 'push rbp', '', {format: 'compact'}),
  P('machine-code', 'ret', '', {format: 'compact'}), P('machine-code', 'xor edi, edi'),
  P('rust', ''), P('unknown', 'fn')
];
write('predict-cases.json', {generated_by: 'tests/gen-fixtures.js', limit: 6, cases: predictInputs.map(input => {
  const r = languages.predict({...input, limit: 6});
  return {...input, expect: r.map(c => ({id: c.id, kind: c.kind, source: c.source, validation: c.validation, text: c.text, match: c.match}))};
})});

// ---------------------------------------------------------------- validators
const V = (profile, text, dialect = {}) => ({profile, text, dialect});
const validateInputs = [
  V('rust', 'fn main() {\n    println!("}");\n}\n'), V('rust', "fn main() {\n    let c = '}';\n    let x: &'a str = \"\";\n}\n"), V('rust', 'fn main() {\n'), V('rust', 'fn f() { ( ] }'),
  V('rust', '/* a /* nested */ still */ fn f() {}'), V('rust', 'let s = r#"raw } text"#;'),
  V('python', 'def f():\n    pass\n'), V('python', 'def f():\n'), V('python', 'def f():\n\n'), V('python', 'def f():\n\treturn 1\n    x=1\n'),
  V('python', 'class A:\n    def g(self):\n        return {\n  1: 2,\n  }\n\nx = 1\n'), V('python', 'if x:\n    y = 1\n  z = 2\n'), V('python', 'x = """\ndef f():\n"""\n'),
  V('python', 'try:\n    pass\nexcept Exception:\n    raise\n'), V('python', 'x = (1,\n  2)\ny = 3\n'),
  V('powershell', '$a ?? 1', {version: '5.1'}), V('powershell', '$a ?? 1', {version: '7'}), V('powershell', 'Get-ChildItem | ? { $_.X }', {version: '5.1'}), V('powershell', '$x = $c ? 1 : 2', {version: '5.1'}),
  V('powershell', 'a && b', {version: '5.1'}), V('powershell', "Write-Host 'a ?? b'", {version: '5.1'}), V('powershell', 'function f {\n  param($a)\n}\n', {version: '5.1'}),
  V('c', 'int main(void){ return 0; }'), V('c', 'constexpr int a = 1;', {std: 'c11'}), V('c', 'constexpr int a = 1;', {std: 'c23'}), V('c', 'char (*fn)(void);'), V('c', 'int x[ ;'),
  V('c', 'int a = 0b101;', {std: 'c17'}), V('c', 'int a = 0b101;', {std: 'c23'}), V('c', '[[nodiscard]] int f(void);', {std: 'c11'}), V('c', 'char *s = "}";'),
  V('typescript', 'const f = <T>(v: T) => v;', {mode: 'tsx'}), V('typescript', 'const f = <T,>(v: T) => v;', {mode: 'tsx'}), V('typescript', 'const f = <T>(v: T) => v;', {mode: 'ts'}),
  V('typescript', 'const t = `a ${b} }`;'), V('typescript', 'function f() {\n'),
  V('assembly', 'mov rax, rbx\npush rbp\nret\n', {syntax: 'intel'}), V('assembly', 'movq %rbx, %rax\n', {syntax: 'intel'}), V('assembly', 'movq %rbx, %rax\n', {syntax: 'att', assembler: 'gas'}),
  V('assembly', 'mov rax, rbx\n', {syntax: 'att', assembler: 'gas'}), V('assembly', 'mov rax, rbx\nmovq %rbx, %rax\n', {syntax: 'intel'}),
  V('assembly', '.intel_syntax noprefix\nmov rax, rbx\n', {syntax: 'att', assembler: 'gas'}), V('assembly', 'section .text\nglobal _start\n_start:\n  ret\n', {assembler: 'nasm'}),
  V('assembly', '.globl _start\n', {assembler: 'nasm'}), V('assembly', 'section .text\n', {assembler: 'gas', syntax: 'att'}), V('assembly', 'mov rax, [rbx] ; load\n', {syntax: 'intel'}),
  V('machine-code', '48 89 D8'), V('machine-code', 'F'), V('machine-code', 'GG'), V('machine-code', '0x90, 0xC3\n// comment\n48 89 d8'), V('machine-code', '90 C')
];
write('validate-cases.json', {generated_by: 'tests/gen-fixtures.js', cases: validateInputs.map(input => {
  const r = validators.validateSource(input.profile, input.text, input.dialect);
  return {...input, expect: {ok: r.ok, level: r.level, codes: r.issues.map(i => i.code), lines: r.issues.map(i => i.line)}};
})});

// ---------------------------------------------------------------- x86 encoder
const asm = ['nop', 'ret', 'int3', 'syscall', 'leave', 'cpuid', 'hlt', 'ud2', 'pause', 'cdq', 'cqo', 'push rbp', 'push rax', 'push r8', 'push r15', 'pop rbp', 'pop r12', 'pop rax',
  'mov rbp, rsp', 'mov rax, rbx', 'mov rax, r8', 'mov r8, rax', 'mov r15, r14', 'mov ecx, edx', 'mov r8d, r9d', 'xor eax, eax', 'xor edi, edi', 'xor r10d, r11d', 'mov eax, 1', 'mov r9d, 5', 'mov ebx, 0xdeadbeef',
  'sub rsp, 0x20', 'add rsp, 0x20', 'sub rsp, 8', 'add rsp, -8', 'sub rsp, 0x100', 'add r12, 1', 'add rax, rbx', 'sub rax, rbx', 'cmp rax, rbx', 'and rax, rbx', 'or rax, rbx', 'xor rax, rax', 'cmp r8d, r9d',
  'MOV  RAX ,RBX', '  nop  ', 'jmp external_symbol', 'call foo', 'je loop_start', 'jmp rax', 'mov rax, 1', 'mov rax, [rbx]', 'lea rax, [rbx]', 'push 5', 'push eax', 'add rsp', 'xyz', '', 'mov eax, 0x1ffffffff'];
write('x86-vectors.json', {generated_by: 'tests/gen-fixtures.js', note: 'encodings are the documented x86-64 forms; unsupported/unresolved cases must NOT yield bytes', cases: asm.map(a => {
  const r = x86.encode(a);
  return r.ok ? {asm: a, ok: true, hex: r.hex} : {asm: a, ok: false, code: r.code};
})});

// ---------------------------------------------------------------- language resolution
const R = (ctx, current = 'unknown', lock = null) => ({ctx, current, lock});
const resolveInputs = [
  R({languageId: 'rust', fileName: 'x.py'}), R({languageId: 'plaintext', fileName: 'a.h'}, 'rust'), R({languageId: 'plaintext', fileName: 'a.asm'}, 'unknown'), R({languageId: 'python', fileName: 'x.py'}, 'python', 'typescript'),
  R({languageId: 'typescriptreact', fileName: 'App.tsx'}), R({languageId: 'plaintext', fileName: 'main.ts'}), R({languageId: 'plaintext', fileName: 'run.ps1'}), R({languageId: 'plaintext', fileName: 'x.c'}),
  R({languageId: 'plaintext', fileName: 'blob.bin'}, 'c'), R({languageId: 'plaintext', fileName: 'Makefile'}, 'rust'), R({languageId: 'plaintext', fileName: 'notes.txt', text: 'if x then for y'}, 'python'),
  R({languageId: 'plaintext', fileName: 'Untitled-1', text: 'fn main() {\n    let mut x = 1;\n}\n'}, 'python'), R({languageId: 'plaintext', fileName: 'Untitled-2', text: 'def f(self):\n    import os\n'}, 'unknown'),
  R({languageId: 'asm', fileName: 'x.asm'}), R({languageId: 'nasm', fileName: 'x.nasm'}), R({languageId: 'cpp', fileName: 'x.cpp'}, 'c'), R({languageId: 'plaintext', fileName: 'x.hex'}, 'rust'),
  R({languageId: 'plaintext', fileName: 'mixed.txt', text: 'fn main() { } def f(self):'}, 'rust')
];
write('resolve-cases.json', {generated_by: 'tests/gen-fixtures.js', cases: resolveInputs.map(input => {
  const r = languages.resolveLanguage(input.ctx, input.current, input.lock);
  return {...input, expect: {language: r.language, reason: r.reason, changed: r.changed, uncertain: !!r.uncertain, suggestion: r.suggestion || null, dialectHints: r.dialectHints}};
})});

// ---------------------------------------------------------------- secret scanning (findings carry no values)
const secretInputs = [
  {text: 'const k = "sk-abcdefghijklmnopqrstuvwxyz0123";\napi_key = "mock-key-test-123"\npassword = process.env.PW\n-----BEGIN RSA PRIVATE KEY-----\nC:\\Users\\alice\\proj\nAKIAABCDEFGHIJKLMNOP\n', fileName: ''},
  {text: 'fn main() {}', fileName: ''}, {text: 'x', fileName: 'config/.env'}, {text: 'x', fileName: 'id_rsa'}, {text: 'x', fileName: 'src/main.rs'},
  {text: 'token = "ghp_' + 'a'.repeat(36) + '"', fileName: ''}, {text: 'Authorization: Bearer abcdefghijklmnopqrstuvwxyz012345', fileName: ''},
  {text: 'url = "https://user:pass@example.com/x"', fileName: ''}, {text: 'secret = "${SECRET}"\napi_key = "your_api_key_here"\npassword: <password>', fileName: ''},
  {text: 'path = "/home/bob/project"', fileName: ''}, {text: 'eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijklmnop', fileName: ''}
];
write('secret-cases.json', {generated_by: 'tests/gen-fixtures.js', cases: secretInputs.map(input => {
  const r = secrets.scan(input.text, {fileName: input.fileName});
  return {...input, expect: {highest: r.highest, sensitiveFile: r.sensitiveFile, findings: r.findings.map(f => ({kind: f.kind, severity: f.severity, line: f.line}))}};
})});
// ---------------------------------------------------------------- edit path: sha256, diff/hunks, excerpts, settings, workspace tokens
// The Rust core (crates/pime-core: sha256, edit::diff, context, settings, grants) must reproduce all of this exactly.
{
  const ctxMod = require(path.join(core, 'context'));
  const diffMod = require(path.join(core, 'diff'));
  const settingsMod = require(path.join(core, 'settings'));
  const aiMod = require(path.join(core, 'ai'));
  const grantsMod = require(path.join(core, 'grants'));
  const lines = (n, f) => Array.from({length: n}, (_, i) => f(i)).join('');

  const shaTexts = ['', 'abc', 'é😀', 'a\u0000b\r\n', 'x'.repeat(1000), '中文テキスト\n', 'line1\nline2\r\nline3\r'];
  const diffPairs = [
    ['line1\r\nline2\r\nline3\r\nline4\r\nline5\r\nline6\r\nline7\r\nline8\r\n', 'line1\r\nLINE2\r\nline3\r\nline4\r\nL5a\r\nL5b\r\nline6\r\nline7\r\nline8'],
    ['a\rb\rc', 'a\rb\rc\n'], ['a😀\nb\nc\n', 'a😀\nB\nc\n'], ['', 'abc\n'], ['abc\n', ''], ['same\n', 'same\n'], ['', ''],
    ['a\nb\n', 'a\nb\nc\n'], ['a\nb\nc\n', 'a\nc\n'], ['a\nb\nc\nd\n', 'd\nc\nb\na\n'], ['x\ny\nz\n', 'y\nz\nx\n'], ['a\na\nb\na\n', 'a\nb\na\na\n'],
    ['a\nb\n', 'a\r\nb\r\n'], ['a\n\nb\r\n\rc', 'a\n\nB\r\n\rc\n'], ['\n\n\n', '\n\n'], ['no newline', 'no newline\nsecond'],
    ['fn a() {}\n\nfn b() {}\n\nfn c() {}\n', 'fn a() { 1 }\n\nfn b() {}\n\nfn c() { 3 }\n'],
    ['😀\n😀😀\n', '😀😀\n😀\n'], ['\u{1F600}x\n'.repeat(3), '\u{1F600}x\n\u{1F600}y\n\u{1F600}x\n'],
    [lines(60, i => `line ${i}\n`), lines(60, i => (i % 5 === 0 ? `LINE ${i}\n` : `line ${i}\n`))],
    [lines(30, i => `row ${i}\n`), lines(30, i => `row ${i}\n`).split('\n').reverse().join('\n')]
  ];
  // too large for the LCS table (n * m > 4M): one replace hunk; stored as generators, not as 40 KB texts
  const synthetic = [
    {head: 1, tail: 1, lines: 2001}, {head: 0, tail: 0, lines: 2001}, {head: 0, tail: 0, lines: 2000}, {head: 5, tail: 2, lines: 1500}
  ];
  const buildSynthetic = ({head, tail, lines: n}) => {
    const keepHead = lines(head, i => `head ${i}\n`), keepTail = lines(tail, i => `tail ${i}\n`);
    return [keepHead + lines(n, i => `old ${i}\n`) + keepTail, keepHead + lines(n, i => `new ${i}\n`) + keepTail];
  };
  const diffCase = (before, after) => {
    const hunks = diffMod.buildHunks(before, after);
    const ids = hunks.map(h => h.id);
    const picks = [ids, [], ids.slice(0, 1), ids.slice(-1), ids.filter((_, i) => i % 2 === 0)];
    return {
      hunks, stats: diffMod.stats(hunks), render: hunks.map(h => diffMod.renderHunk(before, h)),
      selections: picks.map(sel => ({ids: sel, edits: diffMod.hunkEdits(hunks, sel), resultHash: ctxMod.sha256(diffMod.applyHunks(before, hunks, sel))})),
      afterHash: ctxMod.sha256(after)
    };
  };
  const excerptInputs = [
    {text: 'x'.repeat(500) + '😀' + 'y'.repeat(500), cursor: 500, before: 80, after: 80}, {text: 'x'.repeat(500) + '😀' + 'y'.repeat(500), cursor: 502, before: 1, after: 1},
    {text: 'x'.repeat(500) + '😀' + 'y'.repeat(500), cursor: 500, before: 1, after: 1}, {text: 'abc', cursor: 99, before: 10, after: 10}, {text: 'abc', cursor: -5, before: 10, after: 10},
    {text: 'aaaa\r\nbbbb', cursor: 7, before: 2, after: 3}, {text: 'aaaa\r\nbbbb', cursor: 6, before: 2, after: 0}, {text: '', cursor: 0, before: 5, after: 5},
    {text: 'one\ntwo\nthree 😀 four\nfive\n', cursor: 14, before: 5, after: 5}, {text: 'a'.repeat(5000), cursor: 2500, before: 1800, after: 900}
  ];
  const providersWithExtras = {oa: {api: 'openai-chat', endpoint: 'https://a.example/v1', maxTokensParam: 'max_completion_tokens'}, an: {api: 'anthropic-messages', endpoint: 'https://b.example/v1', apiVersion: '2026-10-21'}};
  const settingsInputs = [
    {}, {providers: {}, scenarios: {}, budget: {}}, {budget: {maxRequests: 3}},
    {providers: {mockA: {api: 'openai-responses', endpoint: 'https://127.0.0.1:9/v1/responses'}}, scenarios: {f1Full: {provider: 'mockA', model: 'm-1', effort: 'high'}, other: {model: 'z'}}, budget: {maxRequests: 3, extra: 1}},
    {providers: {openai: {endpoint: 'https://proxy.example/v1/responses'}, 'bad id': {api: 'x'}, nul: null, b: {api: 'openai-chat', endpoint: 'https://b.example/'}, 10: {api: 'openai-chat', endpoint: 'https://x.example/'}, 2: {api: 'openai-chat', endpoint: 'https://y.example/'}}},
    {providers: providersWithExtras, scenarios: {f1Full: {provider: 'oa', effort: 'xhigh'}, f1Insert: {provider: 'an', effort: 'none'}, realtime: {provider: 'oa', effort: 'minimal'}}},
    {providers: {...providersWithExtras, bad1: {api: 'openai-chat', endpoint: 'https://c.example/v1', maxTokensParam: 'max_new_tokens'}, bad2: {api: 'anthropic-messages', endpoint: 'https://d.example/v1', apiVersion: 'latest'}}},
    {providers: {p: {api: 'nope', endpoint: 'http://insecure.example'}}, scenarios: {realtime: {provider: 'ghost', effort: 'turbo', maxOutputTokens: 0, timeoutMs: 999}, f1Full: {maxOutputTokens: 64001, timeoutMs: 300001}, f1Insert: {maxOutputTokens: 1.5}}},
    {scenarios: {f1Insert: {provider: 'ghost'}, realtime: {effortCapability: {state: 'supported', supported: ['low'], checkedAt: '2026-01-01T00:00:00.000Z'}}}}
  ];
  const endpoints = [
    'https://api.openai.com/v1/responses', 'HTTPS://Example.COM/x', 'https:example.com', 'https:\\\\example.com\\x', 'https:///example.com', 'https://127.0.0.1:8443/v1', 'https://[::1]:8443/v1',
    'https://@example.com', 'https://:@example.com', 'https://example.com:/x', '  https://example.com/x \n', 'ht\ttps://example.com', 'https://user@example.com', 'https://user:pw@example.com',
    'https://:pw@example.com', 'https://example.com:99999', 'https://example.com:80a', 'https://exa mple.com', 'https://exa<mple.com', 'https://a@', 'http://example.com', 'ftp://example.com',
    'https://', 'https:///', '', 'example.com', '//example.com', 'https://example.com:65535', 'https://example.com:65536', 'https://a.b.c.d.example.com/path?query=1#frag', 'https://exa%6Dple.com/',
    'https://exa%25mple.com', 'https://exa%zzmple.com', 'https://exa%2', 'https://exämple.com/', 'https://localhost:8080/x', 'https://1.2.3.4/x'
  ];
  const workspaces = [['file:///w'], ['file:///b', 'file:///a'], ['file:///a', 'file:///b'], ['￿', '\u{1F600}'], [], ['no-folder'], ['file:///w/ü', 'file:///w/中']];
  const settle = values => {
    const e = settingsMod.effective(values);
    const resolved = {};
    for (const name of settingsMod.SCENARIOS) { try { resolved[name] = settingsMod.resolveScenario(e, name); } catch (err) { resolved[name] = {error: err.code}; } }
    let nope; try { settingsMod.resolveScenario(e, 'nope'); } catch (err) { nope = err.code; }
    return {values, effective: e, issues: settingsMod.validateSettings(e), resolved, unknownScenario: nope};
  };
  write('edit-cases.json', {
    generated_by: 'tests/gen-fixtures.js',
    sha256: shaTexts.map(text => ({text, sha256: ctxMod.sha256(text)})),
    diff: diffPairs.map(([before, after]) => ({before, after, expect: diffCase(before, after)})),
    syntheticDiff: synthetic.map(spec => { const [before, after] = buildSynthetic(spec); const e = diffCase(before, after); return {...spec, expect: {hunks: e.hunks.map(({oldText, newText, ...geometry}) => geometry), stats: e.stats, selections: e.selections.map(({edits, ...rest}) => ({...rest, edits: edits.map(x => ({start: x.start, end: x.end}))})), afterHash: e.afterHash}}; }),
    excerpt: excerptInputs.map(i => ({...i, expect: ctxMod.excerpt(i.text, i.cursor, {before: i.before, after: i.after})})),
    settings: settingsInputs.map(settle),
    endpoints: endpoints.map(endpoint => { let ok = true; try { aiMod.checkEndpoint(endpoint); } catch { ok = false; } return {endpoint, ok}; }),
    workspaceTokens: workspaces.map(uris => ({uris, token: grantsMod.workspaceToken(uris)}))
  });
}
if (CHECK) { console.log(drift ? `${drift} fixture file(s) differ` : 'fixtures are up to date'); process.exit(drift ? 1 : 0); }
console.log('fixtures written to', out);
