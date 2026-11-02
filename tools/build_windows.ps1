<#
.SYNOPSIS
  Reproducible Windows build of RoomLab: tests -> frontend -> Tauri release exe -> portable folder + zip.
.DESCRIPTION
  Developer prerequisites (NOT needed by end users):
    - Rust stable (rustup, x86_64-pc-windows-msvc)  https://rustup.rs
    - Visual Studio 2022 Build Tools with "Desktop development with C++" (MSVC + Windows SDK)
    - Node.js 20+ with npm
    - WebView2 runtime (preinstalled on Windows 11)
  Usage:  powershell -ExecutionPolicy Bypass -File tools\build_windows.ps1 [-SkipTests]
#>
param([switch]$SkipTests)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
if (Test-Path "$env:USERPROFILE\.cargo\bin") { $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH" }

foreach ($c in "cargo", "rustc", "node", "npm") {
  if (-not (Get-Command $c -ErrorAction SilentlyContinue)) { throw "Missing prerequisite: $c (see header of this script)" }
}
Write-Host "rustc: $(rustc --version)"; Write-Host "node:  $(node --version)"

Write-Host "== npm ci (locked) =="
npm ci --no-audit --no-fund
if ($LASTEXITCODE) { throw "npm ci failed" }

if (-not $SkipTests) {
  Write-Host "== Rust tests (roomlab-core) =="
  cargo test -p roomlab-core
  if ($LASTEXITCODE) { throw "cargo test failed" }
  Write-Host "== Frontend tests =="
  npx vitest run
  if ($LASTEXITCODE) { throw "vitest failed" }
}

Write-Host "== Regenerate JSON Schemas from Rust contracts =="
cargo run -q -p roomlab-core --bin roomlab-schemas -- contracts
if ($LASTEXITCODE) { throw "schema generation failed" }

Write-Host "== Tauri release build (no installer; portable exe) =="
npx tauri build --no-bundle
if ($LASTEXITCODE) { throw "tauri build failed" }

& "$PSScriptRoot\package_portable.ps1"
