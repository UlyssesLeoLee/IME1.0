'use strict';
// glue-check (P0 of docs/RUST_MIGRATION_PLAN.md): measures how much hand-written JavaScript is left in the product and
// enforces the per-phase budget so that logic cannot drift back into the VS Code glue.
//   node scripts/glue-check.js               report only (always exit 0)
//   node scripts/glue-check.js --enforce p1  fail if the P1 budget is exceeded
//   node scripts/glue-check.js --enforce p6  fail if the final (B level) budget is exceeded
//   node scripts/glue-check.js --golden      fail if the live fixtures lost or changed a frozen golden-v0.2 case
// "Hand-written product JS" = editor-bridges/vscode/src/**/*.js (tests, scripts, uat are tooling, not product).
// A file whose first 5 lines contain "@generated" (wasm-bindgen output) is counted separately and never against the limit.
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const srcDir = path.join(root, 'editor-bridges', 'vscode', 'src');
const BUDGET = {
  p1: {maxLines: 700, noCoreDir: true, allowFiles: null},
  p6: {maxLines: 150, noCoreDir: true, allowFiles: ['extension.js']}
};
// business-logic markers that must not appear in glue once the logic lives in Rust
const FORBIDDEN = /createHash|node:crypto|\bsha256\b|buildHunks|applyHunks|EditCoordinator|PermissionBroker|negotiateEffort|parseModelResult|validateSource|resolveLanguage|assessOutbound/;

function walk(dir, out = []) {
  if (!fs.existsSync(dir)) return out;
  for (const e of fs.readdirSync(dir, {withFileTypes: true})) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p, out); else if (e.name.endsWith('.js')) out.push(p);
  }
  return out;
}
function measure() {
  const files = walk(srcDir).map(f => {
    const text = fs.readFileSync(f, 'utf8');
    return {file: path.relative(srcDir, f).split(path.sep).join('/'), lines: text.split(/\r?\n/).length - (text.endsWith('\n') ? 1 : 0),
      generated: /@generated/.test(text.split(/\r?\n/).slice(0, 5).join('\n')), forbidden: FORBIDDEN.test(text)};
  });
  const hand = files.filter(f => !f.generated);
  return {files, handLines: hand.reduce((n, f) => n + f.lines, 0), generatedLines: files.filter(f => f.generated).reduce((n, f) => n + f.lines, 0),
    coreFiles: files.filter(f => f.file.startsWith('core/')).length, forbiddenFiles: hand.filter(f => f.forbidden).map(f => f.file)};
}

const args = process.argv.slice(2);
if (args.includes('--golden')) {
  const gold = path.join(root, 'tests', 'golden-v0.2'), live = path.join(root, 'tests', 'fixtures');
  let bad = 0, checked = 0;
  const key = (name, c) => name === 'x86-vectors.json' ? c.asm : name === 'resolve-cases.json' ? JSON.stringify([c.ctx, c.current, c.lock])
    : name === 'secret-cases.json' ? JSON.stringify([c.text, c.fileName]) : JSON.stringify([c.profile, c.prefix, c.before, c.after, c.text, c.dialect]);
  for (const name of ['predict-cases.json', 'validate-cases.json', 'x86-vectors.json', 'resolve-cases.json', 'secret-cases.json']) {
    const g = JSON.parse(fs.readFileSync(path.join(gold, name), 'utf8')).cases, l = new Map(JSON.parse(fs.readFileSync(path.join(live, name), 'utf8')).cases.map(c => [key(name, c), c]));
    for (const c of g) {
      checked++;
      const hit = l.get(key(name, c));
      if (!hit) { bad++; console.error(`golden case missing in live fixtures: ${name} ${key(name, c).slice(0, 60)}`); continue; }
      if (JSON.stringify(hit.expect ?? hit.hex ?? hit.code) !== JSON.stringify(c.expect ?? c.hex ?? c.code)) { bad++; console.error(`golden case changed: ${name} ${key(name, c).slice(0, 60)}`); }
    }
  }
  console.log(`golden-v0.2: ${checked - bad}/${checked} frozen cases unchanged in the live fixtures`);
  process.exit(bad ? 1 : 0);
}

const m = measure();
console.log(`hand-written product JS: ${m.handLines} lines in ${m.files.filter(f => !f.generated).length} files (generated: ${m.generatedLines} lines)`);
console.log(`files under src/core: ${m.coreFiles}; files with business-logic markers: ${m.forbiddenFiles.length}${m.forbiddenFiles.length ? ' (' + m.forbiddenFiles.join(', ') + ')' : ''}`);
for (const f of [...m.files].sort((a, b) => b.lines - a.lines)) console.log(`  ${String(f.lines).padStart(5)}  ${f.file}${f.generated ? '  [generated]' : ''}${f.forbidden ? '  [logic markers]' : ''}`);

const i = args.indexOf('--enforce');
if (i >= 0) {
  const phase = args[i + 1], b = BUDGET[phase];
  if (!b) { console.error(`unknown phase "${phase}" (p1 | p6)`); process.exit(2); }
  const problems = [];
  if (m.handLines > b.maxLines) problems.push(`${m.handLines} hand-written lines exceed the ${phase} limit of ${b.maxLines}`);
  if (b.noCoreDir && m.coreFiles) problems.push(`${m.coreFiles} JS files remain under src/core`);
  if (m.forbiddenFiles.length) problems.push(`business-logic markers in: ${m.forbiddenFiles.join(', ')}`);
  if (b.allowFiles) { const extra = m.files.filter(f => !f.generated && !b.allowFiles.includes(f.file)).map(f => f.file); if (extra.length) problems.push(`files outside the ${phase} whitelist: ${extra.join(', ')}`); }
  if (problems.length) { console.error(`glue-check FAILED for ${phase}:\n - ${problems.join('\n - ')}`); process.exit(1); }
  console.log(`glue-check OK for ${phase}`);
}
