# Manual E2E acceptance checklist (not executed)

Record OS build, VS Code version, installed language packs, source commit and logs.

| ID | Procedure | Expected |
|---|---|---|
| G0-01 | In Windows 11 VM install zh-CN, en-US, ja-JP; test-sign TSF DLL | Build succeeds and recognized by Windows (needs proof) |
| G0-02 | Register via regsvr32 and inspect each language profile | Three PIME entries; default IMEs preserved |
| G0-03 | In Notepad select each profile, type English | All key events pass through; no lost chars |
| G0-04 | Unregister and reboot | No PIME entry, original input still functional |
| G1-01 | Open a dirty Rust file in VS Code; trigger F1 | Explicit send-consent and two edit alternatives |
| G1-02 | Accept full-file proposal after reviewing Diff | Only current buffer modified; file not saved automatically |
| G1-03 | Press Ctrl+Z | Original buffer restored (must be measured) |
| G1-04 | Modify file while AI request in flight | STALE_DOCUMENT refusal |
| G1-05 | Move cursor before accepting Insert Here | STALE_SELECTION refusal |
| G1-06 | Trigger F1 without API Key | Clear error, no file mutation |
| G2-01 | Set OpenAI/compatible endpoint and a test key | Key stays in VS Code SecretStorage |
| G2-02 | Enable realtime AI, approve per-workspace notice | Only nearby code sent to configured host |
| G2-03 | Disable realtime AI | No more API calls |
| G2-04 | Kill provider connection | Local candidate search still works |
| G3-01 | Run local template for each of 7 languages | Correct language-specific candidate |
| G3-02 | Try unsupported x86-64 instruction | Explicit unsupported; no fabricated bytes |
| G3-03 | Paste API credentials in code near cursor | User should review/exclude before authorizing upload |

**No checkbox in this document indicates a test was executed.**
