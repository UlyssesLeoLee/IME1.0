'use strict';
// Secret-class FILE NAMES: decides from the name alone whether a file may hold secrets, so the caller can ask the user
// BEFORE reading it (companion of secrets.js, which scans content). This module never sees content, never touches the
// file system or the clock (the gate takes `now`), and never turns user input into a RegExp. Pure and deterministic.
//
// What the wiring does with it:
//   needs=true, state 'ask'  -> show the confirmation, then gate.record(path, choice)
//   needs=true, state 'deny' -> refuse silently (the user already said no; do not nag on every keystroke)
//   needs=false but classification.secret -> the gate allows it: still call gate.consume(path) right before the read, so
//                                             a 'once' grant is spent; consume() is false for a path never recorded
//
// Known limits, by design: a symlink, junction or hard link hides its target behind an innocent name, so the wiring must
// resolve the real path first; 8.3 short names (`ENV~1`) are out of scope; names are never decoded (`%2E` stays literal),
// pass an fsPath, not a URI; case is folded for ASCII only, so two non-ASCII spellings are two keys (the user is asked
// again, which fails safe) and a Kelvin sign never merges two different files under one key; a POSIX file name that
// contains a colon shares its key with the part before the colon (NTFS stream syntax wins).

const KINDS = Object.freeze(['env', 'private-key', 'keystore', 'credential-file', 'cloud-credentials', 'token-store', 'infra-state', 'password-db', 'user-pattern']);

// ---------------------------------------------------------------- rule table
// Declarative on purpose (the Rust core gets a parity port): {id, kind, match}, evaluated in order, first hit wins.
// match = exactly one primary form plus optional filters, all applied to the lower-cased base name:
//   base: [glob]               any glob matches the whole base name
//   head: [text], ext: [text]  head = text before the first dot (a leading dot is skipped), ext = text after the last dot
//   not:  [glob]               none may match (an exclusion always beats the inclusion of its own entry)
//   dir:  [a/b, c, ...]        alternatives; each is a run of parent segments (joined by `/`) that sit next to each other, in this
//                              order, somewhere above the file (`.aws/sso/cache` is also inside `.aws`)
// Glob: `*` any characters, `?` one character (neither crosses a separator), `**` anything including separators.
const freeze = rule => Object.freeze({id: rule.id, kind: rule.kind, match: Object.freeze(Object.fromEntries(Object.entries(rule.match).map(([k, v]) => [k, Object.freeze([...v])])))});

// Template names carry no secrets by convention: they are committed with placeholder values on purpose.
const TEMPLATE = Object.freeze(['*.example', '*.sample', '*.template', '*.dist', '*.default', '*.defaults', '*.tpl', '*.tmpl', '*-example', '*_example', '*-sample', '*_sample', '*-template', '*_template',
  '*.example.*', '*.sample.*', '*.template.*', '*.dist.*', '*.tpl.*', '*.tmpl.*']); // the last row: secrets.example.json

// Data formats that can hold a secret; source code and documents (ts, js, rs, py, md ...) are deliberately absent.
const DATA_EXT = Object.freeze(['json', 'yaml', 'yml', 'toml', 'ini', 'env', 'txt', 'properties', 'xml', 'conf', 'cfg', 'plist']);
const PUBLIC = Object.freeze(['*.pub']); // a public key is not a secret
const LOGS = Object.freeze(['*.log']);

