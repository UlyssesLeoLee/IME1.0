# F2 prompt: select text or code, press F2, get a 5W1H prompt on the clipboard

Requirement added by the project owner after the design baseline (SRS v1.2 has no ids for it; tests use ids `PRM-xx`, UAT ids `UAT-36..41`).
It extends FR-016 (authorisation), FR-049 (privacy filter) and the AI contract of SRS §6.3 without changing them. Original wording (Chinese):
"圈选内容后，如果按 F2，会在剪贴板里存储优化这段代码的提示词，要包含 5W1H 的足够完善的提示词，如果圈选的是文字，就基于这段文字优化提示词，
如果圈选的是代码，就审视代码所在文件，给出优化方案提示词，如果已经不存在问题，则弹窗无需优化，但这个弹窗应该出现在右下角，并在下次输入或者鼠标移动时自动消失".

## What the user gets

| Selection | What happens | Clipboard |
|---|---|---|
| prose (a requirement, a draft prompt, a comment) | the user's own AI turns the text into a complete prompt: Who / What / When / Where / Why / How, gaps listed, nothing invented | the prompt |
| code | the user's own AI reviews the FILE around the selection (up to ~400 lines, the whole file when shorter); every finding has to quote the file; the result is an optimisation-plan prompt for the selected code | the prompt |
| code or prose, nothing to fix | a notice at the bottom right: "No optimisation needed" (own wording for code and prose) | **unchanged** |
| AI off / workspace untrusted / no model+key / consent declined / file over 2 MiB | the UNREVIEWED local template (first line says so) and a notice that says why | the template |
| sensitive content the user does not accept | notice "Looks sensitive: nothing was sent and nothing was copied" | **unchanged** |
| the reply is unusable, the request fails | a warning / error message, once | **unchanged** |

**The previous clipboard content is lost** whenever a prompt or a template is copied (there is one system clipboard; F2 does not keep a history).
It is never touched by "no optimisation needed", by a cancelled or failed request, or by a refused sensitive selection.

The prompt (`renderPrompt`, `src/core/promptcraft.js`) has the six headings in the language of the prompt (code: the UI language; prose: the language of the
selected text, kana = Japanese, Han = Chinese, else English), the material in a fenced block marked as data, the checked findings (code) or the gaps (prose),
constraints, acceptance criteria and the requested output format. `verify5W1H` must pass or nothing is copied.

## Command, key and settings

* Command `programmingIme.f2Prompt` ("Programming IME: F2 Prompt (5W1H Prompt to Clipboard)"; titles in `package.nls*.json`).
* Keybinding `F2` when `editorTextFocus && editorHasSelection && programmingIme.f2Key.f2`; `Shift+F2` when `... && programmingIme.f2Key.shiftF2`.
  **F2 normally renames a symbol.** Because of the `editorHasSelection` clause F2 without a selection keeps doing Rename, and nothing is taken over while
  Programming IME is disabled (the default), the binding is `none`/`shift+f2`, or the language is excluded.
* Settings: `programmingIme.f2Binding` (`f2` | `shift+f2` | `none`, default `f2`), `programmingIme.f2ExcludedLanguages` (language ids, default `[]`).
* The context keys follow the rule of F1 (`updateContextKeys` in `extension.js`): enabled + the chosen binding + language not excluded. They are refreshed on a
  configuration change, on the enable toggle and when the active editor changes. A user who wants a selection-only rename can bind `shift+f2` or `none`.
* From the Command Palette with no (or only blank) selection the command shows a notice "Select some text or code first".
* A selection over 200,000 characters is refused with a notice (nothing is classified or sent).

## Flow (`src/f2.js`; pure logic in `src/core/promptcraft.js`)

1. Gate: Programming IME enabled, an editor, a non-blank selection of at most 200,000 characters. One request at a time: a second F2 aborts the first one,
   which ends silently (no error, no clipboard write). Cancel on the progress notification does the same.
2. Classify: `classifySelection({profile, languageId, fileName, text, fileText, start, end})`. `profile` is the Programming IME language when it is one of the seven,
   else empty; `fileText/start/end` are always passed (Markdown fences, comments inside code).
