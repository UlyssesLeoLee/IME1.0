'use strict';
// ST-040: the F1 path under a controlled provider - how long the first candidate takes, whether the progress is
// visible while it runs, and what a cancellation and a provider timeout do.
// The design case (`00_Documents/02_UT_IT_ST/Programming_IME_ST_Test_Design_v1.0.md`, ST-040) says:
//   input: "a delay matrix, 30 responses"; steps: "simulate 30 F1 requests and a cancellation/timeout, compute the P95";
//   criterion: "the first candidate P95 <= 15 s, only under fixed network/model conditions; there is clear progress".
// The network condition is the delay the provider injects - fixed per run, the same for every request of the matrix -
// so the number is only claimed for that condition, exactly the wording the design uses.
// The panel, the permission broker, the privacy gate, the edit coordinator and the AI client are the real ones; the
// provider is the recording fake fetch of the testkit with a fixed delay.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const {okJson, httpError, responsesBody} = require('./helpers');
const {RUST, atText, rig, replyOf, until} = require('./f1rig');

const BUDGET_MS = 15000; // the design: the P95 of the time to the first candidate
const REQUESTS = 30;     // the design: 30 responses
const DELAYS = Array.from({length: REQUESTS}, (_, i) => 30 + 15 * i); // the delay matrix

const percentile = (values, p) => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(p * sorted.length) - 1))];
};

// the state of the annotate tab as the panel received it, with the moment it was posted
const withStamps = h => {
  const posts = [];
  const original = h.vscode.window.createWebviewPanel;
  h.vscode.window.createWebviewPanel = (type, title) => {
    const panel = original(type, title);
    const post = panel.webview.postMessage;
    panel.webview.postMessage = message => {
      posts.push({at: Date.now(), tab: message && message.tabs ? message.tabs.annotate : null});
      return post.call(panel.webview, message);
    };
    return panel;
  };
  return posts;
};
const lastStatus = posts => {
  const last = posts[posts.length - 1];
  return last && last.tab ? last.tab.status : null;
};
// the first candidate: the tab goes from "loading" to "ready" with hunks
const firstCandidate = posts => {
  const loading = posts.find(p => p.tab && p.tab.status === 'loading');
  const ready = posts.find(p => p.tab && p.tab.status === 'ready' && p.tab.hunks.length > 0);
  return {loading, ready, ms: loading && ready ? ready.at - loading.at : null};
};

// the provider of the controlled condition: it answers after `delayMs`; with `abort` it gives up when the request
// is cancelled (the abort signal of the request reaches the fetch), with `status` it answers with an HTTP error
const provider = (delayMs, {abort = false, status = 0} = {}) => async call => {
  if (abort) {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(resolve, delayMs);
      const signal = call.init && call.init.signal;
      if (signal) signal.addEventListener('abort', () => {
        clearTimeout(timer);
        reject(Object.assign(new Error('aborted'), {name: 'AbortError'}));
      });
    });
  } else {
    await new Promise(resolve => setTimeout(resolve, delayMs));
  }
  if (status) return httpError(status);
  return okJson(responsesBody(JSON.stringify(replyOf([1, 'Explains it.']))));
};

// one F1 request: the panel opens, the user asks for a proposal, the provider answers after the fixed delay
async function oneRequest(delayMs, options = {}) {
  const h = rig({config: {f1AnnotateOnOpen: false}, handler: provider(delayMs, options)});
  const posts = withStamps(h);
  await h.open(atText(RUST, 'n += 1', 3)); // the panel opens; nothing is requested yet
  const panel = h.ui.panels[h.ui.panels.length - 1];
  panel.handler({type: 'generate', intent: 'explain this line'}); // the request starts
  // the terminal state of the tab: a candidate ("ready") or a failure ("error"). "idle" is also the state before
  // the request starts, so it cannot be the end of the wait.
  await until(() => posts.some(p => p.tab && (p.tab.status === 'ready' || p.tab.status === 'error')), 15000);
  return {h, panel, posts};
}

test('ST-040 the F1 path under a controlled provider: the P95 of the time to the first candidate is inside the budget, and the progress is visible', async () => {
  const measured = [];
  for (const delay of DELAYS) {
    const {posts} = await oneRequest(delay);
    const {loading, ready, ms} = firstCandidate(posts);
    assert.ok(loading, 'the tab never showed that it was working');
    assert.ok(ready, 'no candidate arrived');
    assert.ok(ms >= delay, `the first candidate arrived after ${ms} ms although the provider needed ${delay} ms`);
    measured.push(ms);
  }
  const p95 = percentile(measured, 0.95);
  const injected = percentile(DELAYS, 0.95);
  assert.ok(p95 <= BUDGET_MS, `the P95 of the first candidate is ${p95} ms (the budget is ${BUDGET_MS} ms)`);
  assert.ok(p95 >= injected * 0.8, `the P95 (${p95} ms) does not show the delay of the provider (${injected} ms)`);
  console.log(`# ST-040 P95(first candidate) = ${p95} ms over ${REQUESTS} requests; the delay matrix P95 = ${injected} ms; the budget is ${BUDGET_MS} ms`);
});

test('ST-040 a cancelled F1 request leaves the tab idle: no candidate and no stuck progress', async () => {
  const h = rig({config: {f1AnnotateOnOpen: false}, handler: provider(5000, {abort: true})});
  const posts = withStamps(h);
  await h.open(atText(RUST, 'n += 1', 3));
  const panel = h.ui.panels[h.ui.panels.length - 1];
  panel.handler({type: 'generate', intent: 'explain this line'});
  await until(() => lastStatus(posts) === 'loading', 5000); // the request is on its way
  panel.handler({type: 'cancel'}); // the user cancels
  await until(() => lastStatus(posts) === 'idle', 5000);
  assert.equal(lastStatus(posts), 'idle');
  assert.ok(!posts.some(p => p.tab && p.tab.status === 'ready'), 'a cancelled request still produced a candidate');
});

test('ST-040 a provider timeout leaves the tab in error, with the reason shown', async () => {
  const {posts} = await oneRequest(10, {status: 504});
  const last = posts[posts.length - 1];
  assert.equal(last.tab.status, 'error');
  assert.ok(last.tab.message, 'the timeout is not explained to the user');
  assert.ok(!posts.some(p => p.tab && p.tab.status === 'ready'), 'a timed out request still produced a candidate');
});
