# Secret-class files: Programming IME asks before it reads one

Requirement added by the project owner after the design baseline (SRS v1.2 has no ids for it; tests use ids `SEC-xx`, UAT ids `UAT-50..55`). Original wording (Chinese):
"通过获取.env等文件名规则，首先分析该文件是否属于密钥类，如果有，弹出确认窗口，用户确认可以读取，才读取".

Meaning: BEFORE the extension reads the content of a file for any purpose, it looks at the file NAME and PATH only. If the name is secret-class (`.env`, a private key, a keystore,
credentials ...), a confirmation window is shown and the file is read only after the user confirmed it.

This is the READ gate. It is separate from, and sits in front of, the downstream gates, which are unchanged: the content scan / redaction / confirmation of what would be SENT
(`rt.engine.privacy`, `core/secrets.js`) and the AI consent (grants L1/L3/L4). Allowing Programming IME to READ `.env` does not allow it to SEND anything: the consent dialog and the
content scan still run, and the content scan still refuses a file with a sensitive name (UAT-52 shows it on F2).

## What counts as secret-class

* The built-in rules of `src/core/secretfile.js` (`classifyFile`), decided from the name and the folders above it, never from the content. The author's list is the contract
  (SEC-05 / SEC-06 in `tests/js/ut-secretfile.test.js`): `.env` and its variants (`.env.local`, `prod.env`, `.envrc`, `.dev.vars`), private keys (`id_rsa*`, `*.pem`, `*.key`, `.ssh/*`, `.gnupg/*`),
  keystores (`*.pfx`, `*.jks` ...), password databases (`*.kdbx` ...), cloud credentials (`.aws/credentials`, `.kube/config`, service-account JSON ...), credential and token files
  (`credentials.json`, `secrets.yml`, `token.json`, `.npmrc`, `.netrc`, `.pgpass` ...), infrastructure state (`*.tfstate`, `*.tfvars`).
  Templates (`.env.example`, `*.sample`), public keys (`*.pub`), source files named like a secret (`credentials.ts`) and design tokens (`tokens.json`) are NOT secret-class.
* Your own patterns: `programmingIme.secretFilePatterns` (array of globs, default `[]`). A pattern without a slash matches the file name, with a slash it matches the path from any folder,
  a trailing slash means everything below it; case is ignored. The patterns ADD to the built-in rules. **No setting switches the gate or a built-in rule off**, and the setting is
  `application` scoped (user settings only), so a workspace cannot weaken the list.
* Backup and editor-swap copies (`.env.bak`, `.env.swp`) count as the file they copy. Names are never decoded (a path, not a URI, is classified).

## What the user sees

| Where | Situation | What happens |
|---|---|---|
| F1, F2 (explicit commands) | secret-class, nothing decided yet | a MODAL: the file label and kind, "reading is local; sending to an AI provider is asked separately", buttons **Allow once** / **Allow for this session**. Esc / Cancel refuses THIS call and remembers nothing: the next command asks again |
| typing, candidates, Show Code Candidates, insertion annotation | secret-class, nothing decided yet | never a modal (it would steal keystrokes). ONE non-modal notification per file and session: **Allow this file for this session** / **Keep off**. Meanwhile no candidates, nothing read |
| any | the user chose "Keep off" | refused silently, for the rest of the session (8 h), without nagging. F1 / F2 say in one line that the file is protected and name the command below |
| any | allowed for the session | no dialog again until the session grant expires (8 h) |
| any | allowed once | the grant is spent by exactly one read (the call that asked) and then gone |
| status bar | the ACTIVE file is secret-class and not allowed | the language item reads `$(lock) PIME protected file`; the tooltip explains; clicking it opens the command |
| command palette, status bar menu | `Programming IME: Secret Files` | a QuickPick: allow the active file for this session, forget a decision (label, state, expiry, folder), or forget all. A forgotten decision is asked again (and announced again) |

A decision is stored in memory only (`SecretFileGate`), per canonical key (case-insensitive drive and name on Windows, one key for every spelling of the same path). It never holds content and
is not written to disk: a reload of the window forgets every decision.

## Real path: links, junctions

A symbolic link, a junction or a mapped drive can hide a secret file behind an innocent name. For a `file:` document the real path is resolved (`fs.realpath.native`, which also expands 8.3 short
names; the plain path when that fails) and BOTH paths are classified: the file is secret-class if either is. The decision is kept under the real path, so every name of one file shares it
(allow `.env` once and a link `notes.txt` to it is covered). The result is cached for 5 seconds (the typing path asks on every keystroke); the status bar and the language detection need a synchronous
answer and resolve the real path synchronously on a cache miss. A document of another scheme (`untitled`, `git`, remote) is judged by its name only.

## Every read point and how it is gated

