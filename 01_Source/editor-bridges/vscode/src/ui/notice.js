'use strict';
// TransientNotice: a short message in the bottom-right corner of the VS Code window that goes away by itself as soon as
// the user does something else (types, clicks, moves the cursor, scrolls, switches editor, leaves the window) or after
// a timeout. Built for "nothing to optimise" (F2), where a modal or a lasting notification would be noise.
//
// WHAT CAN AND CANNOT BE OBSERVED (checked against the VS Code API reference and the workbench sources):
//  - Mouse MOVEMENT is not observable by an extension: the extension host is a separate process with no DOM access and
//    the API has no pointer event. Clicks reach us only as a selection change in a text editor, wheel/scrollbar only as
//    a visible-range change. So "mouse movement" is approximated by selection ('selection') and scroll ('scroll').
//  - Typing in a TERMINAL, the Explorer, a webview or any other non-text-editor view is not observable either. The
//    stable API has no key event; Pseudoterminal.handleInput works only for terminals this extension creates itself;
//    TerminalState.isInteractedWith flips once per terminal (not per key), so it is not used.
//  - Partial help, used here: WindowState.active (onDidChangeWindowState). The workbench sets it on keydown, mousedown
//    and touchstart anywhere in the window (terminal included, but not webviews and NOT mousemove) and clears it after
//    10 s without any of those. So an inactive -> active flip means "the user pressed a key or clicked", reason
//    'activity'. It can only fire after >= 10 s of idleness, so it is a late safety net, not an instant trigger.
//    Also used when present: onDidStartTerminalShellExecution (a command was run in a terminal with shell integration)
//    and onDidChangeActiveTerminal, both reason 'terminal'.
//
// PRESENTATION (internal, swappable through PRESENTATIONS; option `presentation`):
//  'toast' (default): window.withProgress({location: Notification, cancellable: false}) whose task resolves when we close.
//     Why: it is the only way for an extension to show a real bottom-right toast AND close it itself (showInformationMessage
//     cannot be closed from the extension; the commands that hide it hide every toast and keep an entry in the
//     notification centre). Closing the progress handle removes the item from the notification centre, so nothing is left
//     behind: the bell dot that an added notification causes is undone by the removal. An item with active progress is
//     sticky, so VS Code's own 10 s auto-hide never races our timeout. Costs, read from the sources:
//       * infinite progress draws an animated bar across the toast, which reads as "working" (not a spinner icon). We
//         report {increment: 100} once: that turns it into a static full bar (worked() never sets `done`, so the item
//         stays sticky and closable).
//       * the severity is always Info and icons are not rendered, so kind 'warn' only adds a leading warning sign.
//       * VS Code shows the toast for at least 800 ms even if we close earlier.
//       * Do Not Disturb (errors only) shows no toast: the title appears as a status-bar progress entry instead. Pressing
//         Escape (notifications.hideToasts), or opening the notification centre, hides the toast but leaves the item in
//         the centre with a status-bar spinner until we close it (<= timeoutMs). The "workbench.notifications.position"
//         setting can move toasts to the top-right.
//  'statusbar': a right-aligned StatusBarItem, fully closable and traceless, but small and tied to the status bar. It is
//     also the automatic fallback when withProgress throws.
//
// Triggers exist only while a notice is open; every subscription is disposed when it closes. Events within graceMs after
// show() are ignored: the keypress or command that produced the notice must not dismiss it. Nothing here throws out of
// show()/dismiss()/dispose(): a failing VS Code call is swallowed and the notice simply is not shown.
// dismissed reasons: 'input' | 'selection' | 'scroll' | 'editor' | 'focus' | 'activity' | 'terminal' | 'timeout' |
//   'replaced' | 'disposed' | 'api' | 'empty' (nothing to show) | 'error' (no presentation worked).

const DEFAULT_TIMEOUT_MS = 20000, DEFAULT_GRACE_MS = 600;
const NOISE_SCHEMES = new Set(['output']); // Output-channel documents are rewritten and auto-scrolled by every extension, never by the user's typing
const WARN = String.fromCharCode(0x26A0); // warning sign; the toast cannot render icons, so a plain character marks kind 'warn'
const swallow = () => {};
const num = (v, dflt, min) => (typeof v === 'number' && Number.isFinite(v) && v >= min ? v : dflt);
const noisy = doc => !!(doc && doc.uri && NOISE_SCHEMES.has(doc.uri.scheme));

function presentToast(vscode, {text, kind}, onLost) {
  let release;
  const gate = new Promise(r => { release = r; });
  try {
    const shown = vscode.window.withProgress({location: vscode.ProgressLocation.Notification, title: kind === 'warn' ? `${WARN} ${text}` : text, cancellable: false}, progress => {
      try { progress.report({increment: 100}); } catch (e) { swallow(e); } // static full bar instead of an endless animation
      return gate;
    });
    Promise.resolve(shown).then(undefined, onLost); // a rejected thenable means nothing is on screen any more
  } catch (e) { release(); throw e; }
  return () => release();
}

