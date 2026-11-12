# The Windows 11 end-to-end runner of the design cases that need a real machine.
#
#   powershell -ExecutionPolicy Bypass -File tests\e2e\Invoke-E2E.ps1 -Config tests\e2e\vmconfig.json
#   powershell -ExecutionPolicy Bypass -File tests\e2e\Invoke-E2E.ps1 -Config ... -Case ST-005
#   powershell -ExecutionPolicy Bypass -File tests\e2e\Invoke-E2E.ps1 -Config ... -PlanOnly
#
# It drives a disposable Windows 11 guest (Hyper-V, a virtual machine of any other hypervisor, or a physical test
# machine) over WinRM or ssh, runs the six design cases ST-001..ST-005 and ST-035 and writes the result into
# tests\e2e\results\ (never into 02_Verification: that folder belongs to the evidence of the delivery).
#
# Exit code: 0 every selected case passed, 1 at least one case failed, 2 the guest or the configuration is not
# usable, 3 no case failed but at least one check could not be executed.
param(
    [string]$Config = (Join-Path $PSScriptRoot 'vmconfig.json'),
    [string[]]$Case = @(),
    [switch]$PlanOnly,
    [switch]$NoRestart
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lib\Common.ps1')
. (Join-Path $PSScriptRoot 'lib\Guest.ps1')
. (Join-Path $PSScriptRoot 'lib\Cases.ps1')

$allCases = [ordered]@{
    'ST-001' = 'Invoke-E2ECaseST001'
    'ST-002' = 'Invoke-E2ECaseST002'
    'ST-003' = 'Invoke-E2ECaseST003'
    'ST-004' = 'Invoke-E2ECaseST004'
    'ST-005' = 'Invoke-E2ECaseST005'
    'ST-035' = 'Invoke-E2ECaseST035'
}
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

Write-E2ELog "the Windows 11 end-to-end runner of IME1.0"
Write-E2ELog "  repository : $repoRoot"
Write-E2ELog "  cases      : $(($allCases.Keys) -join ', ')"

if ($PlanOnly) {
    Write-E2ELog "PLAN ONLY: nothing is executed and no result is produced."
    foreach ($id in $allCases.Keys) {
        $script = Join-Path $PSScriptRoot ("cases\$id.ps1")
        Write-E2ELog ("  {0}  {1}" -f $id, (Test-Path -LiteralPath $script))
    }
    Write-E2ELog "  the guest scripts: $((Get-ChildItem (Join-Path $PSScriptRoot 'guest') -Filter '*.ps1').Name) -join ', ')"
    exit 0
}

try {
    $configuration = Get-E2EConfig -Path $Config
} catch {
    Write-E2ELog $_.Exception.Message -Level 'ERROR'
    Write-E2ELog "see tests\e2e\README.md and tests\e2e\vmconfig.example.json" -Level 'ERROR'
    exit 2
}
Write-E2ELog "  guest      : $($configuration.transport)://$($configuration.host) as $($configuration.username)"

$selected = if ($Case.Count) { $Case } else { @($allCases.Keys) }
foreach ($id in $selected) {
    if (-not $allCases.Contains($id)) {
        Write-E2ELog "unknown case '$id' (known: $($allCases.Keys -join ', '))" -Level 'ERROR'
        exit 2
    }
}
if ($NoRestart -and $configuration.restart.enabled) {
    $configuration.restart.enabled = $false
    Write-E2ELog "  the restart is switched off (--NoRestart)"
}

# The guest has to answer before anything else happens: without it the runner says so and stops.
$state = Test-E2EGuest -Config $configuration
if (-not $state.reachable) {
    Write-E2ELog "the guest does not answer: $($state.message)" -Level 'ERROR'
    Write-E2ELog "check the host, the port, the account and the password in $Config" -Level 'ERROR'
    exit 2
}
Write-E2ELog "  the guest answers: $($state.message)"

$context = @{
    Config = $configuration
    RepoRoot = $repoRoot
    InstallerInGuest = (Join-Path $configuration.workDirectory (Split-Path $configuration.installer -Leaf))
    OlderInstallerInGuest = (Join-Path $configuration.workDirectory 'older-installer.exe')
    InstallerCopied = $false
    Product = @{
        Clsid = '{43F95ED1-95B4-421D-8F5A-837128C2D854}'
        InstallDir = 'C:\Program Files\IME1.0'
        Packs = @('assembly', 'c', 'machine-code', 'powershell', 'python', 'rust', 'typescript')
        Profiles = @{
            'zh' = @{ languageTag = 'zh-Hans-CN'; langId = '0804'; guid = '{223FD919-F8CB-476C-A852-351E7381C801}' }
            'en' = @{ languageTag = 'en-US'; langId = '0409'; guid = '{68ED92DE-77CF-43B1-A7B2-D3BE2686E2A0}' }
            'ja' = @{ languageTag = 'ja-JP'; langId = '0411'; guid = '{22E54B52-5082-42D3-9E41-1C2E0CE74DFB}' }
        }
        Guids = @('{43F95ED1-95B4-421D-8F5A-837128C2D854}', '{223FD919-F8CB-476C-A852-351E7381C801}',
            '{68ED92DE-77CF-43B1-A7B2-D3BE2686E2A0}', '{22E54B52-5082-42D3-9E41-1C2E0CE74DFB}')
        Allowed = @('{43F95ED1-95B4-421D-8F5A-837128C2D854}', '{223FD919-F8CB-476C-A852-351E7381C801}',
            '{68ED92DE-77CF-43B1-A7B2-D3BE2686E2A0}', '{22E54B52-5082-42D3-9E41-1C2E0CE74DFB}',
            'IME1.0', 'Programming IME', 'ProgrammingIME', 'pime_tsf.dll', 'pime-core.exe')
    }
}

# The work directory and the guest scripts.
$prepare = Invoke-E2EGuestScript -Config $configuration -Text (New-E2EGuestScript -Body @"
if (-not (Test-Path -LiteralPath `$Parameters.work)) { New-Item -ItemType Directory -Force -Path `$Parameters.work | Out-Null }
`$Output = @{ ok = `$true; work = `$Parameters.work }
"@ -Parameters @{ work = $configuration.workDirectory })
if (-not $prepare.ok) { Write-E2ELog "the work directory could not be prepared: $($prepare.error)" -Level 'ERROR'; exit 2 }
Install-E2EGuestScripts -Context $context | Out-Null
Write-E2ELog "  the guest scripts are in $($configuration.workDirectory)"

$results = @()
$designPath = Join-Path (Split-Path $repoRoot -Parent) '00_Documents\02_UT_IT_ST\Programming_IME_ST_Test_Design_v1.0.md'
foreach ($id in $selected) {
    Write-E2ELog ""
    Write-E2ELog "== $id"
    $function = $allCases[$id]
    . (Join-Path $PSScriptRoot "cases\$id.ps1")
    try {
        $caseResult = & $function -Context $context
        $designTitle = Get-E2EDesignTitle -Id $id -DesignPath $designPath
        if ($designTitle) { $caseResult.title = $designTitle }
        $results += $caseResult
    } catch {
        $failed = New-E2ECaseResult -Id $id -Title $id
        Add-E2ECheck -Result $failed -Name 'the case runs' -Status 'Fail' -Expected 'the case runs to the end' -Actual "$($_.Exception.Message)"
        $results += (Complete-E2ECase -Result $failed)
    }
}

$report = New-E2EReport -Cases $results -Note "Windows 11 guest $($configuration.host) ($($configuration.transport)); generated by 01_Source/tests/e2e/Invoke-E2E.ps1"
$directory = Join-Path $PSScriptRoot 'results'
$file = Write-E2EReport -Report $report -Directory $directory
Write-E2ELog ""
Write-E2ELog "the result is in $file"
Write-E2ELog "summary: $($report.pass) passed, $($report.fail) failed, $($report.notRun) not run"

if ($report.fail -gt 0) { exit 1 }
if ($report.notRun -gt 0) { exit 3 }
exit 0
