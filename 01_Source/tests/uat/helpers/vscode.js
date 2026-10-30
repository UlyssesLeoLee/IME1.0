'use strict';
// Drives the REAL Visual Studio Code with the Programming IME extension loaded from source
// (--extensionDevelopmentPath) in a throw-away profile, over the Chrome DevTools protocol with Playwright.
// Nothing here touches the user's own VS Code settings, extensions, secrets or workspaces.
const {chromium} = require('@playwright/test');
const {spawn, spawnSync} = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const net = require('node:net');

const CODE_EXE = process.env.VSCODE_EXE || path.join(process.env.LOCALAPPDATA || '', 'Programs', 'Microsoft VS Code', 'Code.exe');
const EXTENSION_DIR = path.resolve(__dirname, '..', '..', '..', 'editor-bridges', 'vscode');
const EVIDENCE_DIR = process.env.UAT_EVIDENCE_DIR || path.resolve(__dirname, '..', '..', '..', '..', '02_Verification', 'uat');
const sleep = ms => new Promise(r => setTimeout(r, ms));
// PIME_ENGINE=rust PIME_CORE_EXE=<path to pime-core.exe> runs the whole suite against the Rust engine (programmingIme.engine=rust).
// Without PIME_ENGINE the profile is exactly what it always was (engine js, no core).
function engineSettings() {
  if (process.env.PIME_ENGINE !== 'rust') return {};
  const exe = process.env.PIME_CORE_EXE || '';
  if (!exe || !fs.existsSync(exe)) throw new Error(`PIME_ENGINE=rust needs PIME_CORE_EXE to point at a built pime-core executable (got "${exe}")`);
  return {'programmingIme.engine': 'rust', 'programmingIme.coreExecutable': exe};
}

const freePort = () => new Promise((resolve, reject) => {
  const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => resolve(p)); }); s.on('error', reject);
});
const BASE_SETTINGS = {
  'window.dialogStyle': 'custom', 'window.titleBarStyle': 'custom', 'workbench.startupEditor': 'none', 'workbench.tips.enabled': false,
  'workbench.enableExperiments': false, 'workbench.welcomePage.walkthroughs.openOnInstall': false, 'workbench.editor.enablePreview': false,
  'update.mode': 'none', 'telemetry.telemetryLevel': 'off', 'extensions.autoUpdate': false, 'extensions.ignoreRecommendations': true,
  'security.workspace.trust.startupPrompt': 'never', 'security.workspace.trust.banner': 'never', 'chat.disableAIFeatures': true,
  'files.autoSave': 'off', 'files.hotExit': 'off', 'editor.minimap.enabled': false, 'editor.autoClosingBrackets': 'never', 'editor.autoClosingQuotes': 'never',
  'editor.autoClosingOvertype': 'never', 'editor.autoIndent': 'none', 'editor.formatOnType': false, 'editor.formatOnPaste': false, 'editor.autoSurround': 'never',
  'editor.quickSuggestions': {other: false, comments: false, strings: false}, 'editor.suggestOnTriggerCharacters': false, 'editor.wordBasedSuggestions': 'off',
  'editor.parameterHints.enabled': false, 'editor.acceptSuggestionOnEnter': 'off', 'editor.tabCompletion': 'off', 'editor.hover.enabled': false,
  'editor.inlineSuggest.enabled': true, 'editor.wordWrap': 'off', 'editor.renderWhitespace': 'none', 'editor.occurrencesHighlight': 'off', 'editor.codeLens': false,
  'editor.stickyScroll.enabled': false, 'editor.bracketPairColorization.enabled': false, 'editor.guides.indentation': false, 'breadcrumbs.enabled': false,
  'git.enabled': false, 'git.autoRepositoryDetection': false, 'terminal.integrated.enablePersistentSessions': false, 'problems.visibility': false,
  'workbench.statusBar.visible': true, 'workbench.layoutControl.enabled': false, 'window.commandCenter': false, 'workbench.editor.restoreViewState': false,
  'json.schemaDownload.enable': false, 'typescript.disableAutomaticTypeAcquisition': true, 'npm.autoDetect': 'off', 'task.autoDetect': 'off'
};

