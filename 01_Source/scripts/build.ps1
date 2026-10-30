$ErrorActionPreference='Stop'
$Repo=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $Repo
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Rust cargo is not installed. Install Rust stable and run this script again.' }
if (-not (Get-Command node -ErrorAction SilentlyContinue)) { throw 'Node.js 20+ is required for the JavaScript tests.' }
Write-Host 'Building the Rust core (zero dependencies)...'
cargo build --workspace --release
Write-Host 'Rust tests...'
cargo test --workspace
Push-Location (Join-Path $Repo 'editor-bridges/vscode')
try { npm test; npm run check } finally { Pop-Location }
node (Join-Path $Repo 'tests/gen-fixtures.js') --check
Write-Host "Core executable: $Repo\target\release\pime-core.exe  (set programmingIme.coreExecutable to use it)"
Write-Host 'UAT: cd uat; npm install; npx playwright test'
Write-Host 'The TSF adapter needs a separate Visual Studio / Windows SDK build; this script neither builds nor registers it.'
