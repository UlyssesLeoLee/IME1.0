'use strict';
// Scope resolver for F1 (ids ANN-20..ANN-29): with no selection the innermost declaration around the cursor is annotated, with a
// selection the selected lines. The resolver is a heuristic text scan, so most of these tests feed it code that is half typed.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {core} = require('./helpers');
const SC = core('scope');
const SY = core('symbols');
const {resolveScope} = SC;

const join = (...ls) => ls.join('\n');
const eol = text => text.split(/\r\n|\r|\n/);
const lineStart = (text, k) => { let off = 0; for (const [i, l] of text.split(/(\r\n|\r|\n)/).entries()) { if (i % 2 === 0 && i / 2 === k) return off; off += l.length; } return off; };
// 0-based line and column -> offset
const pos = (text, line, col = 0) => lineStart(text, line) + col;
const run = (profile, text, line, col = 0, extra = {}) => resolveScope({profile, text, start: pos(text, line, col), end: pos(text, line, col), ...extra});
const sel = (profile, text, l1, c1, l2, c2, extra = {}) => resolveScope({profile, text, start: pos(text, l1, c1), end: pos(text, l2, c2), ...extra});
const brief = r => r.ok ? `${r.kind}:${r.label}:${r.startLine}-${r.endLine}` : r.reason;
const chain = r => r.chain.map(c => `${c.kind}:${c.label}:${c.startLine}-${c.endLine}`);
const slice = (text, r) => text.slice(r.start, r.end);
// every line of [from, to] resolves to the same declaration
const same = (profile, text, from, to, expected) => { for (let k = from; k <= to; k++) assert.equal(brief(run(profile, text, k)), expected, `line ${k}`); };

// ---------------------------------------------------------------------------------------------------------------- Rust
const RUST = join(
  'use std::io;',                                   // 0
  '',                                               // 1
  '/// Parses a header.',                           // 2
  '#[inline]',                                      // 3
  'pub fn parse_header(x: &str) -> u32 {',          // 4
  '    let v = "}{ fn fake() {";',                  // 5
  '    // } a comment brace {',                     // 6
  '    if x.is_empty() {',                          // 7
  '        return 0;',                              // 8
  '    }',                                          // 9
  '    1',                                          // 10
  '}',                                              // 11
  '',                                               // 12
  'impl Foo {',                                     // 13
  '    /// Adds.',                                  // 14
  '    fn add(&self) {',                            // 15
  '        let c = |x| {',                          // 16
  '            x + 1',                              // 17
  '        };',                                     // 18
  '        fn nested() {',                          // 19
  '            let y = 1;',                         // 20
  '        }',                                      // 21
  '    }',                                          // 22
  '',                                               // 23
  '    fn other(&self) {}',                         // 24
  '}',                                              // 25
  '',                                               // 26
  'const N: u32 = 3;',                              // 27
  'let loose = 1;'                                  // 28
);

test('ANN-20a rust: cursor anywhere in a function (attributes, doc comment, header, body, closing line) resolves to the whole function', () => {
  same('rust', RUST, 2, 11, 'declaration:fn parse_header:2-11');
  const r = run('rust', RUST, 8);
  assert.deepEqual([r.kind, r.symbolKind, r.symbolName, r.label], ['declaration', 'fn', 'parse_header', 'fn parse_header']);
  assert.equal(slice(RUST, r).split('\n')[0], '/// Parses a header.', 'the doc comment and the attribute above the header belong to the function');
  assert.equal(slice(RUST, r).split('\n').pop(), '}', 'the range ends at the end of the closing line, without its line break');
  assert.equal(r.start, pos(RUST, 2)); assert.equal(r.end, pos(RUST, 11, 1));
  assert.deepEqual(chain(r), ['declaration:fn parse_header:2-11', 'file:file:0-28']);
  assert.equal(r.tooLarge, false);
});

test('ANN-20b rust: an impl block yields to the method inside it, a nested fn to its own body, a closure stays part of its method', () => {
  assert.equal(brief(run('rust', RUST, 13)), 'declaration:impl Foo:13-25', 'on the impl header');
  assert.equal(brief(run('rust', RUST, 23)), 'declaration:impl Foo:13-25', 'on a blank line between two methods');
  same('rust', RUST, 14, 18, 'declaration:fn add:14-22');
  assert.equal(brief(run('rust', RUST, 22)), 'declaration:fn add:14-22', 'closing line of the method');
  same('rust', RUST, 19, 21, 'declaration:fn nested:19-21');
  assert.equal(brief(run('rust', RUST, 24)), 'declaration:fn other:24-24', 'one-line method with its closing brace on the same line');
  assert.equal(brief(run('rust', RUST, 25)), 'declaration:impl Foo:13-25', 'closing line of the impl');
  const r = run('rust', RUST, 20);
  assert.deepEqual(chain(r), ['declaration:fn nested:19-21', 'declaration:fn add:14-22', 'declaration:impl Foo:13-25', 'file:file:0-28']);
  assert.deepEqual(r.chain.map(c => c.symbolKind), ['fn', 'fn', 'impl', null]);
});

test('ANN-20c rust: between items and at top level the paragraph is the block; a blank line uses the nearest paragraph above, else below', () => {
  assert.equal(brief(run('rust', RUST, 0)), 'block:block:0-0', 'a paragraph never swallows the declaration under it');
  assert.equal(brief(run('rust', RUST, 27)), 'block:block:27-28');
  assert.equal(brief(run('rust', RUST, 28)), 'block:block:27-28');
  assert.equal(brief(run('rust', RUST, 1)), 'block:block:0-0', 'blank line: the paragraph above');
  assert.equal(brief(run('rust', RUST, 12)), 'block:block:2-11', 'blank line after a function: that whole function, however many blank lines it has inside');
  assert.equal(brief(run('rust', RUST, 26)), 'block:block:13-25');
  const lead = join('', '', 'let a = 1;', 'let b = 2;');
  assert.equal(brief(run('rust', lead, 0)), 'block:block:2-3', 'blank line with nothing above: the paragraph below');
  assert.deepEqual(chain(run('rust', RUST, 0)), ['block:block:0-0', 'file:file:0-28']);
});

test('ANN-20d rust: items, macros, impl-for, generics and array types in the header are named like symbols.js names them', () => {
  const text = join(
    '#![allow(dead_code)]',                         // 0
    '#[derive(Debug)]',                             // 1
    'pub struct Foo<T> {',                          // 2
    '    a: [u8; 4],',                              // 3
    '}',                                            // 4
    'impl<T: Fn() -> i32> Foo<T> where T: Copy {',  // 5
    '    pub(crate) async fn go(&self, v: [i32; 3]) -> Result<[u8; 2], E> {', // 6
    '        1',                                    // 7
    '    }',                                        // 8
    '}',                                            // 9
    'impl fmt::Display for Foo {',                  // 10
    '    fn fmt(&self) {}',                         // 11
    '}',                                            // 12
    'macro_rules! twice {',                         // 13
    '    ($x:expr) => { $x + $x };',                // 14
    '}',                                            // 15
    '#[cfg(test)]',                                 // 16
    'mod tests {',                                  // 17
    '    #[test]',                                  // 18
    '    fn t() {}',                                // 19
    '}',                                            // 20
    'enum E { A, B }',                              // 21
    'trait T { fn a(&self); fn b(&self) {} }',      // 22
    'unsafe impl Send for Foo {}',                  // 23
    'extern "C" fn cb() {}'                         // 24
  );
  assert.equal(brief(run('rust', text, 3)), 'declaration:struct Foo:1-4', 'the derive attribute belongs to the struct');
  assert.equal(brief(run('rust', text, 7)), 'declaration:fn go:6-8');
  assert.equal(brief(run('rust', text, 6, 40)), 'declaration:fn go:6-8', 'array types with ; inside the parameters do not end the header');
  assert.equal(brief(run('rust', text, 5)), 'declaration:impl Foo:5-9', 'nested generics: impl<T: Fn() -> i32> Foo<T>');
  assert.equal(brief(run('rust', text, 11)), 'declaration:fn fmt:11-11');
  assert.equal(brief(run('rust', text, 10)), 'declaration:impl Display for Foo:10-12');
  assert.equal(run('rust', text, 10).symbolName, 'Foo');
  assert.equal(brief(run('rust', text, 14)), 'declaration:macro_rules! twice:13-15');
  assert.deepEqual([run('rust', text, 14).symbolKind, run('rust', text, 14).symbolName], ['macro', 'twice']);
  assert.equal(brief(run('rust', text, 19)), 'declaration:fn t:18-19', 'the #[test] attribute is part of the function, the mod around it is the next step');
  assert.deepEqual(chain(run('rust', text, 19)), ['declaration:fn t:18-19', 'declaration:mod tests:16-20', 'file:file:0-24']);
  assert.equal(brief(run('rust', text, 21)), 'declaration:enum E:21-21');
  assert.equal(brief(run('rust', text, 22, 17)), 'declaration:trait T:22-22', 'the body-less trait method is no declaration of its own');
  assert.equal(brief(run('rust', text, 22, 2)), 'declaration:trait T:22-22');
  assert.equal(brief(run('rust', text, 22, 32)), 'declaration:fn b:22-22', 'but one with a body is, even on the same line as the trait');
  assert.deepEqual(chain(run('rust', text, 22, 32)), ['declaration:fn b:22-22', 'file:file:0-24'], 'both are the same single line, the trait adds nothing');
  assert.equal(brief(run('rust', text, 23)), 'declaration:impl Send for Foo:23-23');
  assert.equal(brief(run('rust', text, 24)), 'declaration:fn cb:24-24');
  for (const k of [3, 7, 11, 14, 19, 21]) { // whatever symbols.js recognises on a header line, the resolver names the same way
    const r = run('rust', text, k), header = eol(text).slice(r.startLine, r.endLine + 1).find(l => SY.extractSymbols('rust', l, 1).length);
    if (header) { const s = SY.extractSymbols('rust', header, 1)[0]; if (s.kind !== 'macro') assert.deepEqual([r.symbolKind, r.symbolName], [s.kind, s.name], header); }
  }
});

