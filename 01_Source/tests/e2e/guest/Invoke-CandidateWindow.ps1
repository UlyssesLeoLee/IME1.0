# Guest side: looks at the candidate window the product opens in Notepad and reports what it exposes.
# Runs in the interactive session of the logged-on user.
#   powershell -NoProfile -File Invoke-CandidateWindow.ps1 -ParametersPath <file> -ResultPath <file>
# The parameters file holds: path, prefix, compositionKeys (array), packNames (array).
#
# ST-004 asks for the dropdown of the seven programming languages. The window of the product is a Win32 window of
# the host process (the text service runs in-process), so UI Automation can look into it. What it finds is reported
# as it is: the case decides from the answer whether the dropdown was driven or has to stay "NOT RUN".
param(
    [Parameter(Mandatory = $true)][string]$ParametersPath,
    [Parameter(Mandatory = $true)][string]$ResultPath
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'PimeKeys.ps1')
$Parameters = Get-Content -LiteralPath $ParametersPath -Raw -Encoding UTF8 | ConvertFrom-Json
$Output = $null
try {
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    $target = $Parameters.path
    if (Test-Path -LiteralPath $target) { Remove-Item -LiteralPath $target -Force }
    Set-Content -LiteralPath $target -Value '' -Encoding UTF8 -NoNewline
    $notepad = Start-Process -FilePath 'notepad.exe' -ArgumentList "`"$target`"" -PassThru
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline -and $notepad.MainWindowHandle -eq 0) {
        Start-Sleep -Milliseconds 200
        $notepad.Refresh()
    }
    if ($notepad.MainWindowHandle -eq 0) { throw "the Notepad window did not appear within 30 s" }
    [void][PimeKeys]::ShowWindow($notepad.MainWindowHandle, 9)
    [void][PimeKeys]::SetForegroundWindow($notepad.MainWindowHandle)
    Start-Sleep -Seconds 2
    [PimeKeys]::TypeUnicode($Parameters.prefix)
    Start-Sleep -Milliseconds 500
    foreach ($key in @($Parameters.compositionKeys)) {
        Invoke-PimeKeySequence -Sequence $key
        Start-Sleep -Milliseconds 600
    }
    Start-Sleep -Seconds 1

    $root = [System.Windows.Automation.AutomationElement]::RootElement
    $condition = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $notepad.Id)
    $windows = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $condition)
    $windowNames = @()
    $itemNames = @()
    foreach ($window in $windows) {
        $windowNames += $window.Current.ClassName
        $items = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,
            [System.Windows.Automation.Condition]::TrueCondition)
        foreach ($item in $items) {
            $name = $item.Current.Name
            if ($name) { $itemNames += "$($item.Current.ControlType.ProgrammaticName)|$name" }
        }
    }
    $found = @()
    foreach ($pack in @($Parameters.packNames)) {
        $hit = @($itemNames | Where-Object { $_ -like "*$pack*" })
        if ($hit.Count) { $found += "$pack => $($hit -join '; ')" }
    }
    $Output = @{
        ok = $true
        windows = $windowNames
        itemCount = $itemNames.Count
        items = $itemNames
        packsFound = $found
    }
    Stop-Process -Id $notepad.Id -Force -ErrorAction SilentlyContinue
} catch {
    $Output = @{ ok = $false; error = $_.Exception.Message }
}
$Output | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
