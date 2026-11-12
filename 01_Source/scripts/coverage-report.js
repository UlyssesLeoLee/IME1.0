'use strict';
// End-to-end coverage report of the design cases and the 90% gate.
//
//   node scripts/coverage-report.js           report + gate (exit code 1 when a feature area is below 90%)
//   node scripts/coverage-report.js --check   the same; --check is the default behaviour
//   node scripts/coverage-report.js --regen   re-run scripts/traceability-report.js first (it re-runs the JS
//                                            suite and REWRITES 02_Verification/test-execution-report.*)
//
// The design documents (00_Documents/02_UT_IT_ST) are the authority: every "### <ID> — <title>" heading is one
// design case. The evidence is 02_Verification/test-execution-report.json, written by scripts/traceability-report.js:
// it maps every design case id to the automated tests that carry that id in their title.
//
// Feature area: the design documents put every case of a document under one top-level section ("3. 试验案例详细定义"),
// so the section alone cannot be gated. The feature area of a case is therefore the product module the case belongs
// to (DD-01..DD-10 of 00_Documents/01_Detailed_Design): the module id in the "设计对象" row of the case, or - when
// the row does not name one - the module that owns the first requirement of the "追踪要件" row. A case under a
// top-level section of its own keeps that section as its feature area, so the report stays correct if the design
// documents are grouped by feature area later.
//
// Nothing here writes to 02_Verification unless --regen is given explicitly.
const fs = require('node:fs');
const path = require('node:path');
const {spawnSync} = require('node:child_process');

const root = path.resolve(__dirname, '..', '..');
const docs = path.join(root, '00_Documents', '02_UT_IT_ST');
const detailed = path.join(root, '00_Documents', '01_Detailed_Design', 'Programming_IME_Detailed_Design_v1.0.md');
const verification = path.join(root, '02_Verification');
const reportFile = path.join(verification, 'test-execution-report.json');
const GATE = 90;
const DESIGN = [
  {group: 'UT', file: 'Programming_IME_UT_Test_Design_v1.0.md'},
  {group: 'IT', file: 'Programming_IME_IT_Test_Design_v1.0.md'},
  {group: 'ST', file: 'Programming_IME_ST_Test_Design_v1.0.md'}
];
// Fallback names, used when the detailed design document cannot be read.
const MODULE_FALLBACK = {
  'DD-01': 'TSF/COM 组件详细设计', 'DD-02': '输入状态机与候选提交', 'DD-03': '七语言包与自动识别',
  'DD-04': '权限上下文、VS Code Bridge', 'DD-05': 'BYOK / Provider 能力 / Effort', 'DD-06': '实时预测与资源调度',
  'DD-07': 'F1 双路径与事务', 'DD-08': '屏幕、事件与无障碍', 'DD-09': '规则库及本地物理数据',
  'DD-10': '安装、迁移、诊断与安全'
};

