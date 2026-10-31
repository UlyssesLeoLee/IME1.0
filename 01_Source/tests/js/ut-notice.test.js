'use strict';
// TransientNotice (src/ui/notice.js): the bottom-right notice that vanishes on the user's next action. The `vscode` module is a fake with
// counted event subscriptions, a recording withProgress/StatusBarItem and failure switches; time is the virtual clock from helpers.js.
// Nothing here runs inside a real VS Code window (see the UAT list in the work-item report).
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {FakeTimers, tick} = require('./helpers');
const {TransientNotice, PRESENTATIONS} = require('../../editor-bridges/vscode/src/ui/notice');

const EVENTS = {
  workspace: ['onDidChangeTextDocument'],
  window: ['onDidChangeTextEditorSelection', 'onDidChangeTextEditorVisibleRanges', 'onDidChangeActiveTextEditor', 'onDidChangeWindowState', 'onDidStartTerminalShellExecution', 'onDidChangeActiveTerminal']
};
const TRIGGER_COUNT = 7;

function makeVscode({missing = []} = {}) {
  const vs = {progressCalls: [], items: [], emitters: new Map(), mode: {progress: 'ok', status: 'ok', statusShow: 'ok', report: 'ok', subscribe: new Set(), disposeSub: false}};
  const ev = name => {
    const em = {listeners: new Set()};
    vs.emitters.set(name, em);
    return l => {
      if (vs.mode.subscribe.has(name)) throw new Error(`subscribe ${name}`);
      em.listeners.add(l);
      return {dispose() { em.listeners.delete(l); if (vs.mode.disposeSub) throw new Error('dispose'); }};
    };
  };
  const window = {state: {focused: true, active: true}};
  const workspace = {};
  for (const [host, names] of Object.entries(EVENTS)) for (const n of names) if (!missing.includes(n)) (host === 'window' ? window : workspace)[n] = ev(n);
  window.withProgress = (options, task) => {
    if (vs.mode.progress === 'throw') throw new Error('withProgress boom');
    const rec = {options, reports: [], settled: false};
    vs.progressCalls.push(rec);
    const progress = {report: v => { if (vs.mode.report === 'throw') throw new Error('report boom'); rec.reports.push(v); }};
    const done = Promise.resolve(task(progress, {isCancellationRequested: false})).then(v => { rec.settled = true; return v; });
    return vs.mode.progress === 'reject' ? Promise.reject(new Error('async boom')) : done;
  };
  window.createStatusBarItem = (id, alignment, priority) => {
    if (vs.mode.status === 'throw') throw new Error('status boom');
    const item = {id, alignment, priority, text: '', tooltip: undefined, backgroundColor: undefined, shown: false, disposed: false, disposedBefore: vs.items.filter(i => i.disposed).length,
      show() { if (vs.mode.statusShow === 'throw') throw new Error('show boom'); this.shown = true; },
      dispose() { this.disposed = true; this.shown = false; }};
    vs.items.push(item);
    return item;
  };
  Object.assign(vs, {window, workspace, ProgressLocation: {Notification: 15}, StatusBarAlignment: {Left: 1, Right: 2}, ThemeColor: class { constructor(id) { this.id = id; } }});
  vs.fire = (name, e) => { for (const l of [...vs.emitters.get(name).listeners]) l(e); };
  vs.live = () => [...vs.emitters.values()].reduce((n, em) => n + em.listeners.size, 0);
  return vs;
}

function rig(opts = {}, vsOpts) {
  const vs = makeVscode(vsOpts), ft = new FakeTimers();
  const notice = new TransientNotice(vs, {timeoutMs: 20000, graceMs: 600, setTimer: (f, ms) => ft.setTimeout(f, ms), clearTimer: id => ft.clearTimeout(id), now: () => ft.now, ...opts});
  return {vs, ft, notice};
}
const reasonOf = h => { const box = {v: undefined, n: 0}; h.dismissed.then(v => { box.v = v; box.n++; }); return box; };
const doc = scheme => ({uri: {scheme}});
const edit = scheme => ({document: doc(scheme), contentChanges: [{text: 'x'}]});
const editor = scheme => ({textEditor: {document: doc(scheme)}});
async function assertClean(vs, ft, notice, msg) {
  await tick();
  assert.equal(notice.active, false, `${msg}: inactive`);
  assert.equal(vs.live(), 0, `${msg}: no live subscription`);
  assert.equal(ft.q.length, 0, `${msg}: timer cancelled`);
  assert.ok(vs.progressCalls.every(c => c.settled), `${msg}: every progress task resolved, so the toast is gone`);
  assert.ok(vs.items.every(i => i.disposed), `${msg}: every status bar item disposed`);
}

