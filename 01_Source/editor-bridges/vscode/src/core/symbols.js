'use strict';
// Heuristic symbol index (names only) used as read-only graph context for the Cypher annotation. Not a parser:
// it recognises common top-level declarations per language so that the AI can refer to symbols that really exist.
// Only {kind, name} pairs ever leave the machine - never the code of other files.
const {maskSource} = require('./validators');

const MAX_SCAN = 200 * 1024;
const NAME = '([A-Za-z_][\\w$]*)';
const RULES = Object.freeze({
  rust: [
    [new RegExp(`^\\s*(?:pub(?:\\([^)]*\\))?\\s+)?(?:async\\s+|unsafe\\s+|const\\s+|extern\\s+"[^"]*"\\s+)*(fn|struct|enum|trait|mod|type|static)\\s+${NAME}`), m => [m[1], m[2]]],
    [new RegExp(`^\\s*impl(?:<[^>]*>)?\\s+(?:[\\w:<>,\\s&']+?\\s+for\\s+)?${NAME}`), m => ['impl', m[1]]]
  ],
  python: [[new RegExp(`^\\s*(?:async\\s+)?(def|class)\\s+${NAME}`), m => [m[1], m[2]]]],
  typescript: [
    [new RegExp(`^\\s*(?:export\\s+)?(?:default\\s+)?(?:declare\\s+)?(?:abstract\\s+)?(?:async\\s+)?(function|class|interface|enum|type|namespace)\\s+${NAME}`), m => [m[1], m[2]]],
    [new RegExp(`^\\s*(?:export\\s+)?const\\s+${NAME}\\s*(?::[^=]+)?=\\s*(?:async\\s*)?(?:\\([^)]*\\)|[A-Za-z_]\\w*)\\s*(?::[^=]+)?=>`), m => ['function', m[1]]]
  ],
  powershell: [[new RegExp('^\\s*(?:filter|function)\\s+([\\w-]+)', 'i'), m => ['function', m[1]]]],
  c: [
    [new RegExp(`^\\s*(?:typedef\\s+)?(struct|enum|union)\\s+${NAME}`), m => [m[1], m[2]]],
    [new RegExp(`^(?!\\s*(?:if|for|while|switch|return|else|do|case|sizeof)\\b)[A-Za-z_][\\w\\s\\*]*?[\\s\\*]${NAME}\\s*\\([^;]*\\)\\s*\\{?\\s*$`), m => ['function', m[1]]],
    [new RegExp(`^\\s*#\\s*define\\s+${NAME}`), m => ['macro', m[1]]]
  ],
  assembly: [[new RegExp('^\\s*([A-Za-z_.$][\\w.$]*):'), m => ['label', m[1]]]]
});

// -> [{kind, name}] in file order, de-duplicated, at most `max` entries
function extractSymbols(profile, text, max = 60) {
  const rules = RULES[profile];
  if (!rules || typeof text !== 'string') return [];
  const sample = text.length > MAX_SCAN ? text.slice(0, MAX_SCAN) : text;
  // comments and string literals must not produce symbols
  const style = {rust: 'rust', c: 'c', typescript: 'typescript', python: 'python', powershell: 'powershell'}[profile];
  const code = style ? maskSource(sample, style) : sample;
  const seen = new Set(), out = [];
  for (const line of code.split(/\r\n|\r|\n/)) {
    for (const [re, pick] of rules) {
      const m = re.exec(line);
      if (!m) continue;
      const [kind, name] = pick(m);
      const key = `${kind}:${name}`;
      if (!seen.has(key)) { seen.add(key); out.push({kind, name}); }
      break;
    }
    if (out.length >= max) break;
  }
  return out;
}
module.exports = {extractSymbols, MAX_SCAN};
