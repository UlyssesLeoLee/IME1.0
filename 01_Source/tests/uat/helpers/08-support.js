'use strict';
// Extra helpers for the UAT group 08 (used only by the 08-*.spec.js files).
// Nothing here touches the product: the helpers only read what the running VS Code shows and what the
// operating system reports about the processes of that VS Code instance.
const {execFileSync} = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const {expect} = require('@playwright/test');

// The label of every row of the open quick pick. A row can carry a second text (the detail of the item);
// the label is always the FIRST .label-name of the row, so the details are not mixed in.
async function quickLabels(app) {
  const rows = app.page.locator('.quick-input-list .monaco-list-row');
  const out = [];
  for (let i = 0; i < await rows.count(); i++) {
    const label = await rows.nth(i).locator('.label-name').first().innerText().catch(() => '');
    out.push(label.replace(/\s+/g, ' ').trim());
  }
  return out;
}
// The description of every row (for a candidate row: the source and the validation state).
async function quickDescriptions(app) {
  const raw = await app.page.locator('.quick-input-list .monaco-list-row .label-description').allInnerTexts();
  return raw.map(t => t.replace(/\s+/g, ' ').trim());
}

// The processes that run under the given root process (the VS Code instance of the test), as {pid, ppid, name}.
// Used as evidence that the IME never executes anything: no shell, loader or compiler appears under VS Code.
// The WMI query is retried: on a busy machine it can fail for a moment (the evidence must not be lost).
function processTree(rootPid) {
  let last = null;
  for (let attempt = 1; attempt <= 3; attempt++) {
    try {
      const raw = execFileSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command',
        "Get-CimInstance -Query 'SELECT ProcessId,ParentProcessId,Name FROM Win32_Process' | Select-Object ProcessId,ParentProcessId,Name | ConvertTo-Json -Compress"],
        {encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe']});
      const parsed = JSON.parse(raw);
      const all = (Array.isArray(parsed) ? parsed : [parsed]).map(p => ({pid: Number(p.ProcessId), ppid: Number(p.ParentProcessId), name: String(p.Name || '')}));
      const out = [];
      const walk = pid => {
        for (const p of all) {
          if (p.ppid === pid && !out.some(o => o.pid === p.pid)) { out.push(p); walk(p.pid); }
        }
      };
      walk(Number(rootPid));
      return out;
    } catch (e) { last = e; sleepMs(800); }
  }
  throw new Error(`the process list could not be read: ${last && last.stderr ? last.stderr.toString() : last && last.message}`);
}
// A short synchronous sleep (the helper is called from synchronous test code).
function sleepMs(ms) { Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms); }
// Executables that would mean that something was executed, injected or compiled.
const EXEC_NAMES = new Set([
  'cmd.exe', 'powershell.exe', 'pwsh.exe', 'bash.exe', 'sh.exe', 'zsh.exe', 'conhost.exe', 'wscript.exe', 'cscript.exe', 'mshta.exe',
  'rundll32.exe', 'regsvr32.exe', 'certutil.exe', 'bitsadmin.exe',
  'gcc.exe', 'cc.exe', 'clang.exe', 'clang-cl.exe', 'link.exe', 'lld-link.exe', 'nasm.exe', 'nasm64.exe', 'yasm.exe', 'as.exe',
  'make.exe', 'mingw32-make.exe', 'cmake.exe', 'ninja.exe', 'cargo.exe', 'rustc.exe', 'cl.exe'
]);
// The processes that appeared under VS Code between two snapshots and that would mean an execution.
function newExecutables(before, after) {
  const known = new Set(before.map(p => p.pid));
  return after.filter(p => !known.has(p.pid) && EXEC_NAMES.has(p.name.toLowerCase()));
}

// The files of the workspace with their size and time stamp: no file may be created, changed or deleted.
function workspaceFiles(dir) {
  const out = [];
  const walk = d => {
    for (const e of fs.readdirSync(d, {withFileTypes: true})) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) { walk(p); continue; }
      const st = fs.statSync(p);
      out.push(`${path.relative(dir, p)}|${st.size}|${st.mtimeMs}`);
    }
  };
  walk(dir);
  return out.sort();
}

// The leading white space of a line, as a number of characters.
const indentOf = line => /^[ \t]*/.exec(line)[0].length;

// The candidate overlay of the bridge (the command is the same one as Ctrl+Alt+Space).
async function openCandidates(app) {
  await app.cmd('Programming IME: Show Code Candidates');
  await app.waitQuick();
  await expect.poll(async () => (await quickLabels(app)).length, {timeout: 8000}).toBeGreaterThan(0);
  return quickLabels(app);
}
// The language drop-down: the label of a row is the profile id, the "(all files)" rows are a different scope.
async function pickLanguage(app, id) {
  await app.cmd('Programming IME: Choose Programming Language');
  await app.waitQuick();
  await app.page.locator('.quick-input-list .monaco-list-row .label-name').filter({hasText: new RegExp(`^${id}$`)}).first().click();
}

module.exports = {quickLabels, quickDescriptions, processTree, newExecutables, EXEC_NAMES, workspaceFiles, indentOf, openCandidates, pickLanguage};
