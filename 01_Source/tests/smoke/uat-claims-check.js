'use strict';
// Binds the UAT numbers the documents quote to the machine product they claim to describe.
//
//   node tests/smoke/uat-claims-check.js
//
// 02_Verification/IME_FRAMEWORK_MATRIX.md carries a machine-readable UAT claims block. This script compares it with
//   - 02_Verification/uat/uat-results.json (the Playwright JSON report of the real run on the real VS Code), and
//   - the prose of the matrix (§5.4) and of the gap ledger (02_Verification/FRAMEWORK_GAPS.md §5.7).
//
// The rule that matters most: a results file in which NOTHING ran is a failure, not a pass. An empty run (every test
// skipped, no result recorded) used to sit in the repository while the documents said "51 passed"; the numbers then
// disagreed with the artifact and nobody noticed, because nothing compared them. The negative control at the bottom
// of this file proves that this script rejects such a file instead of accepting it.
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..', '..', '..'); // the repository root
const matrixPath = path.join(root, '02_Verification', 'IME_FRAMEWORK_MATRIX.md');
const ledgerPath = path.join(root, '02_Verification', 'FRAMEWORK_GAPS.md');

let failed = 0;
const check = (ok, text) => { console.log(`${ok ? 'ok  ' : 'FAIL'} ${text}`); if (!ok) failed++; };
const note = text => console.log(`note ${text}`);

// ---- counting the tests of a Playwright JSON report -----------------------------------------------------------------------------------
// The count comes from the suites, not from the "stats" object: the suites are what the reporter wrote per test, and
// the stats are a summary of them. Both are compared, so a hand-edited summary cannot hide an empty run.
function countTests(report) {
  const counts = {passed: 0, skipped: 0, failed: 0, flaky: 0, total: 0, skippedTitles: []};
  for (const suite of report.suites || []) {
    for (const spec of suite.specs || []) {
      for (const test of spec.tests || []) {
        counts.total++;
        switch (test.status) {
          case 'expected': counts.passed++; break;
          case 'skipped':
            counts.skipped++;
            counts.skippedTitles.push(spec.title || test.title || '');
            break;
          case 'unexpected': counts.failed++; break;
          case 'flaky': counts.flaky++; break;
          default: break; // 'interrupted' and the like: neither passed nor failed, but counted in total
        }
      }
    }
  }
  return counts;
}

