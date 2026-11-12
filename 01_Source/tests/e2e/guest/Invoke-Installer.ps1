# Guest side: runs the installer (or the uninstaller) with the given arguments.
# Runs elevated (S4U): the installer writes HKLM and Program Files. An already elevated process does not ask for
# permission again, so the run is unattended.
#   powershell -NoProfile -File Invoke-Installer.ps1 -ParametersPath <file> -ResultPath <file>
# The parameters file holds: installerPath, arguments (array), logPath, timeoutSeconds.
param(
    [Parameter(Mandatory = $true)][string]$ParametersPath,
    [Parameter(Mandatory = $true)][string]$ResultPath
)
$ErrorActionPreference = 'Stop'
$Parameters = Get-Content -LiteralPath $ParametersPath -Raw -Encoding UTF8 | ConvertFrom-Json
$Output = $null
try {
    if (-not (Test-Path -LiteralPath $Parameters.installerPath)) {
        throw "the installer '$($Parameters.installerPath)' does not exist in the guest"
    }
    $arguments = @($Parameters.arguments)
    $stdout = Join-Path $env:TEMP ('pime-install-' + [Guid]::NewGuid().ToString('N').Substring(0, 8) + '.out.txt')
    $process = Start-Process -FilePath $Parameters.installerPath -ArgumentList $arguments -PassThru -NoNewWindow `
        -RedirectStandardOutput $stdout -RedirectStandardError "$stdout.err"
    if (-not $process.WaitForExit([int]$Parameters.timeoutSeconds * 1000)) {
        try { $process.Kill() } catch { }
        throw "the installer did not finish within $($Parameters.timeoutSeconds) s"
    }
    $log = ''
    if ($Parameters.logPath -and (Test-Path -LiteralPath $Parameters.logPath)) {
        $log = (Get-Content -LiteralPath $Parameters.logPath -Raw -Encoding UTF8)
    }
    $output = ''
    if (Test-Path -LiteralPath $stdout) { $output = Get-Content -LiteralPath $stdout -Raw -Encoding UTF8 }
    $errorText = ''
    if (Test-Path -LiteralPath "$stdout.err") { $errorText = Get-Content -LiteralPath "$stdout.err" -Raw -Encoding UTF8 }
    $Output = @{
        ok = $true
        exitCode = $process.ExitCode
        stdout = $output
        stderr = $errorText
        log = $log
    }
} catch {
    $Output = @{ ok = $false; error = $_.Exception.Message }
}
$Output | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
