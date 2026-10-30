'use strict';
// Outbound sensitivity screening (FR-049, DD §7.3). Findings carry kinds and offsets only - never the matched
// value - so they are safe to log. The scanner cannot find every secret; it reduces accidental exposure.
const SENSITIVE_FILE = /(?:^|[\\/])(?:\.env(?:\.[\w.-]+)?|[^\\/]*\.(?:pem|key|pfx|p12|jks|keystore)|id_(?:rsa|dsa|ecdsa|ed25519)|credentials(?:\.json)?|\.npmrc|\.netrc|\.pypirc|secrets?\.(?:json|ya?ml|toml))$/i;
const PLACEHOLDER_VALUE = /^(?:x{4,}|\*{3,}|\.{3,}|your[_-].*|change-?me|example.*|placeholder.*|<.*>|todo|none|null|undefined|true|false)$/i;
const RULES = Object.freeze([
  {kind: 'private-key', severity: 'high', re: /-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----/g},
  {kind: 'aws-access-key', severity: 'high', re: /\b(?:AKIA|ASIA)[0-9A-Z]{16}\b/g},
  {kind: 'github-token', severity: 'high', re: /\bgh[pousr]_[A-Za-z0-9]{36,}\b|\bgithub_pat_[A-Za-z0-9_]{40,}\b/g},
  {kind: 'api-key', severity: 'high', re: /\bsk-(?:proj-|live-|test-|ant-)?[A-Za-z0-9_-]{20,}\b/g},
  {kind: 'slack-token', severity: 'high', re: /\bxox[abprs]-[A-Za-z0-9-]{10,}\b/g},
  {kind: 'google-api-key', severity: 'high', re: /\bAIza[0-9A-Za-z_-]{35}\b/g},
  {kind: 'bearer-token', severity: 'high', re: /\bBearer\s+[A-Za-z0-9._~+/-]{20,}=*/g},
  {kind: 'jwt', severity: 'medium', re: /\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b/g},
  {kind: 'url-credentials', severity: 'medium', re: /\b[a-z][a-z0-9+.-]*:\/\/[^\s/:@"']+:[^\s/@"']+@/gi},
  {kind: 'credential-assignment', severity: 'medium', group: 1,
   re: /\b[\w.-]*(?:api[_-]?key|secret|token|passwd|password|pwd|private[_-]?key|access[_-]?key|client[_-]?secret)[\w.-]*\s*[:=]\s*["']?(?!\$\{|<|\{\{|process\.env|os\.environ|getenv|env\()([^\s"'`;,#]{8,})/gi},
  {kind: 'private-path', severity: 'medium', re: /\b[A-Za-z]:\\Users\\[^\\\s"'<>|]+|\/(?:Users|home)\/[^/\s"'<>]+/g}
]);
const MAX_SCAN = 4 * 1024 * 1024;

const isSensitiveFileName = name => typeof name === 'string' && SENSITIVE_FILE.test(name);

function scan(text, {fileName = ''} = {}) {
  const findings = [];
  const sensitiveFile = isSensitiveFileName(fileName);
  if (typeof text !== 'string') return {findings, blocked: false, sensitiveFile, highest: 'none'};
  const sample = text.length > MAX_SCAN ? text.slice(0, MAX_SCAN) : text;
  const lineAt = offset => { let n = 1; for (let i = 0; i < offset; i++) if (sample.charCodeAt(i) === 10) n++; return n; };
  for (const rule of RULES) {
    rule.re.lastIndex = 0;
    const re = new RegExp(rule.re.source, rule.re.flags.includes('d') ? rule.re.flags : rule.re.flags + 'd');
    let m;
    while ((m = re.exec(sample))) {
      const span = rule.group ? m.indices[rule.group] : m.indices[0];
      if (!span) continue;
      if (rule.kind === 'credential-assignment' && PLACEHOLDER_VALUE.test(m[rule.group])) continue;
      findings.push({kind: rule.kind, severity: rule.severity, start: span[0], end: span[1], line: lineAt(span[0])});
      if (m[0].length === 0) re.lastIndex++;
    }
  }
  findings.sort((a, b) => a.start - b.start || b.end - a.end);
  const highest = findings.some(f => f.severity === 'high') || sensitiveFile ? 'high' : findings.length ? 'medium' : 'none';
  return {findings, blocked: highest === 'high', sensitiveFile, highest, truncated: text.length > MAX_SCAN};
}

// Replace findings with PIME_REDACTED_n markers; the map lets proposals be restored locally.
function redact(text, findings) {
  const map = [];
  let out = '', cursor = 0, n = 0;
  for (const f of findings) {
    if (f.start < cursor) continue; // overlapping finding already covered
    n++;
    out += text.slice(cursor, f.start) + `PIME_REDACTED_${n}`;
    map.push({marker: `PIME_REDACTED_${n}`, original: text.slice(f.start, f.end), kind: f.kind});
    cursor = f.end;
  }
  return {text: out + text.slice(cursor), map, count: n};
}
function restore(text, map) {
  const byMarker = new Map(map.map(x => [x.marker, x.original]));
  let unresolved = 0;
  const out = text.replace(/PIME_REDACTED_(\d+)/g, (all) => {
    if (byMarker.has(all)) return byMarker.get(all);
    unresolved++; return all;
  });
  return {text: out, unresolved};
}
// Decide what may leave the machine. realtime: never send high-risk context; f1: user must decide explicitly.
function assessOutbound(text, {fileName = '', scope = 'realtime'} = {}) {
  const scanned = scan(text, {fileName});
  if (scanned.highest === 'none') return {action: 'allow', scanned};
  if (scanned.highest === 'medium') return {action: 'redact', scanned};
  if (scope === 'realtime') return {action: 'block', scanned};
  return {action: 'confirm', scanned};
}
const summarize = findings => {
  const counts = {};
  for (const f of findings) counts[f.kind] = (counts[f.kind] || 0) + 1;
  return counts;
};
module.exports = {isSensitiveFileName, scan, redact, restore, assessOutbound, summarize, RULES};
