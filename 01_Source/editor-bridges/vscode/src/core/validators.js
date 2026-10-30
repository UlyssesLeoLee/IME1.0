'use strict';
// Lightweight structural validators (FR-038, DD §5.2). They are NOT compilers or full parsers: "valid" means
// "no structural problem found by these checks". Source is treated purely as text; nothing is executed.
const REG64 = 'rax|rbx|rcx|rdx|rsi|rdi|rbp|rsp|r(?:[89]|1[0-5])';
const REG32 = 'eax|ebx|ecx|edx|esi|edi|ebp|esp|r(?:[89]|1[0-5])d';
const REG16_8 = 'ax|bx|cx|dx|si|di|bp|sp|al|bl|cl|dl|ah|bh|ch|dh|sil|dil|bpl|spl';
const REG_RE = new RegExp(`(?<![\\w%$.])(?:${REG64}|${REG32}|${REG16_8})(?![\\w])`, 'i');
const ATT_REG_RE = new RegExp(`%(?:${REG64}|${REG32}|${REG16_8})\\b`, 'i');

function scanQuoted(text, i, quote, escape) {
  let j = i + 1;
  while (j < text.length) {
    const c = text[j];
    if (escape === 'backslash' && c === '\\') { j += 2; continue; }
    if (escape === 'backtick' && c === '`') { j += 2; continue; }
    if (c === quote) {
      if ((escape === 'doubling' || escape === 'backtick') && text[j + 1] === quote) { j += 2; continue; }
      return j + 1;
    }
    j++;
  }
  return text.length;
}
const lineEnd = (text, i) => { const n = text.indexOf('\n', i); return n < 0 ? text.length : n; };

// Replace comments and literals by blanks (newlines kept) so that bracket/indent logic only sees code.
// Optional `spans` receives {start, end, kind: 'comment' | 'string'} for every blanked region (behaviour is unchanged).
function maskSource(text, style, spans) {
  const out = text.split('');
  const blank = (a, b) => { for (let k = a; k < b && k < out.length; k++) if (out[k] !== '\n' && out[k] !== '\r') out[k] = ' '; };
  const n = text.length;
  let i = 0;
  while (i < n) {
    const c = text[i], d = text[i + 1];
    let end = -1, kind = 'string';
    if (style === 'python') {
      if (c === '#') { end = lineEnd(text, i); kind = 'comment'; }
      else if (c === '"' || c === "'") {
        if (text.startsWith(c.repeat(3), i)) { const e = text.indexOf(c.repeat(3), i + 3); end = e < 0 ? n : e + 3; }
        else end = scanQuoted(text, i, c, 'backslash');
      }
    } else if (style === 'powershell') {
      if (c === '<' && d === '#') { const e = text.indexOf('#>', i + 2); end = e < 0 ? n : e + 2; kind = 'comment'; }
      else if (c === '#') { end = lineEnd(text, i); kind = 'comment'; }
      else if (c === '@' && (d === '"' || d === "'") && /^[ \t]*\r?$/.test(text.slice(i + 2, lineEnd(text, i)))) {
        const closer = new RegExp(`\\n${d}@`); const m = closer.exec(text.slice(i)); end = m ? i + m.index + 3 : n;
      }
      else if (c === "'") end = scanQuoted(text, i, "'", 'doubling');
      else if (c === '"') end = scanQuoted(text, i, '"', 'backtick');
    } else { // rust | c | typescript
      if (c === '/' && d === '/') { end = lineEnd(text, i); kind = 'comment'; }
      else if (c === '/' && d === '*') {
        kind = 'comment';
        let depth = 1, j = i + 2;
        while (j < n && depth > 0) {
          if (style === 'rust' && text[j] === '/' && text[j + 1] === '*') { depth++; j += 2; }
          else if (text[j] === '*' && text[j + 1] === '/') { depth--; j += 2; }
          else j++;
        }
        end = j;
      } else if (style === 'rust' && (c === 'r' || c === 'b') && !/[\w]/.test(text[i - 1] || ' ')) {
        const m = /^b?r(#*)"/.exec(text.slice(i, i + 40));
        if (m) { const e = text.indexOf('"' + m[1], i + m[0].length); end = e < 0 ? n : e + 1 + m[1].length; }
      }
      if (end < 0 && c === '"') end = scanQuoted(text, i, '"', 'backslash');
      else if (end < 0 && c === '`' && style === 'typescript') end = scanQuoted(text, i, '`', 'backslash');
      else if (end < 0 && c === "'") {
        if (style === 'rust') {
          const m = /^'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|.)|[^\\'\n])'/.exec(text.slice(i, i + 14));
          end = m ? i + m[0].length : -1; // otherwise a lifetime such as 'a
        } else end = scanQuoted(text, i, "'", 'backslash');
      }
    }
    if (end > i) { if (spans) spans.push({start: i, end, kind}); blank(i, end); i = end; } else i++;
  }
  return out.join('');
}

