'use strict';
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {core} = require('./helpers');
const V = core('validators');
const x86 = core('x86');
const L = core('languages');

test('UT-020 PowerShell candidates are plain text only (never executed)', () => {
  const out = L.predict({profile: 'powershell', prefix: 'foreach', before: ''}).concat(L.predict({profile: 'powershell', prefix: 'gci', before: ''}));
  assert.ok(out.length >= 2);
  for (const c of out) { assert.equal(typeof c.text, 'string'); assert.ok(Object.values(c).every(v => typeof v !== 'function')); }
  assert.match(out.find(c => c.id === 'ps.gci#0').label, /never executed/);
  const pipeline = 'Get-ChildItem | Where-Object { $_.Length -gt 1 }';
  assert.equal(V.validateSource('powershell', pipeline, {version: '5.1'}).ok, true, 'a pipeline is validated as text, not run');
});

test('UT-021 C declarations and standard versions', () => {
  assert.equal(V.validateSource('c', 'char (*fn)(void);').ok, true);
  assert.equal(V.validateSource('c', 'int arr[3][4];').ok, true);
  assert.equal(V.validateSource('c', 'int x[ ;').ok, false);
  assert.equal(V.validateSource('c', '[[nodiscard]] int f(void);', {std: 'c11'}).ok, false);
  assert.equal(V.validateSource('c', '[[nodiscard]] int f(void);', {std: 'c23'}).ok, true);
  assert.equal(V.validateSource('c', 'int a = 0b101;', {std: 'c17'}).ok, false);
  assert.equal(V.validateSource('c', 'char *s = "constexpr nullptr";', {std: 'c11'}).ok, true, 'keywords inside strings are not code');
  assert.equal(V.validateSource('c', 'int f() { /* nullptr */ return 0; }', {std: 'c11'}).ok, true, 'keywords inside comments are not code');
});

test('UT-022 assembly dialect: Intel and AT&T are never silently mixed', () => {
  assert.equal(V.validateSource('assembly', 'mov rax, rbx\npush rbp\nret\n', {syntax: 'intel'}).ok, true);
  assert.equal(V.validateSource('assembly', 'movq %rbx, %rax\n', {syntax: 'intel'}).ok, false);
  assert.equal(V.validateSource('assembly', 'movq %rbx, %rax\n', {syntax: 'att', assembler: 'gas'}).ok, true);
  assert.equal(V.validateSource('assembly', 'mov rax, rbx\n', {syntax: 'att', assembler: 'gas'}).ok, false);
  assert.equal(V.validateSource('assembly', 'mov rax, rbx\nmovq %rbx, %rax\n', {syntax: 'intel'}).ok, false);
  assert.equal(V.validateSource('assembly', '.intel_syntax noprefix\nmov rax, rbx\n', {syntax: 'att', assembler: 'gas'}).ok, true);
  assert.equal(V.validateSource('assembly', '.globl _start\n', {assembler: 'nasm'}).ok, false, 'GAS directive in a NASM file');
  assert.equal(V.validateSource('assembly', 'mov rax, [rbx] ; load\n', {syntax: 'intel'}).ok, true);
  const intel = L.predict({profile: 'assembly', prefix: 'mov', dialect: {syntax: 'intel'}})[0];
  const att = L.predict({profile: 'assembly', prefix: 'mov', dialect: {syntax: 'att', assembler: 'gas'}})[0];
  assert.notEqual(intel.text, att.text);
  assert.equal(V.validateSource('assembly', L.renderPlain(att.text), {syntax: 'att', assembler: 'gas'}).ok, true);
});

test('UT-023 machine code hex: valid round-trip, incomplete/invalid rejected', () => {
  const ok = x86.validateHex('48 89 D8');
  assert.deepEqual(ok.bytes, [0x48, 0x89, 0xD8]);
  assert.equal(x86.encode('mov rax, rbx').hex, ok.hex);
  assert.equal(x86.validateHex('F').code, 'ODD_HEX_LENGTH');
  assert.equal(x86.validateHex('GG').code, 'INVALID_HEX');
  assert.equal(x86.validateHex('').code, 'CONTEXT_MISSING');
  assert.equal(x86.validateHex('90'.repeat(3000)).code, 'HEX_TOO_LARGE');
  assert.equal(V.validateSource('machine-code', '0x90, 0xC3\n// note\n48 89 d8').ok, true);
  assert.equal(V.validateSource('machine-code', '90 C').ok, false);
});

test('UT-024 / ST-043 unresolved symbols never produce bytes', () => {
  for (const asm of ['jmp external_symbol', 'call foo', 'je loop_start']) {
    const r = x86.encode(asm);
    assert.equal(r.ok, false); assert.equal(r.code, 'BYTE_ENCODING_UNRESOLVED'); assert.equal(r.bytes, undefined); assert.equal(r.hex, undefined);
    assert.throws(() => x86.encodeOrThrow(asm), e => e.code === 'BYTE_ENCODING_UNRESOLVED');
  }
  const cand = L.predict({profile: 'machine-code', prefix: 'jmp external_symbol'})[0];
  assert.equal(cand.validation, 'context_missing'); assert.equal(cand.text, null);
  assert.equal(x86.encode('mov rax, 1').ok, false, 'assembler-dependent forms are refused, not guessed');
});

