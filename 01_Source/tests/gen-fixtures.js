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
if (CHECK) { console.log(drift ? `${drift} fixture file(s) differ` : 'fixtures are up to date'); process.exit(drift ? 1 : 0); }
console.log('fixtures written to', out);
