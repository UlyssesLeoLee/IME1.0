'use strict';
// Input session epochs, realtime AI scheduling and Direct fallback (FR-027/028, NFR-102/202, DD §8/§16).
// Local candidates are never routed through this class: it only governs the optional, asynchronous AI path.
const {resolveLanguage, LANGUAGES} = require('./languages');

const DEBOUNCE = Object.freeze({min: 150, max: 1000, def: 350});
const clampDebounce = ms => Math.min(DEBOUNCE.max, Math.max(DEBOUNCE.min, Number.isFinite(ms) ? ms : DEBOUNCE.def));

// profile_epoch / document_epoch / input_seq: any change of one invalidates results tied to the older value.
class InputSession {
  constructor({language = 'unknown'} = {}) {
    this.language = language; this.profileEpoch = 0; this.documentEpoch = 0; this.inputSeq = 0;
    this.fileLocks = new Map(); this.globalLock = null; this.paused = false;
  }
  noteInput() { return ++this.inputSeq; }
  noteDocumentChange() { return ++this.documentEpoch; }
  setLanguage(language) {
    if (language === this.language) return false;
    this.language = language; this.profileEpoch++; return true;
  }
  lock(profile, scope = 'file', fileKey = '') {
    if (!LANGUAGES.includes(profile)) return false;
    if (scope === 'global') this.globalLock = profile; else this.fileLocks.set(fileKey, profile);
    return true;
  }
  unlock(scope = 'file', fileKey = '') { if (scope === 'global') this.globalLock = null; else this.fileLocks.delete(fileKey); }
  lockFor(fileKey) { return this.fileLocks.get(fileKey) || this.globalLock || null; }
  // A manual lock always beats automatic detection; weak evidence never switches the profile (UT-012..015).
  resolve({languageId, fileName, text, fileKey = ''}) {
    const decision = resolveLanguage({languageId, fileName, text}, this.language, this.lockFor(fileKey));
    if (decision.language && decision.language !== 'unknown') this.setLanguage(decision.language);
    return {...decision, language: this.language};
  }
  key() { return {seq: this.inputSeq, profileEpoch: this.profileEpoch, documentEpoch: this.documentEpoch}; }
  isCurrent(k) { return !!k && k.seq === this.inputSeq && k.profileEpoch === this.profileEpoch && k.documentEpoch === this.documentEpoch; }
}

const sameKey = (a, b) => !!a && !!b && a.seq === b.seq && a.profileEpoch === b.profileEpoch && a.documentEpoch === b.documentEpoch;

class RealtimeScheduler {
  constructor({debounceMs = DEBOUNCE.def, timers = {setTimeout, clearTimeout}, run, onResult = () => {}, onDiscard = () => {}, isAllowed = () => true}) {
    Object.assign(this, {timers, run, onResult, onDiscard, isAllowed});
    this.debounceMs = clampDebounce(debounceMs);
    this.latest = null; this.timer = null; this.controller = null; this.chain = Promise.resolve();
    this.stats = {scheduled: 0, started: 0, delivered: 0, discarded: 0, inFlight: 0, maxInFlight: 0};
  }
  setDebounce(ms) { this.debounceMs = clampDebounce(ms); }
  // Local state is invalidated at once; the network abort is best effort (already-sent data cannot be recalled).
  schedule(key, payload) {
    this.stats.scheduled++;
    this.latest = key;
    if (this.timer) { this.timers.clearTimeout(this.timer); this.timer = null; }
    if (this.controller) this.controller.abort();
    if (!this.isAllowed()) return;
    this.timer = this.timers.setTimeout(() => {
      this.timer = null;
      this.chain = this.chain.then(() => this._fire(key, payload)).catch(() => {});
    }, this.debounceMs);
  }
  async _fire(key, payload) {
    if (!sameKey(key, this.latest) || !this.isAllowed()) return; // superseded while waiting for the previous request
    const controller = this.controller = new AbortController();
    this.stats.started++; this.stats.inFlight++; this.stats.maxInFlight = Math.max(this.stats.maxInFlight, this.stats.inFlight);
    try {
      const result = await this.run(payload, controller.signal, key);
      if (!sameKey(key, this.latest) || controller.signal.aborted) { this.stats.discarded++; this.onDiscard('STALE_RESULT', key); return; }
      this.stats.delivered++; this.onResult(result, key);
    } catch (e) {
      this.stats.discarded++; this.onDiscard(e && e.code ? e.code : 'ERROR', key);
    } finally { this.stats.inFlight--; }
  }
  cancel() {
    this.latest = null;
    if (this.timer) { this.timers.clearTimeout(this.timer); this.timer = null; }
    if (this.controller) this.controller.abort();
  }
}

// Core/IPC loss: the input path goes to Direct immediately and stays there until the core answers again.
class CoreSupervisor {
  constructor({now = Date.now, graceMs = 2000} = {}) { this.now = now; this.graceMs = graceMs; this.state = 'core'; this.directSince = null; this.failures = 0; }
  reportFailure() { this.failures++; if (this.state !== 'direct') { this.state = 'direct'; this.directSince = this.now(); } }
  reportRecovered() { this.state = 'core'; this.directSince = null; }
  get isDirect() { return this.state === 'direct'; }
  // time between the failure being reported and Direct being in force (design target: <= 2 s, NFR-102)
  timeToDirect(failureAt) { return this.directSince === null ? Infinity : this.directSince - failureAt; }
}
module.exports = {DEBOUNCE, clampDebounce, InputSession, RealtimeScheduler, CoreSupervisor, sameKey};
