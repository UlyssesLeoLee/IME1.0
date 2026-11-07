'use strict';
// Parity of the candidate ranking, the context classification and the privacy policy with the Rust core of the input
// method (01_Source/crates/tase-ime-core). The fixture tests/fixtures/tase-ranking-cases.json is read by BOTH sides:
// tests/rust/parity_tase_ime.rs runs the crate (the only truth source) and this test runs the JavaScript mirror in
// editor-bridges/vscode/src/core/candidates.js. The expectations of the fixture are printed by the Rust side
// (TASE_PARITY_EMIT=1), so they come from the truth source and not from this file.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {core} = require('./helpers');
const C = core('candidates');

const dir = path.join(__dirname, '..', 'fixtures');
const load = name => JSON.parse(fs.readFileSync(path.join(dir, name), 'utf8'));
// The tolerance of the comparison: the ranking itself compares the scores rounded to 1e-9.
const EPSILON = 1e-9;

// The comparison of two values; the numbers are compared with the tolerance of the ranking, the rest exactly.
function same(got, want, at) {
  if (typeof got === 'number' && typeof want === 'number') {
    assert.ok(Math.abs(got - want) <= EPSILON, `${at}: ${got} != ${want}`);
    return;
  }
  if (Array.isArray(got) && Array.isArray(want)) {
    assert.equal(got.length, want.length, `${at}: ${got.length} != ${want.length} entries`);
    got.forEach((value, i) => same(value, want[i], `${at}[${i}]`));
    return;
  }
  if (got && want && typeof got === 'object' && typeof want === 'object') {
    assert.deepEqual(Object.keys(got).sort(), Object.keys(want).sort(), `${at}: the keys`);
    for (const key of Object.keys(want)) same(got[key], want[key], `${at}.${key}`);
    return;
  }
  assert.deepEqual(got, want, at);
}

// The context of a case.
const contextOf = value => C.makeContext({
  kind: value.kind,
  language_hint: value.language_hint === undefined ? null : value.language_hint,
  focus_context_generation: value.focus_context_generation === undefined ? 0 : value.focus_context_generation,
  excerpt: value.excerpt === undefined ? null : value.excerpt
});

// The request of a case.
const requestOf = c => C.makeRankRequest({
  query: c.request.query,
  language: c.request.language === undefined ? null : c.request.language,
  context: contextOf(c.context),
  now_ms: c.request.now_ms === undefined ? 0 : c.request.now_ms
});

// What the mirror answers for one case: the order, the parts, the decision and the generation checks.
function expectations(c) {
  const context = contextOf(c.context);
  const request = requestOf(c);
  const policy = C.makePrivacyPolicy(c.policy || {});
  const ranked = C.rank((c.candidates || []).map(x => C.makeCandidate(x)), request);
  const decision = policy.decide(context);
  return {
    order: ranked.map(entry => entry.candidate.candidate_id),
    scores: ranked.map(entry => ({
      candidate_id: entry.candidate.candidate_id,
      total: C.round9(entry.score.total),
      language_match: C.round9(entry.score.language_match),
      keyword_match: C.round9(entry.score.keyword_match),
      applicability: C.round9(entry.score.applicability),
      risk_penalty: C.round9(entry.score.risk_penalty)
    })),
    decision: {kind: decision.kind, reason: decision.reason},
    excerpt_allowed: policy.excerpt_allowed(context),
    may_share_with_agent: policy.may_share_with_agent(context),
    stale: (c.stale_checks || []).map(check => ({
      current_generation: check.current_generation,
      is_stale: C.contextIsStale(context, check.current_generation)
    }))
  };
}

test('PARITY the TASE ranking and the privacy policy match the shared fixture', () => {
  const f = load('tase-ranking-cases.json');
  assert.ok(f.cases.length >= 20, `the fixture must cover every rule, it has ${f.cases.length} cases`);
  for (const c of f.cases) {
    assert.ok(c.expect && Object.keys(c.expect).length > 0, `${c.name}: the fixture has no expectations`);
    same(expectations(c), c.expect, c.name);
  }
});

test('PARITY a broken candidate is refused with the same code and the same message', () => {
  const f = load('tase-ranking-cases.json');
  assert.ok(f.invalid_candidates.length >= 8, 'the fixture must cover the broken shapes of a candidate');
  for (const entry of f.invalid_candidates) {
    let got;
    try {
      C.makeCandidate(entry.candidate);
      got = {code: 'NO_ERROR', message: 'the candidate was accepted'};
    } catch (e) {
      got = {code: e.code, message: e.message};
    }
    assert.deepEqual(got, entry.error, JSON.stringify(entry.candidate));
  }
});

