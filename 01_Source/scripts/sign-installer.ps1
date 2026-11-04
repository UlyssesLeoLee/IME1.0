<#
.SYNOPSIS
    Signs the IME1.0 installer with an Authenticode code signing certificate.

.DESCRIPTION
    The installer (01_Source\dist\ProgrammingIME-<version>-win-x64-setup.exe) is built unsigned by
    scripts\build-installer.js, so Windows SmartScreen reports "Unknown publisher" and an enterprise
    policy may block it. This script signs the installer in place, verifies the signature and then
    re-runs the installer's own --dry-run.

    The dry run after signing is the point of the script: the payload lives in a PE overlay and the
    Authenticode certificate table is appended AFTER that overlay, so the loader has to find the
    payload through the PE security directory again. A signed installer whose payload can no longer be
    located would look fine to a user (the signature is valid) and be broken on every machine. The
    dry run proves that the signed file still finds and verifies its own payload.

    The certificate is read from a local drop folder (scripts\certs) that must never be committed.
    The script needs the Windows SDK (signtool.exe) and network access to the timestamp server; it
    does not need to be online for anything else.

.PARAMETER Installer
    The installer to sign. Default: the newest 01_Source\dist\ProgrammingIME-*-win-x64-setup.exe.

.PARAMETER CertFolder
    Folder that holds the certificate. Default: the scripts\certs folder next to this script.

.PARAMETER CertFile
    Name of the .pfx inside CertFolder. Default: cert.pfx

.PARAMETER PasswordFile
    File that holds the password. Default: cert.password.txt inside CertFolder.

.PARAMETER Password
    The password itself. Overrides the password file.

.PARAMETER SignTool
    Full path to signtool.exe. Default: looked up in PATH, in the Windows SDK environment variables
    and in the registry. Nothing is guessed: if it is not found the script says where it looked.

.PARAMETER TimestampUrl
    RFC 3161 timestamp server. Default: http://timestamp.digicert.com
    Pass an empty string to sign without a timestamp (the signature then expires with the
    certificate).

.PARAMETER VerifyOnly
    Do not sign. Only verify an existing signature and re-run the dry run.

.PARAMETER Help
    Print the usage and exit.

.EXAMPLE
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File 01_Source\scripts\sign-installer.ps1

.EXAMPLE
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File 01_Source\scripts\sign-installer.ps1 -VerifyOnly
#>
[CmdletBinding()]
param(
    [string] $Installer,
    [string] $CertFolder,
    [string] $CertFile = 'cert.pfx',
    [string] $PasswordFile,
    [string] $Password,
    [string] $SignTool,
    [string] $TimestampUrl = 'http://timestamp.digicert.com',
    [switch] $VerifyOnly,
    [switch] $Help
)

$ErrorActionPreference = 'Stop'

# Exit codes (documented in 01_Source\docs\SIGNING.md).
$EXIT_OK = 0
$EXIT_USAGE = 2
$EXIT_NO_CERT = 3
$EXIT_BAD_PASSWORD = 4
$EXIT_NO_SIGNTOOL = 5
$EXIT_SIGN_FAILED = 6
$EXIT_VERIFY_FAILED = 7
$EXIT_DRYRUN_FAILED = 8
$EXIT_NO_PAYLOAD = 9

function Write-Step {
    param([string] $Message)
    Write-Host "==> $Message"
}

function Stop-WithError {
    param([int] $Code, [string] $Message, [string[]] $Details = @())
    Write-Host ''
    Write-Host "sign-installer FAILED: $Message" -ForegroundColor Red
    foreach ($block in $Details) {
        if (-not $block) { continue }
        foreach ($line in ($block -split "`r?`n")) {
            if ($line.Trim().Length -gt 0) { Write-Host "  $line" }
        }
    }
    exit $Code
}

