'use strict';
// Builds the Windows x64 installer: dist/ProgrammingIME-<version>-win-x64-setup.exe (+ .sha256).
//
//   node scripts/build-installer.js [<version>] [--skip-dry-run]
//
// The version defaults to the one in editor-bridges/vscode/package.json. Steps: release builds of pime-core and pime-tsf
// (x86_64-pc-windows-gnu), the .vsix (scripts/package-vsix.js), the pime-setup stub, then the payload (the four files plus
// meta.json) is appended to the stub in the format documented in crates/pime-setup/src/payload.rs. Finally the result is
// checked as a PE file (x64, system DLL imports only) and `--dry-run` proves that the exe finds and verifies its payload.
// Nothing is installed or registered: the installer is never run except with --dry-run.
//
// Needs: cargo (1.99, gnu host), node >= 22, and network access for @vscode/vsce the first time (see package-vsix.js).
// The installer is NOT code signed: Windows SmartScreen will warn about it.
const {spawnSync} = require('node:child_process');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {parsePe} = require('./pe-info.js');
const {packageVsix} = require('./package-vsix.js');

const ROOT = path.resolve(__dirname, '..');
const DIST = path.join(ROOT, 'dist');
const TARGET = 'x86_64-pc-windows-gnu';
const targetDir = process.env.CARGO_TARGET_DIR || path.join(os.tmpdir(), 'pime-cargo-target');
const release = path.join(targetDir, TARGET, 'release');
// system DLLs present on every Windows 10/11 x64 installation (no MinGW runtime DLL a user would not have)
const ALLOWED_IMPORT = /^(kernel32|ntdll|msvcrt|advapi32|user32|shell32|ole32|oleaut32|gdi32|userenv|ws2_32|bcryptprimitives|propsys|msctf|api-ms-win-[a-z0-9-]+)\.dll$/;
const MAGIC = Buffer.from('PIME-PAYLOAD-V1\0', 'latin1');
const sha256 = data => crypto.createHash('sha256').update(data).digest();

function fail(message) {
  console.error(`\nbuild-installer FAILED: ${message}`);
  process.exit(1);
}

function cargo(cwd, args, label) {
  console.log(`\n== ${label}: cargo ${args.join(' ')}  (${path.relative(ROOT, cwd) || '.'})`);
  for (let attempt = 1; attempt <= 3; attempt++) { // a freshly linked file can fail once while Defender scans it
    const r = spawnSync('cargo', args, {cwd, encoding: 'utf8', env: {...process.env, CARGO_TARGET_DIR: targetDir}});
    const text = `${r.stdout || ''}${r.stderr || ''}`;
    if (r.status === 0) { console.log(text.trim().split(/\r?\n/).slice(-2).join('\n')); return; }
    if (!/os error 5/.test(text) || attempt === 3) fail(`${label} failed (exit ${r.status}${r.error ? `, ${r.error.message}` : ''})\n${text}`);
    console.log('  (os error 5, probably a virus scanner: retrying)');
  }
}

// The payload format of crates/pime-setup/src/payload.rs
function packPayload(entries) {
  const parts = [];
  const u32 = Buffer.alloc(4); u32.writeUInt32LE(entries.length); parts.push(u32);
  for (const [name, data] of entries) {
    const nameBytes = Buffer.from(name, 'utf8');
    const nameLen = Buffer.alloc(2); nameLen.writeUInt16LE(nameBytes.length);
    const dataLen = Buffer.alloc(8); dataLen.writeBigUInt64LE(BigInt(data.length));
    parts.push(nameLen, nameBytes, dataLen, sha256(data), data);
  }
  const blob = Buffer.concat(parts);
  const blobLen = Buffer.alloc(8); blobLen.writeBigUInt64LE(BigInt(blob.length));
  return Buffer.concat([blob, MAGIC, blobLen, sha256(blob)]);
}

// Independent reader of the same format (so a packing bug cannot verify itself through the Rust side only)
function readPayload(file) {
  const exe = fs.readFileSync(file);
  const footer = exe.length - 56;
  if (!exe.subarray(footer, footer + 16).equals(MAGIC)) throw new Error('no payload footer at the end of the file');
  const blobLen = Number(exe.readBigUInt64LE(footer + 16));
  const blob = exe.subarray(footer - blobLen, footer);
  if (!sha256(blob).equals(exe.subarray(footer + 24, footer + 56))) throw new Error('payload hash does not match the footer');
  const entries = [];
  let at = 4;
  for (let i = 0; i < blob.readUInt32LE(0); i++) {
    const nameLen = blob.readUInt16LE(at); at += 2;
    const name = blob.toString('utf8', at, at + nameLen); at += nameLen;
    const size = Number(blob.readBigUInt64LE(at)); at += 8;
    const hash = blob.subarray(at, at + 32); at += 32;
    const data = blob.subarray(at, at + size); at += size;
    if (!sha256(data).equals(hash)) throw new Error(`hash of ${name} does not match`);
    entries.push({name, size, sha256: hash.toString('hex')});
  }
  if (at !== blob.length) throw new Error('bytes remain after the last payload entry');
  return {entries};
}

