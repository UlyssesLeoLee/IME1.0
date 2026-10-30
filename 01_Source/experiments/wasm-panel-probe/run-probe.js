'use strict';
// Runs the WASM-in-webview probe on the real VS Code twice: with the planned CSP ('wasm-unsafe-eval') and without it
// (negative control). Prints one JSON line per run. Needs: VS Code, the uat/ dependencies (npm install in ../../uat).
//   node run-probe.js        (build the wasm first, see rust/panel.rs)
const {chromium} = require('../../uat/node_modules/@playwright/test');
const {spawn, spawnSync} = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const net = require('node:net');

const CODE = process.env.VSCODE_EXE || path.join(process.env.LOCALAPPDATA || '', 'Programs', 'Microsoft VS Code', 'Code.exe');
const EXT = path.join(__dirname, 'ext');
const sleep = ms => new Promise(r => setTimeout(r, ms));
const freePort = () => new Promise(res => { const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => res(p)); }); });

async function run(variant) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pime-probe-'));
  fs.mkdirSync(path.join(root, 'user', 'User'), {recursive: true}); fs.mkdirSync(path.join(root, 'ext')); fs.mkdirSync(path.join(root, 'ws'));
  fs.writeFileSync(path.join(root, 'user', 'User', 'settings.json'), JSON.stringify({'window.dialogStyle': 'custom', 'workbench.startupEditor': 'none', 'telemetry.telemetryLevel': 'off', 'update.mode': 'none', 'chat.disableAIFeatures': true}));
  const port = await freePort();
  const env = {...process.env, PROBE_CSP: variant}; delete env.ELECTRON_RUN_AS_NODE;
  const t0 = Date.now();
  const proc = spawn(CODE, [`--user-data-dir=${path.join(root, 'user')}`, `--extensions-dir=${path.join(root, 'ext')}`, `--extensionDevelopmentPath=${EXT}`, `--remote-debugging-port=${port}`,
    '--disable-gpu', '--disable-updates', '--skip-welcome', '--disable-workspace-trust', '--new-window', path.join(root, 'ws')], {stdio: 'ignore', env});
  let browser, result = {variant};
  try {
    for (let i = 0; i < 60 && !browser; i++) { try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`); } catch { await sleep(1000); } }
    const page = browser.contexts()[0].pages().find(p => /workbench/.test(p.url()));
    const frame = page.frameLocator('iframe.webview').last().frameLocator('#active-frame');
    await frame.locator('#out').waitFor({timeout: 30000});
    result.openedAfterLaunchMs = Date.now() - t0;
    for (let i = 0; i < 40; i++) { const s = await frame.locator('#out').getAttribute('data-state'); if (s !== 'pending') break; await sleep(250); }
    result.state = await frame.locator('#out').getAttribute('data-state');
    result.text = (await frame.locator('#out').innerText()).trim();
    result.instantiateMs = await frame.locator('#out').getAttribute('data-ms');
  } catch (e) { result.error = String(e.message).split('\n')[0]; }
  finally { try { await browser?.close(); } catch { /* ignore */ } spawnSync('taskkill', ['/F', '/T', '/PID', String(proc.pid)], {stdio: 'ignore'}); await sleep(800); try { fs.rmSync(root, {recursive: true, force: true}); } catch { /* best effort */ } }
  return result;
}
(async () => {
  if (!fs.existsSync(path.join(EXT, 'media', 'panel.wasm'))) { console.error('build ext/media/panel.wasm first (see rust/panel.rs)'); process.exit(2); }
  for (const v of ['with', 'without', 'with']) console.log(JSON.stringify(await run(v)));
})();