// [label, expected reason, setup before show, fire]
const CASES = [
  ['typing', 'input', null, vs => vs.fire('onDidChangeTextDocument', edit('file'))],
  ['click / cursor move', 'selection', null, vs => vs.fire('onDidChangeTextEditorSelection', editor('file'))],
  ['wheel / scrollbar', 'scroll', null, vs => vs.fire('onDidChangeTextEditorVisibleRanges', editor('untitled'))],
  ['switch editor', 'editor', null, vs => vs.fire('onDidChangeActiveTextEditor', undefined)],
  ['window loses focus', 'focus', null, vs => vs.fire('onDidChangeWindowState', {focused: false, active: false})],
  ['key or click after >=10 s idle', 'activity', vs => { vs.window.state.active = false; }, vs => vs.fire('onDidChangeWindowState', {focused: true, active: true})],
  ['terminal command run', 'terminal', null, vs => vs.fire('onDidStartTerminalShellExecution', {execution: {}})],
  ['another terminal focused', 'terminal', null, vs => vs.fire('onDidChangeActiveTerminal', {name: 'bash'})]
];

for (const presentation of ['toast', 'statusbar']) {
  test(`ANN-40 [${presentation}] every trigger closes the notice with its own reason and leaves no subscription, timer or visible item`, async () => {
    for (const [label, expected, setup, fire] of CASES) {
      const {vs, ft, notice} = rig({presentation});
      if (setup) setup(vs);
      const h = notice.show({text: 'Nothing to optimise'});
      const r = reasonOf(h);
      assert.equal(notice.active, true, label);
      assert.equal(vs.live(), TRIGGER_COUNT, `${label}: every trigger subscribed`);
      assert.equal(presentation === 'toast' ? vs.progressCalls.length : vs.items.length, 1, `${label}: shown through ${presentation}`);
      await ft.advance(700);
      assert.equal(notice.active, true, `${label}: still open before the event`);
      fire(vs);
      assert.equal(await h.dismissed, expected, label);
      assert.equal(r.n, 1);
      await assertClean(vs, ft, notice, label);
      fire(vs); await tick(); // late event after close: the listeners are gone, nothing resolves again
      assert.equal(r.n, 1, `${label}: dismissed resolves exactly once`);
    }
  });
}

test('ANN-41 events inside graceMs are ignored (the keypress that produced the notice), events at graceMs close it', async () => {
  const {vs, ft, notice} = rig();
  const h = notice.show({text: 'Nothing to optimise'}); const r = reasonOf(h);
  for (const [label, , setup, fire] of CASES) { if (setup) continue; fire(vs); await tick(); assert.equal(notice.active, true, `${label} at t=0 is ignored`); }
  await ft.advance(599);
  vs.fire('onDidChangeTextDocument', edit('file')); await tick();
  assert.equal(notice.active, true, 'still inside the grace period at t=599');
  assert.equal(vs.live(), TRIGGER_COUNT);
  await ft.advance(1);
  vs.fire('onDidChangeTextDocument', {document: doc('file'), contentChanges: []}); await tick();
  assert.equal(notice.active, true, 'a document event without content changes (dirty/save) is not typing');
  vs.fire('onDidChangeTextDocument', edit('file'));
  assert.equal(await h.dismissed, 'input');
  assert.equal(r.n, 1);
  await assertClean(vs, ft, notice, 'after grace');
});

test('ANN-42 Output-channel documents (rewritten and auto-scrolled by every extension) never dismiss the notice, real editors do', async () => {
  const {vs, ft, notice} = rig();
  const h = notice.show({text: 'Nothing to optimise'});
  await ft.advance(700);
  vs.fire('onDidChangeTextDocument', edit('output'));
  vs.fire('onDidChangeTextEditorVisibleRanges', editor('output'));
  vs.fire('onDidChangeTextEditorSelection', editor('output'));
  await tick();
  assert.equal(notice.active, true);
  vs.fire('onDidChangeTextEditorVisibleRanges', editor('file'));
  assert.equal(await h.dismissed, 'scroll');
});