// Not secret by name, on purpose (a name check cannot see inside them): source and docs (credentials.ts, secrets.rs, keys.js,
// env.ts, token.js, README.md, passwords.md), key.txt, settings_local.py, application-prod.properties, appsettings.*.json,
// .yarnrc.yml and nuget.config (every project has one), ciphertext (credentials.yml.enc, *.sops.yaml, sealed secrets),
// public material (*.pub, *.crt, *.cer, *.csr, *.mobileprovision, known_hosts, .aws/config), design tokens (tokens.json),
// locale files (auth.json) and templates (.env.example). The superset rule of secrets.js forces a few false positives that
// stay: Keynote `*.key`, public CA bundles `*.pem`, Android `debug.keystore`.
const DEFAULT_RULES = Object.freeze([
  // --- env files. A virtualenv directory named `.env` is a directory segment, not a base name, so only the names of the
  // files inside it count.
  {id: 'env-file', kind: 'env', match: {base: ['.env', '.env.*', '.env-*', '.env_*'], not: TEMPLATE}}, // .env, .env.local, .env.production, .env-dev
  {id: 'env-extension', kind: 'env', match: {base: ['*.env'], not: [...TEMPLATE, 'example.env', 'sample.env', 'template.env', 'default.env', 'defaults.env', 'dist.env']}}, // prod.env, docker.env
  {id: 'envrc', kind: 'env', match: {base: ['.envrc', '.envrc.*'], not: TEMPLATE}}, // direnv: exports secrets
  {id: 'dev-vars', kind: 'env', match: {base: ['.dev.vars', '.dev.vars.*'], not: TEMPLATE}}, // Cloudflare Workers local secrets
  {id: 'local-settings', kind: 'credential-file', match: {base: ['local.settings.json'], not: TEMPLATE}}, // Azure Functions: git-ignored for holding connection strings
  // --- private keys
  {id: 'ssh-dir', kind: 'private-key', match: {dir: ['.ssh'], base: ['*'], not: [...PUBLIC, 'known_hosts*', 'authorized_keys*', 'config', 'allowed_signers']}},
  {id: 'ssh-private-key', kind: 'private-key', match: {base: ['id_rsa*', 'id_dsa*', 'id_ecdsa*', 'id_ed25519*'], not: PUBLIC}}, // id_ed25519_work too
  {id: 'ssh-named-key', kind: 'private-key', match: {base: ['*_rsa', '*-rsa', '*_dsa', '*-dsa', '*_ecdsa', '*-ecdsa', '*_ed25519', '*-ed25519']}}, // deploy_ed25519
  {id: 'key-extension', kind: 'private-key', match: {base: ['*.pem', '*.key', '*.ppk', '*.p8', '*.pk8', '*.snk']}}, // p8 = Apple AuthKey_*.p8, pk8 = Android, snk = .NET strong-name pair, key = Rails master.key
  {id: 'key-material', kind: 'private-key', match: {base: ['adbkey', 'ssh_host_*_key', 'private_key', 'private-key', 'privatekey', 'secret_key', 'secret-key', 'secretkey', 'master_key', 'master-key', 'encryption_key', 'encryption-key', 'signing_key', 'signing-key']}}, // extension-less key files: adb (Android) private key, sshd host keys
  {id: 'gnupg-dir', kind: 'private-key', match: {dir: ['.gnupg', 'appdata/roaming/gnupg'], base: ['*'], not: ['*.conf', 'pubring.*', 'trustdb.gpg', 's.*', '.#lk*', ...PUBLIC, ...LOGS]}}, // s.* are agent sockets, .#lk* lock files (found in a real ~/.gnupg); the second location is Gpg4win
  // --- keystores and password databases
  {id: 'keystore-extension', kind: 'keystore', match: {base: ['*.pfx', '*.p12', '*.pkcs12', '*.jks', '*.jceks', '*.keystore', '*.bks', '*.keychain', '*.keychain-db']}},
  {id: 'keystore-properties', kind: 'credential-file', match: {base: ['keystore.properties', 'key.properties']}}, // Android / Flutter signing passwords
  {id: 'wallet', kind: 'password-db', match: {base: ['wallet.dat', '*.wallet']}},
  {id: 'password-database', kind: 'password-db', match: {base: ['*.kdbx', '*.kdb', '*.psafe3', '*.1pux', '*.1pif', '*.mpw']}},
  {id: 'password-list', kind: 'password-db', match: {head: ['password', 'passwords', 'passwd'], ext: ['txt', 'csv', 'json', 'xlsx', 'xls', 'yml', 'yaml', 'ini'], not: TEMPLATE}},
  // --- cloud credentials (specific places first, so their kind wins over the generic `credentials` rule)
  {id: 'aws-credentials', kind: 'cloud-credentials', match: {dir: ['.aws'], base: ['credentials']}}, // .aws/config is not secret
  {id: 'aws-sso-cache', kind: 'cloud-credentials', match: {dir: ['.aws/sso/cache'], base: ['*']}},
  {id: 'aws-cli-cache', kind: 'cloud-credentials', match: {dir: ['.aws/cli/cache'], base: ['*']}},
  {id: 'azure-dir', kind: 'cloud-credentials', match: {dir: ['.azure'], base: ['*'], not: LOGS}},
  {id: 'gcloud-dir', kind: 'cloud-credentials', match: {dir: ['.config/gcloud', 'appdata/roaming/gcloud'], base: ['*'], not: LOGS}}, // the second is the Windows location
  {id: 'kube-config', kind: 'cloud-credentials', match: {dir: ['.kube'], base: ['config']}},
  {id: 'kubeconfig', kind: 'cloud-credentials', match: {base: ['kubeconfig', '*.kubeconfig', 'kubeconfig.yaml', 'kubeconfig.yml', 'kubeconfig.json']}},
  {id: 'service-account-key', kind: 'cloud-credentials', match: {base: ['*service-account*.json', '*service_account*.json', 'serviceaccount*.json', '*serviceaccountkey*.json', '*firebase-adminsdk*.json'], not: TEMPLATE}},
  {id: 'oauth-client-secret', kind: 'cloud-credentials', match: {base: ['client_secret*.json', 'client-secret*.json'], not: TEMPLATE}}, // Google client_secret_<id>.json
  // --- credential files by name
  {id: 'credentials-file', kind: 'credential-file', match: {base: ['credentials', '*credentials.json', 'credentials.tfrc.json']}}, // .aws/credentials, .claude/.credentials.json, gcp-credentials.json
  {id: 'credentials-data', kind: 'credential-file', match: {head: ['credentials', 'credential'], ext: DATA_EXT, not: TEMPLATE}}, // credentials.toml (cargo), credentials.xml (Jenkins); credentials.yml.enc is ciphertext, credentials.ts is code
  {id: 'secret-data', kind: 'credential-file', match: {head: ['secret', 'secrets'], ext: DATA_EXT, not: TEMPLATE}}, // secrets.yml, secret.json, secrets.dev.json
  {id: 'secret-extension', kind: 'credential-file', match: {base: ['*.secret', '*.secrets']}},
  {id: 'token-data', kind: 'token-store', match: {head: ['token', 'access_token', 'access-token', 'refresh_token', 'auth_token', 'auth-token', 'api_key', 'apikey', 'api-key'], ext: [...DATA_EXT, 'pickle'], not: TEMPLATE}}, // token.json (Google quickstart); tokens.json is design tokens, token.js is code
  {id: 'token-file', kind: 'token-store', match: {base: ['.token', '*.token', '*_token', '*-token']}}, // .emulator_console_auth_token, logout_token: a name with no extension that ends in token
  {id: 'token-store', kind: 'token-store', match: {base: ['.npmrc', '.netrc', '_netrc', '.git-credentials', '.vault-token', '.dockercfg']}},
  {id: 'docker-config', kind: 'token-store', match: {dir: ['.docker'], base: ['config.json']}},
  {id: 'gh-hosts', kind: 'token-store', match: {dir: ['.config/gh', 'appdata/roaming/github cli'], base: ['hosts.yml']}},
  {id: 'cli-auth-json', kind: 'token-store', match: {dir: ['.codex', 'com.vercel.cli', '.vercel', '.now', '.composer', 'appdata/roaming/composer'], base: ['auth.json']}}, // a bare auth.json is a locale file in half the web projects, so only where a CLI keeps its login
  {id: 'password-file', kind: 'credential-file', match: {base: ['.pypirc', '.htpasswd', '.htdigest', '.pgpass', 'pgpass.conf', '.my.cnf', '.mylogin.cnf', '.s3cfg', '.boto', 'rclone.conf', 'settings-security.xml', 'passwd']}},
  {id: 'git-config', kind: 'credential-file', match: {dir: ['.git'], base: ['config']}}, // remote URLs may embed a token
  {id: 'maven-settings', kind: 'credential-file', match: {dir: ['.m2'], base: ['settings.xml']}}, // server passwords
  {id: 'gradle-home-properties', kind: 'credential-file', match: {dir: ['.gradle'], base: ['gradle.properties']}}, // signing and repository tokens
  {id: 'vault-password', kind: 'credential-file', match: {base: ['.vault_pass*', '.vault-pass*', 'vault_pass*', 'vault-pass*']}}, // Ansible vault password files
  {id: 'vpn-profile', kind: 'credential-file', match: {base: ['*.ovpn']}}, // inline keys and auth-user-pass
  {id: 'browser-secrets', kind: 'token-store', match: {base: ['cookies', 'cookies.txt', 'cookies.sqlite', 'login data', 'login data for account', 'logins.json', 'key3.db', 'key4.db', 'signons.sqlite']}},
  // --- infrastructure state and variables
  {id: 'infra-state', kind: 'infra-state', match: {base: ['terraform.tfstate*', '*.tfstate', '*.tfstate.*', '*.tfvars', '*.tfvars.json'], not: TEMPLATE}} // state holds every secret value in clear
].map(freeze));