test('ANN-20e rust: braces, keywords and quotes inside strings, raw strings, char literals and comments are ignored', () => {
  const text = join(
    'fn real() {',                                  // 0
    '    let a = "} fn fake() {";',                 // 1
    '    let b = r#"} "quoted" {"#;',               // 2
    '    let c = \'}\';',                           // 3
    '    let d: &\'static str = "x";',              // 4
    '    /* } fn fake2() { */',                     // 5
    '    // fn fake3() {',                          // 6
    '}',                                            // 7
    '',                                             // 8
    'fn second() {',                                // 9
    '}'                                             // 10
  );
  same('rust', text, 0, 7, 'declaration:fn real:0-7');
  same('rust', text, 9, 10, 'declaration:fn second:9-10');
});

test('ANN-20f rust: else-if chains, match arms and closures are blocks of their function, never declarations', () => {
  const text = join(
    'fn pick(v: i32) -> i32 {',                     // 0
    '    if v > 1 {',                               // 1
    '        1',                                    // 2
    '    } else if v > 0 {',                        // 3
    '        2',                                    // 4
    '    } else {',                                 // 5
    '        match v { 0 => { 3 } _ => 4 }',        // 6
    '    }',                                        // 7
    '}'                                             // 8
  );
  same('rust', text, 0, 8, 'declaration:fn pick:0-8');
});

// ---------------------------------------------------------------------------------------------------------- TypeScript
const TS = join(
  '// header',                                      // 0
  'import x from "y";',                             // 1
  '',                                               // 2
  '@Component({',                                   // 3
  '  selector: "a"',                                // 4
  '})',                                             // 5
  'export class Foo extends Bar {',                 // 6
  '  private n = 1;',                               // 7
  '  constructor(private a: number) {',             // 8
  '    super();',                                   // 9
  '  }',                                            // 10
  '  async load(key: string): Promise<void> {',     // 11
  '    if (key) { return; }',                       // 12
  '    else if (a) {',                              // 13
  '    }',                                          // 14
  '    for (const x of y) {',                       // 15
  '    }',                                          // 16
  '  }',                                            // 17
  '  get value() { return 1; }',                    // 18
  '  static make = () => {',                        // 19
  '    return 1;',                                  // 20
  '  }',                                            // 21
  '}',                                              // 22
  '',                                               // 23
  'const f = (a, b) => {',                          // 24
  '  const g = function () {',                      // 25
  '  };',                                           // 26
  '  return 1;',                                    // 27
  '};',                                             // 28
  'const h = x => x + 1;',                          // 29
  'function after() {',                             // 30
  '}',                                              // 31
  'const o = {',                                    // 32
  '  m() {',                                        // 33
  '  },',                                           // 34
  '  p: async (x) => {',                            // 35
  '  },',                                           // 36
  '};',                                             // 37
  'describe("x", () => {',                          // 38
  '  it("y", function () {',                        // 39
  '  });',                                          // 40
  '});'                                             // 41
);

test('ANN-21a typescript: classes, constructors, methods, getters; control flow inside a method never becomes a declaration', () => {
  same('typescript', TS, 3, 6, 'declaration:class Foo:3-22');
  assert.equal(brief(run('typescript', TS, 7)), 'declaration:class Foo:3-22', 'a field is not a declaration');
  same('typescript', TS, 8, 10, 'declaration:method constructor:8-10');
  same('typescript', TS, 11, 17, 'declaration:method load:11-17');
  assert.equal(brief(run('typescript', TS, 18)), 'declaration:method value:18-18');
  same('typescript', TS, 19, 21, 'declaration:function make:19-21');
  assert.equal(brief(run('typescript', TS, 22)), 'declaration:class Foo:3-22');
  assert.deepEqual(chain(run('typescript', TS, 13)), ['declaration:method load:11-17', 'declaration:class Foo:3-22', 'file:file:0-41']);
  const r = run('typescript', TS, 9);
  assert.deepEqual([r.symbolKind, r.symbolName], ['method', 'constructor']);
  assert.equal(slice(TS, run('typescript', TS, 6)).split('\n')[0], '@Component({', 'a multi-line decorator belongs to its class');
});

test('ANN-21b typescript: functions assigned to a name, expression-bodied arrows, property functions and nested functions', () => {
  for (const k of [24, 27, 28]) assert.equal(brief(run('typescript', TS, k)), 'declaration:function f:24-28');
  assert.equal(brief(run('typescript', TS, 25)), 'declaration:function g:25-26', 'a function expression assigned inside another function');
  assert.deepEqual(chain(run('typescript', TS, 25)), ['declaration:function g:25-26', 'declaration:function f:24-28', 'file:file:0-41']);
  assert.equal(brief(run('typescript', TS, 29)), 'declaration:function h:29-29', 'an arrow with an expression body has no braces but is a declaration');
  same('typescript', TS, 30, 31, 'declaration:function after:30-31');
  assert.equal(brief(run('typescript', TS, 32)), 'block:block:32-32', 'an object literal is not a declaration, and its paragraph stops at the methods inside it');
  assert.equal(brief(run('typescript', TS, 33)), 'declaration:method m:33-34', 'an object method is');
  assert.equal(brief(run('typescript', TS, 36)), 'declaration:function p:35-36');
  assert.equal(brief(run('typescript', TS, 39)), 'block:block:37-41', 'test callbacks are anonymous: no declaration (and the `};` above them is part of the paragraph)');
  assert.equal(brief(run('typescript', TS, 1)), 'block:block:0-1', 'comment and import make one paragraph');
  const multi = join('const total = items', '  .map(x => x.n)', '  .reduce((a, b) => a + b, 0);', 'const next = 1;');
  assert.equal(brief(run('typescript', multi, 1)), 'block:block:0-3', 'no declaration without a function');
  const arrow = join('export const total = (items) =>', '  items', '    .map(x => x.n)', '    .reduce((a, b) => a + b, 0);', 'const next = 1;');
  same('typescript', arrow, 0, 3, 'declaration:function total:0-3');
  const typed = join('const handler: Handler = async (req, res): Promise<void> => {', '  res.end();', '};');
  same('typescript', typed, 0, 2, 'declaration:function handler:0-2');
});

test('ANN-21c typescript: if/else/for/while/switch/try/catch/with and call arguments are not methods', () => {
  const text = join(
    'function run(a) {',                            // 0
    '  if (a) {',                                   // 1
    '    one();',                                   // 2
    '  } else if (b) {',                            // 3
    '    two();',                                   // 4
    '  } else {',                                   // 5
    '  }',                                          // 6
    '  while (c) {',                                // 7
    '  }',                                          // 8
    '  switch (d) {',                               // 9
    '    case 1: {',                                // 10
    '      break;',                                 // 11
    '    }',                                        // 12
    '  }',                                          // 13
    '  try {',                                      // 14
    '  } catch (e) {',                              // 15
    '  } finally {',                                // 16
    '  }',                                          // 17
    '  items.forEach(item => {',                    // 18
    '    use(item);',                               // 19
    '  });',                                        // 20
    '  fetch(u).then(function (r) {',               // 21
    '    return r;',                                // 22
    '  });',                                        // 23
    '}'                                             // 24
  );
  same('typescript', text, 0, 24, 'declaration:function run:0-24');
});

test('ANN-21d typescript: regex literals with quotes or braces, strings and template literals cannot unbalance the file', () => {
  const text = join(
    'const re = /["\']{2}\\{\\{/g;',                // 0
    'function a() {',                               // 1
    '  const s = "}" + \'{\' + `${ "{" }`;',        // 2
    '  return s.replace(/[\\/"\']/g, "");',         // 3
    '}',                                            // 4
    'function b() {',                               // 5
    '  const half = a / 2 / 3;',                    // 6
    '}'                                             // 7
  );
  same('typescript', text, 1, 4, 'declaration:function a:1-4');
  same('typescript', text, 5, 7, 'declaration:function b:5-7');
});

