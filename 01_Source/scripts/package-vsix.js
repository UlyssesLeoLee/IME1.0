'use strict';
// Builds the VS Code extension package (.vsix) and verifies it.
//
//   node scripts/package-vsix.js [--out <dir>]        (default output folder: 01_Source/dist)
//
// Why a staging folder: the extension reads its rule packs from <extension root>/language-packs (src/core/languages.js), but in
// the repository that folder lives next to the extension (01_Source/language-packs). Packaging the extension folder as it is
// would produce a .vsix whose rules cannot be found. The script therefore copies exactly what the extension needs into
// dist/vsix-staging and packages that; nothing in the source tree is modified.
//
// vsce is fetched with `npx --yes @vscode/vsce@<exact version>`: it needs access to the npm registry the first time.
// The version is pinned on purpose: 4.0.0 is the newest stable release that was at least two weeks old on 2026-10-30
// (4.1.0 was published 2026-10-28). Raise it deliberately, after reading the release notes.
const {spawnSync} = require('node:child_process');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const zlib = require('node:zlib');

const VSCE_VERSION = '4.0.0';
const ROOT = path.resolve(__dirname, '..');
const EXT = path.join(ROOT, 'editor-bridges', 'vscode');
const DEFAULT_OUT = path.join(ROOT, 'dist');

// ---------------------------------------------------------------- staging

function copyTree(from, to, skip = () => false) {
  fs.mkdirSync(to, {recursive: true});
  for (const entry of fs.readdirSync(from, {withFileTypes: true})) {
    const src = path.join(from, entry.name);
    if (skip(entry, src)) continue;
    if (entry.isDirectory()) copyTree(src, path.join(to, entry.name), skip);
    else if (entry.isFile()) fs.copyFileSync(src, path.join(to, entry.name));
  }
}

function stage(stagingDir, log) {
  fs.rmSync(stagingDir, {recursive: true, force: true});
  fs.mkdirSync(stagingDir, {recursive: true});
  const files = ['package.json', 'LICENSE', '.vscodeignore', ...fs.readdirSync(EXT).filter(n => /^package\.nls(\..+)?\.json$/.test(n))];
  for (const f of files) {
    if (!fs.existsSync(path.join(EXT, f))) throw new Error(`the extension folder has no ${f}`);
    fs.copyFileSync(path.join(EXT, f), path.join(stagingDir, f));
  }
  copyTree(path.join(EXT, 'src'), path.join(stagingDir, 'src'));
  if (fs.existsSync(path.join(EXT, 'media'))) copyTree(path.join(EXT, 'media'), path.join(stagingDir, 'media'));
  else log('note: the extension has no media/ folder yet (P6 produces it); the .vsix is built without one');
  const packs = path.join(ROOT, 'language-packs');
  if (!fs.existsSync(path.join(packs, 'rust.json'))) throw new Error(`${packs} has no rule packs`);
  copyTree(packs, path.join(stagingDir, 'language-packs'));
}

// ---------------------------------------------------------------- running vsce

function npxInvocation() {
  // `npx` is a .cmd shim on Windows, which Node refuses to spawn without a shell; run npm's own script with node instead.
  const cli = path.join(path.dirname(process.execPath), 'node_modules', 'npm', 'bin', 'npx-cli.js');
  if (fs.existsSync(cli)) return {command: process.execPath, args: [cli]};
  return {command: 'npx', args: [], shell: process.platform === 'win32'};
}

function runVsce(stagingDir, outFile, log) {
  const inv = npxInvocation();
  const args = [...inv.args, '--yes', `@vscode/vsce@${VSCE_VERSION}`, 'package', '--no-dependencies', '--allow-missing-repository', '--out', outFile];
  log(`vsce ${VSCE_VERSION}: npx ${args.slice(inv.args.length).join(' ')}`);
  const r = spawnSync(inv.command, args, {cwd: stagingDir, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 5 * 60 * 1000, shell: inv.shell});
  const output = `${r.stdout || ''}${r.stderr || ''}`.trim();
  if (r.status !== 0 || !fs.existsSync(outFile)) {
    throw new Error(`@vscode/vsce@${VSCE_VERSION} did not produce ${outFile} (exit ${r.status}${r.error ? `, ${r.error.message}` : ''}).\n` +
      'It is fetched with npx and needs access to the npm registry; check the network and the npm configuration.\n' + output);
  }
  if (output) log(output.split(/\r?\n/).map(l => `  | ${l}`).join('\n'));
}