// A backup or editor-swap copy of a secret file is as secret as the file (`.env.swp` holds its text): peel these first.
const BACKUP_SUFFIXES = Object.freeze(['~', '.bak', '.backup', '.old', '.orig', '.save', '.swp', '.swo', '.tmp', '-journal', '-wal', '-shm']); // the last three: SQLite sidecars keep copies of pages (Cookies-journal)

// ---------------------------------------------------------------- name normalisation (shared by classify and key)
// Linear scans only: no RegExp over untrusted text with nested or overlapping quantifiers.
const MAX_PATH = 1 << 16; // longer input keeps its tail: the name and the nearest directories
const lowerAscii = s => s.replace(/[A-Z]+/g, m => m.toLowerCase());
const trimTail = s => { let e = s.length; while (e > 0 && (s.charCodeAt(e - 1) === 46 || s.charCodeAt(e - 1) === 32)) e--; return s.slice(0, e); }; // Win32 drops trailing dots and spaces
const cutStream = s => { const i = s.indexOf(':'); return i < 0 ? s : s.slice(0, i); }; // NTFS `name:stream:$DATA`
const hex4 = n => n.toString(16).padStart(4, '0');
const LABEL_JUNK = new RegExp('[' + [[0, 31], [127, 159], [0x61c, 0x61c], [0x200b, 0x200f], [0x202a, 0x202e], [0x2060, 0x2064], [0x2066, 0x2069], [0xfeff, 0xfeff]].map(([a, b]) => '\\u' + hex4(a) + '-\\u' + hex4(b)).join('') + ']', 'g');
// What a dialog shows: no control or bidirectional-override characters (they could disguise the name), at most 120 characters.
const cleanLabel = s => { const c = Array.from((s.length > 400 ? s.slice(0, 400) : s).replace(LABEL_JUNK, '?')); return c.length > 120 ? c.slice(0, 120).join('') + '...' : c.join(''); };

