# Shared helper of the guest side scripts: keyboard injection and "type into Notepad, read it back".
# Dot-sourced by the other scripts of this folder (they all run in the interactive session).

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class PimeKeys {
    [DllImport("user32.dll", SetLastError = true)] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int cmd);
    public const uint KEYUP = 0x0002, UNICODE = 0x0004;
    public static void TypeUnicode(string text) {
        foreach (char c in text) {
            keybd_event(0, 0, UNICODE, (UIntPtr)c);
            keybd_event(0, 0, UNICODE | KEYUP, (UIntPtr)c);
        }
    }
    public static void Press(byte vk) {
        keybd_event(vk, 0, 0, UIntPtr.Zero);
        keybd_event(vk, 0, KEYUP, UIntPtr.Zero);
    }
    public static void Chord(byte modifier, byte key) {
        keybd_event(modifier, 0, 0, UIntPtr.Zero);
        keybd_event(key, 0, 0, UIntPtr.Zero);
        keybd_event(key, 0, KEYUP, UIntPtr.Zero);
        keybd_event(modifier, 0, KEYUP, UIntPtr.Zero);
    }
}
'@

function Invoke-PimeKeySequence {
    # Sends one key combination. The language is small on purpose: "ctrl+alt+space", "enter", "tab", "esc",
    # "shift+tab", "ctrl+s" - the keys the product documents (docs/TSF_VM_TEST_PLAN.md, section 8).
    param([Parameter(Mandatory = $true)][string]$Sequence)
    $names = @{
        'space' = 0x20; 'enter' = 0x0D; 'tab' = 0x09; 'esc' = 0x1B; 'backspace' = 0x08
        'left' = 0x25; 'up' = 0x26; 'right' = 0x27; 'down' = 0x28; 'home' = 0x24; 'end' = 0x23
    }
    $parts = $Sequence.ToLower().Split('+')
    $key = $parts[-1].Trim()
    $modifiers = if ($parts.Length -gt 1) { @($parts[0..($parts.Length - 2)]) } else { @() }
    # Everything is checked before a single key is pressed: a modifier must never stay stuck down.
    $vk = 0
    if ($names.ContainsKey($key)) {
        $vk = $names[$key]
    } elseif ($key.Length -eq 1) {
        $vk = [byte][char]::Parse($key)
    } else {
        throw "unknown key '$key' in the sequence '$Sequence'"
    }
    $codes = @()
    foreach ($modifier in $modifiers) {
        switch ($modifier.Trim()) {
            'ctrl' { $codes += 0x11 }
            'alt' { $codes += 0x12 }
            'shift' { $codes += 0x10 }
            default { throw "unknown modifier '$modifier' in the sequence '$Sequence'" }
        }
    }
    foreach ($code in $codes) { [PimeKeys]::Press([byte]$code) }
    [PimeKeys]::Press([byte]$vk)
    for ($i = $codes.Count - 1; $i -ge 0; $i--) { [PimeKeys]::Press([byte]$codes[$i]) }
}

function Invoke-PimeNotepadTyping {
    # Types $Text into a real Notepad window (the file is opened by Notepad itself, so Ctrl+S saves in place) and
    # returns what Notepad wrote to the disk. $Backspaces are sent after the text, $Keys are key sequences.
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text,
        [int]$Backspaces = 0,
        [string[]]$Keys = @(),
        [int]$SettleSeconds = 2
    )
    if (Test-Path -LiteralPath $Path) { Remove-Item -LiteralPath $Path -Force }
    Set-Content -LiteralPath $Path -Value '' -Encoding UTF8 -NoNewline
    $notepad = Start-Process -FilePath 'notepad.exe' -ArgumentList "`"$Path`"" -PassThru
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline -and $notepad.MainWindowHandle -eq 0) {
        Start-Sleep -Milliseconds 200
        $notepad.Refresh()
    }
    if ($notepad.MainWindowHandle -eq 0) {
        Stop-Process -Id $notepad.Id -Force -ErrorAction SilentlyContinue
        throw "the Notepad window did not appear within 30 s"
    }
    [void][PimeKeys]::ShowWindow($notepad.MainWindowHandle, 9)   # SW_RESTORE
    [void][PimeKeys]::SetForegroundWindow($notepad.MainWindowHandle)
    Start-Sleep -Seconds $SettleSeconds
    [PimeKeys]::TypeUnicode($Text)
    Start-Sleep -Milliseconds 300
    foreach ($key in $Keys) {
        Invoke-PimeKeySequence -Sequence $key
        Start-Sleep -Milliseconds 400
    }
    for ($i = 0; $i -lt $Backspaces; $i++) { [PimeKeys]::Press(0x08) }   # VK_BACK
    Start-Sleep -Milliseconds 300
    [PimeKeys]::Chord(0x11, 0x53)                                       # Ctrl+S
    Start-Sleep -Seconds 2
    Stop-Process -Id $notepad.Id -Force -ErrorAction SilentlyContinue
    $saved = Get-Content -LiteralPath $Path -Raw -Encoding UTF8
    return @{
        ok = $true
        path = $Path
        length = [Text.Encoding]::UTF8.GetByteCount($saved)
        sha256 = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
        text = $saved
    }
}
