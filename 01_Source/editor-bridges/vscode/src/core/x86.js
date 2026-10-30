'use strict';
// Deliberately small x86-64 (long mode) encoder: only forms whose encoding is unique and independent of the
// assembler. Anything else is refused (BYTE_ENCODING_UNRESOLVED / unsupported) instead of guessing bytes (FR-052).
// Nothing here executes, assembles to a file, or touches a process.
const {fail} = require('./errors');

const R64 = ['rax', 'rcx', 'rdx', 'rbx', 'rsp', 'rbp', 'rsi', 'rdi', 'r8', 'r9', 'r10', 'r11', 'r12', 'r13', 'r14', 'r15'];
const R32 = ['eax', 'ecx', 'edx', 'ebx', 'esp', 'ebp', 'esi', 'edi', 'r8d', 'r9d', 'r10d', 'r11d', 'r12d', 'r13d', 'r14d', 'r15d'];
const FIXED = Object.freeze({
  nop: [0x90], ret: [0xC3], int3: [0xCC], syscall: [0x0F, 0x05], leave: [0xC9], cpuid: [0x0F, 0xA2],
  hlt: [0xF4], ud2: [0x0F, 0x0B], pause: [0xF3, 0x90], cdq: [0x99], cqo: [0x48, 0x99]
});
const ALU = Object.freeze({add: [0x01, 0], or: [0x09, 1], and: [0x21, 4], sub: [0x29, 5], xor: [0x31, 6], cmp: [0x39, 7], mov: [0x89, -1]});
const LABEL = /^[A-Za-z_.$][\w.$@]*$/;
const BRANCH = /^(jmp|call|j(?:e|ne|z|nz|a|ae|b|be|g|ge|l|le|s|ns|o|no|p|np|c|nc)|loop|loope|loopne)$/;

const hex = bytes => bytes.map(b => b.toString(16).toUpperCase().padStart(2, '0')).join(' ');
const reg64 = name => R64.indexOf(name);
const reg32 = name => R32.indexOf(name);
const le32 = n => [n & 0xFF, (n >>> 8) & 0xFF, (n >>> 16) & 0xFF, (n >>> 24) & 0xFF];
function parseImm(token) {
  const t = token.trim().toLowerCase();
  if (/^-?0x[0-9a-f]+$/.test(t)) return t[0] === '-' ? -parseInt(t.slice(1), 16) : parseInt(t, 16);
  if (/^-?\d+$/.test(t)) return parseInt(t, 10);
  return null;
}
function normalize(text) {
  return String(text).trim().toLowerCase().replace(/\s+/g, ' ').replace(/\s*,\s*/g, ', ');
}
function rexRM(w, reg, rm) {
  const rex = 0x40 | (w ? 8 : 0) | (reg >= 8 ? 4 : 0) | (rm >= 8 ? 1 : 0);
  return (rex !== 0x40 || w) ? [rex] : [];
}
// Returns {ok:true, bytes:[..], hex:'..', form}; {ok:false, code, reason} otherwise. Never throws on user text.
function encode(instruction) {
  const text = normalize(instruction);
  if (!text || text.length > 120) return {ok: false, code: 'UNSUPPORTED_ENCODING', reason: 'empty or too long'};
  if (Object.hasOwn(FIXED, text)) return done(FIXED[text], 'fixed');
  const sp = text.indexOf(' ');
  const mnemonic = sp < 0 ? text : text.slice(0, sp);
  const operands = sp < 0 ? [] : text.slice(sp + 1).split(', ');
  if (BRANCH.test(mnemonic)) {
    if (operands.length === 1 && LABEL.test(operands[0]) && parseImm(operands[0]) === null && !R64.includes(operands[0]) && !R32.includes(operands[0]))
      return {ok: false, code: 'BYTE_ENCODING_UNRESOLVED', reason: 'label/address unknown: relocation or displacement is not determined'};
    return {ok: false, code: 'UNSUPPORTED_ENCODING', reason: 'branch forms other than a symbolic target are not in the verified table'};
  }
  if ((mnemonic === 'push' || mnemonic === 'pop') && operands.length === 1) {
    const r = reg64(operands[0]);
    if (r < 0) return {ok: false, code: 'UNSUPPORTED_ENCODING', reason: 'only 64-bit register operands are supported'};
    return done([...(r >= 8 ? [0x41] : []), (mnemonic === 'push' ? 0x50 : 0x58) + (r & 7)], mnemonic + ' r64');
  }
  if (Object.hasOwn(ALU, mnemonic) && operands.length === 2) {
    const [dst, src] = operands;
    const [opcode, ext] = ALU[mnemonic];
    for (const [table, wide] of [[R64, true], [R32, false]]) {
      const d = table.indexOf(dst), s = table.indexOf(src);
      if (d >= 0 && s >= 0) return done([...rexRM(wide, s, d), opcode, 0xC0 | ((s & 7) << 3) | (d & 7)], `${mnemonic} ${wide ? 'r64' : 'r32'}, ${wide ? 'r64' : 'r32'}`);
    }
    const imm = parseImm(src);
    if (mnemonic === 'mov' && reg32(dst) >= 0 && imm !== null && imm >= 0 && imm <= 0xFFFFFFFF) {
      const d = reg32(dst);
      return done([...(d >= 8 ? [0x41] : []), 0xB8 + (d & 7), ...le32(imm)], 'mov r32, imm32');
    }
    if (mnemonic === 'mov' && reg64(dst) >= 0 && imm !== null)
      return {ok: false, code: 'UNSUPPORTED_ENCODING', reason: 'mov r64, imm has several valid encodings; the choice depends on the assembler'};
    if ((mnemonic === 'add' || mnemonic === 'sub') && reg64(dst) >= 0 && imm !== null) {
      const d = reg64(dst), digit = ext;
      if (imm >= -128 && imm <= 127) return done([...rexRM(true, 0, d), 0x83, 0xC0 | (digit << 3) | (d & 7), imm & 0xFF], `${mnemonic} r64, imm8`);
      if (imm >= -0x80000000 && imm <= 0x7FFFFFFF) return done([...rexRM(true, 0, d), 0x81, 0xC0 | (digit << 3) | (d & 7), ...le32(imm >>> 0)], `${mnemonic} r64, imm32`);
    }
  }
  return {ok: false, code: 'UNSUPPORTED_ENCODING', reason: 'not in the verified x86-64 table (this is not a general assembler)'};
}
function done(bytes, form) { return {ok: true, bytes, hex: hex(bytes), form}; }