const NO_NAME = Object.freeze({root: '', key: '', label: '', base: '', alt: '', dirs: Object.freeze([])});
// -> {root, key, label, base, alt, dirs}: key = canonical lower-case path; base = lower-case base name with stream and trailing
// dots/spaces removed; alt = the same name with the colon part kept ('' when there is none); dirs = lower-case parent segments.
function analyse(input) {
  if (typeof input !== 'string') return NO_NAME;
  let s = (input.length > MAX_PATH ? input.slice(-MAX_PATH) : input).trimStart().split('\\').join('/');
  const pre = lowerAscii(s.slice(0, 8));
  if (pre === '//?/unc/') s = '//' + s.slice(8); // \\?\UNC\server\share
  else if (pre.startsWith('//?/') || pre.startsWith('//./')) s = s.slice(4); // \\?\C:\ and \\.\C:\
  let root = '';
  if (/^[A-Za-z]:/.test(s)) { root = lowerAscii(s.slice(0, 2)); s = s.slice(2); if (s[0] === '/') root += '/'; } // the drive goes before streams are cut
  else if (s.startsWith('//') && s[2] !== '/') root = '//'; // UNC: exactly two separators
  else if (s[0] === '/') root = '/';
  const segs = [];
  for (const p of s.split('/')) {
    if (p === '' || p === '.') continue;
    if (p === '..') { if (segs.length && segs[segs.length - 1].v !== '..') segs.pop(); else if (!root) segs.push({v: '..', w: '..'}); continue; }
    const v = trimTail(cutStream(p)); // every segment: Win32 normalises each one, so `.ssh.\id_x` is still inside `.ssh`
    if (v !== '') segs.push({v, w: trimTail(p)});
  }
  if (!segs.length) return input.length ? {...NO_NAME, root, key: root} : NO_NAME;
  const last = segs[segs.length - 1];
  return {root, key: root + segs.map(x => lowerAscii(x.v)).join('/'), label: cleanLabel(last.v), base: lowerAscii(last.v),
    alt: altName(last.w), dirs: segs.slice(0, -1).map(x => lowerAscii(x.v))};
}
// The colon kept, for a POSIX name that really contains one (`foo:bar.pem`); not when it is NTFS syntax (`$DATA` in it), so
// `.env.example::$DATA` stays the template it is.
const altName = w => { const c = w.indexOf(':'); return c >= 0 && !w.includes('$', c) ? lowerAscii(w) : ''; };
const peel = name => {
  for (let n = 0; n < 2; n++) {
    const suffix = BACKUP_SUFFIXES.find(x => name.length > x.length && name.endsWith(x));
    if (!suffix) break;
    name = name.slice(0, -suffix.length);
  }
  return name;
};
// The names a rule is tried on: the base name, and (a POSIX name may really contain a colon) the one with the colon part kept.
const namesOf = a => a.alt ? [peel(a.base), peel(a.alt)] : [peel(a.base)];