3. AI path available: `aiEnabled()`, trusted workspace, a model and a key for the scenario. **Code uses scenario `f1Full`, prose `f1Insert`**: no new scenario
   (the settings schema and the Rust port know only realtime / f1Full / f1Insert), so the user's F1 choices of provider, model and effort apply.
   Reply budget: `max(maxOutputTokens, 3000)` (the f1Insert default of 1500 would cut a six-field JSON reply).
4. Consent (FR-016, never silent): code needs a grant of scope **L3** (`consent.full`: the file is sent), prose **L1** (`consent.nearby`: only the selection is
   sent). Choices: this request / this workspace for 24 h / cancel. Cancel is a decision, not an error: the template path is taken.
5. Sensitivity gate on exactly what will be sent (code: the numbered file window and the selection; prose: the selection), `privacy.assess(..., scope 'f1')`:
   allow, redact (medium risk, silently, as in F1), confirm (high risk: dialog "Redact and send" / "Send unredacted", closing it means NO), block (hard stop).
   A stop means: nothing is sent **and nothing is copied**.
6. Redaction (mode redact): ONE `privacy.redact` call over the whole file (both engines number the markers from 1 in every call, so two calls into one set would
   reuse `PIME_REDACTED_1` for different values). The selection is cut out of the redacted file by line and column; if the text before the start or after the end
   changed, the whole redacted line is used. If redaction changed the number of lines, nothing is sent (line numbers would no longer match).
7. Request: `buildReviewRequest`, then `rt.ai.infer` with `signal`, `grantId`, `grantScope`, `allowSensitive` (only after "Send unredacted") inside a cancellable
   progress notification. Only the base name of the file is sent, never the path; no intent.
8. Reply: `applyReviewReply(raw, {fileText: <exactly the text the model saw>})`.
   * `ok: false` (not the requested JSON): warning message, clipboard untouched. This is an error, **never** "nothing to optimise".
   * `needsOptimization: false`: the notice, clipboard untouched. If the model claimed problems but none of its findings could be found in the file
     (`findings-unverified`), the notice says exactly that instead ("none of them could be found in your file") and nothing is copied.
   * otherwise `renderPrompt` -> `verify5W1H` -> `clipboard.writeText` -> notice "Copied N characters: paste it into your AI assistant". The notice is always the
     LAST step (anything that reveals, selects or diffs later than 600 ms after `show()` would dismiss it).
9. Errors never reach the user as an exception: `try/catch`, diagnostics `{component: 'f2', error_code, request_type}` (codes and request types only), one error
   message. The text of an `INTERNAL` error is not shown (it may quote content), only its code.

## Privacy

* The provider receives only what the consent said: code = the file window (about 400 lines around the selection, at most 40,000 characters, lines cut at 500) and
  the selection; prose = the selection (cut at 20,000 characters, head and tail). The file name is the base name. No other file, no directory data.
* Secrets: see steps 5 and 6. The client's own last gate (`SENSITIVE_BLOCKED`) still refuses high-risk bytes that were neither redacted nor accepted.
* **Redact mode keeps the markers in the clipboard prompt.** The material block, the evidence quotes and the model's fields all carry `PIME_REDACTED_n`; the prompt says
  so in its own language (`Scope: N secret-like value(s) replaced by PIME_REDACTED_n markers (not restored)`). `privacy.restore` is used only as a check: a field or finding
  that names a marker which is not in the set was made up by the model and is dropped (a code review with no finding left becomes "unverified", nothing copied).
  Reason: evidence quotes are verified against the redacted file, so they contain the marker of a secret-bearing line; restoring them would put the real secret on the
  clipboard, next to a material block that hides it, for a prompt that is meant to be pasted into another AI service. (If the owner prefers restoring, it is a one-line change
  in `keepResolvable`; the trade-off is exactly that leak.) Markers of the file part and of the selection are the same numbers, because the selection is cut out of the redacted file.
* The offline template is also a clipboard write: a high-risk selection or a credential file name is refused there too (no dialog); medium risk is replaced by markers.
* Diagnostics and logs hold codes only: no prompt, no code, no selection (UT-074 and `PRM-33`).
* The clipboard is overwritten only on the two success paths (prompt, template).

