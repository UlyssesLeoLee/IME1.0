# ST-035 - the signed installer: install, repair, upgrade, uninstall and the restart.
# The judgement of the design (Programming_IME_ST_Test_Design_v1.0.md, ST-035):
#   "only the components of this product change, and a failed upgrade can be
#   rolled back.
#
# Every step is checked against the machine itself:
#   1  the installer carries a valid signature (the design asks for a signed one)
#   2  a new install: the exit code, the registration, the files
#   3  only the components of the product changed (the registry and the program list are compared)
#   4  repair: the same installer again, nothing changes
#   5  upgrade: an older installer first, then the new one (only when an older one is configured)
#   6  the rollback: a write error is injected, the installer must fail and leave the machine as it was
#   7  the uninstall: the registration and the files are gone
#   8  the restart: what was left for the restart is gone as well
function Invoke-E2ECaseST035 {
    param([Parameter(Mandatory = $true)]$Context)
    $result = New-E2ECaseResult -Id 'ST-035' -Title 'the signed installer: install, repair, uninstall and the restart'
    $allowed = @($Context.Product.Allowed)

    # --- 1: the signature -----------------------------------------------------------------------------------
    $state = Get-E2ESystemState -Context $Context
    if (-not $state.ok) {
        Add-E2ECheck -Result $result -Name 'the guest answers' -Status 'Fail' -Expected 'a system state' -Actual "$($state.error)"
        return (Complete-E2ECase -Result $result)
    }
    if (-not $state.signature) {
        Add-E2ECheck -Result $result -Name 'the installer is signed' -Status 'Fail' `
            -Expected 'an Authenticode signature' -Actual 'the installer of the configuration does not exist'
        return (Complete-E2ECase -Result $result)
    }
    $signed = "$($state.signature.status)" -eq 'Valid'
    Add-E2ECheck -Result $result -Name 'the installer is signed' -Status $(if ($signed) { 'Pass' } else { 'Fail' }) `
        -Expected 'the Authenticode signature is valid' `
        -Actual "status=$($state.signature.status) signer=$($state.signature.subject)"

    # --- 2: a new install -----------------------------------------------------------------------------------
    $before = Get-E2ESnapshot -Context $Context
    $install = Install-E2EProduct -Context $Context
    $installOk = $install.ok -and $install.exitCode -in @(0, 3010)
    Add-E2ECheck -Result $result -Name 'the new install succeeds' -Status $(if ($installOk) { 'Pass' } else { 'Fail' }) `
        -Expected 'exit code 0 (3010 means: a restart is needed)' `
        -Actual "exit code $($install.exitCode) $($install.error)"

    $state = Get-E2ESystemState -Context $Context
    $profiles = @($state.languageProfiles.PSObject.Properties.Name)
    Add-E2ECheck -Result $result -Name 'the three profiles are registered' `
        -Status $(if ($profiles.Count -eq 3 -and $state.clsidRegistered) { 'Pass' } else { 'Fail' }) `
        -Expected 'three language profiles and the CLSID' -Actual "($($profiles -join ', ')) clsid=$($state.clsidRegistered)"
    Add-E2ECheck -Result $result -Name 'the program is in "Apps & features"' `
        -Status $(if ($state.arp) { 'Pass' } else { 'Fail' }) `
        -Expected 'an entry of the product' -Actual $(if ($state.arp) { "$($state.arp.displayName) $($state.arp.displayVersion)" } else { 'no entry' })

    # --- 3: only the components of the product changed -----------------------------------------------------
    $after = Get-E2ESnapshot -Context $Context
    $foreign = Compare-E2ESnapshot -Before $before -After $after -Allowed $allowed
    Add-E2ECheck -Result $result -Name 'only the components of the product changed' `
        -Status $(if ($foreign.Count -eq 0) { 'Pass' } else { 'Fail' }) `
        -Expected 'every difference names the product' -Actual (($foreign | Select-Object -First 5) -join '; ')

    # --- 4: repair (the same installer again) --------------------------------------------------------------
    $filesBefore = @($state.files | ForEach-Object { "$($_.path)|$($_.sha256)" })
    $repair = Install-E2EProduct -Context $Context
    $state2 = Get-E2ESystemState -Context $Context
    $filesAfter = @($state2.files | ForEach-Object { "$($_.path)|$($_.sha256)" })
    $same = @(Compare-Object $filesBefore $filesAfter).Count -eq 0
    Add-E2ECheck -Result $result -Name 'the repair (the same installer again) succeeds' `
        -Status $(if ($repair.ok -and $repair.exitCode -in @(0, 3010)) { 'Pass' } else { 'Fail' }) `
        -Expected 'exit code 0' -Actual "exit code $($repair.exitCode) $($repair.error)"
    Add-E2ECheck -Result $result -Name 'the repair changes nothing' -Status $(if ($same) { 'Pass' } else { 'Fail' }) `
        -Expected 'the same files with the same hashes' `
        -Actual (@(Compare-Object $filesBefore $filesAfter | Select-Object -First 3 | ForEach-Object { "$($_.InputObject)" }) -join '; ')

    # --- 5: upgrade ----------------------------------------------------------------------------------------
    $older = $Context.Config.olderInstaller
    if (-not $older) {
        Add-E2ECheck -Result $result -Name 'the upgrade replaces the old version' -Status 'NOT RUN' `
            -Expected 'an older installer to upgrade from' -Actual 'the configuration names no older installer'
    } else {
        if (-not (Test-Path -LiteralPath $older)) {
            Add-E2ECheck -Result $result -Name 'the upgrade replaces the old version' -Status 'Fail' `
                -Expected 'the older installer exists on this machine' -Actual "the file $older does not exist"
        } else {
            Copy-E2EFileToGuest -Context $Context -LocalPath $older -RemotePath $Context.OlderInstallerInGuest | Out-Null
            $old = Invoke-E2EInstaller -Context $Context -Arguments @('--silent') -InstallerPath $Context.OlderInstallerInGuest
            $upgrade = Invoke-E2EInstaller -Context $Context -Arguments @('--silent')
            $state3 = Get-E2ESystemState -Context $Context
            Add-E2ECheck -Result $result -Name 'the upgrade replaces the old version' `
                -Status $(if ($upgrade.ok -and $upgrade.exitCode -in @(0, 3010) -and $state3.arp -and $state3.arp.displayVersion -eq $Context.Config.expectedVersion) { 'Pass' } else { 'Fail' }) `
                -Expected "version $($Context.Config.expectedVersion) in Apps & features" `
                -Actual "old exit=$($old.exitCode) upgrade exit=$($upgrade.exitCode) version=$($state3.arp.displayVersion)"
        }
    }

    # --- 6: the rollback ------------------------------------------------------------------------------------
    $beforeFailure = Get-E2ESnapshot -Context $Context
    $deny = Invoke-E2EGuestBody -Context $Context -LogonType 'S4U' -Parameters @{ dir = $Context.Product.InstallDir } -Body @'
if (-not (Test-Path -LiteralPath $Parameters.dir)) { throw "the install directory does not exist" }
$out = & icacls.exe $Parameters.dir /deny '*S-1-5-32-544:(OI)(CI)(W,D)' 2>&1
$Output = @{ ok = $true; icacls = ($out -join "`n") }
'@
    $failed = Invoke-E2EInstaller -Context $Context -Arguments @('--silent')
    $restore = Invoke-E2EGuestBody -Context $Context -LogonType 'S4U' -Parameters @{ dir = $Context.Product.InstallDir } -Body @'
$out = & icacls.exe $Parameters.dir /remove:d '*S-1-5-32-544' 2>&1
$out2 = & icacls.exe $Parameters.dir /grant:r '*S-1-5-32-544:(OI)(CI)(F)' 2>&1
$Output = @{ ok = $true; icacls = (($out + $out2) -join "`n") }
'@
    $afterFailure = Get-E2ESnapshot -Context $Context
    $leftovers = @()
    foreach ($name in $beforeFailure.exports.PSObject.Properties.Name) {
        $leftovers += @(Compare-Object @($beforeFailure.exports.$name -split "`r?`n") @($afterFailure.exports.$name -split "`r?`n"))
    }
    Add-E2ECheck -Result $result -Name 'a failed install rolls back' `
        -Status $(if ($failed.ok -and $failed.exitCode -ne 0 -and $leftovers.Count -eq 0) { 'Pass' } else { 'Fail' }) `
        -Expected 'the installer fails and the machine is unchanged' `
        -Actual "exit code $($failed.exitCode), $($leftovers.Count) registry difference(s)"

    # --- 7: the uninstall -----------------------------------------------------------------------------------
    $uninstall = Invoke-E2EInstaller -Context $Context -Arguments @('--uninstall', '--silent')
    Add-E2ECheck -Result $result -Name 'the uninstall succeeds' `
        -Status $(if ($uninstall.ok -and $uninstall.exitCode -in @(0, 3010)) { 'Pass' } else { 'Fail' }) `
        -Expected 'exit code 0' -Actual "exit code $($uninstall.exitCode) $($uninstall.error)"
    $state4 = Get-E2ESystemState -Context $Context
    Add-E2ECheck -Result $result -Name 'the registration is gone' `
        -Status $(if (-not $state4.tipRegistered -and -not $state4.clsidRegistered -and -not $state4.arp) { 'Pass' } else { 'Fail' }) `
        -Expected 'no TIP, no CLSID, no entry in Apps & features' `
        -Actual "tip=$($state4.tipRegistered) clsid=$($state4.clsidRegistered) arp=$($state4.arp)"

    # --- 8: the restart -------------------------------------------------------------------------------------
    if (-not $Context.Config.restart.enabled) {
        Add-E2ECheck -Result $result -Name 'the restart finishes the uninstall' -Status 'NOT RUN' `
            -Expected 'the files that were left for the restart are gone' -Actual 'the restart is disabled in the configuration'
    } else {
        $restart = Restart-E2EGuest -Config $Context.Config
        if (-not $restart.ok) {
            Add-E2ECheck -Result $result -Name 'the guest restarts' -Status 'Fail' -Expected 'the guest comes back' -Actual $restart.message
        } else {
            $state5 = Get-E2ESystemState -Context $Context
            $gone = -not (Test-Path -LiteralPath $Context.Product.InstallDir)
            Add-E2ECheck -Result $result -Name 'the restart finishes the uninstall' -Status $(if ($gone) { 'Pass' } else { 'Fail' }) `
                -Expected "the directory $($Context.Product.InstallDir) is gone" `
                -Actual "the directory exists: $(-not $gone); tip=$($state5.tipRegistered)"
        }
    }
    return (Complete-E2ECase -Result $result)
}