// ---------------------------------------------------------------- glob matcher
// An NFA run: the set of live pattern positions advances once per input character, so the cost is O(text x pattern) and a
// pathological `*a*a*a*b` cannot backtrack. `budget` ({left}) bounds the total work of one classification.
const T_LIT = 0, T_ANY1 = 1, T_STAR = 2, T_DEEP = 3, T_DIRS = 4;
function compileGlob(pattern) {
  const kinds = [], chars = [], cps = Array.from(pattern);
  for (let i = 0; i < cps.length;) {
    if (cps[i] === '*') {
      let j = i; while (cps[j] === '*') j++;
      if (j - i === 1) { kinds.push(T_STAR); chars.push(''); }
      else if (cps[j] === '/' && (i === 0 || cps[i - 1] === '/')) { kinds.push(T_DIRS); chars.push(''); j++; } // `**/` also matches zero directories
      else { kinds.push(T_DEEP); chars.push(''); }
      i = j;
    } else { kinds.push(cps[i] === '?' ? T_ANY1 : T_LIT); chars.push(cps[i]); i++; }
  }
  return {kinds, chars};
}
// true, false, or null when the budget ran out
function globMatch(g, text, budget) {
  const {kinds, chars} = g, n = kinds.length, mark = new Int32Array(2 * n + 2); // states n+1+i are the "inside a `**/`" loops
  let stamp = 0, cur = [], next = [];
  const add = (set, i) => { // a state plus everything reachable without consuming a character
    for (;;) {
      if (mark[i] === stamp) return;
      mark[i] = stamp; set.push(i);
      if (i < n && kinds[i] !== T_LIT && kinds[i] !== T_ANY1) i++; else return;
    }
  };
  stamp++; add(cur, 0);
  for (const c of text) {
    if (budget && (budget.left -= cur.length + 1) < 0) return null;
    stamp++; next.length = 0;
    for (const s of cur) {
      if (s > n) { add(next, s); if (c === '/') add(next, s - n); } // loop of the `**/` at s-n-1: any character stays, a separator leaves
      else if (s < n) {
        const k = kinds[s];
        if (k === T_LIT) { if (chars[s] === c) add(next, s + 1); }
        else if (k === T_ANY1) { if (c !== '/') add(next, s + 1); }
        else if (k === T_STAR) { if (c !== '/') add(next, s); }
        else if (k === T_DEEP) add(next, s);
        else { add(next, n + 1 + s); if (c === '/') add(next, s + 1); }
      }
    }
    [cur, next] = [next, cur];
    if (!cur.length) return false;
  }
  return cur.includes(n);
}