## Notice at the bottom right: what can and cannot be done

The notice is `TransientNotice` (`src/ui/notice.js`, its header lists the sources checked). It is a real toast at the bottom right of the window, and it closes by itself on
typing in an editor, a selection change (a click or cursor move in a text editor), scrolling, switching editor, the window losing focus, and after 20 s.
**Mouse movement cannot be observed by a VS Code extension** (the extension host is a separate process; the API has no pointer event), so "disappears when the mouse moves"
is approximated by click/selection and scroll. Typing in a terminal, the Explorer or a webview is not observable either. VS Code shows a toast for at least 800 ms, and with
Do Not Disturb on, a toast becomes a status-bar entry. `workbench.notifications.position` can move toasts to the top right.

## Both engines

Nothing is engine-specific. The AI pipeline is shared: F2 calls `rt.engine.privacy.assess / redactionSet / redact / restore / release`, `rt.broker.find / authorize` and
`rt.ai.infer` (with `signal`, `grantId`, `grantScope`, `allowSensitive`) exactly like `f1.js` and `annotate.js`, on the JS engine and on the Rust engine
(`engine.js`; the Rust `privacy.assess` accepts any scope string, only `realtime` turns "confirm" into "block"). `promptcraft.js` is plain JavaScript in the extension host;
per `docs/RUST_MIGRATION_PLAN.md` it can move to the core later, with fixtures generated from it first.

## Limits

* Markers: a secret in the selection and in the file window gets the same marker (single redaction call). A secret crossing the start or end column of the selection makes that
  edge fall back to the whole line.
* A file over 2 MiB is not reviewed (template with the reason named). Below that, a long file is reviewed through a window of about 400 lines around the selection,
  and the prompt says so ("Only a window of the file around the selection was reviewed").
* The review sees one file; problems that need other files are out of scope, and the prompt says when only a window was reviewed.
* The prose consent text is the existing "nearby code excerpt" wording (scope L1); it says "code" although a text selection is sent.
* i18n keys `f2.*` are in `src/core/i18n.js`; the Rust table and `tests/fixtures/i18n-cases.json` must be synchronised by the integrator.

## Tests

| ID | What |
|---|---|
| PRM-01..09 | pure logic (`ut-promptcraft.test.js`) |
| PRM-20 | routing: code -> f1Full + L3 + numbered file; prose -> f1Insert + L1 + selection only |
| PRM-21 | code review: findings with evidence on the clipboard, notice after the copy, one progress with Cancel |
| PRM-22 | prose language selects the prompt language; prose inside a code comment stays prose |
| PRM-23 | AI off / untrusted / no key / consent declined / file over 2 MiB: UNREVIEWED template, zero requests |
| PRM-24 | redact: no secret in the request or on the clipboard, markers kept, invented markers dropped, "Send unredacted" only on the user's word |
| PRM-25 | needsOptimization:false: notice (code / prose), clipboard untouched; the real `TransientNotice` is the last progress |
| PRM-26 | unusable reply: warning, never "no optimisation needed" |
| PRM-27 | unverifiable findings dropped; none left: nothing copied |
| PRM-28 | sensitive selection / file: never copied, never sent (also AI off) |
| PRM-29 / PRM-30 | a second F2 cancels the first; Cancel on the progress: silent |
| PRM-31 | gates: disabled, no/blank selection, over-large selection |
| PRM-32 | context keys, manifest (keybindings, settings, nls) and `extension.js` wiring |
| PRM-33 | diagnostics and error text carry no content; one error message |
| PRM-34 | selection cut out of the redacted file |
| PRM-35 | i18n completeness; no logging or file writes in `f2.js` |
| UAT-36..41 (`tests/uat/tests/f2-prompt.spec.js`, PRM-40..45) | real VS Code + mock provider: prose, code, bottom-right notice and its disappearance, AI-off template, Rename without a selection, secrets |

The unit tests run against fakes for `vscode`, the clipboard and the notice, with the real JS privacy gate, permission broker and AI client (the provider is a recording fake
fetch, so "no secret in the request" is checked on the real request body). The UAT spec has **not been run yet**; it reads and writes the real Windows clipboard.
