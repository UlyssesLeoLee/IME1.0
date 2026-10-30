$ErrorActionPreference='Stop'
$Repo=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $Repo
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Rust cargo is not installed. Install Rust stable and run this script again.' }
if (-not (Get-Command node -ErrorAction SilentlyContinue)) { throw 'Node.js (22.22+, 24.15+ or 26) is required for the tests.' }
Write-Host 'Building the Rust core (zero dependencies)...'
cargo build --workspace --release
Write-Host 'Running the stored tests (tests/run-all.js, no GUI)...'
node tests/run-all.js
Write-Host "Core executable: $Repo\target\release\pime-core.exe  (set programmingIme.engine / programmingIme.coreExecutable to use it)"
Write-Host 'UAT (real VS Code): node tests/run-all.js --uat   |   with the Rust engine: node tests/run-all.js --uat-rust'
Write-Host 'The TSF adapter needs a separate Visual Studio / Windows SDK build; this script neither builds nor registers it.'