// All instruction texts that encode() accepts, used for prefix prediction and reverse (disassembly) preview.
let catalog = null;
function listEncodable() {
  if (catalog) return catalog;
  const out = [];
  for (const k of Object.keys(FIXED)) out.push(k);
  for (const r of R64) { out.push('push ' + r, 'pop ' + r); }
  for (const op of Object.keys(ALU)) {
    for (const d of R64) for (const s of R64) out.push(`${op} ${d}, ${s}`);
    for (const d of R32) for (const s of R32) out.push(`${op} ${d}, ${s}`);
  }
  catalog = out.map(asm => ({asm, hex: encode(asm).hex}));
  return catalog;
}
let reverse = null;
function disassembleKnown(hexText) {
  const key = String(hexText).toUpperCase().replace(/[^0-9A-F]/g, '').match(/.{1,2}/g);
  if (!key) return null;
  const spaced = key.join(' ');
  if (!reverse) { reverse = new Map(); for (const e of listEncodable()) if (!reverse.has(e.hex)) reverse.set(e.hex, e.asm); }
  return reverse.get(spaced) || null;
}
// Hex bytes validation (UT-023): even nibble count, hex digits only, bounded size.
function validateHex(value, {maxBytes = 2048} = {}) {
  const compact = String(value).replace(/\s+/g, '');
  if (compact.length === 0) return {ok: false, code: 'CONTEXT_MISSING', reason: 'empty'};
  if (!/^[0-9a-fA-F]+$/.test(compact)) return {ok: false, code: 'INVALID_HEX', reason: 'non-hex character'};
  if (compact.length % 2) return {ok: false, code: 'ODD_HEX_LENGTH', reason: 'incomplete byte'};
  if (compact.length / 2 > maxBytes) return {ok: false, code: 'HEX_TOO_LARGE', reason: 'too many bytes'};
  const bytes = []; for (let i = 0; i < compact.length; i += 2) bytes.push(parseInt(compact.slice(i, i + 2), 16));
  return {ok: true, bytes, hex: hex(bytes)};
}
// Encoding that must never be presented as "verified" is surfaced as an error.
function encodeOrThrow(instruction) {
  const r = encode(instruction);
  if (r.ok) return r;
  throw fail(r.code === 'BYTE_ENCODING_UNRESOLVED' ? 'BYTE_ENCODING_UNRESOLVED' : 'VALIDATION_FAILED', r.reason);
}
module.exports = {R64, R32, encode, encodeOrThrow, listEncodable, disassembleKnown, validateHex, hex, normalize};
