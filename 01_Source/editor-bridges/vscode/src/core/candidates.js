'use strict';
// The mirror of the TASE input method core (01_Source/crates/tase-ime-core). The Rust crate is the ONLY truth source
// for the candidate order, the score parts and the privacy decisions; this file has to give the same order, the same
// parts and the same refusal reasons for the same input. The two parity tests read the same fixture
// (01_Source/tests/fixtures/tase-ranking-cases.json): tests/js/ut-tase-candidates.test.js runs this file and
// tests/rust/parity_tase_ime.rs runs the crate, so a change on one side has to be made on the other.
//
// Mirrored: candidate.rs (the candidate model), context.rs (the context kind, the language hint, the focus
// generation), rank.rs (the four score parts, the weights and the stable order) and policy.rs (the decisions).
// The design references are DD-IME sections 20.2 / 20.3 / 20.4 and RD-IME section 15.
//
// The rule pack candidates of the bridge (core/languages.js) are a DIFFERENT kind of candidate: a code snippet with a
// pin, a weight and a validation state, ordered by DD §4.2. They keep that order; the ranking below is the order of
// the asset cards of the catalogue (DD-IME 20.4).
const {fail} = require('./errors');

// ---------------------------------------------------------------- the weights of rank.rs

// The score of a candidate whose language is the one the user asked for.
const LANGUAGE_MATCH = 1.0;
// The score of a candidate whose language is another one.
const LANGUAGE_MISMATCH = -1.0;
// The weight of the segments of the asset id in the applicability.
const APPLICABILITY_SEGMENT_WEIGHT = 0.6;
// The weight of the kind of the asset (`pattern`, `algorithm`) in the applicability.
const APPLICABILITY_KIND_WEIGHT = 0.4;
// The penalty of a candidate that changes the whole project (it goes to the IDE service for a preview).
const RISK_PENALTY_COMPLETE_PROJECT_EDIT = -0.5;
// The penalty of a candidate that carries a note about its risk.
const RISK_PENALTY_NOTED = -0.1;

// The budget of the local cache candidates: `p95 <= 50ms` (DD-IME 20.2).
const LOCAL_CACHE_P95_BUDGET_MS = 50;
// The ways out the input method is not allowed to take (DD-IME 20.2).
const FORBIDDEN_SHORTCUTS = Object.freeze([
  'keyboard_simulation', 'process_injection', 'clipboard_read', 'screen_read'
]);

// The reason of the refusal in a password control.
const REASON_PASSWORD = 'password: the candidates are disabled in a password control';
// The reason of the refusal on the secure desktop.
const REASON_SECURE_DESKTOP = 'secure_desktop: the candidates are disabled on the secure desktop';
// The reason of the refusal in an elevation prompt.
const REASON_UAC_PROMPT = 'uac_prompt: the candidates are disabled in an elevation prompt';
// The reason of the refusal in a restricted AppContainer.
const REASON_APP_CONTAINER = 'app_container: the candidates are disabled in a restricted AppContainer';
// The reason of the refusal in a secure input field of a browser.
const REASON_BROWSER_SECURE_INPUT = 'browser_secure_input: the candidates are disabled in a secure input field of a browser';

// ---------------------------------------------------------------- the kinds of context.rs

// Every kind of context (context.rs: ContextKind::ALL).
const CONTEXT_KINDS = Object.freeze([
  'plain_text', 'code', 'password', 'secure_desktop', 'uac_prompt', 'app_container', 'browser_secure_input', 'unknown'
]);
// The five protected kinds: nothing is read and nothing is shown in them.
const SENSITIVE_KINDS = Object.freeze([
  'password', 'secure_desktop', 'uac_prompt', 'app_container', 'browser_secure_input'
]);
// Every status of a candidate (candidate.rs: CandidateStatus::ALL).
const CANDIDATE_STATUSES = Object.freeze(['CANDIDATE_ONLY', 'IMPLEMENTATION_AVAILABLE', 'CONFIRMATION_REQUIRED']);
// The three decisions of policy.rs.
const DECISION_ALLOW = 'allow';
const DECISION_ALLOW_READ_ONLY = 'allow_read_only';
const DECISION_DENY = 'deny';

// The largest generation: the number saturates there instead of wrapping (a wrapped number would look like an old
// generation and would let a stale candidate through).
const MAX_GENERATION = 18446744073709551615n;

// ---------------------------------------------------------------- the small helpers