test('ANN-21g typescript: JSX text with apostrophes and closing tags does not break the structure of the file (TSX / JSX)', () => {
  const text = join('function App() {', "  return <div>don't <b>stop</b></div>;", '}', 'const Next = () => {', "  return <p>it's {x / 2}</p>;", '};', 'class K {', '  render() {', '    return <a href="/x">y</a>;', '  }', '}');
  same('typescript', text, 0, 2, 'declaration:function App:0-2');
  same('typescript', text, 3, 5, 'declaration:function Next:3-5');
  assert.equal(brief(run('typescript', text, 8)), 'declaration:method render:7-9');
});

test('ANN-21e typescript: destructured and defaulted parameters and object types in the header belong to the header', () => {
  const text = join(
    "function scan(text, {fileName = ''} = {}) {",  // 0
    '  let a = 1;',                                 // 1
    '  return a;',                                  // 2
    '}',                                            // 3
    'async function load(): Promise<{ok: boolean}> {', // 4
    '  return 1;',                                  // 5
    '}',                                            // 6
    'class A {',                                    // 7
    '  m(x: {a: number}): {b: string} {',           // 8
    '    return 1;',                                // 9
    '  }',                                          // 10
    '}',                                            // 11
    'const f = ({a, b}) => {',                      // 12
    '  return a;',                                  // 13
    '};'                                            // 14
  );
  same('typescript', text, 0, 3, 'declaration:function scan:0-3');
  same('typescript', text, 4, 6, 'declaration:function load:4-6');
  same('typescript', text, 8, 10, 'declaration:method m:8-10');
  same('typescript', text, 12, 14, 'declaration:function f:12-14');
});

test('ANN-21f typescript: interface, enum, namespace and type alias with a body are declarations', () => {
  const text = join('export interface Opts {', '  a: number;', '}', 'declare enum E { A }', 'namespace N.M {', '  const x = 1;', '}', 'export type Pair = {', '  a: 1;', '};');
  same('typescript', text, 0, 2, 'declaration:interface Opts:0-2');
  assert.equal(brief(run('typescript', text, 3)), 'declaration:enum E:3-3');
  same('typescript', text, 4, 6, 'declaration:namespace N.M:4-6');
  same('typescript', text, 7, 9, 'declaration:type Pair:7-9');
});

// ------------------------------------------------------------------------------------------------------------- Python
const PY = join(
  'import os',                                      // 0
  '',                                               // 1
  '@decorator',                                     // 2
  '@other(a,',                                      // 3
  '       b)',                                      // 4
  '# about foo',                                    // 5
  'def foo(a,',                                     // 6
  '        b):',                                    // 7
  '    """doc',                                     // 8
  'def not_a_def():',                               // 9
  '    """',                                        // 10
  '    x = [1,',                                    // 11
  '2]',                                             // 12
  '    def inner():',                               // 13
  '        return 1',                               // 14
  '    # trailing comment, deeper than the def',    // 15
  '',                                               // 16
  '# comment for Bar',                              // 17
  'class Bar(Base):',                               // 18
  '    def m(self):',                               // 19
  '        s = "def fake():"',                      // 20
  '        return \\',                              // 21
  '            1',                                  // 22
  '    x = 1',                                      // 23
  '',                                               // 24
  'async def after():',                             // 25
  '    pass',                                       // 26
  'if True:',                                       // 27
  '    def cond(): return 1',                       // 28
  'print(1)'                                        // 29
);

test('ANN-22a python: def and class end where the indentation falls back; decorators and comments directly above belong to them', () => {
  same('python', PY, 2, 12, 'declaration:def foo:2-15');
  assert.equal(brief(run('python', PY, 15)), 'declaration:def foo:2-15', 'a trailing comment indented like the body belongs to it');
  same('python', PY, 13, 14, 'declaration:def inner:13-14');
  assert.equal(slice(PY, run('python', PY, 6)).split('\n')[0], '@decorator', 'a multi-line decorator is carried along');
  for (const k of [17, 18, 23]) assert.equal(brief(run('python', PY, k)), 'declaration:class Bar:17-23');
  assert.equal(brief(run('python', PY, 19)), 'declaration:def m:19-22');
  assert.equal(brief(run('python', PY, 22)), 'declaration:def m:19-22', 'after a backslash continuation');
  assert.equal(brief(run('python', PY, 23)), 'declaration:class Bar:17-23', 'a class attribute is not a def');
  same('python', PY, 25, 26, 'declaration:def after:25-26');
  assert.equal(brief(run('python', PY, 28)), 'declaration:def cond:28-28', 'a one-line def inside an if');
  assert.deepEqual(chain(run('python', PY, 14)), ['declaration:def inner:13-14', 'declaration:def foo:2-15', 'file:file:0-29']);
  assert.deepEqual([run('python', PY, 6).symbolKind, run('python', PY, 6).symbolName], ['def', 'foo']);
  assert.equal(run('python', PY, 25).label, 'def after');
});

test('ANN-22b python: lines inside brackets and triple-quoted strings (even `def` and column-0 text) do not end or start a declaration', () => {
  same('python', PY, 8, 12, 'declaration:def foo:2-15');
  const text = join('def f():', '    s = """', 'not code', 'def g():', '"""', '    return (1,', '2)', 'x = 1');
  same('python', text, 0, 6, 'declaration:def f:0-6');
  assert.equal(brief(run('python', text, 7)), 'block:block:7-7');
});

test('ANN-22c python: between functions is a block, tabs and CRLF indent the same way, a half-typed header is no declaration', () => {
  assert.equal(brief(run('python', PY, 0)), 'block:block:0-0');
  assert.equal(brief(run('python', PY, 16)), 'block:block:2-15', 'blank line after a def: that whole def');
  assert.equal(brief(run('python', PY, 29)), 'block:block:29-29', 'top level statements: the paragraph stops at the declaration above it');
  assert.equal(brief(run('python', PY, 27)), 'block:block:27-27');
  const tabs = join('class A:', '\tdef f(self):', '\t\treturn 1', '\tdef g(self):', '\t\treturn 2');
  assert.equal(brief(run('python', tabs, 2)), 'declaration:def f:1-2');
  assert.equal(brief(run('python', tabs, 4)), 'declaration:def g:3-4');
  const half = join('def a():', '    pass', '', 'def half(', '', 'def after():', '    pass');
  assert.equal(brief(run('python', half, 6)), 'declaration:def after:5-6', 'the unclosed ( of the half-typed header does not swallow the next def');
  assert.equal(brief(run('python', half, 3)), 'block:block:3-3', 'a header without its closing ): is only a paragraph');
  assert.equal(brief(run('python', half, 1)), 'declaration:def a:0-1');
  const nocolon = join('def a()', '    pass');
  assert.equal(brief(run('python', nocolon, 1)), 'block:block:0-1');
});

// ------------------------------------------------------------------------------------------------------------ PowerShell
const PS = join(
  '<#',                                             // 0
  '.SYNOPSIS',                                      // 1
  ' Gets a thing',                                  // 2
  '#>',                                             // 3
  '[CmdletBinding()]',                              // 4
  'function Get-Thing {',                           // 5
  '    param([string]$Name)',                       // 6
  '    $h = @{ a = 1; b = { 2 } }',                 // 7
  '    if ($Name) {',                               // 8
  '        "x"',                                    // 9
  '    }',                                          // 10
  '    $x = @"',                                    // 11
  'function Fake {',                                // 12
  '"@',                                             // 13
  '}',                                              // 14
  '',                                               // 15
  'filter Pick-It { $_ }',                          // 16
  '',                                               // 17
  'class Foo : Base {',                             // 18
  '    [int]$Value',                                // 19
  '    Foo([int]$v) : base($v) {',                  // 20
  '        $this.Value = $v',                       // 21
  '    }',                                          // 22
  '    [string] Name() {',                          // 23
  '        return "n"',                             // 24
  '    }',                                          // 25
  '    static [int] Make() { return 1 }',           // 26
  '}',                                              // 27
  '',                                               // 28
  'foreach ($i in 1..3) {',                         // 29
  '    Write-Host $i',                              // 30
  '}'                                               // 31
);

test('ANN-23a powershell: function, filter, class and its methods; help comment and [CmdletBinding()] directly above belong to the function', () => {
  same('powershell', PS, 0, 14, 'declaration:function Get-Thing:0-14');
  assert.equal(slice(PS, run('powershell', PS, 9)).split('\n')[0], '<#');
  assert.equal(brief(run('powershell', PS, 16)), 'declaration:function Pick-It:16-16');
  assert.equal(brief(run('powershell', PS, 18)), 'declaration:class Foo:18-27');
  assert.equal(brief(run('powershell', PS, 19)), 'declaration:class Foo:18-27', 'a property is not a declaration');
  same('powershell', PS, 20, 22, 'declaration:method Foo:20-22');
  same('powershell', PS, 23, 25, 'declaration:method Name:23-25');
  assert.equal(brief(run('powershell', PS, 26)), 'declaration:method Make:26-26');
  assert.deepEqual(chain(run('powershell', PS, 24)), ['declaration:method Name:23-25', 'declaration:class Foo:18-27', 'file:file:0-31']);
  assert.deepEqual([run('powershell', PS, 7).symbolKind, run('powershell', PS, 7).symbolName], ['function', 'Get-Thing']);
});

