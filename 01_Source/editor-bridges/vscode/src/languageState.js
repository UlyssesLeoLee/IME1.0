'use strict';
// Per-document language/dialect state for the bridge (FR-010/011/012). Manual locks beat auto detection, weak
// evidence never switches the profile, and locks persist per file (hashed uri, no path text is stored).
const vscode = require('vscode');
const crypto = require('node:crypto');
const {rt, cfg} = require('./runtime');
const {resolveDialect, LANGUAGES} = require('./core/languages');

const fileKey = doc => crypto.createHash('sha256').update(doc.uri.toString()).digest('hex').slice(0, 16);
let cache = null;

function initLocks() {
  const files = rt.ctx.workspaceState.get('pime.fileLocks') || {};
  for (const [k, v] of Object.entries(files)) rt.session.lock(v, 'file', k);
  const global = rt.ctx.globalState.get('pime.globalLock');
  if (global) rt.session.lock(global, 'global');
}
function persistLocks() {
  rt.ctx.workspaceState.update('pime.fileLocks', Object.fromEntries(rt.session.fileLocks));
  rt.ctx.globalState.update('pime.globalLock', rt.session.globalLock || undefined);
}
function dialectFor(profile, hints = {}) {
  const user = (cfg().get('dialects') || {})[profile] || {};
  return resolveDialect(profile, {...user, ...hints}).values;
}
// A secret-class file that the user has not allowed (src/filegate.js) is detected from its NAME only: its text is never read here. The check
// is the shared gate unless a test replaces it.
let lockCheck = doc => require('./filegate').get().isLocked(doc);
function setLockCheck(fn) { lockCheck = typeof fn === 'function' ? fn : () => false; cache = null; }
// Result: {language, dialect, uncertain, suggestion, reason, locked}
function infoFor(doc) {
  const secret = !!lockCheck(doc); // part of the cache key: allowing the file must not keep the name-only answer alive
  if (cache && cache.uri === doc.uri.toString() && cache.version === doc.version && cache.locks === lockSignature() && cache.epoch === rt.session.profileEpoch && cache.secret === secret) return cache.info;
  const head = secret ? '' : doc.getText(new vscode.Range(0, 0, Math.min(doc.lineCount, 400), 0));
  const d = rt.session.resolve({languageId: doc.languageId, fileName: doc.fileName, text: head, fileKey: fileKey(doc)});
  const info = {language: d.language, dialect: dialectFor(d.language, d.dialectHints), uncertain: !!d.uncertain, suggestion: d.suggestion, reason: d.reason, locked: !!d.locked};
  cache = {uri: doc.uri.toString(), version: doc.version, locks: lockSignature(), epoch: rt.session.profileEpoch, secret, info};
  return info;
}
const lockSignature = () => JSON.stringify([...rt.session.fileLocks]) + (rt.session.globalLock || '');
function lock(doc, profile, scope) {
  if (!LANGUAGES.includes(profile)) return;
  rt.session.lock(profile, scope, fileKey(doc));
  rt.session.setLanguage(profile);
  persistLocks(); cache = null;
}
function unlock(doc) { rt.session.unlock('file', fileKey(doc)); rt.session.unlock('global'); persistLocks(); cache = null; }
module.exports = {initLocks, infoFor, dialectFor, lock, unlock, fileKey, setLockCheck, invalidate: () => { cache = null; }};
