# Host side helpers of the cases: how the runner talks to the guest and what it compares.
# Sourced by Invoke-E2E.ps1; not a standalone script.

function Get-E2EGuestScriptPath {
    param([Parameter(Mandatory = $true)]$Context, [Parameter(Mandatory = $true)][string]$Name)
    return (Join-Path (Join-Path $Context.RepoRoot 'tests\e2e\guest') $Name)
}

function Invoke-E2EGuestBody {
    # Runs an inline body in the guest (as a scheduled task, so it can be elevated or run in the desktop session).
    param(
        [Parameter(Mandatory = $true)]$Context,
        [Parameter(Mandatory = $true)][string]$Body,
        [hashtable]$Parameters = @{},
        [ValidateSet('Interactive', 'S4U')][string]$LogonType = 'S4U',
        [int]$TimeoutSeconds = 180
    )
    $text = @"
`$ErrorActionPreference = 'Stop'
`$Parameters = Get-Content -LiteralPath `$ParametersPath -Raw -Encoding UTF8 | ConvertFrom-Json
`$Output = `$null
try {
$Body
} catch {
`$Output = @{ ok = `$false; error = `$_.Exception.Message }
}
if (`$null -eq `$Output) { `$Output = @{ ok = `$true } }
`$Output | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath `$ResultPath -Encoding UTF8
"@
    return (Invoke-E2EGuestTask -Config $Context.Config -ScriptText $text -Parameters $Parameters -LogonType $LogonType -TimeoutSeconds $TimeoutSeconds)
}

function Invoke-E2EGuestFile {
    # Ships a script of the guest folder to the guest and runs it there (elevated or in the interactive session).
    param(
        [Parameter(Mandatory = $true)]$Context,
        [Parameter(Mandatory = $true)][string]$Name,
        [hashtable]$Parameters = @{},
        [ValidateSet('Interactive', 'S4U')][string]$LogonType = 'S4U',
        [int]$TimeoutSeconds = 180
    )
    $path = Get-E2EGuestScriptPath -Context $Context -Name $Name
    if (-not (Test-Path -LiteralPath $path)) { throw "the guest script '$Name' is missing ($path)" }
    $text = Get-Content -LiteralPath $path -Raw -Encoding UTF8
    return (Invoke-E2EGuestTask -Config $Context.Config -ScriptText $text -Parameters $Parameters -LogonType $LogonType -TimeoutSeconds $TimeoutSeconds)
}

