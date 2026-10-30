'use strict';
// F1 workflow (FR-031..040, DD §9): two independent paths - whole-file change and insert-at-cursor - each with its
// own request, grant, state and error. Nothing is written until the user presses Apply; Apply goes through the
// EditCoordinator (version+hash re-check, one-use token, read-back) as a single WorkspaceEdit.
const vscode = require('vscode');
const crypto = require('node:crypto');
const {rt, L, errorText, enabled, aiEnabled, workspaceId, grantProviderId} = require('./runtime');
const {fail, codeOf} = require('./core/errors');
const ctxMod = require('./core/context');
const diffMod = require('./core/diff');
const secrets = require('./core/secrets');
const aiMod = require('./core/ai');
const settingsMod = require('./core/settings');
const {validateSource} = require('./core/validators');
const {applyEdits} = require('./core/textrange');
const {F1Panel, I18N_KEYS} = require('./ui/f1panel');
const {bundle} = require('./core/i18n');
const lang = require('./languageState');

const PREVIEW_SCHEME = 'pime-preview';
const MAX_FULL_BYTES = 2 * 1024 * 1024;
const previews = new Map();
let session = null;

const toSel = s => ({start: {line: s.start.line, character: s.start.character}, end: {line: s.end.line, character: s.end.character}});
const short = h => h.slice(0, 12);
const visibleEditorFor = doc => vscode.window.visibleTextEditors.find(e => e.document === doc);
const baseName = p => String(p || '').split(/[\\/]/).pop();

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
  const full = {status: 'idle', message: '', notes: [], hunks: [], selected: new Set(), proposed: '', grantId: null, validation: {level: 'none', text: '', issues: []}};
  const insert = {status: 'idle', message: '', notes: [], candidates: [], grantId: null, stale: false};
  for (const [name, tab] of [['full', full], ['insert', insert]]) {
    const reason = unavailableReason(doc, snapshot, name);
    if (reason) { tab.status = 'unavailable'; tab.message = name === 'full' ? L('f1.unavailable.full', reason) : reason; }
  }
  return {full, insert};
}

// ------------------------------------------------------------------ state for the webview
function resultText(s) { const t = s.tabs.full; return diffMod.applyHunks(s.snapshot.content, t.hunks, [...t.selected]); }
function recomputeFull(s) {
  const t = s.tabs.full;
  const out = resultText(s);
  const after = validateSource(s.profile, out, s.dialect);
  t.resultText = out; t.resultHash = ctxMod.sha256(out);
  if (after.ok) t.validation = {level: 'ok', text: L('f1.validation.ok'), issues: []};
  else if (s.baselineValid) t.validation = {level: 'bad', text: L('f1.validation.bad'), issues: after.issues.slice(0, 5).map(i => `${i.line ? 'L' + i.line + ': ' : ''}${i.message}`)};
  else t.validation = {level: 'warn', text: L('f1.validation.warn'), issues: after.issues.slice(0, 3).map(i => `${i.line ? 'L' + i.line + ': ' : ''}${i.message}`)};
}
function viewState() {
  const s = session, snap = s.snapshot, I = bundle(rt.locale, I18N_KEYS);
  const full = s.tabs.full, ins = s.tabs.insert;
  const canActFull = full.status === 'ready' && full.selected.size > 0 && full.validation.level !== 'bad' && !s.stale && !s.busy && !s.done;
  const canActIns = ins.status === 'ready' && !s.stale && !ins.stale && !s.busy && !s.done;
  const budget = rt.budget.snapshot();
  return {
    i18n: I, lang: rt.locale, active: s.active, busy: s.busy, stale: s.stale, intent: s.intent, canGenerate: !s.busy && !s.done && (full.status !== 'unavailable' || ins.status !== 'unavailable'),
    metaText: `${s.profile} | ${baseName(snap.file_name) || snap.document_uri} | v${snap.document_version} | ${snap.eol} | ${s.models.full || '-'} / ${s.models.insert || '-'}`,
    banner: s.banner,
    usageText: L('f1.usage', budget.requests, budget.tokens, budget.estimated ? ' (estimated)' : ''),
    tabs: {
      full: {
        status: full.status, message: full.message, notes: full.notes,
        scopeText: L('f1.scope.full', baseName(snap.file_name), snap.document_version, short(snap.sha256)),
        hunksText: L('f1.hunks', full.hunks.length),
        hunks: full.hunks.map((h, i) => ({...diffMod.renderHunk(snap.content, h), title: L('f1.hunk', i + 1, h.oldStartLine, h.oldLineCount, h.newLineCount), selected: full.selected.has(h.id)})),
        preview: {title: L('f1.preview', (full.resultText || '').length, short(full.resultHash || '')), text: (full.resultText || '').slice(0, 4000) + ((full.resultText || '').length > 4000 ? '\n…' : '')},
        validation: full.validation, canApply: canActFull
      },
      insert: {
        status: ins.status, message: ins.message, notes: ins.notes,
        scopeText: L('f1.scope.insert', `${snap.selection.start.line + 1}:${snap.selection.start.character + 1}`),
        targetText: s.targetText, candidates: ins.candidates.map(c => ({id: c.id, text: c.text})), canApply: canActIns
      }
    }
  };
}
const render = () => { if (session && session.panel) session.panel.update(viewState()); };