// Rust `char::is_alphanumeric` is the Unicode property `Alphabetic` OR the general categories Nd, Nl and No.
const ALPHANUMERIC = /[\p{Alphabetic}\p{Number}]/u;
const NON_ALPHANUMERIC = /[^\p{Alphabetic}\p{Number}]+/u;
// Rust `str::trim` removes the characters of the Unicode property `White_Space`.
const WHITE_SPACE = /^\p{White_Space}+|\p{White_Space}+$/gu;

// The words of a text: the runs of alphanumeric characters, in lower case (rank.rs: tokenize).
function tokenize(text) {
  const out = [];
  for (const part of String(text === null || text === undefined ? '' : text).split(NON_ALPHANUMERIC)) {
    if (part !== '') out.push(part.toLowerCase());
  }
  return out;
}

// Rust `str::trim` (the Unicode property `White_Space`).
function trim(text) {
  return String(text === null || text === undefined ? '' : text).replace(WHITE_SPACE, '');
}

// Rust `str::eq_ignore_ascii_case`: only the ASCII letters are folded, the rest is compared as it is.
function eqIgnoreAsciiCase(a, b) {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const x = a.charCodeAt(i), y = b.charCodeAt(i);
    if (x === y) continue;
    const fx = x >= 65 && x <= 90 ? x + 32 : x;
    const fy = y >= 65 && y <= 90 ? y + 32 : y;
    if (fx !== fy) return false;
  }
  return true;
}

// Rust `String::cmp`: the bytes of the UTF-8 form are compared. The UTF-8 order is the order of the code points, so
// the code points are compared here (a lone surrogate cannot reach the Rust side at all).
function compareIds(a, b) {
  if (a === b) return 0;
  const left = a[Symbol.iterator](), right = b[Symbol.iterator]();
  for (;;) {
    const x = left.next(), y = right.next();
    if (x.done && y.done) return 0;
    if (x.done) return -1;
    if (y.done) return 1;
    const cx = x.value.codePointAt(0), cy = y.value.codePointAt(0);
    if (cx !== cy) return cx < cy ? -1 : 1;
  }
}

// Rust `f64::round`: half away from zero (JavaScript rounds half up, so the negative half is handled apart).
function roundHalfAwayFromZero(value) {
  return value < 0 ? -Math.round(-value) : Math.round(value);
}

// A score rounded to the resolution of the comparison (rank.rs: round9).
function round9(value) {
  return roundHalfAwayFromZero(value * 1e9) / 1e9;
}

// The order of two scores, rounded to the resolution of the comparison (rank.rs: compare_scores).
function compareScores(a, b) {
  const x = round9(a), y = round9(b);
  if (Number.isNaN(x) || Number.isNaN(y)) return 0;
  return x < y ? -1 : x > y ? 1 : 0;
}

// ---------------------------------------------------------------- candidate.rs

// The reference of an asset: the id of the asset and the version it is at, joined by `@`.
function parseAssetRef(text) {
  if (typeof text !== 'string') return null;
  const trimmed = text.trim();
  if (trimmed === '') return null;
  const at = trimmed.indexOf('@');
  if (at < 0) return Object.freeze({id: trimmed, version: null});
  const id = trimmed.slice(0, at).trim();
  const version = trimmed.slice(at + 1).trim();
  // a second `@` means the text is not a reference at all
  if (id === '' || version === '' || version.includes('@')) return null;
  return Object.freeze({id, version});
}

// The reference as the text of the contract (`id@version` or `id`).
function assetRefString(ref) {
  if (!ref) return '';
  return ref.version === null || ref.version === undefined ? ref.id : `${ref.id}@${ref.version}`;
}

// The segments of the id (`pattern.gof.behavioral.strategy` -> `pattern`, `gof`, `behavioral`, `strategy`).
function assetRefSegments(ref) {
  if (!ref) return [];
  return ref.id.split('.').filter(segment => segment !== '');
}

// The kind of the asset (the first segment of the id).
function assetRefKind(ref) {
  const segments = assetRefSegments(ref);
  return segments.length ? segments[0] : null;
}

// The category of the asset (the second segment of the id).
function assetRefCategory(ref) {
  const segments = assetRefSegments(ref);
  return segments.length > 1 ? segments[1] : null;
}

