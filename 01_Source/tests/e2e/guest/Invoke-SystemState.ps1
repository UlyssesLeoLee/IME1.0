# Guest side: the machine level state of the product (registration, files, signature, packs).
# Runs elevated (S4U), because it reads HKLM and Program Files.
#   powershell -NoProfile -File Invoke-SystemState.ps1 -ParametersPath <file> -ResultPath <file>
# The parameters file holds: clsid, profiles (zh/en/ja profile GUIDs), installerPath, installDir, packs (7 names).
param(
    [Parameter(Mandatory = $true)][string]$ParametersPath,
    [Parameter(Mandatory = $true)][string]$ResultPath
)
$ErrorActionPreference = 'Stop'
$Parameters = Get-Content -LiteralPath $ParametersPath -Raw -Encoding UTF8 | ConvertFrom-Json
$Output = $null
try {
    $os = Get-CimInstance -ClassName Win32_OperatingSystem

    # The TIP registration of the product: one text service, three language profiles.
    $tipKey = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$($Parameters.clsid)"
    $languageProfiles = @{}
    $profileKey = Join-Path $tipKey 'LanguageProfile'
    if (Test-Path -LiteralPath $profileKey) {
        foreach ($child in (Get-ChildItem -LiteralPath $profileKey)) {
            $values = @((Get-ItemProperty -LiteralPath $child.PSPath).PSObject.Properties |
                Where-Object { $_.Name -notmatch '^PS' } | ForEach-Object { "$($_.Name)" })
            $languageProfiles[$child.PSChildName] = $values
        }
    }
    $tipCategories = @()
    $categoryKey = Join-Path $tipKey 'Category'
    if (Test-Path -LiteralPath $categoryKey) {
        $tipCategories = @((Get-Item -LiteralPath $categoryKey).GetValueNames())
    }

    # The COM class of the text service: the DLL that every profile runs in.
    $clsidKey = "HKLM:\SOFTWARE\Classes\CLSID\$($Parameters.clsid)"
    $dllPath = $null
    $threadingModel = $null
    $className = $null
    if (Test-Path -LiteralPath $clsidKey) {
        $className = (Get-Item -LiteralPath $clsidKey).GetValue('')
        $inproc = Join-Path $clsidKey 'InprocServer32'
        if (Test-Path -LiteralPath $inproc) {
            $dllPath = (Get-Item -LiteralPath $inproc).GetValue('')
            $threadingModel = (Get-Item -LiteralPath $inproc).GetValue('ThreadingModel')
        }
    }

    # The installed files.
    $files = @()
    if ($Parameters.installDir -and (Test-Path -LiteralPath $Parameters.installDir)) {
        foreach ($file in (Get-ChildItem -LiteralPath $Parameters.installDir -File -Recurse -ErrorAction SilentlyContinue)) {
            $files += [ordered]@{
                path = $file.FullName
                size = $file.Length
                sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
            }
        }
    }

    # The seven language packs are compiled into the binary: check the installed file for their names.
    $packsFound = @()
    $packBinary = $null
    foreach ($candidate in @($dllPath, (Join-Path $Parameters.installDir 'pime-core.exe'))) {
        if (-not $candidate -or -not (Test-Path -LiteralPath $candidate)) { continue }
        $bytes = [System.IO.File]::ReadAllBytes($candidate)
        $ascii = [Text.Encoding]::ASCII.GetString($bytes)
        $hit = @()
        foreach ($pack in $Parameters.packs) { if ($ascii.Contains($pack)) { $hit += $pack } }
        if ($hit.Count -gt $packsFound.Count) { $packsFound = $hit; $packBinary = $candidate }
    }

    # The signature of the installer (ST-035 asks for a signed one).
    $signature = $null
    if ($Parameters.installerPath -and (Test-Path -LiteralPath $Parameters.installerPath)) {
        $sig = Get-AuthenticodeSignature -LiteralPath $Parameters.installerPath
        $signature = [ordered]@{
            status = "$($sig.Status)"
            subject = if ($sig.SignerCertificate) { $sig.SignerCertificate.Subject } else { $null }
            issuer = if ($sig.SignerCertificate) { $sig.SignerCertificate.Issuer } else { $null }
            sha256 = (Get-FileHash -LiteralPath $Parameters.installerPath -Algorithm SHA256).Hash
        }
    }

    # The entry in "Apps & features".
    $arpKey = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\ProgrammingIME'
    $arp = $null
    if (Test-Path -LiteralPath $arpKey) {
        $item = Get-ItemProperty -LiteralPath $arpKey
        $arp = [ordered]@{ displayName = $item.DisplayName; displayVersion = $item.DisplayVersion; installLocation = $item.InstallLocation }
    }

    $Output = @{
        ok = $true
        os = [ordered]@{ caption = $os.Caption; version = $os.Version; build = $os.BuildNumber; architecture = $os.OSArchitecture }
        tipRegistered = (Test-Path -LiteralPath $tipKey)
        languageProfiles = $languageProfiles
        tipCategories = $tipCategories
        clsidRegistered = (Test-Path -LiteralPath $clsidKey)
        className = $className
        dllPath = $dllPath
        threadingModel = $threadingModel
        files = $files
        packsFound = $packsFound
        packBinary = $packBinary
        signature = $signature
        arp = $arp
        aiWorkerRunning = @((Get-Process -Name 'pime-ai-worker' -ErrorAction SilentlyContinue)).Count
    }
} catch {
    $Output = @{ ok = $false; error = $_.Exception.Message }
}
$Output | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