test('ANN-43 WindowState: only an inactive -> active flip counts as activity; losing focus is "focus"; the baseline is read from window.state', async () => {
  // baseline active: an "active" event is no flip; idle then active is
  {
    const {vs, ft, notice} = rig();
    const h = notice.show({text: 'x'});
    await ft.advance(700);
    vs.fire('onDidChangeWindowState', {focused: true, active: true}); await tick();
    assert.equal(notice.active, true, 'no flip');
    vs.fire('onDidChangeWindowState', {focused: true, active: false}); await tick();
    assert.equal(notice.active, true, 'going idle is not input');
    vs.fire('onDidChangeWindowState', {focused: true, active: true});
    assert.equal(await h.dismissed, 'activity');
  }
  // the flip is tracked during the grace period too: idle at t=100, active again after grace is not a new flip, so nothing closes
  {
    const {vs, ft, notice} = rig();
    notice.show({text: 'x'});
    await ft.advance(100);
    vs.fire('onDidChangeWindowState', {focused: true, active: false});
    await ft.advance(100);
    vs.fire('onDidChangeWindowState', {focused: true, active: true}); // inside grace: ignored
    await ft.advance(600);
    vs.fire('onDidChangeWindowState', {focused: true, active: true}); await tick();
    assert.equal(notice.active, true, 'already active since t=200');
  }
  // an older host without WindowState.active: never "activity", focus still works
  {
    const {vs, ft, notice} = rig();
    vs.window.state = {focused: true};
    const h = notice.show({text: 'x'});
    await ft.advance(700);
    vs.fire('onDidChangeWindowState', {focused: true}); await tick();
    assert.equal(notice.active, true);
    vs.fire('onDidChangeWindowState', {focused: false});
    assert.equal(await h.dismissed, 'focus');
  }
});

test('ANN-44 show() while open closes the old notice as "replaced" first, then opens the new one with a single set of subscriptions', async () => {
  for (const presentation of ['toast', 'statusbar']) {
    const {vs, ft, notice} = rig({presentation});
    const a = notice.show({text: 'first'}); const ra = reasonOf(a);
    await ft.advance(100);
    const b = notice.show({text: 'second'}); const rb = reasonOf(b);
    await tick();
    assert.equal(ra.v, 'replaced', presentation);
    assert.equal(rb.v, undefined);
    assert.equal(notice.active, true);
    assert.equal(vs.live(), TRIGGER_COUNT, 'subscriptions of the old notice are gone, not doubled');
    assert.equal(ft.q.length, 1, 'only the new timeout is pending');
    if (presentation === 'toast') { assert.equal(vs.progressCalls.length, 2); assert.equal(vs.progressCalls[0].settled, true); assert.equal(vs.progressCalls[1].settled, false); }
    else { assert.equal(vs.items.length, 2); assert.equal(vs.items[1].disposedBefore, 1, 'the old item was disposed before the new one was created'); assert.equal(vs.items[1].shown, true); }
    await ft.advance(700); // the grace period belongs to the new notice
    vs.fire('onDidChangeActiveTextEditor', undefined);
    assert.equal(await b.dismissed, 'editor');
    assert.equal(ra.n, 1);
    await assertClean(vs, ft, notice, presentation);
  }
});

test('ANN-45 the timeout closes the notice at timeoutMs and not earlier; invalid options fall back to 20000 / 600', async () => {
  const {vs, ft, notice} = rig({timeoutMs: 5000});
  const h = notice.show({text: 'x'});
  await ft.advance(4999);
  assert.equal(notice.active, true);
  await ft.advance(1);
  assert.equal(await h.dismissed, 'timeout');
  await assertClean(vs, ft, notice, 'timeout');
  for (const bad of [0, -5, NaN, Infinity, '20', null, undefined]) {
    const x = rig({timeoutMs: bad, graceMs: -1});
    const hx = x.notice.show({text: 'x'}); const rx = reasonOf(hx);
    x.vs.fire('onDidChangeActiveTextEditor', undefined); await tick();
    assert.equal(rx.v, undefined, `graceMs -1 falls back to 600 (timeoutMs ${String(bad)})`);
    await x.ft.advance(19999);
    assert.equal(x.notice.active, true, `timeoutMs ${String(bad)} falls back to 20000`);
    await x.ft.advance(1);
    assert.equal(await hx.dismissed, 'timeout');
  }
});

