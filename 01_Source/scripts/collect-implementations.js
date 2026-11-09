'use strict';
// Writes the implementations of the catalogue into the manifests of the assets.
//
//   node scripts/collect-implementations.js            # rewrite the manifests and the test manifests
//   node scripts/collect-implementations.js --check    # only report what would change (exit 1 when something would)
//
// The catalogue holds the knowledge cards; an implementation is code that somebody can call. The implementations of
// this round live in the crate `crates/tase-impls` (the fifty algorithms of the catalogue). This script is the
// bridge between the two: it reads the markers the code carries, and it writes them into the manifest of the asset.
//
//   * every public function of the crate names its asset in its doc comment:
//
//     /// * asset: `algorithm.sorting.merge_sort`
//     /// * card: `01_Source/assets/tase-catalog/assets/algorithm.sorting.merge_sort/card.md`
//
//   * the test of an implementation is named after the asset (`merge_sort_...`), so the catalogue can name the
//     tests that stand behind an implementation - the certification follows the evidence: an implementation with a
//     named test is TESTED, one without is only IMPLEMENTED.
//
// The digest of an implementation is the SHA-256 of the file that holds it (the text is hashed as its LF
// normalised content, exactly like MANIFEST_SHA256.txt does it), so it is a real digest and never a placeholder.
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

const SOURCE_ROOT = path.resolve(__dirname, '..');
const REPO_ROOT = path.resolve(SOURCE_ROOT, '..');
const CATALOG_DIR = path.join(SOURCE_ROOT, 'assets', 'tase-catalog');
const IMPL_DIR = path.join(SOURCE_ROOT, 'crates', 'tase-impls', 'src', 'algorithm');
const TESTS_DIR = path.join(CATALOG_DIR, 'implementations');
const check = process.argv.includes('--check');

// The states of an implementation (design §14.4). APPROVED needs a person to look at the code, so the script never
// writes it: the highest state it writes is TESTED.
const STATUS_IMPLEMENTED = 'IMPLEMENTED';
const STATUS_TESTED = 'TESTED';

// The digest of a text file: the LF normalised content (the bytes of its git blob).
const sha256OfText = text =>
  crypto.createHash('sha256').update(Buffer.from(text.replace(/\r\n/g, '\n'), 'utf8')).digest('hex');

const readCargoVersion = () => {
  const file = path.join(SOURCE_ROOT, 'crates', 'tase-impls', 'Cargo.toml');
  const text = fs.readFileSync(file, 'utf8');
  const match = /^version\s*=\s*"([^"]+)"/m.exec(text);
  if (!match) throw new Error('the crate of the implementations has no version');
  return match[1];
};