test('ANN-23b powershell: a here-string that contains `function` and braces is a string; loops and hash tables are blocks', () => {
  same('powershell', PS, 11, 13, 'declaration:function Get-Thing:0-14');
  assert.equal(brief(run('powershell', PS, 29)), 'block:block:29-31', 'a foreach at the top level is a paragraph');
  assert.equal(brief(run('powershell', PS, 30)), 'block:block:29-31');
  assert.equal(brief(run('powershell', PS, 15)), 'block:block:0-14');
  const lower = join('FUNCTION Show-It', '{', '    param($a)', '}', 'Function Other { }');
  assert.equal(brief(run('powershell', lower, 2)), 'declaration:function Show-It:0-3', 'case-insensitive keyword, brace on its own line');
  assert.equal(brief(run('powershell', lower, 4)), 'declaration:function Other:4-4');
});

// ------------------------------------------------------------------------------------------------------------------- C
const C = join(
  '#include <stdio.h>',                             // 0
  '#define MAX(a, b) \\',                           // 1
  '    ((a) > (b) ? (a) : (b))',                    // 2
  '#define ONE 1',                                  // 3
  '',                                               // 4
  '/* a point */',                                  // 5
  'typedef struct Point {',                         // 6
  '    int x, y;',                                  // 7
  '} Point;',                                       // 8
  '',                                               // 9
  'typedef struct {',                               // 10
  '    int a;',                                     // 11
  '} Anon;',                                        // 12
  '',                                               // 13
  'static int',                                     // 14
  'add(int a, int b)',                              // 15
  '{',                                              // 16
  '    if (a > b) {',                               // 17
  '        return a;',                              // 18
  '    } else if (b) {',                            // 19
  '        return b;',                              // 20
  '    }',                                          // 21
  '    for (;;) { break; }',                        // 22
  '    return a + b; /* } */',                      // 23
  '}',                                              // 24
  '',                                               // 25
  'int proto(int);',                                // 26
  '',                                               // 27
  'enum Color { RED, GREEN };',                     // 28
  '',                                               // 29
  'struct Foo *make(void) {',                       // 30
  '    struct Foo x = {',                           // 31
  '        1',                                      // 32
  '    };',                                         // 33
  '    return 0;',                                  // 34
  '}',                                              // 35
  '',                                               // 36
  'namespace ns {',                                 // 37
  'class A : public B {',                           // 38
  'public:',                                        // 39
  '    void f() const { }',                         // 40
  '    A() : x(1), y(2) { }',                       // 41
  '    bool operator==(const A &o) const {',        // 42
  '        return true;',                           // 43
  '    }',                                          // 44
  '};',                                             // 45
  '}',                                              // 46
  '',                                               // 47
  'int arr[] = {1, 2, 3};'                          // 48
);

test('ANN-24a c: function definitions (K&R layout, `if` is no function), struct/enum/typedef, multi-line #define, prototypes are not declarations', () => {
  same('c', C, 14, 24, 'declaration:function add:14-24');
  assert.equal(brief(run('c', C, 17)), 'declaration:function add:14-24', 'if / else if / for blocks stay part of the function');
  same('c', C, 5, 8, 'declaration:struct Point:5-8');
  same('c', C, 10, 12, 'declaration:struct Anon:10-12');
  assert.equal(brief(run('c', C, 28)), 'declaration:enum Color:28-28');
  same('c', C, 30, 35, 'declaration:function make:30-35');
  assert.equal(brief(run('c', C, 31)), 'declaration:function make:30-35', 'a `struct Foo x = {` initialiser is not a type');
  assert.equal(brief(run('c', C, 26)), 'block:block:26-26', 'a prototype ending in ; is not a definition');
  assert.equal(brief(run('c', C, 48)), 'block:block:48-48');
  assert.equal(brief(run('c', C, 0)), 'block:block:0-0');
  assert.equal(brief(run('c', C, 3)), 'block:block:3-3', 'a one-line #define is a paragraph, only a continued one is a declaration');
  same('c', C, 1, 2, 'declaration:macro MAX:1-2');
  const r = run('c', C, 19);
  assert.deepEqual([r.symbolKind, r.symbolName], ['function', 'add']);
  assert.deepEqual([run('c', C, 11).symbolKind, run('c', C, 11).symbolName], ['struct', 'Anon'], 'a typedef of an anonymous struct is named after the typedef');
});

test('ANN-24b c++: namespaces, classes with access labels, inline methods, constructors with initialiser lists, operators, templates', () => {
  for (const k of [37, 46]) assert.equal(brief(run('c', C, k)), 'declaration:namespace ns:37-46');
  assert.equal(brief(run('c', C, 38)), 'declaration:class A:38-45');
  assert.equal(brief(run('c', C, 39)), 'declaration:class A:38-45', 'an access label is part of the class body');
  assert.equal(brief(run('c', C, 40)), 'declaration:function f:40-40');
  assert.equal(brief(run('c', C, 41)), 'declaration:function A:41-41');
  same('c', C, 42, 44, 'declaration:function operator==:42-44');
  assert.deepEqual(chain(run('c', C, 43)), ['declaration:function operator==:42-44', 'declaration:class A:38-45', 'declaration:namespace ns:37-46', 'file:file:0-48']);
  const t = join('template <typename T>', 'T twice(T v) {', '    return v + v;', '}', '', 'template <class T> struct Box {', '    T v;', '};');
  same('c', t, 0, 3, 'declaration:function twice:0-3');
  same('c', t, 5, 7, 'declaration:struct Box:5-7');
});

test('ANN-24c c: __declspec / __attribute__ / [[attributes]] in front of a function do not hide it, headers that continue over lines work', () => {
  const text = join('__declspec(dllexport) int __stdcall Foo(int a)', '{', '    return a;', '}', '', 'int __attribute__((noinline)) bar(void) {', '    return 1;', '}', '', '[[nodiscard]] int baz() {', '    return 2;', '}',
    '', 'static const char *', 'name_of(', '    int id,', '    int flags)', '{', '    return 0;', '}');
  same('c', text, 0, 3, 'declaration:function Foo:0-3');
  same('c', text, 5, 7, 'declaration:function bar:5-7');
  same('c', text, 9, 11, 'declaration:function baz:9-11');
  same('c', text, 13, 19, 'declaration:function name_of:13-19');
});

// -------------------------------------------------------------------------------------------- assembly and machine code
const ASM = join(
  '; header',                                       // 0
  'section .text',                                  // 1
  'global _start',                                  // 2
  '',                                               // 3
  '; entry',                                        // 4
  '_start:',                                        // 5
  '    mov rax, 1',                                 // 6
  '.loop:',                                         // 7
  '    dec rax',                                    // 8
  '    jnz .loop',                                  // 9
  '    ret',                                        // 10
  '',                                               // 11
  'helper:',                                        // 12
  '    ret',                                        // 13
  '',                                               // 14
  'section .data',                                  // 15
  'msg: db "hi; there", 0',                         // 16
  '',                                               // 17
  'foo PROC',                                       // 18
  'bar:',                                           // 19
  '    nop',                                        // 20
  'foo ENDP',                                       // 21
  'END'                                             // 22
);

test('ANN-25a assembly: a label owns the lines up to the next label, local labels nest in it, PROC..ENDP is one region', () => {
  same('assembly', ASM, 4, 6, 'declaration:label _start:4-10');
  assert.equal(brief(run('assembly', ASM, 9)), 'declaration:label .loop:7-10');
  assert.deepEqual(chain(run('assembly', ASM, 9)), ['declaration:label .loop:7-10', 'declaration:label _start:4-10', 'file:file:0-22']);
  same('assembly', ASM, 12, 13, 'declaration:label helper:12-13');
  assert.equal(brief(run('assembly', ASM, 16)), 'declaration:label msg:16-16', 'a section directive ends the region of the label above it');
  assert.equal(brief(run('assembly', ASM, 20)), 'declaration:label bar:19-20');
  assert.equal(brief(run('assembly', ASM, 18)), 'declaration:proc foo:18-21');
  assert.equal(brief(run('assembly', ASM, 21)), 'declaration:proc foo:18-21');
  assert.deepEqual(chain(run('assembly', ASM, 20)), ['declaration:label bar:19-20', 'declaration:proc foo:18-21', 'file:file:0-22']);
  assert.equal(brief(run('assembly', ASM, 2)), 'block:block:0-2', 'directives before the first label are a paragraph');
  assert.equal(brief(run('assembly', ASM, 11)), 'block:block:4-10', 'blank line after a label region: that region');
  assert.deepEqual([run('assembly', ASM, 6).symbolKind, run('assembly', ASM, 6).symbolName], ['label', '_start']);
  assert.equal(run('assembly', ASM, 20).symbolName, 'bar');
});

