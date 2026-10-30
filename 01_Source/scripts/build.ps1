$ErrorActionPreference='Stop'
$Repo=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $Repo
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'Rust cargo 未安装。请安装 Rust stable 后执行本脚本。'
}
Write-Host 'Building Rust input core...'
cargo build --workspace --release
Write-Host 'Running Rust unit tests...'
cargo test --workspace
if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    throw 'Node.js 未安装。需要 Node.js 20+ 运行 VS Code 逻辑测试。'
}
Push-Location (Join-Path $Repo 'editor-bridges/vscode')
try { npm test; npm run check } finally { Pop-Location }
Write-Host "核心可执行文件：$Repo\target\release\pime-core.exe"
Write-Host '可在 VS Code 开发扩展宿主中按 F5 运行 editor-bridges/vscode。'
Write-Host 'TSF 注册原型需单独 Visual Studio SDK 编译；本脚本不安装或注册输入法。'