// A candidate of an asset: the model of candidate.rs, with the same defaults and the same refusals.
function makeCandidate(value) {
  const v = value && typeof value === 'object' ? value : {};
  if (typeof v.candidate_id !== 'string') throw fail('OUTPUT_INVALID', 'the candidate has no candidate_id');
  if (typeof v.asset_ref !== 'string') throw fail('OUTPUT_INVALID', 'the candidate has no asset_ref');
  const assetRef = parseAssetRef(v.asset_ref);
  if (!assetRef) throw fail('OUTPUT_INVALID', 'the asset_ref is not an id@version reference');
  let implementationRef = null;
  if (v.implementation_ref !== undefined && v.implementation_ref !== null) {
    if (typeof v.implementation_ref !== 'string') throw fail('OUTPUT_INVALID', 'the implementation_ref is not a string');
    implementationRef = parseAssetRef(v.implementation_ref);
    if (!implementationRef) throw fail('OUTPUT_INVALID', 'the implementation_ref is not an id@version reference');
  }
  const text = key => {
    if (v[key] === undefined) return '';
    if (typeof v[key] !== 'string') throw fail('OUTPUT_INVALID', 'the candidate shape');
    return v[key];
  };
  const flag = key => {
    if (v[key] === undefined) return false;
    if (typeof v[key] !== 'boolean') throw fail('OUTPUT_INVALID', 'the candidate shape');
    return v[key];
  };
  let risk = null;
  if (v.risk !== undefined && v.risk !== null) {
    if (typeof v.risk !== 'string') throw fail('OUTPUT_INVALID', 'the risk is not a string');
    risk = v.risk;
  }
  let status = CANDIDATE_STATUSES[0];
  if (v.status !== undefined) {
    if (!CANDIDATE_STATUSES.includes(v.status)) throw fail('OUTPUT_INVALID', 'the status of the candidate is unknown');
    status = v.status;
  }
  return Object.freeze({
    candidate_id: v.candidate_id,
    asset_ref: assetRef,
    implementation_ref: implementationRef,
    language: text('language'),
    label: text('label'),
    insert_preview: text('insert_preview'),
    selection_requires_confirmation: flag('selection_requires_confirmation'),
    complete_project_edit: flag('complete_project_edit'),
    risk,
    status
  });
}

// ---------------------------------------------------------------- context.rs

// The kind of a name; an unknown name is refused.
function parseContextKind(name) {
  return CONTEXT_KINDS.includes(name) ? name : null;
}

// Whether the candidates must be disabled in this kind of context.
function isSensitiveKind(kind) {
  return SENSITIVE_KINDS.includes(kind);
}

// The generation as the Rust `u64` it is: the number is a BigInt, so it never loses precision.
function toGeneration(value) {
  if (typeof value === 'bigint') return value;
  if (typeof value === 'number') {
    if (!Number.isInteger(value) || value < 0) throw fail('OUTPUT_INVALID', 'the focus generation is not a whole number');
    return BigInt(value);
  }
  if (typeof value === 'string' && /^\d+$/.test(value)) return BigInt(value);
  throw fail('OUTPUT_INVALID', 'the focus generation is not a whole number');
}

// The context the user is typing in.
function makeContext({kind, language_hint = null, focus_context_generation = 0, excerpt = null} = {}) {
  if (!parseContextKind(kind)) throw fail('OUTPUT_INVALID', 'the context kind is unknown');
  return Object.freeze({
    kind,
    language_hint: language_hint === null || language_hint === undefined ? null : String(language_hint),
    focus_context_generation: toGeneration(focus_context_generation),
    excerpt: excerpt === null || excerpt === undefined ? null : String(excerpt)
  });
}

// The generation the candidates of this context belong to.
function contextGeneration(context) {
  return context.focus_context_generation;
}

// Whether the candidates of this context are stale (DD-IME 20.2).
function contextIsStale(context, currentGeneration) {
  return toGeneration(currentGeneration) !== context.focus_context_generation;
}

// The next generation: the context changed, so the old candidates are dropped. The excerpt goes with the old focus.
function advanceContextGeneration(context) {
  const next = context.focus_context_generation >= MAX_GENERATION ? MAX_GENERATION : context.focus_context_generation + 1n;
  return Object.freeze({...context, focus_context_generation: next, excerpt: null});
}

// The excerpt of the context, when it has one.
function contextExcerpt(context) {
  return context.excerpt;
}

// The language of the context, when it has one.
function contextLanguage(context) {
  return context.language_hint;
}

// ---------------------------------------------------------------- rank.rs

// What the ranking is asked to do.
function makeRankRequest({query = '', language = null, context = null, now_ms = 0} = {}) {
  return Object.freeze({
    query: query === null || query === undefined ? '' : String(query),
    language: language === null || language === undefined ? null : String(language),
    context: context || makeContext({kind: 'unknown'}),
    now_ms: Number(now_ms) || 0
  });
}

