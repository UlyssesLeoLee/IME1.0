# Common helpers of the Windows 11 end-to-end runner: configuration, logging and the result model.
# Sourced by Invoke-E2E.ps1; not a standalone script.

function Write-E2ELog {
    param([Parameter(Mandatory = $true)][string]$Message, [string]$Level = 'INFO')
    Write-Host ("[{0}] {1,-5} {2}" -f (Get-Date).ToString('HH:mm:ss'), $Level, $Message)
}

function Get-E2EConfig {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { throw "vm configuration not found: $Path" }
    $config = Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
    foreach ($key in @('transport', 'host', 'username', 'interactiveUser', 'workDirectory', 'installer')) {
        if (-not $config.PSObject.Properties.Name.Contains($key) -or -not $config.$key) {
            throw "vm configuration '$Path' is missing the required key '$key'"
        }
    }
    if (@('winrm', 'ssh') -notcontains $config.transport) { throw "transport must be 'winrm' or 'ssh', found '$($config.transport)'" }
    if (-not $config.installer -or -not (Test-Path -LiteralPath $config.installer)) {
        throw "the installer '$($config.installer)' does not exist on this machine (build it with 'node scripts/build-installer.js')"
    }
    if ($config.transport -eq 'winrm') {
        $envName = if ($config.PSObject.Properties.Name.Contains('passwordEnvironmentVariable')) { $config.passwordEnvironmentVariable } else { 'PIME_E2E_PASSWORD' }
        $password = [Environment]::GetEnvironmentVariable($envName)
        if (-not $password) { throw "the environment variable '$envName' is empty (it holds the password of '$($config.username)')" }
        $config | Add-Member -NotePropertyName 'password' -NotePropertyValue $password -Force
    }
    if (-not $config.PSObject.Properties.Name.Contains('timeouts')) {
        $config | Add-Member -NotePropertyName 'timeouts' -NotePropertyValue ([pscustomobject]@{commandSeconds = 300; interactiveSeconds = 180}) -Force
    }
    if (-not $config.PSObject.Properties.Name.Contains('restart')) {
        $config | Add-Member -NotePropertyName 'restart' -NotePropertyValue ([pscustomobject]@{enabled = $true; timeoutSeconds = 300}) -Force
    }
    return $config
}

function Get-E2EDesignTitle {
    # The title of a design case, read from the design document (the scripts themselves stay ASCII, so that they
    # run on every code page: the em dash of the headings is written as Escape sequence).
    param([Parameter(Mandatory = $true)][string]$Id, [string]$DesignPath)
    if (-not $DesignPath -or -not (Test-Path -LiteralPath $DesignPath)) { return $null }
    $text = Get-Content -LiteralPath $DesignPath -Raw -Encoding UTF8
    $match = [regex]::Match($text, "(?m)^###\s+$Id\s+\u2014\s+(.+?)\s*$")
    if ($match.Success) { return $match.Groups[1].Value }
    return $null
}

function New-E2ECaseResult {
    param([Parameter(Mandatory = $true)][string]$Id, [Parameter(Mandatory = $true)][string]$Title)
    return [ordered]@{
        id = $Id
        title = $Title
        status = 'NOT RUN'
        startedAt = (Get-Date).ToString('o')
        finishedAt = $null
        checks = @()
        notes = @()
    }
}

function Add-E2ECheck {
    param(
        [Parameter(Mandatory = $true)]$Result,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][ValidateSet('Pass', 'Fail', 'NOT RUN')][string]$Status,
        [string]$Expected = '',
        [string]$Actual = '',
        [string[]]$Evidence = @()
    )
    $Result.checks += [ordered]@{
        name = $Name
        status = $Status
        expected = $Expected
        actual = $Actual
        evidence = $Evidence
    }
    $mark = @{ 'Pass' = 'ok  '; 'Fail' = 'FAIL'; 'NOT RUN' = 'skip' }[$Status]
    Write-E2ELog ("  {0} {1}" -f $mark, $Name) -Level $(if ($Status -eq 'Fail') { 'ERROR' } else { 'INFO' })
    if ($Status -ne 'Pass') {
        if ($Expected) { Write-E2ELog ("       expected: {0}" -f $Expected) -Level 'INFO' }
        if ($Actual) { Write-E2ELog ("       actual  : {0}" -f $Actual) -Level 'INFO' }
    }
}

function Complete-E2ECase {
    param([Parameter(Mandatory = $true)]$Result)
    $Result.finishedAt = (Get-Date).ToString('o')
    if (@($Result.checks | Where-Object { $_.status -eq 'Fail' }).Count -gt 0) { $Result.status = 'Fail' }
    elseif (@($Result.checks | Where-Object { $_.status -eq 'NOT RUN' }).Count -gt 0) { $Result.status = 'NOT RUN' }
    elseif ($Result.checks.Count -eq 0) { $Result.status = 'NOT RUN' }
    else { $Result.status = 'Pass' }
    Write-E2ELog ("{0}: {1} ({2} checks)" -f $Result.id, $Result.status, $Result.checks.Count) -Level $(if ($Result.status -eq 'Pass') { 'INFO' } else { 'ERROR' })
    return $Result
}

function New-E2EReport {
    param([Parameter(Mandatory = $true)][object[]]$Cases, [string]$Note = '')
    $summary = [ordered]@{
        generated = (Get-Date).ToString('o')
        note = $Note
        total = $Cases.Count
        pass = @($Cases | Where-Object { $_.status -eq 'Pass' }).Count
        fail = @($Cases | Where-Object { $_.status -eq 'Fail' }).Count
        notRun = @($Cases | Where-Object { $_.status -eq 'NOT RUN' }).Count
        cases = $Cases
    }
    return $summary
}

function Write-E2EReport {
    param([Parameter(Mandatory = $true)]$Report, [Parameter(Mandatory = $true)][string]$Directory)
    if (-not (Test-Path -LiteralPath $Directory)) { New-Item -ItemType Directory -Force -Path $Directory | Out-Null }
    $json = Join-Path $Directory 'e2e-results.json'
    Set-Content -LiteralPath $json -Value ($Report | ConvertTo-Json -Depth 12) -Encoding UTF8
    $md = @()
    $md += "# Windows 11 end-to-end results (generated by 01_Source/tests/e2e/Invoke-E2E.ps1)"
    $md += ""
    $md += "Generated $($Report.generated). PASS means every mechanical check of the case ran and passed; NOT RUN means the"
    $md += "check could not be executed (with the reason); FAIL means it ran and the design criterion was not met."
    $md += ""
    if ($Report.note) { $md += "> $($Report.note)"; $md += "" }
    $md += "| Case | Title | Result | Checks |"
    $md += "|---|---|---|---|"
    foreach ($case in $Report.cases) {
        $md += "| $($case.id) | $($case.title) | $($case.status) | $($case.checks.Count) |"
    }
    $md += ""
    foreach ($case in $Report.cases) {
        $md += "## $($case.id) - $($case.title)"
        $md += ""
        $md += "| Check | Result | Expected | Actual |"
        $md += "|---|---|---|---|"
        foreach ($check in $case.checks) {
            $md += "| $($check.name) | $($check.status) | $($check.expected -replace '\|', '/') | $($check.actual -replace '\|', '/') |"
        }
        $md += ""
        foreach ($note in $case.notes) { $md += "- $note" }
        if ($case.notes.Count) { $md += "" }
    }
    Set-Content -LiteralPath (Join-Path $Directory 'e2e-results.md') -Value ($md -join "`r`n") -Encoding UTF8
    return $json
}