test('the mirror knows the kinds, the reasons and the weights of the design', () => {
  assert.deepEqual([...C.CONTEXT_KINDS], ['plain_text', 'code', 'password', 'secure_desktop', 'uac_prompt', 'app_container', 'browser_secure_input', 'unknown']);
  assert.deepEqual([...C.SENSITIVE_KINDS], ['password', 'secure_desktop', 'uac_prompt', 'app_container', 'browser_secure_input']);
  assert.deepEqual([...C.FORBIDDEN_SHORTCUTS], ['keyboard_simulation', 'process_injection', 'clipboard_read', 'screen_read']);
  assert.equal(C.LOCAL_CACHE_P95_BUDGET_MS, 50);
  assert.equal(C.LANGUAGE_MATCH, 1.0);
  assert.equal(C.LANGUAGE_MISMATCH, -1.0);
  assert.equal(C.APPLICABILITY_SEGMENT_WEIGHT, 0.6);
  assert.equal(C.APPLICABILITY_KIND_WEIGHT, 0.4);
  assert.equal(C.RISK_PENALTY_COMPLETE_PROJECT_EDIT, -0.5);
  assert.equal(C.RISK_PENALTY_NOTED, -0.1);
  assert.equal(C.REASON_PASSWORD, 'password: the candidates are disabled in a password control');
  assert.equal(C.REASON_SECURE_DESKTOP, 'secure_desktop: the candidates are disabled on the secure desktop');
  assert.equal(C.REASON_UAC_PROMPT, 'uac_prompt: the candidates are disabled in an elevation prompt');
  assert.equal(C.REASON_APP_CONTAINER, 'app_container: the candidates are disabled in a restricted AppContainer');
  assert.equal(C.REASON_BROWSER_SECURE_INPUT, 'browser_secure_input: the candidates are disabled in a secure input field of a browser');
});

test('the words of a text are the runs of alphanumeric characters, in lower case', () => {
  assert.deepEqual(C.tokenize('Strategy: trait + implementations'), ['strategy', 'trait', 'implementations']);
  assert.deepEqual(C.tokenize('pattern.gof.behavioral.strategy'), ['pattern', 'gof', 'behavioral', 'strategy']);
  assert.deepEqual(C.tokenize('策略 trait 带来抽象'), ['策略', 'trait', '带来抽象']);
  assert.deepEqual(C.tokenize(''), []);
  assert.deepEqual(C.tokenize('___'), []);
});

test('the order is total and stable: the same input always gives the same order', () => {
  const request = C.makeRankRequest({query: 'strategy', language: 'rust', context: C.makeContext({kind: 'code', language_hint: 'rust', focus_context_generation: 1})});
  const one = C.makeCandidate({candidate_id: 'cand-b', asset_ref: 'pattern.gof.behavioral.strategy@1.0.0', language: 'rust', label: 'Strategy'});
  const two = C.makeCandidate({candidate_id: 'cand-a', asset_ref: 'pattern.gof.behavioral.strategy@1.0.0', language: 'rust', label: 'Strategy'});
  const ids = list => C.rank(list, request).map(entry => entry.candidate.candidate_id);
  assert.deepEqual(ids([one, two]), ['cand-a', 'cand-b']);
  assert.deepEqual(ids([two, one]), ['cand-a', 'cand-b']);
});

test('the generation decides whether the candidates are stale, and it never wraps', () => {
  const context = C.makeContext({kind: 'code', language_hint: 'rust', focus_context_generation: 7});
  assert.equal(C.contextGeneration(context), 7n);
  assert.equal(C.contextIsStale(context, 7), false);
  assert.equal(C.contextIsStale(context, 8), true);
  assert.equal(C.contextIsStale(context, 6), true);
  const advanced = C.advanceContextGeneration(context);
  assert.equal(C.contextGeneration(advanced), 8n);
  assert.equal(C.contextExcerpt(advanced), null);
});

test('the asset cards of the window keep the order of the input method, the other candidates keep theirs', () => {
  const request = C.makeRankRequest({query: 'strategy', language: 'rust', context: C.makeContext({kind: 'code', language_hint: 'rust', focus_context_generation: 1})});
  const card = (id, language) => ({candidate_id: id, asset_ref: 'pattern.gof.behavioral.strategy@1.0.0', language, label: 'Strategy'});
  const list = [
    {id: 'rule.1', label: 'a rule pack candidate'},
    card('cand-b', 'python'),
    {id: 'rule.2', label: 'another rule pack candidate'},
    card('cand-a', 'rust')
  ];
  const ordered = C.rankAssetCandidates(list, request);
  assert.deepEqual(ordered.map(c => c.candidate_id || c.id), ['rule.1', 'cand-a', 'rule.2', 'cand-b']);
  // a list without a card comes back unchanged
  const plain = [{id: 'rule.1'}, {id: 'rule.2'}];
  assert.deepEqual(C.rankAssetCandidates(plain, request), plain);
});

test('a protected context is refused whatever the permissions are', () => {
  const policy = C.makePrivacyPolicy({allow_excerpt_in_code_context: true, allow_agent_orchestration: true});
  for (const kind of C.SENSITIVE_KINDS) {
    const context = C.makeContext({kind, focus_context_generation: 1});
    const decision = policy.decide(context);
    assert.equal(decision.kind, 'deny', `${kind} is refused`);
    assert.ok(decision.reason && decision.reason.includes(kind), `the reason of ${kind} names it: ${decision.reason}`);
    assert.equal(policy.excerpt_allowed(context), false, `${kind} never has an excerpt`);
    assert.equal(policy.may_share_with_agent(context), false, `${kind} is never shared`);
  }
  const unknown = C.makeContext({kind: 'unknown', focus_context_generation: 1});
  assert.equal(policy.decide(unknown).kind, 'allow_read_only');
  assert.equal(policy.may_share_with_agent(unknown), false, 'an unknown context is not an authorized editing context');
});
