'use strict';
const path = require('node:path');
const evidence = process.env.UAT_EVIDENCE_DIR || path.resolve(__dirname, '..', '..', '..', '02_Verification', 'uat');
module.exports = {
  testDir: path.join(__dirname, 'tests'),
  outputDir: path.join(evidence, 'artifacts'),
  timeout: 120000,
  expect: {timeout: 10000},
  // Without an action timeout a click that is swallowed (a floating widget above the target, an element that never
  // becomes clickable) is retried until the whole spec times out after 120 s, and the report then says nothing
  // about what was in the way. Fail fast instead: the helper gets the overlay out of the way and the error names
  // the element that blocked the click. 15 s is far above what a click on a local VS Code needs.
  use: {actionTimeout: 15000},
  workers: 1,               // one real VS Code at a time
  fullyParallel: false,
  retries: 0,
  reporter: [['list'], ['json', {outputFile: path.join(evidence, 'uat-results.json')}]]
};
