# Windows TSF G0 pass-through adapter

**EXPERIMENTAL / Windows-only / not yet built or exercised on Windows 11.**

This implements a minimal in-process COM `ITfTextInputProcessor` and `ITfKeyEventSink`.
All keys intentionally pass through (`pfEaten = FALSE`). This is meant ONLY to validate
registration/activation under `zh-CN (0x0804)`, `en-US (0x0409)`, and `ja-JP (0x0411)`.
It has **no candidate window, no Rust IPC wiring, no AI inference, no TSF Composition edits**.
Those are upcoming G0/G1 engineering tasks, not completed capabilities.

Never install an unsigned test DLL as a general-use IME. Test in a Windows 11 VM with
snapshot and rollback; official third-party IME code signing requirements still apply.
The prototype GUID values are temporary until product-release ID governance.

## Experimental Windows build

From a Visual Studio 2022 developer PowerShell with Windows SDK and CMake:

```powershell
cmake -S windows/tsf -B build/tsf -A x64
cmake --build build/tsf --config Debug
```

After verified signing and review, registration may be tested using 64-bit
`C:\Windows\System32\regsvr32.exe` with the DLL's full path. To unregister use `/u`.
For safety there is intentionally no automatic installer/registration in the scripts.

**Known risk:** HKCU COM+TSF profile registration and language list appearance have not
been proven in the test target. Signing, per-user vs. machine registration, language
packs, activation, host process bitness and rollback need G0 validation. The C++ code
may still require SDK-specific build fixes.

Reference: https://learn.microsoft.com/en-us/windows/win32/tsf/text-service-registration
