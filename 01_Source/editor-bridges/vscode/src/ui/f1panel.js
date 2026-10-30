'use strict';
// F1 panel (SCR-02/03/04): two independent tabs (whole file / insert at cursor), per-hunk selection, exact target
// positions, text markers (+/-) so meaning never relies on colour alone. The page has no network access (CSP) and
// renders model text with textContent only; it cannot write to the document - only the extension can, after Apply.
const crypto = require('node:crypto');

const I18N_KEYS = ['f1.title', 'f1.intent', 'f1.generate', 'f1.regen', 'f1.cancel', 'f1.tab.full', 'f1.tab.insert', 'f1.idle', 'f1.loading', 'f1.openDiff', 'f1.applySelected', 'f1.applyInsert',
  'f1.validation.ok', 'f1.validation.bad', 'f1.validation.warn', 'f1.noChange'];

function getHtml(cspSource, nonce) {
  return `<!DOCTYPE html>
<html lang="en"><head><meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'nonce-${nonce}'; script-src 'nonce-${nonce}';">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Programming IME F1</title>
<style nonce="${nonce}">
  body { font-family: var(--vscode-font-family); color: var(--vscode-foreground); padding: 8px 14px 24px; font-size: var(--vscode-font-size); }
  h1 { font-size: 1.15em; margin: 4px 0 8px; }
  .meta { color: var(--vscode-descriptionForeground); font-size: .9em; margin-bottom: 8px; }
  .row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; margin: 6px 0; }
  input[type=text] { flex: 1 1 260px; background: var(--vscode-input-background); color: var(--vscode-input-foreground); border: 1px solid var(--vscode-input-border, transparent); padding: 4px 6px; }
  button { background: var(--vscode-button-background); color: var(--vscode-button-foreground); border: 0; padding: 5px 12px; cursor: pointer; }
  button.secondary { background: var(--vscode-button-secondaryBackground); color: var(--vscode-button-secondaryForeground); }
  button:disabled { opacity: .45; cursor: not-allowed; }
  button:focus-visible, input:focus-visible, [role=tab]:focus-visible { outline: 2px solid var(--vscode-focusBorder); outline-offset: 1px; }
  [role=tablist] { display: flex; border-bottom: 1px solid var(--vscode-panel-border); margin-top: 10px; }
  [role=tab] { background: none; color: var(--vscode-foreground); border: 0; border-bottom: 2px solid transparent; padding: 6px 12px; }
  [role=tab][aria-selected=true] { border-bottom-color: var(--vscode-focusBorder); font-weight: 600; }
  .banner { padding: 6px 10px; margin: 8px 0; border-left: 4px solid var(--vscode-focusBorder); background: var(--vscode-editorWidget-background); }
  .banner.warn { border-left-color: var(--vscode-editorWarning-foreground); }
  .banner.error { border-left-color: var(--vscode-editorError-foreground); }
  .status { margin: 8px 0; }
  fieldset.hunk { border: 1px solid var(--vscode-panel-border); margin: 8px 0; padding: 4px 8px; }
  legend { font-size: .9em; }
  .diff { font-family: var(--vscode-editor-font-family); font-size: var(--vscode-editor-font-size); white-space: pre-wrap; margin: 2px 0; }
  .diff .l { display: block; padding: 0 4px; }
  .diff .add { background: var(--vscode-diffEditor-insertedLineBackground, rgba(0,160,0,.15)); }
  .diff .del { background: var(--vscode-diffEditor-removedLineBackground, rgba(200,0,0,.15)); }
  pre.preview { max-height: 180px; overflow: auto; border: 1px solid var(--vscode-panel-border); padding: 4px 6px; font-family: var(--vscode-editor-font-family); }
  .cand { border: 1px solid var(--vscode-panel-border); margin: 8px 0; padding: 6px 8px; }
  .cand pre { margin: 4px 0; white-space: pre-wrap; font-family: var(--vscode-editor-font-family); }
  .hidden { display: none; }
</style></head>
<body>
  <h1 id="title"></h1>
  <div class="meta" id="meta"></div>
  <div class="row">
    <label for="intent" id="intent-label"></label>
    <input type="text" id="intent" autocomplete="off">
    <button id="generate"></button>
    <button id="cancel" class="secondary"></button>
  </div>
  <div id="banner" class="banner hidden" role="status" aria-live="polite"></div>
  <div role="tablist" aria-label="F1">
    <button role="tab" id="tab-full" aria-controls="panel-full" aria-selected="true"></button>
    <button role="tab" id="tab-insert" aria-controls="panel-insert" aria-selected="false"></button>
  </div>
  <section role="tabpanel" id="panel-full" aria-labelledby="tab-full"></section>
  <section role="tabpanel" id="panel-insert" aria-labelledby="tab-insert" class="hidden"></section>
  <div class="meta" id="usage"></div>
<script nonce="${nonce}">
(function () {
  const vscode = acquireVsCodeApi();
  const $ = id => document.getElementById(id);
  let S = null, intentTouched = false;
  const post = m => vscode.postMessage(m);
  function mk(tag, props, ...kids) {
    const n = document.createElement(tag);
    for (const [k, v] of Object.entries(props || {})) { if (k === 'class') n.className = v; else if (k.startsWith('on')) n.addEventListener(k.slice(2), v); else if (v !== undefined && v !== null && v !== false) n.setAttribute(k, v === true ? '' : v); }
    for (const kid of kids.flat()) if (kid !== undefined && kid !== null) n.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
    return n;
  }
  function statusLine(tab) {
    const msg = tab.status === 'loading' ? S.i18n['f1.loading'] : tab.message || (tab.status === 'idle' ? S.i18n['f1.idle'] : '');
    return mk('div', {class: 'status', role: 'status', 'data-status': tab.status}, msg);
  }
  function renderFull(panel, tab) {
    panel.replaceChildren(statusLine(tab));
    if (tab.scopeText) panel.append(mk('div', {class: 'meta'}, tab.scopeText));
    for (const note of tab.notes || []) panel.append(mk('div', {class: 'meta'}, note));
    if (tab.status !== 'ready') { if (tab.status !== 'unavailable') panel.append(regenRow('full')); return; }   // errors can be retried
    if (tab.hunks.length) {
      panel.append(mk('div', {}, tab.hunksText));
      tab.hunks.forEach(h => {
        const box = mk('input', {type: 'checkbox', id: 'cb-' + h.id, 'data-hunk': h.id, checked: h.selected, disabled: S.stale || S.busy, onchange: e => post({type: 'toggleHunk', id: h.id, on: e.target.checked})});
        const diff = mk('div', {class: 'diff'}, mk('div', {class: 'l'}, h.header), h.lines.map(l => mk('span', {class: 'l ' + (l.kind === '+' ? 'add' : l.kind === '-' ? 'del' : '')}, l.kind + ' ' + l.text)));
        panel.append(mk('fieldset', {class: 'hunk', 'data-hunk': h.id}, mk('legend', {}, box, ' ', mk('label', {for: 'cb-' + h.id}, h.title)), diff));
      });
      panel.append(mk('div', {class: 'meta', id: 'preview-title'}, tab.preview.title), mk('pre', {class: 'preview', id: 'preview', tabindex: '0'}, tab.preview.text));
      const v = tab.validation;
      panel.append(mk('div', {class: 'banner ' + (v.level === 'bad' ? 'error' : v.level === 'warn' ? 'warn' : ''), id: 'validation', 'data-level': v.level}, v.text, (v.issues || []).map(i => mk('div', {}, '- ' + i))));
      panel.append(mk('div', {class: 'row'},
        mk('button', {id: 'open-diff', class: 'secondary', onclick: () => post({type: 'openDiff'})}, S.i18n['f1.openDiff']),
        mk('button', {id: 'apply-full', disabled: !tab.canApply, onclick: () => post({type: 'applyFull'})}, S.i18n['f1.applySelected'])));
    }
    panel.append(regenRow('full'));
  }
  function regenRow(tab) {
    return mk('div', {class: 'row'}, mk('button', {class: 'secondary', id: 'regen-' + tab, disabled: S.busy, onclick: () => post({type: 'regenerate', tab})}, S.i18n['f1.regen']));
  }
  function renderInsert(panel, tab) {
    panel.replaceChildren(statusLine(tab));
    if (tab.scopeText) panel.append(mk('div', {class: 'meta'}, tab.scopeText));
    if (tab.targetText) panel.append(mk('div', {class: 'meta', id: 'target'}, tab.targetText));
    for (const note of tab.notes || []) panel.append(mk('div', {class: 'meta'}, note));
    if (tab.status === 'ready') {
      tab.candidates.forEach((c, i) => panel.append(mk('div', {class: 'cand', 'data-cand': c.id},
        mk('pre', {}, c.text),
        mk('div', {class: 'row'}, mk('button', {class: 'apply', disabled: !tab.canApply, 'data-cand': c.id, onclick: () => post({type: 'applyInsert', id: c.id})}, S.i18n['f1.applyInsert'] + ' #' + (i + 1))))));
    }
    if (tab.status !== 'unavailable') panel.append(regenRow('insert'));
  }
  function render() {
    const I = S.i18n;
    document.documentElement.lang = S.lang || 'en';
    $('title').textContent = I['f1.title'];
    $('meta').textContent = S.metaText;
    $('intent-label').textContent = I['f1.intent'];
    if (!intentTouched && $('intent').value === '') $('intent').value = S.intent || '';
    $('generate').textContent = I['f1.generate']; $('generate').disabled = !!S.busy || !S.canGenerate;
    $('cancel').textContent = I['f1.cancel']; $('cancel').disabled = !S.busy;
    const b = $('banner');
    b.className = 'banner' + (S.banner ? (S.banner.kind === 'error' ? ' error' : S.banner.kind === 'warn' ? ' warn' : '') : ' hidden');
    b.textContent = S.banner ? S.banner.text : '';
    $('tab-full').textContent = I['f1.tab.full']; $('tab-insert').textContent = I['f1.tab.insert'];
    for (const t of ['full', 'insert']) {
      const active = S.active === t;
      $('tab-' + t).setAttribute('aria-selected', String(active)); $('tab-' + t).tabIndex = active ? 0 : -1;
      $('panel-' + t).classList.toggle('hidden', !active);
    }
    renderFull($('panel-full'), S.tabs.full);
    renderInsert($('panel-insert'), S.tabs.insert);
    $('usage').textContent = S.usageText || '';
  }
  $('generate').addEventListener('click', () => post({type: 'generate', intent: $('intent').value}));
  $('cancel').addEventListener('click', () => post({type: 'cancel'}));
  $('intent').addEventListener('input', () => { intentTouched = true; });
  $('intent').addEventListener('keydown', e => { if (e.key === 'Enter' && !$('generate').disabled) post({type: 'generate', intent: $('intent').value}); });
  for (const t of ['full', 'insert']) $('tab-' + t).addEventListener('click', () => post({type: 'tab', tab: t}));
  window.addEventListener('message', e => { S = e.data; render(); });
  post({type: 'ready'});
})();
</script></body></html>`;
}