// The language the ranking compares against: the one of the request, or the hint of the context.
function requestLanguage(request) {
  return request.language !== null && request.language !== undefined ? request.language : contextLanguage(request.context);
}

// The language of the candidate against the language of the request.
function languageMatch(candidate, request) {
  const language = requestLanguage(request);
  if (language === null || language === undefined) return 0.0;
  const wanted = trim(language);
  const given = trim(candidate.language);
  if (wanted === '' || given === '') return 0.0;
  return eqIgnoreAsciiCase(given, wanted) ? LANGUAGE_MATCH : LANGUAGE_MISMATCH;
}

// The words of the query against the label, the asset reference and the risk of the candidate.
function keywordMatch(candidate, request) {
  const query = tokenize(request.query);
  if (!query.length) return 0.0;
  const haystack = tokenize(candidate.label).concat(tokenize(candidate.asset_ref.id));
  if (candidate.risk !== null && candidate.risk !== undefined) haystack.push(...tokenize(candidate.risk));
  let matched = 0;
  for (const word of query) {
    if (haystack.some(answer => answer.startsWith(word))) matched++;
  }
  return matched / query.length;
}

// The segments of the asset id against the field of the query.
function applicability(candidate, request) {
  const domain = tokenize(request.query);
  if (!domain.length) return 0.0;
  const segments = assetRefSegments(candidate.asset_ref);
  if (!segments.length) return 0.0;
  let hits = 0;
  for (const segment of segments) {
    if (domain.some(word => segment.startsWith(word) || word.startsWith(segment))) hits++;
  }
  const ratio = hits / segments.length;
  const kind = assetRefKind(candidate.asset_ref);
  const kindHit = kind !== null && domain.some(word => word.startsWith(kind) || kind.startsWith(word));
  return APPLICABILITY_SEGMENT_WEIGHT * ratio + (kindHit ? APPLICABILITY_KIND_WEIGHT : 0.0);
}

// What the candidate changes.
function riskPenalty(candidate) {
  let penalty = 0.0;
  if (candidate.complete_project_edit) penalty += RISK_PENALTY_COMPLETE_PROJECT_EDIT;
  if (candidate.risk !== null && candidate.risk !== undefined && trim(candidate.risk) !== '') penalty += RISK_PENALTY_NOTED;
  return penalty;
}

// The score of a candidate for a request: the sum of the four parts.
function scoreCandidate(candidate, request) {
  const language = languageMatch(candidate, request);
  const keyword = keywordMatch(candidate, request);
  const applicable = applicability(candidate, request);
  const risk = riskPenalty(candidate);
  return Object.freeze({
    total: language + keyword + applicable + risk,
    language_match: language,
    keyword_match: keyword,
    applicability: applicable,
    risk_penalty: risk
  });
}

// The order of two candidates: the best one comes first (rank.rs: compare).
function compareRanked(a, b) {
  const total = compareScores(b.score.total, a.score.total);
  if (total !== 0) return total;
  // the same total: the parts decide, one after the other, so that the order never depends on the order of the
  // candidates in the list
  for (const part of ['language_match', 'keyword_match', 'applicability', 'risk_penalty']) {
    const decided = compareScores(b.score[part], a.score[part]);
    if (decided !== 0) return decided;
  }
  // the same score: the id decides
  return compareIds(a.candidate.candidate_id, b.candidate.candidate_id);
}

// The candidates of the request, the best one first. The order is total and stable.
function rank(candidates, request) {
  const list = Array.isArray(candidates) ? candidates : [];
  const ranked = list.map(candidate => ({candidate, score: scoreCandidate(candidate, request)}));
  ranked.sort(compareRanked);
  return ranked;
}

// The candidates of the candidate window: the asset cards (the ones that carry an asset_ref) are ordered by the
// ranking of the input method, so the window shows them in the same order as the IME window does. The rule pack
// candidates of the bridge keep the order of their own rule (DD §4.2): they are a different kind of candidate. The
// cards keep their place in the list; only their order among themselves changes.
function rankAssetCandidates(candidates, request) {
  const list = Array.isArray(candidates) ? candidates.slice() : [];
  const cards = [], models = [], positions = [];
  list.forEach((candidate, index) => {
    if (!candidate || !candidate.asset_ref || typeof candidate.candidate_id !== 'string') return;
    let model;
    try {
      model = makeCandidate(candidate);
    } catch {
      return; // a card that cannot be read keeps its place
    }
    cards.push(candidate);
    models.push(model);
    positions.push(index);
  });
  if (cards.length < 2) return list;
  const place = new Map();
  rank(models, request).forEach((entry, index) => {
    if (!place.has(entry.candidate.candidate_id)) place.set(entry.candidate.candidate_id, index);
  });
  const sorted = cards.slice().sort((a, b) => (place.get(a.candidate_id) || 0) - (place.get(b.candidate_id) || 0));
  positions.forEach((position, index) => {
    list[position] = sorted[index];
  });
  return list;
}