const OPEN = {'(': ')', '[': ']', '{': '}'};
function bracketIssues(masked) {
  const issues = [], stack = [];
  let line = 1;
  for (const c of masked) {
    if (c === '\n') line++;
    else if (OPEN[c]) stack.push({c, line});
    else if (c === ')' || c === ']' || c === '}') {
      const top = stack.pop();
      if (!top) { issues.push({line, code: 'BRACKET_UNEXPECTED', message: `unexpected '${c}'`}); if (issues.length >= 20) break; }
      else if (OPEN[top.c] !== c) { issues.push({line, code: 'BRACKET_MISMATCH', message: `'${top.c}' from line ${top.line} closed by '${c}'`}); if (issues.length >= 20) break; }
    }
  }
  for (const s of stack) { if (issues.length < 20) issues.push({line: s.line, code: 'BRACKET_UNCLOSED', message: `'${s.c}' is never closed`}); }
  return issues;
}

function pythonIssues(text) {
  const masked = maskSource(text, 'python');
  const lines = text.split(/\r\n|\r|\n/), mlines = masked.split(/\r\n|\r|\n/);
  const issues = [], stack = [0];
  let depth = 0, opener = false, cont = false, openerLine = 0, sawTab = false, sawSpace = false;
  for (let k = 0; k < lines.length; k++) {
    const m = mlines[k];
    if (m.trim() === '') continue;
    const lead = /^[ \t]*/.exec(lines[k])[0];
    if (lead.includes(' ') && lead.includes('\t')) issues.push({line: k + 1, code: 'TAB_SPACE_MIX', message: 'tabs and spaces mixed in one indentation'});
    if (lead.includes('\t')) sawTab = true;
    if (lead.includes(' ')) sawSpace = true;
    const indent = lead.length, top = stack[stack.length - 1];
    if (depth === 0 && !cont) {
      if (opener) {
        if (indent <= top) issues.push({line: openerLine, code: 'EXPECTED_INDENT', message: 'expected an indented block'});
        else stack.push(indent);
      } else if (indent > top) issues.push({line: k + 1, code: 'UNEXPECTED_INDENT', message: 'unexpected indent'});
      else if (indent < top) {
        while (stack.length > 1 && stack[stack.length - 1] > indent) stack.pop();
        if (stack[stack.length - 1] !== indent) issues.push({line: k + 1, code: 'UNINDENT_MISMATCH', message: 'unindent does not match any outer indentation level'});
      }
    }
    for (const c of m) { if (OPEN[c]) depth++; else if (c === ')' || c === ']' || c === '}') depth = Math.max(0, depth - 1); }
    const trimmed = m.replace(/\s+$/, '');
    cont = trimmed.endsWith('\\');
    opener = depth === 0 && !cont && trimmed.endsWith(':');
    if (opener) openerLine = k + 1;
  }
  if (opener) issues.push({line: openerLine, code: 'EXPECTED_INDENT', message: 'expected an indented block'});
  if (sawTab && sawSpace && !issues.some(x => x.code === 'TAB_SPACE_MIX')) issues.push({line: 0, code: 'TAB_SPACE_MIX', message: 'file mixes tab-indented and space-indented lines'});
  return issues.concat(bracketIssues(masked));
}

