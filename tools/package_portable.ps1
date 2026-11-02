<#
.SYNOPSIS
  Assemble release\RoomLab_Win64_Portable\ (section 16.2) from an existing release build and zip it.
  The frontend is embedded in RoomLab.exe; WebView2 is an OS prerequisite (preinstalled on Windows 11).
  No model weights or inference binaries are included.
#>
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
if (Test-Path "$env:USERPROFILE\.cargo\bin") { $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH" }

$exe = @("target\release\RoomLab.exe", "target\release\roomlab.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $exe) { throw "No release exe found. Run: npx tauri build --no-bundle" }

$out = Join-Path $root "release\RoomLab_Win64_Portable"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out, "$out\app", "$out\runtime\inference", "$out\docs", "$out\templates\commons", "$out\templates\guild", "$out\templates\story", "$out\templates\crucible" | Out-Null

Copy-Item $exe "$out\RoomLab.exe"
Copy-Item "templates\story\forgotten_keep" "$out\templates\story\" -Recurse
Copy-Item "templates\README.md" "$out\templates\README.md" -ErrorAction SilentlyContinue
foreach ($k in "commons", "guild", "crucible") { Copy-Item "templates\$k\README.md" "$out\templates\$k\README.md" -ErrorAction SilentlyContinue }
foreach ($d in "USER_GUIDE.md", "BUILD_AND_RUN.md", "TEST_REPORT.md", "KNOWN_LIMITATIONS.md", "RELEASE_NOTES.md", "SECURITY_BOUNDARIES.md") {
  if (Test-Path "docs\$d") { Copy-Item "docs\$d" "$out\docs\$d" }
}
Copy-Item "docs\README_FIRST_RUN.txt" "$out\README_FIRST_RUN.txt"
if (Test-Path "docs\screenshots") { Copy-Item "docs\screenshots" "$out\docs\screenshots" -Recurse }

Set-Content "$out\app\README.txt" "The RoomLab user interface is compiled into RoomLab.exe (Tauri). This folder is reserved for optional runtime assets; nothing else is required."
Set-Content "$out\runtime\inference\README.txt" @"
No inference engine is bundled in this release (Release A: Connected Portable).
Options:
  1. Connected: run LM Studio (Developer tab -> Start Server) or Ollama, then Settings -> Inference -> Test /models.
  2. Managed: put your own llama-server.exe (+ its DLLs) anywhere, then Settings -> Advanced -> Managed llama-server
     and select it plus a .gguf model file. RoomLab binds it to 127.0.0.1 and stops only that process on exit.
Model weights are never included or downloaded by RoomLab.
"@

# -- Third-party notices, generated from the locked dependency graph --
$meta = cargo metadata --format-version 1 --locked | ConvertFrom-Json
$lines = @("RoomLab third-party notices (generated $(Get-Date -Format s) from Cargo.lock and package-lock.json)", "", "== Rust crates in the locked dependency graph (some are platform-specific and not compiled on Windows) ==")
$meta.packages | Where-Object { $_.source } | Sort-Object name | ForEach-Object { $lines += ("{0} {1} - {2}" -f $_.name, $_.version, ($(if ($_.license) { $_.license } else { "see crate" }))) }
$lines += "", "== JavaScript packages bundled into the UI =="
foreach ($p in "react", "react-dom", "scheduler", "@tauri-apps/api", "@tauri-apps/plugin-dialog") {
  $pj = "node_modules\$p\package.json"
  if (Test-Path $pj) { $j = Get-Content $pj -Raw | ConvertFrom-Json; $lines += ("{0} {1} - {2}" -f $j.name, $j.version, $j.license) }
}
$lines += "", "SQLite is bundled via libsqlite3-sys (public domain).", "Microsoft Edge WebView2 Runtime is an OS component and is not redistributed here.", "No model weights or llama.cpp binaries are included."
Set-Content "$out\THIRD_PARTY_NOTICES.txt" ($lines -join "`r`n") -Encoding UTF8

$zip = Join-Path $root "release\RoomLab_Win64_Portable.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path $out -DestinationPath $zip
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash
Set-Content "$zip.sha256" "$hash  RoomLab_Win64_Portable.zip"
$size = [math]::Round((Get-Item $zip).Length / 1MB, 2)
Write-Host "Portable folder: $out"
Write-Host "Zip: $zip ($size MB) sha256=$hash"