// ---------------------------------------------------------------- policy.rs

// The decision of the policy: the same three states as the Rust enum.
function decision(kind, reason = null) {
  return Object.freeze({kind, reason});
}

// Whether the candidates may be shown and the text may be inserted.
function isAllowed(d) {
  return d.kind === DECISION_ALLOW;
}

// Whether the context may only be read.
function isReadOnly(d) {
  return d.kind === DECISION_ALLOW_READ_ONLY;
}

// Whether the context is refused.
function isDenied(d) {
  return d.kind === DECISION_DENY;
}

// Whether the context may be read at all (a refused context may not).
function allowsRead(d) {
  return !isDenied(d);
}

// The privacy and safety policy of the input method.
function makePrivacyPolicy({allow_excerpt_in_code_context = false, allow_agent_orchestration = false} = {}) {
  const policy = {
    allow_excerpt_in_code_context: allow_excerpt_in_code_context === true,
    allow_agent_orchestration: allow_agent_orchestration === true,
    // What the input method may do in this context.
    decide(context) {
      switch (context.kind) {
        case 'password': return decision(DECISION_DENY, REASON_PASSWORD);
        case 'secure_desktop': return decision(DECISION_DENY, REASON_SECURE_DESKTOP);
        case 'uac_prompt': return decision(DECISION_DENY, REASON_UAC_PROMPT);
        case 'app_container': return decision(DECISION_DENY, REASON_APP_CONTAINER);
        case 'browser_secure_input': return decision(DECISION_DENY, REASON_BROWSER_SECURE_INPUT);
        case 'code':
        case 'plain_text': return decision(DECISION_ALLOW);
        // an unknown context is read, but nothing is inserted and nothing is sent
        case 'unknown': return decision(DECISION_ALLOW_READ_ONLY);
        default: throw fail('OUTPUT_INVALID', 'the context kind is unknown');
      }
    },
    // Whether the text around the caret may be collected in this context.
    excerpt_allowed(context) {
      if (isSensitiveKind(context.kind)) return false;
      // only the source code context of an editor may be collected, and only with the permission
      return context.kind === 'code' && policy.allow_excerpt_in_code_context;
    },
    // Whether the context may be handed to the orchestration of an agent.
    may_share_with_agent(context) {
      return policy.allow_agent_orchestration && isAllowed(policy.decide(context));
    },
    // How long the local cache candidates stay fresh.
    expires_after_ms() {
      return LOCAL_CACHE_P95_BUDGET_MS;
    }
  };
  return Object.freeze(policy);
}

// Whether the local cache candidates were fast enough (DD-IME 20.2: `p95 <= 50ms`).
function withinLocalBudget(elapsedMs) {
  return elapsedMs <= LOCAL_CACHE_P95_BUDGET_MS;
}

module.exports = {
  LANGUAGE_MATCH, LANGUAGE_MISMATCH, APPLICABILITY_SEGMENT_WEIGHT, APPLICABILITY_KIND_WEIGHT,
  RISK_PENALTY_COMPLETE_PROJECT_EDIT, RISK_PENALTY_NOTED,
  LOCAL_CACHE_P95_BUDGET_MS, FORBIDDEN_SHORTCUTS,
  REASON_PASSWORD, REASON_SECURE_DESKTOP, REASON_UAC_PROMPT, REASON_APP_CONTAINER, REASON_BROWSER_SECURE_INPUT,
  CONTEXT_KINDS, SENSITIVE_KINDS, CANDIDATE_STATUSES, DECISION_ALLOW, DECISION_ALLOW_READ_ONLY, DECISION_DENY,
  parseAssetRef, assetRefString, assetRefSegments, assetRefKind, assetRefCategory, makeCandidate,
  parseContextKind, isSensitiveKind, makeContext, contextGeneration, contextIsStale, advanceContextGeneration,
  contextExcerpt, contextLanguage,
  makeRankRequest, scoreCandidate, rank, rankAssetCandidates, compareScores, round9, tokenize,
  makePrivacyPolicy, decision, isAllowed, isReadOnly, isDenied, allowsRead, withinLocalBudget
};
