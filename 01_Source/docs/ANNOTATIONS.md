# Annotations on inserted code: `autoComment` and `graphComment`

Requirements added by the project owner after the design baseline (SRS v1.2 has no ids for them; tests use ids `ANN-xx`).
They extend FR-016 (authorisation), FR-049 (privacy filter) and the AI contract of SRS §6.3 without changing them.

> The same wishes also drive **F1 annotate** (F1 with no selection comments the scope under the cursor, with a selection exactly the selected lines; comments are
> anchored by our code, reviewed as hunks): see `docs/F1_ANNOTATE.md` (settings `programmingIme.f1Mode`, `programmingIme.f1AnnotateOnOpen`).

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `programmingIme.autoComment` | **on** | Code inserted from the candidate list gets very detailed, beginner-level ("nanny-level") comments written by the user's own AI, which sees the whole current file so the comments are accurate. |
| `programmingIme.graphComment` | **off** (decision: see below) | Also adds a Cypher comment block that describes the inserted code as a node and its edges to other symbols. |
| `programmingIme.graphCommentScope` | `directory` | `file`: relate only to symbols of the current file. `directory`: also to the **names** (never the code) of symbols in the other files of the same folder. |

Nothing happens while AI is off (`aiEnabled`), not configured (no model/key), the workspace is untrusted, or the language has
no line comments (Rust, Python, TypeScript, PowerShell, C, Assembly and Machine Code are covered). The checkbox being ticked by
default therefore never sends anything on its own.

`graphComment` is off by default because it sends additional workspace information (symbol names of sibling files). Ticking it is the
explicit act; a one-line change of the default is possible if you prefer it on.

## Behaviour

1. Where: a candidate picked in `Programming IME: Show Code Candidates` (rule candidates and AI candidates). The F1 panel paths get the same
   wishes as extra instructions to the model (the proposal is reviewed in the panel / Diff before anything is written). Inline ghost-text
   acceptance (Tab) is **not** annotated - VS Code gives the extension no hook there.
2. Consent (FR-016): the first time, a modal states the receiver, that the COMPLETE file is sent and - for the graph - that symbol NAMES of
   other files are sent. Choices: this request only / this workspace for 24 h / cancel (= insert unchanged). Revocation: `Manage AI Permissions`.
   The directory-names data uses grant scope `L4` (read-only context from other files), the file uses `L3`.
3. Privacy: the whole-file text goes through the existing secret scan. A file with high-risk content is **not** sent for annotation; the snippet
   is inserted unchanged with a notice. Medium-risk values are redacted before sending. Sensitive file names (`.env`, keys...) in the folder are
   never read for the symbol index; only `{kind, name}` pairs leave the machine.
4. Safety contract: the AI may only add comments. The commented snippet is accepted only if, after removing comments and normalising whitespace
   (Python keeps indentation), it is identical to the original snippet including strings and `${placeholders}`. Otherwise the comments are
   discarded, the original snippet is inserted and the user is told. Cypher lines are not taken as text: each item must start with
   MERGE/MATCH/CREATE, must not contain DELETE/DETACH/DROP/CALL/LOAD CSV, is limited to 12 lines of 300 characters, and is re-rendered by us with the
   language's own line-comment prefix (a trailing backslash is removed because it would continue a C line comment). `${` inside comments is neutralised
   so it cannot become a snippet tab stop.
5. Never blocks typing: a progress notification with Cancel; cancel, timeout, error or a stale position (the user moved or typed while waiting)
   fall back to unchanged insertion or the existing stale-candidate refusal. The result is one editor operation (one `Ctrl+Z`).
6. Comment language follows the UI language (`programmingIme.uiLocale`): zh-CN, ja-JP or English.

## Example (Rust, both settings on)

```rust
// [graph] Cypher - node/edge relations of this block:
// MERGE (f:File {name:'main.rs'})
// MERGE (m:Function {name:'main', file:'main.rs'})
// MERGE (f)-[:DEFINES]->(m)
// The entry point: the operating system starts your program here.
fn main() {
    // Write your own steps below this line, one per line.
}
```

## Where the code lives, and the Rust migration

Pure logic: `src/core/annotate.js` (prompt, verification, Cypher rendering) and `src/core/symbols.js` (heuristic symbol index, names only);
`maskSource` in `validators.js` gained an optional `spans` argument (behaviour unchanged, parity fixtures still pass). VS Code glue: `src/annotate.js`.
Per `docs/RUST_MIGRATION_PLAN.md` this logic is to be ported to the Rust core (plan step 1.3): add `annotate.*` and `symbols.*` operations,
generate `annotate-cases.json` / `symbols-cases.json` fixtures from the JS version first, then delete the JS.

## Evidence

`tests/js/ut-annotate.test.js` (ANN-01..07) and `tests/uat/tests/04-annotate.spec.js` (ANN-10..15, real VS Code with the mock provider). Not covered: real AI providers,
Tab acceptance of inline suggestions, very large folders.