test('ANN-25b assembly: comment characters per assembler, a `;` inside a string is no comment, a label-looking comment is no label', () => {
  const gas = join('# fake:', '', 'main:', '    movq %rax, %rbx   # not_a_label:', '    ret', '.L1:', '    nop');
  assert.equal(brief(run('assembly', gas, 3, 0, {dialect: {assembler: 'gas'}})), 'declaration:label main:2-6');
  assert.equal(brief(run('assembly', gas, 6, 0, {dialect: {assembler: 'gas'}})), 'declaration:label .L1:5-6');
  assert.equal(brief(run('assembly', gas, 0, 0, {dialect: {assembler: 'gas'}})), 'block:block:0-0', 'a comment that looks like a label is a comment');
  assert.equal(brief(run('assembly', join('; note', 'main:', '    ret'), 2)), 'declaration:label main:0-2', 'a comment directly above a label belongs to it');
  const nasm = join('msg: db "a; b: c", 0', 'next: ; trailing comment', '    ret');
  assert.equal(brief(run('assembly', nasm, 0)), 'declaration:label msg:0-0');
  assert.equal(brief(run('assembly', nasm, 2)), 'declaration:label next:1-2');
});

test('ANN-25c machine-code: blocks only, a paragraph is a run of non-blank lines, comment lines belong to it', () => {
  const text = join('// prologue', '55', '48 89 E5', '', '48 31 C0 // xor rax, rax', 'C3', '', '90');
  assert.equal(brief(run('machine-code', text, 1)), 'block:block:0-2');
  assert.equal(brief(run('machine-code', text, 5)), 'block:block:4-5');
  assert.equal(brief(run('machine-code', text, 3)), 'block:block:0-2', 'blank line: the paragraph above');
  assert.equal(brief(run('machine-code', text, 7)), 'block:block:7-7');
  for (let k = 0; k < 8; k++) assert.equal(run('machine-code', text, k).symbolKind, null);
  assert.deepEqual(chain(run('machine-code', text, 5)), ['block:block:4-5', 'file:file:0-7']);
});

// ---------------------------------------------------------------------------------------------- selection, chain, limits
test('ANN-26a selection: snapped to whole lines, a partial first or last line is included, an end at column 0 leaves that line out', () => {
  let r = sel('rust', RUST, 5, 10, 8, 4);
  assert.equal(brief(r), 'selection:selection:5-8');
  assert.equal(r.start, pos(RUST, 5)); assert.equal(r.end, pos(RUST, 8, RUST.split('\n')[8].length), 'ends at the end of the last line, without its line break');
  assert.equal(r.symbolKind, null); assert.equal(r.symbolName, null);
  assert.equal(brief(sel('rust', RUST, 5, 0, 9, 0)), 'selection:selection:5-8', 'dragging down whole lines');
  assert.equal(brief(sel('rust', RUST, 5, 0, 5, 8)), 'selection:selection:5-5');
  assert.equal(brief(sel('rust', RUST, 5, 3, 6, 0)), 'selection:selection:5-5', 'ending at column 0 of the next line selects only the first');
  assert.equal(brief(resolveScope({profile: 'rust', text: RUST, start: pos(RUST, 8, 4), end: pos(RUST, 5, 10)})), 'selection:selection:5-8', 'a backwards selection is the same');
  assert.deepEqual(chain(sel('rust', RUST, 5, 10, 8, 4)), ['selection:selection:5-8', 'declaration:fn parse_header:2-11', 'file:file:0-28']);
  assert.deepEqual(chain(sel('rust', RUST, 17, 0, 20, 5)), ['selection:selection:17-20', 'declaration:fn add:14-22', 'declaration:impl Foo:13-25', 'file:file:0-28'], 'only declarations that strictly contain the selection');
});

test('ANN-26b selection: whole-file and whole-function selections, no duplicates in the chain, a selection across two functions widens to the common parent', () => {
  const all = resolveScope({profile: 'rust', text: RUST, start: 0, end: RUST.length});
  assert.equal(brief(all), 'selection:selection:0-28');
  assert.deepEqual(chain(all), ['selection:selection:0-28']);
  const withEol = RUST + '\n';
  const all2 = resolveScope({profile: 'rust', text: withEol, start: 0, end: withEol.length});
  assert.equal(brief(all2), 'selection:selection:0-28', 'the empty line after the final line break is not a line of the selection');
  assert.equal(all2.end, RUST.length);
  const fn = sel('rust', RUST, 2, 0, 11, 1);
  assert.deepEqual(chain(fn), ['selection:selection:2-11', 'file:file:0-28'], 'the function with exactly the selected lines is not repeated as a declaration');
  const across = sel('rust', RUST, 15, 4, 24, 10);
  assert.deepEqual(chain(across), ['selection:selection:15-24', 'declaration:impl Foo:13-25', 'file:file:0-28']);
  assert.deepEqual(chain(sel('rust', RUST, 3, 0, 15, 4)), ['selection:selection:3-15', 'file:file:0-28']);
});

test('ANN-26c selection: only whitespace or blank lines falls back to the cursor at its start; a cursor needs no selection', () => {
  assert.equal(brief(sel('rust', RUST, 12, 0, 13, 0)), brief(run('rust', RUST, 12)), 'two blank lines');
  assert.equal(brief(sel('rust', RUST, 12, 0, 13, 0)), 'block:block:2-11');
  assert.equal(brief(sel('rust', RUST, 8, 0, 8, 8)), 'declaration:fn parse_header:2-11', 'the indentation of a code line is only whitespace');
  assert.equal(brief(sel('rust', RUST, 8, 0, 8, 9)), 'selection:selection:8-8', 'one character of code is a selection');
  assert.equal(brief(resolveScope({profile: 'rust', text: RUST, start: pos(RUST, 8, 3)})), 'declaration:fn parse_header:2-11', 'end defaults to start');
  assert.equal(brief(resolveScope({profile: 'rust', text: RUST, start: pos(RUST, 8, 3), end: pos(RUST, 8, 3)})), 'declaration:fn parse_header:2-11');
});

test('ANN-26d chain: widening candidates are strictly nested, innermost first, cut off before the first that exceeds maxChars; tooLarge when the first does', () => {
  const r = run('rust', RUST, 20);
  const sizes = r.chain.map(c => c.end - c.start);
  assert.ok(sizes[0] < sizes[1] && sizes[1] < sizes[2] && sizes[2] < sizes[3]);
  for (const [limit, count] of [[sizes[3], 4], [sizes[3] - 1, 3], [sizes[2], 3], [sizes[2] - 1, 2], [sizes[1] - 1, 1], [sizes[0], 1]]) {
    const c = run('rust', RUST, 20, 0, {maxChars: limit});
    assert.equal(c.chain.length, count, `maxChars ${limit}`);
    assert.equal(c.tooLarge, false);
    for (const x of c.chain) assert.ok(x.end - x.start <= limit);
    assert.equal(c.chain[0].kind, c.kind);
  }
  const big = run('rust', RUST, 20, 0, {maxChars: sizes[0] - 1});
  assert.equal(big.tooLarge, true, 'the innermost result is longer than maxChars');
  assert.equal(big.kind, 'declaration', 'it is still returned');
  assert.equal(big.label, 'fn nested');
  assert.equal(big.chain.length, 1, 'and nothing wider fits');
  assert.equal(run('rust', RUST, 20, 0, {maxChars: 0}).tooLarge, true);
  assert.equal(run('rust', RUST, 20, 0, {maxChars: -5}).tooLarge, false, 'a nonsensical limit is the default');
  assert.equal(run('rust', RUST, 20, 0, {maxChars: NaN}).chain.length, 4);
  assert.equal(run('rust', RUST, 20, 0, {maxChars: Infinity}).chain.length, 4, 'Infinity: no limit');
  assert.equal(run('rust', RUST, 20, 0, {maxChars: 'many'}).chain.length, 4, 'not a number: the default');
  const whole = resolveScope({profile: 'rust', text: RUST, start: 0, end: RUST.length, maxChars: 10});
  assert.equal(whole.tooLarge, true); assert.equal(whole.kind, 'selection');
  const file = run('rust', RUST, 0, 0, {maxChars: RUST.length - 1});
  assert.deepEqual(chain(file), ['block:block:0-0'], "'file' is only offered when the whole file fits");
  assert.deepEqual(chain(run('rust', RUST, 0, 0, {maxChars: RUST.length})), ['block:block:0-0', 'file:file:0-28']);
  assert.equal(chain(run('rust', RUST, 0, 0, {maxChars: RUST.length})).length, 2);
  const single = 'fn only() {\n}';
  assert.deepEqual(chain(run('rust', single, 0)), ['declaration:fn only:0-1'], 'a function that is the whole file has nothing wider');
});

test('ANN-26e result shape: ok, kind, offsets, lines, label and symbol fields; chain[0] is the result itself; 0-based inclusive lines', () => {
  const r = run('rust', RUST, 16);
  assert.deepEqual(Object.keys(r).sort(), ['chain', 'end', 'endLine', 'kind', 'label', 'ok', 'start', 'startLine', 'symbolKind', 'symbolName', 'tooLarge']);
  assert.deepEqual(Object.keys(r.chain[0]).sort(), ['end', 'endLine', 'kind', 'label', 'start', 'startLine', 'symbolKind', 'symbolName']);
  for (const k of ['kind', 'start', 'end', 'startLine', 'endLine', 'label', 'symbolKind', 'symbolName']) assert.equal(r.chain[0][k], r[k], k);
  assert.equal(r.ok, true);
  assert.equal(RUST.slice(r.start, r.end).split('\n').length, r.endLine - r.startLine + 1);
  const b = run('rust', RUST, 0);
  assert.deepEqual([b.kind, b.label, b.symbolKind, b.symbolName], ['block', 'block', null, null]);
  const f = resolveScope({profile: 'rust', text: RUST, start: 0, end: 0, maxChars: 1e9}).chain.at(-1);
  assert.deepEqual([f.kind, f.label, f.start, f.end, f.startLine, f.endLine], ['file', 'file', 0, RUST.length, 0, 28]);
});