class F1Panel {
  constructor(vscode, {onMessage, onDispose}) {
    this.vscode = vscode;
    this.nonce = crypto.randomBytes(16).toString('base64').replace(/[^A-Za-z0-9]/g, 'x');
    this.panel = vscode.window.createWebviewPanel('programmingIme.f1', 'Programming IME F1', {viewColumn: vscode.ViewColumn.Beside, preserveFocus: false},
      {enableScripts: true, retainContextWhenHidden: true, localResourceRoots: []});
    this.panel.webview.html = getHtml(this.panel.webview.cspSource, this.nonce);
    this.ready = false; this.pending = null; this.disposed = false;
    this.panel.webview.onDidReceiveMessage(m => {
      if (m && m.type === 'ready') { this.ready = true; if (this.pending) { this.panel.webview.postMessage(this.pending); this.pending = null; } return; }
      onMessage(m);
    });
    this.panel.onDidDispose(() => { this.disposed = true; onDispose(); });
  }
  setTitle(t) { this.panel.title = t; }
  update(state) {
    if (this.disposed) return;
    if (this.ready) this.panel.webview.postMessage(state); else this.pending = state;
  }
  reveal() { if (!this.disposed) this.panel.reveal(this.vscode.ViewColumn.Beside, false); }
  dispose() { if (!this.disposed) this.panel.dispose(); }
}
module.exports = {F1Panel, getHtml, I18N_KEYS};
