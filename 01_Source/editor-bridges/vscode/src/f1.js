'use strict';
// F1 workflow (FR-031..040, DD §9): three independent paths - annotate the scope under the cursor (or the selection), whole-file
// change and insert-at-cursor - each with its own request, grant, state and error. Nothing is written until the user presses Apply;
// Apply goes through the EditCoordinator (version+hash re-check, one-use token, read-back) as a single WorkspaceEdit.
//
// The annotate tab (docs/F1_ANNOTATE.md) is the default while programmingIme.f1Mode is "annotate": no selection -> the innermost declaration
// around the cursor, a selection -> exactly the selected lines. The model only says "above scope line N put this comment"; core/annotate.js
// builds the commented text, so code and existing comments cannot change. The proposal then goes through the same hunk review, Diff and
// Apply machinery as the whole-file tab (kind 'full').
// `vscode` and the runtime are injected (createF1), so the flow is tested against fakes (tests/js/ut-f1-annotate.test.js); the exports at
// the end wire the real ones the first time they are used.
// A secret-class file (.env, keys ...) is not read at all until the user confirmed it (src/filegate.js): open() asks before openSession()
// takes the first byte of the document, and a refusal opens no panel.
const crypto = require('node:crypto');
const {fail, codeOf} = require('./core/errors');
const {gateFor} = require('./filegate');
const settingsMod = require('./core/settings');
const scopeMod = require('./core/scope');
const annotateMod = require('./core/annotate');
const {F1Panel, I18N_KEYS} = require('./ui/f1panel');
const {bundle} = require('./core/i18n');

const PREVIEW_SCHEME = 'pime-preview';
const MAX_FULL_BYTES = 2 * 1024 * 1024;
const MAX_SCOPE_CHARS = 20000;   // a scope above this is never sent (core/scope.js default)
const MAX_SCOPE_LINES = 1200;    // ... nor one with more lines: past ~2000 lines the line diff of the review (core/diff.js MAX_CELLS) degrades into ONE block
const MIN_ANNOTATE_TOKENS = 3000; // room for the JSON reply with its comments
const EOL = /\r\n|\r|\n/g;
const MARKER = /PIME_REDACTED_\d+/g;

const toSel = s => ({start: {line: s.start.line, character: s.start.character}, end: {line: s.end.line, character: s.end.character}});
const short = h => h.slice(0, 12);
const baseName = p => String(p || '').split(/[\\/]/).pop();
const proposalTab = () => ({status: 'idle', message: '', notes: [], hunks: [], review: null, selected: new Set(), proposed: '', result: null, grantId: null, validation: {level: 'none', text: '', issues: []}});

// ------------------------------------------------------------------ pure helpers (exported for the tests)
// Number of lines as the scope resolver counts them (\r\n, \r and \n each end a line).
function lineCount(text) { return (text.match(EOL) || []).length + 1; }
// Offsets {start, end} of lines first..last (0-based, inclusive); end excludes the line break. null when the text has fewer lines.
// Cutting by offsets keeps the original line breaks (a split and join would turn CRLF into LF).
function lineSpan(text, first, last) {
  const starts = [0], ends = [];
  EOL.lastIndex = 0;
  for (let m; (m = EOL.exec(text));) { ends.push(m.index); starts.push(m.index + m[0].length); }
  ends.push(text.length);
  return first < 0 || last < first || last >= starts.length ? null : {start: starts[first], end: ends[last]};
}
// The reply was written against the redacted file: a marker in a comment must never become a secret again, so it turns into a plain word.
const maskMarkers = raw => String(raw).replace(MARKER, '<redacted>');
// The line break the document uses where the scope has none of its own (a one-line scope): lines inserted next to it must use the same
// one, or the editor would normalise them and the read-back of the edit would no longer match the proposal.
function documentEol(fileText) { const m = /\r\n|\r|\n/.exec(fileText); return m ? m[0] : '\n'; }
function fitEol(text, scopeText, fileText) {
  if (/[\r\n]/.test(scopeText)) return text; // a multi-line scope carries its own line break and the comment lines copy it
  const eol = documentEol(fileText);
  return eol === '\n' ? text : text.replace(/\r\n|\r|\n/g, eol);
}
// -> {ok:false, reason} | {ok:true, all, allowed, blocked, tooLarge}. all: widening chain of the resolver, innermost first (resolved without a size limit);
// allowed: the leading part that fits maxChars and maxLines; blocked: the first entry that does not fit (shown, never selectable).
function resolveTarget({profile, dialect, text, start, end}, maxChars = MAX_SCOPE_CHARS, maxLines = MAX_SCOPE_LINES) {
  const r = scopeMod.resolveScope({profile, dialect, text, start, end, maxChars: Infinity});
  if (!r.ok) return r;
  const fits = e => e.end - e.start <= maxChars && e.endLine - e.startLine + 1 <= maxLines;
  const allowed = [];
  for (const e of r.chain) { if (!fits(e)) break; allowed.push(e); }
  return {ok: true, all: r.chain, allowed, blocked: allowed.length < r.chain.length ? r.chain[allowed.length] : null, tooLarge: allowed.length === 0};
}