// ------------------------------------------------------------------------------------------ robustness: broken and odd input
test('ANN-27a broken code: an unbalanced brace, an unterminated string or comment, a half-typed header degrade to a block and never throw', () => {
  const open = join('fn a() {', '    let x = 1;', '', 'fn b() {', '    let y = 2;', '}');
  assert.equal(brief(run('rust', open, 1)), 'block:block:0-1', 'the unclosed fn a is not trusted');
  assert.equal(brief(run('rust', open, 4)), 'declaration:fn b:3-5', 'but the complete function after it is');
  const extra = join('fn a() {', '    let x = 1;', '    }', '}', '', 'fn b() {', '}');
  assert.equal(brief(run('rust', extra, 1)), 'declaration:fn a:0-2');
  assert.equal(brief(run('rust', extra, 6)), 'declaration:fn b:5-6');
  const half = join('fn ok() {', '}', '', 'fn foo(', '', 'fn after() {', '    1', '}');
  assert.equal(brief(run('rust', half, 3)), 'block:block:3-3', 'half-typed `fn foo(` is a block');
  assert.equal(brief(run('rust', half, 6)), 'declaration:fn after:5-7', 'and does not swallow the next function');
  assert.equal(brief(run('rust', half, 0)), 'declaration:fn ok:0-1');
  const str = join('fn a() {', '    let s = "unterminated;', '}', '', 'fn b() {', '}');
  assert.equal(brief(run('rust', str, 0)).startsWith('block'), true);
  assert.ok(run('rust', str, 5).ok);
  const cmt = join('fn a() {', '}', '/* never closed', 'fn b() {', '}');
  assert.equal(brief(run('rust', cmt, 1)), 'declaration:fn a:0-1');
  assert.equal(brief(run('rust', cmt, 3)), 'block:block:2-4');
  const tsq = join('function a() {', '  const s = "oops;', '}', 'function b() {', '  return 1;', '}');
  assert.equal(brief(run('typescript', tsq, 0)), 'declaration:function a:0-2', 'a stray quote ends with its line, as in a real JavaScript parser');
  assert.equal(brief(run('typescript', tsq, 4)), 'declaration:function b:3-5');
  const cq = join('int a(void) {', '    char c = \'x;', '}', 'int b(void) {', '}');
  assert.equal(brief(run('c', cq, 4)), 'declaration:function b:3-4');
  const pyq = join('def a():', '    s = "oops', '    return 1', 'def b():', '    pass');
  assert.equal(brief(run('python', pyq, 2)), 'declaration:def a:0-2');
  assert.equal(brief(run('python', pyq, 4)), 'declaration:def b:3-4');
  assert.equal(brief(run('typescript', 'function (', 0)), 'block:block:0-0');
  assert.equal(brief(run('typescript', 'class {\n  m() {\n', 1)), 'block:block:0-1', 'text ending in the middle of a class');
  assert.ok(run('powershell', 'function Foo {\n  if ($x) {\n', 1).ok);
  assert.ok(run('assembly', 'foo:\n  mov rax, [\n', 1).ok);
  assert.ok(run('c', 'int main( {{ )) } }}{', 0).ok);
});

test('ANN-27b the same text with CRLF, LF, CR or mixed line ends resolves to the same lines; offsets are UTF-16 code units', () => {
  for (const [profile, text, probes] of [['rust', RUST, [0, 4, 9, 12, 16, 20, 23, 27]], ['typescript', TS, [1, 5, 9, 13, 20, 25, 29, 33, 39]], ['python', PY, [0, 4, 8, 14, 16, 22, 28, 29]], ['powershell', PS, [3, 9, 12, 15, 24, 29]], ['c', C, [1, 3, 11, 16, 26, 40, 48]], ['assembly', ASM, [3, 9, 13, 20, 22]]]) {
    for (const nl of ['\r\n', '\r']) {
      const other = text.replace(/\n/g, nl);
      for (const k of probes) {
        const a = run(profile, text, k), b = run(profile, other, k);
        assert.equal(brief(b), brief(a), `${profile} ${JSON.stringify(nl)} line ${k}`);
        assert.deepEqual(chain(b), chain(a));
        assert.equal(slice(other, b), slice(text, a).replace(/\n/g, nl), 'the same text is selected');
      }
    }
    const mixed = text.split('\n').map((l, i) => l + (i % 3 === 0 ? '\r\n' : i % 3 === 1 ? '\n' : '\r')).join('').slice(0, -1);
    for (const k of probes) assert.equal(brief(run(profile, mixed, k)), brief(run(profile, text, k)), `${profile} mixed line ${k}`);
  }
  const lone = join('// x', 'fn a() {', '}').replace(/\n/g, '\r');
  assert.equal(brief(run('rust', lone, 2)), 'declaration:fn a:0-2', 'a lone CR ends a line comment, which then belongs to the function under it');
  const emoji = join('fn a() { // 😀 = two code units', '    let s = "𝒳}";', '}', 'fn b() {', '}');
  const r = run('rust', emoji, 1, 15);
  assert.equal(brief(r), 'declaration:fn a:0-2');
  assert.equal(slice(emoji, r), emoji.split('\n').slice(0, 3).join('\n'));
  assert.equal(r.end, emoji.indexOf('\nfn b'), 'surrogate pairs count as two units');
});

test('ANN-27c a BOM, tabs, odd whitespace and unusual arguments are harmless', () => {
  const bom = '\uFEFF' + RUST;
  for (const k of [0, 4, 11, 20, 27]) {
    const a = run('rust', RUST, k), b = run('rust', bom, k);
    assert.equal(brief(b), brief(a));
    assert.equal(b.start, k === 0 || a.start === 0 ? 0 : a.start + 1, 'the BOM is part of the first line');
  }
  assert.equal(run('rust', bom, 0).start, 0);
  const pyBom = '\uFEFF' + PY;
  assert.equal(brief(run('python', pyBom, 6)), brief(run('python', PY, 6)));
  const tab = 'fn a() {\n\tif x {\n\t\ty();\n\t}\n}';
  assert.equal(brief(run('rust', tab, 2)), 'declaration:fn a:0-4');
  const weird = [null, undefined, 42, ''];
  for (const text of weird) assert.deepEqual(resolveScope({profile: 'rust', text, start: 0, end: 0}), {ok: false, reason: 'empty-document'});
  assert.equal(brief(resolveScope({profile: 'rust', text: 'let a = 1;', start: NaN, end: Infinity})), 'selection:selection:0-0');
  assert.equal(brief(resolveScope({profile: 'rust', text: 'let a = 1;\nlet b;', start: -50, end: 1e9})), 'selection:selection:0-1');
  assert.equal(brief(resolveScope({profile: 'rust', text: 'let a = 1;', start: 3})), 'block:block:0-0');
  assert.equal(brief(resolveScope({profile: 'rust', text: 'let a = 1;'})), 'block:block:0-0');
  assert.equal(brief(resolveScope({profile: 'rust', text: 'a\n', start: 2, end: 2})), 'block:block:0-0', 'the cursor after the final line break');
  assert.equal(brief(resolveScope({profile: 'rust', text: 'a b', start: 1.9, end: 1.2})), 'block:block:0-0');
  assert.equal(brief(resolveScope({profile: 'rust', text: RUST, start: 8, end: 3, dialect: null})), 'selection:selection:0-0');
  assert.deepEqual(resolveScope(undefined), {ok: false, reason: 'unsupported-language'});
  assert.deepEqual(resolveScope({}), {ok: false, reason: 'unsupported-language'});
});

test('ANN-27d empty or whitespace-only documents and unsupported profiles are reported, not guessed', () => {
  for (const profile of SC.PROFILES) {
    for (const text of ['', '   ', '\n\n', '\r\n \t\r\n', '\uFEFF', '\uFEFF\n  \u00A0\n']) assert.deepEqual(resolveScope({profile, text, start: 0, end: 0}), {ok: false, reason: 'empty-document'}, `${profile} ${JSON.stringify(text)}`);
  }
  for (const profile of ['cobol', '', 'Rust', 'javascript', null, undefined, 42]) assert.deepEqual(resolveScope({profile, text: 'fn a() {}', start: 0, end: 0}), {ok: false, reason: 'unsupported-language'});
  assert.deepEqual(resolveScope({profile: 'cobol', text: '', start: 0, end: 0}), {ok: false, reason: 'unsupported-language'}, 'the language is checked first');
  assert.deepEqual([...SC.PROFILES], ['rust', 'typescript', 'python', 'powershell', 'c', 'assembly', 'machine-code']);
});