// ---------------------------------------------------------------- reading a zip (no tool needed)

function readZip(buf) {
  let eocd = -1;
  for (let i = buf.length - 22; i >= Math.max(0, buf.length - 22 - 65535); i--) if (buf.readUInt32LE(i) === 0x06054b50) { eocd = i; break; }
  if (eocd < 0) throw new Error('not a zip file (no end-of-central-directory record)');
  const total = buf.readUInt16LE(eocd + 10);
  if (total === 0xffff) throw new Error('zip64 archives are not supported by this checker');
  let p = buf.readUInt32LE(eocd + 16);
  const entries = [];
  for (let i = 0; i < total; i++) {
    if (buf.readUInt32LE(p) !== 0x02014b50) throw new Error('damaged central directory');
    const e = {
      method: buf.readUInt16LE(p + 10), crc: buf.readUInt32LE(p + 16), size: buf.readUInt32LE(p + 24), compressed: buf.readUInt32LE(p + 20),
      local: buf.readUInt32LE(p + 42), name: buf.toString('utf8', p + 46, p + 46 + buf.readUInt16LE(p + 28)),
    };
    p += 46 + buf.readUInt16LE(p + 28) + buf.readUInt16LE(p + 30) + buf.readUInt16LE(p + 32);
    entries.push(e);
  }
  const read = e => {
    if (buf.readUInt32LE(e.local) !== 0x04034b50) throw new Error(`damaged local header of ${e.name}`);
    const start = e.local + 30 + buf.readUInt16LE(e.local + 26) + buf.readUInt16LE(e.local + 28);
    const raw = buf.subarray(start, start + e.compressed);
    const data = e.method === 0 ? raw : e.method === 8 ? zlib.inflateRawSync(raw) : (() => { throw new Error(`unsupported compression ${e.method} in ${e.name}`); })();
    if (data.length !== e.size || zlib.crc32(data) !== e.crc) throw new Error(`${e.name} is corrupt (size or CRC mismatch)`);
    return data;
  };
  return {entries, read};
}

// ---------------------------------------------------------------- verification

// Escaping paths that are known and harmless: a development-layout fallback that is only tried after the in-package candidate.
const KNOWN_ESCAPES = [{file: 'extension/src/core/languages.js', target: 'language-packs', note: 'development-layout fallback of findPackDir() (the in-package language-packs/ is tried first)'}];

function resolveRef(fileInZip, segments) {
  return path.posix.normalize(path.posix.join(path.posix.dirname(fileInZip), ...segments));
}

