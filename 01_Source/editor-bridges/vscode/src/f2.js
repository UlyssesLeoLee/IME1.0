'use strict';
// F2 prompt (command programmingIme.f2Prompt): the selection becomes a complete 5W1H prompt on the clipboard (docs/F2_PROMPT.md).
//  - prose selection: the user's own AI turns the text into a prompt     (scenario f1Insert, grant L1: only the selection is sent);
//  - code selection:  the user's own AI reviews the file around it       (scenario f1Full,   grant L3: up to ~400 lines of the file are sent)
//    and the result is an optimisation-plan prompt; nothing to fix -> a transient notice and the clipboard is NOT touched;
//  - AI not available (off, untrusted workspace, no key, consent declined, file over 2 MiB): the UNREVIEWED local template is copied and the
//    notice says so and why. A sensitive selection / file is never copied.
// The clipboard is overwritten only when a prompt (reviewed or template) is really copied. Pure logic: core/promptcraft.js. This file is the glue:
// availability, consent, the outbound sensitivity gate, the request, the clipboard and the notices. `vscode` and the runtime are injected
// (createF2), so the flow is tested against fakes; init() wires the real ones.
// A secret-class file (.env, keys ...) is not read at all until the user confirmed it (src/filegate.js): the check comes before the selection
// or the file text is read, and a refusal is a notice with the clipboard untouched.
const {fail, codeOf} = require('./core/errors');
const {gateFor} = require('./filegate');
const settingsMod = require('./core/settings');
const promptcraft = require('./core/promptcraft');
const {LANGUAGES} = require('./core/languages');
const {t} = require('./core/i18n');
const {TransientNotice} = require('./ui/notice');

const LIMITS = Object.freeze({selectionChars: 200000, fileBytes: 2 * 1024 * 1024, minOutputTokens: 3000});
const EOL = /\r\n|\r|\n/;
const baseName = p => String(p || '').split(/[\\/]/).pop();
const kindsOf = verdict => Object.entries(verdict.summary || {}).map(([k, n]) => `${k}×${n}`).join(', ');
// the error code each "AI path not available" reason is recorded under (diagnostics hold codes only, never text)
const WHY_CODE = Object.freeze({aiOff: 'AI_DISABLED', untrusted: 'CONSENT_REQUIRED', noKey: 'MODEL_UNSUPPORTED', declined: 'CONSENT_REQUIRED', tooLarge: 'PAYLOAD_TOO_LARGE'});

// Context keys of the F2 keybindings (same rule as F1: enabled, the chosen binding, language not excluded).
function contextKeys({enabled, binding, excluded}) {
  return {f2: !!enabled && binding === 'f2' && !excluded, shiftF2: !!enabled && binding === 'shift+f2' && !excluded};
}

// The selected text as it reads in the REDACTED file. Redaction never adds or removes a line (the caller checks), so every selected line has a
// redacted twin. The first / last line keep their column cut only while the text before the start / after the end is unchanged; otherwise the
// whole redacted line is used (more than was selected, but only text of a line that was sent anyway).
function selectionInRedacted(orig, red, a, ca, b, cb) {
  const first = red[a], last = red[b];
  const fromEnd = ca >= orig[a].length, toStart = cb === 0 && b > a; // nothing of that line is selected: its redaction does not matter
  const fromOk = fromEnd || first.startsWith(orig[a].slice(0, ca)), toOk = toStart || last.endsWith(orig[b].slice(cb));
  const from = fromEnd ? first.length : fromOk ? ca : 0, to = toStart ? 0 : toOk ? last.length - (orig[b].length - cb) : last.length;
  if (a === b) return fromOk && toOk && from <= to ? first.slice(from, to) : first;
  const lines = red.slice(a, b + 1);
  lines[0] = first.slice(from);
  lines[lines.length - 1] = last.slice(0, to);
  return lines.join('\n');
}

