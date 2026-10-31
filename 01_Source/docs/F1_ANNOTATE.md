# F1 annotate: press F1, the scope under the cursor (or the selection) gets comments

Requirement added by the project owner after the design baseline (SRS v1.2 has no ids for it; tests use ids `ANN-60..85`, UAT ids `UAT-42..49`).
It extends FR-016 (authorisation), FR-049 (privacy filter) and the F1 review/apply contract (FR-031..040) without changing them. Original wording (Chinese):
"按f1时，如果没有圈选，就自动识别当前的scope，给这个scope加注释，如果有圈选，就给选中的内容加注释，运用涌现式设计把这个核心功能完善到位"

## What the user gets

* **F1, no selection**: the innermost declaration (function, method, class, impl, assembly label ...) around the cursor is the scope. If the cursor is not in a
  declaration the scope is the paragraph ("block") around it.
* **F1, with a selection**: exactly the selected lines (snapped to whole lines; a selection that ends at column 0 of a line does not include that line).
* The panel opens on the new **C. Annotate scope** tab and, when AI is available, **starts the request on its own** (after the consent modal the first time).
  The result is a normal reviewable proposal: one block per added comment, checkboxes, result preview, syntax check, **Open full Diff**, **Apply selected blocks**
  (one WorkspaceEdit = one `Ctrl+Z`), stale protection. Nothing is written until Apply.
* **Only comment lines are added.** The model never rewrites code: it answers "above scope line N put this comment" and OUR code
  (`applyScopeReply`, `src/core/annotate.js`) builds the text, so the code and the comments that are already there cannot change. Anchors that would change the
  program (inside a string, template, block comment, after a line continuation, above a `#!` line ...) are never offered.

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `programmingIme.f1Mode` | `annotate` | `annotate`: F1 opens on the Annotate tab. `edit`: F1 is exactly the previous behaviour (opens on *Whole file*, Generate runs the whole-file and the insert request, nothing starts on its own); the Annotate tab stays reachable and idle. Read each time F1 opens. |
| `programmingIme.f1AnnotateOnOpen` | `true` | With `annotate`: start the first request when F1 opens. `false`: F1 only shows the scope and waits for Generate. |

Existing settings are reused: `autoComment` (default on) and `graphComment` (default off) initialise the two checkboxes **Comments** / **Cypher graph** of the tab
(changing them in the tab is for this session only and never writes a setting), `graphCommentScope` says whether the Cypher block may use the symbol **names** of
the other files of the folder (`directory`, needs the L4 grant) or only the current file (`file`). If both checkboxes are off the tab says what to tick and nothing is sent.

In `annotate` mode **Generate and Enter in the intent field run the tab on screen only** (annotate by default; *Whole file* or *Insert* if the user switched to
it), so one F1 is one request, never three. The intent text is passed to the prompt as `intent` (the label of the field reads "What should the comments focus on?").

## The tab

* Header `Scope: fn parse_header (lines 12-22, 11 lines)` / `Selection: lines 14-18 (5 lines)` / `Block around the cursor: ...` / `Whole file: ...`.
* Breadcrumb of the widening chain, innermost first: `fn parse_header > impl Parser > file` (the current entry in bold).
* **Wider / Narrower** move along the chain (disabled at the ends and while a request runs). Changing the scope, or a checkbox, resets the tab to *idle*:
  it never sends anything by itself; press **Add comments** / Regenerate / Generate. The first entry that is too large is shown crossed out with the reason.
* The target range is highlighted in the editor (below).
* Notes under the status: `N comment(s) skipped: not a safe position`, `... duplicates of a comment that is already there or already proposed`,
  `... empty, malformed or over the size limits`, `No Cypher graph comment was added ...`, `N secret-like value(s) were redacted ...`.
* A reply that adds nothing is **ready** with "Nothing to add" (not an error). A reply that is not the expected JSON, or that would have changed code (the
  invariant check of `applyScopeReply`), is an **error** with its own wording and Regenerate is offered (the same rule as F2: an unusable reply is never "nothing to do").

Unavailable (message only, no request, no highlight): AI off, untrusted workspace, language without line comments (`unknown`...), empty document, not a file or untitled
buffer, file over 2 MiB, scope too large.

## Limits

A scope may have at most **20 000 characters and 1 200 lines** (a wider chain entry above that is not offered). The line limit exists because the review diff
(`core/diff.js`, LCS with a 4 million cell cap) degrades into ONE "removed N lines, added M lines" block past ~2 000 lines; measured: 1 500 lines of short lines give
51 blocks, 2 000 lines give a single block. The file as a whole is the context (L3) and is limited to 2 MiB like the whole-file tab. A scope that is itself too large has nothing
narrower in the chain (the chain only widens): the message tells the user to select a smaller part and press F1 again.