class VsCode {
  static async launch({settings = {}, files = {}, trusted = true, caFile = null, keepProfile = false, env = {}} = {}) {
    if (!fs.existsSync(CODE_EXE)) throw new Error(`VS Code not found at ${CODE_EXE}; set VSCODE_EXE`);
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pime-uat-'));
    const app = new VsCode();
    Object.assign(app, {root, userDir: path.join(root, 'user'), extDir: path.join(root, 'ext'), ws: path.join(root, 'ws'), keepProfile, shots: 0});
    for (const d of [path.join(app.userDir, 'User'), app.extDir, app.ws]) fs.mkdirSync(d, {recursive: true});
    fs.writeFileSync(path.join(app.userDir, 'User', 'settings.json'), JSON.stringify({...BASE_SETTINGS, ...engineSettings(), ...settings}, null, 2));
    for (const [name, content] of Object.entries(files)) { fs.mkdirSync(path.dirname(path.join(app.ws, name)), {recursive: true}); fs.writeFileSync(path.join(app.ws, name), content); }
    app.port = await freePort();
    const childEnv = {...process.env, ...env};
    delete childEnv.ELECTRON_RUN_AS_NODE;
    // The mock provider uses a throw-away self-signed certificate. The extension host (Electron/BoringSSL) ignores
    // NODE_EXTRA_CA_CERTS, so verification is relaxed for THIS test process only; the product still insists on HTTPS.
    if (caFile) { childEnv.NODE_EXTRA_CA_CERTS = caFile; childEnv.NODE_TLS_REJECT_UNAUTHORIZED = '0'; }
    const args = [`--user-data-dir=${app.userDir}`, `--extensions-dir=${app.extDir}`, `--extensionDevelopmentPath=${EXTENSION_DIR}`, `--remote-debugging-port=${app.port}`,
      '--disable-gpu', '--disable-updates', '--skip-welcome', '--skip-release-notes', '--new-window', '--locale=en', ...(trusted ? ['--disable-workspace-trust'] : []), app.ws];
    app.proc = spawn(CODE_EXE, args, {stdio: 'ignore', env: childEnv, windowsHide: false});
    app.proc.on('exit', () => { app.exited = true; });
    for (let i = 0; i < 90 && !app.browser; i++) {
      try { app.browser = await chromium.connectOverCDP(`http://127.0.0.1:${app.port}`); } catch { await sleep(1000); }
    }
    if (!app.browser) { app.kill(); throw new Error('could not connect to VS Code over CDP'); }
    const ctx = app.browser.contexts()[0];
    for (let i = 0; i < 60 && !app.page; i++) { app.page = ctx.pages().find(p => /workbench/.test(p.url())); if (!app.page) await sleep(500); }
    await app.page.waitForSelector('.monaco-workbench', {timeout: 60000});
    await app.page.setViewportSize({width: 1500, height: 950}).catch(() => {});
    await app.page.waitForSelector('.statusbar', {timeout: 30000});
    await app.waitStatus(/PIME/, 45000);
    return app;
  }
  get statusbar() { return this.page.locator('.statusbar'); }
  async statusText() { return (await this.statusbar.innerText()).replace(/\s+/g, ' ').trim(); }
  async waitStatus(re, timeout = 15000) {
    const end = Date.now() + timeout; let last = '';
    while (Date.now() < end) { last = await this.statusText(); if (re.test(last)) return last; await sleep(150); }
    throw new Error(`status bar never matched ${re}; last: ${last}`);
  }
  // ---------------------------------------------------------------- quick input (command palette, pick lists, input boxes)
  get quick() { return this.page.locator('.quick-input-widget'); }
  async quickVisible() { return this.quick.isVisible().catch(() => false); }
  async quickRows() { return (await this.page.locator('.quick-input-list .monaco-list-row').allInnerTexts()).map(t => t.replace(/\s+/g, ' ').trim()); }
  async quickTitle() { return (await this.page.locator('.quick-input-title').innerText().catch(() => '')).trim(); }
  // A shortcut can be lost while focus is in limbo (for example right after a webview panel was closed): retry, giving the workbench focus.
  async openQuick(keys) {
    for (let attempt = 1; attempt <= 4; attempt++) {
      await this.page.keyboard.press(keys);
      try { await this.quick.waitFor({state: 'visible', timeout: 3000}); return; }
      catch (e) {
        if (attempt === 4) throw e;
        await this.page.keyboard.press('Escape').catch(() => {});
        await this.page.locator('.statusbar').click({position: {x: 2, y: 2}, timeout: 2000}).catch(() => {});
        await sleep(300);
      }
    }
  }
  async waitQuick(timeout = 10000) { await this.quick.waitFor({state: 'visible', timeout}); }
  async cmd(title) {
    await this.openQuick('Control+Shift+P');
    await this.page.keyboard.type(title, {delay: 5});
    await this.page.locator('.quick-input-list .monaco-list-row', {hasText: title}).first().waitFor({timeout: 8000});
    await this.page.keyboard.press('Enter');
    // The palette can stay on screen for a moment after Enter (seen on VS Code 1.144): wait until it is gone or has been replaced
    // by the picker/input the command opens, so callers never read the palette rows by mistake.
    await this.page.waitForFunction(() => {
      const w = document.querySelector('.quick-input-widget');
      if (!w || getComputedStyle(w).display === 'none') return true;
      const input = w.querySelector('.quick-input-box input');
      return !!input && !input.value.startsWith('>');
    }, undefined, {timeout: 6000}).catch(() => {});
  }
  async pickRow(text, {timeout = 8000} = {}) {
    const row = this.page.locator('.quick-input-list .monaco-list-row', {hasText: text}).first();
    await row.waitFor({timeout}); await row.click();
  }
  async inputBox(value, {timeout = 8000} = {}) {
    const box = this.page.locator('.quick-input-box input');
    await box.waitFor({state: 'visible', timeout});
    await box.fill(value); await this.page.keyboard.press('Enter');
  }
  async escapeQuick() { if (await this.quickVisible()) { await this.page.keyboard.press('Escape'); await this.quick.waitFor({state: 'hidden', timeout: 5000}).catch(() => {}); } }
  // ---------------------------------------------------------------- dialogs and notifications
  get dialog() { return this.page.locator('.monaco-dialog-box'); }
  async waitDialog(timeout = 10000) { await this.dialog.waitFor({state: 'visible', timeout}); return {message: (await this.page.locator('.dialog-message-text').innerText()).trim(), detail: (await this.page.locator('.dialog-message-detail').innerText().catch(() => '')).trim(), buttons: (await this.dialog.locator('.dialog-buttons .monaco-button').allInnerTexts()).map(s => s.trim())}; }
  async dialogClick(text) { await this.dialog.locator('.dialog-buttons .monaco-button', {hasText: text}).first().click(); await this.dialog.waitFor({state: 'hidden', timeout: 5000}).catch(() => {}); }
  async dialogVisible() { return this.dialog.isVisible().catch(() => false); }
  async notifications() { return (await this.page.locator('.notifications-toasts .notification-list-item-message').allInnerTexts()).map(s => s.trim()); }
  async waitNotification(re, timeout = 10000) {
    const end = Date.now() + timeout; let last = [];
    while (Date.now() < end) { last = await this.notifications(); const hit = last.find(t => re.test(t)); if (hit) return hit; await sleep(200); }
    throw new Error(`no notification matched ${re}; saw: ${JSON.stringify(last)}`);
  }
  async clearNotifications() { await this.openQuick('Control+Shift+P'); await this.page.keyboard.type('Notifications: Clear All Notifications', {delay: 3}); await this.page.keyboard.press('Enter'); await sleep(200); }
  // ---------------------------------------------------------------- editor
  async openFile(name) {
    await this.openQuick('Control+P');
    await this.page.keyboard.type(name, {delay: 5});
    const base = path.basename(name);
    const exact = this.page.locator('.quick-input-list .monaco-list-row').filter({has: this.page.locator('.label-name', {hasText: new RegExp(`^${base.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`)})}).first();
    await exact.waitFor({timeout: 8000});
    await exact.click();
    await this.page.locator('.tab.active', {hasText: base}).first().waitFor({timeout: 10000});
    await this.page.locator('.monaco-editor.focused, .monaco-editor').first().waitFor();
    await sleep(500);
    await this.focusEditor();
  }
  // Close every editor/panel, discarding unsaved edits, so each test starts from a single clean editor group.
  async closeAll() {
    await this.cmd('View: Close All Editors');
    for (let i = 0; i < 6; i++) {
      await sleep(350);
      if (await this.dialogVisible()) { await this.dialogClick("Don't Save"); continue; }
      if (await this.page.locator('.tab').count() === 0) break;
    }
    await sleep(300);
  }
  async focusEditor() { await this.page.locator('.editor-group-container.active .monaco-editor .view-lines').first().click({position: {x: 5, y: 5}}); }
  async editorText() {
    return this.page.evaluate(() => {
      const groups = [...document.querySelectorAll('.editor-group-container')];
      const active = groups.find(g => g.classList.contains('active') && g.querySelector('.monaco-editor .view-lines')) || groups.find(g => g.querySelector('.monaco-editor .view-lines'));
      const lines = [...active.querySelectorAll('.monaco-editor .view-lines .view-line')];
      lines.sort((a, b) => parseFloat(a.style.top) - parseFloat(b.style.top));
      // inline-suggestion ghost text is drawn inside the line but is NOT part of the document: leave it out
      return lines.map(l => { const c = l.cloneNode(true); c.querySelectorAll('[class*="ghost-text"]').forEach(e => e.remove()); return c.textContent.replace(/ /g, ' '); }).join('\n');
    });
  }
  async waitEditorText(re, timeout = 8000) {
    const end = Date.now() + timeout; let t = '';
    while (Date.now() < end) { t = await this.editorText(); if (typeof re === 'string' ? t.includes(re) : re.test(t)) return t; await sleep(150); }
    throw new Error(`editor text never matched ${re}; last: ${JSON.stringify(t.slice(0, 300))}`);
  }
  async goto(line, col = 1) { await this.openQuick('Control+G'); await this.page.keyboard.type(`${line}:${col}`); await this.page.keyboard.press('Enter'); await sleep(150); }
  async end() { await this.page.keyboard.press('Control+End'); }
  async type(text, delay = 12) { await this.page.keyboard.type(text, {delay}); }
  async isActiveTabDirty() { return this.page.locator('.tab.active.dirty').count().then(n => n > 0); }
  async dirtyTabs() { return this.page.locator('.tab.dirty').count(); }
  async undo() { await this.page.keyboard.press('Control+Z'); await sleep(250); }
  async save() { await this.page.keyboard.press('Control+S'); await sleep(400); }
  ghostText() { return this.page.evaluate(() => [...document.querySelectorAll('.ghost-text-decoration, .ghost-text')].map(e => e.textContent).join('')); }
  // ---------------------------------------------------------------- F1 panel (webview)
  get panelFrame() { return this.page.frameLocator('iframe.webview').last().frameLocator('#active-frame'); }
  async waitPanel(timeout = 15000) {
    await this.page.locator('iframe.webview').last().waitFor({timeout});
    await this.panelFrame.locator('#title').waitFor({timeout});
    await this.panelFrame.locator('#generate').waitFor({timeout});
  }
  async panelText(sel) { return (await this.panelFrame.locator(sel).innerText()).replace(/\s+/g, ' ').trim(); }
  async waitPanelStatus(tab, re, timeout = 20000) {
    const loc = this.panelFrame.locator(`#panel-${tab} .status`).first();
    const end = Date.now() + timeout; let last = '';
    while (Date.now() < end) { last = (await loc.getAttribute('data-status').catch(() => '')) || ''; if (re.test(last)) return last; await sleep(200); }
    throw new Error(`panel ${tab} status never matched ${re}; last: ${last}`);
  }
  // ---------------------------------------------------------------- misc
  async configureAI({provider, model, key, endpoint}) {
    await this.clearNotifications();      // so that waiting for "key saved" cannot match an older toast
    await this.cmd('Programming IME: Configure AI Provider');
    await this.waitQuick(); await this.pickRow(provider);
    const prompt = text => this.page.locator('.quick-input-message', {hasText: text}).waitFor({timeout: 8000});
    await prompt('HTTPS endpoint');
    await this.inputBox(endpoint || await this.page.locator('.quick-input-box input').inputValue());
    await prompt('Exact model id'); await this.inputBox(model);
    await prompt('API key'); await this.inputBox(key);
    await this.waitNotification(/SecretStorage/i, 10000);
    await sleep(600);                     // let the settings writes finish
  }
  userFile(name) { return path.join(this.ws, name); }
  readUser(name) { return fs.readFileSync(this.userFile(name), 'utf8'); }
  filesContaining(needle) {
    const hits = [], utf8 = Buffer.from(needle, 'utf8'), utf16 = Buffer.from(needle, 'utf16le');
    const walk = dir => { for (const e of fs.readdirSync(dir, {withFileTypes: true})) {
      const p = path.join(dir, e.name);
      if (e.isDirectory()) { if (!/Cache|GPUCache|Code Cache|DawnCache|blob_storage|Crashpad|logs|CachedData|ext$/i.test(e.name)) walk(p); continue; }
      try { if (fs.statSync(p).size > 60 * 1024 * 1024) continue; const b = fs.readFileSync(p); if (b.includes(utf8) || b.includes(utf16)) hits.push(path.relative(this.root, p)); } catch { /* locked file */ }
    } };
    walk(this.userDir);
    return hits;
  }
  async screenshot(name) {
    fs.mkdirSync(path.join(EVIDENCE_DIR, 'screenshots'), {recursive: true});
    const file = path.join(EVIDENCE_DIR, 'screenshots', `${String(++this.shots).padStart(2, '0')}-${name.replace(/[^\w.-]+/g, '_')}.png`);
    await this.page.screenshot({path: file}).catch(() => {});
    return file;
  }
  kill() {
    try { if (this.browser) this.browser.close().catch(() => {}); } catch { /* ignore */ }
    if (this.proc && !this.exited) spawnSync('taskkill', ['/F', '/T', '/PID', String(this.proc.pid)], {stdio: 'ignore'});
  }
  async close() {
    this.kill(); await sleep(800);
    if (!this.keepProfile) { try { fs.rmSync(this.root, {recursive: true, force: true, maxRetries: 5, retryDelay: 300}); } catch { /* temp dir cleanup is best effort */ } }
  }
}
module.exports = {VsCode, sleep, EVIDENCE_DIR, CODE_EXE};
