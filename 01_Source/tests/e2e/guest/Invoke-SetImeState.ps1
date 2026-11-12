# Guest side: changes the per-user input method state and reads it back.
# Runs in the interactive session of the logged-on user (the language list and the default input method are per user).
#   powershell -NoProfile -File Invoke-SetImeState.ps1 -ParametersPath <file> -ResultPath <file>
# The parameters file holds: mode ('ensure-tip' | 'set-override'), tip, languageTag, profileGuid.
param(
    [Parameter(Mandatory = $true)][string]$ParametersPath,
    [Parameter(Mandatory = $true)][string]$ResultPath
)
$ErrorActionPreference = 'Stop'
$Parameters = Get-Content -LiteralPath $ParametersPath -Raw -Encoding UTF8 | ConvertFrom-Json
$Output = $null
try {
    $languages = Get-WinUserLanguageList
    $changed = $false
    if ($Parameters.mode -eq 'ensure-tip') {
        # The design step "install and tick that language": the keyboard of the product must be offered for it.
        $entry = $languages | Where-Object { $_.LanguageTag -eq $Parameters.languageTag } | Select-Object -First 1
        if (-not $entry) { throw "the language '$($Parameters.languageTag)' is not in the language list of this user" }
        if (@($entry.InputMethodTips) -notcontains $Parameters.tip) {
            $entry.InputMethodTips.Add($Parameters.tip)
            Set-WinUserLanguageList $languages -Force
            $changed = $true
            Start-Sleep -Seconds 1
            $languages = Get-WinUserLanguageList
        }
    } elseif ($Parameters.mode -eq 'set-override') {
        # The mechanical equivalent of "Win+Space selects the keyboard of the product".
        Set-WinDefaultInputMethodOverride -InputTip $Parameters.tip
        Start-Sleep -Seconds 1
    } else {
        throw "unknown mode '$($Parameters.mode)'"
    }
    $state = @()
    foreach ($language in (Get-WinUserLanguageList)) {
        $state += [ordered]@{ tag = $language.LanguageTag; tips = @($language.InputMethodTips) }
    }
    $override = $null
    try { $override = Get-WinDefaultInputMethodOverride } catch { $override = $null }
    $lastProfile = $null
    if (Test-Path -LiteralPath 'HKCU:\Software\IME1.0') {
        $value = Get-ItemProperty -LiteralPath 'HKCU:\Software\IME1.0' -Name 'LastProfile' -ErrorAction SilentlyContinue
        if ($value) { $lastProfile = "$($value.LastProfile)" }
    }
    $Output = @{ ok = $true; changed = $changed; languages = $state; defaultOverride = $override; lastProfile = $lastProfile }
} catch {
    $Output = @{ ok = $false; error = $_.Exception.Message }
}
$Output | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