test('ANN-46 dismiss() is idempotent: first reason wins, default reason is "api", no notice is a no-op', async () => {
  const {vs, ft, notice} = rig();
  assert.doesNotThrow(() => notice.dismiss());
  const h = notice.show({text: 'x'}); const r = reasonOf(h);
  notice.dismiss(); notice.dismiss('other'); notice.dismiss();
  assert.equal(await h.dismissed, 'api');
  assert.equal(r.n, 1);
  await assertClean(vs, ft, notice, 'dismiss');
  const h2 = notice.show({text: 'y'});
  notice.dismiss('custom');
  assert.equal(await h2.dismissed, 'custom');
  await ft.advance(30000); // the cancelled timeout of the first notice must not close the second one
  const h3 = notice.show({text: 'z'}); const r3 = reasonOf(h3);
  await ft.advance(100); notice.dismiss(); await tick();
  assert.equal(r3.v, 'api'); assert.equal(r3.n, 1);
});

test('ANN-47 dispose() dismisses with "disposed", unsubscribes everything, and turns later show() into a no-op', async () => {
  const {vs, ft, notice} = rig();
  const h = notice.show({text: 'x'});
  notice.dispose();
  assert.equal(await h.dismissed, 'disposed');
  await assertClean(vs, ft, notice, 'dispose');
  const calls = vs.progressCalls.length;
  const again = notice.show({text: 'y'});
  assert.equal(await again.dismissed, 'disposed');
  assert.equal(notice.active, false); assert.equal(vs.progressCalls.length, calls); assert.equal(vs.items.length, 0); assert.equal(vs.live(), 0);
  assert.doesNotThrow(() => { notice.dispose(); notice.dismiss(); });
  const idle = rig().notice; // dispose without ever showing
  assert.doesNotThrow(() => idle.dispose());
});

test('ANN-48 repeated show/dismiss cycles (every way of closing) leak no subscription, timer or visible item', async () => {
  for (const presentation of ['toast', 'statusbar']) {
    const {vs, ft, notice} = rig({presentation});
    for (let i = 0; i < 60; i++) {
      const h = notice.show({text: `n${i}`, kind: i % 2 ? 'warn' : 'info'}); const r = reasonOf(h);
      assert.equal(vs.live(), TRIGGER_COUNT, `cycle ${i}`);
      assert.equal(ft.q.length, 1, `cycle ${i}: exactly one timer`);
      switch (i % 6) {
        case 0: notice.dismiss(); break;
        case 1: await ft.advance(700); vs.fire('onDidChangeTextDocument', edit('file')); break;
        case 2: await ft.advance(20000); break;
        case 3: notice.show({text: `replacement ${i}`}); assert.equal(vs.live(), TRIGGER_COUNT); notice.dismiss('x'); break;
        case 4: await ft.advance(700); vs.fire('onDidChangeWindowState', {focused: false}); break;
        default: notice.dismiss('custom'); break;
      }
      await tick();
      assert.notEqual(r.v, undefined, `cycle ${i} resolved`);
      assert.equal(vs.live(), 0, `cycle ${i}: nothing left subscribed`);
      assert.equal(ft.q.length, 0, `cycle ${i}: nothing left scheduled`);
    }
    await assertClean(vs, ft, notice, presentation);
    assert.ok(vs.progressCalls.length + vs.items.length >= 60 + 10, 'every cycle really presented');
  }
});

