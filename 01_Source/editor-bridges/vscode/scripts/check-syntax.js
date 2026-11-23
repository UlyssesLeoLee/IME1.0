'use strict';
// Syntax-checks every JavaScript source file (replaces the hard-coded v0.1 list).
const {execFileSync} = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const root = path.join(__dirname, '..');
// the extension sources, every script under 01_Source/tests and every script under 01_Source/scripts
// (the scripts of the gate itself: a syntax error in one of them would break the gate without anybody noticing)
const roots = [root, path.join(root, '..', '..', 'tests'), path.join(root, '..', '..', 'scripts')];
const files = [];
(function walk(dir) {
  for (const entry of fs.readdirSync(dir, {withFileTypes: true})) {
    if (entry.name === 'node_modules' || entry.name.startsWith('.')) continue;
    const p = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(p); else if (entry.name.endsWith('.js')) files.push(p);
  }
})(roots[0]);
for (const extra of roots.slice(1)) if (fs.existsSync(extra)) (function walkTests(dir) {
  for (const entry of fs.readdirSync(dir, {withFileTypes: true})) {
    if (entry.name === 'node_modules' || entry.name.startsWith('.')) continue;
    const p = path.join(dir, entry.name);
    if (entry.isDirectory()) walkTests(p); else if (entry.name.endsWith('.js')) files.push(p);
  }
})(extra);
let failed = 0;
for (const f of files) {
  try { execFileSync(process.execPath, ['--check', f], {stdio: 'pipe'}); }
  catch (e) { failed++; console.error(`SYNTAX ERROR ${path.relative(root, f)}\n${e.stderr}`); }
}
console.log(`syntax check: ${files.length - failed}/${files.length} files OK`);
process.exit(failed ? 1 : 0);