// ------------------------------------------------------------------------------------------- regressions from dogfooding
test('ANN-27e dogfooding regressions: header braces, one-line functions beside their parent, impl blocks, symbol naming', () => {
  // `const store = () => {...}` and the `update: (k, v) => {...}` inside it share a line: the character offset tells them apart
  const one = join("const store = () => { const m = new Map(); return {get: k => m.get(k), update: (k, v) => { m.set(k, v); return 1; }}; };", 'const next = 1;');
  assert.equal(brief(run('typescript', one, 0, 5)), 'declaration:function store:0-0');
  assert.equal(brief(run('typescript', one, 0, one.indexOf('m.set'))), 'declaration:function update:0-0');
  // the same cursor line, both neighbours on one line: whichever starts closest to the cursor
  const two = 'fn a() { } fn b() { }';
  assert.equal(run('rust', two, 0, 2).label, 'fn a');
  assert.equal(run('rust', two, 0, 19).label, 'fn b');
  const shared = join('fn a() {', '} fn b() {', '}');
  assert.equal(brief(run('rust', shared, 0)), 'declaration:fn a:0-1');
  assert.equal(brief(run('rust', shared, 2)), 'declaration:fn b:1-2');
  // a very long impl yields to the method under the cursor on every one of its lines
  const methods = Array.from({length: 40}, (_, i) => `    fn m${i}(&self) {\n        ${i}\n    }`);
  const impl = `impl Big {\n${methods.join('\n')}\n}`;
  for (let i = 0; i < 40; i++) assert.equal(brief(run('rust', impl, 1 + i * 3 + 1)), `declaration:fn m${i}:${1 + i * 3}-${3 + i * 3}`);
  assert.equal(brief(run('rust', impl, 0)), 'declaration:impl Big:0-121');
  // symbols.js and the resolver agree on kind and name wherever symbols.js recognises the header line
  const ts = join('export function load(a) {', '}', 'export class K {', '}', 'export const arrow = async (x) => {', '};', 'const expr = y => y * 2;');
  for (const k of [0, 2, 4, 6]) {
    const r = run('typescript', ts, k), s = SY.extractSymbols('typescript', eol(ts)[k], 1)[0];
    assert.deepEqual([r.symbolKind, r.symbolName], [s.kind, s.name]);
  }
  const py = join('class A:', '    def m(self):', '        pass', 'async def top():', '    pass');
  for (const k of [0, 1, 3]) { const r = run('python', py, k), s = SY.extractSymbols('python', eol(py)[k], 1)[0]; assert.deepEqual([r.symbolKind, r.symbolName], [s.kind, s.name]); }
  const c = join('static int add(int a, int b) {', '    return a + b;', '}', 'struct S {', '};', '#define M(x) \\', '    (x)');
  for (const k of [0, 3, 5]) { const r = run('c', c, k), s = SY.extractSymbols('c', eol(c)[k], 1)[0]; assert.deepEqual([r.symbolKind, r.symbolName], [s.kind, s.name]); }
  const asm = 'start:\n    ret\n';
  assert.deepEqual([run('assembly', asm, 0).symbolKind, run('assembly', asm, 0).symbolName], [SY.extractSymbols('assembly', 'start:', 1)[0].kind, 'start']);
  const psText = 'function Get-It {\n}\nfilter Pick { $_ }';
  for (const k of [0, 2]) { const r = run('powershell', psText, k), s = SY.extractSymbols('powershell', eol(psText)[k], 1)[0]; assert.deepEqual([r.symbolKind, r.symbolName], [s.kind, s.name]); }
});

// ------------------------------------------------------------------------------------------------------ cost and limits
const real = file => fs.readFileSync(path.join(__dirname, '..', '..', file), 'utf8');
const fill = (unit, n) => unit.repeat(Math.ceil(n / unit.length)).slice(0, n);
const MiB = 1024 * 1024;
const timed = fn => { const t = process.hrtime.bigint(); const r = fn(); return [r, Number(process.hrtime.bigint() - t) / 1e6]; };

test('ANN-28a a 2 MiB file resolves quickly and still finds the declaration under the cursor', () => {
  const unit = 'fn item_0() {\n    let a = 1;\n    let b = "}";\n}\n\n';
  const text = fill(unit, 2 * MiB);
  const mid = Math.floor(text.length / 2), cut = mid - (mid % unit.length);
  const [r, ms] = timed(() => resolveScope({profile: 'rust', text, start: cut + 20, end: cut + 20}));
  assert.equal(r.kind, 'declaration'); assert.equal(r.label, 'fn item_0');
  assert.equal(r.start, cut); assert.equal(r.end, cut + unit.length - 2);
  assert.equal(r.startLine, (cut / unit.length) * 5); assert.equal(r.endLine, r.startLine + 3, 'line numbers are those of the whole document');
  assert.ok(ms < 1000, `${ms} ms`);
  for (const [profile, file] of [['typescript', 'editor-bridges/vscode/src/engine.js'], ['rust', 'crates/pime-core/src/input.rs']]) {
    const big = fill(real(file) + '\n', 2 * MiB), [res, t] = timed(() => resolveScope({profile, text: big, start: 1234567, end: 1234567}));
    assert.ok(res.ok, profile); assert.ok(t < 1000, `${profile} ${t} ms`);
    assert.ok(res.start <= 1234567 && res.end >= 1234567);
  }
});

test('ANN-28b a text above the cap is analysed through a window of lines around the selection and never as a whole', () => {
  const unit = 'def f_0():\n    return 1\n\n';
  const text = fill(unit, 2 * MiB + 5000), cut = 1500000 - (1500000 % unit.length);
  const [r, ms] = timed(() => resolveScope({profile: 'python', text, start: cut + 15, end: cut + 15}));
  assert.equal(r.label, 'def f_0'); assert.equal(r.start, cut); assert.equal(r.startLine, (cut / unit.length) * 3);
  assert.equal(r.chain.at(-1).kind, 'declaration', 'no file entry: the whole file was not analysed');
  assert.ok(ms < 1000, `${ms} ms`);
  const [wide, ms2] = timed(() => resolveScope({profile: 'python', text, start: cut, end: cut + 4000, maxChars: 1e9}));
  assert.equal(wide.kind, 'selection');
  assert.equal(wide.chain.at(-1).kind, 'file', 'a limit above the document size offers the whole document');
  assert.ok(ms2 < 1000, `${ms2} ms`);
  const lines = fill('x = 1\n', 3 * MiB);
  const r2 = resolveScope({profile: 'python', text: lines, start: 100, end: lines.length - 3});
  assert.equal(r2.kind, 'selection'); assert.equal(r2.startLine, 16); assert.equal(r2.endLine, Math.floor((lines.length - 3) / 6)); assert.equal(r2.tooLarge, true);
  assert.equal(r2.start, 96, 'snapped to the start of its line');
  const blank = '\n'.repeat(3 * MiB) + 'x = 1\n';
  assert.equal(brief(resolveScope({profile: 'python', text: blank, start: 100, end: 100})), `block:block:100-100`, 'a blank neighbourhood in a document that is not blank');
  assert.deepEqual(resolveScope({profile: 'python', text: '\n'.repeat(3 * MiB), start: 5, end: 5}), {ok: false, reason: 'empty-document'});
});

test('ANN-28c hostile inputs stay cheap: floods of quotes, brackets and headers, unterminated arrows and strings, deep nesting', () => {
  const cases = [
    ['typescript', fill('x = "a;\n', 2 * MiB)], ['typescript', fill('const a = (x) => (\n', 2 * MiB)], ['typescript', fill('x = /["\']/g;\n', 2 * MiB)],
    ['typescript', fill('{', 2 * MiB)], ['c', fill('{(', 2 * MiB)], ['rust', fill('fn f(\n', 2 * MiB)], ['python', fill('def f(\n', 2 * MiB)], ['python', fill('"', 2 * MiB)],
    ['powershell', fill('function f(\n', 2 * MiB)], ['assembly', fill('a:\n', 2 * MiB)], ['c', fill('#define A(x) \\\n  x\n', 2 * MiB)], ['rust', fill('{{{{{{{{}}}}}}}}\n', 2 * MiB)],
    ['python', Array.from({length: 1500}, (_, i) => ' '.repeat(i) + 'def f():').join('\n')], ['rust', fill('/*', 2 * MiB)], ['machine-code', fill('48 89 D8\n', 2 * MiB)]
  ];
  for (const [profile, text] of cases) {
    const [r, ms] = timed(() => resolveScope({profile, text, start: Math.floor(text.length / 2), end: Math.floor(text.length / 2)}));
    assert.ok(r.ok, profile);
    assert.ok(ms < 1500, `${profile} ${JSON.stringify(text.slice(0, 20))} took ${ms} ms`);
  }
});