test('ANN-49 a throwing VS Code API never escapes: fallback to the other presentation, else nothing', async () => {
  // withProgress throws -> status bar takes over
  {
    const {vs, ft, notice} = rig();
    vs.mode.progress = 'throw';
    let h; assert.doesNotThrow(() => { h = notice.show({text: 'x'}); });
    assert.equal(notice.active, true); assert.equal(vs.items.length, 1); assert.equal(vs.items[0].shown, true);
    notice.dismiss(); assert.equal(await h.dismissed, 'api');
    await assertClean(vs, ft, notice, 'fallback to status bar');
  }
  // the status bar item cannot be shown: it is disposed again, the toast takes over
  {
    const {vs, ft, notice} = rig({presentation: 'statusbar'});
    vs.mode.statusShow = 'throw';
    notice.show({text: 'x'});
    assert.equal(notice.active, true); assert.equal(vs.items.length, 1); assert.equal(vs.items[0].disposed, true); assert.equal(vs.progressCalls.length, 1);
    notice.dismiss(); await assertClean(vs, ft, notice, 'fallback to toast');
  }
  // both fail: nothing shown, nothing subscribed, dismissed resolves with "error"
  {
    const {vs, ft, notice} = rig();
    vs.mode.progress = 'throw'; vs.mode.status = 'throw';
    let h; assert.doesNotThrow(() => { h = notice.show({text: 'x'}); });
    assert.equal(await h.dismissed, 'error');
    await assertClean(vs, ft, notice, 'nothing works');
    assert.doesNotThrow(() => { notice.dismiss(); notice.dispose(); });
  }
  // the progress thenable rejects after it was accepted: nothing is on screen any more, so the notice ends
  {
    const {vs, ft, notice} = rig();
    vs.mode.progress = 'reject';
    const h = notice.show({text: 'x'});
    assert.equal(await h.dismissed, 'error');
    await assertClean(vs, ft, notice, 'async rejection');
  }
  // progress.report throws: the toast is still there, just with the endless bar
  {
    const {vs, ft, notice} = rig();
    vs.mode.report = 'throw';
    const h = notice.show({text: 'x'});
    assert.equal(notice.active, true); assert.equal(vs.progressCalls.length, 1);
    notice.dismiss(); await h.dismissed; await assertClean(vs, ft, notice, 'report throws');
  }
  // one event cannot be subscribed: only that trigger is lost
  {
    const {vs, ft, notice} = rig();
    vs.mode.subscribe.add('onDidChangeTextEditorSelection');
    const h = notice.show({text: 'x'});
    assert.equal(vs.live(), TRIGGER_COUNT - 1);
    await ft.advance(700);
    vs.fire('onDidChangeTextEditorVisibleRanges', editor('file'));
    assert.equal(await h.dismissed, 'scroll');
    await assertClean(vs, ft, notice, 'one subscription failed');
  }
  // a subscription whose dispose() throws does not stop the close or the other disposals
  {
    const {vs, ft, notice} = rig();
    vs.mode.disposeSub = true;
    const h = notice.show({text: 'x'});
    assert.doesNotThrow(() => notice.dismiss());
    assert.equal(await h.dismissed, 'api');
    await assertClean(vs, ft, notice, 'dispose throws');
  }
  // the timer cannot be set: the notice still opens and can still be dismissed
  {
    const {vs, notice} = rig({setTimer: () => { throw new Error('no timers'); }});
    let h; assert.doesNotThrow(() => { h = notice.show({text: 'x'}); });
    assert.equal(notice.active, true);
    notice.dismiss(); assert.equal(await h.dismissed, 'api'); assert.equal(vs.live(), 0);
  }
  // a malformed event payload is swallowed, the notice stays
  {
    const {vs, ft, notice} = rig();
    notice.show({text: 'x'});
    await ft.advance(700);
    assert.doesNotThrow(() => { vs.fire('onDidChangeTextDocument', {}); vs.fire('onDidChangeTextEditorSelection', undefined); vs.fire('onDidChangeWindowState', undefined); });
    assert.equal(notice.active, true);
  }
});

test('ANN-50 empty or blank text shows nothing, leaves an open notice alone and returns an already resolved dismissed', async () => {
  const {vs, notice} = rig();
  for (const bad of [{text: ''}, {text: '   \n\t'}, {}, undefined, {text: null}, {text: 5}]) {
    const h = notice.show(bad);
    assert.equal(await h.dismissed, 'empty');
  }
  assert.equal(vs.progressCalls.length + vs.items.length, 0); assert.equal(notice.active, false); assert.equal(vs.live(), 0);
  const open = notice.show({text: 'real'}); const r = reasonOf(open);
  const h = notice.show({text: ' '});
  assert.equal(await h.dismissed, 'empty');
  await tick();
  assert.equal(r.v, undefined, 'the open notice was not replaced');
  assert.equal(notice.active, true); assert.equal(vs.progressCalls.length, 1);
});

