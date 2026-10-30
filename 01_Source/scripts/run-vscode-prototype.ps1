$ErrorActionPreference = 'Stop'
$Repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Ext = Join-Path $Repo 'editor-bridges\vscode'
if (-not (Get-Command code -ErrorAction SilentlyContinue)) {
    throw '找不到 code CLI。请在 VS Code 中直接打开 editor-bridges/vscode 文件夹后按 F5。'
}
& code --new-window --extensionDevelopmentPath="$Ext"