function Write-Usage {
    Write-Host @'
sign-installer.ps1 - signs the IME1.0 installer and proves that it still works.

Usage:
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File 01_Source\scripts\sign-installer.ps1 [options]

Options:
  -Installer <path>     The installer to sign.
                        Default: the newest 01_Source\dist\ProgrammingIME-*-win-x64-setup.exe
  -CertFolder <path>    Folder that holds the certificate.
                        Default: 01_Source\scripts\certs
  -CertFile <name>      Name of the .pfx inside CertFolder. Default: cert.pfx
  -PasswordFile <path>  File that holds the password.
                        Default: cert.password.txt inside CertFolder
  -Password <text>      The password itself. Overrides the password file.
  -SignTool <path>      Full path to signtool.exe. Default: found in PATH, in the Windows SDK
                        environment variables and in the registry.
  -TimestampUrl <url>   RFC 3161 timestamp server. Default: http://timestamp.digicert.com
                        (an empty string signs without a timestamp)
  -VerifyOnly           Do not sign: verify an existing signature and re-run the dry run.
  -Help                 Print this help and exit.

Certificate drop folder (never commit it, see 01_Source\scripts\certs\README.md):
  cert.pfx              the code signing certificate (PKCS#12)
  cert.password.txt     the password of the .pfx (first non-empty line)

Exit codes:
  0  success
  2  usage or configuration error (bad parameter, missing password file)
  3  no certificate found in the certificate folder
  4  the certificate could not be opened (wrong password)
  5  signtool.exe not found
  6  signing failed
  7  the signature did not verify
  8  the dry run after signing failed
  9  the installer carries no payload
'@
}

# Runs a native program and returns its exit code and its output. The error action preference is
# lowered for the call: in Windows PowerShell a native program that writes to stderr would
# otherwise raise a terminating NativeCommandError and hide the real exit code.
function Invoke-Native {
    param([string] $Exe, [string[]] $Arguments)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $output = & $Exe @Arguments 2>&1
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
    $text = ''
    foreach ($line in @($output)) { $text = $text + ([string] $line) + "`n" }
    return [pscustomobject]@{ ExitCode = $code; Output = $text }
}

function Get-SignToolIn {
    param([string] $Root)
    $found = New-Object System.Collections.Generic.List[string]
    if ([string]::IsNullOrWhiteSpace($Root)) { return $found }
    if (-not (Test-Path -LiteralPath $Root -PathType Container)) { return $found }
    # A root can be the kit root (... \Windows Kits\10), the bin folder (... \bin) or the version
    # folder (... \bin\10.0.22621.0). All three are searched, so the layout of the SDK does not
    # have to be known here.
    $bases = @()
    $bases += (Join-Path $Root 'bin')
    $bases += $Root
    foreach ($base in $bases) {
        if (-not (Test-Path -LiteralPath $base -PathType Container)) { continue }
        $dirs = @()
        $dirs += (Get-ChildItem -LiteralPath $base -Directory -ErrorAction SilentlyContinue)
        foreach ($dir in $dirs) {
            foreach ($arch in @('x64', 'x86', 'arm64')) {
                $p = Join-Path $dir.FullName (Join-Path $arch 'signtool.exe')
                if (Test-Path -LiteralPath $p -PathType Leaf) { $found.Add((Resolve-Path -LiteralPath $p).Path) }
            }
            $p = Join-Path $dir.FullName 'signtool.exe'
            if (Test-Path -LiteralPath $p -PathType Leaf) { $found.Add((Resolve-Path -LiteralPath $p).Path) }
        }
        foreach ($arch in @('x64', 'x86', 'arm64')) {
            $p = Join-Path $base (Join-Path $arch 'signtool.exe')
            if (Test-Path -LiteralPath $p -PathType Leaf) { $found.Add((Resolve-Path -LiteralPath $p).Path) }
        }
        $p = Join-Path $base 'signtool.exe'
        if (Test-Path -LiteralPath $p -PathType Leaf) { $found.Add((Resolve-Path -LiteralPath $p).Path) }
    }
    return $found
}

# Finds signtool.exe. Never guesses: if nothing is found the error lists every place that was
# searched.
function Resolve-SignTool {
    param([string] $Explicit)
    $searched = New-Object System.Collections.Generic.List[string]
    if (-not [string]::IsNullOrWhiteSpace($Explicit)) {
        if (-not (Test-Path -LiteralPath $Explicit -PathType Leaf)) {
            Stop-WithError $EXIT_NO_SIGNTOOL "the signtool given with -SignTool does not exist: $Explicit" @(
                "Give the full path of signtool.exe, for example the one of the Windows SDK:",
                '  C:\Program Files (x86)\Windows Kits\10\bin\<version>\x64\signtool.exe'
            )
        }
        return (Resolve-Path -LiteralPath $Explicit).Path
    }

    $candidates = New-Object System.Collections.Generic.List[string]
    $command = Get-Command 'signtool.exe' -CommandType Application -ErrorAction SilentlyContinue
    if ($command) { $candidates.Add($command.Source) }

    foreach ($root in @($env:WindowsSdkVerBinPath, $env:WindowsSdkBinPath, $env:WindowsSdkDir)) {
        if ([string]::IsNullOrWhiteSpace($root)) { continue }
        $searched.Add("environment variable: $root")
        foreach ($p in (Get-SignToolIn -Root $root)) { $candidates.Add($p) }
    }

    foreach ($key in @('HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots',
                       'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots')) {
        $searched.Add("registry key: $key")
        $props = Get-ItemProperty -Path $key -ErrorAction SilentlyContinue
        if (-not $props) { continue }
        foreach ($name in @('KitsRoot10', 'KitsRoot11', 'KitsRoot81')) {
            $value = $null
            if ($props.PSObject.Properties.Name -contains $name) { $value = $props.$name }
            if ([string]::IsNullOrWhiteSpace($value)) { continue }
            $searched.Add("registry value: $name = $value")
            foreach ($p in (Get-SignToolIn -Root $value)) { $candidates.Add($p) }
        }
    }

    $searched.Add('PATH')
    $unique = @($candidates | Select-Object -Unique)
    if ($unique.Count -eq 0) {
        $details = @(
            'Install the Windows SDK (the "Windows SDK" or "Debugging Tools for Windows" component of',
            'the Windows SDK installer; it ships signtool.exe), or point at it with',
            '  -SignTool "C:\Program Files (x86)\Windows Kits\10\bin\<version>\x64\signtool.exe"',
            'These places were searched:'
        )
        foreach ($place in $searched) { $details += "  $place" }
        Stop-WithError $EXIT_NO_SIGNTOOL 'signtool.exe was not found' $details
    }

    # Prefer x64 and the newest SDK version.
    $ranked = $unique | ForEach-Object {
        $arch = 2
        if ($_ -match '\\x64\\') { $arch = 0 } elseif ($_ -match '\\x86\\') { $arch = 1 }
        $version = [version]'0.0'
        if ($_ -match '\\([0-9]+\.[0-9]+\.[0-9]+\.[0-9]+)\\') { $version = [version] $Matches[1] }
        [pscustomobject]@{ Path = $_; Arch = $arch; Version = $version }
    }
    $best = $ranked | Sort-Object -Property @{ Expression = 'Arch'; Ascending = $true },
                                          @{ Expression = 'Version'; Descending = $true } |
              Select-Object -First 1
    return $best.Path
}

# The certificate and the password come from the drop folder, never from the command line history
# if that can be avoided. The password is still handed to signtool on its command line, which is a
# limitation of signtool itself (documented in docs\SIGNING.md).
function Resolve-Certificate {
    param([string] $Folder, [string] $Name)
    $pfx = Join-Path $Folder $Name
    if (-not (Test-Path -LiteralPath $pfx -PathType Leaf)) {
        Stop-WithError $EXIT_NO_CERT "no certificate found in $Folder" @(
            "Expected the file '$Name' in that folder.",
            'Put your code signing certificate there:',
            "  $Folder\$Name",
            'and its password in:',
            "  $Folder\cert.password.txt",
            'or pass -CertFile / -CertFolder / -Password on the command line.',
            'That folder must never be committed: see 01_Source\scripts\certs\README.md.'
        )
    }
    return (Resolve-Path -LiteralPath $pfx).Path
}

function Resolve-Password {
    param([string] $Folder, [string] $File, [string] $Given)
    if (-not [string]::IsNullOrEmpty($Given)) { return $Given }
    $path = $File
    if ([string]::IsNullOrWhiteSpace($path)) { $path = Join-Path $Folder 'cert.password.txt' }
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        Stop-WithError $EXIT_USAGE "no password: the file $path does not exist" @(
            'Create it (first line = the password of the .pfx) or pass -Password.',
            'Do not commit it: see 01_Source\scripts\certs\README.md.'
        )
    }
    $line = Get-Content -LiteralPath $path | Where-Object { $_.Trim().Length -gt 0 } | Select-Object -First 1
    if ($null -eq $line) {
        Stop-WithError $EXIT_USAGE "the password file is empty: $path" @(
            'Write the password of the .pfx in its first line.'
        )
    }
    return $line.Trim()
}

