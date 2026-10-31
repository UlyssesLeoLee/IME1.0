'use strict';
// Requirement added by the project owner after the design baseline (no UT/IT ids exist for it): before the extension reads a
// file it looks at the NAME only; a secret-class file (.env and friends) is read only after the user confirms. These tests
// cover the pure parts: the name rules, the glob matcher behind user patterns and the decision store. Ids here are SEC-xx.
// Every name in this file is synthetic (the real-machine walk that shaped the table is described in the commit message only).
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {core} = require('./helpers');
const sf = core('secretfile');
const {isSensitiveFileName} = core('secrets');

const cls = (p, userPatterns) => sf.classifyFile({path: p, userPatterns});
const u = (...cps) => String.fromCodePoint(...cps); // non-ASCII names are built from code points, never typed
const CJK_USER = u(0x7528, 0x6237), CJK_PROJECT = u(0x9879, 0x76ee), KATAKANA_USER = u(0x30e6, 0x30fc, 0x30b6, 0x30fc);
const PASSWORD_JA = u(0x30d1, 0x30b9, 0x30ef, 0x30fc, 0x30c9), SECRET_KEY_JA = u(0x79d8, 0x5bc6, 0x9375), KEY_NOTE_ZH = u(0x5bc6, 0x94a5, 0x8bf4, 0x660e);
const KEY_RU = u(0x43a, 0x43b, 0x44e, 0x447), KEY_EMOJI = u(0x1f511);

// ---------------------------------------------------------------- the labelled corpus
const POSITIVE = [], NEGATIVE = [];
const pos = (kind, ...names) => { for (const n of names) POSITIVE.push([n, kind]); };
pos('env', '.env', '.env.local', '.env.development', '.env.production', '.env.test', '.env.staging', '.env.development.local', '.env.production.local',
  '.env.keys', '.env.vault', '.env-dev', '.env_prod', '.ENV', '.Env.Local', '.env.', '.env ', 'prod.env', 'docker.env', 'app.env', 'local.env', 'secrets.env',
  '.envrc', '.envrc.local', '.dev.vars', '.dev.vars.production', '.env.bak', '.env.old', '.env~', '.env.swp', '.env.local.bak', CJK_USER + '.env',
  'C:\\Users\\x\\proj\\.env', 'D:/repo/config/.env.production', '\\\\server\\share\\.env', '/home/x/proj/.env', '/Users/x/Projects/app/.env.local',
  'C:\\proj\\.env::$DATA', 'C:\\proj\\.env:Zone.Identifier', 'C:\\proj\\.env. ', '\\\\?\\C:\\proj\\.env', 'C:\\Users\\' + CJK_USER + '\\' + CJK_PROJECT + '\\.env',
  '/home/' + KATAKANA_USER + '/app/.env.production', 'x/'.repeat(300) + '.env', 'C:\\proj\\.ENV.LOCAL');
pos('private-key', 'id_rsa', 'id_dsa', 'id_ecdsa', 'id_ed25519', 'id_rsa_github', 'id_ed25519_work', 'id_ecdsa_sk', 'id_rsa.bak', 'github_rsa', 'deploy_ed25519', 'my-ed25519',
  'server.pem', 'SERVER.PEM', 'server.key', 'private.key', 'tls.key', 'privkey.pem', 'cert.pem', 'master.key', 'config/credentials/production.key', 'config/master.key', 'AuthKey_ABC123DEFG.p8',
  'platform.pk8', 'putty.ppk', 'strongname.snk', '/home/x/.ssh/id_rsa', 'C:\\Users\\x\\.ssh\\work-key', '/home/x/.ssh/github', 'C:\\Users\\x\\.ssh.\\id_x',
  '/home/x/.ssh/keys/prod/anything', '/home/x/.gnupg/secring.gpg', 'C:\\Users\\x\\AppData\\Roaming\\gnupg\\secring.gpg', '/home/x/.gnupg/private-keys-v1.d/ABCDEF0123.key',
  'adbkey', 'ssh_host_ed25519_key', 'ssh_host_rsa_key', 'private_key', 'secret-key', 'x'.repeat(5000) + '.pem', SECRET_KEY_JA + '.pem', 'C:\\Users\\x\\.ssh\\id_rsa::$DATA', 'D:\\a\\id_rsa.');
pos('keystore', 'release.keystore', 'debug.keystore', 'my.jks', 'store.jceks', 'client.p12', 'cert.pfx', 'a.pkcs12', 'trust.bks', 'login.keychain-db', 'x.keychain',
  'C:\\keys\\Release.JKS', 'android/app/upload-keystore.jks');
pos('password-db', 'wallet.dat', 'my.wallet', 'vault.kdbx', 'old.kdb', 'pw.psafe3', 'export.1pux', 'passwords.csv', 'passwords.txt', 'Password.xlsx', 'passwd.json',
  'masterpassword.mpw', 'C:\\Users\\x\\Documents\\Passwords.XLS');
pos('cloud-credentials', '/home/x/.aws/credentials', 'C:\\Users\\x\\.aws\\credentials', '/home/x/.aws/sso/cache/abc123.json', '/home/x/.aws/cli/cache/abc123.json',
  '/home/x/.azure/accessTokens.json', 'C:\\Users\\x\\.azure\\msal_token_cache.json', '/home/x/.config/gcloud/credentials.db',
  '/home/x/.config/gcloud/application_default_credentials.json', 'C:\\Users\\x\\AppData\\Roaming\\gcloud\\access_tokens.db', '/home/x/.kube/config', 'kubeconfig',
  'prod.kubeconfig', 'kubeconfig.yaml', 'service-account.json', 'my-service-account-key.json', 'gcp_service_account.json', 'serviceAccountKey.json',
  'firebase-adminsdk-abc12-0123456789.json', 'client_secret_123-abc.apps.googleusercontent.com.json', 'client_secrets.json', 'client_secret.json');
pos('credential-file', 'credentials', 'credentials.json', '.credentials.json', 'gcp-credentials.json', 'credentials.toml', 'credentials.xml', 'credentials.yaml',
  'credentials.tfrc.json', 'credential.json', 'secrets.json', 'secrets.yml', 'secrets.yaml', 'secrets.toml', 'secrets.ini', 'secret.txt', 'secrets.properties',
  'secrets.xml', 'secrets.dev.json', 'Secrets.plist', 'app.secret', 'db.secrets', '.secret', '.pypirc', '.htpasswd', '.pgpass', 'pgpass.conf', '.my.cnf', '.s3cfg',
  'rclone.conf', 'settings-security.xml', '/home/x/proj/.git/config', '/home/x/.m2/settings.xml', 'C:\\Users\\x\\.gradle\\gradle.properties', '.vault_pass',
  'vault_pass.txt', 'office.ovpn', 'keystore.properties', 'key.properties', 'local.settings.json', 'passwd', 'C:\\proj\\SECRETS.JSON', 'C:\\proj\\Secrets.Json.');