// ------------------------------------------------------------------ opening
async function open(editorOverride) {
  if (!enabled()) { vscode.window.showWarningMessage(L('f1.disabledHere')); return; }
  const editor = editorOverride || vscode.window.activeTextEditor;
  if (!editor) { vscode.window.showInformationMessage(L('f1.noEditor')); return; }
  closeSession();
  const doc = editor.document;
  const text = doc.getText();
  const sel = toSel(editor.selection);
  const snapshot = ctxMod.makeSnapshot({uri: doc.uri.toString(), version: doc.version, text, languageId: doc.languageId, selection: sel, isDirty: doc.isDirty, fileName: doc.fileName});
  const info = lang.infoFor(doc);
  const selStart = doc.offsetAt(editor.selection.start), selEnd = doc.offsetAt(editor.selection.end);
  const replaced = selEnd - selStart;
  const s = session = {
    doc, snapshot, profile: info.language, dialect: info.dialect, intent: '', active: 'full', banner: null, busy: false, stale: false, applying: false, done: false,
    offsets: {start: selStart, end: selEnd}, controllers: {full: null, insert: null}, models: {full: '', insert: ''},
    baselineValid: validateSource(info.language, text, info.dialect).ok,
    targetText: L('f1.target', sel.start.line + 1, sel.start.character + 1, replaced ? L('f1.replaces', replaced) : L('f1.zeroLength')),
    tabs: newTabs(doc, snapshot)
  };
  for (const [name, sc] of [['full', 'f1Full'], ['insert', 'f1Insert']]) { try { s.models[name] = settingsMod.resolveScenario(rt.settings, sc).model; } catch { /* shown on generate */ } }
  s.panel = new F1Panel(vscode, {onMessage: m => onMessage(m).catch(e => fatal(e)), onDispose: () => { if (session === s) closeSession(true); }});
  s.panel.setTitle(L('f1.title'));
  render();
}
// A stale/finished proposal can never be re-applied: start over from a fresh snapshot of the live buffer.
async function reopen(tabs) {
  const old = session, intent = old.intent, editor = visibleEditorFor(old.doc) || vscode.window.activeTextEditor;
  await open(editor);
  if (session) { session.intent = intent; await generate(tabs); }
}
function fatal(e) { if (session) { session.banner = {kind: 'error', text: L('f1.error', errorText(e))}; session.busy = false; render(); } }
function closeSession(fromPanel = false) {
  if (!session) return;
  const s = session; session = null;
  for (const c of Object.values(s.controllers)) if (c) c.abort();
  rt.coordinator.invalidate(s.snapshot.document_uri);
  rt.coordinator.purge();
  if (!fromPanel && s.panel) s.panel.dispose();
}

async function onMessage(m) {
  const s = session; if (!s) return;
  switch (m.type) {
    case 'tab': s.active = m.tab === 'insert' ? 'insert' : 'full'; break;
    case 'generate': s.intent = String(m.intent || '').slice(0, 2000); if (s.stale || s.done) { await reopen(['full', 'insert']); return; } await generate(['full', 'insert']); return;
    case 'regenerate': if (s.stale || s.done) { await reopen([m.tab === 'insert' ? 'insert' : 'full']); return; } await generate([m.tab === 'insert' ? 'insert' : 'full']); return;
    case 'cancel': for (const c of Object.values(s.controllers)) if (c) c.abort(); break;
    case 'toggleHunk': { const t = s.tabs.full; if (m.on) t.selected.add(m.id); else t.selected.delete(m.id); recomputeFull(s); break; }
    case 'openDiff': await openDiff(); return;
    case 'applyFull': await applyFull(); return;
    case 'applyInsert': await applyInsert(m.id); return;
    default: break;
  }
  render();
}