## Highlight

A whole-line `TextEditorDecorationType` in theme colours (`editor.rangeHighlightBackground`, overview ruler `editorOverviewRuler.rangeHighlightForeground`) on the editor of the
document, visible while the annotate tab is on screen and the proposal can still be applied. It never takes focus and never changes the selection; `revealRange`
(`InCenterIfOutsideViewport`) is used only after Wider / Narrower and only when the new scope is entirely out of view. It follows the scope, moves to a new editor object
when the tab is shown again (`onDidChangeVisibleTextEditors`), and is disposed on stale, Apply, tab switch, panel close and `closeSession`.

## Request, privacy, consent (`src/f1.js`, `runAnnotate`)

* Scenario `f1Insert` (no new scenario), `maxOutputTokens >= 3000`, grant scope **L3** (the whole file) plus **L4** only for the directory graph. One modal says both
  (the L3 text, then the NAMES paragraph of `annotate.consent.graph`); a one-use L4 grant is consumed with the request; a workspace grant is reused without asking.
* The sensitivity gate of F1 runs on the whole file as everywhere in F1 (medium risk: redacted silently, high risk: *Redact and send* / *Send unredacted* / close = nothing sent).
* Redaction: the **whole file is redacted once** (one redaction set; both engines number markers from 1 on every call), the scope is cut out of the redacted text **by line
  range** (the request is refused when the redaction changed the number of lines), and the prompt is built from the redacted text. The comments are applied to the **original**
  scope text and file text (the line numbers are the same) and in the comment texts every `PIME_REDACTED_n` becomes the literal `<redacted>`: a secret is never restored into a comment.
* The request carries only the base name of the file, the numbered scope, the list of anchorable lines, the whole file, symbol names of this file (and of the folder for the directory graph).

## Where the code is

`src/f1.js` (flow; `createF1(env)` is injected like `createF2`, the real `vscode`/runtime are wired on first use), `src/ui/f1panel.js` (webview), `src/core/scope.js` (scope
resolver, ANN-20..29), `src/core/annotate.js` (`scopeAnchors`, `buildScopePrompt`, `applyScopeReply`, ANN-30..39), i18n keys `f1.ann.*` (36, all three locales).
The proposal reuses the whole-file machinery (`rt.engine.propose`, `select`, `hunkEdits`, `commit()` with kind `full`); `recompute`, `toggleHunk`, `openDiff` and `applyProposal`
take a tab name instead of being tied to the whole-file tab, whose behaviour is unchanged (its element ids, messages and requests are the same).

**Engines.** `scope.js` and the anchored-comment functions are plain JavaScript used directly with BOTH engines. The Rust core has no operations for them yet: they belong to the
P1C parity port (to be listed in `docs/RUST_MIGRATION_PLAN.md` section 13 by the integrator). Everything else the tab uses (snapshot, privacy, propose/select/hunkEdits,
coordinator, broker, symbols) already goes through the engine adapter (the provider call itself stays in JS with either engine). The flow was exercised against the real `pime-core`
once, in ANN-85 (propose, select, hunkEdits, coordinator apply, redaction of the core; skipped without `PIME_CORE_EXE`); everything else in the tests uses the JS engine, and the
Rust path is the same propose/commit path the whole-file tab already uses.

**i18n parity.** The 36 `f1.ann.*` keys are only in `core/i18n.js`: `crates/pime-core/src/i18n_table.rs`, the key counts and `tests/fixtures/i18n-cases.json` are synced by the
integrator, and until then the JS suite fails exactly two tests (`PARITY fixtures are exactly what the generator produces`, whose output names one drifted fixture,
`i18n-cases.json`, and `CFG engine: every interface text, in every locale, is the JS text`, the key set of the Rust table).

## What the first slice taught us

The first slice was the whole loop with fakes: resolve the scope of real functions of this repository, a fake reply, `applyScopeReply`, `engine.propose`, then the same through
`createF1` with a fake editor. What it changed:

1. **A one-line scope in a CRLF file.** `applyScopeReply` takes the line break from the first `\n` of the scope text; a one-line scope has none, so the comment came out with a
   bare `\n` in a CRLF document. The editor normalises inserted text to the document's line ending, so the coordinator's read-back of the edit would not match the proposal
   (`EDIT_UNKNOWN` after the write). `fitEol` now uses the document's line break when the scope has none; the fake document of the tests normalises like the editor (ANN-72, caught by a mutant). Core suggestion: take the document EOL as an input.
