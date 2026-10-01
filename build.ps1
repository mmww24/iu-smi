$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$Cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $Cargo) {
    throw "cargo was not found. Install Rust from https://rustup.rs and re-run .\build.ps1"
}

& $Cargo.Source build --manifest-path (Join-Path $Root "Cargo.toml") --release
if ($LASTEXITCODE -ne 0) {
    throw "cargo build failed with exit code $LASTEXITCODE"
}

$Built = Join-Path $Root "target\release\iu-smi.exe"
$Out = Join-Path $Root "iu-smi.exe"
Copy-Item -LiteralPath $Built -Destination $Out -Force
Write-Host "Built $Out"