// ------------------------------------------------------------------ consent (FR-016/049)
function scopeFor(tab) { return tab === 'full' ? 'L3' : 'L1'; }
async function obtainGrants(s, runnable) {
  const workspace = workspaceId();
  const missing = [];
  for (const r of runnable) {
    const found = rt.broker.find({providerId: r.grantKey, workspace, scope: r.scope});
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
      const g = rt.broker.authorize({providerId: grantKey, workspace, scope: r.scope, once: choice === L('consent.once'), ttlMs: choice === L('consent.once') ? 10 * 60 * 1000 : 24 * 3600 * 1000});
      r.grantId = g.id;
    }
  }
  return true;
}
// Returns {text, map, allowSensitive, redactedCount, cancelled}
async function sensitivityGate(s) {
  const verdict = secrets.assessOutbound(s.snapshot.content, {fileName: s.snapshot.file_name, scope: 'f1'});
  if (verdict.action === 'allow') return {mode: 'plain'};
  if (verdict.action === 'redact') return {mode: 'redact'};
  const kinds = Object.entries(secrets.summarize(verdict.scanned.findings)).map(([k, n]) => `${k}×${n}`).join(', ') || 'sensitive file name';
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
    const sc = settingsMod.resolveScenario(rt.settings, name === 'full' ? 'f1Full' : 'f1Insert');
    const key = await rt.ctx.secrets.get('pime-key:' + sc.providerId);
    if (!sc.model || !key) { tab.status = 'error'; tab.message = L('f1.error', L('consent.noKey')); continue; }
    runnable.push({name, sc, key, scope: scopeFor(name), grantKey: grantProviderId(sc), grantId: null});
    s.models[name] = sc.model;
  }
  if (!runnable.length) { render(); return; }
  s.busy = true; render();
  try {
    if (!await obtainGrants(s, runnable)) { s.busy = false; render(); return; }
    const gate = await sensitivityGate(s);
    if (gate.cancelled) { s.busy = false; render(); return; }
    for (const r of runnable) { s.tabs[r.name].status = 'loading'; s.tabs[r.name].message = ''; s.tabs[r.name].notes = []; s.tabs[r.name].grantId = r.grantId; }
    render();
    await Promise.allSettled(runnable.map(r => (r.name === 'full' ? runFull(s, r, gate) : runInsert(s, r, gate)).then(render)));
  } catch (e) { s.banner = {kind: 'error', text: L('f1.error', errorText(e))}; }
  finally { s.busy = false; render(); }
}
function prepareText(text, gate) {
  if (gate.mode === 'redact') { const sc = secrets.scan(text, {fileName: ''}); const red = secrets.redact(text, sc.findings); return {text: red.text, map: red.map}; }
  return {text, map: []};
}
const noteCommon = (tab, res, parsed, map) => {
  if (res.effort.status !== 'omitted' && res.effort.status !== 'sent') tab.notes.push(L('f1.effort', res.effort.message));
  if (parsed.ignoredKeys.length) tab.notes.push(L('f1.ignored', parsed.ignoredKeys.slice(0, 5).join(', ')));
  if (parsed.rejectedTargets) tab.notes.push(L('f1.crossFile', parsed.rejectedTargets));
  if (map.length) tab.notes.push(L('f1.redacted', map.length));
};
async function runFull(s, r, gate) {
  const tab = s.tabs.full, snap = s.snapshot;
  const controller = s.controllers.full = new AbortController();
  try {
    const sent = prepareText(snap.content, gate);
    const prompt = aiMod.buildPrompt('full', {task: s.intent || 'suggest safe improvements around the cursor', language: s.profile, dialect: s.dialect, fileName: baseName(snap.file_name),
      cursorOffsetUtf16: s.offsets.start, selectionText: snap.content.slice(s.offsets.start, s.offsets.end).slice(0, 4000), currentFile: sent.text});
    const res = await rt.ai.infer({...r.sc, providerId: r.grantKey, key: r.key, ...prompt, signal: controller.signal, grantId: r.grantId, grantScope: 'L3', allowSensitive: gate.mode === 'raw'});
    const parsed = aiMod.parseModelResult(res.text, {task: 'full', baseline: sent.text, currentFile: snap.file_name});
    noteCommon(tab, res, parsed, sent.map);
    let proposed = parsed.proposedText;
    if (proposed !== undefined && sent.map.length) {
      const restored = secrets.restore(proposed, sent.map);
      if (restored.unresolved) throw fail('OUTPUT_INVALID', 'redaction placeholder left in the proposal');
      proposed = restored.text;
    }
    if (proposed === undefined || proposed === snap.content) { tab.status = 'ready'; tab.hunks = []; tab.selected = new Set(); tab.message = L('f1.noChange') + (parsed.explanation ? ` ${parsed.explanation}` : ''); return; }
    tab.proposed = proposed; tab.hunks = diffMod.buildHunks(snap.content, proposed); tab.selected = new Set(tab.hunks.map(h => h.id));
    tab.status = 'ready'; tab.message = parsed.explanation || '';
    for (const a of parsed.assumptions) tab.notes.push(a);
    recomputeFull(s);
  } catch (e) {
    tab.hunks = []; tab.selected = new Set();
    if (codeOf(e) === 'AI_CANCELLED') { tab.status = 'idle'; tab.message = errorText(e); }
    else { tab.status = 'error'; tab.message = L('f1.error', errorText(e)); }
  } finally { if (s.controllers.full === controller) s.controllers.full = null; }
}
async function runInsert(s, r, gate) {
  const tab = s.tabs.insert, snap = s.snapshot;
  const controller = s.controllers.insert = new AbortController();
  try {
    const ex = ctxMod.excerpt(snap.content, s.offsets.start, {before: 1800, after: 900});
    const before = prepareText(ex.text.slice(0, ex.cursorInExcerpt), gate), after = prepareText(ex.text.slice(ex.cursorInExcerpt), gate);
    const selected = prepareText(snap.content.slice(s.offsets.start, s.offsets.end).slice(0, 4000), gate);
    const map = [...before.map, ...after.map, ...selected.map];
    const prompt = aiMod.buildPrompt('insert', {task: s.intent || 'suggest code for the cursor', language: s.profile, dialect: s.dialect, fileName: baseName(snap.file_name), before: before.text, after: after.text, selectionText: selected.text});
    const res = await rt.ai.infer({...r.sc, providerId: r.grantKey, key: r.key, ...prompt, signal: controller.signal, grantId: r.grantId, grantScope: 'L1', allowSensitive: gate.mode === 'raw'});
    const parsed = aiMod.parseModelResult(res.text, {task: 'insert'});
    noteCommon(tab, res, parsed, map);
    const seen = new Set(), out = [];
    for (const raw of parsed.insertCandidates) {
      const restored = map.length ? secrets.restore(raw, map) : {text: raw, unresolved: 0};
      if (restored.unresolved || seen.has(restored.text)) continue;
      seen.add(restored.text);
      out.push({id: 'c' + (out.length + 1), text: restored.text});
    }
    tab.candidates = out; tab.status = 'ready'; tab.message = out.length ? (parsed.explanation || '') : L('f1.noChange');
  } catch (e) {
    tab.candidates = [];
    if (codeOf(e) === 'AI_CANCELLED') { tab.status = 'idle'; tab.message = errorText(e); }
    else { tab.status = 'error'; tab.message = L('f1.error', errorText(e)); }
  } finally { if (s.controllers.insert === controller) s.controllers.insert = null; }
}

