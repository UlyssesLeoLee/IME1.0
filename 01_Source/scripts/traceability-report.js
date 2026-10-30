'use strict';
// Builds 02_Verification/test-execution-report.{md,json}: for every UT/IT/ST case id of the design documents, which
// automated evidence exists (JS unit tests, Rust tests, Playwright UAT) and its real result. An id is "covered" when
// a test whose title carries that id passed; this is a mapping by title, NOT a formal sign-off of every design step.
// Usage: node scripts/traceability-report.js   (run the three test suites first; the script re-runs the JS tests)
const fs = require('node:fs');
const path = require('node:path');
const {spawnSync} = require('node:child_process');

const root = path.resolve(__dirname, '..', '..');
const docs = path.join(root, '00_Documents', '02_UT_IT_ST');
const verification = path.join(root, '02_Verification');
const ID = /\b(?:UT|IT|ST|AT)-\d{3}\b/g;

function cases(file, tag) {
  const text = fs.readFileSync(path.join(docs, file), 'utf8');
  return [...text.matchAll(new RegExp(`^### (${tag}-\\d{3}) — (.+)$`, 'gm'))].map(m => ({id: m[1], title: m[2].trim()}));
}
const all = [
  ...cases('Programming_IME_UT_Test_Design_v1.0.md', 'UT'), ...cases('Programming_IME_IT_Test_Design_v1.0.md', 'IT'), ...cases('Programming_IME_ST_Test_Design_v1.0.md', 'ST')
];
const evidence = new Map(all.map(c => [c.id, {js: [], rust: [], uat: [], failed: []}]));
// Tests for requirements added after the design baseline (ANN-xx annotations, API-xx provider compatibility): no design-case ids exist for them.
const EXTRA = /\b(?:ANN|API)-\d{2}[a-z]?\b/g;
const extras = [];
const noteExtra = (title, kind, ok) => { for (const id of title.match(EXTRA) || []) extras.push({id, kind, title: title.replace(/\s*\[.*$/, ''), ok}); };
const note = (id, kind, title, ok) => { const e = evidence.get(id); if (!e) return; (ok ? e[kind] : e.failed).push(`${kind}: ${title}`); if (ok && !e[kind].includes(title)) e[kind].push(title); };

// ---- JS (node:test, TAP)
const js = spawnSync(process.execPath, ['--test', '--test-reporter=tap', '*.test.js'], {cwd: path.join(root, '01_Source', 'tests', 'js'), encoding: 'utf8'});
let jsTotal = 0, jsPass = 0;
for (const m of js.stdout.matchAll(/^(not ok|ok) \d+ - (.+)$/gm)) {
  if (/ # (SKIP|TODO)\b/i.test(m[2])) continue; // a skipped test is not evidence (the engine tests skip without PIME_CORE_EXE)
  jsTotal++; const ok = m[1] === 'ok'; if (ok) jsPass++;
  for (const id of m[2].match(ID) || []) note(id, 'js', m[2], ok);
  noteExtra(m[2], 'JS', ok);
}
// ---- Rust (raw cargo test output saved by the Rust run)
let rustTotal = 0, rustPass = 0;
const rustFile = path.join(verification, 'rust-test-result.txt');
if (fs.existsSync(rustFile)) {
  // only section 1 (the default `cargo test --workspace` run); later sections of the file repeat tests (release, clippy)
  const firstRun = fs.readFileSync(rustFile, 'utf8').split(/^={10,} 2\./m)[0];
  for (const m of firstRun.matchAll(/^test (\S+) \.\.\. (ok|FAILED)$/gm)) {
    rustTotal++; const ok = m[2] === 'ok'; if (ok) rustPass++;
    const u = /ut_(\d{3})/.exec(m[1]);
    if (u) note(`UT-${u[1]}`, 'rust', m[1], ok);
  }
}
// ---- Playwright UAT
let uatTotal = 0, uatPass = 0;
const uatFile = path.join(verification, 'uat', 'uat-results.json');
if (fs.existsSync(uatFile)) {
  const walk = suite => { for (const s of suite.specs || []) { if ((s.tests || []).every(t => t.status === 'skipped')) continue; /* a skipped spec is not evidence (Playwright reports it as ok) */ uatTotal++; const ok = s.ok === true; if (ok) uatPass++; for (const id of s.title.match(ID) || []) note(id, 'uat', s.title.replace(/\s*\[.*$/, ''), ok); noteExtra(s.title, 'UAT', ok); } (suite.suites || []).forEach(walk); };
  (JSON.parse(fs.readFileSync(uatFile, 'utf8')).suites || []).forEach(walk);
}

// Reasons why a case has no automated evidence here (kept short and factual).
const reason = id => {
  const n = Number(id.slice(3));
  if (id.startsWith('UT') && [60].includes(n)) return 'P1 plugin host not implemented';
  if (id.startsWith('IT') && [1, 2, 3, 4, 5, 37, 42, 48].includes(n)) return 'needs the Windows TSF DLL, a Windows 11 VM or named-pipe IPC (not built)';
  if (id.startsWith('IT') && [16, 38, 39, 41, 44].includes(n)) return 'needs Windows Credential Manager worker, plugin host, installer or multi-process log merge (not built)';
  if (id.startsWith('ST') && [1, 2, 3, 4, 5, 35, 37].includes(n)) return 'needs a Windows 11 VM with the TSF DLL / signed installer (not built)';
  if (id.startsWith('ST') && [38, 39, 40, 32].includes(n)) return 'long-run / benchmark-machine / multi-monitor DPI measurement not run';
  if (id.startsWith('ST') && n === 34) return 'P1 plugin isolation not implemented';
  if (id.startsWith('UT') && [1, 2, 3, 4, 5, 9, 55, 61, 64].includes(n)) return 'pure logic only; no TSF host (Rust tests where listed)';
  return 'no automated test mapped to this id in this delivery';
};
const rows = all.map(c => {
  const e = evidence.get(c.id);
  const pass = ['js', 'rust', 'uat'].filter(k => e[k].length);
  const status = e.failed.length ? 'FAIL' : pass.length ? 'PASS' : 'NOT RUN';
  return {id: c.id, title: c.title, status, via: pass, tests: [...e.js, ...e.rust, ...e.uat], failed: e.failed, reason: status === 'NOT RUN' ? reason(c.id) : ''};
});
const count = (s, p) => rows.filter(r => r.id.startsWith(p) && r.status === s).length;
const summary = {generated: new Date().toISOString(), node: process.version, js: {total: jsTotal, pass: jsPass}, rust: {total: rustTotal, pass: rustPass}, uat: {total: uatTotal, pass: uatPass},
  cases: Object.fromEntries(['UT', 'IT', 'ST'].map(p => [p, {total: rows.filter(r => r.id.startsWith(p)).length, pass: count('PASS', p), fail: count('FAIL', p), notRun: count('NOT RUN', p)}]))};
fs.writeFileSync(path.join(verification, 'test-execution-report.json'), JSON.stringify({summary, rows}, null, 1) + '\n');

const md = [];
md.push('# Test execution report (generated by `01_Source/scripts/traceability-report.js`)', '');
md.push(`Generated ${summary.generated}. Evidence is mapped **by test title id**: PASS means at least one automated test carrying that id passed and none failed. It is not a formal sign-off that every step of the design case was executed.`, '');
md.push('| Suite | Executed | Passed |', '|---|---|---|', `| JavaScript unit/integration (node:test) | ${jsTotal} | ${jsPass} |`, `| Rust core (cargo test) | ${rustTotal} | ${rustPass} |`, `| Playwright UAT on real VS Code | ${uatTotal} | ${uatPass} |`, '');
md.push('| Design case group | Cases | With passing automated evidence | Failed | Not run |', '|---|---|---|---|---|');
for (const p of ['UT', 'IT', 'ST']) { const c = summary.cases[p]; md.push(`| ${p} | ${c.total} | ${c.pass} | ${c.fail} | ${c.notRun} |`); }
md.push('', 'Not run does NOT mean passed. Windows TSF, installer, named-pipe IPC, Credential Manager, DPI/multi-monitor, 30-minute soak and plugin cases need environments that were not available.', '');
md.push('| ID | Title | Status | Evidence / reason |', '|---|---|---|---|');
for (const r of rows) md.push(`| ${r.id} | ${r.title.replace(/\|/g, '/')} | ${r.status} | ${r.status === 'NOT RUN' ? r.reason : [...new Set(r.via)].join(' + ') + (r.failed.length ? ' ; FAILED: ' + r.failed.join('; ') : '')} |`);
md.push('', '## Requirements added after the design baseline (no UT/IT/ST ids; see docs/ANNOTATIONS.md and docs/VERSIONS.md)', '', '| ID | Suite | Test | Result |', '|---|---|---|---|');
for (const x of extras.sort((a, b) => a.id.localeCompare(b.id))) md.push(`| ${x.id} | ${x.kind} | ${x.title.replace(/\|/g, '/')} | ${x.ok ? 'PASS' : 'FAIL'} |`);
summary.extras = {total: extras.length, pass: extras.filter(x => x.ok).length};
fs.writeFileSync(path.join(verification, 'test-execution-report.json'), JSON.stringify({summary, rows, extras}, null, 1) + '\n');
fs.writeFileSync(path.join(verification, 'test-execution-report.md'), md.join('\n') + '\n');
console.log(JSON.stringify(summary, null, 1));
