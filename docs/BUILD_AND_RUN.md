# Build and run (Windows 11 x64)

## Developer prerequisites (end users need none of these)

| Tool | Version used for this build | Notes |
|---|---|---|
| Rust (rustup, `stable-x86_64-pc-windows-msvc`) | rustc 1.99.1, cargo 1.99.1 | https://rustup.rs |
| MSVC + Windows SDK | Visual Studio 2022 / 2026 Community, SDK 10.0.26100 | "Desktop development with C++" |
| Node.js / npm | 26.13.0 / 11.20.0 | Node 20+ should work |
| WebView2 Runtime | 156.0.4314.58 | Preinstalled on Windows 11 |

Dependencies are locked: `Cargo.lock` and `package-lock.json` are committed. Use `npm ci`.

## One-command build

```powershell
powershell -ExecutionPolicy Bypass -File tools\build_windows.ps1
```

This runs: `npm ci` → `cargo test -p roomlab-core` → `npx vitest run` → schema generation →
`npx tauri build --no-bundle` → `tools\package_portable.ps1`.

Output:
- `target\release\RoomLab.exe` (frontend embedded; ~17 MB)
- `release\RoomLab_Win64_Portable\` and `release\RoomLab_Win64_Portable.zip` (+ `.sha256`)

## Individual steps

```powershell
npm ci
cargo test -p roomlab-core                    # core + integration tests (stub provider, no network)
npx vitest run                                # frontend unit tests (rich-text safety, Unicode)
npx tsc -p tsconfig.json --noEmit             # typecheck
cargo run -p roomlab-core --bin roomlab-schemas -- contracts
npx tauri build --no-bundle                   # release exe
powershell -File tools\package_portable.ps1   # portable folder + zip
```

Development mode with hot reload: `npx tauri dev`.

## GUI tests against the real executable

```powershell
powershell -File tools\smoke_windows.ps1 -Exe target\release\RoomLab.exe
```

`tests/e2e/smoke.mjs` launches `RoomLab.exe` with a throw-away data folder and
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333`, attaches Playwright over CDP,
drives the actual UI and saves screenshots to `docs/screenshots/`.

Real-model GUI test (requires a running LM Studio/Ollama):

```powershell
node tests\e2e\real_model.mjs --exe target\release\RoomLab.exe --model <model-id> --allowance 1200
```

## Real-inference tests (opt-in; never faked)

```powershell
# T21: real model through the full agent pipeline
$env:ROOMLAB_REAL_BASE_URL="http://127.0.0.1:1234/v1"; $env:ROOMLAB_REAL_MODEL="<model-id>"; $env:ROOMLAB_REAL_REASONING_ALLOWANCE="1200"
cargo test -p roomlab-core --test g3_provider_gateway t21 -- --ignored --nocapture

# T57/T62: managed llama-server (your own binary + GGUF)
$env:ROOMLAB_LLAMA_EXE="C:\path\llama-server.exe"; $env:ROOMLAB_GGUF="C:\path\model.gguf"
cargo test -p roomlab-core --test g3_provider_gateway t57_t62 -- --ignored --nocapture
```

## Benchmark (T64)

```powershell
cargo build --release -p roomlab-core --bin roomlab-bench
target\release\roomlab-bench --base-url http://127.0.0.1:1234/v1 --model <id> --agents 1,3,5 --repeats 3 --reasoning-allowance 1200 --out docs\BENCHMARK.md
```

Reports only measured calls (p50/p95 with n). Exits with `BLOCKED` if the endpoint is unreachable.

## Data location

Portable by default: `data\`, `exports\`, `logs\` next to `RoomLab.exe`. If that folder is not
writable, RoomLab uses `%LOCALAPPDATA%\RoomLab` and shows a notice. Override with
`ROOMLAB_DATA_DIR=<folder>`.
