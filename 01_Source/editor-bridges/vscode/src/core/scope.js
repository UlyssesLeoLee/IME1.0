'use strict';
// Scope resolver for F1 ("annotate what I am looking at"): from a cursor or a selection find the innermost declaration around it
// (or the paragraph "block", or the selection snapped to whole lines) plus the chain of wider candidates used for widening.
// Heuristic text scan, NOT a parser: the user presses F1 mid-edit, so nothing may be assumed to be well formed. Whatever cannot be
// matched with confidence degrades to a plain block; the resolver never throws and never returns a range that misses the cursor.
// Pure and deterministic (no vscode, fs, clock or randomness). All offsets are UTF-16 code units, lines are 0-based.
const {maskSource} = require('./validators');

const PROFILES = Object.freeze(['rust', 'typescript', 'python', 'powershell', 'c', 'assembly', 'machine-code']);
const DEFAULT_MAX_CHARS = 20000;
const HARD_CAP = 2 * 1024 * 1024;  // the analysed text never exceeds this: a selection that large is reported without analysis
const FULL_SCAN_MAX = 512 * 1024;  // a larger text is analysed through a window of whole lines around the selection (keeps F1 snappy)
const HUGE_WINDOW = 256 * 1024;    // characters taken on each side of the selection in that case
const MAX_DEPTH = 1000;            // brace nesting deeper than this is ignored
const MAX_LOOKUP_LINES = 2000;     // comment/decorator lines taken along above a header, trailing comment lines below a Python body
const HEADER_WINDOW = 1500;        // how far back from a '{' the header of a declaration is looked for
const MASK_STYLE = {rust: 'rust', typescript: 'typescript', python: 'python', powershell: 'powershell', c: 'c'};

// ---- text model: lines (\r\n, \r and \n end a line, as in VS Code), masked code, per-line kind ----
const isWs = c => c === 32 || (c >= 9 && c <= 13) || c === 0xA0 || c === 0xFEFF || c === 0x1680 || (c >= 0x2000 && c <= 0x200A) || c === 0x2028 || c === 0x2029 || c === 0x202F || c === 0x205F || c === 0x3000;
const clampOff = (v, n) => typeof v !== 'number' || Number.isNaN(v) ? 0 : Math.min(n, Math.max(0, Math.trunc(v)));

function lineIndex(text) {
  const starts = [0], ends = [], n = text.length;
  for (let i = 0; i < n; i++) {
    const c = text.charCodeAt(i);
    if (c === 10) { ends.push(i); starts.push(i + 1); }
    else if (c === 13) { ends.push(i); if (text.charCodeAt(i + 1) === 10) i++; starts.push(i + 1); }
  }
  ends.push(n);
  return {starts, ends};
}
function lineOf(src, off) {
  let lo = 0, hi = src.L - 1;
  while (lo < hi) { const mid = (lo + hi + 1) >> 1; if (src.starts[mid] <= off) lo = mid; else hi = mid - 1; }
  return lo;
}

// Assembly: comments run from ';' (NASM/MASM) or '#' / '//' (GAS) to the end of the line, quotes protect the comment character.
function asmMask(text, starts, ends, dialect) {
  const gas = dialect.assembler === 'gas', parts = [], spans = [];
  for (let k = 0; k < starts.length; k++) {
    const a = starts[k], b = ends[k];
    let cut = -1, q = 0;
    for (let i = a; i < b; i++) {
      const c = text.charCodeAt(i);
      if (q) { if (c === q) q = 0; continue; }
      if (c === 34 || (c === 39 && !gas)) { q = c; continue; }
      if (gas ? (c === 35 || (c === 47 && text.charCodeAt(i + 1) === 47)) : c === 59) { cut = i; break; }
    }
    if (cut < 0) parts.push(text.slice(a, b));
    else { parts.push(text.slice(a, cut) + ' '.repeat(b - cut)); spans.push({start: cut, end: b, kind: 'comment'}); }
    parts.push(text.slice(b, k + 1 < starts.length ? starts[k + 1] : b));
  }
  return {m: parts.join(''), spans};
}
// In these languages a ' or " string ends with its line (unless the newline is escaped), but maskSource lets it run on until the
// next quote. A regex literal such as /["']/ (maskSource does not know them) or a quote typed mid-edit would therefore turn the
// rest of the file inside out. Such a span is bogus: its quote is neutralised and the rest is masked again (bounded repairs).
const LINE_STRINGS = new Set(['typescript', 'c', 'python']);
function bogusString(t, style, sp) {
  if (sp.kind !== 'string') return false;
  const q = t[sp.start];
  if (q !== '"' && q !== "'") return false;
  if (style === 'python' && t.startsWith(q.repeat(3), sp.start)) return false;
  for (let i = sp.start + 1; i < Math.min(sp.end, t.length); i++) {
    if (t.charCodeAt(i) !== 10) continue;
    let k = i - 1; if (k > sp.start && t.charCodeAt(k) === 13) k--;
    let bs = 0; while (k > sp.start && t.charCodeAt(k) === 92) { bs++; k--; }
    if (bs % 2 === 0) return true;
  }
  return false;
}
function maskLines(style, t) {
  const parts = [], spans = [];
  let base = 0, seg = t, budget = Math.max(8 * t.length, 1 << 20); // characters that may be masked again (every repair redoes the rest)
  for (;;) {
    const sp = [], mm = maskSource(seg, style, sp);
    const bad = LINE_STRINGS.has(style) && budget >= seg.length ? sp.find(x => bogusString(seg, style, x)) : null;
    const upto = bad ? bad.start : seg.length;
    for (const x of sp) { if (x.end > upto) break; spans.push({start: x.start + base, end: x.end + base, kind: x.kind}); }
    if (!bad) { parts.push(mm); break; }
    budget -= seg.length;
    parts.push(mm.slice(0, bad.start) + ' ');
    base += bad.start + 1; seg = seg.slice(bad.start + 1);
  }
  return {m: parts.join(''), spans};
}
// maskSource does not know JavaScript regex literals, whose quotes and brackets (/["']/, /\{\{/) would unbalance everything after
// them. They are rewritten into same-length "   " string literals first, so that maskSource then masks them like any string.
const JS_TOKEN = /\/\/[^\n]*|\/\*[\s\S]*?(?:\*\/|$)|"(?:[^"\\\n]|\\(?:\r\n|[\s\S]))*"?|'(?:[^'\\\n]|\\(?:\r\n|[\s\S]))*'?|`(?:[^`\\]|\\[\s\S])*`?|\//g;
const REGEX_AFTER_WORD = new Set(['return', 'typeof', 'case', 'in', 'of', 'delete', 'void', 'throw', 'new', 'else', 'do', 'yield', 'await', 'instanceof']);
function regexOrDivision(text, p) { // p: index of the last significant character before a '/'
  const c = text.charCodeAt(p);
  if (isWord(c)) { let q = p; while (q > 0 && isWord(text.charCodeAt(q - 1))) q--; return REGEX_AFTER_WORD.has(text.slice(q, p + 1)); }
  return '(,=:[!&|?{;+-*%>~^'.includes(text[p]);
}
function neutraliseRegex(text) {
  JS_TOKEN.lastIndex = 0;
  const parts = [];
  let copied = 0, gap = 0, prev = 'start', at = -1, m;
  while ((m = JS_TOKEN.exec(text))) {
    const i = m.index, t = m[0];
    let p = i - 1; while (p >= gap && isWs(text.charCodeAt(p))) p--;
    if (p >= gap) { prev = 'char'; at = p; }
    if (t === '/') {
      const ok = prev === 'start' || (prev === 'char' && regexOrDivision(text, at));
      let end = -1;
      if (ok) {
        for (let j = i + 1, cls = false; j < text.length; j++) {
          const c = text.charCodeAt(j);
          if (c === 10 || c === 13) break;
          if (c === 92) j++;
          else if (c === 91) cls = true;
          else if (c === 93) cls = false;
          else if (c === 47 && !cls) { end = j; break; }
        }
      }
      if (end > i + 1) { parts.push(text.slice(copied, i), '"' + ' '.repeat(end - i - 1) + '"'); copied = end + 1; JS_TOKEN.lastIndex = end + 1; gap = end + 1; prev = 'value'; }
      else { prev = 'char'; at = i; gap = i + 1; }
    } else { gap = i + t.length; if (t[0] !== '/') prev = 'value'; }
  }
  if (!parts.length) return text;
  parts.push(text.slice(copied));
  return parts.join('');
}
function maskFor(profile, dialect, text, starts, ends) {
  const style = MASK_STYLE[profile];
  if (style) {
    // maskSource only knows '\n'; a lone '\r' (old Mac files) must still end a line comment. Same length, so offsets are unchanged.
    let t = /\r(?!\n)/.test(text) ? text.replace(/\r(?!\n)/g, '\n') : text;
    if (profile === 'typescript') t = neutraliseRegex(t);
    return maskLines(style, t);
  }
  if (profile === 'assembly') return asmMask(text, starts, ends, dialect);
  return {m: text, spans: []};
}
// kind[k]: 0 blank, 1 comment only, 2 anything else (code, or a line that lies inside a string literal)
function buildSource(profile, dialect, text) {
  const {starts, ends} = lineIndex(text), L = starts.length, {m, spans} = maskFor(profile, dialect, text, starts, ends);
  const kind = new Uint8Array(L);
  let sp = 0;
  for (let k = 0; k < L; k++) {
    const a = starts[k], b = ends[k];
    let p = a; while (p < b && isWs(text.charCodeAt(p))) p++;
    if (p === b) continue;
    let q = a; while (q < b && isWs(m.charCodeAt(q))) q++;
    if (q < b) { kind[k] = 2; continue; }
    while (sp < spans.length && spans[sp].end <= p) sp++;
    kind[k] = sp < spans.length && spans[sp].start <= p && spans[sp].kind === 'comment' ? 1 : 2;
  }
  return {profile, dialect, text, m, n: text.length, starts, ends, L, kind, spans};
}

