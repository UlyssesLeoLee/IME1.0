# Programming IME — Playwright user-acceptance tests

Drives the real Visual Studio Code (`Code.exe`) with the extension loaded from `../editor-bridges/vscode`
(`--extensionDevelopmentPath`) in a throw-away profile (own user-data dir, extensions dir, workspace, SecretStorage).
Your own VS Code settings, extensions and secrets are not touched.

```powershell
npm install
npx playwright test                       # all 24 scenarios (about 4 minutes)
npx playwright test tests/02-ai-f1.spec.js -g "UAT-23"
```

Requirements: Windows, VS Code stable (set `VSCODE_EXE` if not in the default location), OpenSSL on PATH or
`OPENSSL_PATH` (Git for Windows ships `mingw64/bin/openssl.exe`).

- `helpers/mock-provider.js`: local HTTPS provider that records every request (OpenAI Responses, OpenAI-compatible Chat,
  Anthropic Messages shapes). Request counts are the evidence for "nothing is sent".
- `helpers/vscode.js`: launch, quick-input, dialog, editor and F1-panel (webview) helpers.
- `tests/01-local.spec.js` local candidates; `02-ai-f1.spec.js` key lifecycle, consent, F1, Diff, stale, secrets;
  `03-policy.spec.js` routing/effort, budget/429, untrusted workspace, UI languages, permissions, realtime AI.
- Results: `../../02_Verification/uat/uat-results.json`, screenshots in `.../uat/screenshots/`.

Notes: the mock certificate is self-signed and the extension host ignores `NODE_EXTRA_CA_CERTS`, so the harness starts VS Code
with `NODE_TLS_REJECT_UNAUTHORIZED=0` (that process only). Candidate lists are opened through the command
(`Show Code Candidates`): the Ctrl+Alt+Space chord did not trigger it in synthetic key events (cause not investigated), so the
keybinding itself is NOT covered by these tests.
The scenarios depend on VS Code DOM class names and may need maintenance after VS Code updates.
