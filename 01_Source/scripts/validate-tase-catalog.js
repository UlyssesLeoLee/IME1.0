'use strict';
// Checks the TASE unified asset catalog: the index, the 93 manifests and the 93
// knowledge cards.
//
//   node scripts/validate-tase-catalog.js [dir]
//
// The directory defaults to assets/tase-catalog. The exit code is 0 when every
// check passes and 1 when at least one check fails.
//
// What is checked (the rules come from the design book: 01_Requirements.md
// §14.1-§14.4 and 03_Detailed_Design.md §19.1-§19.5):
//   1. catalog.json holds exactly 93 assets, and the groups are GoF 23 / modern
//      engineering patterns 20 / algorithms 50.
//   2. every asset has a readable manifest.json with the fields of §19.1, the
//      schema version tase.asset.v1, and an asset_id that matches both its
//      directory and its catalog entry.
//   3. name carries zh-CN and en-US, intent is not empty, and applicability has
//      constraints, counterexamples (not empty) and alternatives.
//   4. every alternative really exists in the catalog, and no alias is an asset
//      id: an alias may not inflate the number of assets.
//   5. status is DOCUMENTED or REVIEWED. An implementation carries its own state (IMPLEMENTED / TESTED /
//      APPROVED / DEPRECATED / REVOKED), names the file it lives in, carries the real digest of that file and
//      names the tests that stand behind it: an implementation that is not APPROVED must carry a sha256, and a
//      digest that is there must be the digest of the file it names.
//   6. an algorithm states a real complexity (never n/a); a pattern has a card.
//   7. every card exists, is not empty and carries the eight required sections.
//   8. every asset id quoted inside a card exists in the catalog.
//   9. the summary prints the counts per group and per namespace, plus the list
//      of what is missing.
const fs = require('node:fs');
const path = require('node:path');

const SOURCE_ROOT = path.resolve(__dirname, '..');
const REPO_ROOT = path.resolve(SOURCE_ROOT, '..');
const catalogDir = path.resolve(process.argv[2] || path.join(SOURCE_ROOT, 'assets', 'tase-catalog'));

// The eight sections every knowledge card must carry.
const CARD_SECTIONS = [
  '## 一句话定位',
  '## 典型源文件形状',
  '## 适用条件',
  '## 反例',
  '## 并发风险',
  '## 替代方案',
  '## 测试说明',
  '## 实现注意',
];

// The fields of the manifest of 03_Detailed_Design.md §19.1.
const REQUIRED_FIELDS = [
  'schema_version',
  'asset_id',
  'version',
  'kind',
  'name',
  'aliases',
  'category',
  'status',
  'intent',
  'applicability',
  'complexity',
  'implementations',
  'provenance',
];

const SCHEMA_VERSION = 'tase.asset.v1';
const EXPECTED_TOTAL = 93;
const EXPECTED_GROUPS = { gof: 23, modern: 20, algorithm: 50 };
const ALLOWED_STATUS = ['DOCUMENTED', 'REVIEWED'];
// The states of an implementation (design §14.4). The card keeps its own state; the implementation has its own.
const ALLOWED_IMPLEMENTATION_STATUS = ['IMPLEMENTED', 'TESTED', 'APPROVED', 'DEPRECATED', 'REVOKED'];
// The fields an implementation must carry (design §19.1, the same list the registry parses).
const REQUIRED_IMPLEMENTATION_FIELDS = [
  'implementation_id',
  'language',
  'version',
  'status',
  'engine',
  'path',
  'sha256',
  'parameters_schema',
  'tests_manifest',
];

// The digest of a text file: the LF normalised content (the bytes of its git blob), exactly like
// MANIFEST_SHA256.txt does it.
const sha256OfText = text =>
  require('node:crypto').createHash('sha256').update(Buffer.from(text.replace(/\r\n/g, '\n'), 'utf8')).digest('hex');

const problems = [];
const missing = [];

function fail(message) {
  problems.push(message);
  console.error('FAIL: ' + message);
}

function ok(message) {
  console.log('ok   ' + message);
}

function readJson(file) {
  let text;
  try {
    text = fs.readFileSync(file, 'utf8');
  } catch (e) {
    return { error: '文件不存在或不可读: ' + e.message };
  }
  if (text.charCodeAt(0) === 0xfeff) {
    return { error: '文件带有 UTF-8 BOM，必须写成无 BOM 的 UTF-8' };
  }
  try {
    return { value: JSON.parse(text) };
  } catch (e) {
    return { error: 'JSON 解析失败: ' + e.message };
  }
}

function isPlainObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function nonEmptyString(value) {
  return typeof value === 'string' && value.trim().length > 0;
}

