# Guest side: types a text into a real Notepad window and saves it.
# Runs in the interactive session of the logged-on user (a WinRM/ssh session has no desktop).
#   powershell -NoProfile -File Invoke-NotepadTyping.ps1 -ParametersPath <file> -ResultPath <file>
# The parameters file holds: path, textB64 (UTF-8), backspaces, keys (array of key sequences), settleSeconds.
param(
    [Parameter(Mandatory = $true)][string]$ParametersPath,
    [Parameter(Mandatory = $true)][string]$ResultPath
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'PimeKeys.ps1')
$Parameters = Get-Content -LiteralPath $ParametersPath -Raw -Encoding UTF8 | ConvertFrom-Json
$Output = $null
try {
    $text = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Parameters.textB64))
    $keys = @()
    if ($Parameters.PSObject.Properties.Name.Contains('keys')) { $keys = @($Parameters.keys) }
    $Output = Invoke-PimeNotepadTyping -Path $Parameters.path -Text $text -Keys $keys `
        -Backspaces ([int]$Parameters.backspaces) -SettleSeconds ([int]$Parameters.settleSeconds)
} catch {
    $Output = @{ ok = $false; error = $_.Exception.Message }
}
$Output | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