| Read point | How it is gated |
|---|---|
| `extension.js` inline provider (typing; also the realtime-AI payload, which is cut from the same document afterwards) | `ensure(doc, {interactive: false})` right after the enabled / paused / auto checks and BEFORE `lang.infoFor`; refusal returns no items |
| `extension.js` `showCandidates` (Ctrl+Alt+Space, command) | `ensure(doc, {interactive: false})` before `lang.infoFor` and `doc.lineAt` / `getText`; refusal shows one info message |
| `languageState.infoFor` (status bar redraw, F1, F2, candidates) | reads the first 400 lines for language detection; for a locked document (`isLocked`: sync, real path included) the head is NOT read: the name decides. The lock state is part of the cache key |
| `extension.js` `updateStatus` | uses `lockInfo` (no read); shows the lock and switches the item's command to `programmingIme.secretFiles` |
| `f1.js` `open` -> `openSession` | `ensure(doc, {interactive: true, purpose: 'f1'})` before the first `getText`, and before an open session is closed; refusal = one info message, no panel, the session on another file stays. A stale session that is refused on reopen is not regenerated |
| `f1.js` apply / change tracking (`stateOf`, `onDocumentChanged`) | runs only inside an approved session (the stale-check re-reads the document of the session that was allowed) |
| `f2.js` `run` | after the empty-selection check (nothing to read, nothing to ask) and before `capture` reads the selection and the file: `ensure(doc, {interactive: true, purpose: 'f2'})`; refusal = a notice, the clipboard is untouched |
| `annotate.js` `annotateInsertion` (candidate insertion with autoComment / graphComment) | after the AI-availability check (no AI = no read = no question), before `doc.getText`: `ensure(doc, {interactive: false, purpose: 'annotate'})`; refusal inserts the snippet unchanged, silently |
| `annotate.js` `directorySymbols` (siblings for the Cypher graph; also used by F1) | no prompt, ever: every sibling is classified by its full path (and under the real folder when the folder is a link), secret-class siblings are never read, open in the editor or not, and their names are not passed to the engine; symlinks stay excluded (`FileType.File` only) |
| `extension.js` `reloadRules`, `core/settings.js`, `core/languages.js` | read the extension's own storage and language packs, not user documents: not gated |

`tests/js/ut-secret-wiring.test.js` (SEC-46) lists the files that read document content; a new reader fails that test until it is wired to the gate and listed.

## Deny memory and one-use grants

* "Keep off" is a `deny` in the gate: remembered for the session, refused silently, listed in the command (so it can be forgotten). Esc on a modal remembers nothing.
* "Allow once" is recorded and spent by the same call (`record` then `consume`, no await between). A one-use grant that already exists is spent by the first read after it.
* The gate never reads content; a decision error fails closed (`ensure` returns `false`, `isLocked` returns `true`).

## Diagnostics

Decisions are recorded with `rt.diagnostics.record({component: 'secretfile', error_code: <rule id>, request_type: 'sec.<event>.<purpose>'})`: the rule id of the built-in rule (or `user-pattern`)
and the event (`prompt`, `once`, `session`, `refused`, `notice`, `deny`, `manual`, `revoke`, `revokeAll`, `internal`) with the purpose (`f1`, `f2`, `candidates`, `annotate`, `notice`, `manual`).
Never a path, a file name or content. A decision is one event, not one per keystroke.

## What it does NOT cover (known limits)

* **Content-based detection stays where it was** (`privacy.assess`, `core/secrets.js`): a file whose name looks harmless but whose text holds a key is read, and its text is scanned
  and redacted before anything is sent. The read gate decides by name only.
* A **hard link** hides its target behind another name and `realpath` does not see through it. Not detected.
* **8.3 short names** are expanded by the native `realpath` when the file exists; for a path that cannot be resolved the name is judged as written.
* Real links and junctions were **not tested for real**: symbolic link creation is blocked on the development machine. The real-path logic is tested with a fake `realpath` (SEC-35, SEC-36, SEC-44),
  and the UAT spec does not create a link either.
* The gate controls what Programming IME reads. VS Code itself has the file open and other extensions can read it; this is not an access-control feature.
* With "Allow once" the language detection that runs after the grant is spent (for example the F1 session's own `infoFor`) uses the file name only; the F1 session already holds the text it read.
* The decisions are lost when the window reloads; there is no persistent allow list (by design: a decision is for the session).
* The Rust engine has no part in this gate (it is all JavaScript bridge code). The interface texts live in `core/i18n.js` and are mirrored in the Rust i18n table, so the new `sec.*` texts need the same
  sync as the `f1.ann.*` texts had (`crates/pime-core/src/i18n_table.rs`, `tests/fixtures/i18n-cases.json`, the hard-coded key counts) before the i18n parity tests pass again.

## Tests

`tests/js/ut-secret-wiring.test.js`: SEC-30..46 (the gate, the real path, the setting, diagnostics, the command, F1, F2, annotation, siblings, the real `extension.js` and `languageState.js` against a fake `vscode`,
the manifest and texts) and ANN-86 (F1 opens on the Whole file tab when only the language or an empty document makes the annotate tab unavailable). Classification and the decision store: `tests/js/ut-secretfile.test.js` (SEC-01..15).
`tests/uat/tests/07-secret-files.spec.js`: UAT-50..55 on a real VS Code, **written but not run**.