// ---- 1. the index -------------------------------------------------------------
const catalogFile = path.join(catalogDir, 'catalog.json');
const catalogRead = readJson(catalogFile);
if (catalogRead.error) {
  fail('catalog.json ' + catalogRead.error);
  console.error('');
  console.error('OK: ' + catalogDir + ' - 目录不可用，检查中止');
  process.exit(1);
}
const catalog = catalogRead.value;

if (!isPlainObject(catalog)) {
  fail('catalog.json 的顶层必须是一个对象');
} else {
  if (catalog.schema_version !== SCHEMA_VERSION) {
    fail('catalog.json 的 schema_version 必须是 ' + SCHEMA_VERSION + '，实际是 ' + JSON.stringify(catalog.schema_version));
  }
  if (!Array.isArray(catalog.assets)) {
    fail('catalog.json 缺少 assets 数组');
  } else {
    if (catalog.count !== catalog.assets.length) {
      fail('catalog.json 的 count (' + catalog.count + ') 与 assets 长度 (' + catalog.assets.length + ') 不一致');
    }
    if (catalog.assets.length !== EXPECTED_TOTAL) {
      fail('catalog.json 必须正好 ' + EXPECTED_TOTAL + ' 条资产，实际是 ' + catalog.assets.length + ' 条');
    }
    ok('catalog.json 可读，' + catalog.assets.length + ' 条资产，count 字段一致');
  }
}

const assets = Array.isArray(catalog.assets) ? catalog.assets : [];
const ids = new Set();
const duplicates = [];
for (const asset of assets) {
  const id = asset && asset.id;
  if (typeof id !== 'string' || !id) {
    fail('catalog.json 里有条目的 id 缺失');
    continue;
  }
  if (ids.has(id)) duplicates.push(id);
  ids.add(id);
}
if (duplicates.length) fail('catalog.json 里有重复的资产 id: ' + duplicates.join(', '));

// ---- the groups: GoF 23 / modern 20 / algorithms 50 ----------------------------
function groupOf(id) {
  if (id.startsWith('pattern.gof.')) return 'gof';
  if (id.startsWith('pattern.')) return 'modern';
  return 'algorithm';
}
const groups = { gof: 0, modern: 0, algorithm: 0 };
for (const id of ids) groups[groupOf(id)] += 1;
for (const key of Object.keys(EXPECTED_GROUPS)) {
  if (groups[key] !== EXPECTED_GROUPS[key]) {
    fail('分组 ' + key + ' 应为 ' + EXPECTED_GROUPS[key] + ' 条，实际是 ' + groups[key] + ' 条');
  }
}
if (!problems.length) {
  ok('分组统计正确：GoF ' + groups.gof + ' / 现代模式 ' + groups.modern + ' / 算法 ' + groups.algorithm);
}

// ---- 2..8. every asset ---------------------------------------------------------
const aliasOwners = new Map();
let manifestCount = 0;
let cardCount = 0;