2. **A chain that only widens cannot be narrowed.** `chain` is cut at the first entry above `maxChars`, so "Wider disabled because the next entry is too large" cannot be read from
   it, and a `tooLarge` scope has no narrower candidate at all. The scope is resolved with `maxChars: Infinity` and cut by `f1.js` instead, which gives the UI the first blocked
   entry (crossed out, with its size) and lets `tooLarge` say "select a smaller part and press F1 again".
3. **Long scopes ruin the review.** 1 500 short lines gave 51 blocks, 2 000 gave one block "-1979 +2046" for comment-only changes (the diff cell cap). Hence the 1 200 line limit next to the character limit (ANN-63).
4. **The first request must not hold the open lock.** `open()` refuses a second F1 while a session is being created; awaiting the request inside it would have swallowed every F1 for
   the whole request. The request starts after the lock is released, and `reopen` (stale -> Generate) passes `autoStart: false`, otherwise the same F1 sent two requests (both caught by mutants).
5. **The Cypher block and the comment above the first line are one block.** Both are insertions at the same place, so the diff gives one hunk: they can only be applied together
   (the other comments are separate blocks: Cypher + a comment above line 1 + a comment above line 2 gives 2 blocks, not 3; the corpus sweep, which commented every line and added the Cypher block, showed it in all 86 scopes).
6. **The existing whole-file check is advisory on real code.** Across 86 scopes of this repository's JS, Rust and PowerShell files every proposal was accepted by `applyScopeReply`, had
   0 invalid anchors, only pure insertions, and the same code; resolve + prompt + apply + propose took 15 ms on average (35 ms at most). The whole-file fingerprint differed in the 18 scopes of
   `core/scope.js` only - a file whose baseline already fails the syntax check (regex literals blind `maskSource`) - which the existing "baseline already invalid: advisory" rule handles; nothing to change.
7. **Duplicate detection looks only above the anchor.** Anchoring on the first line of an existing comment run puts the new comment right above the old one and is not seen as a
   duplicate (my first test did this by accident); `dropped.duplicate` also counts a second comment for one line, so the note says "duplicates", not "the same explanation is already there". Reported to the core owner.
8. **One `obtainGrants` for L3 + L4 says the wrong thing.** With two runnables for one host it asks "the complete document and a nearby excerpt". The annotate tab has its own modal (the text of the
   insertion annotation), and the webview, which rebuilds its panels on every state, lost the keyboard focus after toggling a checkbox or Wider: it now puts the focus back by id.

## Tests

* `tests/js/ut-f1-annotate.test.js` (ANN-60..85): routing (cursor / selection / blank), Wider and Narrower, size limits, options, consent L3/L4, redaction (the secret is not in the request
  body, comments read `<redacted>`, line-count refusal), nothing to add / unusable reply / provider error, Apply as one undo step, stale, CRLF, Open Diff, edit-mode regression, active-tab-only
  Generate, auto-start conditions, unavailable reasons, six languages and real repository files, the highlight (set, reused, moved, disposed, never revealing needlessly), reopen, helpers, and the
  page script run against a small fake DOM (ids, messages, the whole-file ids unchanged). All against fakes: no real VS Code window, no real provider.
* `tests/uat/tests/06-annotate-scope.spec.js` (UAT-42..49): **written, not run** (no Playwright, VS Code or UAT run was possible; syntax-checked only). Existing UAT specs keep their behaviour:
  `helpers/vscode.js` sets `programmingIme.f1Mode: 'edit'` in the base profile; the mock provider labels the new request `scope`. The editor highlight is read from Monaco's own overlay
  DOM (`highlightedLines()`, a guess at its structure: if it is wrong it returns nothing, UAT-43 then fails and the "cleared after Apply" check of UAT-46 proves nothing).

## Deviations from the work-item brief, open questions

* `f1.js` became `createF1(env)` (injection, like `f2.js`); the module exports are unchanged plus `onVisibleEditorsChanged` (hooked in `extension.js`) and `settled()` for the tests.
* `tooLarge`: Narrower is disabled (nothing narrower exists), the message says what to do; plus the line limit (finding 3). Wider-blocked shows the reason.
* An unusable reply is an error, "nothing to add" is ready (finding: follows F2). The annotate request uses its own consent function instead of `obtainGrants`.
* The command title `programmingIme.f1Edit` now reads "F1 AI (Annotate Scope / Whole File / Insert)" (nls files, id unchanged). A new intent label for the annotate tab.
* Open: when the annotate tab is unavailable only for the language (`unknown`) F1 still opens on it; falling back to *Whole file* would be kinder. A one-use consent asks again on every F1 (workspace
  authorisation avoids that). Should the auto-start wait one beat so that a quick second F1 does not show a consent modal? The Rust parity port of `scope.js` / `applyScopeReply` is not done.