// ---------------------------------------------------------------- built-in rules
const COMPILED = DEFAULT_RULES.map(r => ({id: r.id, kind: r.kind, base: (r.match.base || []).map(compileGlob), not: (r.match.not || []).map(compileGlob),
  head: r.match.head || [], ext: r.match.ext || [], dir: (r.match.dir || []).map(d => d.split('/'))}));
const splitName = n => { const h = n[0] === '.' ? n.slice(1) : n, f = h.indexOf('.'), l = n.lastIndexOf('.'); return f < 0 || l <= 0 ? null : {head: h.slice(0, f), ext: n.slice(l + 1)}; };
const hasRun = (dirs, seq) => { for (let i = 0; i + seq.length <= dirs.length; i++) if (seq.every((s, k) => dirs[i + k] === s)) return true; return false; };
const hasDirs = (dirs, alts) => alts.length === 0 || alts.some(seq => hasRun(dirs, seq));
function entryMatches(e, name, dirs) {
  if (!hasDirs(dirs, e.dir)) return false;
  const p = e.base.length ? null : splitName(name);
  const hit = e.base.length ? e.base.some(g => globMatch(g, name)) : !!p && e.head.includes(p.head) && e.ext.includes(p.ext);
  return hit && !e.not.some(g => globMatch(g, name));
}

// ---------------------------------------------------------------- user patterns
const MAX_PATTERNS = 200, MAX_PATTERN_LEN = 300, MAX_PATTERN_SCAN = 2000, MAX_MATCH_TEXT = 1024, USER_BUDGET = 2000000;
// Backslash is a separator. A pattern with a separator that is not rooted (`/..`, `**..`, `c:..`) is matched from any directory
// (implicit `**/`), because the setting is written relative to the workspace but the path arrives absolute; a trailing `/` means
// "everything below". A pattern without a separator is matched against the base name.
function prepare(text) {
  let p = lowerAscii(text).split('\\').join('/');
  while (p.startsWith('./')) p = p.slice(2);
  const sep = p.includes('/');
  if (p.endsWith('/')) p += '**';
  if (sep && !p.startsWith('/') && !p.startsWith('**') && !/^[a-z]:/.test(p)) p = '**/' + p;
  return {glob: compileGlob(p), onBase: !sep};
}
function userGlobs(list) {
  const out = [];
  if (!Array.isArray(list)) return out;
  for (let i = 0; i < list.length && i < MAX_PATTERN_SCAN && out.length < MAX_PATTERNS; i++) {
    const t = typeof list[i] === 'string' ? list[i].trim() : '';
    if (t !== '' && t.length <= MAX_PATTERN_LEN) out.push(prepare(t)); // a too-long pattern is skipped, never cut (cutting would change its meaning)
  }
  return out;
}
function matchesUser(list, a, names) {
  const globs = userGlobs(list);
  if (!globs.length) return false;
  const dirPart = a.key.slice(0, a.key.length - a.base.length), budget = {left: USER_BUDGET};
  for (const g of globs) for (const n of names) {
    const text = g.onBase ? n : (dirPart + n).slice(-MAX_MATCH_TEXT);
    if (globMatch(g.glob, text, budget) !== false) return true; // null = budget spent: fail closed, ask the user
  }
  return false;
}