# Runs the signed installer with --dry-run and a prefix folder that does not exist. The dry run must
# not create anything; the folder is only used to make the plan concrete.
function Test-DryRun {
    param([string] $Exe)
    $temp = [System.IO.Path]::GetTempPath()
    if ([string]::IsNullOrWhiteSpace($temp)) { $temp = $env:TEMP }
    $probe = Join-Path $temp ('pime-sign-dryrun-' + [guid]::NewGuid().ToString('N'))
    Write-Step "dry run: $Exe --dry-run --prefix $probe"
    $run = Invoke-Native -Exe $Exe -Arguments @('--dry-run', '--prefix', $probe)
    $text = $run.Output
    if ($text -like '*carries no payload*') {
        Stop-WithError $EXIT_NO_PAYLOAD 'the installer carries no payload' @(
            'The loader did not find the payload overlay in the file.',
            'If the file was just signed, this is the failure mode the dry run is here to catch:',
            'the certificate table was appended after the overlay and the loader could not locate',
            'the payload any more. Do not distribute this file; rebuild it and report the result.',
            $text.Trim()
        )
    }
    if ($run.ExitCode -ne 0) {
        Stop-WithError $EXIT_DRYRUN_FAILED "the dry run of the installer failed (exit $($run.ExitCode))" @(
            $text.Trim()
        )
    }
    if ($text -notlike '*install plan (dry run: nothing is changed)*') {
        Stop-WithError $EXIT_DRYRUN_FAILED 'the dry run did not print an install plan' @($text.Trim())
    }
    if ($text -notlike '*SHA-256 of every file verified*') {
        Stop-WithError $EXIT_DRYRUN_FAILED 'the dry run did not verify the payload of the installer' @(
            'The plan says that the payload was not verified, so the file must not be distributed.',
            $text.Trim()
        )
    }
    if (Test-Path -LiteralPath $probe) {
        $created = @(Get-ChildItem -LiteralPath $probe -Force -ErrorAction SilentlyContinue)
        if ($created.Count -eq 0) {
            Remove-Item -LiteralPath $probe -Force -ErrorAction SilentlyContinue
            Stop-WithError $EXIT_DRYRUN_FAILED 'the dry run created a folder (it must change nothing)' @(
                "The empty folder $probe was created and removed again."
            )
        }
        Stop-WithError $EXIT_DRYRUN_FAILED 'the dry run created files (it must change nothing)' @(
            "The folder $probe was created and still holds $($created.Count) item(s).",
            'It was left in place so that it can be inspected; delete it by hand.'
        )
    }
    Write-Step 'dry run: the installer still finds and verifies its own payload'
}