const PS7_ONLY = [
  [/\?\?=?/, 'PS7_NULL_COALESCING', 'null-coalescing operators need PowerShell 7+'],
  [/\?\.|\?\[/, 'PS7_NULL_CONDITIONAL', 'null-conditional access needs PowerShell 7+'],
  [/&&|\|\|/, 'PS7_PIPELINE_CHAIN', 'pipeline chain operators need PowerShell 7+'],
  [/-Parallel\b/i, 'PS7_PARALLEL', 'ForEach-Object -Parallel needs PowerShell 7+'],
  [/(?<!\|)\s\S[^?\n]*\s\?\s+\S[^:\n]*\s:\s+\S/, 'PS7_TERNARY', 'ternary operator needs PowerShell 7+']
];
function powershellIssues(text, dialect = {}) {
  const masked = maskSource(text, 'powershell');
  const issues = bracketIssues(masked);
  if ((dialect.version || '7') === '5.1') {
    masked.split(/\r\n|\r|\n/).forEach((line, idx) => {
      for (const [re, code, message] of PS7_ONLY) {
        if (code === 'PS7_TERNARY' && /\|\s*\?\s*\{/.test(line)) continue; // `? {}` is the Where-Object alias
        if (re.test(line)) issues.push({line: idx + 1, code, message});
      }
    });
  }
  return issues;
}

const C23_ONLY = [
  [/\bnullptr\b/, 'C23_NULLPTR', 'nullptr needs C23'],
  [/\bconstexpr\b/, 'C23_CONSTEXPR', 'constexpr needs C23'],
  [/\btypeof(?:_unqual)?\b/, 'C23_TYPEOF', 'typeof needs C23'],
  [/\[\[/, 'C23_ATTRIBUTE', '[[attribute]] syntax needs C23'],
  [/^\s*#\s*embed\b/, 'C23_EMBED', '#embed needs C23'],
  [/\b0[bB][01]+\b/, 'C23_BINARY_LITERAL', 'binary literals need C23'],
  [/\b_BitInt\b/, 'C23_BITINT', '_BitInt needs C23']
];
function cIssues(text, dialect = {}) {
  const masked = maskSource(text, 'c');
  const issues = bracketIssues(masked);
  if ((dialect.std || 'c17') !== 'c23') {
    masked.split(/\r\n|\r|\n/).forEach((line, idx) => {
      for (const [re, code, message] of C23_ONLY) if (re.test(line)) issues.push({line: idx + 1, code, message});
    });
  }
  return issues;
}
function tsIssues(text, dialect = {}) {
  const masked = maskSource(text, 'typescript');
  const issues = bracketIssues(masked);
  if (dialect.mode === 'tsx') {
    masked.split(/\r\n|\r|\n/).forEach((line, idx) => {
      if (/(?:=|\()\s*<[A-Za-z_]\w*>\s*\(/.test(line)) issues.push({line: idx + 1, code: 'TSX_GENERIC_AMBIGUOUS', message: 'a bare <T> generic arrow is parsed as JSX in TSX; write <T,>'});
    });
  }
  return issues;
}

// Intel vs AT&T operand syntax must not be mixed silently (UT-022).
const ASM_DIRECTIVE_NASM = /^\s*(?:section|global|extern|bits|default)\b/i;
const ASM_DIRECTIVE_GAS = /^\s*\.(?:text|data|bss|globl|global|section|intel_syntax|att_syntax)\b/i;
function assemblyIssues(text, dialect = {}) {
  const issues = [];
  let syntax = dialect.syntax || 'intel';
  const assembler = dialect.assembler || 'nasm';
  const seen = new Set();
  text.split(/\r\n|\r|\n/).forEach((raw, idx) => {
    const line = raw.replace(assembler === 'gas' ? /#.*$|\/\/.*$/ : /;.*$/, '').trim();
    if (!line) return;
    if (assembler === 'gas') {
      if (/^\.intel_syntax\b/i.test(line)) { syntax = 'intel'; return; }
      if (/^\.att_syntax\b/i.test(line)) { syntax = 'att'; return; }
      if (ASM_DIRECTIVE_NASM.test(line) && !ASM_DIRECTIVE_GAS.test(line)) issues.push({line: idx + 1, code: 'ASM_DIRECTIVE_MISMATCH', message: 'NASM/MASM directive in a GAS file'});
    } else if (assembler === 'nasm' && ASM_DIRECTIVE_GAS.test(line)) issues.push({line: idx + 1, code: 'ASM_DIRECTIVE_MISMATCH', message: 'GAS directive in a NASM file'});
    const hasAtt = ATT_REG_RE.test(line) || /(?:^|[\s,])\$-?(?:0x[0-9a-f]+|\d+)\b/i.test(line);
    const stripped = line.replace(/^[\w.$]+:\s*/, '').replace(ATT_REG_RE, '');
    const hasIntel = (/\[[^\]]+\]/.test(stripped) || REG_RE.test(stripped.replace(/^\S+\s*/, '')));
    if (hasAtt) seen.add('att'); if (hasIntel) seen.add('intel');
    if (syntax === 'intel' && hasAtt) issues.push({line: idx + 1, code: 'ASM_SYNTAX_MIX', message: 'AT&T operand (%reg or $imm) in Intel syntax'});
    if (syntax === 'att' && hasIntel) issues.push({line: idx + 1, code: 'ASM_SYNTAX_MIX', message: 'Intel operand (bare register or [mem]) in AT&T syntax'});
  });
  if (seen.size > 1 && !issues.length) issues.push({line: 0, code: 'ASM_SYNTAX_MIX', message: 'Intel and AT&T syntax mixed in one file'});
  return issues;
}
function hexIssues(text) {
  const issues = [];
  text.split(/\r\n|\r|\n/).forEach((line, idx) => {
    const t = line.replace(/\/\/.*$|;.*$/, '').trim();
    if (!t) return;
    const compact = t.replace(/0x/gi, '').replace(/[\s,]+/g, '');
    if (!/^[0-9a-fA-F]+$/.test(compact)) issues.push({line: idx + 1, code: 'INVALID_HEX', message: 'non-hex character'});
    else if (compact.length % 2) issues.push({line: idx + 1, code: 'ODD_HEX_LENGTH', message: 'incomplete byte'});
  });
  return issues;
}

const VALIDATOR_BY_PROFILE = Object.freeze({
  rust: text => bracketIssues(maskSource(text, 'rust')),
  python: pythonIssues, typescript: tsIssues, powershell: powershellIssues, c: cIssues,
  assembly: assemblyIssues, 'machine-code': hexIssues
});
function validateSource(profileId, text, dialect = {}) {
  const fn = VALIDATOR_BY_PROFILE[profileId];
  if (!fn) return {ok: false, level: 'unchecked', issues: [{line: 0, code: 'NO_VALIDATOR', message: 'no validator for profile'}]};
  if (typeof text !== 'string') return {ok: false, level: 'invalid', issues: [{line: 0, code: 'NOT_TEXT', message: 'text expected'}]};
  const issues = fn(text, dialect);
  return {ok: issues.length === 0, level: issues.length ? 'invalid' : 'valid', issues};
}
module.exports = {maskSource, bracketIssues, validateSource, pythonIssues, powershellIssues, cIssues, tsIssues, assemblyIssues, hexIssues};
