'use strict';
// Annotation flow for code the user inserts from the candidate list (settings autoComment / graphComment):
// ask the user's own AI - which sees the whole file - to add detailed comments and/or a Cypher node/edge comment.
// It never blocks plain insertion: on any problem (AI off, no key, refused consent, sensitive file, error, cancel,
// rejected reply) the snippet is inserted unchanged. Only comments can be added: see core/annotate.js.
const vscode = require('vscode');
const path = require('node:path');
const {rt, cfg, L, errorText, aiEnabled, workspaceId, grantProviderId} = require('./runtime');
const annotate = require('./core/annotate');
const {extractSymbols} = require('./core/symbols');
const secrets = require('./core/secrets');
const settingsMod = require('./core/settings');
const languages = require('./core/languages');
const {codeOf} = require('./core/errors');

const MAX_FILE_BYTES = 1.5 * 1024 * 1024;
const MAX_DIR_FILES = 30, MAX_DIR_FILE_BYTES = 400 * 1024, MAX_DIR_SYMBOLS = 300;

const wanted = () => ({comment: cfg().get('autoComment') !== false, graph: cfg().get('graphComment') === true, scope: cfg().get('graphCommentScope') === 'file' ? 'file' : 'directory'});
async function readySettings(profile) {
  const w = wanted();
  if (!(w.comment || w.graph) || !aiEnabled() || !vscode.workspace.isTrusted || !annotate.supportsAnnotation(profile)) return null;
  let sc; try { sc = settingsMod.resolveScenario(rt.settings, 'f1Insert'); } catch { return null; }
  const key = sc.model ? await rt.ctx.secrets.get('pime-key:' + sc.providerId) : null;
  return key ? {w, sc, key} : null;
}

// Names of declarations in the other files of the same folder. Only {file, symbols:[{kind,name}]} is ever returned.
async function directorySymbols(doc) {
  if (doc.uri.scheme !== 'file') return [];
  const dir = vscode.Uri.file(path.dirname(doc.uri.fsPath)), out = [];
  let total = 0;
  let entries = [];
  try { entries = await vscode.workspace.fs.readDirectory(dir); } catch { return []; }
  for (const [name, type] of entries) {
    if (out.length >= MAX_DIR_FILES || total >= MAX_DIR_SYMBOLS) break;
    if (type !== vscode.FileType.File || name === path.basename(doc.uri.fsPath) || secrets.isSensitiveFileName(name)) continue;
    const decision = languages.resolveLanguage({languageId: '', fileName: name});
    if (decision.reason !== 'extension' && decision.reason !== 'bridge-languageId') continue;
    try {
      const uri = vscode.Uri.joinPath(dir, name);
      const open = vscode.workspace.textDocuments.find(d => d.uri.toString() === uri.toString());
      let text;
      if (open) text = open.getText();
      else { const stat = await vscode.workspace.fs.stat(uri); if (stat.size > MAX_DIR_FILE_BYTES) continue; text = Buffer.from(await vscode.workspace.fs.readFile(uri)).toString('utf8'); }
      const symbols = extractSymbols(decision.language, text, 40);
      if (symbols.length) { out.push({file: name, symbols}); total += symbols.length; }
    } catch { /* unreadable file: skipped */ }
  }
  return out;
}

// -> {text, kinds:[...]} or null (= insert the snippet unchanged)
async function annotateInsertion({doc, info, template, position}) {
  const ready = await readySettings(info.language);
  if (!ready) return null;
  const {w, sc, key} = ready;
  const fileText = doc.getText();
  if (Buffer.byteLength(fileText, 'utf8') > MAX_FILE_BYTES) return null;
  const verdict = secrets.assessOutbound(fileText, {fileName: doc.fileName, scope: 'f1'});
  if (verdict.action === 'confirm' || verdict.action === 'block') {
    vscode.window.showWarningMessage(L('annotate.skipSensitive', Object.keys(secrets.summarize(verdict.scanned.findings)).join(', ') || 'file name'));
    return null;
  }
  const sent = verdict.action === 'redact' ? secrets.redact(fileText, verdict.scanned.findings).text : fileText;
  const needDirectory = w.graph && w.scope === 'directory';
  // authorisation: the whole file (L3) and, for the graph, symbol names of other files (L4); never silent
  const grantKey = grantProviderId(sc), workspace = workspaceId();
  const scopes = needDirectory ? ['L3', 'L4'] : ['L3'];
  const missing = [];
  for (const s of scopes) if (!await rt.broker.find({providerId: grantKey, workspace, scope: s})) missing.push(s); // the broker may answer asynchronously
  let onceGrant = null;
  if (missing.length) {
    const host = grantKey.slice(grantKey.indexOf(':') + 1);
    const message = L('annotate.consent.file', host) + (needDirectory ? `\n\n${L('annotate.consent.graph')}` : '');
    const choice = await vscode.window.showWarningMessage(message, {modal: true, detail: L('consent.risk')}, L('consent.once'), L('consent.workspace'));
    if (!choice) return null;
    const once = choice === L('consent.once');
    for (const s of missing) {
      const g = await rt.broker.authorize({providerId: grantKey, workspace, scope: s, once, ttlMs: once ? 10 * 60 * 1000 : 24 * 3600 * 1000});
      if (once && s === 'L3') onceGrant = g;
      else if (once) await rt.broker.consume(g.id); // the directory-names grant is consumed by this one request
    }
  }
  const symbolsHere = extractSymbols(info.language, fileText, 60);
  const dirSyms = needDirectory ? await directorySymbols(doc) : [];
  const prompt = annotate.buildAnnotatePrompt({language: info.language, dialect: info.dialect, locale: rt.locale, comment: w.comment, graph: w.graph, fileName: path.basename(doc.fileName),
    fileText: sent, snippet: template, line: position.line + 1, currentSymbols: symbolsHere, directorySymbols: dirSyms});
  const controller = new AbortController();
  try {
    const res = await vscode.window.withProgress({location: vscode.ProgressLocation.Notification, title: L('annotate.progress'), cancellable: true}, (_p, token) => {
      token.onCancellationRequested(() => controller.abort());
      return rt.ai.infer({...sc, providerId: grantKey, key, ...prompt, maxOutputTokens: Math.max(sc.maxOutputTokens, 3000), signal: controller.signal, grantId: onceGrant ? onceGrant.id : undefined, grantScope: 'L3'});
    });
    const out = annotate.applyAnnotateReply(res.text, {profile: info.language, original: template, dialect: info.dialect, comment: w.comment, graph: w.graph});
    if (out.notes.some(n => n.startsWith('comment-rejected'))) vscode.window.showWarningMessage(L('annotate.rejected'));
    const kinds = [out.commented && L('annotate.kind.comment'), out.graph && L('annotate.kind.graph')].filter(Boolean);
    return out.text === template ? null : {text: out.text, kinds};
  } catch (e) {
    if (codeOf(e) !== 'AI_CANCELLED') vscode.window.showWarningMessage(L('annotate.failed', errorText(e)));
    return null;
  }
}
module.exports = {annotateInsertion, directorySymbols, wanted};
