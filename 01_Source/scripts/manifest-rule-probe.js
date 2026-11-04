'use strict';
// Diagnostic for MANIFEST_SHA256.txt: which normalisation reproduces the hashes in the manifest?
//
// The answer was measured, not guessed (see 02_Verification/FRAMEWORK_GAPS.md):
//   * a text file is listed as its LF-normalised content (= the bytes of its git blob);
//   * a binary file (.xlsx, .png, .ico) is listed as it is.
// The legacy forms are shown too, because the entries written before the rule was fixed use them.
//
//   node scripts/manifest-rule-probe.js
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..', '..');
const hash = buffer => crypto.createHash('sha256').update(buffer).digest('hex');
const isBinary = bytes => bytes.includes(0);
const rules = {
  'the rule (LF for text, raw for binary)': bytes =>
    isBinary(bytes)
      ? hash(bytes)
      : hash(Buffer.from(bytes.toString('utf8').replace(/\r\n/g, '\n'), 'utf8')),
  'raw bytes of the working tree': bytes => hash(bytes),
  'LF for every file': bytes =>
    hash(Buffer.from(bytes.toString('utf8').replace(/\r\n/g, '\n'), 'utf8')),
  'CRLF for every file': bytes =>
    hash(Buffer.from(bytes.toString('utf8').replace(/\r?\n/g, '\r\n'), 'utf8')),
};

const lines = fs.readFileSync(path.join(root, 'MANIFEST_SHA256.txt'), 'utf8').split(/\r?\n/).filter(l => l.trim());
for (const [name, rule] of Object.entries(rules)) {
  let ok = 0;
  let stale = 0;
  let missing = 0;
  const staleFiles = [];
  for (const line of lines) {
    const match = /^([0-9a-f]{64})\s+(.+)$/.exec(line.trim());
    if (!match) continue;
    const file = path.join(root, match[2]);
    if (!fs.existsSync(file)) {
      missing++;
      continue;
    }
    if (rule(fs.readFileSync(file)) === match[1]) ok++;
    else {
      stale++;
      staleFiles.push(match[2]);
    }
  }
  console.log(`${name}: ok=${ok} stale=${stale} missing=${missing}`);
  if (stale && stale <= 30) console.log('   ' + staleFiles.join('\n   '));
}