function verifyVsix(file, {expectedVersion, log = console.log} = {}) {
  const problems = [];
  const fail = m => { problems.push(m); log(`FAIL ${m}`); };
  const ok = m => log(`ok   ${m}`);
  const buf = fs.readFileSync(file);
  const zip = readZip(buf);
  const names = zip.entries.map(e => e.name).filter(n => !n.endsWith('/'));
  const set = new Set(names);
  log(`${path.basename(file)}: ${buf.length} bytes, ${names.length} files`);
  for (const n of names.slice().sort()) log(`       ${n}`);

  const bad = names.filter(n => n.startsWith('/') || n.includes('\\') || n.split('/').includes('..'));
  bad.length ? fail(`unsafe entry names: ${bad.join(', ')}`) : ok('every entry stays inside the archive');

  // contents the extension needs
  const need = ['[Content_Types].xml', 'extension.vsixmanifest', 'extension/package.json', 'extension/src/extension.js'];
  const nls = fs.readdirSync(EXT).filter(n => /^package\.nls(\..+)?\.json$/.test(n)).map(n => `extension/${n}`);
  const packs = fs.readdirSync(path.join(ROOT, 'language-packs')).filter(n => n.endsWith('.json')).map(n => `extension/language-packs/${n}`);
  const missing = [...need, ...nls, ...packs].filter(n => !set.has(n));
  if (!names.some(n => /^extension\/LICENSE(\.txt|\.md)?$/.test(n))) missing.push('extension/LICENSE');
  const srcFiles = []; (function walk(d, rel) { for (const e of fs.readdirSync(d, {withFileTypes: true})) e.isDirectory() ? walk(path.join(d, e.name), `${rel}/${e.name}`) : srcFiles.push(`${rel}/${e.name}`); })(path.join(EXT, 'src'), 'extension/src');
  missing.push(...srcFiles.filter(n => !set.has(n)));
  if (fs.existsSync(path.join(EXT, 'media')) && !names.some(n => n.startsWith('extension/media/'))) missing.push('extension/media/*');
  missing.length ? fail(`missing from the package: ${missing.join(', ')}`) : ok(`contains src/ (${srcFiles.length} files), package.nls*.json (${nls.length}), LICENSE, language-packs/ (${packs.length})`);
  if (!names.some(n => n.startsWith('extension/media/'))) log('note: no media/ in the package (the extension folder has none yet)');
  const forbidden = names.filter(n => /^extension\/(scripts|\.vscode|node_modules|tests?)\//.test(n) || /\.map$/.test(n));
  forbidden.length ? fail(`must not be packaged: ${forbidden.join(', ')}`) : ok('no scripts/, .vscode/, node_modules/, tests or source maps');

  // version
  const entry = n => zip.entries.find(e => e.name === n);
  const pkg = JSON.parse(zip.read(entry('extension/package.json')).toString('utf8'));
  const sourcePkg = JSON.parse(fs.readFileSync(path.join(EXT, 'package.json'), 'utf8'));
  const want = expectedVersion || sourcePkg.version;
  pkg.version === want && pkg.version === sourcePkg.version ? ok(`package.json version ${pkg.version}`) : fail(`package.json version is ${pkg.version}, expected ${want} (source: ${sourcePkg.version})`);
  const manifestXml = zip.read(entry('extension.vsixmanifest')).toString('utf8');
  manifestXml.includes(`Version="${want}"`) && manifestXml.includes(`Id="${sourcePkg.name}"`) && manifestXml.includes(`Publisher="${sourcePkg.publisher}"`)
    ? ok(`extension.vsixmanifest: ${sourcePkg.publisher}.${sourcePkg.name} ${want}`) : fail('extension.vsixmanifest does not carry the expected id/publisher/version');

  // extract every file (CRC checked by the reader) into a temp folder, syntax-check JS, check references, load the rule packs
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'pime-vsix-'));
  try {
    for (const n of names) { const dest = path.join(tmp, ...n.split('/')); fs.mkdirSync(path.dirname(dest), {recursive: true}); fs.writeFileSync(dest, zip.read(entry(n))); }
    const js = names.filter(n => n.endsWith('.js'));
    const syntax = js.filter(n => spawnSync(process.execPath, ['--check', path.join(tmp, ...n.split('/'))], {encoding: 'utf8'}).status !== 0);
    syntax.length ? fail(`syntax errors in: ${syntax.join(', ')}`) : ok(`${js.length} JavaScript files pass node --check`);

    const refProblems = [];
    const notes = [];
    for (const n of js) {
      const text = fs.readFileSync(path.join(tmp, ...n.split('/')), 'utf8');
      for (const m of text.matchAll(/require\(\s*['"](\.{1,2}\/[^'"]*)['"]\s*\)/g)) {
        const target = resolveRef(n, [m[1]]);
        const found = [target, `${target}.js`, `${target}.json`, `${target}/index.js`].some(c => set.has(c));
        if (!found) refProblems.push(`${n} requires ${m[1]} which is not in the package`);
      }
      for (const m of text.matchAll(/path\.(?:join|resolve)\(\s*__dirname((?:\s*,\s*'[^']*')*)\s*\)/g)) {
        const segs = [...m[1].matchAll(/'([^']*)'/g)].map(x => x[1]);
        const target = resolveRef(n, segs);
        if (target.startsWith('..') || target.split('/')[0] !== 'extension') {
          const known = KNOWN_ESCAPES.find(k => k.file === n && segs[segs.length - 1] === k.target);
          known ? notes.push(`${n}: ${segs.join('/')} leaves the extension (${known.note})`) : refProblems.push(`${n} builds a path that leaves the extension: __dirname/${segs.join('/')}`);
        } else if (!names.some(x => x === target || x.startsWith(`${target}/`))) refProblems.push(`${n} refers to ${target}, which is not in the package`);
      }
    }
    refProblems.length ? fail(refProblems.join('; ')) : ok('every relative require() and __dirname path resolves inside the package');
    for (const note of notes) log(`note: ${note}`);

    // behaviour: the rule packs load from the package itself (the environment override removed, run from the extracted copy)
    const loader = `const {loadRegistry} = require(${JSON.stringify(path.join(tmp, 'extension', 'src', 'core', 'languages.js'))}); const r = loadRegistry(); console.log(r.packs.size);`;
    const env = {...process.env}; delete env.PIME_LANGUAGE_PACKS;
    const withPacks = spawnSync(process.execPath, ['-e', loader], {encoding: 'utf8', env});
    withPacks.status === 0 && Number(withPacks.stdout.trim()) === packs.length ? ok(`loadRegistry() from the extracted package finds ${packs.length} rule packs`) : fail(`loadRegistry() from the extracted package failed: ${(withPacks.stderr || withPacks.stdout).trim().split('\n')[0]}`);
    fs.renameSync(path.join(tmp, 'extension', 'language-packs'), path.join(tmp, 'extension', 'language-packs.off'));
    const withoutPacks = spawnSync(process.execPath, ['-e', loader], {encoding: 'utf8', env});
    withoutPacks.status !== 0 ? ok('negative control: without the packaged language-packs/ the same call fails (it does not find the repository copy)') : fail('negative control failed: the check passed without the packaged rule packs, so it proves nothing');
  } finally { fs.rmSync(tmp, {recursive: true, force: true}); }

  return {ok: problems.length === 0, problems, files: names, size: buf.length, sha256: crypto.createHash('sha256').update(buf).digest('hex'), version: pkg.version, id: `${sourcePkg.publisher}.${sourcePkg.name}`};
}

// ---------------------------------------------------------------- entry points

function packageVsix({outDir = DEFAULT_OUT, log = console.log} = {}) {
  const pkg = JSON.parse(fs.readFileSync(path.join(EXT, 'package.json'), 'utf8'));
  fs.mkdirSync(outDir, {recursive: true});
  const staging = path.join(outDir, 'vsix-staging');
  const out = path.join(outDir, `${pkg.name}-${pkg.version}.vsix`);
  fs.rmSync(out, {force: true});
  stage(staging, log);
  runVsce(staging, out, log);
  const result = verifyVsix(out, {log});
  fs.rmSync(staging, {recursive: true, force: true});
  if (!result.ok) throw new Error(`the package failed verification:\n  ${result.problems.join('\n  ')}`);
  return {file: out, ...result};
}

if (require.main === module) {
  const i = process.argv.indexOf('--out');
  try {
    const r = packageVsix({outDir: i > 0 ? path.resolve(process.argv[i + 1]) : DEFAULT_OUT});
    console.log(`\npackage-vsix: ${r.file}\n  ${r.size} bytes, ${r.files.length} files, version ${r.version}, extension id ${r.id}\n  sha256 ${r.sha256}`);
  } catch (e) {
    console.error(`\npackage-vsix FAILED: ${e.message}`);
    process.exit(1);
  }
}
module.exports = {packageVsix, verifyVsix, readZip, VSCE_VERSION};
