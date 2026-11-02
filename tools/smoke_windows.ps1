<#
.SYNOPSIS
  GUI smoke test of the packaged RoomLab.exe using a fresh, throw-away data folder.
  Drives the real app through WebView2's DevTools protocol (Playwright connectOverCDP) and saves screenshots.
  Usage: powershell -File tools\smoke_windows.ps1 [-Exe release\RoomLab_Win64_Portable\RoomLab.exe]
#>
param([string]$Exe = "release\RoomLab_Win64_Portable\RoomLab.exe")
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$data = Join-Path $env:TEMP ("roomlab-smoke-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory $data | Out-Null
node tests\e2e\smoke.mjs --exe (Resolve-Path $Exe) --data $data --shots docs\screenshots
$code = $LASTEXITCODE
Write-Host "Smoke data folder (inspect or delete): $data"
exit $code