// ---------------------------------------------------------------- classification
// A bare string is accepted as the path (a caller bug then fails closed instead of silently reading); anything else that is
// not an object is an empty request. Nothing here catches errors on purpose: a bug must not turn into "not secret".
function classifyFile(arg) {
  const o = typeof arg === 'string' ? {path: arg} : arg && typeof arg === 'object' ? arg : {};
  const a = analyse(o.path), plain = {secret: false, kind: null, rule: null, label: a.label};
  if (!a.base) return plain;
  const names = namesOf(a);
  for (const e of COMPILED) if (names.some(n => entryMatches(e, n, a.dirs))) return {secret: true, kind: e.kind, rule: e.id, label: a.label};
  if (matchesUser(o.userPatterns, a, names)) return {secret: true, kind: 'user-pattern', rule: 'user-pattern', label: a.label}; // wins over a built-in carve-out
  return plain;
}

// ---------------------------------------------------------------- decision store
// Decisions are kept per canonical key, in memory only, and never hold content. 'once' is spent by one consume() and also
// expires (a stale approval must not silently authorise a later, unrelated read); 'session' and 'deny' last sessionTtlMs.
// A clock that moves backwards expires everything (fail closed).
class SecretFileGate {
  constructor(opts) {
    const o = opts && typeof opts === 'object' ? opts : {};
    const pos = (v, d) => Number.isFinite(v) && v > 0 ? v : d;
    this.now = typeof o.now === 'function' ? o.now : Date.now;
    this.sessionTtlMs = pos(o.sessionTtlMs, 8 * 3600 * 1000);
    this.onceTtlMs = pos(o.onceTtlMs, 5 * 60 * 1000);
    this.maxEntries = Math.max(1, Math.floor(pos(o.maxEntries, 500)));
    this.map = new Map(); // insertion order = age: the first key is the oldest
  }
  key(path) { return analyse(path).key; }
  _live(e, t) { return t >= e.at && t < e.expiresAt; } // written so that a NaN clock is never live
  _entry(path) {
    const k = this.key(path), e = k ? this.map.get(k) : null;
    if (!e) return null;
    if (!this._live(e, this.now())) { this.map.delete(k); return null; }
    return e;
  }
  state(path) { const e = this._entry(path); return !e ? 'ask' : e.choice === 'deny' ? 'deny' : 'allow'; }
  record(path, choice) {
    const a = analyse(path), ttl = choice === 'once' ? this.onceTtlMs : choice === 'session' || choice === 'deny' ? this.sessionTtlMs : 0;
    if (!a.base || !ttl) return this.state(path); // not a file name or an unknown choice: nothing changes
    const t = this.now();
    for (const [k, e] of this.map) if (!this._live(e, t)) this.map.delete(k); // the dead go before a live decision is evicted
    this.map.delete(a.key); // a new answer replaces the old one and counts as the newest
    this.map.set(a.key, {choice, label: a.label, at: t, expiresAt: t + ttl});
    while (this.map.size > this.maxEntries) this.map.delete(this.map.keys().next().value);
    return this.state(path);
  }
  consume(path) {
    const e = this._entry(path);
    if (!e || e.choice === 'deny') return false;
    if (e.choice === 'once') this.map.delete(this.key(path));
    return true;
  }
  revoke(path) { const k = this.key(path); return k ? this.map.delete(k) : false; }
  clear() { const n = this.map.size; this.map.clear(); return n; }
  list() {
    const t = this.now(), out = [];
    for (const [k, e] of this.map) {
      if (!this._live(e, t)) { this.map.delete(k); continue; }
      out.push({key: k, label: e.label, state: e.choice === 'deny' ? 'deny' : 'allow', expiresAt: e.expiresAt});
    }
    return out;
  }
}

// needs=true when the file is secret-class and the gate does not currently allow it; `state` is the raw gate state.
function needsConfirmation(path, gate, userPatterns) {
  const classification = classifyFile({path, userPatterns});
  const state = gate && typeof gate.state === 'function' ? gate.state(path) : 'ask';
  return {needs: classification.secret && state !== 'allow', classification, state};
}

module.exports = {classifyFile, DEFAULT_RULES, SecretFileGate, needsConfirmation, KINDS};