// ---- declarations: {cs, ce} is the character span of header..closing token (used for nesting), {first, last} the line range
// reported to the caller (first includes attributes, decorators and comments directly above the header) ----
const skipWs = (s, i) => { while (i < s.length && isWs(s.charCodeAt(i))) i++; return i; };
// index just after the bracket group that opens at s[i] ('(' '[' or '<'), or -1 when it is not closed inside s
function skipGroup(s, i, open, close) {
  let depth = 0;
  for (let j = i; j < s.length; j++) {
    const c = s[j];
    if (c === open) depth++;
    else if (c === close && !(close === '>' && s[j - 1] === '-')) { if (--depth === 0) return j + 1; }
  }
  return -1;
}
function attachAbove(src, line, attrRe) {
  let k = line;
  while (k > 0 && line - k < MAX_LOOKUP_LINES) {
    const j = k - 1;
    if (src.kind[j] === 1) { k = j; continue; }
    if (src.kind[j] === 2 && attrRe && attrRe.test(src.m.slice(src.starts[j], Math.min(src.ends[j], src.starts[j] + 240)))) { k = j; continue; }
    break;
  }
  return k;
}
function makeDecl(src, lang, info, closeIdx) {
  if (info.anon) { // typedef struct { ... } Name;
    const t = /^\s*([A-Za-z_]\w*)\s*[;,]/.exec(src.m.slice(closeIdx + 1, closeIdx + 120));
    if (t) info = {...info, name: t[1], label: `${info.sk} ${t[1]}`};
  }
  const first = attachAbove(src, lineOf(src, info.firstOff === undefined ? info.cs : info.firstOff), lang.attr);
  return {cs: info.cs, ce: closeIdx + 1, first, last: lineOf(src, closeIdx), sk: info.sk, name: info.name, label: info.label, children: [], parent: null};
}

// Brace languages: one pass over the masked text. A '{' is classified from the header text in front of it (back to the previous
// statement boundary), a matching '}' closes the declaration. Unmatched openers and stray closers never produce a declaration.
function braceScan(src, lang) {
  const {m, n} = src, decls = [], stack = [];
  let pd = 0, ss = 0, budget = 8 * n + 100000, atLine = true, deep = 0;
  for (let i = 0; i < n; i++) {
    if (atLine) {
      atLine = false;
      let p = i; while (p < n && (m.charCodeAt(p) === 32 || m.charCodeAt(p) === 9 || m.charCodeAt(p) === 0xFEFF)) p++;
      if (lang.preproc && m.charCodeAt(p) === 35) { // a preprocessor directive (with its continuation lines) is neither code nor a boundary of braces
        let j = p;
        for (;;) {
          const e = m.indexOf('\n', j);
          if (e < 0) { j = n; break; }
          const k = m.charCodeAt(e - 1) === 13 ? e - 2 : e - 1;
          if (m.charCodeAt(k) === 92) { j = e + 1; continue; }
          j = e; break;
        }
        ss = Math.min(n, j + 1); i = j - 1; // the loop's i++ lands on the newline, which re-arms atLine
        if (j >= n) break;
        continue;
      }
      if (pd > 0 && lang.strong) { lang.strong.lastIndex = p; if (lang.strong.test(m)) { pd = 0; ss = p; } } // unbalanced '(' of a half-typed header: resync
    }
    const c = m.charCodeAt(i);
    if (c === 10) atLine = true;
    else if (c === 40 || c === 91) pd++;
    else if (c === 41 || c === 93) { if (pd > 0) pd--; }
    else if (c === 59) { if (pd === 0) ss = i + 1; }
    else if (c === 123) {
      if (stack.length >= MAX_DEPTH) { deep++; continue; } // absurd nesting (or a flood of '{'): not worth a frame
      // a '{' inside parentheses (destructured or defaulted parameters) or in a type position (`Promise<{ok: boolean}>`, `: {a: number}`)
      // is part of the header, not a body: it is never a declaration and its '}' must not end the statement
      let info = null, inHeader = pd > 0;
      if (!inHeader && lang.typeBrace) { let q = i - 1; while (q >= ss && isWs(m.charCodeAt(q))) q--; inHeader = q >= ss && lang.typeBrace.includes(m[q]); }
      if (!inHeader && budget > 0) { const from = Math.max(ss, i - HEADER_WINDOW); budget -= i - from + 1; info = lang.classify(src, from, i, stack.length ? stack[stack.length - 1].info : null); }
      stack.push({ss, pd, info, inHeader});
      pd = 0; ss = i + 1;
    } else if (c === 125) {
      if (deep > 0) { deep--; continue; }
      const f = stack.pop();
      if (f) { if (f.info) decls.push(makeDecl(src, lang, f.info, i)); pd = f.pd; ss = f.pd === 0 && !f.inHeader ? i + 1 : f.ss; } else ss = i + 1;
    }
  }
  return decls;
}