pos('token-store', '.npmrc', '.netrc', '_netrc', '.git-credentials', '.vault-token', '.dockercfg', '/home/x/.docker/config.json', '/home/x/.config/gh/hosts.yml', 'token.json',
  'token.pickle', 'access_token.txt', 'api_key.txt', 'apikey.json', '/home/x/.codex/auth.json', 'C:\\Users\\x\\AppData\\Roaming\\com.vercel.cli\\Data\\auth.json',
  '.emulator_console_auth_token', 'logout_token', 'x.token', 'Cookies', 'cookies.txt', 'cookies.sqlite', 'Login Data', 'Login Data-journal', 'Cookies-journal', 'logins.json',
  'key4.db', 'cookies.sqlite-wal', 'C:\\Users\\x\\AppData\\Local\\Google\\Chrome\\User Data\\Default\\Network\\Cookies');
pos('infra-state', 'terraform.tfstate', 'terraform.tfstate.backup', 'prod.tfstate', 'terraform.tfvars', 'prod.auto.tfvars', 'a.tfvars.json', 'infra/.terraform/terraform.tfstate',
  'terraform.tfstate.1234.backup', 'TERRAFORM.TFSTATE');

const neg = (...names) => NEGATIVE.push(...names);
neg( // source and documents are never secret by name alone
  'credentials.ts', 'credentials.js', 'credentials.py', 'credentials.rs', 'secrets.rs', 'secrets.ts', 'secret.rs', 'secrets.js', 'secrets.go', 'secrets.md', 'keys.js', 'keys.ts',
  'env.ts', 'env.js', 'env.d.ts', 'vite-env.d.ts', 'README.md', 'key.txt', 'token.js', 'token.ts', 'tokens.json', 'design-tokens.json', 'passwords.md', 'password.ts', 'Password.cs',
  'auth.json', 'auth.ts', 'locales/en/auth.json', 'settings_local.py', 'application-prod.properties', 'application.yml', 'appsettings.Production.json', 'config.yml', 'config.json',
  'package.json', 'tsconfig.json', '.gitignore', '.eslintrc.js', 'Dockerfile', 'docker-compose.yml', 'Makefile', 'main.rs', 'index.html', 'LICENSE', '.yarnrc.yml', 'nuget.config',
  'pom.xml', 'settings.gradle', 'gradle.properties', 'settings.xml', '.editorconfig', '.npmignore', 'dotenv.js', 'environment.ts', '.environment', 'env', 'venv', '.venv', 'environment.yml',
  'env.json', 'envconfig.js', '.envfile');
neg( // public material, ciphertext and templates
  'id_rsa.pub', 'id_ed25519.pub', 'github_rsa.pub', 'known_hosts', 'known_hosts.old', 'authorized_keys', '/home/x/.ssh/config', '/home/x/.ssh/id_rsa.pub', '/home/x/.ssh/known_hosts2',
  '/home/x/.aws/config', 'server.crt', 'server.cer', 'ca.csr', 'app.mobileprovision', 'cert.der', 'adbkey.pub', '/home/x/.gnupg/gpg.conf', '/home/x/.gnupg/pubring.kbx',
  '/home/x/.gnupg/trustdb.gpg', '/home/x/.gnupg/S.gpg-agent', 'server.pem.pub', 'id_rsa.pub.bak', 'ssh_host_rsa_key.pub',
  'credentials.yml.enc', 'config/credentials.yml.enc', 'config/credentials/production.yml.enc', 'secrets.yaml.enc', 'app.sops.yaml', 'sealed-secret.yaml', 'secrets.yml.gpg',
  '.env.example', '.env.sample', '.env.template', '.env.dist', '.env.defaults', '.env.default', '.env.local.example', '.env.production.sample', '.env-example', '.env_sample',
  'example.env', 'sample.env', 'terraform.tfvars.example', 'prod.tfvars.sample', '.envrc.example', '.dev.vars.example', 'env.example', '.env.example.bak', 'secrets.example.json',
  'credentials.sample.yml', 'token.template.json', 'local.settings.example.json');
neg( // names that merely contain secret-ish words
  'secret-cases.json', 'secret-sauce.png', 'secretary.txt', 'keyboard.cnf', 'keybindings.json', 'monkey.png', 'donkey.txt', 'turkey.md', 'keyboard.js', 'keys', 'keystore.ts', 'keystore',
  'sessions.json', 'session.db', 'cookies.py', 'cookies.js', 'cookiecutter.json', 'token_utils.py', 'jwt.ts', 'auth.rs', 'vault.rs', 'wallet.ts', 'wallet.md', 'keychain.ts', 'keychains.json',
  'login.html', 'login data.md', 'passwd.c', 'passwd.md', 'password-reset.html', 'private.md', 'private_key.rs', 'private-key.ts', 'privatekey.js', 'pem.js', 'pemfile.txt', 'p12.md',
  'x.pem.md', 'monkey.key.js', 'tokenizer.json', 'token-list.json', 'tokens.ts', 'token.css', 'keys.properties', 'local.properties', 'settings.json', 'launch.json', 'secretfile.js',
  'cacert.dat', 'Trust Tokens', 'Safe Browsing Cookies', 'client_secret.ts', 'client_secrets.py', 'firebase.json', 'google-services.json', 'GoogleService-Info.plist',
  'service-account.yaml', 'serviceaccount.yaml', 'hosts.yml', 'hosts.yaml', 'main.tf', 'variables.tf', 'outputs.tf', 'terraform.lock.hcl', '.terraformrc', 'kubeconfig.ts', 'kube-config.js',
  'kubeconfig.go', 'kubernetes.yaml', 'deployment.yaml', 'cookies.md', 'cookie-banner.js', 'app.sqlite', 'ssh.md', '.ssh-config.md', 'keymap.rs', 'session.rs');
neg( // by directory: a virtualenv named `.env`, tool folders that hold no secret, directories and degenerate input
  '/p/.env/lib/python3.11/site-packages/foo.py', '/p/.env/pyvenv.cfg', 'C:\\p\\.env\\Scripts\\activate.bat', '/p/.env/bin/python', 'ssh/readme.md', 'docs/.aws-setup.md',
  '/home/x/.aws/cli/alias', '/home/x/.azure/logs/az.log', '/home/x/.config/gcloud/logs/2026.01.01.log', '/home/x/.docker/daemon.json', '/home/x/.git/HEAD', '/home/x/.git/index',
  '/home/x/.git/hooks/pre-commit.sample', '/home/x/.m2/repository/foo.jar', '/home/x/.gradle/caches/x', '/home/x/.ssh', '/home/x/.kube/cache/discovery.json',
  '', 'C:\\', '/', '.', '..', 'C:\\proj\\', 'dir/', '   ');