// ---- the claims block ----------------------------------------------------------------------------------------------------------------
if (!fs.existsSync(matrixPath)) {
  console.log(`FAIL the framework matrix is missing: ${matrixPath}`);
  process.exit(1);
}
const matrix = fs.readFileSync(matrixPath, 'utf8');
const block = matrix.match(/<!--\s*uat-claims:begin\s*-->([\s\S]*?)<!--\s*uat-claims:end\s*-->/);
if (!block) {
  console.log(`FAIL ${matrixPath} has no machine-readable UAT claims block (<!-- uat-claims:begin --> ... end)`);
  process.exit(1);
}
const claims = {};
for (const raw of block[1].split(/\r?\n/)) {
  const line = raw.trim();
  if (!line || line.startsWith('#')) continue;
  const eq = line.indexOf('=');
  if (eq < 0) continue;
  claims[line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
}
const claim = key => {
  if (!(key in claims)) { check(false, `the UAT claims block has no "${key}"`); return null; }
  return claims[key];
};
note(`UAT claims read from ${path.relative(root, matrixPath)}: ${Object.keys(claims).length} entries`);

// ---- the machine product ------------------------------------------------------------------------------------------------------------
const resultsName = claim('uat_results_file');
const resultsPath = resultsName ? path.join(root, '02_Verification', resultsName) : null;
if (!resultsPath || !fs.existsSync(resultsPath)) {
  check(false, `the UAT results file is missing: ${resultsPath || '?'}`);
} else {
  let report = null;
  try {
    report = JSON.parse(fs.readFileSync(resultsPath, 'utf8'));
  } catch (e) {
    check(false, `the UAT results file does not parse: ${e.message}`);
  }
  if (report) {
    const counts = countTests(report);
    note(`${path.relative(root, resultsPath)}: ${counts.total} tests, ${counts.passed} passed, ${counts.skipped} skipped, ${counts.failed} failed, ${counts.flaky} flaky`);

    // The check that the previous evidence file would have failed: a report in which nothing ran is not a pass.
    check(counts.passed > 0, `the UAT evidence records ${counts.passed} executed and passed tests (a report without a single executed test is not a pass)`);

    const pairs = [
      ['uat_scenarios', counts.total],
      ['uat_passed', counts.passed],
      ['uat_skipped', counts.skipped],
      ['uat_failed', counts.failed],
      ['uat_flaky', counts.flaky],
    ];
    for (const [key, actual] of pairs) {
      const claimed = claim(key);
      check(claimed !== null && Number(claimed) === actual, `${key}: the results say ${actual}, the document says ${claimed === null ? 'nothing' : claimed}`);
    }

    // the report's own summary must agree with the counted tests
    const stats = report.stats || {};
    if (stats && typeof stats.expected === 'number') {
      check(stats.expected === counts.passed && (stats.skipped || 0) === counts.skipped && (stats.unexpected || 0) === counts.failed,
        `the report's own summary agrees with its tests (${stats.expected}/${stats.skipped}/${stats.unexpected})`);
    } else {
      check(false, 'the UAT results file has no usable "stats" object');
    }

    // the one skipped scenario must be the one the documents name
    const skippedCase = claim('uat_skipped_case');
    if (skippedCase) {
      const named = counts.skippedTitles.filter(t => t.includes(skippedCase));
      check(counts.skipped === 1 && named.length === 1,
        `the only skipped scenario is ${skippedCase} (the report says ${counts.skippedTitles.map(t => t.split(' ')[0]).join(', ') || 'none'})`);
    }

    // the evidence must be fresh enough to describe this round: the report carries its own start time
    const started = report.stats && report.stats.startTime ? Date.parse(report.stats.startTime) : NaN;
    check(Number.isFinite(started), `the UAT report carries its start time (${report.stats && report.stats.startTime})`);
  }
}

// ---- the prose the documents quote ---------------------------------------------------------------------------------------------------
// Both documents write the same sentence; it is the number a reader will believe, so it is compared to the artifact.
const prose = /(\d+) 个场景[：:]\s*(\d+) 通过\s*\/\s*(\d+) 按设计跳过\s*\/\s*(\d+) 失败/;
for (const [label, file] of [['the matrix §5.4', matrixPath], ['the gap ledger §5.7', ledgerPath]]) {
  if (!fs.existsSync(file)) { check(false, `${label} is missing: ${file}`); continue; }
  const text = fs.readFileSync(file, 'utf8');
  const m = text.match(prose);
  if (!m) { check(false, `${label} does not quote the UAT result sentence ("N 个场景：P 通过 / S 按设计跳过 / F 失败")`); continue; }
  const quoted = {scenarios: Number(m[1]), passed: Number(m[2]), skipped: Number(m[3]), failed: Number(m[4])};
  const claimed = {
    scenarios: Number(claim('uat_scenarios')),
    passed: Number(claim('uat_passed')),
    skipped: Number(claim('uat_skipped')),
    failed: Number(claim('uat_failed')),
  };
  check(quoted.scenarios === claimed.scenarios && quoted.passed === claimed.passed &&
    quoted.skipped === claimed.skipped && quoted.failed === claimed.failed,
    `${label} quotes ${quoted.scenarios}/${quoted.passed}/${quoted.skipped}/${quoted.failed}, the claims block says ${claimed.scenarios}/${claimed.passed}/${claimed.skipped}/${claimed.failed}`);
}

// ---- negative control: this script must reject what it is meant to reject ---------------------------------------------------------
// A synthetic report in which nothing ran must fail the "a report without a single executed test" rule. If this
// control ever passes silently, the rule above has stopped working and the whole check is decoration.
const emptyRun = {suites: [{specs: [{title: 'UAT-01 [x]', tests: [{status: 'skipped'}]}]}], stats: {expected: 0, skipped: 1, unexpected: 0}};
const emptyCounts = countTests(emptyRun);
check(emptyCounts.passed === 0 && emptyCounts.total === 1 && emptyCounts.skipped === 1,
  'negative control: a report in which nothing ran counts as 0 passed (and is therefore rejected above)');
const wrongNumber = {suites: [{specs: [{title: 'x', tests: [{status: 'expected'}, {status: 'unexpected'}]}]}], stats: {expected: 1, skipped: 0, unexpected: 1}};
const wrongCounts = countTests(wrongNumber);
check(wrongCounts.passed === 1 && wrongCounts.failed === 1,
  'negative control: a report with a failing test counts as 1 failed (and does not agree with the claims above)');

console.log(failed
  ? `uat-claims-check: ${failed} check(s) FAILED - the documents and the UAT evidence disagree`
  : 'uat-claims-check: all checks passed (the documents match the UAT evidence)');
process.exit(failed ? 1 : 0);