# --- main -------------------------------------------------------------------------------------

if ($Help) {
    Write-Usage
    exit $EXIT_OK
}

$SourceRoot = Split-Path -Parent $PSScriptRoot
$DistFolder = Join-Path $SourceRoot 'dist'
if ([string]::IsNullOrWhiteSpace($CertFolder)) { $CertFolder = Join-Path $PSScriptRoot 'certs' }
if (-not (Test-Path -LiteralPath $CertFolder -PathType Container)) {
    Stop-WithError $EXIT_USAGE "the certificate folder does not exist: $CertFolder" @(
        'Create it (01_Source\scripts\certs) or pass -CertFolder.'
    )
}
$CertFolder = (Resolve-Path -LiteralPath $CertFolder).Path

if ([string]::IsNullOrWhiteSpace($Installer)) {
    $found = @(Get-ChildItem -LiteralPath $DistFolder -Filter 'ProgrammingIME-*-win-x64-setup.exe' -File -ErrorAction SilentlyContinue |
               Sort-Object -Property LastWriteTime -Descending)
    if ($found.Count -eq 0) {
        Stop-WithError $EXIT_USAGE "no installer found in $DistFolder" @(
            'Build one first:  node scripts\build-installer.js   (see 01_Source\docs\INSTALLER.md)',
            'or pass the file with -Installer.'
        )
    }
    $Installer = $found[0].FullName
    Write-Step "installer: $Installer (the newest one in $DistFolder)"
} else {
    if (-not (Test-Path -LiteralPath $Installer -PathType Leaf)) {
        Stop-WithError $EXIT_USAGE "the installer does not exist: $Installer"
    }
    $Installer = (Resolve-Path -LiteralPath $Installer).Path
    Write-Step "installer: $Installer"
}

