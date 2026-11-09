'use strict';
// The cost and the latency benchmark of the product: it runs the example of `tase-metrics`, writes the evidence
// into 02_Verification/cost-benchmark.json and checks the budget of the design book.
//
//   node scripts/bench-cost.js [price table path] [iterations]
//
// The price table is the file the operator supplies; the default is the SAMPLE table of the repository (the numbers
// of a sample table are not the price of any provider - put your own table there and the numbers are yours).
//
// The exit code is 0 when the local candidate path stays inside its budget (DD-IME §20.2: p95 <= 50 ms) and 1
// when it does not.
const {spawnSync} = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const SOURCE_ROOT = path.resolve(__dirname, '..');
const REPO_ROOT = path.resolve(SOURCE_ROOT, '..');
const CRATE = path.join(SOURCE_ROOT, 'crates', 'tase-metrics');
const EVIDENCE = path.join(REPO_ROOT, '02_Verification', 'cost-benchmark.json');

const tableArg = process.argv[2];
const iterations = process.argv[3] || '200';
const table = tableArg
  ? path.resolve(REPO_ROOT, tableArg)
  : path.join(SOURCE_ROOT, 'assets', 'pricing', 'sample-price-table.json');

if (!fs.existsSync(table)) {
  console.error('FAIL: the price table is not there: ' + table);
  process.exit(2);
}

const targetDir = process.env.CARGO_TARGET_DIR || path.join(SOURCE_ROOT, 'target');
const r = spawnSync(
  'cargo',
  ['run', '--release', '--example', 'cost_bench', '--', table, iterations],
  {
    cwd: CRATE,
    encoding: 'utf8',
    env: {...process.env, CARGO_TARGET_DIR: targetDir},
    maxBuffer: 16 * 1024 * 1024,
  }
);
if (r.error) {
  console.error('FAIL: cargo could not be run: ' + r.error.message);
  process.exit(2);
}
if (r.status !== 0) {
  console.error((r.stderr || '') + (r.stdout || ''));
  console.error('FAIL: the benchmark did not run (exit ' + r.status + ')');
  process.exit(2);
}

// The example prints one JSON object on its standard output; the cargo chatter goes to the standard error.
const line = (r.stdout || '')
  .split(/\r?\n/)
  .map(text => text.trim())
  .filter(text => text.startsWith('{'))
  .pop();
if (!line) {
  console.error('FAIL: the benchmark printed no report');
  console.error(r.stdout || '');
  process.exit(2);
}
let report;
try {
  report = JSON.parse(line);
} catch (e) {
  console.error('FAIL: the report of the benchmark is not valid JSON: ' + e.message);
  process.exit(2);
}

const latency = report.latency || {};
const cost = report.cost || {};
const evidence = {
  schema_version: 'tase.cost-benchmark.v1',
  measured_at: new Date().toISOString(),
  machine: process.env.COMPUTERNAME || 'unknown',
  budget: {
    rule: 'the local candidate path stays inside its budget (DD-IME 20.2)',
    p95_us: latency.budget_us,
    source: 'design §20.2 (p95 <= 50 ms for the local cache)',
  },
  latency: latency,
  cost: cost,
};
fs.mkdirSync(path.dirname(EVIDENCE), {recursive: true});
fs.writeFileSync(EVIDENCE, JSON.stringify(evidence, null, 2) + '\n', 'utf8');

const ok = latency.within_budget === true;
console.log('the local candidate path: p50 ' + latency.p50_us + ' us, p95 ' + latency.p95_us + ' us, max ' + latency.max_us + ' us (budget ' + latency.budget_us + ' us)');
console.log('the samples: ' + latency.samples + ' over the everyday prefixes, ' + latency.candidates_seen + ' candidates');
console.log('one model call: ' + cost.input_tokens + ' input tokens + ' + cost.output_tokens + ' output tokens');
for (const model of cost.models || []) {
  console.log('  the price with ' + model.provider + '/' + model.model + ': ' + model.cost_usd + ' USD');
}
console.log('the evidence: ' + path.relative(REPO_ROOT, EVIDENCE));
if (!ok) {
  console.error('FAIL: the local candidate path is outside its budget (p95 ' + latency.p95_us + ' us)');
  process.exit(1);
}
console.log('OK: the local candidate path is inside its budget');