neg( // unicode and very long names
  KEY_NOTE_ZH + '.md', PASSWORD_JA + '.md', KEY_RU + '.txt', KEY_EMOJI + '.png', 'README_' + CJK_PROJECT + '.md', SECRET_KEY_JA + 'a.md', 'a'.repeat(5000) + '.txt',
  'C:\\' + 'dir\\'.repeat(200) + 'readme.md', 'x'.repeat(70000) + '.md', CJK_USER + '/' + CJK_PROJECT + '/main.rs', '.env' + String.fromCharCode(0x200b), '.' + u(0x435) + 'nv', 'e' + u(0x2024) + 'pem');

const check = (list, f) => list.map(f).filter(Boolean);

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
  const same = ['C:\\a\\.env', 'c:/a/.env', 'C:\\a\\.ENV.', 'C:\\a\\.env ', 'C:\\a\\\\.env', 'C:/a/./b/../.env', '\\\\?\\C:\\a\\.env', 'C:\\a\\.env::$DATA', 'C:\\a\\.env:Zone.Identifier', 'C:\\a\\.env. ::$DATA', '  C:\\a\\.env'];
  const gate = new sf.SecretFileGate();
  for (const p of same) assert.equal(gate.key(p), 'c:/a/.env', p);
  assert.equal(gate.key('\\\\server\\share\\.env'), '//server/share/.env');
  assert.equal(gate.key('\\\\?\\UNC\\server\\share\\.env'), '//server/share/.env');
  assert.equal(gate.key('/home/x/.env'), '/home/x/.env');
  assert.equal(gate.key('C:.env'), 'c:.env', 'a drive-relative name keeps its drive: the colon is not a stream');
  assert.equal(gate.key('C:\\..\\.env'), 'c:/.env', '.. cannot climb above a root');
  assert.equal(gate.key('../.env'), '../.env');
  assert.equal(gate.key(''), ''); assert.equal(gate.key(null), '');
  for (const p of same) assert.equal(cls(p).secret, true, p);
  assert.equal(cls('C:\\a\\.ENV.').label, '.ENV', 'the label keeps the spelling, without the dots, spaces and stream');
});

test('SEC-02a every path segment is normalised, not only the last: `.ssh.\\id_x` is inside `.ssh`; a colon in a POSIX name does not hide its extension', () => {
  assert.equal(cls('/home/x/.ssh./notes').rule, 'ssh-dir');
  assert.equal(cls('C:\\Users\\x\\.SSH \\notes').rule, 'ssh-dir');
  assert.equal(cls('/tmp/foo:bar.pem').secret, true, 'read as an NTFS stream the name would be `foo`: both readings are tried');
  assert.equal(new sf.SecretFileGate().key('/tmp/foo:bar.pem'), '/tmp/foo', 'the key follows the NTFS reading (documented limit)');
  assert.equal(cls('a/b/../../.env').secret, true);
  assert.equal(cls('a/.ssh/../notes.txt').secret, false, '.. is resolved before the directory rules look');
  assert.equal(cls('x'.repeat(100000) + '/.env').secret, true, 'a huge input keeps its tail, which holds the name');
  assert.ok(new sf.SecretFileGate().key('x'.repeat(100000) + '/.env').length <= 65536);
});

test('SEC-02b never throws, and a non-path is an empty answer; a bare string is taken as the path (a caller bug fails closed)', () => {
  const none = {secret: false, kind: null, rule: null, label: ''};
  for (const bad of [undefined, null, 42, {}, [], Symbol('x'), () => 1, true, NaN]) {
    assert.deepEqual(sf.classifyFile(bad), none);
    assert.deepEqual(sf.classifyFile({path: bad}), none);
  }
  assert.deepEqual(sf.classifyFile(), none);
  assert.deepEqual(cls(''), none);
  assert.equal(sf.classifyFile('C:\\a\\.env').secret, true);
  assert.equal(sf.classifyFile({path: '.env', userPatterns: {length: 1, 0: 'x'}}).secret, true, 'userPatterns that is not an array is ignored');
});