function createF1(env) {
  const {vscode, rt, L, errorText, enabled, aiEnabled, workspaceId, grantProviderId, cfg, infoFor, annotation} = env;
  const Panel = env.Panel || F1Panel;
  const fileGate = gateFor(env);
  const previews = new Map();
  let session = null;
  let opening = false;
  const inflight = new Set(); // panel messages being handled: settled() waits for them

  const visibleEditorFor = doc => vscode.window.visibleTextEditors.find(e => e.document === doc);
  // autoComment / graphComment: the proposal is reviewed in the panel, so the settings only extend the model instructions
  async function withAnnotation(prompt, task, s) {
    const w = annotation.wanted();
    const add = await rt.engine.annotate.f1Addendum({task, comment: w.comment, graph: w.graph, locale: rt.locale, language: s.profile, dialect: s.dialect});
    return add ? {...prompt, instructions: prompt.instructions + add} : prompt;
  }

  function registerPreviewProvider(ctx) {
    ctx.subscriptions.push(vscode.workspace.registerTextDocumentContentProvider(PREVIEW_SCHEME, {provideTextDocumentContent: uri => previews.get(uri.path.slice(1)) || ''}));
  }

  // ------------------------------------------------------------------ availability (FR-015/039, UT-053)
  function unavailableReason(doc, snapshot, tab) {
    if (!vscode.workspace.isTrusted) return L('consent.untrusted');
    if (!aiEnabled()) return `${L('ai.off')} - Programming IME: Toggle AI`;
    if (tab === 'full') {
      if (!['file', 'untitled'].includes(doc.uri.scheme)) return errorText(fail('FULL_CONTEXT_REQUIRED'));
      if (Buffer.byteLength(snapshot.content, 'utf8') > MAX_FULL_BYTES) return L('f1.tooLarge');
    }
    return null;
  }
  function newTabs(doc, snapshot) {
    const full = proposalTab();
    const insert = {status: 'idle', message: '', notes: [], candidates: [], grantId: null, stale: false};
    for (const [name, tab] of [['full', full], ['insert', insert]]) {
      const reason = unavailableReason(doc, snapshot, name);
      if (reason) { tab.status = 'unavailable'; tab.message = name === 'full' ? L('f1.unavailable.full', reason) : reason; }
    }
    return {full, insert};
  }

  // ------------------------------------------------------------------ the annotate tab: target scope and options
  const currentScope = tab => tab.scope ? (tab.scope.allowed[tab.scope.index] || tab.scope.all[0]) : null;
  function resetAnnotate(t) {
    Object.assign(t, {status: 'idle', message: '', notes: [], hunks: [], review: null, selected: new Set(), proposed: '', result: null, grantId: null, validation: {level: 'none', text: '', issues: []}});
  }
  // Apply goes through kind 'full', so the tab is unavailable wherever the whole-file tab is; plus what only this tab needs.
  // `because` marks the two reasons that belong to the CONTENT only (the language has no annotation, the document is empty): the other tabs work there,
  // so F1 opens on the whole-file tab instead (see openSession). Every other reason (AI off, untrusted, not a file, too large) hits all tabs alike.
  function newAnnotateTab(doc, snapshot, info, offsets, carried) {
    const w = annotation.wanted();
    const tab = {...proposalTab(), scope: null, options: carried ? {...carried} : {comment: w.comment, graph: w.graph}, graphScope: w.scope, because: null};
    const off = (why, because = null) => { tab.status = 'unavailable'; tab.message = why; tab.because = because; return tab; };
    if (!vscode.workspace.isTrusted) return off(L('consent.untrusted'));
    if (!aiEnabled()) return off(`${L('ai.off')} - Programming IME: Toggle AI`);
    if (!['file', 'untitled'].includes(doc.uri.scheme)) return off(errorText(fail('FULL_CONTEXT_REQUIRED')));
    if (Buffer.byteLength(snapshot.content, 'utf8') > MAX_FULL_BYTES) return off(L('f1.ann.fileTooLarge'));
    if (!annotateMod.supportsAnnotation(info.language)) return off(L('f1.ann.unsupported', info.language), 'language');
    const r = resolveTarget({profile: info.language, dialect: info.dialect, text: snapshot.content, start: offsets.start, end: offsets.end});
    if (!r.ok) return r.reason === 'empty-document' ? off(L('f1.ann.empty'), 'empty') : off(L('f1.ann.unsupported', info.language), 'language');
    tab.scope = {all: r.all, allowed: r.allowed, blocked: r.blocked, index: 0};
    if (r.tooLarge) return off(sizeMessage('f1.ann.tooLarge', r.all[0]));
    return tab;
  }
  const scopeLabel = e => e.kind === 'declaration' ? e.label : L(`f1.ann.crumb.${e.kind}`); // a declaration label is code: never translated
  const sizeMessage = (key, e) => L(key, scopeLabel(e), e.end - e.start, e.endLine - e.startLine + 1, MAX_SCOPE_CHARS, MAX_SCOPE_LINES);
  const rangeText = e => e.startLine === e.endLine ? L('f1.ann.range.one', e.startLine + 1) : L('f1.ann.range.many', e.startLine + 1, e.endLine + 1);
  const countText = e => e.endLine === e.startLine ? L('f1.ann.count.one') : L('f1.ann.count.many', e.endLine - e.startLine + 1);
  function scopeHeader(e) {
    if (e.kind === 'declaration') return L('f1.ann.scope.decl', e.label, rangeText(e), countText(e));
    return L(`f1.ann.scope.${e.kind}`, rangeText(e), countText(e));
  }
  function moveScope(s, move) {
    const t = s.tabs.annotate, sc = t.scope;
    if (!sc || s.busy || s.applying || s.stale || s.done || t.status === 'unavailable') return;
    const next = sc.index + (move === 'wider' ? 1 : move === 'narrower' ? -1 : 0);
    if (next < 0 || next >= sc.allowed.length || next === sc.index) return;
    sc.index = next;
    resetAnnotate(t); // a new scope is a new request: the user presses Generate, nothing is sent on its own
    s.reveal = true;
  }
  function setOption(s, name, on) {
    const t = s.tabs.annotate;
    if (s.busy || s.applying || s.done || (name !== 'comment' && name !== 'graph')) return;
    t.options[name] = !!on;
    if (t.status !== 'unavailable') resetAnnotate(t);
  }

  // ------------------------------------------------------------------ highlight of the target scope in the editor
  // Visible only while the annotate tab is the active one and the proposal can still be applied. It never moves the cursor or the focus;
  // the editor scrolls only when the scope is entirely out of view and the user has just changed it (Wider / Narrower).
  const wantsHighlight = s => !s.closed && !s.stale && !s.done && s.active === 'annotate' && !!currentScope(s.tabs.annotate);
  function dropHighlight(s) { if (s.hl) { try { s.hl.type.dispose(); } catch { /* already gone */ } s.hl = null; } }
  function syncHighlight(s) {
    if (!wantsHighlight(s)) { dropHighlight(s); return; }
    const editor = visibleEditorFor(s.doc);
    if (!editor) return;
    const e = currentScope(s.tabs.annotate), key = `${e.startLine}:${e.endLine}`;
    if (s.hl && s.hl.key === key && s.hl.editor === editor && !s.reveal) return;
    if (!s.hl) {
      s.hl = {key: '', editor: null, type: vscode.window.createTextEditorDecorationType({
        isWholeLine: true, backgroundColor: new vscode.ThemeColor('editor.rangeHighlightBackground'),
        overviewRulerColor: new vscode.ThemeColor('editorOverviewRuler.rangeHighlightForeground'), overviewRulerLane: vscode.OverviewRulerLane.Center})};
    }
    if (s.hl.editor && s.hl.editor !== editor) s.hl.editor.setDecorations(s.hl.type, []);
    const range = new vscode.Range(e.startLine, 0, e.endLine, 0);
    editor.setDecorations(s.hl.type, [range]);
    s.hl.key = key; s.hl.editor = editor;
    if (s.reveal) {
      s.reveal = false;
      if (!(editor.visibleRanges || []).some(r => r.start.line <= e.endLine && r.end.line >= e.startLine)) editor.revealRange(range, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
    }
  }

  // ------------------------------------------------------------------ state for the webview
  // The engine (js or rust) builds hunks, applies a selection and checks the syntax of the result; `result` is
  // {length, hash, preview, truncated, validation}. Without `result` the selection is evaluated now.
  async function recompute(s, name, result) {
    const t = s.tabs[name];
    const r = t.result = result || await rt.engine.select(s.snapshot, t.review, [...t.selected], {language: s.profile, dialect: s.dialect});
    const after = r.validation;
    if (after.ok) t.validation = {level: 'ok', text: L('f1.validation.ok'), issues: []};
    else if (s.baselineValid) t.validation = {level: 'bad', text: L('f1.validation.bad'), issues: after.issues.slice(0, 5).map(i => `${i.line ? 'L' + i.line + ': ' : ''}${i.message}`)};
    else t.validation = {level: 'warn', text: L('f1.validation.warn'), issues: after.issues.slice(0, 3).map(i => `${i.line ? 'L' + i.line + ': ' : ''}${i.message}`)};
  }
  const hunkViews = t => t.hunks.map((h, i) => ({id: h.id, header: h.header, lines: h.lines, title: L('f1.hunk', i + 1, h.oldStartLine, h.oldLineCount, h.newLineCount), selected: t.selected.has(h.id)}));
  const previewOf = t => ({title: L('f1.preview', t.result ? t.result.length : 0, short(t.result ? t.result.hash : '')), text: t.result ? t.result.preview + (t.result.truncated ? '\n…' : '') : ''});
  function annotateView(s) {
    const t = s.tabs.annotate, sc = t.scope, cur = currentScope(t);
    const any = t.options.comment || t.options.graph;
    const canAct = t.status === 'ready' && t.selected.size > 0 && t.validation.level !== 'bad' && !s.stale && !s.busy && !s.done;
    const scope = !cur ? null : {
      text: scopeHeader(cur),
      crumbs: [...sc.allowed.map((e, i) => ({label: scopeLabel(e), current: i === sc.index})),
        ...(sc.blocked ? [{label: scopeLabel(sc.blocked), disabled: true, title: sizeMessage('f1.ann.widerBlocked', sc.blocked)}] : [])],
      index: sc.index,
      canWider: sc.index < sc.allowed.length - 1 && !s.busy && !s.applying && !s.stale && !s.done && t.status !== 'unavailable',
      canNarrower: sc.index > 0 && !s.busy && !s.applying && !s.stale && !s.done && t.status !== 'unavailable',
      hint: sc.index === sc.allowed.length - 1 && sc.blocked ? sizeMessage('f1.ann.widerBlocked', sc.blocked) : ''
    };
    return {
      status: t.status, message: t.status === 'idle' && !any ? L('f1.ann.nothingRequested') : t.message, notes: t.notes,
      scope, options: {comment: t.options.comment, graph: t.options.graph, locked: s.busy || s.applying || s.done || t.status === 'unavailable'},
      hunksText: L('f1.hunks', t.hunks.length), hunks: hunkViews(t), preview: previewOf(t), validation: t.validation, canApply: canAct, canRun: any && t.status !== 'unavailable'
    };
  }
  function viewState() {
    const s = session, snap = s.snapshot, I = bundle(rt.locale, I18N_KEYS);
    const full = s.tabs.full, ins = s.tabs.insert;
    const canActFull = full.status === 'ready' && full.selected.size > 0 && full.validation.level !== 'bad' && !s.stale && !s.busy && !s.done;
    const canActIns = ins.status === 'ready' && !s.stale && !ins.stale && !s.busy && !s.done;
    const budget = rt.budget.snapshot();
    const ann = annotateView(s);
    const canGenerate = s.mode === 'annotate' ? !s.busy && !s.done && s.tabs[s.active].status !== 'unavailable' && (s.active !== 'annotate' || ann.canRun)
      : !s.busy && !s.done && (full.status !== 'unavailable' || ins.status !== 'unavailable');
    return {
      i18n: I, lang: rt.locale, active: s.active, mode: s.mode, busy: s.busy, stale: s.stale, intent: s.intent, canGenerate,
      metaText: `${s.profile} | ${baseName(snap.file_name) || snap.document_uri} | v${snap.document_version} | ${snap.eol} | ${s.models.full || '-'} / ${s.models.insert || '-'}`,
      banner: s.banner,
      usageText: L('f1.usage', budget.requests, budget.tokens, budget.estimated ? ' (estimated)' : ''),
      tabs: {
        full: {
          status: full.status, message: full.message, notes: full.notes,
          scopeText: L('f1.scope.full', baseName(snap.file_name), snap.document_version, short(snap.sha256)),
          hunksText: L('f1.hunks', full.hunks.length),
          hunks: hunkViews(full),
          preview: previewOf(full),
          validation: full.validation, canApply: canActFull
        },
        insert: {
          status: ins.status, message: ins.message, notes: ins.notes,
          scopeText: L('f1.scope.insert', `${snap.selection.start.line + 1}:${snap.selection.start.character + 1}`),
          targetText: s.targetText, candidates: ins.candidates.map(c => ({id: c.id, text: c.text})), canApply: canActIns
        },
        annotate: ann
      }
    };
  }
  const render = () => { if (session && session.panel) { session.panel.update(viewState()); syncHighlight(session); } };

  // ------------------------------------------------------------------ opening
  // opts.autoStart=false: the caller starts the request itself (reopen). opts.carry: {active, options} of the session being replaced.
  async function open(editorOverride, opts = {}) {
    if (!enabled()) { vscode.window.showWarningMessage(L('f1.disabledHere')); return; }
    const editor = editorOverride || vscode.window.activeTextEditor;
    if (!editor) { vscode.window.showInformationMessage(L('f1.noEditor')); return; }
    if (opening) return; // the engine call below is asynchronous: a second F1 must not start a second session
    opening = true;
    let opened = null;
    try {
      // before the first byte of the document is read, and before an open session on another file is closed: a refusal changes nothing
      if (!await fileGate.ensure(editor.document, {interactive: true, purpose: 'f1'})) { vscode.window.showInformationMessage(L('sec.refused.f1')); return; }
      opened = await openSession(editor, opts);
    } catch (e) { vscode.window.showErrorMessage(L('f1.error', errorText(e))); } finally { opening = false; }
    // After the lock is released: the request can take a while and F1 must stay usable meanwhile (a second F1 starts a fresh session).
    if (opened && opts.autoStart !== false) opened.autostart = autoStart(opened);
  }
  async function openSession(editor, opts) {
    closeSession();
    const doc = editor.document;
    const text = doc.getText();
    const sel = toSel(editor.selection);
    const version = doc.version;
    const info = infoFor(doc);
    const selStart = doc.offsetAt(editor.selection.start), selEnd = doc.offsetAt(editor.selection.end);
    const replaced = selEnd - selStart;
    const mode = cfg().get('f1Mode') === 'edit' ? 'edit' : 'annotate';
    const snapshot = await rt.engine.snapshot({uri: doc.uri.toString(), version, text, languageId: doc.languageId, selection: sel, isDirty: doc.isDirty, fileName: doc.fileName});
    const baselineValid = (await rt.engine.validateSnapshot(snapshot, info.language, info.dialect)).ok;
    const carry = opts.carry || null;
    const s = session = {
      doc, snapshot, profile: info.language, dialect: info.dialect, intent: '', mode, active: mode === 'annotate' ? (carry && carry.active) || 'annotate' : 'full', banner: null, busy: false, stale: false, applying: false, done: false,
      closed: false, reveal: false, hl: null, autostart: null,
      offsets: {start: selStart, end: selEnd}, controllers: {full: null, insert: null, annotate: null}, models: {full: '', insert: ''},
      baselineValid,
      targetText: L('f1.target', sel.start.line + 1, sel.start.character + 1, replaced ? L('f1.replaces', replaced) : L('f1.zeroLength')),
      tabs: newTabs(doc, snapshot)
    };
    s.tabs.annotate = newAnnotateTab(doc, snapshot, info, s.offsets, carry && carry.options);
    // annotate mode, but the annotate tab cannot work only because of the language (or an empty document) while AI is available: the whole-file tab can, so F1 opens on it
    if (s.active === 'annotate' && s.tabs.annotate.status === 'unavailable' && s.tabs.annotate.because && s.tabs.full.status !== 'unavailable') s.active = 'full';
    if (doc.version !== version) { s.stale = true; s.banner = {kind: 'warn', text: L('f1.stale')}; } // typed while the snapshot was being taken
    for (const [name, sc] of [['full', 'f1Full'], ['insert', 'f1Insert']]) { try { s.models[name] = settingsMod.resolveScenario(rt.settings, sc).model; } catch { /* shown on generate */ } }
    s.panel = new Panel(vscode, {onMessage: m => { const p = onMessage(m).catch(e => fatal(e)); inflight.add(p); p.finally(() => inflight.delete(p)); }, onDispose: () => { if (session === s) closeSession(true); }});
    s.panel.setTitle(L('f1.title'));
    render();
    return s;
  }
  // annotate mode, AI ready, something to add: the first request starts without a click (the consent modal still appears when it is needed)
  async function autoStart(s) {
    try {
      const t = s.tabs.annotate;
      if (s.mode !== 'annotate' || cfg().get('f1AnnotateOnOpen') === false || t.status === 'unavailable' || s.stale || !(t.options.comment || t.options.graph)) return;
      let sc; try { sc = settingsMod.resolveScenario(rt.settings, 'f1Insert'); } catch { return; }
      if (!sc.model || !await rt.ctx.secrets.get('pime-key:' + sc.providerId)) return; // not configured: the tab waits for the user
      if (session !== s) return;
      await generate(['annotate']);
    } catch (e) { fatal(e); }
  }
  // A stale/finished proposal can never be re-applied: start over from a fresh snapshot of the live buffer.
  async function reopen(tabs) {
    const old = session, intent = old.intent, editor = visibleEditorFor(old.doc) || vscode.window.activeTextEditor;
    await open(editor, {autoStart: false, carry: {active: old.active, options: old.tabs.annotate.options}});
    if (session && session !== old) { session.intent = intent; await generate(tabs); } // not when the new session was refused (a secret-class file the user declined): the old one stays stale
  }
  function fatal(e) { if (session) { session.banner = {kind: 'error', text: L('f1.error', errorText(e))}; session.busy = false; render(); } }
  function closeSession(fromPanel = false) {
    if (!session) return;
    const s = session; session = null;
    s.closed = true;
    dropHighlight(s);
    for (const c of Object.values(s.controllers)) if (c) c.abort();
    rt.coordinator.invalidate(s.snapshot.document_uri);
    rt.coordinator.purge();
    rt.engine.release(s.snapshot); // the engine forgets the document text of this session
    if (!fromPanel && s.panel) s.panel.dispose();
  }

  // the tabs one Generate runs: edit mode keeps the two requests of old, annotate mode runs only the tab on screen (one F1 never spends three requests)
  const tabsToRun = s => s.mode === 'annotate' ? [s.active] : ['full', 'insert'];
  const tabName = v => v === 'insert' ? 'insert' : v === 'annotate' ? 'annotate' : 'full';
  const proposalName = v => v === 'annotate' ? 'annotate' : 'full';
  async function onMessage(m) {
    const s = session; if (!s) return;
    switch (m.type) {
      case 'tab': s.active = tabName(m.tab); break;
      case 'generate': s.intent = String(m.intent || '').slice(0, 2000); if (s.stale || s.done) { await reopen(tabsToRun(s)); return; } await generate(tabsToRun(s)); return;
      case 'regenerate': if (s.stale || s.done) { await reopen([tabName(m.tab)]); return; } await generate([tabName(m.tab)]); return;
      case 'cancel': for (const c of Object.values(s.controllers)) if (c) c.abort(); break;
      case 'toggleHunk': { const name = proposalName(m.tab), t = s.tabs[name]; if (m.on) t.selected.add(m.id); else t.selected.delete(m.id); await recompute(s, name); break; }
      case 'scope': moveScope(s, m.move); break;
      case 'option': setOption(s, m.name, m.on); break;
      case 'openDiff': await openDiff(proposalName(m.tab)); return;
      case 'applyFull': await applyProposal('full'); return;
      case 'applyAnnotate': await applyProposal('annotate'); return;
      case 'applyInsert': await applyInsert(m.id); return;
      default: break;
    }
    render();
  }

  // ------------------------------------------------------------------ consent (FR-016/049)
  const scopeFor = tab => tab === 'full' || tab === 'annotate' ? 'L3' : 'L1';
  async function obtainGrants(s, runnable) {
    const workspace = workspaceId();
    const missing = [];
    for (const r of runnable) {
      const found = await rt.broker.find({providerId: r.grantKey, workspace, scope: r.scope});
      if (found) r.grantId = found.id; else missing.push(r);
    }
    const byHost = new Map();
    for (const r of missing) { const k = r.grantKey; byHost.set(k, [...(byHost.get(k) || []), r]); }
    for (const [grantKey, list] of byHost) {
      const host = grantKey.slice(grantKey.indexOf(':') + 1);
      const both = list.length > 1;
      const msg = both ? L('consent.both', host) : (list[0].scope === 'L3' ? L('consent.full', host) : L('consent.nearby', host));
      const choice = await vscode.window.showWarningMessage(msg, {modal: true, detail: L('consent.risk')}, L('consent.once'), L('consent.workspace'));
      if (!choice) return false;
      for (const r of list) {
        const g = await rt.broker.authorize({providerId: grantKey, workspace, scope: r.scope, once: choice === L('consent.once'), ttlMs: choice === L('consent.once') ? 10 * 60 * 1000 : 24 * 3600 * 1000});
        r.grantId = g.id;
      }
    }
    return true;
  }
  // The annotate request carries the whole file (L3) and, for the directory graph, symbol NAMES of the other files (L4): one modal says both
  // (the same wording and grants as the insertion annotation of src/annotate.js). A one-use L4 grant is consumed by this one request.
  async function obtainAnnotateGrants(s, r) {
    const grantKey = r.grantKey, workspace = workspaceId();
    const scopes = r.needDirectory ? ['L3', 'L4'] : ['L3'];
    const missing = [];
    for (const sc of scopes) {
      const found = await rt.broker.find({providerId: grantKey, workspace, scope: sc});
      if (found) { if (sc === 'L3') r.grantId = found.id; } else missing.push(sc);
    }
    if (!missing.length) return true;
    const host = grantKey.slice(grantKey.indexOf(':') + 1);
    const msg = L('f1.ann.consent', host) + (r.needDirectory && missing.includes('L4') ? `\n\n${L('annotate.consent.graph')}` : '');
    const choice = await vscode.window.showWarningMessage(msg, {modal: true, detail: L('consent.risk')}, L('consent.once'), L('consent.workspace'));
    if (!choice) return false;
    const once = choice === L('consent.once');
    for (const sc of missing) {
      const g = await rt.broker.authorize({providerId: grantKey, workspace, scope: sc, once, ttlMs: once ? 10 * 60 * 1000 : 24 * 3600 * 1000});
      if (sc === 'L3') r.grantId = g.id; else if (once) await rt.broker.consume(g.id);
    }
    return true;
  }
  // Returns {text, map, allowSensitive, redactedCount, cancelled}
  async function sensitivityGate(s) {
    const verdict = await rt.engine.privacy.assess(s.snapshot.content, {fileName: s.snapshot.file_name, scope: 'f1'});
    if (verdict.action === 'allow') return {mode: 'plain'};
    if (verdict.action === 'redact') return {mode: 'redact'};
    const kinds = Object.entries(verdict.summary).map(([k, n]) => `${k}×${n}`).join(', ') || 'sensitive file name';
    const choice = await vscode.window.showWarningMessage(L('consent.sensitive', kinds), {modal: true, detail: L('consent.risk')}, L('consent.redact'), L('consent.raw'));
    if (!choice) return {cancelled: true};
    return choice === L('consent.redact') ? {mode: 'redact'} : {mode: 'raw'};
  }

  // ------------------------------------------------------------------ generation
  async function generate(tabs) {
    const s = session; if (!s || s.busy) return;
    s.banner = null; s.stale = false; s.done = false;
    const runnable = [];
    for (const name of tabs) {
      const tab = s.tabs[name];
      if (tab.status === 'unavailable') continue;
      if (name === 'annotate' && !(tab.options.comment || tab.options.graph)) { resetAnnotate(tab); continue; } // the tab says what to enable
      const sc = settingsMod.resolveScenario(rt.settings, name === 'full' ? 'f1Full' : 'f1Insert');
      const key = await rt.ctx.secrets.get('pime-key:' + sc.providerId);
      if (!sc.model || !key) { tab.status = 'error'; tab.message = L('f1.error', L('consent.noKey')); continue; }
      runnable.push({name, sc, key, scope: scopeFor(name), grantKey: grantProviderId(sc), grantId: null, needDirectory: name === 'annotate' && tab.options.graph && tab.graphScope === 'directory'});
      s.models[name] = sc.model;
    }
    if (!runnable.length) { render(); return; }
    s.busy = true; render();
    try {
      const plain = runnable.filter(r => r.name !== 'annotate'), ann = runnable.find(r => r.name === 'annotate');
      if (plain.length && !await obtainGrants(s, plain)) { s.busy = false; render(); return; }
      if (ann && !await obtainAnnotateGrants(s, ann)) { s.busy = false; render(); return; }
      const gate = await sensitivityGate(s);
      if (gate.cancelled) { s.busy = false; render(); return; }
      for (const r of runnable) { s.tabs[r.name].status = 'loading'; s.tabs[r.name].message = ''; s.tabs[r.name].notes = []; s.tabs[r.name].grantId = r.grantId; }
      render();
      await Promise.allSettled(runnable.map(r => (r.name === 'full' ? runFull(s, r, gate) : r.name === 'annotate' ? runAnnotate(s, r, gate) : runInsert(s, r, gate)).then(render)));
    } catch (e) { s.banner = {kind: 'error', text: L('f1.error', errorText(e))}; }
    finally { s.busy = false; render(); }
  }
  // One redaction set per request: every part of the prompt that is redacted joins it, and the model's reply is restored from
  // it. The originals stay wherever the engine keeps them (the core, with the Rust engine).
  const prepareText = async (text, gate, set) => gate.mode === 'redact' ? rt.engine.privacy.redact(text, set) : text;
  const noteCommon = (tab, res, parsed, set) => {
    if (res.effort.status !== 'omitted' && res.effort.status !== 'sent') tab.notes.push(L('f1.effort', res.effort.message));
    if (parsed.ignoredKeys.length) tab.notes.push(L('f1.ignored', parsed.ignoredKeys.slice(0, 5).join(', ')));
    if (parsed.rejectedTargets) tab.notes.push(L('f1.crossFile', parsed.rejectedTargets));
    if (set.count) tab.notes.push(L('f1.redacted', set.count));
  };
  async function runFull(s, r, gate) {
    const tab = s.tabs.full, snap = s.snapshot;
    const controller = s.controllers.full = new AbortController();
    const set = rt.engine.privacy.redactionSet();
    try {
      const sentText = await prepareText(snap.content, gate, set);
      const prompt = await rt.engine.buildPrompt('full', {task: s.intent || 'suggest safe improvements around the cursor', language: s.profile, dialect: s.dialect, fileName: baseName(snap.file_name),
        cursorOffsetUtf16: s.offsets.start, selectionText: snap.content.slice(s.offsets.start, s.offsets.end).slice(0, 4000), currentFile: sentText});
      const res = await rt.ai.infer({...r.sc, providerId: r.grantKey, key: r.key, ...await withAnnotation(prompt, 'full', s), signal: controller.signal, grantId: r.grantId, grantScope: 'L3', allowSensitive: gate.mode === 'raw'});
      const parsed = await rt.engine.parseModelResult(res.text, {task: 'full', baseline: sentText, currentFile: snap.file_name});
      noteCommon(tab, res, parsed, set);
      let proposed = parsed.proposedText;
      if (proposed !== undefined && set.count) {
        const restored = await rt.engine.privacy.restore(proposed, set);
        if (restored.unresolved) throw fail('OUTPUT_INVALID', 'redaction placeholder left in the proposal');
        proposed = restored.text;
      }
      if (proposed === undefined || proposed === snap.content) { tab.status = 'ready'; tab.hunks = []; tab.selected = new Set(); tab.message = L('f1.noChange') + (parsed.explanation ? ` ${parsed.explanation}` : ''); return; }
      const prop = await rt.engine.propose(snap, proposed, {language: s.profile, dialect: s.dialect});
      tab.proposed = proposed; tab.review = prop.review; tab.hunks = prop.hunks; tab.selected = new Set(tab.hunks.map(h => h.id));
      tab.status = 'ready'; tab.message = parsed.explanation || '';
      for (const a of parsed.assumptions) tab.notes.push(a);
      await recompute(s, 'full', prop.result);
    } catch (e) {
      tab.hunks = []; tab.selected = new Set();
      if (codeOf(e) === 'AI_CANCELLED') { tab.status = 'idle'; tab.message = errorText(e); }
      else { tab.status = 'error'; tab.message = L('f1.error', errorText(e)); }
    } finally { if (s.controllers.full === controller) s.controllers.full = null; await rt.engine.privacy.release(set); }
  }
  async function runInsert(s, r, gate) {
    const tab = s.tabs.insert, snap = s.snapshot;
    const controller = s.controllers.insert = new AbortController();
    const set = rt.engine.privacy.redactionSet();
    try {
      const ex = await rt.engine.excerpt(snap, s.offsets.start, {before: 1800, after: 900});
      const before = await prepareText(ex.text.slice(0, ex.cursorInExcerpt), gate, set), after = await prepareText(ex.text.slice(ex.cursorInExcerpt), gate, set);
      const selected = await prepareText(snap.content.slice(s.offsets.start, s.offsets.end).slice(0, 4000), gate, set);
      const prompt = await rt.engine.buildPrompt('insert', {task: s.intent || 'suggest code for the cursor', language: s.profile, dialect: s.dialect, fileName: baseName(snap.file_name), before, after, selectionText: selected});
      const res = await rt.ai.infer({...r.sc, providerId: r.grantKey, key: r.key, ...await withAnnotation(prompt, 'insert', s), signal: controller.signal, grantId: r.grantId, grantScope: 'L1', allowSensitive: gate.mode === 'raw'});
      const parsed = await rt.engine.parseModelResult(res.text, {task: 'insert'});
      noteCommon(tab, res, parsed, set);
      const seen = new Set(), out = [];
      for (const raw of parsed.insertCandidates) {
        const restored = await rt.engine.privacy.restore(raw, set);
        if (restored.unresolved || seen.has(restored.text)) continue;
        seen.add(restored.text);
        out.push({id: 'c' + (out.length + 1), text: restored.text});
      }
      tab.candidates = out; tab.status = 'ready'; tab.message = out.length ? (parsed.explanation || '') : L('f1.noChange');
    } catch (e) {
      tab.candidates = [];
      if (codeOf(e) === 'AI_CANCELLED') { tab.status = 'idle'; tab.message = errorText(e); }
      else { tab.status = 'error'; tab.message = L('f1.error', errorText(e)); }
    } finally { if (s.controllers.insert === controller) s.controllers.insert = null; await rt.engine.privacy.release(set); }
  }
  // The scope (whole lines) as comments only. The request is built from the REDACTED file (one redaction set, the scope cut out by line
  // range); the comments are applied to the ORIGINAL scope text, whose line numbers are the same, and no marker is ever turned back into a
  // secret (it reads <redacted>). A reply without anything to add is "ready", an unusable reply is an error that can be retried.
  async function runAnnotate(s, r, gate) {
    const tab = s.tabs.annotate, snap = s.snapshot, e = currentScope(tab), w = tab.options;
    const controller = s.controllers.annotate = new AbortController();
    const set = rt.engine.privacy.redactionSet();
    try {
      const fileText = snap.content, scopeText = fileText.slice(e.start, e.end), startLine = e.startLine + 1;
      let promptFile = fileText, promptScope = scopeText;
      if (gate.mode === 'redact') {
        promptFile = await rt.engine.privacy.redact(fileText, set);
        const cut = lineCount(promptFile) === lineCount(fileText) ? lineSpan(promptFile, e.startLine, e.endLine) : null;
        if (!cut) throw fail('SENSITIVE_BLOCKED', 'redaction changed the number of lines: nothing was sent'); // the line numbers would no longer match
        promptScope = promptFile.slice(cut.start, cut.end);
      }
      const currentSymbols = await rt.engine.symbols.extract(s.profile, promptFile, 60);
      const directorySymbols = r.needDirectory ? await annotation.directorySymbols(s.doc) : [];
      const prompt = annotateMod.buildScopePrompt({language: s.profile, dialect: s.dialect, locale: rt.locale, comment: w.comment, graph: w.graph, fileName: baseName(snap.file_name),
        fileText: promptFile, intent: s.intent, scope: {startLine, endLine: e.endLine + 1, label: e.label, kind: e.kind, text: promptScope}, currentSymbols, directorySymbols});
      const res = await rt.ai.infer({...r.sc, providerId: r.grantKey, key: r.key, ...prompt, maxOutputTokens: Math.max(r.sc.maxOutputTokens, MIN_ANNOTATE_TOKENS), signal: controller.signal,
        grantId: r.grantId, grantScope: 'L3', allowSensitive: gate.mode === 'raw'});
      if (res.effort.status !== 'omitted' && res.effort.status !== 'sent') tab.notes.push(L('f1.effort', res.effort.message));
      if (set.count) tab.notes.push(L('f1.ann.redacted', set.count));
      const out = annotateMod.applyScopeReply(gate.mode === 'redact' ? maskMarkers(res.text) : res.text, {profile: s.profile, dialect: s.dialect, scopeText, fileText, startLine, comment: w.comment, graph: w.graph});
      const d = out.dropped || {}, other = (d.empty || 0) + (d.malformed || 0) + (d.overLimit || 0);
      if (d.invalidAnchor) tab.notes.push(L('f1.ann.skippedPosition', d.invalidAnchor));
      if (d.duplicate) tab.notes.push(L('f1.ann.skippedDuplicate', d.duplicate));
      if (other) tab.notes.push(L('f1.ann.skippedOther', other));
      if (out.notes.includes('graph-empty')) tab.notes.push(L('f1.ann.graphEmpty'));
      if (!out.ok) {
        tab.hunks = []; tab.selected = new Set();
        if (out.notes.includes('reply-invalid')) { tab.status = 'error'; tab.message = L('f1.ann.replyInvalid'); }
        else if (out.notes.includes('invariant-failed')) { tab.status = 'error'; tab.message = L('f1.ann.unsafe'); }
        else { tab.status = 'ready'; tab.message = L('f1.ann.nothingToAdd'); }
        return;
      }
      const proposed = fileText.slice(0, e.start) + fitEol(out.text, scopeText, fileText) + fileText.slice(e.end);
      const prop = await rt.engine.propose(snap, proposed, {language: s.profile, dialect: s.dialect});
      tab.proposed = proposed; tab.review = prop.review; tab.hunks = prop.hunks; tab.selected = new Set(tab.hunks.map(h => h.id));
      const kinds = [out.added.comments > 0 && L('f1.ann.commentCount', out.added.comments), out.added.graph && L('annotate.kind.graph')].filter(Boolean);
      tab.status = 'ready'; tab.message = L('f1.ann.summary', kinds.join(', '));
      await recompute(s, 'annotate', prop.result);
    } catch (err) {
      tab.hunks = []; tab.selected = new Set();
      if (codeOf(err) === 'AI_CANCELLED') { tab.status = 'idle'; tab.message = errorText(err); }
      else { tab.status = 'error'; tab.message = L('f1.error', errorText(err)); }
    } finally { if (s.controllers.annotate === controller) s.controllers.annotate = null; await rt.engine.privacy.release(set); }
  }

  // ------------------------------------------------------------------ preview and apply
  async function openDiff(name) {
    const s = session; if (!s || s.tabs[name].status !== 'ready') return;
    const t = s.tabs[name];
    const id = crypto.randomBytes(8).toString('hex');
    previews.set(id, await rt.engine.resultText(s.snapshot, t.review, [...t.selected]));
    if (previews.size > 20) previews.delete(previews.keys().next().value);
    await vscode.commands.executeCommand('vscode.diff', s.doc.uri, vscode.Uri.from({scheme: PREVIEW_SCHEME, path: `/${id}`}), `${L('f1.title')} - READ-ONLY preview`);
  }
  function stateOf(s) {
    const editor = visibleEditorFor(s.doc);
    return {uri: s.doc.uri.toString(), version: s.doc.version, text: s.doc.getText(), isClosed: s.doc.isClosed, editable: true, selection: editor ? toSel(editor.selection) : s.snapshot.selection};
  }
  const adapterFor = s => ({
    getState: () => stateOf(s),
    applyEdit: async edits => {
      const we = new vscode.WorkspaceEdit();
      for (const e of edits) we.replace(s.doc.uri, new vscode.Range(s.doc.positionAt(e.start), s.doc.positionAt(e.end)), e.text);
      return vscode.workspace.applyEdit(we);
    }
  });
  async function commit(s, kind, edits, grantId, validationOk) {
    s.applying = true; render();
    try {
      const plan = await rt.coordinator.createPlan({kind, snapshot: s.snapshot, edits, targets: [s.snapshot.document_uri], selection: s.snapshot.selection});
      const token = await rt.coordinator.prepare(plan.id, {state: stateOf(s), confirmed: true, grantId, grantOk: await rt.broker.stillValid(grantId), validationOk});
      const outcome = await rt.coordinator.apply(token, adapterFor(s));
      rt.diagnostics.record({component: 'edit', error_code: 'OK', request_type: kind});
      s.done = true; s.banner = {kind: 'info', text: L('f1.applied')};
      return outcome;
    } catch (e) {
      const code = codeOf(e);
      rt.diagnostics.record({component: 'edit', error_code: code, request_type: kind});
      if (code === 'STALE_DOCUMENT' || code === 'STALE_SELECTION') s.stale = true;
      s.banner = {kind: code === 'EDIT_UNKNOWN' ? 'error' : 'warn', text: code === 'STALE_DOCUMENT' ? L('f1.stale') : errorText(e)};
      return null;
    } finally { s.applying = false; render(); }
  }
  // the selected hunks of the whole-file tab or of the annotate tab: one WorkspaceEdit, one undo step
  async function applyProposal(name) {
    const s = session; if (!s || s.applying) return;
    const t = s.tabs[name];
    if (t.status !== 'ready' || !t.selected.size) return;
    const edits = rt.engine.hunkEdits(t.review, [...t.selected]);
    await commit(s, 'full', edits, t.grantId, t.validation.level !== 'bad');
  }
  async function applyInsert(id) {
    const s = session; if (!s || s.applying) return;
    const t = s.tabs.insert;
    const cand = t.candidates.find(c => c.id === id);
    if (!cand || t.status !== 'ready') return;
    const edits = [{start: s.offsets.start, end: s.offsets.end, text: cand.text}];
    const resulting = await rt.engine.validateSnapshot(s.snapshot, s.profile, s.dialect, edits);
    await commit(s, 'insert', edits, t.grantId, !(s.baselineValid && !resulting.ok));
  }

  // ------------------------------------------------------------------ change tracking (DD §16)
  function onDocumentChanged(e) {
    const s = session; if (!s || s.applying) return;
    if (e.document.uri.toString() !== s.snapshot.document_uri) return;
    if (e.contentChanges.length === 0) return;
    rt.coordinator.invalidate(s.snapshot.document_uri);
    s.stale = true; s.banner = {kind: 'warn', text: L('f1.stale')};
    render();
  }
  function onSelectionChanged(e) {
    const s = session; if (!s || s.applying || s.done) return;
    if (e.textEditor.document !== s.doc) return;
    const now = toSel(e.selections[0]), was = s.snapshot.selection;
    if (now.start.line !== was.start.line || now.start.character !== was.start.character || now.end.line !== was.end.line || now.end.character !== was.end.character) {
      if (!s.tabs.insert.stale) { s.tabs.insert.stale = true; s.tabs.insert.notes.push(errorText(fail('STALE_SELECTION'))); render(); }
    }
  }
  function onDocumentClosed(doc) {
    if (session && session.doc === doc) { rt.coordinator.invalidate(session.snapshot.document_uri); session.stale = true; session.banner = {kind: 'warn', text: errorText(fail('DOCUMENT_CLOSED'))}; render(); }
  }
  // a tab that was hidden and shown again is a new editor object: the highlight is put on it again
  function onVisibleEditorsChanged() { if (session) syncHighlight(session); }
  // resolves when every panel message handled so far (and the automatic first request) has finished
  async function settled() {
    for (let round = 0; round < 100; round++) {
      const waiting = [...inflight];
      if (session && session.autostart) waiting.push(session.autostart); // an automatic request that has already ended resolves at once
      await Promise.allSettled(waiting);
      if (!inflight.size) return;
    }
  }
  return {open, registerPreviewProvider, onDocumentChanged, onSelectionChanged, onDocumentClosed, onVisibleEditorsChanged, closeSession, settled, current: () => session};
}

// ------------------------------------------------------------------ wiring (real vscode and runtime), created on first use
let instance = null;
function real() {
  if (instance) return instance;
  const vscode = require('vscode');
  const runtime = require('./runtime');
  const lang = require('./languageState');
  instance = createF1({vscode, rt: runtime.rt, L: runtime.L, errorText: runtime.errorText, enabled: runtime.enabled, aiEnabled: runtime.aiEnabled, workspaceId: runtime.workspaceId,
    grantProviderId: runtime.grantProviderId, cfg: runtime.cfg, infoFor: lang.infoFor, annotation: require('./annotate'), fileGate: require('./filegate').get()});
  return instance;
}
module.exports = {
  createF1, lineCount, lineSpan, maskMarkers, fitEol, resolveTarget, MAX_SCOPE_CHARS, MAX_SCOPE_LINES,
  open: (...a) => real().open(...a), registerPreviewProvider: ctx => real().registerPreviewProvider(ctx),
  onDocumentChanged: e => real().onDocumentChanged(e), onSelectionChanged: e => real().onSelectionChanged(e), onDocumentClosed: d => real().onDocumentClosed(d),
  onVisibleEditorsChanged: () => real().onVisibleEditorsChanged(), closeSession: (...a) => real().closeSession(...a), current: () => real().current()
};
