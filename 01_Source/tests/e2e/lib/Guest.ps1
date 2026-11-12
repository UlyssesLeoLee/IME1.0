# Transport to the Windows 11 guest: WinRM (PowerShell remoting) or ssh (OpenSSH).
# Sourced by Invoke-E2E.ps1; not a standalone script.
#
# Every guest script returns a JSON string on stdout. The host parses it; nothing else crosses the boundary, so the
# two transports behave the same.

function Get-E2ECredential {
    param([Parameter(Mandatory = $true)]$Config)
    if (-not $Config.PSObject.Properties.Name.Contains('password') -or -not $Config.password) {
        throw "the configuration carries no password (transport 'winrm' needs one)"
    }
    $secure = ConvertTo-SecureString -String $Config.password -AsPlainText -Force
    return New-Object System.Management.Automation.PSCredential($Config.username, $secure)
}

function New-E2EGuestScript {
    # Builds the text of a guest script. The body assigns its result to $Output.
    param([Parameter(Mandatory = $true)][string]$Body, [hashtable]$Parameters = @{})
    $json = $Parameters | ConvertTo-Json -Depth 12 -Compress
    $b64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($json))
    return (
        "`$ErrorActionPreference = 'Stop'`r`n" +
        "`$Parameters = ConvertFrom-Json ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$b64')))`r`n" +
        "`$Output = `$null`r`n" +
        "try {`r`n" + $Body + "`r`n" +
        "} catch {`r`n" +
        "  `$Output = @{ ok = `$false; error = `$_.Exception.Message; line = `$_.InvocationInfo.ScriptLineNumber }`r`n" +
        "}`r`n" +
        "if (`$Output -is [string]) { `$Output } else { `$Output | ConvertTo-Json -Depth 12 -Compress }`r`n"
    )
}

function Test-E2EGuest {
    # Fails loudly and clearly when the guest cannot be reached.
    param([Parameter(Mandatory = $true)]$Config)
    try {
        $probe = New-E2EGuestScript -Body "`$Output = @{ ok = `$true; host = `$env:COMPUTERNAME; os = [Environment]::OSVersion.VersionString }"
        $result = Invoke-E2EGuestScript -Config $Config -Text $probe -WhatIfOnly
        if ($result.ok) { return @{ reachable = $true; message = "$($result.host) ($($result.os))" } }
        return @{ reachable = $false; message = $result.error }
    } catch {
        return @{ reachable = $false; message = $_.Exception.Message }
    }
}

function Invoke-E2EGuestScript {
    # Runs a guest script and returns the parsed JSON object it produced.
    param(
        [Parameter(Mandatory = $true)]$Config,
        [Parameter(Mandatory = $true)][string]$Text,
        [switch]$WhatIfOnly
    )
    if ($WhatIfOnly) {
        # The reachability probe is the one call that must not throw when the guest is down.
        $raw = try { Invoke-E2EGuestRaw -Config $Config -Text $Text } catch { @{ error = $_.Exception.Message } }
        if ($raw.error) { return @{ ok = $false; error = $raw.error } }
        return (ConvertFrom-Json $raw.stdout)
    }
    $raw = Invoke-E2EGuestRaw -Config $Config -Text $Text
    if ($raw.error) { throw "the guest script failed: $($raw.error)`r`n$($raw.stderr)" }
    if (-not $raw.stdout.Trim()) { throw "the guest script produced no output`r`n$($raw.stderr)" }
    return (ConvertFrom-Json $raw.stdout)
}

function Invoke-E2EGuestRaw {
    param([Parameter(Mandatory = $true)]$Config, [Parameter(Mandatory = $true)][string]$Text)
    if ($Config.transport -eq 'winrm') {
        $params = @{
            ComputerName = $Config.host
            ScriptBlock = [scriptblock]::Create($Text)
            ErrorAction = 'Stop'
        }
        if ($Config.PSObject.Properties.Name.Contains('port') -and $Config.port) { $params.Port = $Config.port }
        if ($Config.PSObject.Properties.Name.Contains('useSsl') -and $Config.useSsl) {
            $params.UseSSL = $true
            $params.SessionOption = New-PSSessionOption -SkipCACheck -SkipCNCheck -OpenTimeout 30000 -OperationTimeout 300000
        }
        if ($Config.PSObject.Properties.Name.Contains('password') -and $Config.password) { $params.Credential = Get-E2ECredential -Config $Config }
        try {
            $out = Invoke-Command @params 2>&1
            $stdout = ($out | ForEach-Object { "$_" }) -join "`r`n"
            return @{ stdout = $stdout; stderr = ''; error = $null }
        } catch {
            return @{ stdout = ''; stderr = "$_"; error = $_.Exception.Message }
        }
    }
    # ssh
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($Text))
    $command = "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand $encoded"
    $args = @('-o', 'BatchMode=yes', '-o', 'ConnectTimeout=15')
    if ($Config.PSObject.Properties.Name.Contains('port') -and $Config.port) { $args += @('-p', "$($Config.port)") }
    $args += @("$($Config.username)@$($Config.host)", $command)
    $stdout = & ssh @args 2>&1
    $code = $LASTEXITCODE
    $text = ($stdout | ForEach-Object { "$_" }) -join "`r`n"
    if ($code -ne 0) { return @{ stdout = ''; stderr = $text; error = "ssh exited with $code" } }
    return @{ stdout = $text; stderr = ''; error = $null }
}

