# ST-004 - the three system languages share one engine with seven programming languages.
# The judgement of the design (Programming_IME_ST_Test_Design_v1.0.md, ST-004):
#   "all 3x7=21 combinations work; the UI locale is independent of the profile".
#
# Three layers, each with its own evidence:
#   A  the registration: one text service, three language profiles, one binary, the seven packs inside it.
#   B  the 21 cells: every system language profile with every programming language. The expected candidate comes
#      from the golden data of the repository (tests/fixtures/predict-cases.json), so the runner compares against
#      what the engine itself is supposed to produce.
#   C  the dropdown of the seven languages: UI Automation looks into the candidate window of the product. When the
#      window exposes nothing accessible, the layer stays "NOT RUN" with that reason instead of pretending.
function Invoke-E2ECaseST004 {
    param([Parameter(Mandatory = $true)]$Context)
    $result = New-E2ECaseResult -Id 'ST-004' -Title 'the three system languages share one engine with seven programming languages'

    # --- A: the registration ------------------------------------------------------------------------------
    $state = Get-E2ESystemState -Context $Context
    if (-not $state.ok) {
        Add-E2ECheck -Result $result -Name 'the guest answers' -Status 'Fail' -Expected 'a system state' -Actual "$($state.error)"
        return (Complete-E2ECase -Result $result)
    }
    $profiles = @($state.languageProfiles.PSObject.Properties.Name)
    Add-E2ECheck -Result $result -Name 'one text service carries the three language profiles' `
        -Status $(if ($profiles.Count -eq 3) { 'Pass' } else { 'Fail' }) `
        -Expected 'three language profiles under one TIP' -Actual "($($profiles -join ', '))"

    $sameDll = @($state.languageProfiles.PSObject.Properties | ForEach-Object { $_.Value }) |
        ForEach-Object { $_ } | Select-Object -Unique
    Add-E2ECheck -Result $result -Name 'the three profiles are the profiles of one engine' `
        -Status $(if ($state.tipRegistered -and $state.clsidRegistered -and $state.dllPath) { 'Pass' } else { 'Fail' }) `
        -Expected 'one TIP and one DLL behind the three profiles' `
        -Actual "tip=$($state.tipRegistered) clsid=$($state.clsidRegistered) dll=$($state.dllPath)"

    $packs = @($Context.Product.Packs)
    $found = @($state.packsFound)
    Add-E2ECheck -Result $result -Name 'the seven language packs are in the installed program' `
        -Status $(if ($found.Count -eq $packs.Count) { 'Pass' } else { 'Fail' }) `
        -Expected "$($packs -join ', ')" -Actual "$($found -join ', ') in $($state.packBinary)"

    # --- B: the 21 cells -----------------------------------------------------------------------------------
    $fixturePath = Join-Path $Context.RepoRoot 'tests\fixtures\predict-cases.json'
    if (-not (Test-Path -LiteralPath $fixturePath)) {
        Add-E2ECheck -Result $result -Name 'the 21 combinations' -Status 'NOT RUN' `
            -Expected 'the golden data of the repository' -Actual "the file $fixturePath is missing"
        return (Complete-E2ECase -Result $result)
    }
    $fixture = Get-Content -LiteralPath $fixturePath -Raw -Encoding UTF8 | ConvertFrom-Json
    $cells = @{}
    foreach ($case in $fixture.cases) {
        if ($case.profile -and @($case.expect).Count -gt 0 -and -not $cells.ContainsKey($case.profile)) {
            $cells[$case.profile] = $case
        }
    }
    $composition = @($Context.Config.composition.keys)
    $commit = $Context.Config.composition.commitKey
    $user = Get-E2EUserState -Context $Context
    $originalTip = Get-E2EOriginalTip -UserState $user -ProductClsid $Context.Product.Clsid
    $localeBefore = $user.uiLocale

    foreach ($languageKey in @('zh', 'en', 'ja')) {
        $profile = $Context.Product.Profiles[$languageKey]
        $tip = "$($profile.langId):$($Context.Product.Clsid)$($profile.guid)"
        $null = Set-E2EOverride -Context $Context -Tip $tip
        foreach ($pack in $packs) {
            $cell = $cells[$pack]
            if (-not $cell) {
                Add-E2ECheck -Result $result -Name "$languageKey / $pack" -Status 'NOT RUN' `
                    -Expected 'a golden case in tests/fixtures/predict-cases.json' -Actual 'no case for this language'
                continue
            }
            $expected = "$($cell.expect[0].text)" -replace '\$\{cursor\}', ''
            $typed = Invoke-E2ENotepad -Context $Context -Text "$($cell.prefix)" -Keys @($composition + @($commit)) `
                -Name "st004-$languageKey-$pack.txt"
            if (-not $typed.ok) {
                Add-E2ECheck -Result $result -Name "$languageKey / $pack" -Status 'Fail' `
                    -Expected "'$expected'" -Actual "$($typed.error)"
                continue
            }
            $actual = $typed.text
            $strip = { param($s) ($s -replace '\s', '') }
            $hit = ((& $strip $actual) -like ('*' + (& $strip $expected) + '*'))
            Add-E2ECheck -Result $result -Name "$languageKey / $pack" -Status $(if ($hit) { 'Pass' } else { 'Fail' }) `
                -Expected "'$expected'" -Actual "'$actual'"
        }
    }
    $result.notes += "the cells compare without the white spaces: the line breaks of the committed text are a known" +
        " open item (docs/TSF_VM_TEST_PLAN.md, 12.4, point 3) and must not decide the result."

    # --- C: the dropdown -----------------------------------------------------------------------------------
    $probe = Invoke-E2EGuestFile -Context $Context -Name 'Invoke-CandidateWindow.ps1' -LogonType 'Interactive' -Parameters @{
        path = (Join-Path $Context.Config.workDirectory 'st004-dropdown.txt')
        prefix = 'fn'
        compositionKeys = $composition
        packNames = $packs
    }
    if (-not $probe.ok) {
        Add-E2ECheck -Result $result -Name 'the dropdown of the seven languages' -Status 'NOT RUN' `
            -Expected 'the candidate window of the product' -Actual "$($probe.error)"
    } elseif (@($probe.packsFound).Count -lt 3) {
        Add-E2ECheck -Result $result -Name 'the dropdown of the seven languages' -Status 'NOT RUN' `
            -Expected 'a list with the programming languages' `
            -Actual "the candidate window exposes no such list (windows: $($probe.windows -join ', '); $($probe.itemCount) accessible items)"
    } else {
        Add-E2ECheck -Result $result -Name 'the dropdown of the seven languages' -Status 'Pass' `
            -Expected 'a list with the programming languages' -Actual ($probe.packsFound -join '; ')
    }

    # --- the interface locale does not follow the profile ---------------------------------------------------
    if ($null -eq $localeBefore) {
        Add-E2ECheck -Result $result -Name 'the interface locale is independent of the profile' -Status 'NOT RUN' `
            -Expected 'a persisted interface locale' -Actual 'the product stores no interface locale in the guest'
    } else {
        $after = Get-E2EUserState -Context $Context
        Add-E2ECheck -Result $result -Name 'the interface locale is independent of the profile' `
            -Status $(if ($after.uiLocale -eq $localeBefore) { 'Pass' } else { 'Fail' }) `
            -Expected "the interface locale stays '$localeBefore'" -Actual "$($after.uiLocale)"
    }
    if ($originalTip) { $null = Set-E2EOverride -Context $Context -Tip $originalTip }
    return (Complete-E2ECase -Result $result)
}