function presentStatusBar(vscode, {text, kind}) {
  const item = vscode.window.createStatusBarItem('programmingIme.notice', vscode.StatusBarAlignment.Right, 1000);
  try {
    const line = text.replace(/\s+/g, ' ').trim();
    item.name = 'Programming IME';
    item.text = `$(${kind === 'warn' ? 'warning' : 'info'}) ${line.replace(/\$\(/g, () => '\\$(')}`; // a literal "$(" in the text must not become an icon
    item.tooltip = text;
    if (kind === 'warn') item.backgroundColor = new vscode.ThemeColor('statusBarItem.warningBackground');
    item.show();
  } catch (e) { try { item.dispose(); } catch (e2) { swallow(e2); } throw e; }
  return () => item.dispose();
}
const PRESENTATIONS = {toast: presentToast, statusbar: presentStatusBar};

// [namespace, event, pick]: pick(e) returns the dismiss reason or a falsy value to ignore the event.
function triggers(vscode) {
  const w = vscode.window;
  let wasActive = !(w && w.state && w.state.active === false); // baseline: only an inactive -> active flip counts
  return [
    ['workspace', 'onDidChangeTextDocument', e => e.contentChanges.length > 0 && !noisy(e.document) && 'input'],
    ['window', 'onDidChangeTextEditorSelection', e => !noisy(e.textEditor && e.textEditor.document) && 'selection'],
    ['window', 'onDidChangeTextEditorVisibleRanges', e => !noisy(e.textEditor && e.textEditor.document) && 'scroll'],
    ['window', 'onDidChangeActiveTextEditor', () => 'editor'],
    ['window', 'onDidChangeWindowState', s => {
      const before = wasActive; wasActive = s.active !== false; // tracked even during the grace period
      if (s.focused === false) return 'focus';
      return s.active === true && !before && 'activity';
    }],
    ['window', 'onDidStartTerminalShellExecution', () => 'terminal'],
    ['window', 'onDidChangeActiveTerminal', () => 'terminal']
  ];
}

class TransientNotice {
  constructor(vscode, {timeoutMs, graceMs, setTimer = setTimeout, clearTimer = clearTimeout, now = Date.now, presentation = 'toast'} = {}) {
    this._vscode = vscode; this._now = () => now();
    this._setTimer = (fn, ms) => setTimer(fn, ms); this._clearTimer = id => clearTimer(id); // called detached: no `this` leaks into setTimeout or a fake clock
    this._timeoutMs = num(timeoutMs, DEFAULT_TIMEOUT_MS, 1); this._graceMs = num(graceMs, DEFAULT_GRACE_MS, 0);
    this._presentation = presentation === 'statusbar' ? 'statusbar' : 'toast';
    this._cur = null; this._disposed = false;
  }
  get active() { return this._cur !== null; }

  show({text, kind = 'info'} = {}) {
    if (this._disposed) return {dismissed: Promise.resolve('disposed')};
    const msg = typeof text === 'string' ? text.trim() : '';
    if (!msg) return {dismissed: Promise.resolve('empty')}; // the open notice, if any, stays as it is
    this.dismiss('replaced');
    const cur = {subs: [], timer: undefined, release: null, shownAt: 0, resolve: swallow};
    const dismissed = new Promise(r => { cur.resolve = r; });
    this._cur = cur;
    try { cur.shownAt = this._now(); } catch (e) { swallow(e); }
    const order = this._presentation === 'statusbar' ? ['statusbar', 'toast'] : ['toast', 'statusbar'];
    for (const name of order) {
      try { cur.release = PRESENTATIONS[name](this._vscode, {text: msg, kind: kind === 'warn' ? 'warn' : 'info'}, () => this._close(cur, 'error')); break; } catch (e) { swallow(e); }
    }
    if (!cur.release) { this._cur = null; cur.resolve('error'); return {dismissed}; }
    if (this._cur !== cur) return {dismissed}; // closed re-entrantly while presenting
    this._subscribe(cur);
    try { cur.timer = this._setTimer(() => this._close(cur, 'timeout'), this._timeoutMs); if (cur.timer && typeof cur.timer.unref === 'function') cur.timer.unref(); } catch (e) { swallow(e); }
    return {dismissed};
  }

  dismiss(reason = 'api') { if (this._cur) this._close(this._cur, reason); }

  dispose() { this._disposed = true; this.dismiss('disposed'); }

  _subscribe(cur) {
    for (const [ns, name, pick] of triggers(this._vscode)) {
      try {
        const host = this._vscode[ns];
        if (!host || typeof host[name] !== 'function') continue; // older VS Code or a reduced fake: skip just this trigger
        const d = host[name](e => {
          if (this._cur !== cur) return;
          let reason = null;
          try { reason = pick(e); } catch (err) { swallow(err); }
          let age = Infinity;
          try { age = this._now() - cur.shownAt; } catch (err) { swallow(err); }
          if (reason && age >= this._graceMs) this._close(cur, reason);
        });
        if (d && typeof d.dispose === 'function') cur.subs.push(d);
      } catch (e) { swallow(e); }
    }
  }

  _close(cur, reason) {
    if (this._cur !== cur) return; // already closed: idempotent
    this._cur = null;
    try { if (cur.timer !== undefined) this._clearTimer(cur.timer); } catch (e) { swallow(e); }
    for (const d of cur.subs.splice(0)) { try { d.dispose(); } catch (e) { swallow(e); } }
    try { if (cur.release) cur.release(); } catch (e) { swallow(e); }
    cur.resolve(reason);
  }
}

module.exports = {TransientNotice, PRESENTATIONS};