for (const entry of assets) {
  const id = entry.id;
  const dir = path.join(catalogDir, 'assets', id);
  const manifestFile = path.join(dir, 'manifest.json');
  const cardFile = path.join(dir, 'card.md');

  if (!fs.existsSync(manifestFile)) {
    fail(id + ': 缺少 manifest.json');
    missing.push(id + '/manifest.json');
    continue;
  }
  const manifestRead = readJson(manifestFile);
  if (manifestRead.error) {
    fail(id + ': manifest.json ' + manifestRead.error);
    continue;
  }
  const manifest = manifestRead.value;
  manifestCount += 1;

  for (const field of REQUIRED_FIELDS) {
    if (!(field in manifest)) fail(id + ': manifest.json 缺少字段 ' + field);
  }
  if (manifest.schema_version !== SCHEMA_VERSION) {
    fail(id + ': schema_version 必须是 ' + SCHEMA_VERSION + '，实际是 ' + JSON.stringify(manifest.schema_version));
  }
  if (manifest.asset_id !== id) {
    fail(id + ': manifest 的 asset_id (' + JSON.stringify(manifest.asset_id) + ') 与目录名及 catalog 条目不一致');
  }
  if (manifest.kind !== entry.kind) {
    fail(id + ': manifest 的 kind 与 catalog 条目不一致');
  }
  if (manifest.category !== entry.category) {
    fail(id + ': manifest 的 category 与 catalog 条目不一致');
  }
  if (!isPlainObject(manifest.name) || manifest.name['zh-CN'] !== entry.name_zh || manifest.name['en-US'] !== entry.name_en) {
    fail(id + ': manifest 的 name 与 catalog 条目不一致');
  }

  // 3. the readable parts
  if (!isPlainObject(manifest.name)) {
    fail(id + ': name 必须是对象');
  } else {
    if (!nonEmptyString(manifest.name['zh-CN'])) fail(id + ': name 缺少 zh-CN');
    if (!nonEmptyString(manifest.name['en-US'])) fail(id + ': name 缺少 en-US');
  }
  if (!nonEmptyString(manifest.intent)) fail(id + ': intent 不能为空');

  const applicability = manifest.applicability;
  if (!isPlainObject(applicability)) {
    fail(id + ': applicability 必须是对象');
  } else {
    if (!isPlainObject(applicability.constraints)) fail(id + ': applicability.constraints 必须是对象');
    if (!Array.isArray(applicability.counterexamples)) {
      fail(id + ': applicability.counterexamples 必须是数组');
    } else if (applicability.counterexamples.length === 0) {
      fail(id + ': applicability.counterexamples 不能为空');
    }
    if (!Array.isArray(applicability.alternatives)) {
      fail(id + ': applicability.alternatives 必须是数组');
    } else {
      for (const alternative of applicability.alternatives) {
        if (!ids.has(alternative)) fail(id + ': 替代方案引用了不存在的资产 ' + JSON.stringify(alternative));
      }
    }
  }

  // 4. the aliases may not inflate the number of assets
  if (!Array.isArray(manifest.aliases)) {
    fail(id + ': aliases 必须是数组');
  } else {
    for (const alias of manifest.aliases) {
      if (!nonEmptyString(alias)) {
        fail(id + ': aliases 里存在空字符串');
        continue;
      }
      if (ids.has(alias)) {
        fail(id + ': 别名 ' + JSON.stringify(alias) + ' 与某个资产 id 相同，别名不得虚增资产数量');
      }
      const key = alias.toLowerCase();
      const owner = aliasOwners.get(key);
      if (owner === undefined) {
        aliasOwners.set(key, id);
      } else if (owner !== id) {
        fail('别名 ' + JSON.stringify(alias) + ' 同时指向 ' + owner + ' 与 ' + id + '，一个别名只能指向一个资产');
      }
    }
  }

  // 5. the honest states: the card keeps its own state, the implementation has its own, and the digest of an
  // implementation that is not approved has to be a real one (the file it names has to carry it).
  if (!ALLOWED_STATUS.includes(manifest.status)) {
    fail(id + ': status 只能是 ' + ALLOWED_STATUS.join(' 或 ') + '，实际是 ' + JSON.stringify(manifest.status));
  }
  if (!Array.isArray(manifest.implementations)) {
    fail(id + ': implementations 必须是数组');
  } else {
    for (const implementation of manifest.implementations) {
      if (!isPlainObject(implementation)) {
        fail(id + ': implementations 的元素必须是对象');
        continue;
      }
      for (const field of REQUIRED_IMPLEMENTATION_FIELDS) {
        if (!(field in implementation)) {
          fail(id + ': 实现 ' + JSON.stringify(implementation.implementation_id) + ' 缺少字段 ' + field);
        }
      }
      const state = implementation.status;
      if (!ALLOWED_IMPLEMENTATION_STATUS.includes(state)) {
        fail(id + ': 实现 ' + JSON.stringify(implementation.implementation_id) + ' 的状态 ' + JSON.stringify(state) + ' 不在 ' + ALLOWED_IMPLEMENTATION_STATUS.join(' / ') + ' 之中');
      }
      if (state !== 'APPROVED' && !/^[0-9a-f]{64}$/.test(String(implementation.sha256 || ''))) {
        fail(id + ': 实现 ' + JSON.stringify(implementation.implementation_id) + ' 不是 APPROVED，必须带 64 位 sha256');
        continue;
      }
      // the digest has to be the digest of the file the implementation names: a placeholder is never published
      const source = path.resolve(REPO_ROOT, implementation.path);
      if (!fs.existsSync(source)) {
        fail(id + ': 实现 ' + JSON.stringify(implementation.implementation_id) + ' 指向不存在的文件 ' + implementation.path);
      } else {
        const actual = sha256OfText(fs.readFileSync(source, 'utf8'));
        if (actual !== implementation.sha256) {
          fail(id + ': 实现 ' + JSON.stringify(implementation.implementation_id) + ' 的 sha256 与 ' + implementation.path + ' 的实际摘要不符（文件被改过或摘要是占位符）');
        }
      }
      // the tests that stand behind the implementation have to be named
      const testsManifest = path.resolve(REPO_ROOT, implementation.tests_manifest);
      if (!fs.existsSync(testsManifest)) {
        fail(id + ': 实现 ' + JSON.stringify(implementation.implementation_id) + ' 的测试清单不存在: ' + implementation.tests_manifest);
      } else {
        const tests = readJson(testsManifest);
        if (tests.error) {
          fail(id + ': 测试清单 ' + implementation.tests_manifest + ' ' + tests.error);
        } else if (tests.value.asset_id !== id) {
          fail(id + ': 测试清单 ' + implementation.tests_manifest + ' 属于另一个资产');
        } else if (state === 'TESTED' && (!Array.isArray(tests.value.tests) || tests.value.tests.length === 0)) {
          fail(id + ': 实现声明自己是 TESTED，但测试清单里没有一条测试');
        }
      }
    }
  }

  // 6. the complexity
  const complexity = manifest.complexity;
  if (!isPlainObject(complexity)) {
    fail(id + ': complexity 必须是对象');
  } else if (manifest.kind === 'algorithm') {
    if (!nonEmptyString(complexity.time) || complexity.time === 'n/a') fail(id + ': 算法的 complexity.time 不能是 n/a');
    if (!nonEmptyString(complexity.extra_space) || complexity.extra_space === 'n/a') fail(id + ': 算法的 complexity.extra_space 不能是 n/a');
  }

  if (!isPlainObject(manifest.provenance)) {
    fail(id + ': provenance 必须是对象');
  } else if (!Array.isArray(manifest.provenance.references)) {
    fail(id + ': provenance.references 必须是数组');
  }

  // 7. the card
  if (!fs.existsSync(cardFile)) {
    fail(id + ': 缺少 card.md');
    missing.push(id + '/card.md');
    continue;
  }
  const card = fs.readFileSync(cardFile, 'utf8');
  cardCount += 1;
  if (card.trim().length === 0) {
    fail(id + ': card.md 是空的');
    continue;
  }
  for (const section of CARD_SECTIONS) {
    if (!card.includes(section)) fail(id + ': card.md 缺少小节 ' + section);
  }
  // 01_Requirements.md §14.2 asks every modern engineering pattern for its
  // applicability, counterexamples, concurrency risks, alternatives and the
  // system boundary.
  if (id.startsWith('pattern.') && !id.startsWith('pattern.gof.')) {
    if (!card.includes('系统边界')) {
      fail(id + ': 现代工程模式的 card.md 必须写明系统边界（需求书 §14.2）');
    }
  }

  // 8. every asset id quoted in the card must exist
  const references = card.match(/\b(?:pattern|algorithm)\.[a-z0-9_]+(?:\.[a-z0-9_]+)+/g) || [];
  for (const reference of new Set(references)) {
    if (!ids.has(reference)) fail(id + ': card.md 引用了不存在的资产 ' + reference);
  }
}

