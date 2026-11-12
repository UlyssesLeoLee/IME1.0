# Guest side: the machine snapshot that the "only our own components changed" check compares.
# Runs elevated (S4U) as the interactive user, so that HKCU is the hive of that user.
#   powershell -NoProfile -File Invoke-Snapshot.ps1 -ParametersPath <file> -ResultPath <file>
# The parameters file holds: installDir, productGuids (the GUIDs that belong to the product).
param(
    [Parameter(Mandatory = $true)][string]$ParametersPath,
    [Parameter(Mandatory = $true)][string]$ResultPath
)
$ErrorActionPreference = 'Stop'
$Parameters = Get-Content -LiteralPath $ParametersPath -Raw -Encoding UTF8 | ConvertFrom-Json
$Output = $null
try {
    $work = Join-Path $env:TEMP ('pime-snapshot-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    $exports = [ordered]@{}
    $keys = [ordered]@{
        'ctf-tip' = 'HKLM\SOFTWARE\Microsoft\CTF\TIP'
        'user-profile' = 'HKCU\Control Panel\International\User Profile'
        'uninstall' = 'HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall'
        'ctf-assemblies' = 'HKLM\SOFTWARE\Microsoft\CTF\Assemblies'
    }
    foreach ($name in $keys.Keys) {
        $file = Join-Path $work ($name + '.reg')
        & reg.exe export $keys[$name] $file /y 2>$null | Out-Null
        if (Test-Path -LiteralPath $file) {
            $exports[$name] = (Get-Content -LiteralPath $file -Raw -Encoding Unicode)
        } else {
            $exports[$name] = ''
        }
    }
    $installListing = ''
    if ($Parameters.installDir -and (Test-Path -LiteralPath $Parameters.installDir)) {
        $installListing = (Get-ChildItem -LiteralPath $Parameters.installDir -Recurse -File -ErrorAction SilentlyContinue |
            ForEach-Object { "$($_.FullName)|$($_.Length)" }) -join "`n"
    }
    $programs = @()
    foreach ($key in (Get-ChildItem 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall' -ErrorAction SilentlyContinue)) {
        $item = Get-ItemProperty -LiteralPath $key.PSPath -ErrorAction SilentlyContinue
        if ($item.DisplayName) { $programs += "$($item.DisplayName)|$($item.DisplayVersion)" }
    }
    $Output = @{ ok = $true; exports = $exports; installListing = $installListing; programs = $programs }
} catch {
    $Output = @{ ok = $false; error = $_.Exception.Message }
} finally {
    if ($work -and (Test-Path -LiteralPath $work)) { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
}
$Output | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