test('ANN-51 presentations: toast = static full-bar Notification progress, status bar = right-aligned item with escaped text and warning colour', async () => {
  {
    const {vs, notice} = rig();
    notice.show({text: 'Nothing to optimise'});
    const [call] = vs.progressCalls;
    assert.deepEqual(call.options, {location: 15, title: 'Nothing to optimise', cancellable: false});
    assert.deepEqual(call.reports, [{increment: 100}], 'one report: static bar, not the endless animation');
    notice.show({text: 'Careful', kind: 'warn'});
    assert.equal(vs.progressCalls[1].options.title, `${String.fromCharCode(0x26A0)} Careful`);
    notice.show({text: 'Odd', kind: 'bogus'});
    assert.equal(vs.progressCalls[2].options.title, 'Odd', 'unknown kinds are info');
  }
  {
    const {vs, notice} = rig({presentation: 'statusbar'});
    notice.show({text: 'Nothing\n  to   optimise $(sync~spin) now'});
    const [item] = vs.items;
    assert.equal(item.alignment, 2); assert.equal(item.shown, true); assert.equal(item.backgroundColor, undefined);
    assert.equal(item.text, '$(info) Nothing to optimise \\$(sync~spin) now', 'one line, literal "$(" escaped so it cannot become an icon');
    assert.equal(item.tooltip, 'Nothing\n  to   optimise $(sync~spin) now', 'the tooltip keeps the full text');
    notice.show({text: 'Careful', kind: 'warn'});
    assert.equal(vs.items[1].text, '$(warning) Careful');
    assert.equal(vs.items[1].backgroundColor.id, 'statusBarItem.warningBackground');
  }
  assert.deepEqual(Object.keys(PRESENTATIONS).sort(), ['statusbar', 'toast']);
});

test('ANN-52 presentation option: unknown values mean toast; statusbar-first falls back to the toast when creating the item throws', async () => {
  const a = rig({presentation: 'carrier-pigeon'}); a.notice.show({text: 'x'});
  assert.equal(a.vs.progressCalls.length, 1); assert.equal(a.vs.items.length, 0);
  const b = rig({presentation: 'statusbar'}); b.vs.mode.status = 'throw'; b.notice.show({text: 'x'});
  assert.equal(b.vs.progressCalls.length, 1); assert.equal(b.notice.active, true);
});

test('ANN-53 events that an older VS Code or a reduced host lacks only drop their own trigger', async () => {
  const {vs, ft, notice} = rig({}, {missing: ['onDidStartTerminalShellExecution', 'onDidChangeActiveTerminal']});
  delete vs.window.state;
  const h = notice.show({text: 'x'});
  assert.equal(vs.live(), TRIGGER_COUNT - 2);
  await ft.advance(700);
  vs.fire('onDidChangeTextEditorSelection', editor('file'));
  assert.equal(await h.dismissed, 'selection');
  await assertClean(vs, ft, notice, 'reduced host');
});

test('ANN-54 default wiring (real setTimeout/Date.now) shows and dismisses without leaving a timer or a throw', async () => {
  const vs = makeVscode();
  const notice = new TransientNotice(vs);
  const h = notice.show({text: 'x'});
  assert.equal(notice.active, true);
  notice.dismiss('api');
  assert.equal(await h.dismissed, 'api');
  await assertClean(vs, {q: []}, notice, 'real timers');
});

test('ANN-55 the module takes vscode by injection (never requires it) and documents the observability limits', () => {
  const src = fs.readFileSync(path.join(__dirname, '..', '..', 'editor-bridges', 'vscode', 'src', 'ui', 'notice.js'), 'utf8');
  assert.doesNotMatch(src, /require\(['"]vscode['"]\)/);
  assert.match(src, /Mouse MOVEMENT is not observable/);
  assert.match(src, /terminal/i);
});