if (-not $VerifyOnly) {
    # The certificate is looked for before signtool: a missing certificate is the more likely
    # reason for a failure and the message says exactly which file is missing.
    $PfxPath = Resolve-Certificate -Folder $CertFolder -Name $CertFile
    $Secret = Resolve-Password -Folder $CertFolder -File $PasswordFile -Given $Password
    Write-Step "certificate: $PfxPath"
    Write-Step "timestamp: $(if ($TimestampUrl) { $TimestampUrl } else { '(none)' })"
}

$SignToolPath = Resolve-SignTool -Explicit $SignTool
Write-Step "signtool: $SignToolPath"

if (-not $VerifyOnly) {

    $signArguments = @('sign', '/fd', 'sha256')
    if (-not [string]::IsNullOrWhiteSpace($TimestampUrl)) {
        $signArguments += @('/tr', $TimestampUrl, '/td', 'sha256')
    }
    $signArguments += @('/f', $PfxPath, '/p', $Secret, '/v', $Installer)

    Write-Step 'signing ...'
    $signed = Invoke-Native -Exe $SignToolPath -Arguments $signArguments
    if ($signed.ExitCode -ne 0) {
        if ($signed.Output -match '(?i)password') {
            Stop-WithError $EXIT_BAD_PASSWORD 'signtool could not open the certificate (wrong password?)' @(
                $signed.Output.Trim()
            )
        }
        Stop-WithError $EXIT_SIGN_FAILED "signing failed (signtool exit $($signed.ExitCode))" @(
            $signed.Output.Trim()
        )
    }
    Write-Step 'signed'
}

Write-Step 'verifying the signature (signtool verify /pa /v) ...'
$verified = Invoke-Native -Exe $SignToolPath -Arguments @('verify', '/pa', '/v', $Installer)
if ($verified.ExitCode -ne 0) {
    Stop-WithError $EXIT_VERIFY_FAILED "the signature did not verify (signtool exit $($verified.ExitCode))" @(
        'A self signed test certificate has to be trusted in the store of the local machine',
        '(Trusted Root Certification Authorities) before /pa accepts it.',
        $verified.Output.Trim()
    )
}
Write-Step 'the signature is valid'

Test-DryRun -Exe $Installer

$digest = (Get-FileHash -LiteralPath $Installer -Algorithm SHA256).Hash.ToLowerInvariant()
$sidecar = "$Installer.sha256"

if ($VerifyOnly) {
    Write-Host ''
    Write-Step "OK: the signature is valid and the installer still finds its payload (nothing was signed)"
    Write-Host "  installer  $Installer"
    Write-Host "  sha256    $digest"
    if (Test-Path -LiteralPath $sidecar -PathType Leaf) {
        $recorded = (Get-Content -LiteralPath $sidecar -Raw -ErrorAction SilentlyContinue)
        if ($recorded -and $recorded -match '([0-9a-fA-F]{64})') {
            if ($Matches[1].ToLowerInvariant() -ne $digest) {
                Write-Host "  WARNING: $sidecar holds an older digest; run the script without -VerifyOnly to rewrite it." -ForegroundColor Yellow
            } else {
                Write-Host "  sidecar   $sidecar (matches)"
            }
        }
    } else {
        Write-Host "  WARNING: $sidecar does not exist; run the script without -VerifyOnly to write it." -ForegroundColor Yellow
    }
    exit $EXIT_OK
}

# The digest changes when the file is signed, so the sidecar has to be written again. It is written
# only after every check passed, so that the sidecar always describes a file that was signed,
# verified and dry run successfully.
$line = "$digest *$([System.IO.Path]::GetFileName($Installer))`n"
[System.IO.File]::WriteAllText($sidecar, $line, (New-Object System.Text.UTF8Encoding($false)))

Write-Host ''
Write-Step 'OK: the installer is signed, the signature verifies and the payload is still found'
Write-Host "  installer  $Installer"
Write-Host "  sidecar   $sidecar"
Write-Host "  sha256    $digest"
Write-Output $digest
exit $EXIT_OK