// ------------------------------------------------------------------------------------------------------------- property
function checkResult(profile, text, s, e, max, label) {
  const r = resolveScope({profile, text, start: s, end: e, maxChars: max});
  assert.equal(r.ok, true, label);
  assert.ok(['selection', 'declaration', 'block'].includes(r.kind), label);
  assert.ok(r.start >= 0 && r.end <= text.length && r.start <= r.end, `${label} bounds`);
  assert.ok(r.start === 0 || /[\r\n]/.test(text[r.start - 1]), `${label} starts at a line start`);
  assert.ok(r.end === text.length || /[\r\n]/.test(text[r.end]), `${label} ends at a line end`);
  assert.equal(r.startLine, eol(text.slice(0, r.start)).length - 1, `${label} startLine`);
  assert.equal(r.endLine, eol(text.slice(0, r.end)).length - 1, `${label} endLine`);
  const cursorLine = text.slice(text.lastIndexOf('\n', s - 1) + 1, (i => i < 0 ? text.length : i)(text.indexOf('\n', s))).replace(/\r/g, '');
  const needsCover = e > s ? /\S/.test(text.slice(s, e)) : /\S/.test(cursorLine);
  if (needsCover) assert.ok(r.start <= s && (e <= r.end || /^[\r\n]*$/.test(text.slice(r.end, e))), `${label}: ${r.kind} ${r.start}-${r.end} must contain ${s}-${e}`);
  assert.deepEqual({...r.chain[0]}, {kind: r.kind, start: r.start, end: r.end, startLine: r.startLine, endLine: r.endLine, label: r.label, symbolKind: r.symbolKind, symbolName: r.symbolName}, `${label} chain[0]`);
  for (let i = 1; i < r.chain.length; i++) {
    const a = r.chain[i - 1], b = r.chain[i];
    assert.ok(b.start <= a.start && b.end >= a.end && (b.start < a.start || b.end > a.end), `${label} chain ${i} strictly contains the previous`);
    assert.ok(b.end - b.start <= max, `${label} chain ${i} fits maxChars`);
  }
  assert.equal(r.tooLarge, r.end - r.start > max, label);
  return r;
}
const SAMPLES = [['rust', RUST], ['typescript', TS], ['python', PY], ['powershell', PS], ['c', C], ['assembly', ASM], ['machine-code', join('55', '48 89 E5', '', '90 // nop', 'C3')],
  ['rust', 'fn a() {\n    let s = "}";\n    fn b() {\n    }\n  \n}\nimpl X {\n    fn c(&self) -> [u8; 2] {\n        [0; 2]\n    }\n\n    fn d(&self) {}\n}'],
  ['typescript', 'class A {\n  m(a: {x: 1}) {\n    if (a) { return /["\']/; }\n  }\n}\nconst f = (x) => x + 1;\nexport default function () {}'],
  ['python', 'class A:\n    @dec\n    def f(self, x=(1,\n                   2)):\n        """doc\n        def fake():"""\n        return \\\n            x\n\n\ndef g(): pass\n']];

test('ANN-29a property: at every cursor position of the samples (LF, CRLF, with BOM) the result is whole lines, contains the cursor and its chain is nested and bounded', () => {
  for (const [profile, base] of SAMPLES) {
    for (const [name, text] of [['LF', base], ['CRLF', base.replace(/\n/g, '\r\n')], ['BOM', '\uFEFF' + base]]) {
      for (let s = 0; s <= text.length; s++) checkResult(profile, text, s, s, 20000, `${profile} ${name} cursor ${s}`);
    }
  }
});

test('ANN-29b property: selections of every kind and every maxChars keep the same invariants', () => {
  let seed = 7;
  const rnd = n => { seed = (seed * 1103515245 + 12345) >>> 0; return seed % n; };
  for (const [profile, text] of SAMPLES) {
    for (let i = 0; i < 400; i++) {
      const a = rnd(text.length + 1), b = rnd(3) === 0 ? a : rnd(text.length + 1);
      checkResult(profile, text, Math.min(a, b), Math.max(a, b), [20000, 200, 60, 0][rnd(4)], `${profile} sel ${Math.min(a, b)}-${Math.max(a, b)}`);
    }
  }
});

test('ANN-29c property: every prefix and every one-character deletion of the samples (code that is being typed) resolves without throwing', () => {
  for (const [profile, text] of SAMPLES) {
    for (let n = 1; n <= text.length; n++) { const t = text.slice(0, n); checkResult(profile, t, n, n, 20000, `${profile} prefix ${n}`); }
    for (let d = 0; d < text.length; d += 3) { const t = text.slice(0, d) + text.slice(d + 1); if (/\S/.test(t)) checkResult(profile, t, Math.min(d, t.length), Math.min(d, t.length), 20000, `${profile} without char ${d}`); }
  }
});

test('ANN-29d corpus: every line of this repository\'s own Rust, JavaScript and C++ sources resolves to whole lines around it, nested and agreeing with symbols.js', () => {
  const root = path.join(__dirname, '..', '..');
  const files = [];
  const walk = (dir, depth) => {
    if (depth > 6 || !fs.existsSync(dir)) return;
    for (const e of fs.readdirSync(dir, {withFileTypes: true})) {
      if (e.name === 'node_modules' || e.name === 'target' || e.name.startsWith('.')) continue;
      const p = path.join(dir, e.name);
      if (e.isDirectory()) walk(p, depth + 1); else if (/\.(rs|js|cpp)$/.test(e.name) && fs.statSync(p).size < 300000) files.push(p);
    }
  };
  for (const d of ['crates', 'editor-bridges/vscode/src', 'tests/js', 'windows']) walk(path.join(root, d), 0);
  if (!files.length) return; // sources not shipped with this checkout: nothing to sweep
  const profileOf = f => f.endsWith('.rs') ? 'rust' : f.endsWith('.js') ? 'typescript' : 'c';
  let positions = 0, declarations = 0;
  const t0 = Date.now();
  for (const f of files) {
    const profile = profileOf(f), text = fs.readFileSync(f, 'utf8'), an = SC._analyse(profile, {}, text), {src} = an;
    const symbols = new Map(eol(text).map((l, k) => [k, SY.extractSymbols(profile, l, 1)[0]]).filter(([, s]) => s));
    const starts = [...new Set(Array.from({length: src.L}, (_, k) => k))];
    for (const k of starts) {
      for (const col of [0, Math.floor((src.ends[k] - src.starts[k]) / 2)]) {
        const off = src.starts[k] + col, r = SC._resolveIn(an, off, off, 20000, false);
        positions++;
        assert.ok(r.ok);
        const where = `${path.basename(f)}:${k + 1}`;
        assert.ok(r.start === src.starts[r.startLine] && r.end === src.ends[r.endLine], `${where} whole lines`);
        if (src.kind[k] !== 0) assert.ok(r.startLine <= k && k <= r.endLine, `${where} contains the cursor line`);
        r.chain.forEach((c, i) => { if (i) { const p = r.chain[i - 1]; assert.ok(c.start <= p.start && c.end >= p.end && (c.start < p.start || c.end > p.end), `${where} chain nesting`); assert.ok(c.end - c.start <= 20000, `${where} chain size`); } });
        if (r.kind === 'declaration') {
          declarations++;
          // whatever symbols.js sees on the header line (a declaration starting on that line, not an inner one) is what the resolver names
          for (let j = r.startLine; j <= r.endLine && j < r.startLine + 12; j++) {
            const s = symbols.get(j);
            if (!s) continue;
            const named = [...(function* walkTree(ds) { for (const d of ds) { yield d; yield* walkTree(d.children); } })(an.roots)].some(d => d.sk === s.kind && d.name === s.name && d.first <= j && j <= d.last);
            if (!named && !(profile === 'c' && s.name === '__declspec')) assert.ok(false, `${where}: symbols.js sees ${s.kind} ${s.name} but the resolver has no such declaration on that line`);
            break;
          }
        }
      }
    }
    // a leaf declaration resolves to itself on every one of its lines; a child never starts where its parent starts
    const all = [...(function* walkTree(ds) { for (const d of ds) { yield d; yield* walkTree(d.children); } })(an.roots)];
    const cover = new Int32Array(src.L);
    for (const d of all) for (let k = d.first; k <= d.last; k++) cover[k]++;
    for (const d of all) {
      if (d.parent) assert.notEqual(d.parent.cs, d.cs, `${path.basename(f)}: ${d.label} starts where its parent starts`);
      if (d.children.length) continue;
      let depth = 1; for (let p = d.parent; p; p = p.parent) depth++;
      const header = (() => { let lo = 0, hi = src.L - 1; while (lo < hi) { const mid = (lo + hi + 1) >> 1; if (src.starts[mid] <= d.cs) lo = mid; else hi = mid - 1; } return lo; })();
      for (let k = d.first; k <= d.last; k++) {
        if (cover[k] !== depth) continue;
        const at = k === header ? Math.max(src.starts[k], d.cs) : src.starts[k]; // a declaration that starts in the middle of its line is found from its first character on
        const r = SC._resolveIn(an, at, at, 20000, false);
        assert.deepEqual([r.kind, r.label, r.startLine, r.endLine], ['declaration', d.label, d.first, d.last], `${path.basename(f)}:${k + 1}`);
      }
    }
  }
  assert.ok(declarations > 1000 && positions > 5000, `${declarations} declarations over ${positions} positions`);
  assert.ok(Date.now() - t0 < 30000, 'the sweep stays fast');
});