function Copy-E2EFileToGuest {
    # Copies a file into the guest (the installer is too big for a single command line).
    param(
        [Parameter(Mandatory = $true)]$Context,
        [Parameter(Mandatory = $true)][string]$LocalPath,
        [Parameter(Mandatory = $true)][string]$RemotePath
    )
    $bytes = [System.IO.File]::ReadAllBytes($LocalPath)
    $chunk = 393216
    $first = $true
    for ($offset = 0; $offset -lt $bytes.Length; $offset += $chunk) {
        $length = [Math]::Min($chunk, $bytes.Length - $offset)
        $part = New-Object byte[] $length
        [Array]::Copy($bytes, $offset, $part, 0, $length)
        $b64 = [Convert]::ToBase64String($part)
        $mode = if ($first) { 'Create' } else { 'Append' }
        $body = @"
`$chunk = [Convert]::FromBase64String('$b64')
`$stream = [System.IO.File]::Open('$RemotePath', [System.IO.FileMode]::$mode, [System.IO.FileAccess]::Write)
try { `$stream.Write(`$chunk, 0, `$chunk.Length) } finally { `$stream.Close() }
`$Output = @{ ok = `$true; bytes = `$chunk.Length }
"@
        $null = Invoke-E2EGuestScript -Config $Context.Config -Text (New-E2EGuestScript -Body $body)
        $first = $false
    }
    return @{ ok = $true; bytes = $bytes.Length; path = $RemotePath }
}

function Install-E2EGuestScripts {
    # Copies the guest scripts (and the shared helper) into the work directory of the guest.
    param([Parameter(Mandatory = $true)]$Context)
    $folder = Join-Path $Context.RepoRoot 'tests\e2e\guest'
    foreach ($file in (Get-ChildItem -LiteralPath $folder -Filter '*.ps1')) {
        Copy-E2EFileToGuest -Context $Context -LocalPath $file.FullName -RemotePath (Join-Path $Context.Config.workDirectory $file.Name)
    }
    return $true
}

function Get-E2ESystemState {
    param([Parameter(Mandatory = $true)]$Context)
    return (Invoke-E2EGuestFile -Context $Context -Name 'Invoke-SystemState.ps1' -LogonType 'S4U' -Parameters @{
        clsid = $Context.Product.Clsid
        profiles = $Context.Product.Profiles
        installerPath = $Context.Config.installer
        installDir = $Context.Product.InstallDir
        packs = $Context.Product.Packs
    })
}

function Get-E2EUserState {
    param([Parameter(Mandatory = $true)]$Context)
    return (Invoke-E2EGuestFile -Context $Context -Name 'Invoke-UserState.ps1' -LogonType 'Interactive' -Parameters @{})
}

function Get-E2ESnapshot {
    param([Parameter(Mandatory = $true)]$Context)
    return (Invoke-E2EGuestFile -Context $Context -Name 'Invoke-Snapshot.ps1' -LogonType 'S4U' -Parameters @{
        installDir = $Context.Product.InstallDir
        productGuids = $Context.Product.Guids
    })
}

function Invoke-E2EInstaller {
    param(
        [Parameter(Mandatory = $true)]$Context,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [string]$InstallerPath = '',
        [int]$TimeoutSeconds = 900
    )
    if (-not $InstallerPath) { $InstallerPath = $Context.InstallerInGuest }
    $log = Join-Path $Context.Config.workDirectory ("install-" + [Guid]::NewGuid().ToString('N').Substring(0, 8) + '.log')
    $result = Invoke-E2EGuestFile -Context $Context -Name 'Invoke-Installer.ps1' -LogonType 'S4U' -TimeoutSeconds $TimeoutSeconds -Parameters @{
        installerPath = $InstallerPath
        arguments = $Arguments
        logPath = $log
        timeoutSeconds = $TimeoutSeconds
    }
    $result | Add-Member -NotePropertyName 'logPath' -NotePropertyValue $log -Force
    return $result
}

function Invoke-E2EActiveProfile {
    param([Parameter(Mandatory = $true)]$Context)
    return (Invoke-E2EGuestFile -Context $Context -Name 'Invoke-ActiveProfile.ps1' -LogonType 'Interactive' -Parameters @{
        clsid = $Context.Product.Clsid
    })
}

function Set-E2EOverride {
    # Sets the default input method of the interactive user (the mechanical "Win+Space selects it").
    param(
        [Parameter(Mandatory = $true)]$Context,
        [Parameter(Mandatory = $true)][string]$Tip
    )
    return (Invoke-E2EGuestFile -Context $Context -Name 'Invoke-SetImeState.ps1' -LogonType 'Interactive' -Parameters @{
        mode = 'set-override'; tip = $Tip
    })
}

function Ensure-E2ETip {
    # Makes sure the keyboard of the product is offered for a language (the design step "install and tick that language").
    param(
        [Parameter(Mandatory = $true)]$Context,
        [Parameter(Mandatory = $true)][string]$LanguageTag,
        [Parameter(Mandatory = $true)][string]$Tip
    )
    return (Invoke-E2EGuestFile -Context $Context -Name 'Invoke-SetImeState.ps1' -LogonType 'Interactive' -Parameters @{
        mode = 'ensure-tip'; tip = $Tip; languageTag = $LanguageTag
    })
}

function Invoke-E2ENotepad {
    # Types a text into a real Notepad window of the guest and returns what Notepad wrote to the disk.
    param(
        [Parameter(Mandatory = $true)]$Context,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text,
        [int]$Backspaces = 0,
        [string[]]$Keys = @(),
        [string]$Name = 'typing.txt'
    )
    $textB64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($Text))
    return (Invoke-E2EGuestFile -Context $Context -Name 'Invoke-NotepadTyping.ps1' -LogonType 'Interactive' -TimeoutSeconds 180 -Parameters @{
        path = (Join-Path $Context.Config.workDirectory $Name)
        textB64 = $textB64
        backspaces = $Backspaces
        keys = $Keys
        settleSeconds = 2
    })
}

function Compare-E2ESnapshot {
    # The "only the components of this product changed" check: every difference must name one of our own keys.
    param(
        [Parameter(Mandatory = $true)]$Before,
        [Parameter(Mandatory = $true)]$After,
        [Parameter(Mandatory = $true)][string[]]$Allowed
    )
    $foreign = @()
    foreach ($name in $Before.exports.PSObject.Properties.Name) {
        $a = @($Before.exports.$name -split "`r?`n")
        $b = @($After.exports.$name -split "`r?`n")
        foreach ($item in (Compare-Object $a $b)) {
            $line = "$($item.InputObject)"
            $mine = $false
            foreach ($needle in $Allowed) { if ($line -like "*$needle*") { $mine = $true; break } }
            if (-not $mine) { $foreign += "$name : $line" }
        }
    }
    foreach ($item in (Compare-Object @($Before.installListing -split "`r?`n") @($After.installListing -split "`r?`n"))) {
        $line = "$($item.InputObject)"
        $mine = $false
        foreach ($needle in $Allowed) { if ($line -like "*$needle*") { $mine = $true; break } }
        if (-not $mine) { $foreign += "install dir : $line" }
    }
    foreach ($item in (Compare-Object @($Before.programs) @($After.programs))) {
        $line = "$($item.InputObject)"
        $mine = $false
        foreach ($needle in $Allowed) { if ($line -like "*$needle*") { $mine = $true; break } }
        if (-not $mine) { $foreign += "programs : $line" }
    }
    return $foreign
}

function Get-E2EOriginalTip {
    # The input method the user had before the product: the default override, or the first keyboard of the list.
    param(
        [Parameter(Mandatory = $true)]$UserState,
        [Parameter(Mandatory = $true)][string]$ProductClsid
    )
    if ($UserState.defaultOverride) { return "$($UserState.defaultOverride)" }
    foreach ($language in $UserState.languages) {
        foreach ($tip in $language.tips) {
            if ($tip -notlike "*$ProductClsid*") { return "$tip" }
        }
    }
    return $null
}

function Install-E2EProduct {
    # Copies the installer into the guest and runs it (silent). Returns the result of the installer.
    param([Parameter(Mandatory = $true)]$Context, [switch]$Uninstall)
    if (-not $Context.InstallerCopied) {
        Copy-E2EFileToGuest -Context $Context -LocalPath $Context.Config.installer -RemotePath $Context.InstallerInGuest | Out-Null
        $Context.InstallerCopied = $true
    }
    if ($Uninstall) {
        return (Invoke-E2EInstaller -Context $Context -Arguments @('--uninstall', '--silent'))
    }
    return (Invoke-E2EInstaller -Context $Context -Arguments @('--silent'))
}

function Invoke-E2EProfileCase {
    # ST-001 / ST-002 / ST-003: the profile of one system language appears, is activated, types, and the original
    # input method comes back unchanged - once before the restart and once after it.
    param(
        [Parameter(Mandatory = $true)]$Context,
        [Parameter(Mandatory = $true)]$Result,
        [Parameter(Mandatory = $true)][ValidateSet('zh', 'en', 'ja')][string]$LanguageKey
    )
    $profile = $Context.Product.Profiles[$LanguageKey]
    $tip = "$($profile.langId):$($Context.Product.Clsid)$($profile.guid)"
    $langKey = '0x0000' + $profile.langId
    $sample = 'Rust'

    # 1. the environment
    $state = Get-E2ESystemState -Context $Context
    if (-not $state.ok) {
        Add-E2ECheck -Result $Result -Name 'the guest answers' -Status 'Fail' -Expected 'a system state' -Actual "$($state.error)"
        return (Complete-E2ECase -Result $Result)
    }
    Add-E2ECheck -Result $Result -Name 'Windows 11 x64' -Status $(if ([int]$state.os.build -ge 22000) { 'Pass' } else { 'Fail' }) `
        -Expected 'build 22000 or newer' -Actual "$($state.os.caption) $($state.os.version) build $($state.os.build) ($($state.os.architecture))"

    # 2. the product is installed (the design step "install and tick that language")
    if (-not ($state.tipRegistered -and $state.clsidRegistered)) {
        Write-E2ELog "the product is not installed in the guest, installing it"
        $install = Install-E2EProduct -Context $Context
        $state = Get-E2ESystemState -Context $Context
        Add-E2ECheck -Result $Result -Name 'the installer succeeds' -Status $(if ($install.ok -and $install.exitCode -in @(0, 3010)) { 'Pass' } else { 'Fail' }) `
            -Expected 'exit code 0 (or 3010)' -Actual "exit code $($install.exitCode) $($install.error)"
    }
    Add-E2ECheck -Result $Result -Name 'the text service is registered' -Status $(if ($state.tipRegistered -and $state.clsidRegistered) { 'Pass' } else { 'Fail' }) `
        -Expected 'the TIP and the CLSID of the product exist in HKLM' -Actual "tip=$($state.tipRegistered) clsid=$($state.clsidRegistered)"

    # 3. the profile of this language is one of the three of the product
    $registered = @($state.languageProfiles.$langKey)
    Add-E2ECheck -Result $Result -Name "the $langKey profile is registered" -Status $(if ($registered -contains $profile.guid) { 'Pass' } else { 'Fail' }) `
        -Expected "$($profile.guid) under $langKey" -Actual "$($registered -join ', ')"

    # 4. the baseline of the user: the original input method and the language list
    $before = Get-E2EUserState -Context $Context
    $originalTip = Get-E2EOriginalTip -UserState $before -ProductClsid $Context.Product.Clsid
    if (-not $originalTip) {
        Add-E2ECheck -Result $Result -Name 'the original input method is known' -Status 'Fail' `
            -Expected 'a default input method in the language list' -Actual 'none found'
        return (Complete-E2ECase -Result $Result)
    }
    $Result.notes += "the original input method of the guest is $originalTip"

    # 5. the keyboard of the product is offered for this language ("install and tick that language")
    $ensure = Ensure-E2ETip -Context $Context -LanguageTag $profile.languageTag -Tip $tip
    Add-E2ECheck -Result $Result -Name "the keyboard is offered for $($profile.languageTag)" `
        -Status $(if ($ensure.ok -and @($ensure.languages | Where-Object { $_.tag -eq $profile.languageTag }).tips -contains $tip) { 'Pass' } else { 'Fail' }) `
        -Expected "$tip in the language list of $($profile.languageTag)" `
        -Actual $(if ($ensure.ok) { "changed=$($ensure.changed)" } else { "$($ensure.error)" })

    # 6. the baseline of the original input method: it must behave the same before and after
    $null = Set-E2EOverride -Context $Context -Tip $originalTip
    $baseline = Invoke-E2ENotepad -Context $Context -Text $sample -Name "st-original-$LanguageKey.txt"
    Add-E2ECheck -Result $Result -Name 'the original input method types' -Status $(if ($baseline.ok) { 'Pass' } else { 'Fail' }) `
        -Expected "the text '$sample' reaches Notepad" -Actual $(if ($baseline.ok) { "'$($baseline.text)'" } else { "$($baseline.error)" })

    # 7. activate the product: the default input method is the profile of this language
    $activate = Set-E2EOverride -Context $Context -Tip $tip
    Add-E2ECheck -Result $Result -Name 'the product is activated' -Status $(if ($activate.ok -and "$($activate.defaultOverride)" -eq $tip) { 'Pass' } else { 'Fail' }) `
        -Expected "the default input method is $tip" -Actual "$($activate.defaultOverride)"

    # 8. the product types: the Direct path must not eat a single character
    $typed = Invoke-E2ENotepad -Context $Context -Text $sample -Name "st-product-$LanguageKey.txt"
    $expected = $sample
    Add-E2ECheck -Result $Result -Name 'the product types the text' -Status $(if ($typed.ok -and $typed.text -eq $expected) { 'Pass' } else { 'Fail' }) `
        -Expected "Notepad contains exactly '$expected'" -Actual $(if ($typed.ok) { "'$($typed.text)'" } else { "$($typed.error)" })

    # 9. back to the original input method: it must behave exactly as before
    $back = Set-E2EOverride -Context $Context -Tip $originalTip
    $again = Invoke-E2ENotepad -Context $Context -Text $sample -Name "st-back-$LanguageKey.txt"
    Add-E2ECheck -Result $Result -Name 'the original input method is active again' `
        -Status $(if ($back.ok -and "$($back.defaultOverride)" -eq $originalTip) { 'Pass' } else { 'Fail' }) `
        -Expected "the default input method is $originalTip again" -Actual "$($back.defaultOverride)"
    Add-E2ECheck -Result $Result -Name 'the original input method is undamaged' `
        -Status $(if ($again.ok -and $baseline.ok -and $again.text -eq $baseline.text) { 'Pass' } else { 'Fail' }) `
        -Expected "Notepad contains what it contained before ('$($baseline.text)')" `
        -Actual $(if ($again.ok) { "'$($again.text)'" } else { "$($again.error)" })

    # 10. the default settings and the other input methods are untouched
    $after = Get-E2EUserState -Context $Context
    $foreign = @()
    foreach ($language in $before.languages) {
        $now = @($after.languages | Where-Object { $_.tag -eq $language.tag })
        foreach ($tipOfOther in $language.tips) {
            if (@($now.tips) -notcontains $tipOfOther) { $foreign += "$($language.tag): $tipOfOther is gone" }
        }
    }
    Add-E2ECheck -Result $Result -Name 'no other input method was removed' -Status $(if ($foreign.Count -eq 0) { 'Pass' } else { 'Fail' }) `
        -Expected 'every keyboard of the original list is still there' -Actual ($foreign -join '; ')

    # 11. restart and repeat the activation (the design asks for it twice)
    if (-not $Context.Config.restart.enabled) {
        Add-E2ECheck -Result $Result -Name 'the same after a restart' -Status 'NOT RUN' `
            -Expected 'the activation works again after the restart' -Actual 'the restart is disabled in the configuration'
    } else {
        $restart = Restart-E2EGuest -Config $Context.Config
        if (-not $restart.ok) {
            Add-E2ECheck -Result $Result -Name 'the guest restarts' -Status 'Fail' -Expected 'the guest comes back' -Actual $restart.message
        } else {
            Add-E2ECheck -Result $Result -Name 'the guest restarts' -Status 'Pass' -Expected 'the guest comes back' -Actual $restart.message
            $activate2 = Set-E2EOverride -Context $Context -Tip $tip
            $typed2 = Invoke-E2ENotepad -Context $Context -Text $sample -Name "st-product2-$LanguageKey.txt"
            Add-E2ECheck -Result $Result -Name 'the product is activated again after the restart' `
                -Status $(if ($activate2.ok -and "$($activate2.defaultOverride)" -eq $tip -and $typed2.ok -and $typed2.text -eq $expected) { 'Pass' } else { 'Fail' }) `
                -Expected "the default input method is $tip and Notepad contains '$expected'" `
                -Actual "override=$($activate2.defaultOverride) text='$(if ($typed2.ok) { $typed2.text } else { $typed2.error })'"
            $null = Set-E2EOverride -Context $Context -Tip $originalTip
        }
    }
    $null = Set-E2EOverride -Context $Context -Tip $originalTip
    return (Complete-E2ECase -Result $Result)
}