function Invoke-E2EGuestTask {
    # Runs a script in the guest through a scheduled task: 'Interactive' runs it in the session of the logged-on
    # user (the only way to reach the desktop), 'S4U' runs it elevated in session 0.
    param(
        [Parameter(Mandatory = $true)]$Config,
        [Parameter(Mandatory = $true)][string]$ScriptText,
        [hashtable]$Parameters = @{},
        [ValidateSet('Interactive', 'S4U')][string]$LogonType = 'S4U',
        [int]$TimeoutSeconds = 300
    )
    $work = $Config.workDirectory
    $name = 'pime-e2e-' + [Guid]::NewGuid().ToString('N').Substring(0, 12)
    $scriptPath = "$work\$name.ps1"
    $parametersPath = "$work\$name.params.json"
    $resultPath = "$work\$name.result.json"
    $scriptB64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($ScriptText))
    $parametersB64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes(($Parameters | ConvertTo-Json -Depth 12 -Compress)))

    $write = New-E2EGuestScript -Parameters @{ work = $work; scriptB64 = $scriptB64; parametersB64 = $parametersB64; scriptPath = $scriptPath; parametersPath = $parametersPath } -Body @'
if (-not (Test-Path -LiteralPath $Parameters.work)) { New-Item -ItemType Directory -Force -Path $Parameters.work | Out-Null }
[System.IO.File]::WriteAllBytes($Parameters.scriptPath, [Convert]::FromBase64String($Parameters.scriptB64))
[System.IO.File]::WriteAllBytes($Parameters.parametersPath, [Convert]::FromBase64String($Parameters.parametersB64))
$Output = @{ ok = $true; scriptPath = $Parameters.scriptPath; parametersPath = $Parameters.parametersPath }
'@
    $null = Invoke-E2EGuestScript -Config $Config -Text $write

    $helper = New-E2EGuestScript -Parameters @{
        taskName = $name; scriptPath = $scriptPath; parametersPath = $parametersPath; resultPath = $resultPath
        userId = $Config.interactiveUser; logonType = $LogonType; timeoutSeconds = $TimeoutSeconds
    } -Body (Get-E2ETaskHelperBody)
    $raw = Invoke-E2EGuestRaw -Config $Config -Text $helper
    if ($raw.error) { throw "the scheduled task could not be started: $($raw.error)`r`n$($raw.stderr)" }
    $outer = ConvertFrom-Json $raw.stdout
    if (-not $outer.ok) { throw "the scheduled task failed: $($outer.error)" }
    $inner = ConvertFrom-Json $outer.result
    return $inner
}

function Get-E2ETaskHelperBody {
    # The guest-side part of Invoke-E2EGuestTask (runs inside the guest).
    return @'
if (Test-Path -LiteralPath $Parameters.resultPath) { Remove-Item -LiteralPath $Parameters.resultPath -Force }
Unregister-ScheduledTask -TaskName $Parameters.taskName -Confirm:$false -ErrorAction SilentlyContinue
$argument = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{0}" -ParametersPath "{1}" -ResultPath "{2}"' -f $Parameters.scriptPath, $Parameters.parametersPath, $Parameters.resultPath
$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $argument
$principal = New-ScheduledTaskPrincipal -UserId $Parameters.userId -LogonType $Parameters.logonType -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Hours 1) -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName $Parameters.taskName -Action $action -Principal $principal -Settings $settings -Force | Out-Null
try {
  Start-ScheduledTask -TaskName $Parameters.taskName
  $deadline = (Get-Date).AddSeconds($Parameters.timeoutSeconds)
  $found = $false
  while ((Get-Date) -lt $deadline) {
    if (Test-Path -LiteralPath $Parameters.resultPath) { $found = $true; break }
    Start-Sleep -Milliseconds 500
  }
  if (-not $found) {
    $info = Get-ScheduledTaskInfo -TaskName $Parameters.taskName -ErrorAction SilentlyContinue
    $state = (Get-ScheduledTask -TaskName $Parameters.taskName -ErrorAction SilentlyContinue).State
    $Output = @{ ok = $false; error = "the task produced no result within $($Parameters.timeoutSeconds) s (state: $state, last result: $($info.LastTaskResult))" }
  } else {
    $raw = Get-Content -LiteralPath $Parameters.resultPath -Raw -Encoding UTF8
    $Output = @{ ok = $true; result = $raw }
  }
} finally {
  Unregister-ScheduledTask -TaskName $Parameters.taskName -Confirm:$false -ErrorAction SilentlyContinue
}
'@
}

function Restart-E2EGuest {
    # Restarts the guest and waits until it answers again.
    param([Parameter(Mandatory = $true)]$Config)
    $command = if ($Config.transport -eq 'winrm') { 'Restart-Computer -Force' } else { 'shutdown /r /t 0 /f' }
    $body = if ($Config.transport -eq 'winrm') { "Restart-Computer -Force`r`n`$Output = @{ ok = `$true; restarting = `$true }" } else { "`$Output = @{ ok = `$true; restarting = `$true }" }
    try {
        $null = Invoke-E2EGuestScript -Config $Config -Text (New-E2EGuestScript -Body $body)
    } catch {
        Write-E2ELog "the restart command did not report back (that is normal): $($_.Exception.Message)" -Level 'WARN'
    }
    $timeout = if ($Config.restart.PSObject.Properties.Name.Contains('timeoutSeconds')) { $Config.restart.timeoutSeconds } else { 300 }
    $deadline = (Get-Date).AddSeconds($timeout)
    Start-Sleep -Seconds 10
    while ((Get-Date) -lt $deadline) {
        $state = Test-E2EGuest -Config $Config
        if ($state.reachable) { return @{ ok = $true; message = $state.message } }
        Start-Sleep -Seconds 5
    }
    return @{ ok = $false; message = "the guest did not come back within $timeout s" }
}