test('SEC-02c the label is what a dialog may show: original spelling, no directories, no control or bidi characters, bounded', () => {
  assert.equal(cls('C:\\Users\\x\\Proj\\Server.PEM').label, 'Server.PEM');
  const rlo = u(0x202e), nul = String.fromCharCode(0), nl = String.fromCharCode(10);
  assert.equal(cls('dir/a' + rlo + 'fdp.pem').label, 'a?fdp.pem');
  assert.equal(cls('dir/a' + nl + 'b' + nul + '.pem').label, 'a?b?.pem');
  const long = cls('x'.repeat(300) + '.pem');
  assert.equal(long.secret, true); assert.equal(long.label.length, 123); assert.ok(long.label.endsWith('...'));
  assert.equal(cls(KATAKANA_USER + '/' + PASSWORD_JA + '.pem').label, PASSWORD_JA + '.pem');
  assert.equal(Array.from(cls(KEY_EMOJI.repeat(200) + '.pem').label).length, 123, 'truncated by code point, no half surrogate');
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

test('SEC-03a gate: one canonical key for every spelling of the same file, and only for that file', () => {
  const gate = new sf.SecretFileGate();
  assert.equal(gate.record('C:\\a\\.env', 'session'), 'allow');
  for (const same of ['c:/a/.env', 'C:\\a\\.ENV.', 'C:\\a\\.env::$DATA', 'C:\\a\\\\.env', '\\\\?\\c:\\A\\.Env']) assert.equal(gate.state(same), 'allow', same);
  for (const other of ['C:\\b\\.env', 'C:\\a\\.env.local', 'D:\\a\\.env', '\\\\a\\a\\.env', 'a/.env', '/a/.env']) assert.equal(gate.state(other), 'ask', other);
  const kelvin = String.fromCharCode(0x212a);
  assert.equal(gate.state('C:\\a\\.env' + kelvin), 'ask');
  assert.notEqual(gate.key('C:\\a\\' + kelvin + '.pem'), gate.key('C:\\a\\k.pem'), 'a Kelvin sign is not a k: no two files share a key through full Unicode folding');
  assert.equal(gate.key('C:\\a\\' + u(0xdc) + '.env'), 'c:/a/' + u(0xdc) + '.env', 'case is folded for ASCII only');
});

test('SEC-03b gate: record replaces an earlier answer and makes it the newest; eviction drops the oldest, expired entries go first', () => {
  let t = 0;
  const gate = new sf.SecretFileGate({now: () => t, sessionTtlMs: 100, onceTtlMs: 10, maxEntries: 3});
  gate.record('/a/.env', 'session'); t = 1; gate.record('/b/.env', 'once'); t = 2; gate.record('/c/.env', 'session');
  assert.deepEqual(gate.list().map(e => e.key), ['/a/.env', '/b/.env', '/c/.env']);
  t = 50; gate.record('/d/.env', 'session'); // /b/.env expired at 11: it goes, the live oldest (/a/.env) stays
  assert.deepEqual(gate.list().map(e => e.key), ['/a/.env', '/c/.env', '/d/.env']);
  gate.record('/a/.env', 'deny'); // a replaced answer is the newest now
  assert.deepEqual(gate.list().map(e => [e.key, e.state]), [['/c/.env', 'allow'], ['/d/.env', 'allow'], ['/a/.env', 'deny']]);
  gate.record('/e/.env', 'session'); // over the limit: the oldest live one (/c/.env) is evicted
  assert.deepEqual(gate.list().map(e => e.key), ['/d/.env', '/a/.env', '/e/.env']);
  assert.equal(gate.state('/c/.env'), 'ask');
  const one = new sf.SecretFileGate({maxEntries: 1}); one.record('/a/.env', 'session'); one.record('/b/.env', 'session');
  assert.deepEqual(one.list().map(e => e.key), ['/b/.env']);
});

test('SEC-03c gate: a deny is remembered (no nagging) until revoked or expired; once and session do not outlive their ttl', () => {
  let t = 0;
  const gate = new sf.SecretFileGate({now: () => t, sessionTtlMs: 1000, onceTtlMs: 100});
  const p = '/a/.env';
  gate.record(p, 'deny'); t = 999;
  assert.equal(gate.state(p), 'deny'); assert.equal(gate.consume(p), false); assert.equal(gate.state(p), 'deny', 'consume never turns a deny into anything else');
  assert.equal(gate.revoke(p), true); assert.equal(gate.state(p), 'ask'); assert.equal(gate.revoke(p), false);
  t = 5000; gate.record(p, 'once'); t = 5099; assert.equal(gate.state(p), 'allow'); t = 5100;
  assert.equal(gate.state(p), 'ask', 'an unused once-grant expires: a stale approval must not authorise a later, unrelated read');
  assert.equal(gate.consume(p), false);
  gate.record(p, 'session'); t = 6099; assert.equal(gate.consume(p), true); t = 6100; assert.equal(gate.consume(p), false);
});

test('SEC-03d gate: list() has exactly key, label, state, expiresAt and hands out copies; clear() empties; hostile input changes nothing', () => {
  let t = 10;
  const gate = new sf.SecretFileGate({now: () => t, sessionTtlMs: 100});
  gate.record('C:\\Users\\x\\Proj\\.ENV.Local', 'session'); gate.record('/x/id_rsa', 'deny');
  const l = gate.list();
  assert.deepEqual(l, [{key: 'c:/users/x/proj/.env.local', label: '.ENV.Local', state: 'allow', expiresAt: 110}, {key: '/x/id_rsa', label: 'id_rsa', state: 'deny', expiresAt: 110}]);
  for (const e of l) assert.deepEqual(Object.keys(e).sort(), ['expiresAt', 'key', 'label', 'state']);
  l[0].state = 'deny'; l.length = 0;
  assert.equal(gate.state('C:\\Users\\x\\Proj\\.env.local'), 'allow'); assert.equal(gate.list().length, 2);
  assert.equal(gate.record(null, 'once'), 'ask'); assert.equal(gate.record('', 'session'), 'ask'); assert.equal(gate.record('C:\\', 'session'), 'ask');
  assert.equal(gate.record('/x/id_rsa', 'forever'), 'deny', 'an unknown choice leaves the earlier answer alone');
  assert.equal(gate.record('/y/.env', undefined), 'ask');
  assert.equal(gate.list().length, 2);
  assert.equal(gate.clear(), 2); assert.deepEqual(gate.list(), []); assert.equal(gate.clear(), 0);
  assert.equal(gate.state(undefined), 'ask'); assert.equal(gate.consume({}), false); assert.equal(gate.revoke(5), false);
});

test('SEC-03e gate: a clock that goes backwards or is not a number fails closed; bad options fall back to the defaults', () => {
  let t = 1000;
  const gate = new sf.SecretFileGate({now: () => t, sessionTtlMs: 100});
  gate.record('/a/.env', 'session');
  t = 500; assert.equal(gate.state('/a/.env'), 'ask', 'time moved back: the grant is void');
  const nan = new sf.SecretFileGate({now: () => NaN}); nan.record('/a/.env', 'session');
  assert.equal(nan.state('/a/.env'), 'ask'); assert.equal(nan.consume('/a/.env'), false);
  for (const opts of [null, undefined, 5, 'x', {sessionTtlMs: -5, onceTtlMs: 0, maxEntries: -1, now: 'no'}]) {
    const g = new sf.SecretFileGate(opts); g.record('/a/.env', 'session');
    const e = g.list()[0];
    assert.equal(g.state('/a/.env'), 'allow'); assert.ok(e.expiresAt - Date.now() > 7 * 3600 * 1000 && e.expiresAt - Date.now() <= 8 * 3600 * 1000, JSON.stringify(opts));
  }
});

test('SEC-04 user patterns add secret-class names (glob, case-insensitive) and never throw on junk', () => {
  const r = cls('D:\\repo\\notes\\prod-db.txt', ['prod*']);
  assert.deepEqual([r.secret, r.kind, r.rule, r.label], [true, 'user-pattern', 'user-pattern', 'prod-db.txt']);
  assert.equal(cls('D:\\repo\\notes\\a.SECRET', ['*.secret']).secret, true);
  assert.equal(cls('D:\\repo\\config\\a.yml', ['**/config/*.yml']).secret, true);
  assert.equal(cls('D:\\repo\\readme.md', ['prod*', 7, null, '']).secret, false);
  assert.equal(cls('D:\\repo\\readme.md', 'prod*').secret, false);
  assert.equal(cls('D:\\repo\\.env.example', ['*.example']).secret, true, 'a user pattern beats a built-in carve-out');
  assert.equal(cls('D:\\repo\\.env', ['.env']).kind, 'env', 'when a built-in rule already matches its kind is reported');
});

test('SEC-04a glob: `*` and `?` stay inside a segment, `**` crosses, `**/` also matches no directory, a trailing `/` means everything below', () => {
  const m = (pattern, p) => cls(p, [pattern]).secret;
  assert.equal(m('**/config/*.yml', 'D:/r/x/y/config/b.yml'), true);
  assert.equal(m('**/config/*.yml', 'config/a.yml'), true, 'zero directories before config');
  assert.equal(m('**/config/*.yml', 'D:/r/config/sub/c.yml'), false);
  assert.equal(m('**/config/*.yml', 'D:/r/config/a.yaml'), false);
  assert.equal(m('**/config/*.yml', 'D:/r/xconfig/a.yml'), false, '`**/` ends at a separator: `xconfig` is not `config`');
  assert.equal(m('**/a?b/x', 'r/aXb/x'), true); assert.equal(m('**/a?b/x', 'r/a/b/x'), false, '`?` does not match a separator');
  assert.equal(m('**/a?b/x', 'r/ab/x'), false, '`?` is exactly one character');
  assert.equal(m('secrets/**', 'D:/r/secrets/a/b/c.txt'), true); assert.equal(m('secrets/', 'D:/r/secrets/a/b/c.txt'), true); assert.equal(m('secrets/', 'D:/r/notsecrets/c.txt'), false);
  assert.equal(m('d:/r/**/x.txt', 'D:\\r\\a\\b\\x.txt'), true); assert.equal(m('d:/r/**/x.txt', 'D:\\r\\x.txt'), true); assert.equal(m('d:/r/**/x.txt', 'D:\\q\\x.txt'), false);
  assert.equal(m('a**b/x', 'r/aX/Yb/x'), true, '`**` inside a name is still `**`: any characters, separators included');
  assert.equal(m('**', 'anything.txt'), true);
  assert.equal(m('a/**/**/b.txt', 'r/a/x/y/z/b.txt'), true);
});

test('SEC-04b glob: no separator = base name only (any case); a separator pattern is matched from any directory unless rooted; backslash is a separator', () => {
  const m = (pattern, p) => cls(p, [pattern]).secret;
  assert.equal(m('prod*', 'x/prod.txt'), true); assert.equal(m('prod*', 'x/PRODUCTION.md'), true); assert.equal(m('prod*', 'x/myprod'), false);
  assert.equal(m('prod*', 'prod/readme.md'), false, 'the directory name is not the base name');
  assert.equal(m('*.SECRET', 'a/B.secret'), true);
  assert.equal(m('config/*.yml', 'D:/repo/config/a.yml'), true, 'written relative to the workspace, the path arrives absolute');
  assert.equal(m('/config/*.yml', 'D:/repo/config/a.yml'), false, 'a rooted pattern is not shifted');
  assert.equal(m('/repo/*.yml', '/repo/a.yml'), true);
  assert.equal(m('config\\*.yml', 'D:\\repo\\config\\a.yml'), true); assert.equal(m('**\\config\\*.yml', 'D:\\repo\\config\\a.yml'), true);
  assert.equal(m('./config/*.yml', 'D:/repo/config/a.yml'), true);
  assert.equal(m('  prod*  ', 'x/prod.txt'), true, 'surrounding blanks of a setting value are ignored');
  assert.equal(m('*.secret', 'x/a.secret.bak'), true, 'a backup of a matching name matches too');
  assert.equal(m('*.secret', '\\\\srv\\share\\a.secret::$DATA'), true);
  assert.equal(m('x', 'a/x/'), true, 'a trailing separator on the path is dropped');
});

test('SEC-04c glob: regex metacharacters are literal characters', () => {
  const m = (pattern, p) => cls(p, [pattern]).secret;
  assert.equal(m('a+b(1).txt', 'd/a+b(1).txt'), true); assert.equal(m('a+b(1).txt', 'd/aab1.txt'), false);
  assert.equal(m('[abc].txt', 'd/[abc].txt'), true); assert.equal(m('[abc].txt', 'd/a.txt'), false);
  assert.equal(m('{a,b}.txt', 'd/{a,b}.txt'), true); assert.equal(m('{a,b}.txt', 'd/a.txt'), false);
  assert.equal(m('a|b', 'd/a|b'), true); assert.equal(m('a|b', 'd/a'), false);
  assert.equal(m('^x$', 'd/^x$'), true); assert.equal(m('^x$', 'd/x'), false);
  assert.equal(m('a.c', 'd/abc'), false, 'the dot is a dot');
  assert.equal(m('(a*)*b', 'd/' + 'a'.repeat(50)), false);
  assert.equal(m('x.*', 'd/x.y'), true); assert.equal(m('x.*', 'd/x.'), false, 'a trailing dot is dropped by the normalisation, so the name is `x`');
});

test('SEC-04d glob: a pathological pattern is linear, and a spent step budget fails closed (the user is asked)', () => {
  const t0 = Date.now();
  assert.equal(cls('d/' + 'a'.repeat(3000), ['*a*a*a*a*a*a*b']).secret, false);
  assert.equal(cls('d/' + 'a'.repeat(3000) + 'b', ['*a*a*a*a*a*a*b']).secret, true);
  assert.equal(cls('/' + 'a/'.repeat(500) + 'x', ['**/a/**/a/**/a/**/b']).secret, false);
  assert.equal(cls('d/' + 'a'.repeat(60000), ['*a*a*a*a*a*a*a*a*a*a*a*a*a*a*b']).secret, false, 'a long name that fits the budget');
  const heavy = Array(200).fill('*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*b');
  const r = cls('d/' + 'a'.repeat(5000), heavy);
  assert.deepEqual([r.secret, r.kind], [true, 'user-pattern'], 'the budget ran out before every pattern was tried: fail closed');
  assert.ok(Date.now() - t0 < 8000, 'bounded work');
});

test('SEC-04e user pattern limits: at most 200 valid patterns, none longer than 300 characters (skipped, not cut), junk does not count', () => {
  const target = 'd/target.txt';
  assert.equal(cls(target, [...Array(199).fill('zzz'), 'target.txt']).secret, true);
  assert.equal(cls(target, [...Array(200).fill('zzz'), 'target.txt']).secret, false, 'the 201st valid pattern is ignored');
  assert.equal(cls(target, [...Array(500).fill(7), ...Array(100).fill(' '), 'target.txt']).secret, true, 'non-strings and blanks are not valid patterns, so they use no slot');
  const name300 = 'a'.repeat(300), name301 = 'a'.repeat(301);
  assert.equal(cls('d/' + name300, [name300]).secret, true);
  assert.equal(cls('d/' + name301, [name301]).secret, false, 'over 300: the whole pattern is skipped');
  assert.equal(cls('d/' + name300, [name301 + '*']).secret, false);
  assert.equal(cls('d/' + name301, ['a'.repeat(299) + '*']).secret, true, 'a 300-character pattern is the longest accepted');
});

test('SEC-05 the positive corpus: every secret-class name is found, with the right kind', () => {
  assert.ok(POSITIVE.length >= 120, `corpus has ${POSITIVE.length} secret names`);
  assert.equal(new Set(POSITIVE.map(x => x[0])).size, POSITIVE.length, 'no duplicates');
  assert.deepEqual(check(POSITIVE, ([p, kind]) => { const r = cls(p); return r.secret && r.kind === kind ? null : `${p.slice(0, 80)} -> ${r.secret} ${r.kind} (${r.rule}), wanted ${kind}`; }), []);
  assert.deepEqual([...new Set(POSITIVE.map(x => x[1]))].sort(), sf.KINDS.filter(k => k !== 'user-pattern').sort(), 'every kind is exercised');
});

test('SEC-06 the negative corpus: source, docs, public keys, templates, ciphertext and tool folders without secrets are never flagged', () => {
  assert.ok(NEGATIVE.length >= 120, `corpus has ${NEGATIVE.length} ordinary names`);
  assert.equal(new Set(NEGATIVE).size, NEGATIVE.length, 'no duplicates');
  assert.deepEqual(check(NEGATIVE, p => { const r = cls(p); return r.secret ? `${p.slice(0, 80)} -> ${r.kind} (${r.rule})` : null; }), []);
});

test('SEC-06a the result is the same under decoration: Windows or POSIX directories, any case, trailing dot or space, stream suffix', () => {
  const decorate = [n => n, n => 'C:\\Users\\x\\proj\\' + n, n => 'D:/repo/deep/er/' + n, n => '\\\\server\\share\\' + n, n => '/home/x/' + n, n => '\\\\?\\C:\\p\\' + n, n => n.toUpperCase(), n => n + '.', n => n + ' ', n => n + '::$DATA'];
  const flat = [...POSITIVE.filter(([n]) => !/[\\/]|[. ]$/.test(n) && !n.includes(':')).map(([n]) => [n, true]), ...NEGATIVE.filter(n => n && !/[\\/]|[. ]$/.test(n) && !n.includes(':') && n.length < 100 && !n.includes(' ')).map(n => [n, false])];
  assert.ok(flat.length > 150);
  const bad = [];
  for (const [n, want] of flat) for (const d of decorate) {
    const p = d(n), r = cls(p);
    // dir rules need the folder; upper-casing is ASCII only
    if (r.secret !== want && !/[^\x00-\x7f]/.test(n)) bad.push(`${p.slice(0, 80)} -> ${r.secret}, wanted ${want}`);
  }
  assert.deepEqual(bad, []);
});

test('SEC-07 the table is data: frozen, unique ids, known kinds, one primary form, lower-case, and no rule is shadowed by an earlier one', () => {
  const rules = sf.DEFAULT_RULES;
  assert.ok(Object.isFrozen(rules)); assert.throws(() => rules.push({}), TypeError);
  assert.equal(new Set(rules.map(r => r.id)).size, rules.length, 'ids are unique');
  const forms = new Set(['base', 'head', 'ext', 'not', 'dir']);
  for (const r of rules) {
    assert.ok(Object.isFrozen(r) && Object.isFrozen(r.match), r.id);
    assert.deepEqual(Object.keys(r).sort(), ['id', 'kind', 'match']);
    assert.match(r.id, /^[a-z0-9]+(?:-[a-z0-9]+)*$/);
    assert.ok(sf.KINDS.includes(r.kind) && r.kind !== 'user-pattern', r.id);
    const keys = Object.keys(r.match);
    assert.ok(keys.every(k => forms.has(k)), r.id);
    assert.ok(r.match.base ? !r.match.head && !r.match.ext : r.match.head && r.match.ext, `${r.id}: base, or head with ext`);
    for (const [k, list] of Object.entries(r.match)) {
      assert.ok(Object.isFrozen(list) && list.length > 0, `${r.id}.${k}`);
      for (const s of list) assert.equal(s, s.toLowerCase(), `${r.id}.${k}: matching runs on lower-case names, an upper-case entry could never match`);
    }
  }
  const hit = new Set(POSITIVE.map(([p]) => cls(p).rule));
  assert.deepEqual(rules.map(r => r.id).filter(id => !hit.has(id)), [], 'a rule no corpus name reaches is shadowed or dead');
});

test('SEC-08 the rules are a superset of secrets.js SENSITIVE_FILE; the only names given up are the committed templates', () => {
  // Every alternative of SENSITIVE_FILE, bare and inside Windows/POSIX directories.
  const synthetic = ['.env', '.env.x', '.env.local.bak', 'a.pem', 'a.key', 'a.pfx', 'a.p12', 'a.jks', 'a.keystore', 'A.PEM', 'id_rsa', 'id_dsa', 'id_ecdsa', 'id_ed25519', 'credentials', 'credentials.json',
    '.npmrc', '.netrc', '.pypirc', 'secret.json', 'secrets.json', 'secret.yaml', 'secrets.yml', 'secret.yml', 'secrets.yaml', 'secret.toml', 'secrets.toml'];
  const wrap = [n => n, n => 'C:\\Users\\x\\p\\' + n, n => '/home/x/p/' + n, n => '\\\\srv\\share\\' + n];
  for (const n of synthetic) for (const w of wrap) { assert.equal(isSensitiveFileName(w(n)), true, `${w(n)} is in SENSITIVE_FILE`); assert.equal(cls(w(n)).secret, true, `${w(n)} must stay secret`); }
  const all = [...POSITIVE.map(x => x[0]), ...NEGATIVE, ...synthetic];
  const given = all.filter(n => isSensitiveFileName(n) && !cls(n).secret).sort();
  const templates = ['.env.default', '.env.defaults', '.env.dist', '.env.example', '.env.example.bak', '.env.local.example', '.env.production.sample', '.env.sample', '.env.template'];
  assert.deepEqual(given, templates, 'SENSITIVE_FILE accepts .env.example and friends; templates carry no secrets, so they are deliberately not flagged');
  // forced false positives that stay (they are in SENSITIVE_FILE): Keynote decks, public CA bundles, the Android debug keystore
  for (const n of ['Keynote deck.key', 'cacert.pem', 'ca-bundle.pem', 'debug.keystore']) assert.equal(cls(n).secret, true, n);
});

test('SEC-09 hostile names cannot make the rules or the key slow (no backtracking anywhere)', () => {
  const hostile = ['a.'.repeat(20000), '.env.'.repeat(8000), ' .'.repeat(20000), 'a'.repeat(100000), ':'.repeat(50000), '\\'.repeat(50000), '../'.repeat(30000), '*'.repeat(5000), '.ssh/'.repeat(10000) + 'x',
    'a'.repeat(30000) + '.example', '.env.' + 'example.'.repeat(5000), 'x'.repeat(30000) + '.bak'.repeat(1000), '/'.repeat(60000) + '.env', 'a.pem' + ' '.repeat(60000), '.'.repeat(60000) + 'x'];
  const t0 = Date.now();
  for (const h of hostile) {
    for (const p of [h, 'C:\\' + h, h + '.env']) {
      const r = cls(p, ['*a*a*a*b', '**/x/**/y']);
      assert.equal(typeof r.secret, 'boolean'); assert.equal(typeof r.label, 'string');
      new sf.SecretFileGate().key(p);
    }
  }
  assert.ok(Date.now() - t0 < 10000, `took ${Date.now() - t0} ms`);
});

test('SEC-10 backup and editor-swap copies of a secret are secret; of a public key or a template are not', () => {
  for (const n of ['.env.bak', '.env.old', '.env.orig', '.env.save', '.env.tmp', '.env.swp', '.env.swo', '.env~', '.env.backup', '.env.local.bak~', 'server.key.bak', 'id_rsa.old', 'secrets.yml.bak', 'terraform.tfstate.backup',
    'Cookies-journal', 'Login Data-wal', 'cookies.sqlite-shm', 'credentials.json.orig', 'x.pem.swp']) assert.equal(cls(n).secret, true, n);
  for (const n of ['.env.example.bak', 'id_rsa.pub.bak', 'id_rsa.pub~', '.bak', '.env.example.swp', 'readme.md.bak', 'secrets.ts.old', 'server.pem.pub', 'known_hosts.old']) assert.equal(cls(n).secret, false, n);
});

test('SEC-11 a virtualenv folder named `.env` does not make its contents secret; the names inside still count', () => {
  for (const n of ['/p/.env/bin/activate', '/p/.env/lib/python3.12/site-packages/pip/__init__.py', 'C:\\p\\.env\\Scripts\\python.exe', '/p/.env/pyvenv.cfg', '/p/.env/include/site/python3.12/README']) assert.equal(cls(n).secret, false, n);
  assert.equal(cls('/p/.env/lib/python3.12/site-packages/certifi/cacert.pem').secret, true, 'a *.pem inside is still a *.pem (known false positive)');
  assert.equal(cls('/p/.env/.env').secret, true);
});

test('SEC-12 dogfood on a real machine: .gnupg agent sockets and lock files are not secrets, its keys are', () => {
  for (const n of ['/home/x/.gnupg/S.keyboxd', '/home/x/.gnupg/S.gpg-agent.extra', '/home/x/.gnupg/public-keys.d/.#lk0x0000000a00007c10.host.926', '/home/x/.gnupg/gpg-agent.conf', '/home/x/.gnupg/pubring.kbx', 'C:\\Users\\x\\AppData\\Roaming\\gnupg\\gpg.conf']) assert.equal(cls(n).secret, false, n);
  for (const n of ['/home/x/.gnupg/private-keys-v1.d/ABCD.key', '/home/x/.gnupg/random_seed', '/home/x/.gnupg/openpgp-revocs.d/ABCD.rev', 'C:\\Users\\x\\AppData\\Roaming\\gnupg\\secring.gpg']) assert.equal(cls(n).secret, true, n);
});

test('SEC-12a dogfood: the SQLite sidecars (-journal, -wal, -shm) of a browser credential store are as secret as the store', () => {
  for (const n of ['Default/Network/Cookies-journal', 'Default/Login Data-wal', 'Default/Login Data For Account', 'Default/Login Data For Account-journal', 'Default/Login Data-journal', 'cookies.sqlite-shm', 'key4.db-wal'])
    assert.deepEqual([cls(n).secret, cls(n).kind], [true, 'token-store'], n);
  for (const n of ['Default/Network/Trust Tokens', 'Default/Network/Device Bound Sessions', 'app.db-wal', 'session.db-shm', 'Safe Browsing Cookies-journal']) assert.equal(cls(n).secret, false, n);
});

test('SEC-12b dogfood: key files without an extension (adb, sshd host keys); their .pub halves are not secret', () => {
  for (const n of ['adbkey', '/home/x/.android/adbkey', 'ssh_host_ecdsa_key', '/etc/ssh/ssh_host_ed25519_key']) assert.equal(cls(n).kind, 'private-key', n);
  for (const n of ['adbkey.pub', 'ssh_host_ecdsa_key.pub', 'adbkeys.md', 'ssh_host_notes.txt']) assert.equal(cls(n).secret, false, n);
});

test('SEC-12c dogfood: a login token kept in a file whose name ends in token (no extension); tokens.json, design tokens and code are not', () => {
  for (const n of ['.emulator_console_auth_token', 'logout_token', '.gh_token', 'npm-token', 'deploy.token', '.token']) assert.equal(cls(n).kind, 'token-store', n);
  for (const n of ['canvas-tokens.d.ts', 'tokens.json', 'token-utils.ts', 'tokentable.ini', 'refresh_token.rs', 'csrf_token.js', 'token']) assert.equal(cls(n).secret, false, n);
});

test('SEC-12d dogfood: auth.json is a login store only inside the folders of the CLIs that keep one (a bare auth.json is a locale file)', () => {
  for (const n of ['/home/x/.codex/auth.json', 'C:\\Users\\x\\AppData\\Roaming\\com.vercel.cli\\Data\\auth.json', '/home/x/.composer/auth.json', 'C:\\Users\\x\\AppData\\Roaming\\Composer\\auth.json', '/p/.vercel/auth.json']) assert.equal(cls(n).kind, 'token-store', n);
  for (const n of ['auth.json', '/p/src/locales/en/auth.json', '/p/public/i18n/auth.json', '/p/codex/auth.json']) assert.equal(cls(n).secret, false, n);
});

test('SEC-12e dogfood: password stores by name (SourceTree passwd, Xshell .mpw, KeePass-format safes of an IDE); the similar names that are code are not', () => {
  assert.equal(cls('C:\\Users\\x\\AppData\\Local\\Vendor\\Tool\\passwd').secret, true);
  assert.equal(cls('C:\\Users\\x\\Documents\\Terminal Suite\\MasterPassword.mpw').kind, 'password-db');
  assert.equal(cls('C:\\Users\\x\\AppData\\Roaming\\IDE2025\\c.kdbx').kind, 'password-db');
  for (const n of ['passwd.c', 'passwd.h', 'passwd.md', 'mpw.md', 'a.mpw.md', 'passwords.md', 'kdbx.md']) assert.equal(cls(n).secret, false, n);
});

test('SEC-12f dogfood: names that look like secrets but are this project\'s own source stay unflagged', () => {
  for (const n of ['crates/core/src/secret.rs', 'crates/core/src/secrets.rs', 'crates/worker/src/vault.rs', 'crates/ipc/src/auth.rs', 'crates/tsf/src/keymap.rs', 'src/core/secretfile.js', 'tests/fixtures/secret-cases.json', 'screenshots/01-key-saved.png']) assert.equal(cls(n).secret, false, n);
});

test('SEC-12g dogfood: a login or license key under a tool folder is a *.key and stays secret (the price of the SENSITIVE_FILE superset)', () => {
  for (const n of ['C:\\Users\\x\\AppData\\Roaming\\Vendor\\IDE2023\\license.key', '/home/x/.android/maps.key', '/home/x/.tool/sessions/12345.0123abcd.key']) assert.equal(cls(n).secret, true, n);
});

test('SEC-13 needsConfirmation: secret and not allowed -> needs; deny means skip silently, once is spent by consume', () => {
  const gate = new sf.SecretFileGate();
  const p = 'C:\\proj\\.env';
  assert.deepEqual(sf.needsConfirmation('C:\\proj\\main.rs', gate), {needs: false, classification: {secret: false, kind: null, rule: null, label: 'main.rs'}, state: 'ask'});
  let n = sf.needsConfirmation(p, gate);
  assert.deepEqual([n.needs, n.state, n.classification.kind, n.classification.rule], [true, 'ask', 'env', 'env-file']);
  gate.record(p, 'once');
  n = sf.needsConfirmation(p, gate); assert.deepEqual([n.needs, n.state], [false, 'allow']);
  assert.equal(gate.consume(p), true);
  n = sf.needsConfirmation(p, gate); assert.deepEqual([n.needs, n.state], [true, 'ask']);
  gate.record(p, 'deny');
  n = sf.needsConfirmation(p, gate); assert.deepEqual([n.needs, n.state], [true, 'deny']);
  gate.record(p, 'session');
  n = sf.needsConfirmation('c:/proj/.ENV', gate); assert.deepEqual([n.needs, n.state], [false, 'allow']);
  for (const bad of [null, undefined, {}, 'x', {state: 'allow'}, {state: () => 'allow!'}]) assert.equal(sf.needsConfirmation(p, bad).needs, true, 'no usable gate: ask');
  assert.equal(sf.needsConfirmation('D:\\notes\\prod-db.txt', gate, ['prod*']).needs, true);
  assert.equal(sf.needsConfirmation('D:\\notes\\prod-db.txt', gate, []).needs, false);
});

test('SEC-14 properties on random names: never throws, same answer twice, key is stable, separator/case/dot/stream spellings agree', () => {
  let seed = 20261031;
  const rnd = () => { seed = (seed * 1664525 + 1013904223) >>> 0; return seed / 4294967296; };
  const pick = a => a[Math.floor(rnd() * a.length)];
  const frag = ['.env', 'id_', 'rsa', '.pem', '.key', 'secret', 'credentials', '.json', 'x', '.', '-', '_', '.example', '.bak', 'token', '.yml', 'a', 'B', 'dir', '.ssh', '.aws', 'config', '~', ' '];
  const odd = ['\\', '/', ':', '$', '*', '?', u(0xfc), u(0x212a), u(0x1f511), String.fromCharCode(0xd800), u(0x202e), String.fromCharCode(0), '\n', '..', '.'];
  const gate = new sf.SecretFileGate();
  const noStreamNoSlash = s => !/[:\\/]/.test(s);
  const bad = [];
  for (let i = 0; i < 3000; i++) {
    let p = ''; for (let k = 0, n = 1 + Math.floor(rnd() * 8); k < n; k++) p += rnd() < 0.8 ? pick(frag) : pick(odd);
    const pats = rnd() < 0.3 ? [pick(frag) + '*', '**/' + pick(frag), pick(odd) + pick(frag)] : undefined;
    const a = sf.classifyFile({path: p, userPatterns: pats}), b = sf.classifyFile({path: p, userPatterns: pats});
    if (JSON.stringify(a) !== JSON.stringify(b)) bad.push(`not deterministic: ${JSON.stringify(p)}`);
    if (typeof a.secret !== 'boolean' || (a.secret ? !sf.KINDS.includes(a.kind) || typeof a.rule !== 'string' : a.kind !== null || a.rule !== null) || typeof a.label !== 'string') bad.push(`bad shape: ${JSON.stringify(p)} ${JSON.stringify(a)}`);
    const k = gate.key(p);
    if (gate.key(k) !== k) bad.push(`key not idempotent: ${JSON.stringify(p)} -> ${JSON.stringify(k)} -> ${JSON.stringify(gate.key(k))}`);
    if (/^[\x20-\x7e]*$/.test(p) && noStreamNoSlash(p) && /[A-Za-z0-9_]/.test(p[0]) && !/\.\.?$/.test(p) && p.trim() === p) {
      for (const v of [p.toUpperCase(), 'C:\\x\\' + p, '/x/' + p, p + '.', p + '::$DATA', p + ' .']) if (sf.classifyFile({path: v}).secret !== sf.classifyFile({path: p}).secret) bad.push(`spelling differs: ${JSON.stringify(p)} vs ${JSON.stringify(v)}`);
    }
  }
  assert.deepEqual(bad.slice(0, 10), []);
});

test('SEC-15 the module is pure: no require, no fs, no clock or randomness of its own (the gate takes now)', () => {
  const src = fs.readFileSync(path.join(__dirname, '..', '..', 'editor-bridges', 'vscode', 'src', 'core', 'secretfile.js'), 'utf8');
  assert.doesNotMatch(src, /\brequire\s*\(/); assert.doesNotMatch(src, /\bprocess\b|\bglobalThis\b|\bsetTimeout\b|\bsetInterval\b|\bfs\./);
  assert.doesNotMatch(src, /Math\.random|new Date\(|Date\.now\(|performance\.now|hrtime/);
  assert.match(src, /o\.now : Date\.now/, 'the clock is only a default for the injected one');
  assert.doesNotMatch(src, /new RegExp\([^)]*(?:pattern|text|path|input)/, 'no RegExp is built from user input');
  assert.doesNotMatch(src, /\bcatch\b/, 'nothing swallows an error into "not secret"');
});