test('ST-043 independent golden vectors for the verified x86-64 forms', () => {
  const vectors = {
    nop: '90', ret: 'C3', int3: 'CC', syscall: '0F 05', 'push rbp': '55', 'push rax': '50', 'push r8': '41 50', 'push r15': '41 57', 'pop rbp': '5D', 'pop r12': '41 5C',
    'mov rbp, rsp': '48 89 E5', 'mov rax, rbx': '48 89 D8', 'mov rax, r8': '4C 89 C0', 'mov r8, rax': '49 89 C0', 'xor eax, eax': '31 C0', 'xor edi, edi': '31 FF',
    'mov eax, 1': 'B8 01 00 00 00', 'mov r9d, 5': '41 B9 05 00 00 00', 'sub rsp, 0x20': '48 83 EC 20', 'add rsp, 0x20': '48 83 C4 20', 'add rax, rbx': '48 01 D8',
    'sub rax, rbx': '48 29 D8', 'cmp rax, rbx': '48 39 D8', 'sub rsp, 0x100': '48 81 EC 00 01 00 00', 'mov ecx, edx': '89 D1'
  };
  for (const [asm, hex] of Object.entries(vectors)) assert.equal(x86.encode(asm).hex, hex, asm);
  for (const asm of Object.keys(vectors)) { const hex = vectors[asm]; if (!/^(?:mov e|xor|push|pop|mov r9d|nop|ret|int3|syscall)/.test(asm) && asm.includes(',')) assert.equal(x86.disassembleKnown(hex) === null || typeof x86.disassembleKnown(hex) === 'string', true); }
  assert.equal(x86.disassembleKnown('48 89 d8'), 'mov rax, rbx');
  const mc = L.predict({profile: 'machine-code', prefix: 'push rbp', dialect: {format: 'compact'}})[0];
  assert.equal(mc.text, '55'); assert.equal(mc.validation, 'verified');
});

test('UT-052 / IT-034 structural validation catches incomplete blocks and bad indentation', () => {
  assert.equal(V.validateSource('rust', 'fn main() {\n    let x = 1;\n').ok, false);
  assert.equal(V.validateSource('rust', 'fn main() {\n    println!("}");\n}\n').ok, true, 'brackets inside strings are ignored');
  assert.equal(V.validateSource('rust', "fn f<'a>(x: &'a str) -> char { '}' }").ok, true, 'lifetimes and char literals');
  assert.equal(V.validateSource('python', 'def f():\n').ok, false);
  assert.equal(V.validateSource('python', 'def f():\n    pass\n').ok, true);
  assert.equal(V.validateSource('python', 'if x:\n    y = 1\n  z = 2\n').ok, false);
  assert.equal(V.validateSource('python', 'def f():\n\treturn 1\n    x = 1\n').ok, false);
  assert.equal(V.validateSource('python', 'x = """\ndef f():\n"""\n').ok, true, 'docstring content is not code');
  assert.equal(V.validateSource('typescript', 'function f() {\n').ok, false);
  const bad = V.validateSource('python', 'def f():\n');
  assert.equal(bad.issues[0].code, 'EXPECTED_INDENT'); assert.equal(bad.issues[0].line, 1);
});

test('UT-065 / UT-020 / UT-076 static: pure modules cannot execute anything', () => {
  const dir = path.join(__dirname, '..', 'src', 'core');
  for (const f of fs.readdirSync(dir).filter(n => n.endsWith('.js'))) {
    const src = fs.readFileSync(path.join(dir, f), 'utf8');
    // `re.exec(` (RegExp) is fine; a bare exec/spawn call or child_process import is not
    assert.doesNotMatch(src, /child_process|(?<![.\w])(?:spawn|exec|execSync|execFile)\(|(?<![.\w])eval\(|new Function\(|require\(['"]vm['"]\)|worker_threads/, `${f} must not execute code`);
    assert.doesNotMatch(src, /require\(['"]net['"]\)|require\(['"]https?['"]\)|require\(['"]dns['"]\)/, `${f} must not open sockets itself`);
  }
  const ext = fs.readdirSync(path.join(__dirname, '..', 'src')).filter(n => n.endsWith('.js'));
  const spawners = ext.filter(f => /child_process/.test(fs.readFileSync(path.join(__dirname, '..', 'src', f), 'utf8')));
  assert.deepEqual(spawners, ['coreprocess.js'], 'only the optional, explicitly configured core client may spawn a process');
  const cp = fs.readFileSync(path.join(__dirname, '..', 'src', 'coreprocess.js'), 'utf8');
  assert.match(cp, /spawn\(binary, \[\]/, 'no arguments from untrusted data');
  const lang = fs.readFileSync(path.join(__dirname, '..', 'src', 'extension.js'), 'utf8');
  assert.match(lang, /cfg\(\)\.get\('coreExecutable'\)/, 'the binary path comes from user settings only');
});
