'use strict';
// Prints what matters about a Windows PE file (exe/dll) without any native toolchain: target machine, subsystem,
// exported function names and imported DLL names. Used to check release artefacts (x64, no unexpected DLL imports).
// Usage: node scripts/pe-info.js <file> [--json]
const fs = require('node:fs');

const MACHINES = {0x8664: 'x64 (AMD64)', 0x14c: 'x86 (i386)', 0xaa64: 'ARM64'};

function parsePe(buf) {
  if (buf.readUInt16LE(0) !== 0x5a4d) throw new Error('not a PE file (no MZ header)');
  const pe = buf.readUInt32LE(0x3c);
  if (buf.readUInt32LE(pe) !== 0x00004550) throw new Error('not a PE file (no PE signature)');
  const machine = buf.readUInt16LE(pe + 4);
  const sections = buf.readUInt16LE(pe + 6);
  const optSize = buf.readUInt16LE(pe + 20);
  const characteristics = buf.readUInt16LE(pe + 22);
  const opt = pe + 24;
  const magic = buf.readUInt16LE(opt);
  if (magic !== 0x20b) throw new Error(`unsupported optional header magic 0x${magic.toString(16)} (PE32+ expected)`);
  const subsystem = buf.readUInt16LE(opt + 68);
  const dir = i => ({rva: buf.readUInt32LE(opt + 112 + i * 8), size: buf.readUInt32LE(opt + 112 + i * 8 + 4)});
  const table = opt + optSize;
  const secs = [];
  for (let i = 0; i < sections; i++) {
    const o = table + i * 40;
    secs.push({va: buf.readUInt32LE(o + 12), vsize: buf.readUInt32LE(o + 8), raw: buf.readUInt32LE(o + 20), rawSize: buf.readUInt32LE(o + 16)});
  }
  const off = rva => {
    const s = secs.find(x => rva >= x.va && rva < x.va + Math.max(x.vsize, x.rawSize));
    if (!s) throw new Error(`RVA 0x${rva.toString(16)} is outside every section`);
    return s.raw + (rva - s.va);
  };
  const cstr = o => { let e = o; while (buf[e] !== 0) e++; return buf.toString('latin1', o, e); };
  const exports = [];
  const ex = dir(0);
  if (ex.rva) {
    const e = off(ex.rva);
    const count = buf.readUInt32LE(e + 24), names = off(buf.readUInt32LE(e + 32));
    for (let i = 0; i < count; i++) exports.push(cstr(off(buf.readUInt32LE(names + i * 4))));
  }
  const imports = [];
  const im = dir(1);
  if (im.rva) {
    for (let e = off(im.rva); buf.readUInt32LE(e + 12) !== 0; e += 20) imports.push(cstr(off(buf.readUInt32LE(e + 12))).toLowerCase());
  }
  return {
    machineCode: machine, machine: MACHINES[machine] || `unknown 0x${machine.toString(16)}`,
    isDll: (characteristics & 0x2000) !== 0, subsystem: ({2: 'windows GUI', 3: 'console'})[subsystem] || String(subsystem),
    exports: exports.sort(), imports: [...new Set(imports)].sort(),
  };
}

if (require.main === module) {
  const file = process.argv[2];
  if (!file) { console.error('usage: node scripts/pe-info.js <file> [--json]'); process.exit(2); }
  const info = parsePe(fs.readFileSync(file));
  if (process.argv.includes('--json')) console.log(JSON.stringify(info, null, 1));
  else console.log(`${file}\n  machine:  ${info.machine}\n  kind:     ${info.isDll ? 'DLL' : 'EXE'} (${info.subsystem})\n  exports:  ${info.exports.join(', ') || '-'}\n  imports:  ${info.imports.join(', ')}`);
}
module.exports = {parsePe};
