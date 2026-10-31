'use strict';
// Requirement added by the project owner after the design baseline (no UT/IT ids exist for it): before the extension reads a
// file it looks at the NAME only; a secret-class file (.env and friends) is read only after the user confirms. These tests
// cover the pure parts: the name rules (SEC-xx) and the decision store. Names in here are synthetic.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const {core} = require('./helpers');
const sf = core('secretfile');

const cls = (path, userPatterns) => sf.classifyFile({path, userPatterns});

test('SEC-01 the thinnest slice: .env and its variants are secret, the committed templates are not', () => {
  for (const name of ['.env', '.env.local', '.env.production', '.env.development.local', '.env-dev', '.env_prod']) {
    const r = cls(name);
    assert.deepEqual([r.secret, r.kind, r.rule, r.label], [true, 'env', 'env-file', name], name);
  }
  for (const name of ['.env.example', '.env.sample', '.env.template', '.env.dist', '.env.defaults', '.env.local.example', 'env.ts', 'dotenv.js', '.environment']) {
    assert.equal(cls(name).secret, false, name);
  }
});

test('SEC-02 paths are normalised before anything is decided: separators, case, drive, UNC, trailing dots, streams', () => {
  const same = ['C:\\a\\.env', 'c:/a/.env', 'C:\\a\\.ENV.', 'C:\\a\\.env ', 'C:\\a\\\\.env', 'C:/a/./b/../.env', '\\\\?\\C:\\a\\.env', 'C:\\a\\.env::$DATA', 'C:\\a\\.env:Zone.Identifier', 'C:\\a\\.env. ::$DATA'];
  const gate = new sf.SecretFileGate();
  for (const p of same) assert.equal(gate.key(p), 'c:/a/.env', p);
  assert.equal(gate.key('\\\\server\\share\\.env'), '//server/share/.env');
  assert.equal(gate.key('\\\\?\\UNC\\server\\share\\.env'), '//server/share/.env');
  assert.equal(gate.key('/home/x/.env'), '/home/x/.env');
  assert.equal(gate.key('C:.env'), 'c:.env', 'a drive-relative name keeps its drive: the colon is not a stream');
  assert.equal(gate.key(''), ''); assert.equal(gate.key(null), '');
  for (const p of same) assert.equal(cls(p).secret, true, p);
  assert.equal(cls('C:\\a\\.ENV.').label, '.ENV', 'the label keeps the spelling, without the dots, spaces and stream');
});

test('SEC-03 gate: once is spent by one consume, session lasts until expiry, deny stays a deny', () => {
  let t = 1000;
  const gate = new sf.SecretFileGate({now: () => t, sessionTtlMs: 100, onceTtlMs: 10});
  const p = 'C:\\a\\.env';
  assert.equal(gate.state(p), 'ask'); assert.equal(gate.consume(p), false);
  gate.record(p, 'once');
  assert.equal(gate.state(p), 'allow'); assert.equal(gate.consume('c:/a/.ENV.'), true); assert.equal(gate.consume(p), false); assert.equal(gate.state(p), 'ask');
  gate.record(p, 'session'); t += 99;
  assert.equal(gate.consume(p), true); assert.equal(gate.consume(p), true); t += 1;
  assert.equal(gate.state(p), 'ask');
  gate.record(p, 'deny'); t += 99;
  assert.equal(gate.state(p), 'deny'); assert.equal(gate.consume(p), false);
  t += 1; assert.equal(gate.state(p), 'ask');
});

test('SEC-04 user patterns add secret-class names (glob, case-insensitive) and never throw on junk', () => {
  assert.equal(cls('D:\\repo\\notes\\prod-db.txt', ['prod*']).kind, 'user-pattern');
  assert.equal(cls('D:\\repo\\notes\\a.SECRET', ['*.secret']).secret, true);
  assert.equal(cls('D:\\repo\\config\\a.yml', ['**/config/*.yml']).secret, true);
  assert.equal(cls('D:\\repo\\readme.md', ['prod*', 7, null, '']).secret, false);
  assert.equal(cls('D:\\repo\\readme.md', 'prod*').secret, false);
  assert.deepEqual(cls(undefined), {secret: false, kind: null, rule: null, label: ''});
  assert.equal(cls('D:\\repo\\.env.example', ['*.example']).secret, true, 'a user pattern beats a built-in carve-out');
});