// ---- Rust ----
const RUST_ITEM = /(?:pub\b(?:\s*\([^()]*\))?\s*)?(?:(?:default|async|unsafe|const|extern|safe)\b\s*)*(?:(fn)\s+([A-Za-z_]\w*)|(struct|enum|trait|union|mod)\s+([A-Za-z_]\w*)|(impl)\b|(macro_rules)\s*!\s*([A-Za-z_]\w*))/y;
const RUST_STRONG = /(?:pub\b(?:\s*\([^()]*\))?\s*)?(?:(?:async|unsafe|const|extern)\b\s*)*(?:(?:fn|struct|enum|trait|mod|union)\s+[A-Za-z_]|impl\s*<|impl\s+[A-Za-z_&'(\[])/y;
const RUST_ATTR = /^\s*#\s*\[/;
const lastSegment = s => { const m = /([A-Za-z_]\w*)\s*(?:<[\s\S]*)?$/.exec(s.replace(/^[\s&!]*(?:'\w+\s+)?(?:mut\s+|dyn\s+|unsafe\s+|const\s+)*/, '').replace(/\s+/g, '')); return m ? m[1] : null; };
function rustImpl(rest) {
  let s = rest.replace(/\bwhere\b[\s\S]*$/, '').trim();
  if (s[0] === '<') { const e = skipGroup(s, 0, '<', '>'); s = e < 0 ? '' : s.slice(e).trim(); }
  let depth = 0, at = -1;
  for (let i = 0; i < s.length; i++) { // ` for ` at generic depth 0 separates trait and type
    const c = s[i];
    if (c === '<') depth++; else if (c === '>' && s[i - 1] !== '-') depth--;
    else if (depth === 0 && c === 'f' && /^\bfor\s/.test(s.slice(i, i + 4)) && (i === 0 || isWs(s.charCodeAt(i - 1)))) { at = i; break; }
  }
  const tail = at < 0 ? s : s.slice(at + 3), name = lastSegment(tail.replace(/<[\s\S]*$/, '')), tr = at < 0 ? null : lastSegment(s.slice(0, at).replace(/<[\s\S]*$/, ''));
  return {name, label: name ? (tr ? `impl ${tr} for ${name}` : `impl ${name}`) : 'impl'};
}
function rustClassify(src, from, open) {
  const P = src.m.slice(from, open);
  let i0 = skipWs(P, 0), i = i0;
  if (i0 >= P.length) return null;
  while (P[i] === '#') { // outer attributes #[..] belong to the item; inner ones #![..] belong to the enclosing scope
    let j = i + 1;
    const inner = P[j] === '!'; if (inner) j++;
    j = skipWs(P, j);
    if (P[j] !== '[') return null;
    const e = skipGroup(P, j, '[', ']');
    if (e < 0) return null;
    i = skipWs(P, e);
    if (inner) i0 = i;
  }
  RUST_ITEM.lastIndex = i;
  const r = RUST_ITEM.exec(P);
  if (!r) return null;
  const cs = from + i0;
  if (r[1]) return {cs, sk: 'fn', name: r[2], label: `fn ${r[2]}`};
  if (r[3]) return {cs, sk: r[3], name: r[4], label: `${r[3]} ${r[4]}`};
  if (r[6]) return {cs, sk: 'macro', name: r[7], label: `macro_rules! ${r[7]}`};
  const im = rustImpl(P.slice(RUST_ITEM.lastIndex));
  return {cs, sk: 'impl', name: im.name, label: im.label};
}

// ---- TypeScript / JavaScript ----
const TS_NOT_METHOD = new Set(['if', 'for', 'while', 'switch', 'catch', 'with', 'return', 'typeof', 'new', 'delete', 'void', 'throw', 'await', 'yield', 'function', 'else', 'do', 'try', 'in', 'of', 'instanceof']);
const TS_FUNC = /(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:async\s+)?function\b\s*\*?\s*([A-Za-z_$][\w$]*)/y;
const TS_CLASS = /(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:abstract\s+)?class\b\s*([A-Za-z_$][\w$]*)/y;
const TS_TYPE = /(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:const\s+)?(interface|enum|namespace|module|type)\s+([A-Za-z_$][\w$.]*)/y;
const TS_TARGET = '(?:export\\s+)?(?:(?:public|private|protected|static|readonly|declare|override)\\s+)*(?:(?:const|let|var)\\s+)?([A-Za-z_$#][\\w$]*(?:\\.[A-Za-z_$][\\w$]*)*)';
const TS_ASSIGN = new RegExp(`${TS_TARGET}\\s*(?::[^=]*?)?=\\s*(?:async\\s+)?`, 'y');   // name = ..., name: Type = ...
const TS_PROP = new RegExp(`${TS_TARGET}\\s*:\\s*(?:async\\s+)?`, 'y');                  // name: function ..., name: (..) => ...
const TS_METHOD = /(?:(?:public|private|protected|static|abstract|override|readonly|async|get|set|declare|accessor)\s+)*(?:\*\s*)?([#]?[A-Za-z_$][\w$]*|\[[^\]]*\])\s*(?:<[^()]*>)?\s*\??\s*\(/y;
const TS_STRONG = /(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:abstract\s+)?(?:async\s+)?(?:function|class|interface|enum|namespace)\s+[A-Za-z_$]/y;
const TS_ATTR = /^\s*@[\w$.]+(?:\(.*\))?\s*$/;
// `rest` is what follows `name =` / `name:`: a function expression or an arrow function that ends exactly at the '{'?
function tsFunctionTail(rest) {
  let j = 0;
  if (/^function\b/.test(rest)) {
    const o = rest.indexOf('(');
    if (o < 0) return false;
    j = skipGroup(rest, o, '(', ')');
    return j >= 0 && /^\s*(?::[\s\S]*)?$/.test(rest.slice(j));
  }
  const g = /^(?:<[^()]*>\s*)?/.exec(rest)[0].length;
  if (rest[g] === '(') { j = skipGroup(rest, g, '(', ')'); return j >= 0 && /^\s*(?::[^=]*)?=>\s*$/.test(rest.slice(j)); }
  return /^[A-Za-z_$][\w$]*\s*=>\s*$/.test(rest);
}
function tsMatch(P, o) {
  TS_FUNC.lastIndex = o; let r = TS_FUNC.exec(P);
  if (r) return {sk: 'function', name: r[1], label: `function ${r[1]}`};
  TS_CLASS.lastIndex = o; r = TS_CLASS.exec(P);
  if (r) return {sk: 'class', name: r[1], label: `class ${r[1]}`};
  TS_TYPE.lastIndex = o; r = TS_TYPE.exec(P);
  if (r && (r[1] !== 'type' || /^\s*(?:<[^=]*>)?\s*=\s*$/.test(P.slice(TS_TYPE.lastIndex)))) return {sk: r[1], name: r[2], label: `${r[1]} ${r[2]}`};
  for (const re of [TS_ASSIGN, TS_PROP]) {
    re.lastIndex = o; r = re.exec(P);
    if (r && tsFunctionTail(P.slice(re.lastIndex))) return {sk: 'function', name: r[1], label: `function ${r[1]}`};
  }
  TS_METHOD.lastIndex = o; r = TS_METHOD.exec(P);
  if (r && !TS_NOT_METHOD.has(r[1])) {
    const e = skipGroup(P, TS_METHOD.lastIndex - 1, '(', ')');
    if (e >= 0 && /^\s*(?::[\s\S]*)?$/.test(P.slice(e))) return {sk: 'method', name: r[1], label: `method ${r[1]}`};
  }
  return null;
}
function tsClassify(src, from, open) {
  const P = src.m.slice(from, open);
  const cands = [];
  for (let i = 0, depth = 0; i < P.length; i++) { // statements and members start at the beginning, after a newline or after a top-level comma
    const c = P[i];
    if (c === '(' || c === '[' || c === '{') depth++; else if (c === ')' || c === ']' || c === '}') depth--;
    else if ((c === '\n' || c === ',') && depth <= 0) cands.push(i + 1); // (lines inside a parameter list cannot start a declaration)
  }
  cands.unshift(0);
  for (let k = cands.length - 1, tries = 0; k >= 0 && tries < 24; k--, tries++) {
    const o = skipWs(P, cands[k]);
    if (o >= P.length) continue;
    const r = tsMatch(P, o);
    if (r) { // a decorator or attribute group at the start of the statement belongs to the declaration
      const s0 = skipWs(P, 0), top = P[s0] === '@' ? s0 : o;
      return {...r, cs: from + o, firstOff: from + top};
    }
  }
  return null;
}

// `const f = x => expr;` has no braces to match: the statement itself is the declaration (ends at ';' or where the expression ends).
const TS_EXPR_ARROW = /[ \t\uFEFF]*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::[^=]+)?=\s*(?:async\s+)?(?:\([^()]*(?:\([^()]*\)[^()]*)*\)|[A-Za-z_$][\w$]*)\s*(?::[^=]+?)?=>\s*/y;
const CONTINUES = '=>,+-*/%&|?:(.[{<>=!';        // a line ending with one of these does not end the statement
const CONTINUED_BY = '.?:+-*/%&|,)]}=<>';        // a line starting with one of these continues the previous one
const TS_STOP = /(?:const|let|var|function|class|export|import|interface|enum|type)\b/y;
function statementEnd(m, from, limit, indent) {
  let depth = 0, last = from - 1;
  for (let i = from; i < limit; i++) {
    const c = m.charCodeAt(i);
    if (c === 40 || c === 91 || c === 123) depth++;
    else if (c === 41 || c === 93 || c === 125) { if (--depth < 0) return last; } // the enclosing construct closes first
    else if (c === 59 && depth === 0) return i;
    else if (c === 10) {
      if (depth === 0 && last >= from && !CONTINUES.includes(m[last])) {
        let j = i + 1; while (j < m.length && isWs(m.charCodeAt(j))) j++;
        if (j >= m.length || !CONTINUED_BY.includes(m[j])) return last;
      }
      let j = i + 1; while (j < m.length && (m.charCodeAt(j) === 32 || m.charCodeAt(j) === 9)) j++;
      TS_STOP.lastIndex = j;
      if (j - (i + 1) <= indent && TS_STOP.test(m)) return -1; // a new declaration at the same indentation: this expression was never closed
    }
    if (!isWs(c)) last = i;
  }
  return limit >= m.length ? last : -1;
}
function tsArrowScan(src) {
  const {m, n, starts, ends, kind, L} = src, out = [];
  let budget = 16 * n + 100000;
  for (let k = 0; k < L && budget > 0; k++) {
    if (kind[k] !== 2) continue;
    let p = starts[k]; while (p < ends[k] && isWs(m.charCodeAt(p))) p++;
    const c0 = m.charCodeAt(p);
    if (c0 !== 99 && c0 !== 108 && c0 !== 118 && c0 !== 101) continue; // c(onst) l(et) v(ar) e(xport)
    const seg = m.slice(p, Math.min(n, p + 600));
    TS_EXPR_ARROW.lastIndex = 0;
    const r = TS_EXPR_ARROW.exec(seg);
    if (!r) continue;
    const after = p + TS_EXPR_ARROW.lastIndex;
    if (after >= n || m[after] === '{') continue; // a block body is found by the brace scan
    const end = statementEnd(m, after, Math.min(n, after + 20000), p - starts[k]);
    budget -= (end >= after ? end - after : 20000) + 600;
    if (end < after) continue;
    out.push({cs: p, ce: end + 1, first: attachAbove(src, k, TS_ATTR), last: lineOf(src, end), sk: 'function', name: r[1], label: `function ${r[1]}`, children: [], parent: null});
  }
  return out;
}

// ---- Python: indentation based ----
const PY_HEAD = /[ \t\uFEFF]*(?:async[ \t]+)?(def|class)[ \t]+([A-Za-z_]\w*)/y;
function pyIndent(src, k) {
  let col = 0;
  for (let i = src.starts[k], e = src.ends[k]; i < e; i++) {
    const c = src.text.charCodeAt(i);
    if (c === 32) col++; else if (c === 9) col = ((col >> 3) << 3) + 8; else if (c === 12) col = 0; else if (c !== 0xFEFF) break;
  }
  return col;
}
function pyScan(src) {
  const {m, starts, ends, L, kind, spans} = src, decls = [], stack = [], lstart = new Int32Array(L).fill(-1);
  let depth = 0, cont = false, sp = 0, lastCode = -1, cur = -1, hdr = null;
  // decorators (possibly several lines long) and comments at the header's indentation directly above belong to the declaration
  const attach = (k0, indent) => {
    let k = k0;
    while (k > 0 && k0 - k < MAX_LOOKUP_LINES) {
      const j = k - 1;
      if (kind[j] === 0) break;
      if (kind[j] === 1) { if (pyIndent(src, j) <= indent) { k = j; continue; } break; }
      const s = lstart[j];
      if (s >= 0 && /^[ \t\uFEFF]*@/.test(m.slice(starts[s], Math.min(ends[s], starts[s] + 40)))) { k = s; continue; }
      break;
    }
    return k;
  };
  const closeDecl = (d, nextK) => {
    let last = Math.max(lastCode, d.hdrEnd), j = last + 1, steps = 0;
    while (j < nextK && steps++ < MAX_LOOKUP_LINES) { // trailing comments indented deeper than the header still belong to the body
      if (kind[j] === 0) { j++; continue; }
      if (kind[j] === 1 && pyIndent(src, j) > d.indent) { last = j; j++; continue; }
      break;
    }
    decls.push({cs: d.cs, ce: ends[last], first: attach(d.k0, d.indent), last, sk: d.kw, name: d.name, label: `${d.kw} ${d.name}`, children: [], parent: null});
  };
  for (let k = 0; k < L; k++) {
    if (kind[k] !== 2) continue; // blank and comment-only lines never change the structure
    const a = starts[k], b = ends[k];
    while (sp < spans.length && spans[sp].end <= a) sp++;
    const inStr = sp < spans.length && spans[sp].kind === 'string' && spans[sp].start < a;
    let isStart = !inStr && depth === 0 && !cont;
    if (!inStr && depth > 0) { PY_HEAD.lastIndex = a; if (PY_HEAD.test(m)) { depth = 0; cont = false; isStart = true; hdr = null; } } // a half-typed header's '(' never closed: resync
    if (isStart) {
      cur = k;
      const indent = pyIndent(src, k);
      while (stack.length && stack[stack.length - 1].indent >= indent) closeDecl(stack.pop(), k);
      PY_HEAD.lastIndex = a;
      const r = PY_HEAD.exec(m);
      let p = a; while (p < b && (m.charCodeAt(p) === 32 || m.charCodeAt(p) === 9 || m.charCodeAt(p) === 0xFEFF)) p++;
      hdr = r ? {k0: k, indent, kw: r[1], name: r[2], colon: false, cs: p, hdrEnd: k} : null;
    }
    lstart[k] = cur;
    let e = b; while (e > a && (m.charCodeAt(e - 1) === 32 || m.charCodeAt(e - 1) === 9)) e--;
    for (let i = a; i < b; i++) {
      const c = m.charCodeAt(i);
      if (c === 40 || c === 91 || c === 123) depth++;
      else if (c === 41 || c === 93 || c === 125) { if (depth > 0) depth--; }
      else if (c === 58 && depth === 0 && hdr) hdr.colon = true;
    }
    cont = e > a && m.charCodeAt(e - 1) === 92;
    if (depth === 0 && !cont && hdr) { if (hdr.colon) { hdr.hdrEnd = k; stack.push(hdr); } hdr = null; }
    lastCode = k;
  }
  while (stack.length) closeDecl(stack.pop(), L);
  return decls;
}

// ---- PowerShell ----
const PS_FUNC = /(?:function|filter|workflow|configuration)\s+(?:(?:global|script|local|private):)?([\w.-]+)/iy;
const PS_CLASS = /(class|enum)\s+([A-Za-z_]\w*)/iy;
const PS_METHOD = /(?:(?:hidden|static)\s+)*(?:\[[^\]]*\]\s*)?([A-Za-z_]\w*)\s*\(/iy;
const PS_STRONG = /(?:function|filter|workflow)\s+[\w.-]/iy;
const PS_ATTR = /^\s*\[.*\]\s*$/;
function psClassify(src, from, open, parent) {
  const P = src.m.slice(from, open), cands = [0];
  for (let i = 0; i < P.length; i++) if (P[i] === '\n') cands.push(i + 1);
  for (let k = cands.length - 1, tries = 0; k >= 0 && tries < 16; k--, tries++) {
    const o = skipWs(P, cands[k]);
    if (o >= P.length) continue;
    PS_FUNC.lastIndex = o; let r = PS_FUNC.exec(P);
    if (r) return {cs: from + o, sk: 'function', name: r[1], label: `function ${r[1]}`};
    PS_CLASS.lastIndex = o; r = PS_CLASS.exec(P);
    if (r) return {cs: from + o, sk: r[1].toLowerCase(), name: r[2], label: `${r[1].toLowerCase()} ${r[2]}`};
    if (parent && parent.sk === 'class') { // a method or constructor: only directly inside a class body (elsewhere `name(..) {` is control flow)
      PS_METHOD.lastIndex = o; r = PS_METHOD.exec(P);
      if (r) {
        const e = skipGroup(P, PS_METHOD.lastIndex - 1, '(', ')');
        if (e >= 0 && /^\s*(?::\s*(?:base|this)\s*\([\s\S]*\))?\s*$/i.test(P.slice(e))) return {cs: from + o, sk: 'method', name: r[1], label: `method ${r[1]}`};
      }
    }
  }
  return null;
}

// ---- C / C++ ----
const C_NOT_FUNC = new Set(['if', 'for', 'while', 'switch', 'return', 'sizeof', 'catch', 'do', 'else', 'alignas', 'alignof', '__attribute__', '__declspec', 'decltype', 'typeof', '_Alignas', '_Static_assert', 'static_assert', 'noexcept', 'throw', 'defined', '__extension__']);
const C_ACCESS = /(?:public|private|protected|signals|slots)\s*:(?!:)\s*/y;
const C_TEMPLATE = /template\s*</y;
const C_TYPE = /(?:typedef\s+)?(?:(?:static|const|volatile|extern|inline|__extension__|constexpr)\s+)*(struct|union|enum|class|namespace)\b(?:\s+(?:class|struct)\b)?/y;
const isWord = c => (c >= 48 && c <= 57) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122) || c === 95 || c === 36;
// __declspec(..), __attribute__((..)), alignas(..) and [[..]] in front of a name are not part of the type
function stripAttrs(s) {
  for (;;) {
    const m = /(?:__declspec|__attribute__|__attribute|alignas|_Alignas)\s*\(|\[\[/.exec(s);
    if (!m) return s;
    const e = m[0] === '[[' ? skipGroup(s, m.index, '[', ']') : skipGroup(s, m.index + m[0].length - 1, '(', ')');
    if (e < 0) return s;
    s = s.slice(0, m.index) + ' ' + s.slice(e);
  }
}
function cFunction(b) {
  let angle = 0, opAt = -1, opEnd = -1;
  for (let k = 0; k < b.length; k++) {
    const c = b[k];
    if (c === 'o' && (k === 0 || !isWord(b.charCodeAt(k - 1))) && b.startsWith('operator', k) && !isWord(b.charCodeAt(k + 8))) {
      const m = /^operator\s*(?:\(\s*\)|new\b|delete\b|[^\s\w(]+|[A-Za-z_][\w:\s*&<>]*?(?=\s*\())/.exec(b.slice(k, k + 60)); // operators are named by punctuation
      if (m) { opAt = k; opEnd = k + m[0].length; k = opEnd - 1; continue; }
    }
    if (c === '[') { const e = skipGroup(b, k, '[', ']'); if (e < 0) return null; k = e - 1; continue; }
    if (c === '<') { angle++; continue; }
    if (c === '>' && b[k - 1] !== '-') { if (angle > 0) angle--; continue; }
    if (c === '(' && angle > 0) { const e = skipGroup(b, k, '(', ')'); if (e < 0) return null; k = e - 1; continue; }
    if (angle > 0) continue;
    if (c === '=') return null; // an initialiser, not a function
    if (c !== '(') continue;
    let name, nameStart;
    if (opAt >= 0 && opEnd <= k && /^\s*$/.test(b.slice(opEnd, k))) { name = b.slice(opAt, opEnd).replace(/\s+/g, ''); nameStart = opAt; }
    else {
      const m = /(~?[A-Za-z_]\w*(?:\s*::\s*~?[A-Za-z_]\w*)*)\s*$/.exec(b.slice(Math.max(0, k - 200), k));
      if (!m) return null;
      name = m[1].replace(/\s+/g, ''); nameStart = k - (b.slice(Math.max(0, k - 200), k).length - m.index);
    }
    const close = skipGroup(b, k, '(', ')');
    if (close < 0) return null;
    if (C_NOT_FUNC.has(name.split('::').pop())) { k = close - 1; continue; } // `if (..)`, `alignas(8) struct S`: look further
    if (!/^[\w\s:*&<>,~[\]-]*$/.test(stripAttrs(b.slice(0, nameStart)))) return null;
    // what may follow the parameter list: qualifiers, a trailing return type, an initialiser list; never '=' or ';' outside parentheses
    if (!/^[\s\w:*&<>,~.[\]-]*(?:\([^()]*(?:\([^()]*\)[^()]*)*\)[\s\w:*&<>,~.[\]-]*)*$/.test(b.slice(close))) return null;
    return {sk: 'function', name, label: `function ${name}`};
  }
  return null;
}
// -> the type's name, null for an anonymous type, undefined when this is not a type definition
function cType(s) {
  for (;;) { // attributes between the keyword and the name
    let m = /^\s*(?:alignas|__attribute__|__declspec|_Alignas)\s*\(/.exec(s), e;
    if (m) e = skipGroup(s, m[0].length - 1, '(', ')');
    else if ((m = /^\s*\[\[/.exec(s))) e = skipGroup(s, s.indexOf('['), '[', ']');
    else break;
    if (e < 0) return undefined;
    s = s.slice(e);
  }
  const n = /^\s*([A-Za-z_]\w*(?:\s*::\s*[A-Za-z_]\w*)*)?/.exec(s), tail = s.slice(n[0].length);
  if (!/^\s*(?:final\b\s*)?(?::[^=();{}]*)?$/.test(tail)) return undefined; // `struct S s = {` is a variable, not a type
  return n[1] ? n[1].replace(/\s+/g, '') : null;
}
function cClassifyText(P, from) {
  let i = skipWs(P, 0);
  if (i >= P.length) return null;
  for (C_ACCESS.lastIndex = i; C_ACCESS.test(P); C_ACCESS.lastIndex = i) i = C_ACCESS.lastIndex;
  const top = i;
  let j = i;
  for (C_TEMPLATE.lastIndex = j; C_TEMPLATE.test(P); C_TEMPLATE.lastIndex = j) { // template<...> belongs to the declaration below it
    const e = skipGroup(P, C_TEMPLATE.lastIndex - 1, '<', '>');
    if (e < 0) return null;
    j = skipWs(P, e);
  }
  const body = P.slice(j), base = {cs: from + top, firstOff: from + top};
  const f = cFunction(body);
  if (f) return {...base, ...f};
  C_TYPE.lastIndex = j;
  const t = C_TYPE.exec(P);
  if (!t) return null;
  const name = cType(P.slice(C_TYPE.lastIndex));
  if (name === undefined) return null;
  const anon = name === null && /^\s*typedef\b/.test(P.slice(j));
  return {...base, sk: t[1], name, label: name ? `${t[1]} ${name}` : t[1], anon};
}
function cClassify(src, from, open) {
  const P = src.m.slice(from, open);
  let r = cClassifyText(P, from);
  if (!r) { // an unclosed '(' earlier in the statement (half-typed code): retry from the last line that starts in column 0
    const q = P.search(/\n[^\s}][^\n]*$/);
    if (q > 0) r = cClassifyText(P.slice(q + 1), from + q + 1);
  }
  return r;
}
// `#define NAME ... \` spanning several lines is one declaration (a single-line define is just a statement of its paragraph)
function cDefines(src) {
  const {m, starts, ends, kind, L} = src, out = [], cont = k => { let e = ends[k]; while (e > starts[k] && (m.charCodeAt(e - 1) === 32 || m.charCodeAt(e - 1) === 9)) e--; return e > starts[k] && m.charCodeAt(e - 1) === 92; };
  for (let k = 0; k < L; k++) {
    if (kind[k] !== 2) continue;
    const seg = m.slice(starts[k], Math.min(ends[k], starts[k] + 200)), r = /^[ \t\uFEFF]*#[ \t]*define[ \t]+([A-Za-z_]\w*)/.exec(seg);
    if (!r) continue;
    let k2 = k; while (k2 < L - 1 && cont(k2)) k2++;
    if (k2 === k) continue;
    out.push({cs: starts[k] + seg.search(/\S/), ce: ends[k2], first: attachAbove(src, k, null), last: k2, sk: 'macro', name: r[1], label: `macro ${r[1]}`, children: [], parent: null});
    k = k2;
  }
  return out;
}

// ---- assembly: a label owns the lines up to the next label; local labels (.loop, @@, inside PROC) nest in the enclosing region ----
const ASM_LABEL = /[ \t\uFEFF]*([A-Za-z_.$?@][\w.$?@]*):(?=\s|$)/y;
const ASM_PROC = /[ \t\uFEFF]*([A-Za-z_@$?][\w@$?]*)[ \t]+proc\b/iy;
const ASM_ENDP = /[ \t\uFEFF]*([A-Za-z_@$?][\w@$?]*)[ \t]+endp\b/iy;
const ASM_SECTION = /[ \t\uFEFF]*(?:section|segment|\.section|\.text|\.data|\.bss|\.rodata|\.code|\.const|\.stack|ends|end)\b/iy;
function asmScan(src) {
  const {m, starts, ends, kind, L} = src, marks = [], open = [];
  let procDepth = 0;
  for (let k = 0; k < L; k++) {
    if (kind[k] !== 2) continue;
    const a = starts[k];
    ASM_PROC.lastIndex = a; let r = ASM_PROC.exec(m);
    if (r) { procDepth++; open.push(marks.length); marks.push({k, t: 'proc', name: r[1], end: -1}); continue; }
    ASM_ENDP.lastIndex = a; r = ASM_ENDP.exec(m);
    if (r) { if (procDepth > 0) procDepth--; const o = open.pop(); if (o !== undefined) marks[o].end = k; marks.push({k, t: 'endp'}); continue; }
    ASM_LABEL.lastIndex = a; r = ASM_LABEL.exec(m);
    if (r) { marks.push({k, t: 'label', name: r[1], local: /^[.@]/.test(r[1]) || procDepth > 0}); continue; }
    ASM_SECTION.lastIndex = a;
    if (ASM_SECTION.test(m)) marks.push({k, t: 'section'});
  }
  const N = marks.length, firstOf = marks.map(x => attachAbove(src, x.k, null)), nextAny = new Int32Array(N + 1).fill(-1), nextOuter = new Int32Array(N + 1).fill(-1);
  for (let i = N - 1; i >= 0; i--) { // the next mark that ends a local label / a global label
    nextAny[i] = i + 1 < N ? i + 1 : -1;
    nextOuter[i] = i + 1 >= N ? -1 : (marks[i + 1].t === 'proc' || marks[i + 1].t === 'section' || (marks[i + 1].t === 'label' && !marks[i + 1].local)) ? i + 1 : nextOuter[i + 1];
  }
  const out = [];
  marks.forEach((x, i) => {
    if (x.t !== 'label' && x.t !== 'proc') return;
    let last;
    if (x.t === 'proc' && x.end >= 0) last = x.end;
    else { const j = x.local ? nextAny[i] : nextOuter[i]; last = j < 0 ? L - 1 : Math.max(x.k, firstOf[j] - 1); }
    while (last > x.k && kind[last] === 0) last--;
    out.push({cs: starts[x.k], ce: ends[last], first: firstOf[i], last, sk: x.t, name: x.name, label: `${x.t} ${x.name}`, children: [], parent: null});
  });
  return out;
}

const LANGS = {
  rust: {classify: rustClassify, strong: RUST_STRONG, attr: RUST_ATTR, typeBrace: '<'},
  typescript: {classify: tsClassify, strong: TS_STRONG, attr: TS_ATTR, typeBrace: ':<|&'},
  powershell: {classify: psClassify, strong: PS_STRONG, attr: PS_ATTR},
  c: {classify: cClassify, preproc: true, attr: null}
};
const SCANS = {
  rust: src => braceScan(src, LANGS.rust),
  typescript: src => braceScan(src, LANGS.typescript).concat(tsArrowScan(src)),
  powershell: src => braceScan(src, LANGS.powershell),
  c: src => braceScan(src, LANGS.c).concat(cDefines(src)),
  python: pyScan, assembly: asmScan
};
function detect(src) { return SCANS[src.profile] ? SCANS[src.profile](src) : []; }

// ---- nesting, lookup ----
function buildTree(decls) {
  decls.sort((a, b) => a.cs - b.cs || b.ce - a.ce);
  const roots = [], stack = [];
  for (const d of decls) {
    while (stack.length && d.ce > stack[stack.length - 1].ce) stack.pop();
    const parent = stack.length ? stack[stack.length - 1] : null;
    d.parent = parent;
    (parent ? parent.children : roots).push(d);
    stack.push(d);
  }
  return roots;
}
const distance = (d, off) => off < d.cs ? d.cs - off : off >= d.ce ? off - d.ce + 1 : 0;
const hasCode = (m, a, b) => { for (let i = a; i < b; i++) if (!isWs(m.charCodeAt(i))) return true; return false; };
// declarations (outermost first) whose lines contain [sl, el]; siblings that share lines are told apart by the character offset. A
// cursor in front of a declaration that starts in the middle of its line, or behind one that ends before the end of its line, is
// not in that declaration (a one-line function inside the body of another one on the same line).
function descend(src, roots, sl, el, off, cursor) {
  const path = [];
  let level = roots;
  const owns = d => {
    if (d.first > sl || el > d.last) return false;
    if (!cursor) return true;
    if (sl === lineOf(src, d.cs) && off < d.cs && hasCode(src.m, src.starts[sl], d.cs)) return false;
    if (sl === d.last && off > d.ce && hasCode(src.m, d.ce, src.ends[sl])) return false;
    return true;
  };
  for (;;) {
    let pick = null;
    for (const d of level) if (owns(d) && (!pick || distance(d, off) < distance(pick, off))) pick = d;
    if (!pick) break;
    path.push(pick); level = pick.children;
  }
  return path;
}
function entry(src, kind, first, last, d) {
  return {kind, start: src.starts[first], end: src.ends[last], startLine: first, endLine: last, label: d ? d.label : kind, symbolKind: d ? d.sk : null, symbolName: d ? d.name : null};
}

function analyse(profile, dialect, text) {
  const src = buildSource(profile, dialect, text), roots = buildTree(detect(src));
  const owner = new Int32Array(src.L);
  roots.forEach((r, idx) => { for (let k = r.first; k <= r.last; k++) owner[k] = idx + 1; });
  return {src, roots, owner};
}

function resolveIn(an, s, e, maxChars, partial) {
  const {src} = an;
  let base, path;
  if (s < e && /\S/.test(src.text.slice(s, e))) {
    const sl = lineOf(src, s); let el = lineOf(src, e);
    if (el > sl && e === src.starts[el]) el--; // dragging down whole lines leaves the end at column 0 of the next line
    base = entry(src, 'selection', sl, el);
    path = descend(src, an.roots, sl, el, s, false);
  } else {
    const l = lineOf(src, s);
    path = descend(src, an.roots, l, l, s, true);
    if (path.length) { const d = path[path.length - 1]; base = entry(src, 'declaration', d.first, d.last, d); }
    else {
      let t = l;
      if (src.kind[t] === 0) { // a blank line: the nearest paragraph above, else below
        let u = t - 1; while (u >= 0 && src.kind[u] === 0) u--;
        if (u < 0) { u = t + 1; while (u < src.L && src.kind[u] === 0) u++; }
        t = Math.min(u, src.L - 1);
      }
      const o = an.owner[t];
      if (o) { const r = an.roots[o - 1]; base = entry(src, 'block', r.first, r.last); }
      else {
        let a = t, b = t;
        while (a > 0 && src.kind[a - 1] !== 0) a--;
        while (b < src.L - 1 && src.kind[b + 1] !== 0) b++;
        // never cut a declaration in half: one that has blank lines inside and crosses a border of the paragraph stays out of it
        // (a declaration that lies wholly inside the paragraph is part of it)
        for (let moved = true; moved;) {
          moved = false;
          const ra = an.owner[a] ? an.roots[an.owner[a] - 1] : null, rb = an.owner[b] ? an.roots[an.owner[b] - 1] : null;
          if (ra && ra.first < a && ra.last < t) { a = ra.last + 1; moved = true; }
          if (rb && rb.last > b && rb.first > t) { b = rb.first - 1; moved = true; }
        }
        base = entry(src, 'block', Math.min(a, t), Math.max(b, t));
      }
      path = [];
    }
  }
  const chain = [base];
  const widen = x => { const p = chain[chain.length - 1]; if (x.start <= p.start && x.end >= p.end && (x.start < p.start || x.end > p.end)) chain.push(x); };
  for (let i = path.length - 1; i >= 0; i--) widen(entry(src, 'declaration', path[i].first, path[i].last, path[i]));
  if (!partial) widen(entry(src, 'file', 0, src.L > 1 && src.starts[src.L - 1] === src.n ? src.L - 2 : src.L - 1));
  let k = 1;
  while (k < chain.length && chain[k].end - chain[k].start <= maxChars) k++;
  chain.length = k;
  return {ok: true, ...base, chain, tooLarge: base.end - base.start > maxChars};
}

// ---- texts above FULL_SCAN_MAX: only whole lines around the selection are analysed (declarations that begin outside the window
// are not seen, and no 'file' entry is offered), so the cost stays bounded whatever the size of the document ----
const lineStartAt = (text, off) => off <= 0 ? 0 : Math.max(text.lastIndexOf('\n', off - 1), text.lastIndexOf('\r', off - 1)) + 1;
function lineEndAt(text, off) {
  const a = text.indexOf('\n', off), b = text.indexOf('\r', off);
  return a < 0 ? (b < 0 ? text.length : b) : (b < 0 ? a : Math.min(a, b));
}
function countEols(text, upto) {
  let n = 0;
  for (let i = 0; i < upto; i++) { const c = text.charCodeAt(i); if (c === 10) n++; else if (c === 13) { if (text.charCodeAt(i + 1) === 10) i++; n++; } }
  return n;
}
function resolveHuge(profile, dialect, text, s, e, max) {
  const n = text.length, selected = s < e && /\S/.test(text.slice(s, e));
  if (!selected) e = s;
  const own = (kind, st, en) => { const lines = countEols(text, st); return {kind, start: st, end: en, startLine: lines, endLine: lines + countEols(text.slice(st, en), en - st), label: kind, symbolKind: null, symbolName: null}; };
  const lo = lineStartAt(text, Math.max(0, s - HUGE_WINDOW)), hi = lineEndAt(text, Math.min(n, e + HUGE_WINDOW));
  if (hi - lo > HARD_CAP) { // a selection as large as the cap itself: nothing to analyse, just report its lines
    const st = lineStartAt(text, s);
    let en = lineEndAt(text, e);
    if (e > st && e === lineStartAt(text, e)) en = Math.max(st, text.charCodeAt(e - 1) === 10 && text.charCodeAt(e - 2) === 13 ? e - 2 : e - 1); // ends at column 0: that line is not included
    const sel = own('selection', st, en);
    return {ok: true, ...sel, chain: [sel], tooLarge: en - st > max};
  }
  const win = text.slice(lo, hi);
  if (!/\S/.test(win)) { // a blank neighbourhood in a document that is not blank: the cursor line itself
    const blk = own('block', lineStartAt(text, s), lineEndAt(text, s));
    return {ok: true, ...blk, chain: [blk], tooLarge: blk.end - blk.start > max};
  }
  const r = resolveIn(analyse(profile, dialect, win), s - lo, e - lo, max, lo > 0 || hi < n), lines = countEols(text, lo);
  const fix = x => ({...x, start: x.start + lo, end: x.end + lo, startLine: x.startLine + lines, endLine: x.endLine + lines});
  const out = {...fix(r), chain: r.chain.map(fix)};
  if (n <= max && (lo > 0 || hi < n)) { // the whole document fits after all: it is the last widening step
    let fe = n; if (text.charCodeAt(fe - 1) === 10) fe -= text.charCodeAt(fe - 2) === 13 ? 2 : 1; else if (text.charCodeAt(fe - 1) === 13) fe--;
    const last = out.chain[out.chain.length - 1];
    if (last.start >= 0 && last.end <= fe && (last.start > 0 || last.end < fe)) out.chain.push({...own('file', 0, fe)});
  }
  return out;
}

function resolveScope({profile, dialect = {}, text, start, end, maxChars = DEFAULT_MAX_CHARS} = {}) {
  if (!PROFILES.includes(profile)) return {ok: false, reason: 'unsupported-language'};
  if (typeof text !== 'string' || !/\S/.test(text)) return {ok: false, reason: 'empty-document'};
  const n = text.length;
  let s = clampOff(start, n), e = clampOff(end === undefined ? s : end, n);
  if (s > e) [s, e] = [e, s];
  const max = typeof maxChars === 'number' && maxChars >= 0 ? maxChars : DEFAULT_MAX_CHARS; // (NaN and negative numbers fail the comparison; Infinity means no limit)
  const d = dialect && typeof dialect === 'object' ? dialect : {};
  if (n > FULL_SCAN_MAX) return resolveHuge(profile, d, text, s, e, max);
  return resolveIn(analyse(profile, d, text), s, e, max, false);
}

module.exports = {resolveScope, PROFILES, HARD_CAP, _analyse: analyse, _resolveIn: resolveIn};