if (!problems.length) {
  ok(manifestCount + ' 个 manifest.json 字段齐全，' + cardCount + ' 张 card.md 结构完整');
  ok('别名与替代方案的引用全部指向目录内真实存在的资产');
}

// ---- 9. the summary ------------------------------------------------------------
const namespaces = new Map();
for (const id of ids) {
  const parts = id.split('.');
  let namespace;
  if (parts[0] === 'algorithm') namespace = 'algorithm.' + parts[1];
  else if (parts[1] === 'gof') namespace = parts.slice(0, 3).join('.');
  else namespace = parts.slice(0, 2).join('.');
  namespaces.set(namespace, (namespaces.get(namespace) || 0) + 1);
}

console.log('');
console.log('--- 汇总 ---');
console.log('目录      : ' + catalogDir);
console.log('资产总数  : ' + ids.size + ' （期望 ' + EXPECTED_TOTAL + '）');
console.log('manifest  : ' + manifestCount + ' / ' + EXPECTED_TOTAL);
console.log('知识卡    : ' + cardCount + ' / ' + EXPECTED_TOTAL);
console.log('kind 统计 : pattern ' + groups.gof + ' + ' + groups.modern + ' = ' + (groups.gof + groups.modern) + '，algorithm ' + groups.algorithm);
console.log('namespace :');
for (const key of [...namespaces.keys()].sort()) {
  console.log('  ' + key.padEnd(26, ' ') + namespaces.get(key));
}
console.log('缺失项    : ' + (missing.length ? missing.join(', ') : '无'));
const implementationCount = assets.reduce((sum, entry) => {
  const manifestFile = path.join(catalogDir, 'assets', entry.id, 'manifest.json');
  if (!fs.existsSync(manifestFile)) return sum;
  try {
    const manifest = JSON.parse(fs.readFileSync(manifestFile, 'utf8'));
    return sum + (Array.isArray(manifest.implementations) ? manifest.implementations.length : 0);
  } catch (e) {
    return sum;
  }
}, 0);
console.log('实现登记  : ' + implementationCount + ' 条');

if (problems.length) {
  console.error('');
  console.error('OK: ' + catalogDir + ' - ' + ids.size + ' assets, ' + problems.length + ' problems');
  process.exit(1);
}
console.log('');
console.log('OK: ' + catalogDir + ' - ' + ids.size + ' assets, 0 problems');