// ---- the design documents -------------------------------------------------------------------------------------
function parseDesign(file, group) {
  const text = fs.readFileSync(path.join(docs, file), 'utf8');
  const lines = text.split(/\r?\n/);
  const cases = [];
  let section = null, current = null;
  for (const line of lines) {
    const h2 = /^##\s+(.+?)\s*$/.exec(line);
    if (h2) { section = h2[1]; current = null; continue; }
    const h3 = /^###\s+((?:UT|IT|ST)-\d{3})\s+—\s+(.*?)\s*$/.exec(line);
    if (h3) {
      current = {id: h3[1], title: h3[2], group, section, object: '', requirements: '', criteria: ''};
      cases.push(current);
      continue;
    }
    if (/^###\s/.test(line)) { current = null; continue; } // another level-3 heading (2.1 共通 Fixture)
    if (!current) continue;
    const row = /^\|\s*([^|]+?)\s*\|\s*(.*?)\s*\|?\s*$/.exec(line);
    if (!row) continue;
    const key = row[1], value = row[2].replace(/\*\*/g, '').trim();
    if (key === '设计对象') current.object = value;
    else if (key === '追踪要件') current.requirements = value;
    else if (key.startsWith('期待结果')) current.criteria = value;
  }
  return cases;
}
const allCases = DESIGN.flatMap(d => parseDesign(d.file, d.group));

// ---- the feature areas (the product modules DD-01..DD-10) ------------------------------------------------------
const moduleNames = {...MODULE_FALLBACK};
const frToModule = new Map();
(function readDetailedDesign() {
  if (!fs.existsSync(detailed)) return;
  const text = fs.readFileSync(detailed, 'utf8');
  for (const m of text.matchAll(/^##\s+\d+\.\s*(DD-\d{2})[：:]\s*(.+?)\s*$/gm)) moduleNames[m[1]] = m[2];
  for (const m of text.matchAll(/^\|\s*(DD-\d{2})\s*\|\s*(FR-[^|]+?)\s*\|/gm)) {
    for (const fr of expandFrRange(m[2])) frToModule.set(fr, m[1]);
  }
})();
function expandFrRange(spec) {
  const out = [];
  for (const part of String(spec).split(/[,、]/)) {
    const range = /FR-(\d{3})\s*[~～\-–—]\s*FR-(\d{3})/.exec(part);
    if (range) { for (let n = +range[1]; n <= +range[2]; n++) out.push(`FR-${String(n).padStart(3, '0')}`); continue; }
    const one = /FR-\d{3}/.exec(part);
    if (one) out.push(one[0]);
  }
  return out;
}
function featureArea(c, sharedSection) {
  // A case under a top-level section of its own keeps that section (the design documents may be grouped later).
  if (c.section && c.section !== sharedSection) return {key: `section:${c.section}`, label: c.section};
  const module = /DD-\d{2}/.exec(c.object);
  if (module) return {key: module[0], label: `${module[0]} ${moduleNames[module[0]] || ''}`.trim()};
  for (const fr of c.requirements.match(/FR-\d{3}/g) || []) {
    const owner = frToModule.get(fr);
    if (owner) return {key: owner, label: `${owner} ${moduleNames[owner] || ''}`.trim()};
  }
  // The detailed design says the cross-cutting safety and non-functional requirements are owned by several DD
  // components at once ("横断安全与非功能要求由多个 DD 组件协同负责"), so they get one area of their own.
  if (/NFR-\d{3}/.test(c.requirements)) return {key: 'NFR', label: 'NFR 非功能要求（跨 DD 组件协同）'};
  return {key: 'unclassified', label: '未归类（设计书未给出模块归属）'};
}
for (const group of DESIGN) {
  const own = allCases.filter(c => c.group === group.group);
  const shared = own.length ? own[0].section : null;
  for (const c of own) c.area = featureArea(c, shared);
}

// ---- the evidence ---------------------------------------------------------------------------------------------
function loadEvidence(regenerate) {
  if (regenerate || !fs.existsSync(reportFile)) {
    console.log(`coverage-report: the evidence file is missing${regenerate ? ' (--regen)' : ''}, running scripts/traceability-report.js ...`);
    const run = spawnSync(process.execPath, [path.join(__dirname, 'traceability-report.js')], {cwd: root, encoding: 'utf8', stdio: 'inherit'});
    if (run.status !== 0) {
      console.error(`coverage-report FAILED: scripts/traceability-report.js exited with ${run.status}`);
      process.exit(2);
    }
  }
  const data = JSON.parse(fs.readFileSync(reportFile, 'utf8'));
  const rows = new Map((data.rows || []).map(r => [r.id, r]));
  return {data, rows, generated: (data.summary && data.summary.generated) || 'unknown'};
}
const args = process.argv.slice(2);
const evidence = loadEvidence(args.includes('--regen'));
const rows = evidence.rows;

// ---- the numbers ----------------------------------------------------------------------------------------------
const cases = allCases.map(c => {
  const row = rows.get(c.id);
  const status = row ? row.status : 'NOT RUN';
  return {...c, status, reason: (row && row.reason) || ''};
});
const tally = list => ({
  cases: list.length,
  covered: list.filter(c => c.status === 'PASS').length,
  failed: list.filter(c => c.status === 'FAIL').length,
  notRun: list.filter(c => c.status !== 'PASS' && c.status !== 'FAIL').length
});
const pct = t => (t.cases ? (100 * t.covered) / t.cases : 100);
const areas = [];
for (const key of [...new Set(cases.map(c => c.area.key))]) {
  const list = cases.filter(c => c.area.key === key);
  areas.push({key, label: list[0].area.label, ...tally(list), members: list});
}
const groups = DESIGN.map(d => ({key: d.group, ...tally(cases.filter(c => c.group === d.group))}));
const overall = tally(cases);
const sections = [...new Set(cases.map(c => c.section))].map(s => ({key: s, ...tally(cases.filter(c => c.section === s))}));

// ---- the report -----------------------------------------------------------------------------------------------
const width = s => [...String(s)].reduce((n, ch) => n + (/[\u1100-\u115F\u2E80-\uA4CF\uAC00-\uD7A3\uF900-\uFAFF\uFE30-\uFE4F\uFF00-\uFF60\uFFE0-\uFFE6]/.test(ch) ? 2 : 1), 0);
const pad = (s, n) => String(s) + ' '.repeat(Math.max(0, n - width(s)));
const cell = (s, n) => pad(s, n);
function table(header, rowsOut) {
  const widths = header.map((h, i) => Math.max(width(h), ...rowsOut.map(r => width(r[i] ?? ''))));
  const line = cells => '| ' + cells.map((c, i) => cell(c, widths[i])).join(' | ') + ' |';
  const sep = '|' + widths.map(w => '-'.repeat(w + 2)).join('|') + '|';
  console.log(line(header));
  console.log(sep);
  for (const r of rowsOut) console.log(line(r));
}
console.log(`\nProgramming IME — design case coverage (gate: every feature area >= ${GATE}%)`);
console.log(`  design documents : 00_Documents/02_UT_IT_ST (${cases.length} cases)`);
console.log(`  evidence         : 02_Verification/test-execution-report.json (generated ${evidence.generated})`);
console.log(`  feature areas    : the product modules DD-01..DD-10 of the detailed design (the design documents put all`);
console.log(`                     cases of a document under one top-level section, so the section alone cannot be gated)\n`);

console.log('Feature areas');
table(['Feature area', 'Cases', 'Covered', 'Failed', 'Not run', 'Coverage'],
  areas.map(a => [a.label, a.cases, a.covered, a.failed, a.notRun, `${pct(a).toFixed(1)}%`]));
console.log('');
console.log('Design case groups');
table(['Group', 'Cases', 'Covered', 'Failed', 'Not run', 'Coverage'],
  groups.map(g => [g.key, g.cases, g.covered, g.failed, g.notRun, `${pct(g).toFixed(1)}%`]));
console.log('');
console.log('Design document sections (the top-level section of each case)');
table(['Section', 'Cases', 'Covered', 'Failed', 'Not run', 'Coverage'],
  sections.map(s => [s.key, s.cases, s.covered, s.failed, s.notRun, `${pct(s).toFixed(1)}%`]));
console.log('');
console.log(`Overall: ${overall.covered}/${overall.cases} covered, ${overall.failed} failed, ${overall.notRun} not run = ${pct(overall).toFixed(1)}%`);

const missing = cases.filter(c => c.status !== 'PASS');
if (missing.length) {
  console.log(`\nCases without passing automated evidence (${missing.length})`);
  table(['ID', 'Title', 'Feature area', 'Status', 'Reason'],
    missing.map(c => [c.id, c.title, c.area.label, c.status, c.reason || 'no automated test carries this id']));
}

const below = areas.filter(a => pct(a) < GATE);
console.log('');
if (!below.length) {
  console.log(`Gate: PASS — every feature area is at or above ${GATE}%.`);
  process.exit(0);
}
console.log(`Gate: FAIL — ${below.length} feature area(s) below ${GATE}%:`);
for (const a of below) {
  const missingHere = a.cases - a.covered;
  const needed = Math.ceil((GATE / 100) * a.cases) - a.covered;
  console.log(`  - ${a.label}: ${a.covered}/${a.cases} = ${pct(a).toFixed(1)}%` +
    ` (${(GATE - pct(a)).toFixed(1)} points below the bar; ${missingHere} case(s) without evidence, ${needed} more needed)`);
}
process.exit(1);