function main() {
  const args = process.argv.slice(2);
  const packageJson = JSON.parse(fs.readFileSync(path.join(ROOT, 'editor-bridges', 'vscode', 'package.json'), 'utf8'));
  const requested = args.find(a => !a.startsWith('--')) || (args.includes('--version') ? args[args.indexOf('--version') + 1] : undefined);
  const version = requested || packageJson.version;
  if (!/^[0-9A-Za-z][0-9A-Za-z.+-]{0,31}$/.test(version)) fail(`"${version}" is not a usable version (letters, digits, . + - only)`);
  fs.mkdirSync(DIST, {recursive: true});
  console.log(`Programming IME installer ${version}\n  cargo target dir: ${targetDir}`);

  cargo(ROOT, ['build', '--release', '--target', TARGET, '-p', 'pime-core', '--bin', 'pime-core'], 'pime-core');
  cargo(path.join(ROOT, 'crates', 'pime-tsf'), ['build', '--release', '--target', TARGET], 'pime-tsf (text service DLL)');
  console.log('\n== VS Code extension');
  let vsix;
  try { vsix = packageVsix({outDir: DIST, log: m => console.log(`  ${m}`)}); } catch (e) { fail(e.message); }
  cargo(path.join(ROOT, 'crates', 'pime-setup'), ['build', '--release', '--target', TARGET], 'pime-setup (installer stub)');

  const parts = {
    dll: path.join(release, 'pime_tsf.dll'), core: path.join(release, 'pime-core.exe'), stub: path.join(release, 'pime-setup.exe'),
    license: path.join(ROOT, 'LICENSE'),
  };
  for (const [k, p] of Object.entries(parts)) if (!fs.existsSync(p)) fail(`missing ${k}: ${p}`);
  const entries = [
    ['pime_tsf.dll', fs.readFileSync(parts.dll)],
    ['pime-core.exe', fs.readFileSync(parts.core)],
    ['LICENSE', fs.readFileSync(parts.license)],
    ['programming-ime.vsix', fs.readFileSync(vsix.file)],
    ['meta.json', Buffer.from(JSON.stringify({version, extension_id: vsix.id}), 'utf8')],
  ];
  const out = path.join(DIST, `ProgrammingIME-${version}-win-x64-setup.exe`);
  const stub = fs.readFileSync(parts.stub);
  fs.writeFileSync(out, Buffer.concat([stub, packPayload(entries)]));
  const outBytes = fs.readFileSync(out);
  const digest = sha256(outBytes).toString('hex');
  fs.writeFileSync(`${out}.sha256`, `${digest} *${path.basename(out)}\n`);

  console.log('\n== checks on the result');
  let failed = 0;
  const check = (ok, text) => { console.log(`${ok ? 'ok  ' : 'FAIL'} ${text}`); if (!ok) failed++; };
  const info = parsePe(outBytes);
  check(info.machineCode === 0x8664, `machine is x64 (0x8664), found ${info.machine}`);
  check(!info.isDll && info.subsystem === 'console', `an EXE for the ${info.subsystem} subsystem`);
  const unexpected = info.imports.filter(n => !ALLOWED_IMPORT.test(n));
  check(unexpected.length === 0, `imports only Windows system DLLs (${info.imports.join(', ')})${unexpected.length ? `; unexpected: ${unexpected.join(', ')}` : ''}`);
  let payload;
  try { payload = readPayload(out); check(true, `payload footer and hashes verify (${payload.entries.length} entries, read by an independent reader)`); }
  catch (e) { check(false, `payload: ${e.message}`); }
  if (!args.includes('--skip-dry-run')) {
    const probe = path.join(os.tmpdir(), `pime-dry-run-${process.pid}`); // must stay absent: a dry run changes nothing
    const r = spawnSync(out, ['--dry-run', '--prefix', probe], {encoding: 'utf8'});
    const text = `${r.stdout || ''}${r.stderr || ''}`;
    check(r.status === 0 && text.includes(`install plan (dry run: nothing is changed)`) && text.includes(`version ${version}, SHA-256 of every file verified`), `--dry-run finds and verifies the payload (exit ${r.status})`);
    check(!fs.existsSync(probe), '--dry-run created nothing');
    if (r.status !== 0) console.log(text);
  }
  if (failed) fail(`${failed} check(s) failed`);

  const size = f => fs.statSync(f).size;
  console.log(`\nbuild-installer: OK
  installer   ${out}
              ${outBytes.length} bytes   sha256 ${digest}   (${path.basename(out)}.sha256 written)
  stub        ${size(parts.stub)} bytes (pime-setup.exe, x64 console, unsigned)
  payload     ${payload.entries.map(e => `${e.name} ${e.size}`).join(', ')}
  extension   ${path.basename(vsix.file)} ${vsix.size} bytes, ${vsix.files.length} files, id ${vsix.id}
  NOT signed (SmartScreen will warn). NOT run: the text service DLL is unverified in a VM, so this is a pre-release.`);
}

main();
