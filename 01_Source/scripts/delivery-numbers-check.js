'use strict';
// The numbers of the delivery must agree with each other AND with the artefacts.
//
// The repository already has this discipline for the claims of the framework matrix
// (tests/smoke/installer-claims-check.js, tests/smoke/evidence-cross-check.js). This script covers the OTHER
// numbers the documents quote: the coverage of the design cases, the size of the manifest, the number of the
// defects on the ledger and the test counts of the acceptance table of the refactor plan. It also makes sure that
// the gate really ran the installer crate in the round whose evidence is persisted - there was a round in which the
// gate was green without that crate in it, and the installer evidence in the repository was weeks older than the code
// under test. The numbers still matched, so nothing said so; now they cannot.
//
//   node scripts/delivery-numbers-check.js
//
// Exit code 0 when every number agrees, 1 when one does not.
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..', '..');
const read = relative => fs.readFileSync(path.join(root, relative), 'utf8');

let problems = 0;
const check = (ok, what, detail) => {
  console.log(`${ok ? 'ok  ' : 'FAIL'}  ${what}${detail ? ' -> ' + detail : ''}`);
  if (!ok) problems++;
};
const numberIn = (text, re, what, expected) => {
  const m = re.exec(text);
  if (!m) { check(false, what, 'the number is not quoted'); return; }
  check(Number(m[1]) === expected, what, `${m[1]} vs ${expected}`);
};

// ---- the artefacts ---------------------------------------------------------------------------------
const trace = JSON.parse(read('02_Verification/test-execution-report.json'));
const cases = trace.summary.cases;
const covered = cases.UT.pass + cases.IT.pass + cases.ST.pass;
const total = cases.UT.total + cases.IT.total + cases.ST.total;
const coverage = Math.round((covered / total) * 1000) / 10;

const manifestEntries = read('MANIFEST_SHA256.txt').split(/\r?\n/).filter(line => line.trim()).length;

const log = read('02_Verification/run-all-2026-11-04.log');
const logCounts = {
  rust: Number(/Rust: cargo test --workspace.*?(\d+) tests/.exec(log)?.[1] ?? 0),
  tsf: Number(/TSF crate .*?(\d+) tests/.exec(log)?.[1] ?? 0),
  tase: Number(/TASE crates .*?(\d+) tests/.exec(log)?.[1] ?? 0),
  agent: Number(/AI agent front-ends .*?(\d+) tests/.exec(log)?.[1] ?? 0),
};

// ---- the documents ---------------------------------------------------------------------------------
const readme = read('README_交付说明.md');
const gaps = read('02_Verification/FRAMEWORK_GAPS.md');
const ledger = read('02_Verification/DEFECT_SUBMISSION_2026-11-07.md');
const plan = read('01_Source/docs/REFACTOR_PLAN.md');

// 1. the coverage of the design cases. The documents quote the baseline of the round as well, so the check is
//    "the CURRENT numbers are in there", not "the first number in the file is the current one".
for (const [name, text] of [['the delivery note', readme], ['the gap ledger', gaps]]) {
  check(new RegExp(`${covered}\\s*/\\s*${total}`).test(text), `${name}: it quotes the covered cases`,
    `${covered}/${total}`);
  check(new RegExp(`${coverage}\\s*%`).test(text), `${name}: it quotes the coverage in per cent`, `${coverage} %`);
}

// 2. the size of the manifest
numberIn(readme, /清单\s*\*\*(\d+)\s*条/, 'the delivery note: the manifest entries', manifestEntries);
numberIn(read('02_Verification/IME_FRAMEWORK_MATRIX.md'), /(\d+)\s*\/\s*\d+\s*对得上/, 'the framework matrix: the manifest entries', manifestEntries);

// 3. the defect ledger counts its own rows
const s2 = ledger.split(/\r?\n/).findIndex(l => l.startsWith('## 2.'));
const s3 = ledger.split(/\r?\n/).findIndex(l => l.startsWith('## 3.'));
const rows = ledger.split(/\r?\n/).slice(s2, s3).filter(l => /^\|\s*\*\*D-\d+/.test(l));
const ids = new Set();
for (const row of rows) for (const m of row.matchAll(/\bD-(\d{2,3})\b/g)) ids.add(Number(m[1]));
const header = /待你裁定的缺陷与取舍\s*\|\s*(\d+)\s*条（D-(\d+)\s*~\s*D-(\d+)/.exec(ledger);
check(!!header, 'the ledger: it has a header row', header ? '' : 'not found');
if (header) {
  // the header counts the distinct defect NUMBERS; a row may carry several of them
  check(Number(header[1]) === ids.size, 'the ledger: the header counts its defect numbers', `${header[1]} vs ${ids.size}`);
  check(Number(header[3]) === Math.max(...ids), 'the ledger: the header ends at the highest id', `D-${header[3]} vs D-${Math.max(...ids)}`);
}

// 4. the acceptance table of the refactor plan names the test counts of this round
check(/TSF (\d+) 项/.exec(plan)?.[1] === String(logCounts.tsf), 'the refactor plan: the TSF count is the one of the last gate run',
  `plan says ${/TSF (\d+) 项/.exec(plan)?.[1]}, the log says ${logCounts.tsf}`);
check(/Rust 核心 (\d+) 项/.exec(plan)?.[1] === String(logCounts.rust), 'the refactor plan: the core count is the one of the last gate run',
  `plan says ${/Rust 核心 (\d+) 项/.exec(plan)?.[1]}, the log says ${logCounts.rust}`);

// 5. the gate must have run the installer crate in THIS round, and the installer evidence that is persisted in the
//    repository must be the evidence of that round. There was a round in which the gate was green without the
//    installer crate in it, and the persisted installer evidence was weeks older than the code under test - the
//    the numbers still matched, so nothing said so. The log's summary says how many installer tests the round ran,
//    and the evidence file says how many it recorded: the two have to be the same sum, and the evidence has to carry
//    the three targets of the crate (the unit tests, the real file system tests, the real link tests).
//    Only the summary line counts: the step prints its own output above it, and that output is not a verdict.
const setupEvidence = read('02_Verification/setup-test-result.txt');
// the doc-tests of the crate print a "0 passed" line of their own; the three targets are the ones that ran tests
const setupCounts = [...setupEvidence.matchAll(/test result: ok\. (\d+) passed/g)].map(m => Number(m[1])).filter(n => n > 0);
const setupSum = setupCounts.reduce((a, b) => a + b, 0);
const logInstaller = Number(/^(?:PASS|FAIL)\s+Installer \(crates\/pime-setup\): cargo test.*?(\d+) tests/m.exec(log)?.[1] ?? 0);
check(setupCounts.length === 3, 'the installer evidence carries the three targets of the installer crate',
  setupCounts.length ? setupCounts.join(' + ') : 'the evidence has no test result at all');
check(logInstaller === setupSum && logInstaller > 0,
  'the gate ran the installer crate in this round: the log and the persisted evidence agree',
  `the log says ${logInstaller}, the evidence says ${setupSum}`);

console.log(`\n${problems} problem(s)`);
process.exit(problems ? 1 : 0);
