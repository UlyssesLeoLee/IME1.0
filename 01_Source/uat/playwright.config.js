'use strict';
const path = require('node:path');
const evidence = process.env.UAT_EVIDENCE_DIR || path.resolve(__dirname, '..', '..', '02_Verification', 'uat');
module.exports = {
  testDir: path.join(__dirname, 'tests'),
  outputDir: path.join(evidence, 'artifacts'),
  timeout: 120000,
  expect: {timeout: 10000},
  workers: 1,               // one real VS Code at a time
  fullyParallel: false,
  retries: 0,
  reporter: [['list'], ['json', {outputFile: path.join(evidence, 'uat-results.json')}]]
};