function createF2(env) {
  const {vscode, rt, L, errorText, enabled, aiEnabled, workspaceId, grantProviderId, infoFor, notice} = env;
  const fileGate = gateFor(env);
  let current = null;

  const show = (text, kind = 'info') => { notice.show({text, kind}); }; // always the LAST step of a flow: see the header of ui/notice.js
  const say = (key, args = [], kind = 'info') => show(L(key, ...args), kind);
  const record = (type, code = 'OK') => {
    try { rt.diagnostics.record({component: 'f2', error_code: code, request_type: type}); } catch { /* diagnostics never break the command */ }
  };

  // -> null (nothing usable selected) | {tooLarge} | the facts about the selection
  function capture(editor) {
    const doc = editor.document, sel = editor.selection;
    if (!sel || sel.isEmpty) return null;
    const text = doc.getText(sel);
    if (!text || !text.trim()) return null;
    if (text.length > LIMITS.selectionChars) return {tooLarge: text.length};
    const fileText = doc.getText();
    const a = sel.start.line, b = sel.end.line;
    return {
      doc, text, fileText, start: doc.offsetAt(sel.start), end: doc.offsetAt(sel.end), lineCount: fileText.split(EOL).length,
      range: {a, ca: sel.start.character, b, cb: sel.end.character},
      startLine: a + 1, endLine: sel.end.character === 0 && b > a ? b : b + 1, // a selection that ends at column 0 does not include that line
      fileName: doc.isUntitled ? '' : baseName(doc.fileName)
    };
  }

  // -> {why} (the AI path is not available) | {sc, key}
  async function availability(job) {
    if (!vscode.workspace.isTrusted) return {why: 'untrusted'};
    if (!aiEnabled()) return {why: 'aiOff'};
    if (job.kind === 'code' && Buffer.byteLength(job.fileText, 'utf8') > LIMITS.fileBytes) return {why: 'tooLarge'};
    let sc;
    try { sc = settingsMod.resolveScenario(rt.settings, job.kind === 'code' ? 'f1Full' : 'f1Insert'); } catch { return {why: 'noKey'}; }
    const key = sc.model ? await rt.ctx.secrets.get('pime-key:' + sc.providerId) : null;
    return sc.model && key ? {sc, key} : {why: 'noKey'};
  }

  // Never silent: a reusable grant, or the same modal as F1. null = declined.
  async function consent(sc, scope) {
    const grantKey = grantProviderId(sc), workspace = workspaceId();
    const found = await rt.broker.find({providerId: grantKey, workspace, scope});
    if (found) return {grantKey, grantId: found.id};
    const host = grantKey.slice(grantKey.indexOf(':') + 1);
    const choice = await vscode.window.showWarningMessage(L(scope === 'L3' ? 'consent.full' : 'consent.nearby', host), {modal: true, detail: L('consent.risk')}, L('consent.once'), L('consent.workspace'));
    if (!choice) return null;
    const once = choice === L('consent.once');
    const g = await rt.broker.authorize({providerId: grantKey, workspace, scope, once, ttlMs: once ? 10 * 60 * 1000 : 24 * 3600 * 1000});
    return {grantKey, grantId: g.id};
  }

  // The outbound sensitivity gate on exactly the text that will leave. -> {mode: 'plain'|'redact'|'raw'} | {blocked: kinds}
  // ask=false (the offline template: nothing leaves, but the clipboard is still not a place for secrets) never opens a dialog.
  async function gate(text, fileName, ask) {
    const verdict = await rt.engine.privacy.assess(text, {fileName, scope: 'f1'});
    if (verdict.action === 'allow') return {mode: 'plain'};
    if (verdict.action === 'redact') return {mode: 'redact'};
    const kinds = kindsOf(verdict) || L('f2.sensitiveName');
    if (!ask || verdict.action === 'block') return {blocked: kinds};
    const choice = await vscode.window.showWarningMessage(L('consent.sensitive', kinds), {modal: true, detail: L('consent.risk')}, L('consent.redact'), L('consent.raw'));
    if (!choice) return {blocked: kinds}; // closing the dialog is "no": nothing is sent and nothing is copied
    return {mode: choice === L('consent.redact') ? 'redact' : 'raw'};
  }

  const buildRequest = (job, fileText, selectionText) => promptcraft.buildReviewRequest({
    kind: job.kind, locale: job.locale, language: job.language, dialect: job.dialect, fileName: job.fileName, fileText: job.kind === 'code' ? fileText : '',
    selection: {text: selectionText, startLine: job.startLine, endLine: job.endLine}, intent: '', scopeLabel: ''
  });
  const factsFor = (job, redacted, request) => {
    let truncated = false;
    try { truncated = !!(JSON.parse(request.input).window || {}).truncated; } catch { /* no window: not truncated */ }
    // the prompt says in its own language that the material was redacted
    return {fileName: job.fileName, language: job.language, startLine: job.startLine, endLine: job.endLine, lineCount: job.lineCount, truncated,
      scopeLabel: redacted ? t(job.locale, 'f2.redacted', redacted) : ''};
  };

  // ONE redact call per set: both engines number the markers from 1 on every call, so two calls would reuse PIME_REDACTED_1 for different values.
  // The selection is cut out of the redacted file; prose has no file part. -> {fileText?, selection}
  async function redactAll(job, set) {
    const {c, kind} = job;
    if (kind !== 'code') return {selection: await rt.engine.privacy.redact(c.text, set)};
    const red = await rt.engine.privacy.redact(c.fileText, set);
    const o = c.fileText.split(EOL), r = red.split(EOL);
    if (o.length !== r.length) throw fail('SENSITIVE_BLOCKED', 'redaction changed the number of lines: nothing was sent'); // line numbers would no longer match
    return {fileText: red, selection: selectionInRedacted(o, r, c.range.a, c.range.ca, c.range.b, c.range.cb)};
  }

  // In redact mode the prompt keeps the markers everywhere (the clipboard goes to another AI service, and the evidence quotes of a secret-bearing
  // line contain the marker: restoring them would hand the secret over). restore() is used as a check only: a field or finding that names a
  // marker which is not in the set was made up by the model and is dropped.
  async function keepResolvable(parsed, set) {
    const fields = {...parsed.fields}, findings = parsed.findings;
    if (!set.count) return {fields, findings};
    const bad = async s => !!s && (await rt.engine.privacy.restore(s, set)).unresolved > 0;
    const textOf = f => [f.issue, f.impact, f.evidence].filter(Boolean).join('\n');
    if (!await bad([...Object.values(fields), ...findings.map(textOf)].filter(Boolean).join('\n'))) return {fields, findings};
    for (const k of Object.keys(fields)) if (await bad(fields[k])) fields[k] = '';
    const kept = [];
    for (const f of findings) if (!await bad(textOf(f))) kept.push(f);
    return {fields, findings: kept};
  }

  async function copy(prompt, locale) {
    const check = promptcraft.verify5W1H(prompt, locale);
    if (!check.ok) throw fail('OUTPUT_INVALID', `the prompt lacks ${[...check.missing, ...check.empty].join(', ')}`); // nothing is copied
    await vscode.env.clipboard.writeText(prompt);
  }

  // ---------------------------------------------------------------- AI path
  async function aiFlow(job, avail, grant) {
    const {c, kind, locale, live} = job;
    const scope = kind === 'code' ? 'L3' : 'L1';
    const first = buildRequest(job, c.fileText, c.text);
    const sent = JSON.parse(first.input); // the gate looks at what the request really carries: the file window and the (cut) selection
    // (the selection is normally part of the window: it is then not counted twice, the window already covers the same bytes)
    const payload = kind !== 'code' ? sent.selection.text : sent.file.includes(sent.selection.text) ? sent.file : `${sent.file}\n${sent.selection.text}`;
    const g = await gate(payload, c.doc.fileName, true);
    live();
    if (g.blocked) { say('f2.sensitiveBlocked', [g.blocked], 'warn'); record('f2.blocked', 'SENSITIVE_BLOCKED'); return; }
    const set = rt.engine.privacy.redactionSet();
    try {
      let request = first, fileSent = c.fileText, selectionSent = c.text;
      if (g.mode === 'redact') {
        const red = await redactAll(job, set);
        fileSent = red.fileText; selectionSent = red.selection;
        request = buildRequest(job, fileSent, selectionSent);
      }
      live();
      const res = await vscode.window.withProgress({location: vscode.ProgressLocation.Notification, title: L('f2.progress'), cancellable: true}, (_p, token) => {
        token.onCancellationRequested(() => job.controller.abort());
        return rt.ai.infer({...avail.sc, providerId: grant.grantKey, key: avail.key, ...request, maxOutputTokens: Math.max(avail.sc.maxOutputTokens, LIMITS.minOutputTokens),
          signal: job.controller.signal, grantId: grant.grantId, grantScope: scope, allowSensitive: g.mode === 'raw'});
      });
      live();
      const facts = factsFor(job, set.count, request);
      // the reply is checked against exactly the file text the model saw (the redacted one in redact mode)
      const parsed = promptcraft.applyReviewReply(res.text, {kind, selectionText: selectionSent, fileText: fileSent, facts, locale});
      if (!parsed.ok) { vscode.window.showWarningMessage(L('f2.replyInvalid')); record(`f2.${kind}`, 'OUTPUT_INVALID'); return; } // an error, never "nothing to optimise"
      const unverified = () => { say('f2.unverified', [], 'warn'); record(`f2.${kind}.unverified`, 'OUTPUT_INVALID'); };
      if (!parsed.needsOptimization) {
        if (parsed.notes.includes('findings-unverified')) unverified();
        else { say(kind === 'code' ? 'f2.noOptCode' : 'f2.noOptProse'); record(`f2.${kind}.noopt`); } // the clipboard stays as it was
        return;
      }
      const kept = await keepResolvable(parsed, set);
      live();
      if (kind === 'code' && !kept.findings.length) { unverified(); return; }
      const prompt = promptcraft.renderPrompt({kind, fields: kept.fields, facts, selectionText: selectionSent, findings: kept.findings, locale, truncated: facts.truncated});
      live(); // a newer F2 must not be overwritten by this older answer
      await copy(prompt, locale);
      record(`f2.${kind}`);
      if (job.controller.signal.aborted) return;
      show(L('f2.copied', prompt.length) + (set.count ? ` ${L('f2.redacted', set.count)}.` : ''));
    } finally { await rt.engine.privacy.release(set); }
  }

  // ---------------------------------------------------------------- offline template
  async function templateFlow(job, why) {
    const {c, kind, locale, live} = job;
    const g = await gate(c.text, c.doc.fileName, false);
    live();
    if (g.blocked) { say('f2.sensitiveBlocked', [g.blocked], 'warn'); record('f2.blocked', 'SENSITIVE_BLOCKED'); return; }
    const set = rt.engine.privacy.redactionSet();
    try {
      const selection = g.mode === 'redact' ? await rt.engine.privacy.redact(c.text, set) : c.text;
      live();
      const facts = {fileName: job.fileName, language: job.language, startLine: job.startLine, endLine: job.endLine, lineCount: job.lineCount, truncated: false,
        scopeLabel: set.count ? t(locale, 'f2.redacted', set.count) : ''};
      const prompt = promptcraft.localPrompt({kind, selectionText: selection, facts, locale});
      await copy(prompt, locale);
      record('f2.template', WHY_CODE[why]);
      if (job.controller.signal.aborted) return;
      show(L('f2.template', prompt.length, L(`f2.why.${why}`)) + (set.count ? ` ${L('f2.redacted', set.count)}.` : ''), 'warn');
    } finally { await rt.engine.privacy.release(set); }
  }

  // ---------------------------------------------------------------- command
  async function run() {
    let kind = 'run';
    const mine = {controller: new AbortController()};
    try {
      notice.dismiss('api'); // an older notice must not outlive the next F2
      if (!enabled()) { vscode.window.showWarningMessage(L('f1.disabledHere')); return; }
      const editor = vscode.window.activeTextEditor;
      if (!editor || !editor.selection || editor.selection.isEmpty) { say('f2.selectionRequired'); return; } // from the palette; the F2 key only reaches us with a selection (when clause)
      // an explicit command: a secret-class file may be read only after the user confirmed it; nothing of the document is read before that
      if (!await fileGate.ensure(editor.document, {interactive: true, purpose: 'f2'})) { say('sec.refused.f2'); return; }
      const c = capture(editor);
      if (!c) { say('f2.selectionRequired'); return; }
      if (c.tooLarge) { say('f2.selectionTooLarge', [c.tooLarge, LIMITS.selectionChars], 'warn'); return; }
      if (current) current.controller.abort(); // one request at a time: the newest F2 wins, the older one ends silently
      current = mine;
      const live = () => { if (mine.controller.signal.aborted) throw fail('AI_CANCELLED', 'superseded or cancelled'); };
      const info = infoFor(c.doc);
      const profile = LANGUAGES.includes(info.language) ? info.language : '';
      const cls = promptcraft.classifySelection({profile, languageId: c.doc.languageId, fileName: c.doc.fileName, text: c.text, fileText: c.fileText, start: c.start, end: c.end});
      kind = cls.kind === 'code' ? 'code' : 'prose';
      const job = {...c, c, kind, live, controller: mine.controller, language: profile || c.doc.languageId || '', dialect: profile ? (info.dialect || {}) : {},
        locale: kind === 'code' ? rt.locale : promptcraft.detectTextLocale(c.text)};
      const avail = await availability(job);
      live();
      if (avail.why) { await templateFlow(job, avail.why); return; }
      const grant = await consent(avail.sc, kind === 'code' ? 'L3' : 'L1');
      live();
      if (!grant) { await templateFlow(job, 'declined'); return; } // declining is a decision, not an error
      await aiFlow(job, avail, grant);
    } catch (e) {
      const code = codeOf(e);
      record(`f2.${kind}`, code);
      if (code === 'AI_CANCELLED') return; // Cancel on the progress, or a newer F2: silent, the clipboard was not touched
      vscode.window.showErrorMessage(L('f2.error', code === 'INTERNAL' ? code : errorText(e))); // once; INTERNAL texts are not shown (they may quote content)
    } finally { if (current === mine) current = null; }
  }

  return {run, cancel: () => { if (current) current.controller.abort(); }, get busy() { return current !== null; }};
}

// ---------------------------------------------------------------- wiring (real vscode and runtime)
let instance = null;
function init(ctx) {
  const vscode = require('vscode');
  const runtime = require('./runtime');
  const lang = require('./languageState');
  const notice = new TransientNotice(vscode);
  const f2 = instance = createF2({vscode, rt: runtime.rt, L: runtime.L, errorText: runtime.errorText, enabled: runtime.enabled, aiEnabled: runtime.aiEnabled,
    workspaceId: runtime.workspaceId, grantProviderId: runtime.grantProviderId, infoFor: lang.infoFor, notice, fileGate: require('./filegate').get()});
  ctx.subscriptions.push(notice, {dispose: () => f2.cancel()}); // the notice must be disposed with the extension (see ui/notice.js)
}
const run = () => instance ? instance.run() : Promise.resolve();

module.exports = {init, run, createF2, contextKeys, selectionInRedacted, LIMITS};
