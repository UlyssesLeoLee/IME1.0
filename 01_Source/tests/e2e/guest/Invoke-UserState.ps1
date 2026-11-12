# Guest side: the per-user input method state of the logged-on user.
# Runs in the interactive session (the language list and the default input method are per user).
#   powershell -NoProfile -File Invoke-UserState.ps1 -ParametersPath <file> -ResultPath <file>
param(
    [Parameter(Mandatory = $true)][string]$ParametersPath,
    [Parameter(Mandatory = $true)][string]$ResultPath
)
$ErrorActionPreference = 'Stop'
$Parameters = Get-Content -LiteralPath $ParametersPath -Raw -Encoding UTF8 | ConvertFrom-Json
$Output = $null
try {
    $languages = @()
    foreach ($language in (Get-WinUserLanguageList)) {
        $languages += [ordered]@{ tag = $language.LanguageTag; tips = @($language.InputMethodTips) }
    }
    $override = $null
    try { $override = Get-WinDefaultInputMethodOverride } catch { $override = $null }

    # The profile the product came from (it writes it when it is switched away from).
    $lastProfile = $null
    $key = 'HKCU:\Software\IME1.0'
    if (Test-Path -LiteralPath $key) {
        $value = Get-ItemProperty -LiteralPath $key -Name 'LastProfile' -ErrorAction SilentlyContinue
        if ($value) { $lastProfile = "$($value.LastProfile)" }
    }

    # The interface locale the product persists (it must not follow the Windows input language).
    $settingsPath = Join-Path $env:LOCALAPPDATA 'Programming IME\settings.json'
    $settings = $null
    if (Test-Path -LiteralPath $settingsPath) {
        try { $settings = Get-Content -LiteralPath $settingsPath -Raw -Encoding UTF8 | ConvertFrom-Json } catch { $settings = $null }
    }
    $locale = $null
    if ($settings) {
        foreach ($name in @('uiLocale', 'ui_locale', 'locale', 'uiLanguage')) {
            if ($settings.PSObject.Properties.Name.Contains($name)) { $locale = "$($settings.$name)"; break }
        }
    }

    $Output = @{
        ok = $true
        user = "$env:USERDOMAIN\$env:USERNAME"
        session = (Get-Process -Id $PID).SessionId
        languages = $languages
        defaultOverride = $override
        lastProfile = $lastProfile
        settingsPath = $settingsPath
        settingsPresent = [bool](Test-Path -LiteralPath $settingsPath)
        uiLocale = $locale
    }
} catch {
    $Output = @{ ok = $false; error = $_.Exception.Message }
}
$Output | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
