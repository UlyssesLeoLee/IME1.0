# Acceptance checklist — what is automated and what still needs a person

v0.2: the VS Code scenarios are now automated by `tests/uat/` (24 scenarios on real VS Code 1.137.0, all passed on 2026-10-30,
see `02_Verification/STATUS.md`). The items below that are NOT automated have **not been executed** by anyone.

## Automated (Playwright, real VS Code, mock provider)

| Old id | Scenario | UAT |
|---|---|---|
| G1-01/G1-02/G1-03 | F1 on a dirty buffer, per-hunk Diff, apply to the buffer only, one Ctrl+Z | UAT-23, UAT-24 |
| G1-04/G1-05 | Edit during generation / cursor moved -> refused | UAT-25 (document); cursor case is a unit test (UT-049 family) |
| G1-06 | F1 without a key -> clear error, no mutation | UAT-20 |
| G2-01 | Key only in SecretStorage (disk scan of the profile) | UAT-20 |
| G2-02/G2-03 | Realtime AI needs authorisation; off -> no calls | UAT-35, UAT-08 |
| G2-04 | Provider unavailable -> local candidates keep working | UAT-31, UAT-26 |
| G3-01 | Local candidates for the profiles that VS Code can open | UAT-02..06 |
| G3-02 | Unsupported/unresolved machine code -> no bytes | UAT-06 |
| G3-03 | Secrets near the cursor are filtered or the user must decide | UAT-28 |

## Still manual / not executed

| ID | Procedure | Needs |
|---|---|---|
| G0-01..04 | Build and sign the TSF DLL, register 3 profiles, type in Notepad, unregister and reboot | Windows 11 VM, MSVC, Windows SDK, test certificate |
| G2-real | Real provider (OpenAI / compatible / Anthropic) with a test key: request shape, model ids, effort acceptance | a disposable API key and network access |
| ACC | Screen reader, high-contrast theme, 100-250 % DPI, multi-monitor, keyboard-only walk-through | a person with the tools |
| SOAK | 30 minutes offline typing across the seven profiles (ST-038), CPU idle <= 1 % (ST-039) | benchmark machine |
| VER | Other VS Code versions, other extensions editing the same buffer, remote/virtual workspaces | matrix of hosts |
| INST | Signed install / repair / upgrade / uninstall with rollback | installer (not written) |

**No checkbox in this document indicates a test was executed by a person.**