// The signature of the function that follows a marker: the name, the generics, the parameters (the parentheses are
// balanced, so a parameter that carries a closure - `impl FnMut(usize) -> usize` - is not cut in half) and the
// return type.
function signatureAfter(text, from) {
  const head = /pub\s+fn\s+([a-z0-9_]+)/.exec(text.slice(from));
  if (!head) return null;
  let at = from + head.index + head[0].length;
  // the generics of the function: <...>, balanced
  while (at < text.length && text[at] === ' ') at++;
  if (text[at] === '<') {
    let depth = 0;
    while (at < text.length) {
      if (text[at] === '<') depth++;
      else if (text[at] === '>') {
        depth--;
        if (depth === 0) {
          at++;
          break;
        }
      }
      at++;
    }
  }
  // the parameters: (...), balanced
  while (at < text.length && /\s/.test(text[at])) at++;
  if (text[at] !== '(') return null;
  let depth = 0;
  const start = at;
  while (at < text.length) {
    if (text[at] === '(') depth++;
    else if (text[at] === ')') {
      depth--;
      if (depth === 0) {
        at++;
        break;
      }
    }
    at++;
  }
  const parameters = text.slice(start + 1, at - 1);
  // the return type: up to the body or the end of the declaration
  const rest = text.slice(at);
  const tail = /^\s*(->\s*[^{;]+)?/.exec(rest);
  const returns = (tail && tail[1] ? tail[1] : '').trim();
  return {
    function: head[1],
    signature: ('pub fn ' + head[1] + text.slice(from + head.index + head[0].length, at) + (returns ? ' ' + returns : ''))
      .replace(/\s+/g, ' ')
      .trim(),
  };
}

// ---- read the code ----------------------------------------------------------------------------------------------
// Every marker of an implementation: the asset, the function and the signature.
function readImplementations() {
  const found = new Map();
  if (!fs.existsSync(IMPL_DIR)) return found;
  for (const name of fs.readdirSync(IMPL_DIR).sort()) {
    if (!name.endsWith('.rs') || name === 'mod.rs') continue;
    const file = path.join(IMPL_DIR, name);
    const text = fs.readFileSync(file, 'utf8');
    const relative = path.relative(REPO_ROOT, file).split(path.sep).join('/');
    const digest = sha256OfText(text);
    // the markers: "* asset: `algorithm.sorting.merge_sort`" (a bullet, a heading or a dash all count) followed by
    // the signature of the function
    const marker = /(?:\*|#|-)\s*asset:\s*`([a-z0-9_.]+)`/g;
    let match;
    while ((match = marker.exec(text)) !== null) {
      const asset = match[1];
      const signature = signatureAfter(text, match.index + match[0].length);
      if (!signature) {
        console.error('note: the marker of ' + asset + ' in ' + relative + ' has no function after it');
        continue;
      }
      const short = asset.split('.').pop();
      // The tests of the file that stand behind this implementation: they are named after the asset or after the
      // function that implements it (`merge_sort_...`, `top_k_...` for the asset `algorithm.sorting.top_k_heap`).
      const tests = [];
      const prefixes = [short + '_'];
      if (!prefixes.includes(signature.function + '_')) prefixes.push(signature.function + '_');
      const testPattern = /#\[test\][\s\S]{0,200}?fn\s+([a-z0-9_]+)/g;
      let test;
      while ((test = testPattern.exec(text)) !== null) {
        if (prefixes.some(prefix => test[1].startsWith(prefix))) tests.push(test[1]);
      }
      found.set(asset, {
        asset,
        file: relative,
        digest,
        function: signature.function,
        signature: signature.signature,
        tests,
      });
    }
  }
  return found;
}

// ---- write the manifests ----------------------------------------------------------------------------------------
function writeJson(file, value) {
  const text = JSON.stringify(value, null, 2) + '\n';
  if (check) {
    const current = fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : '';
    if (current !== text) {
      console.log('stale  ' + path.relative(REPO_ROOT, file));
      return false;
    }
    return true;
  }
  fs.mkdirSync(path.dirname(file), {recursive: true});
  fs.writeFileSync(file, text, 'utf8');
  return true;
}

const version = readCargoVersion();
const implementations = readImplementations();
if (!implementations.size) {
  console.error('no implementation was found: the markers "* asset: `...`" are missing');
  process.exit(1);
}

let changed = 0;
let tested = 0;
for (const [asset, info] of implementations) {
  const manifestFile = path.join(CATALOG_DIR, 'assets', asset, 'manifest.json');
  if (!fs.existsSync(manifestFile)) {
    console.error('FAIL: the asset ' + asset + ' is not in the catalogue (no manifest)');
    process.exit(1);
  }
  const manifest = JSON.parse(fs.readFileSync(manifestFile, 'utf8'));
  const status = info.tests.length ? STATUS_TESTED : STATUS_IMPLEMENTED;
  if (status === STATUS_TESTED) tested += 1;
  const implementationId = 'tase-impls.rust.' + asset;
  manifest.implementations = [
    {
      implementation_id: implementationId,
      language: 'rust',
      version,
      status,
      engine: 'rust',
      path: info.file,
      sha256: info.digest,
      parameters_schema: info.signature,
      tests_manifest: '01_Source/assets/tase-catalog/implementations/' + asset + '.json',
    },
  ];
  const before = fs.readFileSync(manifestFile, 'utf8');
  const after = JSON.stringify(manifest, null, 2) + '\n';
  if (before !== after) {
    changed += 1;
    console.log('write  ' + asset + '  (' + status + ', ' + info.tests.length + ' tests)');
    if (!check) fs.writeFileSync(manifestFile, after, 'utf8');
  }
  const testsManifest = {
    schema_version: 'tase.implementation.tests.v1',
    asset_id: asset,
    implementation_id: implementationId,
    status,
    language: 'rust',
    version,
    source: {path: info.file, sha256: info.digest, function: info.function},
    tests: info.tests,
    command: 'cargo test -p tase-impls ' + info.function,
  };
  if (!writeJson(path.join(TESTS_DIR, asset + '.json'), testsManifest)) changed += 1;
}

console.log('');
console.log('implementations : ' + implementations.size + ' (' + tested + ' with named tests, ' + (implementations.size - tested) + ' without)');
console.log('files to write  : ' + changed);
if (check && changed) {
  console.error('');
  console.error('OK: the catalogue does not carry the implementations yet - run the script without --check');
  process.exit(1);
}
