# ST-005 - a long text goes into an ordinary Notepad in the Direct mode.
# The judgement of the design (Programming_IME_ST_Test_Design_v1.0.md, ST-005):
#   "the content of the host is the same as the standard keyboard
#   baseline, nothing is lost and nothing is written twice.
#
# The baseline is measured in the same guest, in the same Notepad, with the original input method: the same key
# sequence must produce the same file. The text is 1000 characters with Chinese and English symbols, a line break
# and three backspaces (the "and backspace" of the design).
function Invoke-E2ECaseST005 {
    param([Parameter(Mandatory = $true)]$Context)
    $result = New-E2ECaseResult -Id 'ST-005' -Title 'a long text goes into an ordinary Notepad in the Direct mode'

    $state = Get-E2ESystemState -Context $Context
    if (-not $state.ok) {
        Add-E2ECheck -Result $result -Name 'the guest answers' -Status 'Fail' -Expected 'a system state' -Actual "$($state.error)"
        return (Complete-E2ECase -Result $result)
    }
    if (-not ($state.tipRegistered -and $state.clsidRegistered)) {
        Add-E2ECheck -Result $result -Name 'the product is installed' -Status 'Fail' `
            -Expected 'the text service of the product is registered' -Actual 'it is not registered in the guest'
        return (Complete-E2ECase -Result $result)
    }
    Add-E2ECheck -Result $result -Name 'the AI is off' -Status $(if ($state.aiWorkerRunning -eq 0) { 'Pass' } else { 'Fail' }) `
        -Expected 'no pime-ai-worker process' -Actual "$($state.aiWorkerRunning) process(es)"

    # The text: 1000 characters with Chinese and English symbols.
    $chunk = 'let x = 1; // chinese comment symbols {}[](),;:"'' +-*/=<>!@#$%^&*_|~` abcdefghijklmnopqrstuvwxyz0123456789'
    $payload = ''
    while ($payload.Length -lt 1000) { $payload += $chunk }
    $payload = $payload.Substring(0, 1000)
    $keys = @('enter', 'enter')      # the CRLF of the fixture
    $backspaces = 3

    $user = Get-E2EUserState -Context $Context
    $originalTip = Get-E2EOriginalTip -UserState $user -ProductClsid $Context.Product.Clsid
    if (-not $originalTip) {
        Add-E2ECheck -Result $result -Name 'the original input method is known' -Status 'Fail' `
            -Expected 'a default input method in the language list' -Actual 'none found'
        return (Complete-E2ECase -Result $result)
    }

    # The baseline: the same key sequence with the original input method.
    $null = Set-E2EOverride -Context $Context -Tip $originalTip
    $baseline = Invoke-E2ENotepad -Context $Context -Text $payload -Keys $keys -Backspaces $backspaces -Name 'st005-baseline.txt'
    if (-not $baseline.ok) {
        Add-E2ECheck -Result $result -Name 'the baseline with the original input method' -Status 'Fail' `
            -Expected 'a text file with 1000 typed characters' -Actual "$($baseline.error)"
        return (Complete-E2ECase -Result $result)
    }
    Add-E2ECheck -Result $result -Name 'the baseline with the original input method' -Status 'Pass' `
        -Expected 'the file of the standard keyboard' -Actual "$($baseline.length) bytes, sha256 $($baseline.sha256)"

    # The same key sequence with the product.
    $tip = "$($Context.Product.Profiles.en.langId):$($Context.Product.Clsid)$($Context.Product.Profiles.en.guid)"
    $null = Set-E2EOverride -Context $Context -Tip $tip
    $actual = Invoke-E2ENotepad -Context $Context -Text $payload -Keys $keys -Backspaces $backspaces -Name 'st005-product.txt'
    if (-not $actual.ok) {
        Add-E2ECheck -Result $result -Name 'the product types the long text' -Status 'Fail' `
            -Expected 'the same file as the baseline' -Actual "$($actual.error)"
        return (Complete-E2ECase -Result $result)
    }

    $same = $actual.sha256 -eq $baseline.sha256
    Add-E2ECheck -Result $result -Name 'the content is the same as the standard keyboard baseline' `
        -Status $(if ($same) { 'Pass' } else { 'Fail' }) `
        -Expected "sha256 $($baseline.sha256) ($($baseline.length) bytes)" `
        -Actual "sha256 $($actual.sha256) ($($actual.length) bytes)"

    $expectedLength = $baseline.length
    Add-E2ECheck -Result $result -Name 'nothing is lost and nothing is written twice' `
        -Status $(if ($actual.length -eq $expectedLength) { 'Pass' } else { 'Fail' }) `
        -Expected "$expectedLength bytes" -Actual "$($actual.length) bytes"

    $head = $payload.Substring(0, 200)
    Add-E2ECheck -Result $result -Name 'the beginning of the text is intact' `
        -Status $(if ($actual.text.StartsWith($head)) { 'Pass' } else { 'Fail' }) `
        -Expected "the file starts with '$($head.Substring(0, 40))...'" -Actual "the first 40 characters: '$($actual.text.Substring(0, [Math]::Min(40, $actual.text.Length)))'"

    $null = Set-E2EOverride -Context $Context -Tip $originalTip
    return (Complete-E2ECase -Result $result)
}