// ------------------------------------------------------------------ preview and apply
async function openDiff() {
  const s = session; if (!s || s.tabs.full.status !== 'ready') return;
  const id = crypto.randomBytes(8).toString('hex');
  previews.set(id, resultText(s));
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
    const plan = rt.coordinator.createPlan({kind, snapshot: s.snapshot, edits, targets: [s.snapshot.document_uri], selection: s.snapshot.selection});
    const token = rt.coordinator.prepare(plan.id, {state: stateOf(s), confirmed: true, grantOk: rt.broker.stillValid(grantId), validationOk});
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
async function applyFull() {
  const s = session; if (!s || s.applying) return;
  const t = s.tabs.full;
  if (t.status !== 'ready' || !t.selected.size) return;
  const edits = diffMod.hunkEdits(t.hunks, [...t.selected]);
  await commit(s, 'full', edits, t.grantId, t.validation.level !== 'bad');
}
async function applyInsert(id) {
  const s = session; if (!s || s.applying) return;
  const t = s.tabs.insert;
  const cand = t.candidates.find(c => c.id === id);
  if (!cand || t.status !== 'ready') return;
  const edits = [{start: s.offsets.start, end: s.offsets.end, text: cand.text}];
  const resulting = validateSource(s.profile, applyEdits(s.snapshot.content, edits), s.dialect);
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
module.exports = {open, registerPreviewProvider, onDocumentChanged, onSelectionChanged, onDocumentClosed, closeSession, current: () => session};
